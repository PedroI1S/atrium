//! M1-06: the client against a scripted daemon on a real unix socket.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use atrium_client::{
    Client, ClientOptions, CommandError, ConnectionState, Delivery, StartDaemon, Window,
};
use atrium_protocol::PROTOCOL_VERSION;
use atrium_protocol::agent::{AgentEvent, AgentEventEnvelope};
use atrium_protocol::limits::MAX_CLIENT_FRAME_LEN;
use atrium_protocol::thread::ThreadId;
use atrium_protocol::wire::{
    ClientMessage, ClientRole, CommandResult, DaemonCommand, ErrorCode, EventId, ServerMessage,
    StoredEvent,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

/// Nothing here should take long; a test that waits this long has failed.
async fn soon<T>(what: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), what)
        .await
        .expect("in time")
}

/// A private folder for a socket, with a path short enough for one.
fn folder() -> tempfile::TempDir {
    let folder = tempfile::Builder::new()
        .prefix("atrium-client-")
        .tempdir_in("/tmp")
        .expect("tempdir");
    // The daemon's runtime folder is 0700; a temporary one follows the umask.
    std::fs::set_permissions(folder.path(), std::fs::Permissions::from_mode(0o700)).expect("chmod");
    folder
}

fn options(socket: PathBuf) -> ClientOptions {
    ClientOptions {
        socket,
        name: "test".into(),
        role: ClientRole::Dev,
        start_daemon: None,
    }
}

/// The daemon's side of one connection.
struct Peer {
    stream: UnixStream,
}

impl Peer {
    /// `None` when the client closed the connection.
    async fn try_read(&mut self) -> Option<ClientMessage> {
        let len = soon(self.stream.read_u32()).await.ok()?;
        assert!(len <= MAX_CLIENT_FRAME_LEN, "a client frame over the limit");
        assert_eq!(
            soon(self.stream.read_u8()).await.ok()?,
            0,
            "a control frame"
        );
        let mut payload = vec![0; len as usize - 1];
        soon(self.stream.read_exact(&mut payload)).await.ok()?;
        Some(serde_json::from_slice(&payload).expect("a client message"))
    }

    async fn read(&mut self) -> ClientMessage {
        self.try_read().await.expect("the client is connected")
    }

    async fn send_bytes(&mut self, kind: u8, payload: &[u8]) {
        let mut frame = (payload.len() as u32 + 1).to_be_bytes().to_vec();
        frame.push(kind);
        frame.extend_from_slice(payload);
        soon(self.stream.write_all(&frame)).await.expect("writes");
    }

    async fn send(&mut self, message: &ServerMessage) {
        self.send_bytes(0, &serde_json::to_vec(message).expect("json"))
            .await;
    }

    async fn ok(&mut self, request_id: u64, value: Value) {
        self.send(&ServerMessage::Response {
            request_id,
            result: CommandResult::Ok { value },
        })
        .await;
    }

    /// Reads `Hello`, welcomes the client and answers its inbox
    /// subscription.
    async fn welcome(&mut self) {
        assert_eq!(
            self.read().await,
            ClientMessage::Hello {
                protocol_version: PROTOCOL_VERSION,
                client_name: "test".into(),
                role: ClientRole::Dev,
            }
        );
        self.send(&ServerMessage::Welcome {
            protocol_version: PROTOCOL_VERSION,
            daemon_version: "0.1.0".into(),
            daemon_pid: 42,
            role: ClientRole::Dev,
        })
        .await;
        let ClientMessage::SubscribeInbox { request_id } = self.read().await else {
            panic!("the inbox subscription comes first");
        };
        self.ok(request_id, json!({ "threads": [], "inbox": [] }))
            .await;
    }
}

struct Daemon {
    _folder: tempfile::TempDir,
    socket: PathBuf,
    listener: UnixListener,
}

impl Daemon {
    fn new() -> Self {
        let folder = folder();
        let socket = folder.path().join("atriumd.sock");
        let listener = UnixListener::bind(&socket).expect("binds");
        Self {
            _folder: folder,
            socket,
            listener,
        }
    }

    async fn accept(&self) -> Peer {
        let (stream, _) = soon(self.listener.accept()).await.expect("accepts");
        Peer { stream }
    }

