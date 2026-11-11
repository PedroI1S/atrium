//! The socket: roles, limits, lanes and subscriptions, against a real
//! daemon in a temporary home.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use atrium_daemon::AppIdentitySource;
use atrium_protocol::PROTOCOL_VERSION;
use atrium_protocol::agent::AgentEvent;
use atrium_protocol::limits::{HELLO_TIMEOUT, MAX_CLIENT_FRAME_LEN, MAX_CONNECTIONS};
use atrium_protocol::thread::{ThreadId, ThreadState};
use atrium_protocol::wire::{
    ClientMessage, ClientRole, DaemonCommand, ErrorCode, EventId, ServerMessage,
};
use common::{Rig, Wire, native, soon};

fn incompatible(first: Option<ServerMessage>) -> String {
    match first {
        Some(ServerMessage::Incompatible {
            message,
            daemon_pid,
            protocol_version,
            ..
        }) => {
            assert_eq!(daemon_pid, std::process::id());
            assert_eq!(protocol_version, PROTOCOL_VERSION);
            message
        }
        other => panic!("expected Incompatible, got {other:?}"),
    }
}

#[tokio::test]
async fn a_role_is_granted_or_the_connection_is_refused() {
    let mut rig = Rig::start().await;

    let (_dev, first) = Wire::hello(rig.socket(), ClientRole::Dev).await;
    match first {
        Some(ServerMessage::Welcome {
            protocol_version,
            daemon_pid,
            role,
            ..
        }) => {
            assert_eq!(protocol_version, PROTOCOL_VERSION);
            assert_eq!(daemon_pid, std::process::id());
            assert_eq!(role, ClientRole::Dev);
        }
        other => panic!("{other:?}"),
    }

    // Never a lesser role: refused, and closed.
    let agent = ClientRole::Agent {
        token: "t".into(),
    };
    let (mut wire, first) = Wire::hello(rig.socket(), agent).await;
    assert_eq!(incompatible(first), "unverified client");
    assert!(wire.closed().await);

    // No app identity is known to this daemon.
    let (mut wire, first) = Wire::hello(rig.socket(), ClientRole::App).await;
    assert_eq!(incompatible(first), "unverified client");
    assert!(wire.closed().await);

    // Another protocol version.
    let mut wire = Wire::connect(rig.socket()).await;
    wire.send(&ClientMessage::Hello {
        protocol_version: PROTOCOL_VERSION + 1,
        client_name: "newer".into(),
        role: ClientRole::Dev,
    })
    .await;
    assert_eq!(
        incompatible(wire.read().await),
        "protocol version mismatch"
    );
    assert!(wire.closed().await);

    // Anything before `Hello` closes the connection, unanswered.
    let mut wire = Wire::connect(rig.socket()).await;
    wire.send(&ClientMessage::SubscribeInbox { request_id: 1 })
        .await;
    assert!(wire.read().await.is_none());

    // And so does a second `Hello`.
    let mut wire = rig.dev().await;
    wire.send(&ClientMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
        client_name: "again".into(),
        role: ClientRole::Dev,
    })
    .await;
    assert!(wire.closed().await);

    // Without `--dev-socket`, `Dev` is refused; the app is let in once the
    // daemon knows its identity and the peer is that very process.
    rig.stop(true).await;
    let me = atrium_host::peer::current_app_identity().expect("this binary's identity");
    rig.boot(|options| {
        options.dev_socket = false;
        options.app_identity = AppIdentitySource::Fixed(me);
    })
    .await;
    let (mut wire, first) = Wire::hello(rig.socket(), ClientRole::Dev).await;
    assert_eq!(incompatible(first), "unverified client");
    assert!(wire.closed().await);
    let (mut app, first) = Wire::hello(rig.socket(), ClientRole::App).await;
    assert!(
        matches!(
            first,
            Some(ServerMessage::Welcome {
                role: ClientRole::App,
                ..
            })
        ),
        "{first:?}"
    );
    assert!(app.ok(DaemonCommand::ListThreads).await.is_array());
    rig.stop(true).await;
}

