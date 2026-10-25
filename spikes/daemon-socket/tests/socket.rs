use spike_daemon_socket::{
    Client,
    agent::*,
    codec::{self, Frame},
    daemon::{self, Identity},
    store::{self, Store},
    wire::*,
};
use std::{
    collections::BTreeSet,
    fs, io,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};
use tokio::{
    net::{UnixListener, UnixStream},
    time::timeout,
};
fn temp() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("a5-")
        .tempdir_in("/tmp")
        .inspect(|d| {
            fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
        })
        .unwrap()
}
fn envelope(seq: u64) -> AgentEventEnvelope {
    AgentEventEnvelope {
        seq,
        at_ms: 0,
        event: AgentEvent::MessageDelta {
            message_id: "m".into(),
            kind: MessageKind::Assistant,
            text: "x".repeat(200),
        },
    }
}
async fn seed(dir: &std::path::Path, n: u64) -> Store {
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    let s = Store::open(&dir.join("events.sqlite")).unwrap();
    s.call(|c| {
        c.execute("INSERT INTO threads(id,running) VALUES('t',0)", [])
            .map_err(io::Error::other)?;
        Ok(())
    })
    .await
    .unwrap();
    s.append(ThreadId("t".into()), (0..n).map(envelope).collect())
        .await
        .unwrap();
    s
}
struct Daemon {
    child: Child,
    dir: PathBuf,
}
impl Daemon {
    async fn start(dir: &std::path::Path) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_spike-daemon"))
            .arg("--dir")
            .arg(dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut d = Self {
            child,
            dir: dir.into(),
        };
        // The child announces readiness only after recovery and socket publication.
        use std::io::BufRead;
        let mut ready = String::new();
        std::io::BufReader::new(d.child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "READY", "daemon exited before ready");
        d
    }
    async fn client(&self) -> Client {
        Client::connect(self.dir.join("daemon.sock"), "test")
            .await
            .unwrap()
    }
    fn crash(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
async fn until_done(c: &mut Client) -> Vec<EventId> {
    timeout(Duration::from_secs(20), async {
        let mut ids = vec![];
        loop {
            match c.next().await.unwrap() {
                ServerMessage::Event { event } => ids.push(event.id),
                ServerMessage::ReplayDone { .. } => break,
                _ => (),
            }
        }
        ids
    })
    .await
    .unwrap()
}
async fn start_fake(c: &mut Client, duration_ms: u64) -> ThreadId {
    let r = c
        .command(DaemonCommand::StartThread {
            agent: AgentLaunch::Fake {
                tokens_per_second: 10_000,
                chunk_chars: 200,
                duration_ms,
                tool_call_every_ms: 20,
            },
            cwd: "/tmp".into(),
        })
        .await
        .unwrap();
    let CommandResult::Ok { value } = r else {
        panic!("{r:?}")
    };
    ThreadId(value["thread_id"].as_str().unwrap().into())
}
#[tokio::test]
async fn codec_roundtrip_and_rejects_bad_frames() {
    let (mut a, mut b) = tokio::io::duplex(4096);
    let control = Frame::Control(ClientMessage::Hello {
        protocol_version: 1,
        client_name: "t".into(),
    });
    codec::write(&mut a, &control).await.unwrap();
    assert_eq!(
        codec::read::<_, ClientMessage>(&mut b).await.unwrap(),
        control
    );
    let binary = Frame::<ClientMessage>::Stream {
        stream_id: 42,
        bytes: vec![0, 255, 4],
    };
    codec::write(&mut a, &binary).await.unwrap();
    assert_eq!(
        codec::read::<_, ClientMessage>(&mut b).await.unwrap(),
        binary
    );
    for bytes in [
        vec![1, 0, 0, 1],
        vec![0, 0, 0, 1, 99],
        vec![0, 0, 0, 2, 0, b'{'],
        vec![0, 0, 0, 0],
        vec![0, 0, 0, 2, 1, 0],
    ] {
        let mut input = bytes.as_slice();
        assert!(codec::read::<_, ClientMessage>(&mut input).await.is_err());
    }
}
#[tokio::test]
async fn security_and_single_instance() {
    let dir = temp();
    let path = dir.path().join("private");
    let guard = daemon::SocketGuard::acquire(&path).unwrap();
    assert!(
        daemon::SocketGuard::acquire(&path)
            .unwrap_err_string()
            .contains("another daemon")
    );
    let meta = fs::symlink_metadata(&path).unwrap();
    assert!(daemon::verify_dir_metadata(&meta, nix::unistd::geteuid().as_raw() + 1).is_err());
    let link = dir.path().join("link");
    symlink(&path, &link).unwrap();
    assert!(daemon::SocketGuard::acquire(&link).is_err());
    drop(guard);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(daemon::SocketGuard::acquire(&path).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    let d = Daemon::start(&path).await;
    assert_eq!(
        fs::metadata(path.join("daemon.sock"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let output = Command::new(env!("CARGO_BIN_EXE_spike-daemon"))
        .arg("--dir")
        .arg(&path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("another daemon"));
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        fs::metadata(path.join("daemon.lock"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let mut survivor = d.client().await;
    assert!(matches!(
        survivor.command(DaemonCommand::ListThreads).await.unwrap(),
        CommandResult::Ok { .. }
    ));
    drop(d);
}
trait ErrString {
    fn unwrap_err_string(self) -> String;
}
impl<T> ErrString for io::Result<T> {
    fn unwrap_err_string(self) -> String {
        match self {
            Ok(_) => panic!("expected error"),
            Err(e) => e.to_string(),
        }
    }
}
#[tokio::test]
async fn subscribe_errors_and_replacement() {
    let dir = temp();
    let _store = seed(dir.path(), 300).await;
    let d = Daemon::start(dir.path()).await;
    let mut c = d.client().await;
    assert!(matches!(
        c.subscribe(ThreadId("missing".into()), None).await.unwrap(),
        CommandResult::Err { .. }
    ));
    assert!(matches!(
        c.subscribe(ThreadId("t".into()), Some(EventId(u64::MAX)))
            .await
            .unwrap(),
        CommandResult::Err { .. }
    ));
    c.subscribe(ThreadId("t".into()), None).await.unwrap();
    assert_eq!(until_done(&mut c).await.len(), 300);
    c.subscribe(ThreadId("t".into()), Some(EventId(298)))
        .await
        .unwrap();
    assert_eq!(until_done(&mut c).await, vec![EventId(299), EventId(300)]);
    c.subscribe(ThreadId("t".into()), Some(EventId(300)))
        .await
        .unwrap();
    assert!(until_done(&mut c).await.is_empty());
}
#[tokio::test]
async fn replay_live_boundary_current_thread() {
    replay_boundary().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replay_live_boundary_multi_thread() {
    replay_boundary().await;
}
async fn replay_boundary() {
    let dir = temp();
    let store = seed(dir.path(), 1024).await;
    let (entered, mut at_boundary) = tokio::sync::mpsc::channel(1);
    let release = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
    let gate = daemon::ReplayGate {
        entered,
        release: release.clone(),
    };
    let (ready, wait_ready) = tokio::sync::oneshot::channel();
    let path = dir.path().to_owned();
    let writer = store.clone();
    let serving = tokio::spawn(async move {
        daemon::run_with_replay_gate(
            &path,
            PathBuf::from(env!("CARGO_BIN_EXE_spike-fake-agent")),
            writer,
            gate,
            ready,
        )
        .await
        .unwrap()
    });
    wait_ready.await.unwrap();
    for round in 0..25 {
        let mut c = Client::connect(dir.path().join("daemon.sock"), "boundary")
            .await
            .unwrap();
        let base = 1024 + round * 64;
        let result = c
            .subscribe(ThreadId("t".into()), Some(EventId(base - 512)))
            .await
            .unwrap();
        let CommandResult::Ok { value } = result else {
            panic!()
        };
        assert_eq!(value["replay_until"].as_u64(), Some(base));
        assert_eq!(at_boundary.recv().await.unwrap(), ThreadId("t".into()));
        // The daemon is blocked after responding and before querying page one.
        store
            .append(
                ThreadId("t".into()),
                (base..base + 64).map(envelope).collect(),
            )
            .await
            .unwrap();
        assert_eq!(
            store::last(
                &store::open_reader(&store.path).unwrap(),
                &ThreadId("t".into())
            )
            .unwrap(),
            Some(EventId(base + 64))
        );
        release.add_permits(1);
        let mut next = base - 512 + 1;
        let mut done = false;
        timeout(Duration::from_secs(10), async {
            while next <= base + 64 || !done {
                match c.next().await.unwrap() {
                    ServerMessage::Event { event } => {
                        assert_eq!(event.id.0, next);
                        assert_eq!(event.id.0 > base, done);
                        c.applied(&event.thread_id, event.id);
                        next += 1;
                    }
                    ServerMessage::ReplayDone { last, .. } => {
                        assert!(!done);
                        assert_eq!(last, Some(EventId(base)));
                        assert_eq!(next, base + 1);
                        done = true;
                    }
                    other => panic!("{other:?}"),
                }
            }
        })
        .await
        .unwrap();
    }
    serving.abort();
    let _ = serving.await;
}
#[tokio::test]
async fn reconnect_replays_read_but_unapplied_after_daemon_crash() {
    let dir = temp();
    let _store = seed(dir.path(), 1000).await;
    let mut d = Daemon::start(dir.path()).await;
    let mut c = d.client().await;
    let thread = ThreadId("t".into());
    c.subscribe(thread.clone(), None).await.unwrap();
    let mut applied = BTreeSet::new();
    for _ in 0..10 {
        let ServerMessage::Event { event } = c.next().await.unwrap() else {
            panic!()
        };
        assert!(applied.insert(event.id));
        c.applied(&thread, event.id);
    }
    let ServerMessage::Event { event } = c.next().await.unwrap() else {
        panic!()
    };
    assert_eq!(event.id, EventId(11));
    d.crash();
    let _restart = Daemon::start(dir.path()).await;
    // Discard transport and unread socket buffers, retaining only applied state.
    c.reconnect().await.unwrap();
    for id in until_done(&mut c).await {
        assert!(applied.insert(id), "duplicate applied {id:?}");
        c.applied(&thread, id);
    }
    assert_eq!(
        applied.into_iter().map(|i| i.0).collect::<Vec<_>>(),
        (1..=1000).collect::<Vec<_>>()
    );
}
#[tokio::test]
async fn command_executed_response_lost_is_unknown_and_never_retried() {
    use spike_daemon_socket::CommandError;
    let dir = temp();
    let d = Daemon::start(dir.path()).await;
    let socket = dir.path().join("proxy.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    let upstream = dir.path().join("daemon.sock");
    let (paused, at_handshake) = tokio::sync::oneshot::channel();
    let (resume, wait_resume) = tokio::sync::oneshot::channel();
    let (finish, wait_finish) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut paused = Some(paused);
        let mut wait_resume = Some(wait_resume);
        let mut wait_finish = Some(wait_finish);
        for attempt in 0..2 {
            let (mut downstream, _) = listener.accept().await.unwrap();
            let mut upstream = UnixStream::connect(&upstream).await.unwrap();
            let hello = codec::read::<_, ClientMessage>(&mut downstream)
                .await
                .unwrap();
            codec::write(&mut upstream, &hello).await.unwrap();
            let welcome = codec::read::<_, ServerMessage>(&mut upstream)
                .await
                .unwrap();
            if attempt == 0 {
                codec::write(&mut downstream, &welcome).await.unwrap();
                let command = codec::read::<_, ClientMessage>(&mut downstream)
                    .await
                    .unwrap();
                assert!(matches!(
                    command,
                    Frame::Control(ClientMessage::Command {
                        command: DaemonCommand::StartThread { .. },
                        ..
                    })
                ));
                codec::write(&mut upstream, &command).await.unwrap();
                assert!(matches!(
                    codec::read::<_, ServerMessage>(&mut upstream)
                        .await
                        .unwrap(),
                    Frame::Control(ServerMessage::Response {
                        result: CommandResult::Ok { .. },
                        ..
                    })
                ));
                drop(downstream); // side effect committed; response lost
            } else {
                // Reconnect is paused before Welcome. A queued command must wait.
                paused.take().unwrap().send(()).unwrap();
                wait_resume.take().unwrap().await.unwrap();
                codec::write(&mut downstream, &welcome).await.unwrap();
                let command = codec::read::<_, ClientMessage>(&mut downstream)
                    .await
                    .unwrap();
                assert!(matches!(
                    command,
                    Frame::Control(ClientMessage::Command {
                        command: DaemonCommand::ListThreads,
                        ..
                    })
                ));
                codec::write(&mut upstream, &command).await.unwrap();
                let response = codec::read::<_, ServerMessage>(&mut upstream)
                    .await
                    .unwrap();
                codec::write(&mut downstream, &response).await.unwrap();
                wait_finish.take().unwrap().await.unwrap();
                return;
            }
        }
    });
    let mut c = Client::connect(socket, "loss").await.unwrap();
    let result = c
        .command(DaemonCommand::StartThread {
            agent: AgentLaunch::Fake {
                tokens_per_second: 100,
                chunk_chars: 200,
                duration_ms: 10,
                tool_call_every_ms: 0,
            },
            cwd: "/tmp".into(),
        })
        .await;
    assert!(matches!(result, Err(CommandError::UnknownOutcome(_))));
    at_handshake.await.unwrap();
    let handle = c.handle();
    let request = handle.command(DaemonCommand::ListThreads);
    tokio::pin!(request);
    std::future::poll_fn(|cx| {
        use std::future::Future;
        assert!(request.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    resume.send(()).unwrap();
    let CommandResult::Ok { value } = request.await.unwrap() else {
        panic!()
    };
    assert_eq!(
        value.as_array().unwrap().len(),
        1,
        "no automatic command retry"
    );
    let thread = ThreadId(value[0]["thread_id"].as_str().unwrap().into());
    let mut observer = d.client().await;
    observer.subscribe(thread, None).await.unwrap();
    loop {
        if let ServerMessage::Event { event } = observer.next().await.unwrap()
            && matches!(event.envelope.event, AgentEvent::SessionEnded { .. })
        {
            break;
        }
    }
    finish.send(()).unwrap();
    server.await.unwrap();
}
#[tokio::test]
async fn automatic_reconnect_from_applied_cursor() {
    let dir = temp();
    let _store = seed(dir.path(), 10).await;
    let d = Daemon::start(dir.path()).await;
    let mut c = d.client().await;
    c.subscribe(ThreadId("t".into()), Some(EventId(5)))
        .await
        .unwrap();
    // EOF after the daemon exits triggers the actor's automatic reconnect.
    let ids = until_done(&mut c).await;
    assert_eq!(ids.len(), 5);
    for id in ids {
        c.applied(&ThreadId("t".into()), id);
    }
    drop(d);
    _store
        .append(ThreadId("t".into()), (10..20).map(envelope).collect())
        .await
        .unwrap();
    let d = Daemon::start(dir.path()).await;
    let replayed = until_done(&mut c).await;
    assert_eq!(replayed, (11..=20).map(EventId).collect::<Vec<_>>());
    for id in replayed {
        c.applied(&ThreadId("t".into()), id);
    }
    c.reconnect().await.unwrap();
    let msg = timeout(Duration::from_secs(5), c.next())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        msg,
        ServerMessage::ReplayDone {
            last: Some(EventId(20)),
            ..
        }
    ));
    drop(d);
}
#[tokio::test]
async fn orphan_recovery_and_identity_mismatch() {
    let dir = temp();
    let mut d = Daemon::start(dir.path()).await;
    // Neither process has a pipe to the daemon. Only recovery can terminate the
    // matching process; its parent (this test) can prove the terminating signal.
    use std::os::unix::process::ExitStatusExt;
    let mut orphan = tokio::process::Command::new("/bin/sleep")
        .arg("60")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut bystander = tokio::process::Command::new("/bin/sleep")
        .arg("60")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let saved = daemon::identity(orphan.id().unwrap()).unwrap();
    let mismatch = Identity {
        start: "different start time".into(),
        ..daemon::identity(bystander.id().unwrap()).unwrap()
    };
    let thread = ThreadId("orphan".into());
    let cdb = rusqlite::Connection::open(dir.path().join("events.sqlite")).unwrap();
    for (id, identity) in [("orphan", &saved), ("mismatch", &mismatch)] {
        cdb.execute(
            "INSERT INTO threads(id,running,pid,start,exe) VALUES(?1,1,?2,?3,?4)",
            rusqlite::params![id, identity.pid, identity.start, identity.exe],
        )
        .unwrap();
    }
    for (seq, event) in [AgentEvent::TurnStarted { turn: 1 }, envelope(0).event]
        .into_iter()
        .enumerate()
    {
        let env = AgentEventEnvelope {
            seq: seq as u64,
            at_ms: 0,
            event,
        };
        cdb.execute(
            "INSERT INTO events(thread_id,envelope_json) VALUES('orphan',?1)",
            [serde_json::to_string(&env).unwrap()],
        )
        .unwrap();
    }
    drop(cdb);
    d.crash();
    assert!(orphan.try_wait().unwrap().is_none());
    let _restart = Daemon::start(dir.path()).await;
    let status = timeout(Duration::from_secs(2), orphan.wait())
        .await
        .expect("recovery did not kill the pipe-independent orphan")
        .unwrap();
    assert_eq!(status.signal(), Some(9));
    assert!(bystander.try_wait().unwrap().is_none());
    bystander.kill().await.unwrap();
    let db = store::open_reader(&dir.path().join("events.sqlite")).unwrap();
    let mut after = None;
    let mut events = vec![];
    loop {
        let page = store::page(&db, &thread, after, None).unwrap();
        if page.is_empty() {
            break;
        }
        after = page.last().map(|e| e.id);
        events.extend(page.into_iter().map(|e| e.envelope.event));
    }
    assert!(matches!(
        events.last(),
        Some(AgentEvent::SessionEnded { .. })
    ));
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Interrupted,
            ..
        }
    )));
    let pos = events
        .iter()
        .rposition(|e| matches!(e, AgentEvent::MessageCompleted { .. }))
        .unwrap();
    assert!(pos < events.len() - 2);
}
#[tokio::test]
async fn agents_continue_without_clients() {
    let dir = temp();
    let d = Daemon::start(dir.path()).await;
    let mut c = d.client().await;
    let thread = start_fake(&mut c, 150).await;
    drop(c);
    let db = store::open_reader(&dir.path().join("events.sqlite")).unwrap();
    timeout(Duration::from_secs(5), async {
        loop {
            let running: bool = db
                .query_row(
                    "SELECT running FROM threads WHERE id=?1",
                    [&thread.0],
                    |r| r.get(0),
                )
                .unwrap();
            if !running {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut c = d.client().await;
    c.subscribe(thread, None).await.unwrap();
    let mut ended = false;
    timeout(Duration::from_secs(5), async {
        loop {
            if let ServerMessage::Event { event } = c.next().await.unwrap()
                && matches!(event.envelope.event, AgentEvent::SessionEnded { .. })
            {
                ended = true;
                break;
            }
        }
    })
    .await
    .unwrap();
    assert!(ended);
}
#[tokio::test]
async fn handshake_required_and_version_checked() {
    let dir = temp();
    let d = Daemon::start(dir.path()).await;
    let mut s = UnixStream::connect(dir.path().join("daemon.sock"))
        .await
        .unwrap();
    codec::write(
        &mut s,
        &Frame::Control(ClientMessage::Hello {
            protocol_version: 999,
            client_name: "bad".into(),
        }),
    )
    .await
    .unwrap();
    assert!(matches!(
        codec::read::<_, ServerMessage>(&mut s).await.unwrap(),
        Frame::Control(ServerMessage::Incompatible { .. })
    ));
    let mut s = UnixStream::connect(dir.path().join("daemon.sock"))
        .await
        .unwrap();
    codec::write(
        &mut s,
        &Frame::Control(ClientMessage::Command {
            request_id: 1,
            command: DaemonCommand::ListThreads,
        }),
    )
    .await
    .unwrap();
    assert!(codec::read::<_, ServerMessage>(&mut s).await.is_err());
    drop(d);
}

#[test]
fn closing_lifecycle_order_and_sequence() {
    let events = vec![
        AgentEvent::SessionStarted {
            provider: Provider::Codex,
            native_session_id: "fake".into(),
            model: None,
            tools: vec![],
        },
        AgentEvent::TurnStarted { turn: 1 },
        envelope(0).event,
        AgentEvent::ToolCallStarted {
            call_id: "call".into(),
            name: "read".into(),
            kind: ToolKind::Read,
            title: None,
            input: serde_json::json!({}),
        },
        AgentEvent::ApprovalRequested {
            request_id: "approval".into(),
            call_id: Some("call".into()),
            name: "read".into(),
            kind: ToolKind::Read,
            title: None,
            input: serde_json::json!({}),
            reason: None,
        },
    ];
    let closed = daemon::closing(
        events
            .into_iter()
            .enumerate()
            .map(|(seq, event)| AgentEventEnvelope {
                seq: seq as u64,
                at_ms: 0,
                event,
            }),
        "Interrupted",
    );
    assert_eq!(
        closed.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![5, 6, 7, 8, 9]
    );
    assert!(
        matches!(&closed[0].event, AgentEvent::MessageCompleted { text, .. } if text.len()==200)
    );
    assert!(matches!(
        closed[1].event,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Cancelled,
            ..
        }
    ));
    assert!(matches!(
        closed[2].event,
        AgentEvent::RequestResolved {
            outcome: RequestOutcome::Cancelled,
            ..
        }
    ));
    assert!(matches!(
        closed[3].event,
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Interrupted,
            ..
        }
    ));
    assert!(matches!(closed[4].event, AgentEvent::SessionEnded { .. }));
    assert!(daemon::closing(closed, "again").is_empty());
    let pre = daemon::closing([], "could not start");
    assert!(matches!(
        pre[0].event,
        AgentEvent::Error { fatal: true, .. }
    ));
    assert!(matches!(pre[1].event, AgentEvent::SessionEnded { .. }));
}

#[tokio::test]
async fn client_death_restores_only_persisted_applied_cursor() {
    let dir = temp();
    let _store = seed(dir.path(), 100).await;
    let d = Daemon::start(dir.path()).await;
    let thread = ThreadId("t".into());
    let mut c = d.client().await;
    c.subscribe(thread.clone(), None).await.unwrap();
    let mut applied = BTreeSet::new();
    let mut persisted = None;
    for _ in 0..10 {
        let ServerMessage::Event { event } = c.next().await.unwrap() else {
            panic!()
        };
        applied.insert(event.id);
        persisted = Some(event.id);
        c.applied(&thread, event.id);
    }
    assert!(
        matches!(c.next().await.unwrap(),ServerMessage::Event {event} if event.id==EventId(11))
    );
    drop(c); // Entire old consumer dies; the daemon remains alive.
    let mut replacement = d.client().await;
    replacement
        .subscribe(thread.clone(), persisted)
        .await
        .unwrap();
    for id in until_done(&mut replacement).await {
        assert!(applied.insert(id));
        replacement.applied(&thread, id);
    }
    assert_eq!(
        applied.into_iter().collect::<Vec<_>>(),
        (1..=100).map(EventId).collect::<Vec<_>>()
    );
}
#[tokio::test]
async fn resubscribe_with_inflight_events_and_drop_applied_ids() {
    let dir = temp();
    let _store = seed(dir.path(), 1000).await;
    let d = Daemon::start(dir.path()).await;
    let mut c = d.client().await;
    let thread = ThreadId("t".into());
    c.subscribe(thread.clone(), None).await.unwrap();
    // Buffered events <= this consumer cursor must never reach the UI.
    c.applied(&thread, EventId(900));
    let ServerMessage::Event { event } = c.next().await.unwrap() else {
        panic!()
    };
    assert_eq!(event.id, EventId(901));
    let handle = c.handle();
    let subscription = handle.subscribe(thread.clone(), Some(EventId(995)));
    tokio::pin!(subscription);
    loop {
        tokio::select! {biased; r=&mut subscription=>{assert!(matches!(r.unwrap(),CommandResult::Ok {..}));break;}, event=c.next()=>{event.unwrap();}}
    }
    assert_eq!(
        until_done(&mut c).await,
        (996..=1000).map(EventId).collect::<Vec<_>>()
    );
}
#[tokio::test]
async fn client_checks_private_directory_and_socket_owner() {
    let dir = temp();
    let d = Daemon::start(dir.path()).await;
    let metadata = fs::symlink_metadata(dir.path().join("daemon.sock")).unwrap();
    assert!(
        daemon::verify_socket_metadata(&metadata, nix::unistd::geteuid().as_raw() + 1).is_err()
    );
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        Client::connect(dir.path().join("daemon.sock"), "bad-mode")
            .await
            .is_err()
    );
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let other = temp();
    let link = other.path().join("alias");
    symlink(dir.path(), &link).unwrap();
    assert!(
        Client::connect(link.join("daemon.sock"), "symlink")
            .await
            .is_err()
    );
    d.client().await;
}
#[tokio::test]
async fn exact_frame_limit_and_write_side_oversize() {
    use spike_daemon_socket::wire::MAX_FRAME_LEN;
    let frame = Frame::<serde_json::Value>::Stream {
        stream_id: 42,
        bytes: vec![7; MAX_FRAME_LEN as usize - 5],
    };
    let mut encoded = vec![];
    codec::write(&mut encoded, &frame).await.unwrap();
    assert_eq!(
        u32::from_be_bytes(encoded[..4].try_into().unwrap()),
        MAX_FRAME_LEN
    );
    assert_eq!(
        codec::read::<_, serde_json::Value>(&mut encoded.as_slice())
            .await
            .unwrap(),
        frame
    );
    let too_big = Frame::<serde_json::Value>::Stream {
        stream_id: 42,
        bytes: vec![0; MAX_FRAME_LEN as usize - 4],
    };
    let mut sink = vec![];
    assert!(codec::write(&mut sink, &too_big).await.is_err());
    assert!(sink.is_empty());
    let mut sink = vec![];
    let too_big = Frame::Control("x".repeat(MAX_FRAME_LEN as usize));
    assert!(codec::write(&mut sink, &too_big).await.is_err());
    assert!(sink.is_empty());
}
#[tokio::test]
async fn connect_or_spawn_is_shared_and_logs_to_private_file() {
    let parent = temp();
    let dir = parent.path().join("private");
    let c = Client::connect_or_spawn(&dir, env!("CARGO_BIN_EXE_spike-daemon"), "spawn")
        .await
        .unwrap();
    let again = Client::connect_or_spawn(&dir, "/not/an/executable", "existing")
        .await
        .unwrap();
    let mut raw = UnixStream::connect(dir.join("daemon.sock")).await.unwrap();
    codec::write(
        &mut raw,
        &Frame::Control(ClientMessage::Hello {
            protocol_version: 1,
            client_name: "pid".into(),
        }),
    )
    .await
    .unwrap();
    let Frame::Control(ServerMessage::Welcome { daemon_pid, .. }) =
        codec::read(&mut raw).await.unwrap()
    else {
        panic!()
    };
    assert_eq!(
        fs::metadata(dir.join("daemon.log"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    drop(c);
    drop(again);
    drop(raw);
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(daemon_pid as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
}

#[tokio::test]
async fn permanent_reconnect_errors_surface_and_queued_commands_are_not_sent() {
    use spike_daemon_socket::{CommandError, ConnectionState};
    for scenario in [
        "incompatible",
        "bad_handshake",
        "unknown thread",
        "cursor past end",
    ] {
        let dir = temp();
        let socket = dir.path().join("s");
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        let (close, closed) = tokio::sync::oneshot::channel();
        let (paused, at_welcome) = tokio::sync::oneshot::channel();
        let (resume, wait_resume) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            codec::read::<_, ClientMessage>(&mut s).await.unwrap();
            codec::write(
                &mut s,
                &Frame::Control(ServerMessage::Welcome {
                    protocol_version: 1,
                    daemon_version: "test".into(),
                    daemon_pid: 1,
                }),
            )
            .await
            .unwrap();
            let Frame::Control(ClientMessage::Subscribe { request_id, .. }) =
                codec::read(&mut s).await.unwrap()
            else {
                panic!()
            };
            codec::write(
                &mut s,
                &Frame::Control(ServerMessage::Response {
                    request_id,
                    result: CommandResult::Ok {
                        value: serde_json::json!({"replay_until":null}),
                    },
                }),
            )
            .await
            .unwrap();
            closed.await.unwrap();
            drop(s);
            let (mut s, _) = listener.accept().await.unwrap();
            codec::read::<_, ClientMessage>(&mut s).await.unwrap();
            paused.send(()).unwrap();
            wait_resume.await.unwrap();
            let message = match scenario {
                "incompatible" => ServerMessage::Incompatible {
                    protocol_version: 2,
                    message: "upgrade required".into(),
                },
                "bad_handshake" => ServerMessage::ReplayDone {
                    thread_id: ThreadId("t".into()),
                    last: None,
                },
                _ => ServerMessage::Welcome {
                    protocol_version: 1,
                    daemon_version: "test".into(),
                    daemon_pid: 1,
                },
            };
            codec::write(&mut s, &Frame::Control(message))
                .await
                .unwrap();
            if !matches!(scenario, "incompatible" | "bad_handshake") {
                let Frame::Control(ClientMessage::Subscribe { request_id, .. }) =
                    codec::read(&mut s).await.unwrap()
                else {
                    panic!()
                };
                codec::write(
                    &mut s,
                    &Frame::Control(ServerMessage::Response {
                        request_id,
                        result: CommandResult::Err {
                            message: scenario.into(),
                        },
                    }),
                )
                .await
                .unwrap();
            }
            // Terminal classification closes this socket; no command was written.
            assert!(codec::read::<_, ClientMessage>(&mut s).await.is_err());
        });
        let mut c = Client::connect(socket, "permanent").await.unwrap();
        c.subscribe(ThreadId("t".into()), None).await.unwrap();
        close.send(()).unwrap();
        at_welcome.await.unwrap();
        let handle = c.handle();
        let request = handle.command(DaemonCommand::ListThreads);
        tokio::pin!(request);
        std::future::poll_fn(|cx| {
            use std::future::Future;
            assert!(request.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        resume.send(()).unwrap();
        assert!(matches!(
            timeout(Duration::from_secs(2), request).await.unwrap(),
            Err(CommandError::NotSent(_))
        ));
        assert_eq!(
            c.next().await.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(matches!(
            *c.handle().connection_state().borrow(),
            ConnectionState::Failed(_)
        ));
        server.await.unwrap();
    }
}
#[tokio::test]
async fn only_appends_notify_store_subscribers() {
    let dir = temp();
    let store = seed(dir.path(), 1).await;
    let mut changed = store.changed.clone();
    store
        .append(ThreadId("t".into()), vec![envelope(1)])
        .await
        .unwrap();
    changed.changed().await.unwrap();
    changed.borrow_and_update();
    store
        .call(|c| {
            c.query_row("SELECT count(*) FROM threads", [], |r| r.get::<_, i64>(0))
                .map_err(io::Error::other)
        })
        .await
        .unwrap();
    assert!(!changed.has_changed().unwrap());
}

#[tokio::test]
async fn invalid_command_frame_is_not_sent_and_connection_remains_usable() {
    let dir = temp();
    let d = Daemon::start(dir.path()).await;
    let mut c = d.client().await;
    let command = DaemonCommand::ThreadCommand {
        thread_id: ThreadId("unused".into()),
        command: SessionCommand::SendUserMessage {
            text: "x".repeat(MAX_FRAME_LEN as usize),
        },
    };
    assert!(matches!(
        c.command(command).await,
        Err(spike_daemon_socket::CommandError::NotSent(_))
    ));
    assert!(matches!(
        c.command(DaemonCommand::ListThreads).await.unwrap(),
        CommandResult::Ok { .. }
    ));
}