    /// Whether a connection arrives within `wait`.
    async fn connection_within(&self, wait: Duration) -> bool {
        tokio::time::timeout(wait, self.listener.accept())
            .await
            .is_ok()
    }
}

async fn state_is(client: &Client, wanted: impl Fn(&ConnectionState) -> bool) -> ConnectionState {
    let mut connection = client.connection();
    soon(connection.wait_for(wanted))
        .await
        .expect("the client is running")
        .clone()
}

async fn connected(client: &Client) {
    state_is(client, |s| matches!(s, ConnectionState::Connected { .. })).await;
}

/// The window's next delivery, acknowledged.
async fn next(window: &mut Window) -> Delivery {
    let outgoing = soon(window.recv()).await.expect("the window is open");
    window.handle().ack(outgoing.seq);
    outgoing.delivery().expect("a delivery")
}

fn event(id: u64, text: &str) -> StoredEvent {
    StoredEvent {
        id: EventId(id),
        thread_id: ThreadId("t".into()),
        envelope: AgentEventEnvelope {
            incarnation: 1,
            seq: id - 1,
            at_ms: 1_000 + id,
            event: AgentEvent::MessageDelta {
                message_id: "m1".into(),
                kind: atrium_protocol::agent::MessageKind::Assistant,
                text: text.into(),
            },
        },
    }
}

#[tokio::test]
async fn it_shakes_hands_and_runs_a_command() {
    let daemon = Daemon::new();
    let client = Client::start(options(daemon.socket.clone()));
    assert_eq!(
        *client.connection().borrow(),
        ConnectionState::Connecting { attempt: 1 }
    );
    let mut peer = daemon.accept().await;
    peer.welcome().await;
    assert_eq!(
        state_is(&client, |s| matches!(s, ConnectionState::Connected { .. })).await,
        ConnectionState::Connected {
            daemon_version: "0.1.0".into(),
            daemon_pid: 42
        }
    );

    let command = tokio::spawn({
        let client = client.clone();
        async move { client.command(DaemonCommand::ListThreads).await }
    });
    let ClientMessage::Command {
        request_id,
        command: DaemonCommand::ListThreads,
    } = peer.read().await
    else {
        panic!("the command");
    };
    peer.ok(request_id, json!(["a thread"])).await;
    assert_eq!(soon(command).await.expect("ran"), Ok(json!(["a thread"])));

    // Refused by the daemon.
    let command = tokio::spawn({
        let client = client.clone();
        async move { client.command(DaemonCommand::ListInbox).await }
    });
    let ClientMessage::Command { request_id, .. } = peer.read().await else {
        panic!("the command");
    };
    peer.send(&ServerMessage::Response {
        request_id,
        result: CommandResult::Err {
            code: ErrorCode::Forbidden,
            message: "not for you".into(),
        },
    })
    .await;
    assert_eq!(
        soon(command).await.expect("ran"),
        Err(CommandError::Rejected {
            code: ErrorCode::Forbidden,
            message: "not for you".into()
        })
    );

    // Too large to send: refused here, and the connection stays up.
    let huge = DaemonCommand::RenameThread {
        thread_id: ThreadId("t".into()),
        title: "x".repeat(MAX_CLIENT_FRAME_LEN as usize),
    };
    assert!(matches!(
        client.command(huge).await,
        Err(CommandError::TooLarge { .. })
    ));
    let command = tokio::spawn({
        let client = client.clone();
        async move { client.command(DaemonCommand::GetSettings).await }
    });
    assert!(matches!(
        peer.read().await,
        ClientMessage::Command {
            command: DaemonCommand::GetSettings,
            ..
        }
    ));
    command.abort();
}