#[tokio::test]
async fn the_app_identity_file_must_be_private() {
    use std::os::unix::fs::PermissionsExt;

    let mut rig = Rig::in_folder(common::folder());
    let file = rig.paths.app_identity.clone();
    rig.boot(|options| {
        options.dev_socket = false;
        options.app_identity = AppIdentitySource::File(file.clone());
    })
    .await;
    let me = atrium_host::peer::current_app_identity().expect("this binary's identity");
    let refused = async |socket| {
        let (_, first) = Wire::hello(socket, ClientRole::App).await;
        assert_eq!(incompatible(first), "unverified client");
    };

    // No file yet.
    refused(rig.socket()).await;
    // Readable by others.
    std::fs::write(&file, serde_json::to_vec(&me).expect("json")).expect("writes");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    refused(rig.socket()).await;
    // A symlink to a private file.
    let real = rig.folder.path().join("identity.json");
    std::fs::rename(&file, &real).expect("moves");
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    std::os::unix::fs::symlink(&real, &file).expect("links");
    refused(rig.socket()).await;
    // Private, and read again at each `Hello`.
    std::fs::remove_file(&file).expect("removes");
    std::fs::rename(&real, &file).expect("moves");
    let (_, first) = Wire::hello(rig.socket(), ClientRole::App).await;
    assert!(matches!(first, Some(ServerMessage::Welcome { .. })), "{first:?}");
    rig.stop(true).await;
}

#[tokio::test]
async fn a_silent_connection_is_closed_at_the_hello_deadline() {
    let mut rig = Rig::start().await;
    let began = Instant::now();
    let mut wire = Wire::connect(rig.socket()).await;
    assert!(wire.read().await.is_none(), "closed without a word");
    let waited = began.elapsed();
    assert!(waited >= HELLO_TIMEOUT - Duration::from_millis(200), "{waited:?}");
    assert!(waited < HELLO_TIMEOUT + Duration::from_secs(5), "{waited:?}");
    rig.stop(true).await;
}

#[tokio::test]
async fn a_frame_that_is_not_one_closes_the_connection() {
    let mut rig = Rig::start().await;

    // Over `MAX_CLIENT_FRAME_LEN`: closed on the length alone.
    let mut wire = rig.dev().await;
    let len = MAX_CLIENT_FRAME_LEN + 1;
    wire.send_raw(0, &vec![b' '; len as usize - 1]).await;
    assert!(wire.closed().await);

    // An unknown kind, a stream frame, JSON that isn't a message.
    for (kind, payload) in [
        (9u8, &b"{}"[..]),
        (1, &[0, 0, 0, 1, b'x'][..]),
        (0, &b"{\"type\":\"nope\"}"[..]),
        (0, &b"not json"[..]),
    ] {
        let mut wire = rig.dev().await;
        wire.send_raw(kind, payload).await;
        assert!(wire.closed().await, "kind {kind}");
    }

    // The largest frame allowed is read (and refused as a message).
    let mut wire = rig.dev().await;
    wire.send_raw(0, &vec![b' '; MAX_CLIENT_FRAME_LEN as usize - 1])
        .await;
    assert!(wire.closed().await);

    // The daemon is none the worse.
    let mut wire = rig.dev().await;
    assert!(wire.ok(DaemonCommand::ListThreads).await.is_array());
    rig.stop(true).await;
}

