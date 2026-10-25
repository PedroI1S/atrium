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
    os::unix::{
        fs::{PermissionsExt, symlink},
        process::CommandExt,
    },
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
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut d = Self {
            child,
            dir: dir.into(),
        };
        for _ in 0..200 {
            if Client::connect(dir.join("daemon.sock"), "ready")
                .await
                .is_ok()
            {
                return d;
            }
            assert!(
                d.child.try_wait().unwrap().is_none(),
                "daemon exited before ready"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("daemon not ready");
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
async fn replay_live_boundary_repeated_with_concurrent_append() {
    let dir = temp();
    let store = seed(dir.path(), 1024).await;
    let path = dir.path().to_owned();
    let writer = store.clone();
    let serving = tokio::spawn(async move {
        daemon::run_with_store(
            &path,
            PathBuf::from(env!("CARGO_BIN_EXE_spike-fake-agent")),
            writer,
        )
        .await
        .unwrap()
    });
    let socket = dir.path().join("daemon.sock");
    for _ in 0..100 {
        if Client::connect(&socket, "ready").await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    for round in 0..25 {
        let mut c = Client::connect(&socket, "boundary").await.unwrap();
        let base = 1024 + round * 64;
        let writer = store.clone();
        let writing = tokio::spawn(async move {
            for seq in base..base + 64 {
                writer
                    .append(ThreadId("t".into()), vec![envelope(seq)])
                    .await
                    .unwrap();
                tokio::task::yield_now().await;
            }
        });
        let response = c
            .subscribe(ThreadId("t".into()), Some(EventId(base - 512)))
            .await
            .unwrap();
        let CommandResult::Ok { value } = response else {
            panic!("subscribe failed")
        };
        let boundary = value["replay_until"].as_u64().unwrap();
        let mut next = base - 512 + 1;
        let mut done = false;
        timeout(Duration::from_secs(10), async {
            while next <= base + 64 || !done {
                match c.next().await.unwrap() {
                    ServerMessage::Event { event } => {
                        assert_eq!(event.id.0, next);
                        if !done {
                            assert!(event.id.0 <= boundary);
                        } else {
                            assert!(event.id.0 > boundary);
                        }
                        next += 1;
                        c.applied(&event.thread_id, event.id);
                    }
                    ServerMessage::ReplayDone { last, .. } => {
                        assert!(!done);
                        assert_eq!(last, Some(EventId(boundary)));
                        assert_eq!(next, boundary + 1);
                        done = true;
                    }
                    _ => panic!("unexpected message"),
                }
            }
        })
        .await
        .unwrap();
        writing.await.unwrap();
        assert!(done);
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
    let dir = temp();
    let d = Daemon::start(dir.path()).await;
    let socket = dir.path().join("proxy.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let upstream = dir.path().join("daemon.sock");
    let server = tokio::spawn(async move {
        let (mut downstream, _) = listener.accept().await.unwrap();
        let mut upstream = UnixStream::connect(upstream).await.unwrap();
        let hello = codec::read::<_, ClientMessage>(&mut downstream)
            .await
            .unwrap();
        codec::write(&mut upstream, &hello).await.unwrap();
        let welcome = codec::read::<_, ServerMessage>(&mut upstream)
            .await
            .unwrap();
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
        // Execution completed in the actual daemon. Discard its response.
        assert!(matches!(
            codec::read::<_, ServerMessage>(&mut upstream)
                .await
                .unwrap(),
            Frame::Control(ServerMessage::Response {
                result: CommandResult::Ok { .. },
                ..
            })
        ));
        drop(downstream);
        assert!(
            timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    });
    let mut c = Client::connect(socket, "loss").await.unwrap();
    assert!(
        c.command(DaemonCommand::StartThread {
            agent: AgentLaunch::Fake {
                tokens_per_second: 100,
                chunk_chars: 200,
                duration_ms: 200,
                tool_call_every_ms: 0
            },
            cwd: "/tmp".into()
        })
        .await
        .is_err()
    );
    server.await.unwrap();
    let mut observer = d.client().await;
    let CommandResult::Ok { value } = observer.command(DaemonCommand::ListThreads).await.unwrap()
    else {
        panic!()
    };
    assert_eq!(
        value.as_array().unwrap().len(),
        1,
        "command must execute once"
    );
    // Let the short-lived child exit and be reaped before dropping the daemon.
    tokio::time::sleep(Duration::from_millis(300)).await;
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
    // Explicitly losing transport (a failed command) exercises next's automatic path.
    let ids = until_done(&mut c).await;
    assert_eq!(ids.len(), 5);
    for id in ids {
        c.applied(&ThreadId("t".into()), id);
    }
    drop(d);
    let d = Daemon::start(dir.path()).await;
    let msg = timeout(Duration::from_secs(5), c.next())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        msg,
        ServerMessage::ReplayDone {
            last: Some(EventId(10)),
            ..
        }
    ));
    drop(d);
}
#[tokio::test]
async fn orphan_recovery_and_identity_mismatch() {
    let dir = temp();
    let mut d = Daemon::start(dir.path()).await;
    let mut c = d.client().await;
    let thread = start_fake(&mut c, 60_000).await;
    c.subscribe(thread.clone(), None).await.unwrap();
    loop {
        if matches!(
            c.next().await.unwrap(),
            ServerMessage::Event {
                event: StoredEvent {
                    envelope: AgentEventEnvelope {
                        event: AgentEvent::MessageDelta { .. },
                        ..
                    },
                    ..
                }
            }
        ) {
            break;
        }
    }
    let db = store::open_reader(&dir.path().join("events.sqlite")).unwrap();
    let saved = db
        .query_row(
            "SELECT pid,start,exe FROM threads WHERE id=?1",
            [&thread.0],
            |r| {
                Ok(Identity {
                    pid: r.get(0)?,
                    start: r.get(1)?,
                    exe: r.get(2)?,
                })
            },
        )
        .unwrap();
    // An unrelated live process with a mismatched stored start must survive restart.
    let mut bystander = Command::new("/bin/sleep")
        .arg("60")
        .process_group(0)
        .spawn()
        .unwrap();
    let mismatch = Identity {
        start: "different start time".into(),
        ..daemon::identity(bystander.id()).unwrap()
    };
    let cdb = rusqlite::Connection::open(dir.path().join("events.sqlite")).unwrap();
    cdb.execute(
        "INSERT INTO threads(id,running,pid,start,exe) VALUES('mismatch',1,?1,?2,?3)",
        rusqlite::params![mismatch.pid, mismatch.start, mismatch.exe],
    )
    .unwrap();
    drop(cdb);
    d.crash();
    assert_eq!(daemon::identity(saved.pid).unwrap(), saved);
    let _restart = Daemon::start(dir.path()).await;
    assert!(bystander.try_wait().unwrap().is_none());
    bystander.kill().unwrap();
    bystander.wait().unwrap();
    // A dead child may briefly remain as a zombie under the host's init.
    for _ in 0..100 {
        if daemon::identity(saved.pid).ok().as_ref() != Some(&saved) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_ne!(daemon::identity(saved.pid).ok().as_ref(), Some(&saved));
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
    tokio::time::sleep(Duration::from_millis(300)).await;
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