#[tokio::test]
async fn an_incompatible_daemon_is_not_tried_again_until_asked() {
    let daemon = Daemon::new();
    let client = Client::start(options(daemon.socket.clone()));
    let mut peer = daemon.accept().await;
    peer.read().await;
    peer.send(&ServerMessage::Incompatible {
        protocol_version: 0,
        message: "the daemon is older".into(),
        daemon_version: "0.0.9".into(),
        daemon_pid: 41,
        running_turns: 2,
    })
    .await;
    drop(peer);
    assert_eq!(
        state_is(&client, |s| matches!(
            s,
            ConnectionState::Incompatible { .. }
        ))
        .await,
        ConnectionState::Incompatible {
            protocol_version: 0,
            message: "the daemon is older".into(),
            daemon_version: "0.0.9".into(),
            daemon_pid: 41,
            running_turns: 2,
        }
    );
    assert_eq!(
        client.command(DaemonCommand::ListThreads).await,
        Err(CommandError::NotConnected)
    );
    assert!(
        !daemon.connection_within(Duration::from_millis(300)).await,
        "it waits to be asked"
    );

    // The app replaced the daemon and asks again.
    client.reconnect();
    let mut peer = daemon.accept().await;
    peer.welcome().await;
    connected(&client).await;
}

#[tokio::test]
async fn a_welcome_for_another_version_or_role_fails() {
    for (protocol_version, role) in [
        (PROTOCOL_VERSION + 1, ClientRole::Dev),
        (PROTOCOL_VERSION, ClientRole::App),
    ] {
        let daemon = Daemon::new();
        let client = Client::start(options(daemon.socket.clone()));
        let mut peer = daemon.accept().await;
        peer.read().await;
        peer.send(&ServerMessage::Welcome {
            protocol_version,
            daemon_version: "0.1.0".into(),
            daemon_pid: 42,
            role,
        })
        .await;
        let state = state_is(&client, |s| matches!(s, ConnectionState::Failed { .. })).await;
        assert!(
            matches!(&state, ConnectionState::Failed { message } if message.contains("another protocol version or role")),
            "{state:?}"
        );
    }
}

#[tokio::test]
async fn a_dropped_connection_comes_back_and_continues_from_the_cursor() {
    let daemon = Daemon::new();
    let client = Client::start(options(daemon.socket.clone()));
    let mut peer = daemon.accept().await;
    peer.welcome().await;

    let mut window = client.open_window("main");
    window.handle().view(ThreadId("t".into()));
    let ClientMessage::Subscribe {
        request_id,
        thread_id,
        after: None,
    } = peer.read().await
    else {
        panic!("a subscription from the start");
    };
    assert_eq!(thread_id, ThreadId("t".into()));
    peer.ok(request_id, json!({ "replay_until": 2 })).await;
    for (id, text) in [(1, "Hel"), (2, "lo")] {
        peer.send(&ServerMessage::Event {
            event: event(id, text),
        })
        .await;
    }
    peer.send(&ServerMessage::ReplayDone {
        thread_id: ThreadId("t".into()),
        last: Some(EventId(2)),
    })
    .await;
    // The connection state, then the snapshot.
    let snapshot = loop {
        match next(&mut window).await {
            Delivery::Connection { .. } => {}
            other => break other,
        }
    };
    assert_eq!(
        snapshot,
        Delivery::Snapshot {
            thread_id: ThreadId("t".into()),
            events: vec![event(1, "Hello")],
            has_older: false,
            truncated: false,
            last: Some(EventId(2)),
        }
    );
    assert!(
        client
            .cached_bytes(&ThreadId("t".into()))
            .is_some_and(|bytes| bytes > 0)
    );

    // A command is in flight when the daemon goes away.
    let command = tokio::spawn({
        let client = client.clone();
        async move { client.command(DaemonCommand::ListThreads).await }
    });
    assert!(matches!(peer.read().await, ClientMessage::Command { .. }));
    let dropped = Instant::now();
    drop(peer);
    assert_eq!(
        soon(command).await.expect("ran"),
        Err(CommandError::UnknownOutcome)
    );

    // It reconnects on its own, at once, and asks for what it is missing.
    let mut peer = daemon.accept().await;
    let reconnected = dropped.elapsed();
    assert!(reconnected < Duration::from_secs(1), "{reconnected:?}");
    peer.welcome().await;
    let ClientMessage::Subscribe {
        request_id,
        after: Some(EventId(2)),
        ..
    } = peer.read().await
    else {
        panic!("a subscription from the cursor");
    };
    peer.ok(request_id, json!({ "replay_until": 3 })).await;
    peer.send(&ServerMessage::Event {
        event: event(3, " again"),
    })
    .await;
    peer.send(&ServerMessage::ReplayDone {
        thread_id: ThreadId("t".into()),
        last: Some(EventId(3)),
    })
    .await;
    // The window sees the connection change and then just the new event.
    let live = loop {
        match next(&mut window).await {
            Delivery::Connection { .. } => {}
            other => break other,
        }
    };
    assert_eq!(
        live,
        Delivery::Event {
            event: event(3, " again")
        }
    );
    connected(&client).await;
}