#[tokio::test]
async fn connections_are_limited() {
    let mut rig = Rig::start().await;
    // Greeted or not, a connection takes a place.
    let mut held = Vec::new();
    for n in 0..MAX_CONNECTIONS {
        held.push(if n % 2 == 0 {
            rig.dev().await
        } else {
            Wire::connect(rig.socket()).await
        });
    }
    let (mut over, first) = Wire::hello(rig.socket(), ClientRole::Dev).await;
    assert_eq!(incompatible(first), "too many clients");
    assert!(over.closed().await);

    // A place that is given back can be taken again.
    held.truncate(MAX_CONNECTIONS - 2);
    soon(async {
        loop {
            let (_, first) = Wire::hello(rig.socket(), ClientRole::Dev).await;
            if matches!(first, Some(ServerMessage::Welcome { .. })) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    rig.stop(true).await;
}

#[tokio::test]
async fn a_subscribe_is_refused_for_an_unknown_thread_or_a_cursor_past_the_end() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;
    let nobody = ThreadId("th-nobody".into());
    assert_eq!(
        wire.subscribe(&nobody, None).await.expect_err("unknown").0,
        ErrorCode::UnknownThread
    );

    let thread = wire.start_thread(native(), Some("hello")).await.thread_id;
    let mut script = rig.script().await;
    // Nothing stored yet: any cursor is past the end.
    assert_eq!(
        wire.subscribe(&thread, Some(EventId(1))).await.expect_err("empty").0,
        ErrorCode::Invalid
    );
    script.started("native-1").await;
    script.turn(1).await;
    wire.thread_when(&thread, |summary| summary.state == ThreadState::Running)
        .await;
    let history = wire.history(&thread).await;
    let last = history.last().expect("events").id;
    assert_eq!(
        wire.subscribe(&thread, Some(EventId(last.0 + 1)))
            .await
            .expect_err("beyond")
            .0,
        ErrorCode::Invalid
    );
    // At the end: nothing to replay.
    assert_eq!(wire.subscribed(&thread, Some(last)).await, Some(last));
    assert_eq!(
        wire.next().await,
        ServerMessage::ReplayDone {
            thread_id: thread.clone(),
            last: Some(last)
        }
    );
    // Then live.
    script.delta("m", "x").await;
    match wire.next().await {
        ServerMessage::Event { event } => assert!(event.id > last),
        other => panic!("{other:?}"),
    }
    rig.stop(true).await;
}

/// Reads one subscription to its `TurnCompleted`: the ids before
/// `ReplayDone`, and the ids after.
async fn follow(wire: &mut Wire, thread: &ThreadId, after: Option<EventId>) -> Vec<EventId> {
    let replay_until = wire.subscribed(thread, after).await;
    let mut ids = Vec::new();
    let mut replayed = None;
    let mut turn_ended = false;
    while !(turn_ended && replayed.is_some()) {
        match wire.next().await {
            ServerMessage::Event { event } => {
                assert_eq!(event.thread_id, *thread);
                ids.push(event.id);
                turn_ended |= matches!(event.envelope.event, AgentEvent::TurnCompleted { .. });
            }
            ServerMessage::ReplayDone { thread_id, last } => {
                assert_eq!(thread_id, *thread);
                assert_eq!(last, replay_until, "ReplayDone ends where announced");
                assert!(replayed.is_none(), "ReplayDone comes once");
                replayed = Some(ids.len());
            }
            other => panic!("{other:?}"),
        }
    }
    let replayed = replayed.expect("ReplayDone came");
    // Everything up to `replay_until` came before it, and nothing after it.
    assert_eq!(ids[..replayed].last().copied().or(after), replay_until.or(after));
    assert!(ids[replayed..].iter().all(|id| Some(*id) > replay_until));
    ids
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replay_then_live_delivers_every_event_once_and_in_order() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;
    let thread = wire.start_thread(native(), Some("go")).await.thread_id;
    let mut script = rig.script().await;
    script.started("native-1").await;
    script.turn(1).await;
    for n in 0..2_000 {
        script.delta("m", &format!("{n} ")).await;
    }
    // A cursor in the middle of what is stored so far.
    let middle = {
        wire.thread_when(&thread, |summary| summary.state == ThreadState::Running)
            .await;
        let so_far = wire.history(&thread).await;
        so_far[so_far.len() / 2].id
    };

    // The session keeps producing while subscribers arrive.
    let writing = tokio::spawn(async move {
        for n in 2_000..30_000 {
            script.delta("m", &format!("{n} ")).await;
            if n % 500 == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }
        script.turn_done(1).await;
        script
    });
    let mut following = Vec::new();
    for n in 0..8u64 {
        let socket = rig.socket().to_path_buf();
        let thread = thread.clone();
        let after = (n % 2 == 1).then_some(middle);
        following.push(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(n * 7)).await;
            let mut wire = Wire::dev(&socket).await;
            (after, follow(&mut wire, &thread, after).await)
        }));
    }
    let _script = soon(writing).await.expect("the script ran");
    wire.thread_when(&thread, |summary| summary.state == ThreadState::Idle)
        .await;

    let all: Vec<EventId> = wire
        .history(&thread)
        .await
        .iter()
        .map(|event| event.id)
        .collect();
    assert!(all.len() > 30_000);
    assert!(all.windows(2).all(|pair| pair[0] < pair[1]));
    for subscriber in following {
        let (after, ids) = soon(subscriber).await.expect("the subscriber ran");
        let expected: Vec<EventId> = all
            .iter()
            .copied()
            .filter(|id| Some(*id) > after)
            .collect();
        assert_eq!(ids.len(), expected.len(), "after {after:?}");
        assert!(ids == expected, "every event once, in order (after {after:?})");
    }
    rig.stop(true).await;
}

