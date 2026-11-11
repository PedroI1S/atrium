//! `atrium-client` (M1-06) against the real daemon: handshake, inbox,
//! viewing a thread, reconnecting from the cursor.

mod common;

use std::time::Duration;

use atrium_client::{Client, ClientOptions, ConnectionState, Delivery, Window};
use atrium_protocol::agent::AgentEvent;
use atrium_protocol::thread::{ThreadId, ThreadState, ThreadSummary};
use atrium_protocol::wire::{ClientRole, DaemonCommand, EventId};
use common::{Rig, native, soon};

/// The window's next delivery, acknowledged.
async fn next(window: &mut Window) -> Delivery {
    let outgoing = soon(window.recv()).await.expect("the window is open");
    window.handle().ack(outgoing.seq);
    outgoing.delivery().expect("a delivery")
}

/// Reads deliveries until one satisfies `wanted`; returns those before it
/// and it.
async fn until_delivery(
    window: &mut Window,
    wanted: impl Fn(&Delivery) -> bool,
) -> (Vec<Delivery>, Delivery) {
    let mut before = Vec::new();
    loop {
        let delivery = next(window).await;
        if wanted(&delivery) {
            return (before, delivery);
        }
        before.push(delivery);
    }
}

async fn connected(client: &Client) -> ConnectionState {
    let mut connection = client.connection();
    soon(connection.wait_for(|state| matches!(state, ConnectionState::Connected { .. })))
        .await
        .expect("the client is running")
        .clone()
}

async fn thread_when(
    client: &Client,
    thread: &ThreadId,
    check: impl Fn(&ThreadSummary) -> bool,
) -> ThreadSummary {
    soon(async {
        loop {
            let got = client
                .command(DaemonCommand::GetThread {
                    thread_id: thread.clone(),
                })
                .await;
            if let Ok(value) = got {
                let summary: ThreadSummary = serde_json::from_value(value).expect("a summary");
                if check(&summary) {
                    return summary;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
}

fn event_ids(deliveries: &[Delivery]) -> Vec<EventId> {
    deliveries
        .iter()
        .filter_map(|delivery| match delivery {
            Delivery::Event { event } => Some(event.id),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_client_and_the_daemon_work_together() {
    let mut rig = Rig::start().await;
    let client = Client::start(ClientOptions {
        socket: rig.socket().to_path_buf(),
        name: "test".into(),
        role: ClientRole::Dev,
        start_daemon: None,
    });

    // The handshake.
    let ConnectionState::Connected { daemon_pid, .. } = connected(&client).await else {
        unreachable!()
    };
    assert_eq!(daemon_pid, std::process::id());

    // The inbox: the current set, then changes.
    let mut window = client.open_window("main");
    let handle = window.handle();
    handle.show_inbox(true);
    let (_, inbox) = until_delivery(&mut window, |delivery| {
        matches!(delivery, Delivery::Inbox { .. })
    })
    .await;
    assert_eq!(
        inbox,
        Delivery::Inbox {
            threads: Vec::new(),
            inbox: Vec::new()
        }
    );
    let started = client
        .command(DaemonCommand::StartThread {
            agent: native(),
            message: Some("Tidy the desktop".into()),
        })
        .await
        .expect("starts");
    let thread = ThreadId(started["thread_id"].as_str().expect("an id").to_owned());
    let (_, updated) = until_delivery(&mut window, |delivery| {
        matches!(delivery, Delivery::ThreadUpdated { summary } if summary.thread_id == thread)
    })
    .await;
    let Delivery::ThreadUpdated { summary } = updated else {
        unreachable!()
    };
    assert_eq!(summary.title, "Tidy the desktop");

    let mut script = rig.script().await;
    script.command().await;
    script.started("native-1").await;
    script.turn(1).await;
    for n in 0..300 {
        script.delta("m", &format!("word{n} ")).await;
    }
    let stored = thread_when(&client, &thread, |summary| {
        summary.state == ThreadState::Running && summary.unread
    })
    .await;

    // Viewing the thread: what is stored so far, folded, then live events.
    handle.view(thread.clone());
    let (_, snapshot) = until_delivery(
        &mut window,
        |delivery| matches!(delivery, Delivery::Snapshot { thread_id, .. } if *thread_id == thread),
    )
    .await;
    let Delivery::Snapshot {
        events,
        last,
        has_older,
        truncated,
        ..
    } = snapshot
    else {
        unreachable!()
    };
    assert!(!has_older && !truncated);
    assert!(last.is_some());
    assert!(last <= stored.last_event.max(last));
    // The 300 deltas are one folded delta with all of the text.
    let text: String = events
        .iter()
        .filter_map(|event| match &event.envelope.event {
            AgentEvent::MessageDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(text.starts_with("word0 word1 "), "{text}");
    script.delta("m", "live ").await;
    script
        .completed(
            "m",
            &{
                let mut deltas: Vec<String> = (0..300).map(|n| format!("word{n} ")).collect();
                deltas.push("live ".into());
                deltas
            }
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        )
        .await;
    script.turn_done(1).await;
    let (before, _) = until_delivery(&mut window, |delivery| {
        matches!(
            delivery,
            Delivery::Event { event }
                if matches!(event.envelope.event, AgentEvent::TurnCompleted { .. })
        )
    })
    .await;
    let mut seen = event_ids(&before);
    assert!(seen.iter().all(|id| Some(*id) > last), "only what is new");
    assert!(seen.windows(2).all(|pair| pair[0] < pair[1]));

    // The daemon goes away and comes back: the client reconnects by itself
    // and continues from its cursor.
    script.ended(Some(0)).await;
    let (before, ended) = until_delivery(&mut window, |delivery| {
        matches!(
            delivery,
            Delivery::Event { event }
                if matches!(event.envelope.event, AgentEvent::SessionEnded { .. })
        )
    })
    .await;
    seen.extend(event_ids(&before));
    seen.extend(event_ids(&[ended]));
    let cursor = *seen.last().expect("events");
    rig.stop(true).await;
    let mut connection = client.connection();
    soon(connection.wait_for(|state| !matches!(state, ConnectionState::Connected { .. })))
        .await
        .expect("the client is running");
    rig.boot(|_| {}).await;
    connected(&client).await;

    let resumed = soon(async {
        loop {
            // A command sent while the client is still connecting fails.
            let resumed = client
                .command(DaemonCommand::ResumeThread {
                    thread_id: thread.clone(),
                })
                .await;
            if let Ok(resumed) = resumed {
                return resumed;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert_eq!(resumed["incarnation"], 2);
    let mut script = rig.script().await;
    script.started("native-1").await;
    script.turn(1).await;
    script.turn_done(1).await;
    let (before, done) = until_delivery(&mut window, |delivery| {
        matches!(
            delivery,
            Delivery::Event { event }
                if event.envelope.incarnation == 2
                    && matches!(event.envelope.event, AgentEvent::TurnCompleted { .. })
        )
    })
    .await;
    // The view was not reloaded, and nothing was delivered twice.
    assert!(
        !before
            .iter()
            .any(|delivery| matches!(delivery, Delivery::Snapshot { .. })),
        "{before:?}"
    );
    let mut after = event_ids(&before);
    after.extend(event_ids(&[done]));
    assert_eq!(after.len(), 3, "the new incarnation's three events");
    assert!(after.iter().all(|id| *id > cursor));
    assert!(after.windows(2).all(|pair| pair[0] < pair[1]));
    client.shutdown();
}