#[tokio::test]
async fn a_daemon_that_welcomes_and_drops_is_tried_with_backoff() {
    let daemon = Daemon::new();
    let client = Client::start(options(daemon.socket.clone()));
    // Each connection is welcomed and closed at once. The first drop is
    // followed by an attempt without a wait, the later ones by waits of
    // 100 ms, 200 ms, 400 ms, 800 ms: connections at about 0, 0, 100, 300,
    // 700 and 1500 ms.
    let started = Instant::now();
    let mut connections = 0;
    while started.elapsed() < Duration::from_millis(1_000) {
        if let Ok(Ok((stream, _))) =
            tokio::time::timeout(Duration::from_millis(50), daemon.listener.accept()).await
        {
            Peer { stream }.welcome().await;
            connections += 1;
        }
    }
    assert!(
        (4..=6).contains(&connections),
        "{connections} connections in a second"
    );
    // Asking for a reconnect clears the record: the next attempt is at once.
    client.reconnect();
    assert!(daemon.connection_within(Duration::from_millis(100)).await);
}

#[tokio::test]
async fn nothing_listening_calls_the_start_hook_once_and_connects_when_it_can() {
    let folder = folder();
    let socket = folder.path().join("atriumd.sock");
    let calls = Arc::new(AtomicU32::new(0));
    let (started, mut starts) = tokio::sync::mpsc::unbounded_channel();
    let start: StartDaemon = Arc::new({
        let calls = calls.clone();
        move || {
            calls.fetch_add(1, Ordering::SeqCst);
            let _ = started.send(());
            Ok(())
        }
    });
    let client = Client::start(ClientOptions {
        start_daemon: Some(start),
        ..options(socket.clone())
    });
    soon(starts.recv()).await.expect("the hook ran");
    // A few failed attempts later the daemon is up.
    state_is(
        &client,
        |s| matches!(s, ConnectionState::Connecting { attempt } if *attempt >= 3),
    )
    .await;
    let listener = UnixListener::bind(&socket).expect("binds");
    let (stream, _) = soon(listener.accept()).await.expect("accepts");
    let mut peer = Peer { stream };
    peer.welcome().await;
    connected(&client).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // A stale socket file (the daemon died) is "nothing listening" too.
    drop(peer);
    drop(listener);
    soon(starts.recv()).await.expect("the hook ran again");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn an_unsafe_socket_is_never_connected_to_and_starts_nothing() {
    let calls = Arc::new(AtomicU32::new(0));
    let start: StartDaemon = Arc::new({
        let calls = calls.clone();
        move || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    });

    // The folder is open to other users.
    let daemon = Daemon::new();
    std::fs::set_permissions(
        daemon.socket.parent().expect("a folder"),
        std::fs::Permissions::from_mode(0o755),
    )
    .expect("chmod");
    let client = Client::start(ClientOptions {
        start_daemon: Some(start.clone()),
        ..options(daemon.socket.clone())
    });
    let state = state_is(&client, |s| matches!(s, ConnectionState::Failed { .. })).await;
    assert!(
        matches!(&state, ConnectionState::Failed { message } if message.contains("open to other users")),
        "{state:?}"
    );
    assert!(!daemon.connection_within(Duration::from_millis(200)).await);

    // The folder is a symlink.
    let real = Daemon::new();
    let links = folder();
    let link = links.path().join("runtime");
    std::os::unix::fs::symlink(real.socket.parent().expect("a folder"), &link).expect("symlinks");
    let client = Client::start(ClientOptions {
        start_daemon: Some(start.clone()),
        ..options(link.join("atriumd.sock"))
    });
    let state = state_is(&client, |s| matches!(s, ConnectionState::Failed { .. })).await;
    assert!(
        matches!(&state, ConnectionState::Failed { message } if message.contains("symlink")),
        "{state:?}"
    );

    // The socket is an ordinary file.
    let plain = folder();
    let socket = plain.path().join("atriumd.sock");
    std::fs::write(&socket, b"").expect("writes");
    let client = Client::start(ClientOptions {
        start_daemon: Some(start),
        ..options(socket)
    });
    let state = state_is(&client, |s| matches!(s, ConnectionState::Failed { .. })).await;
    assert!(
        matches!(&state, ConnectionState::Failed { message } if message.contains("not a socket")),
        "{state:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_daemon_that_breaks_the_protocol_fails_the_connection_until_asked() {
    let bad: [(u8, &[u8], &str); 4] = [
        (9, b"{}", "invalid frame"),
        (1, b"\0\0\0\x01x", "invalid frame"),
        (0, b"{\"type\":\"nonsense\"}", "can't read"),
        (
            0,
            br#"{"type":"response","request_id":99,"result":{"status":"ok","value":null}}"#,
            "broke the protocol",
        ),
    ];
    for (kind, payload, why) in bad {
        let daemon = Daemon::new();
        let client = Client::start(options(daemon.socket.clone()));
        let mut peer = daemon.accept().await;
        peer.welcome().await;
        connected(&client).await;
        peer.send_bytes(kind, payload).await;
        let state = state_is(&client, |s| matches!(s, ConnectionState::Failed { .. })).await;
        assert!(
            matches!(&state, ConnectionState::Failed { message } if message.contains(why)),
            "{state:?}"
        );
        // The client hung up, and doesn't come back by itself.
        assert!(peer.try_read().await.is_none());
        assert!(!daemon.connection_within(Duration::from_millis(200)).await);
        client.reconnect();
        let mut peer = daemon.accept().await;
        peer.welcome().await;
        connected(&client).await;
    }
}

#[tokio::test]
async fn shutting_down_ends_the_windows_and_the_connection() {
    let daemon = Daemon::new();
    let client = Client::start(options(daemon.socket.clone()));
    let mut peer = daemon.accept().await;
    peer.welcome().await;
    connected(&client).await;
    let mut window = client.open_window("main");
    assert!(matches!(
        next(&mut window).await,
        Delivery::Connection {
            state: ConnectionState::Connected { .. }
        }
    ));
    assert!(client.window("main").is_some());
    assert!(client.window("other").is_none());

    client.shutdown();
    assert_eq!(
        next(&mut window).await,
        Delivery::Connection {
            state: ConnectionState::Closed
        }
    );
    assert!(soon(window.recv()).await.is_none());
    assert!(peer.try_read().await.is_none(), "the socket closed");
    assert_eq!(*client.connection().borrow(), ConnectionState::Closed);
    assert_eq!(
        client.command(DaemonCommand::ListThreads).await,
        Err(CommandError::NotConnected)
    );

    // Dropping every `Client` does the same.
    let daemon = Daemon::new();
    let client = Client::start(options(daemon.socket.clone()));
    let mut peer = daemon.accept().await;
    peer.welcome().await;
    connected(&client).await;
    let mut window = client.open_window("main");
    drop(client);
    while let Some(outgoing) = soon(window.recv()).await {
        window.handle().ack(outgoing.seq);
    }
    assert!(peer.try_read().await.is_none());
}

#[tokio::test]
async fn a_window_that_never_acknowledges_does_not_delay_the_other() {
    let daemon = Daemon::new();
    let client = Client::start(options(daemon.socket.clone()));
    let mut peer = daemon.accept().await;
    peer.welcome().await;
    let mut main = client.open_window("main");
    let mut hung = client.open_window("hung");
    main.handle().view(ThreadId("t".into()));
    hung.handle().view(ThreadId("t".into()));
    let ClientMessage::Subscribe { request_id, .. } = peer.read().await else {
        panic!("one subscription for both");
    };
    peer.ok(request_id, json!({ "replay_until": null })).await;
    peer.send(&ServerMessage::ReplayDone {
        thread_id: ThreadId("t".into()),
        last: None,
    })
    .await;

    // 40 MiB of deltas, more than twice a window's queue. The hung window's
    // bridge takes them and its webview never acknowledges.
    let chunk = "x".repeat(1024 * 1024);
    let feed = tokio::spawn(async move {
        for id in 1..=40 {
            peer.send(&ServerMessage::Event {
                event: event(id, &chunk),
            })
            .await;
        }
        peer
    });
    let taking = tokio::spawn(async move {
        let mut taken = 0;
        while let Ok(Some(_)) = tokio::time::timeout(Duration::from_secs(2), hung.recv()).await {
            taken += 1;
        }
        (hung, taken)
    });
    let mut seen = 0;
    while seen < 40 {
        if let Delivery::Event { event } = next(&mut main).await {
            seen += 1;
            assert_eq!(event.id, EventId(seen));
        }
    }
    let mut peer = soon(feed).await.expect("fed");
    let (mut hung, taken) = soon(taking).await.expect("took");
    assert!(taken < 40, "it was cut off after about 16 MiB, at {taken}");
    assert_eq!(hung.handle().resyncs(), 1);
    assert!(hung.handle().needs_resync());
    assert_eq!(main.handle().resyncs(), 0);

    // A command isn't behind any of that.
    let command = tokio::spawn({
        let client = client.clone();
        async move { client.command(DaemonCommand::ListThreads).await }
    });
    let request_id = loop {
        if let ClientMessage::Command { request_id, .. } = peer.read().await {
            break request_id;
        }
    };
    peer.ok(request_id, json!([])).await;
    assert_eq!(soon(command).await.expect("ran"), Ok(json!([])));

    // When the webview comes back and acknowledges, it gets the present.
    hung.handle().ack(taken);
    let snapshot = loop {
        match next(&mut hung).await {
            Delivery::Connection { .. } => {}
            other => break other,
        }
    };
    let Delivery::Snapshot { events, last, .. } = snapshot else {
        panic!("a snapshot, not the missed events");
    };
    assert_eq!(last, Some(EventId(40)));
    assert_eq!(events.len(), 1);
}

/// Contract §14 gates reconnects against the real daemon (cursor 1,000
/// events behind, p95 of 20 under 100 ms). This is the client's share of
/// that, against a daemon that answers at once; the bound here is loose
/// because tests run unoptimized.
#[tokio::test]
async fn catching_up_a_thousand_events_after_a_reconnect_is_quick() {
    let daemon = Daemon::new();
    let client = Client::start(options(daemon.socket.clone()));
    let mut peer = daemon.accept().await;
    peer.welcome().await;
    let mut window = client.open_window("main");
    window.handle().view(ThreadId("t".into()));
    let ClientMessage::Subscribe { request_id, .. } = peer.read().await else {
        panic!("a subscription");
    };
    peer.ok(request_id, json!({ "replay_until": null })).await;
    peer.send(&ServerMessage::ReplayDone {
        thread_id: ThreadId("t".into()),
        last: None,
    })
    .await;

    let mut cursor = 0;
    let mut times = Vec::new();
    for _ in 0..20 {
        // Connections that drop this soon after each other are spaced
        // (a daemon that keeps dropping); asking clears that, and what is
        // measured is a reconnect without a wait.
        client.reconnect();
        let dropped = Instant::now();
        drop(peer);
        peer = daemon.accept().await;
        peer.welcome().await;
        let ClientMessage::Subscribe {
            request_id, after, ..
        } = peer.read().await
        else {
            panic!("a subscription");
        };
        assert_eq!(after.map(|id| id.0).unwrap_or(0), cursor);
        let until = cursor + 1_000;
        peer.ok(request_id, json!({ "replay_until": until })).await;
        for id in cursor + 1..=until {
            peer.send(&ServerMessage::Event {
                event: event(id, "token "),
            })
            .await;
        }
        peer.send(&ServerMessage::ReplayDone {
            thread_id: ThreadId("t".into()),
            last: Some(EventId(until)),
        })
        .await;
        loop {
            if let Delivery::Event { event } = next(&mut window).await
                && event.id == EventId(until)
            {
                break;
            }
        }
        times.push(dropped.elapsed());
        cursor = until;
    }
    times.sort();
    let (median, p95) = (times[10], times[18]);
    println!("reconnect and 1,000 events: median {median:?}, p95 {p95:?}");
    assert!(p95 < Duration::from_secs(1), "{times:?}");
}