#[tokio::test]
async fn nothing_of_a_replaced_subscription_follows_the_new_answer() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;
    let thread = wire.start_thread(native(), Some("go")).await.thread_id;
    let mut script = rig.script().await;
    script.started("native-1").await;
    script.turn(1).await;
    // Several megabytes: far more than a socket holds, so the first replay
    // is still queued when the second subscribe arrives.
    let chunk = "x".repeat(2_000);
    for _ in 0..3_000 {
        script.delta("m", &chunk).await;
    }
    script.turn_done(1).await;
    wire.thread_when(&thread, |summary| summary.state == ThreadState::Idle)
        .await;
    let all: Vec<EventId> = wire
        .history(&thread)
        .await
        .iter()
        .map(|event| event.id)
        .collect();
    let middle = all[all.len() / 2];

    let mut slow = rig.dev().await;
    let first = slow.request_id();
    slow.send(&ClientMessage::Subscribe {
        request_id: first,
        thread_id: thread.clone(),
        after: None,
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let second = slow.request_id();
    slow.send(&ClientMessage::Subscribe {
        request_id: second,
        thread_id: thread.clone(),
        after: Some(middle),
    })
    .await;

    // Up to the second answer: the first answer and some of its replay.
    let mut old = Vec::new();
    loop {
        match slow.read().await.expect("open") {
            ServerMessage::Response { request_id, .. } if request_id == first => {}
            ServerMessage::Response { request_id, .. } if request_id == second => break,
            ServerMessage::Event { event } => old.push(event.id),
            other => panic!("{other:?}"),
        }
    }
    assert!(old.len() < all.len(), "the first replay was cut short");
    assert!(old == all[..old.len()]);

    // After it: the new replay only, from its cursor, then its ReplayDone.
    let mut new = Vec::new();
    loop {
        match slow.read().await.expect("open") {
            ServerMessage::Event { event } => new.push(event.id),
            ServerMessage::ReplayDone { last, .. } => {
                assert_eq!(last, all.last().copied());
                break;
            }
            other => panic!("{other:?}"),
        }
    }
    let expected: Vec<EventId> = all.iter().copied().filter(|id| *id > middle).collect();
    assert!(new == expected, "exactly the events after the new cursor");

    // An `Unsubscribe` ends it too: once a later command is answered,
    // nothing more of the thread arrives.
    slow.send(&ClientMessage::Unsubscribe {
        thread_id: thread.clone(),
    })
    .await;
    assert!(slow.ok(DaemonCommand::ListThreads).await.is_array());
    slow.aside.clear();
    script.turn(2).await;
    script.turn_done(2).await;
    wire.thread_when(&thread, |summary| summary.last_event > all.last().copied())
        .await;
    assert!(slow.ok(DaemonCommand::ListThreads).await.is_array());
    assert!(slow.aside.is_empty(), "{:?}", slow.aside);
    rig.stop(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reader_that_stops_lags_alone() {
    const LANE: usize = 1024 * 1024;
    let mut rig = Rig::start_with(|options| options.connection_queue_bytes = LANE).await;
    let mut wire = rig.dev().await;
    let thread = wire.start_thread(native(), Some("go")).await.thread_id;
    let mut script = rig.script().await;
    script.started("native-1").await;
    script.turn(1).await;

    wire.thread_when(&thread, |summary| summary.state == ThreadState::Running)
        .await;

    // Both are live before anything large is stored.
    let mut stalled = rig.dev().await;
    let mut steady = rig.dev().await;
    for wire in [&mut stalled, &mut steady] {
        let replay_until = wire.subscribed(&thread, None).await;
        loop {
            match wire.next().await {
                ServerMessage::ReplayDone { last, .. } => {
                    assert_eq!(last, replay_until);
                    break;
                }
                ServerMessage::Event { .. } => {}
                other => panic!("{other:?}"),
            }
        }
    }

    // `steady` reads as fast as it is sent to.
    let seen = Arc::new(AtomicUsize::new(0));
    let reading = tokio::spawn({
        let seen = seen.clone();
        async move {
            let mut ids = Vec::new();
            loop {
                match steady.next().await {
                    ServerMessage::Event { event } => {
                        ids.push(event.id);
                        seen.fetch_add(1, Ordering::Relaxed);
                        if matches!(event.envelope.event, AgentEvent::TurnCompleted { .. }) {
                            return ids;
                        }
                    }
                    other => panic!("the steady reader got {other:?}"),
                }
            }
        }
    });

    // 12 MiB in bursts no larger than a quarter of the lane, each sent
    // once `steady` has taken the one before. `stalled` reads nothing.
    let chunk = "y".repeat(4_096);
    let (bursts, per_burst) = (48usize, 64usize);
    for burst in 0..bursts {
        for _ in 0..per_burst {
            script.delta("m", &chunk).await;
        }
        let target = (burst + 1) * per_burst;
        common::until(|| seen.load(Ordering::Relaxed) >= target).await;
    }
    script.turn_done(1).await;
    let steady_ids = soon(reading).await.expect("the steady reader ran");
    wire.thread_when(&thread, |summary| summary.state == ThreadState::Idle)
        .await;

    let all: Vec<EventId> = wire
        .history(&thread)
        .await
        .iter()
        .map(|event| event.id)
        .collect();
    let live_from = all.len() - steady_ids.len();
    assert!(steady_ids == all[live_from..], "the steady reader missed nothing");
    assert_eq!(steady_ids.len(), bursts * per_burst + 1);

    // The stalled one still gets its command answered, and is told it
    // lagged; what it was sent before is in order, with no gap.
    let answer = stalled.ok(DaemonCommand::GetThread {
        thread_id: thread.clone(),
    });
    let answer = soon(answer).await;
    assert_eq!(answer["thread_id"], thread.0);
    let mut sent = Vec::new();
    let mut lagged = None;
    for message in stalled.aside.drain(..) {
        match message {
            ServerMessage::Event { event } => {
                assert!(lagged.is_none(), "nothing of it after SubscriptionLagged");
                sent.push(event.id);
            }
            ServerMessage::SubscriptionLagged {
                thread_id,
                last_sent,
            } => {
                assert_eq!(thread_id, thread);
                lagged = Some(last_sent);
            }
            other => panic!("{other:?}"),
        }
    }
    let last_sent = lagged.expect("it was told");
    assert!(sent.len() < steady_ids.len(), "it was not sent everything");
    assert!(sent == all[live_from..live_from + sent.len()]);
    assert_eq!(last_sent, sent.last().copied());

    // Subscribing again from there gives it the rest.
    let rest = follow(&mut stalled, &thread, last_sent).await;
    assert!(rest == all[live_from + sent.len()..]);
    rig.stop(true).await;
}
