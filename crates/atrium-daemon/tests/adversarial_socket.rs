#[path = "common/mod.rs"]
mod common;

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use atrium_agents::{AdapterError, AgentAdapter, BoxFuture, Session, SessionConfig, Transport};
use atrium_core::store::{Migration, SqliteStore, StoreOptions};
use atrium_daemon::AppIdentitySource;
use atrium_host::peer::AppIdentity;
use atrium_protocol::PROTOCOL_VERSION;
use atrium_protocol::agent::{ApprovalDecision, Provider, QuestionAnswer};
use atrium_protocol::limits::{HELLO_TIMEOUT, MAX_CLIENT_FRAME_LEN};
use atrium_protocol::policy::{EnforcementDescriptor, Preset};
use atrium_protocol::thread::{ThreadId, ThreadState};
use atrium_protocol::wire::{ClientMessage, ClientRole, DaemonCommand, EventId, ServerMessage};
use common::{Rig, Wire, native, soon, until};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::Notify;

fn assert_incompatible(first: Option<ServerMessage>, expected: &str) {
    match first {
        Some(ServerMessage::Incompatible {
            message,
            daemon_pid,
            protocol_version,
            ..
        }) => {
            assert_eq!(daemon_pid, std::process::id());
            assert_eq!(protocol_version, PROTOCOL_VERSION);
            assert_eq!(message, expected);
        }
        other => panic!("expected Incompatible({expected:?}), got {other:?}"),
    }
}

#[tokio::test]
async fn socket_roles_wrong_app_identity_dev_off_and_pre_hello_commands() {
    let me = atrium_host::peer::current_app_identity().expect("current app identity");

    let wrong_variant = AppIdentity::MacSigned {
        requirement: "identifier \"com.atrium.fake\"".into(),
    };
    let wrong_ino = AppIdentity::Linux {
        executable: std::env::current_exe().expect("exe"),
        dev_ino: Some((u64::MAX, u64::MAX)),
    };
    let wrong_path = AppIdentity::Linux {
        executable: std::env::temp_dir().join("not-the-atrium-app"),
        dev_ino: None,
    };
    let non_root_packaged = AppIdentity::Linux {
        executable: std::env::current_exe().expect("exe"),
        dev_ino: None,
    };

    for bad in [wrong_variant, wrong_ino, wrong_path, non_root_packaged] {
        let mut rig = Rig::start_with(|options| {
            options.dev_socket = false;
            options.app_identity = AppIdentitySource::Fixed(bad);
        })
        .await;

        let (mut app, first) = Wire::hello(rig.socket(), ClientRole::App).await;
        assert_incompatible(first, "unverified client");
        assert!(app.closed().await);

        let (mut dev, first) = Wire::hello(rig.socket(), ClientRole::Dev).await;
        assert_incompatible(first, "unverified client");
        assert!(dev.closed().await);

        let (mut agent, first) = Wire::hello(
            rig.socket(),
            ClientRole::Agent {
                token: "forged".into(),
            },
        )
        .await;
        assert_incompatible(first, "unverified client");
        assert!(agent.closed().await);

        rig.stop(true).await;
    }

    let mut rig = Rig::in_folder(common::folder());
    let id_file = rig.paths.app_identity.clone();
    rig.boot(|options| {
        options.dev_socket = false;
        options.app_identity = AppIdentitySource::File(id_file.clone());
    })
    .await;

    std::fs::create_dir_all(&id_file).expect("mkdir");
    let (mut app, first) = Wire::hello(rig.socket(), ClientRole::App).await;
    assert_incompatible(first, "unverified client");
    assert!(app.closed().await);
    std::fs::remove_dir(&id_file).expect("rmdir");

    std::fs::write(&id_file, b"{not valid json").expect("write");
    std::fs::set_permissions(&id_file, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    let (mut app, first) = Wire::hello(rig.socket(), ClientRole::App).await;
    assert_incompatible(first, "unverified client");
    assert!(app.closed().await);

    let forged = AppIdentity::Linux {
        executable: std::env::current_exe().expect("exe"),
        dev_ino: Some((0, 0)),
    };
    std::fs::write(&id_file, serde_json::to_vec(&forged).expect("json")).expect("write");
    std::fs::set_permissions(&id_file, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    let (mut app, first) = Wire::hello(rig.socket(), ClientRole::App).await;
    assert_incompatible(first, "unverified client");
    assert!(app.closed().await);

    let mut huge = vec![b' '; 70 * 1024];
    huge.extend_from_slice(&serde_json::to_vec(&me).expect("json"));
    std::fs::write(&id_file, &huge).expect("write");
    std::fs::set_permissions(&id_file, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    let (mut app, first) = Wire::hello(rig.socket(), ClientRole::App).await;
    assert_incompatible(first, "unverified client");
    assert!(app.closed().await);

    std::fs::write(&id_file, serde_json::to_vec(&me).expect("json")).expect("write");
    for mode in [0o640, 0o620, 0o604, 0o601] {
        std::fs::set_permissions(&id_file, std::fs::Permissions::from_mode(mode)).expect("chmod");
        let (mut app, first) = Wire::hello(rig.socket(), ClientRole::App).await;
        assert_incompatible(first, "unverified client");
        assert!(app.closed().await);
    }

    for msg in [
        ClientMessage::SubscribeInbox { request_id: 1 },
        ClientMessage::Subscribe {
            request_id: 2,
            thread_id: ThreadId("th-1".into()),
            after: None,
        },
        ClientMessage::Unsubscribe {
            thread_id: ThreadId("th-1".into()),
        },
        ClientMessage::Command {
            request_id: 3,
            command: DaemonCommand::ListThreads,
        },
        ClientMessage::Command {
            request_id: 4,
            command: DaemonCommand::ResolveApproval {
                thread_id: ThreadId("th-1".into()),
                incarnation: 1,
                request_id: "req-1".into(),
                decision: ApprovalDecision::Allow,
                remember: None,
            },
        },
        ClientMessage::Command {
            request_id: 5,
            command: DaemonCommand::AnswerQuestion {
                thread_id: ThreadId("th-1".into()),
                incarnation: 1,
                request_id: "req-2".into(),
                answers: vec![QuestionAnswer {
                    question_id: "q1".into(),
                    selected: vec!["a".into()],
                    text: None,
                }],
            },
        },
        ClientMessage::Command {
            request_id: 6,
            command: DaemonCommand::SetSetting {
                key: "theme".into(),
                value: json!("dark"),
            },
        },
        ClientMessage::Command {
            request_id: 7,
            command: DaemonCommand::ShutdownDaemon { force: true },
        },
    ] {
        let mut wire = Wire::connect(rig.socket()).await;
        wire.send(&msg).await;
        assert!(wire.read().await.is_none(), "pre-Hello {msg:?} must close");
    }

    for ver in [0, PROTOCOL_VERSION.saturating_sub(1), u32::MAX] {
        if ver == PROTOCOL_VERSION {
            continue;
        }
        let mut wire = Wire::connect(rig.socket()).await;
        wire.send(&ClientMessage::Hello {
            protocol_version: ver,
            client_name: "bad-ver".into(),
            role: ClientRole::App,
        })
        .await;
        assert_incompatible(wire.read().await, "protocol version mismatch");
        assert!(wire.closed().await);
    }

    std::fs::set_permissions(&id_file, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    let (mut app, first) = Wire::hello(rig.socket(), ClientRole::App).await;
    assert!(matches!(
        first,
        Some(ServerMessage::Welcome {
            role: ClientRole::App,
            ..
        })
    ));
    assert!(app.ok(DaemonCommand::ListThreads).await.is_array());
    rig.stop(true).await;
}

#[tokio::test]
async fn hello_sent_byte_by_byte_across_timeout_is_closed() {
    let mut rig = Rig::start().await;

    let hello_payload = serde_json::to_vec(&ClientMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
        client_name: "drip".into(),
        role: ClientRole::Dev,
    })
    .expect("serializes");
    let mut frame = ((hello_payload.len() + 1) as u32).to_be_bytes().to_vec();
    frame.push(0);
    frame.extend_from_slice(&hello_payload);

    let began = Instant::now();
    let mut stream = UnixStream::connect(rig.socket()).await.expect("connects");
    for &byte in &frame {
        if stream.write_all(&[byte]).await.is_err() {
            break;
        }
        let mut buf = [0u8; 16];
        match tokio::time::timeout(Duration::from_millis(250), stream.read(&mut buf)).await {
            Ok(Ok(0) | Err(_)) => break,
            Ok(Ok(n)) => panic!("unexpected server bytes before Hello finished: {n}"),
            Err(_) => {}
        }
    }
    let mut sink = [0u8; 64];
    let n = soon(stream.read(&mut sink)).await.unwrap_or(0);
    assert_eq!(n, 0, "daemon must close without welcoming a slow Hello");
    let elapsed = began.elapsed();
    assert!(
        elapsed >= HELLO_TIMEOUT - Duration::from_millis(250),
        "closed too early: {elapsed:?}"
    );
    assert!(
        elapsed < HELLO_TIMEOUT + Duration::from_secs(3),
        "closed too late: {elapsed:?}"
    );

    let mut fast = UnixStream::connect(rig.socket()).await.expect("connects");
    for &byte in &frame {
        fast.write_all(&[byte]).await.expect("writes byte");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let len = soon(fast.read_u32()).await.expect("len");
    let kind = soon(fast.read_u8()).await.expect("kind");
    assert_eq!(kind, 0);
    let mut payload = vec![0u8; len as usize - 1];
    soon(fast.read_exact(&mut payload)).await.expect("payload");
    let msg: ServerMessage = serde_json::from_slice(&payload).expect("ServerMessage");
    assert!(matches!(msg, ServerMessage::Welcome { .. }), "{msg:?}");

    rig.stop(true).await;
}

#[tokio::test]
async fn oversized_zero_and_truncated_frames_close_the_connection() {
    let mut rig = Rig::start().await;

    for raw_len in [0u32, MAX_CLIENT_FRAME_LEN + 1, 0x8000_0000, u32::MAX] {
        let mut stream = UnixStream::connect(rig.socket()).await.expect("connects");
        let _ = stream.write_all(&raw_len.to_be_bytes()).await;
        let mut buf = [0u8; 16];
        let n = soon(stream.read(&mut buf)).await.unwrap_or(0);
        assert_eq!(n, 0, "pre-Hello raw_len {raw_len} must close immediately");
    }

    for raw_len in [0u32, MAX_CLIENT_FRAME_LEN + 1, u32::MAX] {
        let mut wire = rig.dev().await;
        let mut stream = UnixStream::connect(rig.socket()).await.expect("connects");
        let hello = serde_json::to_vec(&ClientMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: "test".into(),
            role: ClientRole::Dev,
        })
        .expect("serializes");
        let mut hframe = ((hello.len() + 1) as u32).to_be_bytes().to_vec();
        hframe.push(0);
        hframe.extend_from_slice(&hello);
        stream.write_all(&hframe).await.expect("hello");
        let wlen = soon(stream.read_u32()).await.expect("wlen");
        let mut wbuf = vec![0u8; wlen as usize];
        soon(stream.read_exact(&mut wbuf)).await.expect("welcome");

        let _ = stream.write_all(&raw_len.to_be_bytes()).await;
        let mut sink = [0u8; 64];
        let n = soon(stream.read(&mut sink)).await.unwrap_or(0);
        assert_eq!(n, 0, "post-Hello raw_len {raw_len} must close");

        assert!(wire.ok(DaemonCommand::ListThreads).await.is_array());
    }

    let mut stream = UnixStream::connect(rig.socket()).await.expect("connects");
    stream
        .write_all(&4096u32.to_be_bytes())
        .await
        .expect("header");
    stream.write_all(&[0u8; 10]).await.expect("partial");
    stream.shutdown().await.expect("EOF");
    let mut sink = [0u8; 32];
    assert_eq!(soon(stream.read(&mut sink)).await.unwrap_or(0), 0);

    rig.stop(true).await;
}

/// The 8 MiB control lane (N4). Deterministic (third part): the client never
/// reads while the daemon queues. Phase 1 queues about 6 MiB of answers and
/// ends with a rename; once the rename shows on another connection, every
/// earlier command on the flooded one has been answered and is waiting in
/// its lane (nothing was read; the socket buffers hold far less than one
/// answer). Phase 2 pushes the lane past 8 MiB. Only then does the client
/// read: the daemon must end the connection before delivering every answer.
/// If the limit were not enforced, every answer would arrive and the test
/// fails at once (it doesn't wait for a close that never comes).
#[tokio::test]
async fn flooding_unread_list_threads_past_8_mib_control_lane_closes_connection() {
    let mut rig = Rig::start().await;
    let mut setup = rig.dev().await;

    let big_native = "n".repeat(260_000);
    let mut marker_thread = None;
    for _ in 0..4 {
        let summary = setup.start_thread(native(), Some("flood")).await;
        let mut script = rig.script().await;
        script.command().await;
        script.started(&big_native).await;
        script.ended(Some(0)).await;
        setup
            .thread_when(&summary.thread_id, |s| s.state == ThreadState::Ended)
            .await;
        marker_thread = Some(summary.thread_id);
    }
    let marker_thread = marker_thread.expect("a thread");

    let sample = serde_json::to_vec(&setup.ok(DaemonCommand::ListThreads).await).expect("json");
    let size = sample.len();
    assert!(size >= 1_000_000, "sample len: {size}");
    const LIMIT: usize = 8 * 1024 * 1024;
    // phase 1: well below the limit
    let phase1 = (6 * 1024 * 1024) / size;
    // phase 2: sized from the lane limit with margin for socket buffering,
    // counting nothing as drained
    let phase2 = (LIMIT + 4 * 1024 * 1024) / size + 2;

    async fn write_msg(stream: &mut UnixStream, message: &ClientMessage) -> std::io::Result<()> {
        let payload = serde_json::to_vec(message).expect("serializes");
        let mut frame = ((payload.len() + 1) as u32).to_be_bytes().to_vec();
        frame.push(0);
        frame.extend_from_slice(&payload);
        stream.write_all(&frame).await
    }

    async fn read_msg(stream: &mut UnixStream) -> Option<ServerMessage> {
        soon(async {
            let len = stream.read_u32().await.ok()?;
            let kind = stream.read_u8().await.ok()?;
            assert_eq!(kind, 0, "M1 has no stream frames");
            let mut payload = vec![0; len as usize - 1];
            stream.read_exact(&mut payload).await.ok()?;
            Some(serde_json::from_slice(&payload).expect("a server message"))
        })
        .await
    }

    let mut flooded = UnixStream::connect(rig.socket()).await.expect("connects");
    write_msg(
        &mut flooded,
        &ClientMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: "test".into(),
            role: ClientRole::Dev,
        },
    )
    .await
    .expect("hello");
    assert!(
        matches!(
            read_msg(&mut flooded).await,
            Some(ServerMessage::Welcome { .. })
        ),
        "expected Welcome"
    );

    let mut req_id = 0u64;
    let mut sent = 0usize;
    for _ in 0..phase1 {
        req_id += 1;
        write_msg(
            &mut flooded,
            &ClientMessage::Command {
                request_id: req_id,
                command: DaemonCommand::ListThreads,
            },
        )
        .await
        .expect("phase1 write");
        sent += 1;
    }
    req_id += 1;
    write_msg(
        &mut flooded,
        &ClientMessage::Command {
            request_id: req_id,
            command: DaemonCommand::RenameThread {
                thread_id: marker_thread.clone(),
                title: "phase-1-done".into(),
            },
        },
    )
    .await
    .expect("phase1 marker write");
    sent += 1;
    // every phase-1 answer is now queued, unread
    setup
        .thread_when(&marker_thread, |s| s.title == "phase-1-done")
        .await;

    let mut closed_during_send = false;
    for _ in 0..phase2 {
        req_id += 1;
        if write_msg(
            &mut flooded,
            &ClientMessage::Command {
                request_id: req_id,
                command: DaemonCommand::ListThreads,
            },
        )
        .await
        .is_err()
        {
            closed_during_send = true;
            break;
        }
        sent += 1;
    }
    if !closed_during_send {
        req_id += 1;
        if write_msg(
            &mut flooded,
            &ClientMessage::Command {
                request_id: req_id,
                command: DaemonCommand::RenameThread {
                    thread_id: marker_thread.clone(),
                    title: "phase-2-done".into(),
                },
            },
        )
        .await
        .is_err()
        {
            closed_during_send = true;
        } else {
            sent += 1;
        }
    }

    // Without reading from `flooded` (so the test does not drain the socket
    // while the daemon is answering `phase2`), wait until either the daemon
    // closes `flooded` (detected by a no-op write failing) or `phase-2-done`
    // becomes visible (a broken daemon answered everything).
    let peer_closed = if closed_during_send {
        true
    } else {
        let probe = ClientMessage::Unsubscribe {
            thread_id: ThreadId("__flood_probe__".into()),
        };
        soon(async {
            tokio::select! {
                _ = setup.thread_when(&marker_thread, |s| s.title == "phase-2-done") => false,
                _ = async {
                    loop {
                        if write_msg(&mut flooded, &probe).await.is_err() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                } => true,
            }
        })
        .await
    };
    assert!(
        peer_closed,
        "the 8 MiB control lane was not enforced: phase-2-done completed on flooded socket"
    );

    let mut received = 0usize;
    while read_msg(&mut flooded).await.is_some() {
        received += 1;
        assert!(
            received < sent,
            "the 8 MiB control lane was not enforced: all {sent} answers arrived"
        );
    }
    assert!(
        soon(async {
            let mut sink = [0u8; 4096];
            loop {
                match flooded.read(&mut sink).await {
                    Ok(0) | Err(_) => return true,
                    Ok(_) => {}
                }
            }
        })
        .await
    );

    let threads = setup.ok(DaemonCommand::ListThreads).await;
    assert_eq!(threads.as_array().expect("array").len(), 4);
    rig.stop(true).await;
}

#[tokio::test]
async fn many_replaced_subscriptions_in_a_row_and_replacement_while_live_obey_d058() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;
    let thread = wire.start_thread(native(), Some("go")).await.thread_id;
    let mut script = rig.script().await;
    script.command().await;
    script.started("native-d058").await;
    script.turn(1).await;

    let chunk = "z".repeat(1_500);
    for _ in 0..2_000 {
        script.delta("m", &chunk).await;
    }
    wire.thread_when(&thread, |s| s.last_event >= Some(EventId(2_000)))
        .await;
    let all_first_batch: Vec<EventId> = wire.history(&thread).await.iter().map(|e| e.id).collect();
    assert!(all_first_batch.len() >= 2_002);

    let mut client = rig.dev().await;
    let mut req_ids = Vec::new();
    for step in 0..25usize {
        let req_id = client.request_id();
        req_ids.push(req_id);
        let after = Some(all_first_batch[step * 50]);
        client
            .send(&ClientMessage::Subscribe {
                request_id: req_id,
                thread_id: thread.clone(),
                after,
            })
            .await;
        if step % 5 == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    let final_req = *req_ids.last().expect("final req");
    let final_after = all_first_batch[24 * 50];

    loop {
        match client.read().await.expect("connection stays open") {
            ServerMessage::Response { request_id, result } if request_id == final_req => {
                assert!(matches!(
                    result,
                    atrium_protocol::wire::CommandResult::Ok { .. }
                ));
                break;
            }
            ServerMessage::Response { .. }
            | ServerMessage::Event { .. }
            | ServerMessage::ReplayDone { .. } => {}
            other => panic!("unexpected before final response: {other:?}"),
        }
    }

    let mut received = Vec::new();
    loop {
        match client.read().await.expect("connection stays open") {
            ServerMessage::Event { event } => {
                assert_eq!(event.thread_id, thread);
                assert!(
                    event.id > final_after,
                    "stale event {:?} <= {:?} leaked after final Subscribe response!",
                    event.id,
                    final_after
                );
                received.push(event.id);
            }
            ServerMessage::ReplayDone { thread_id, last } => {
                assert_eq!(thread_id, thread);
                assert_eq!(last, all_first_batch.last().copied());
                break;
            }
            other => panic!("unexpected after final response: {other:?}"),
        }
    }
    let expected: Vec<EventId> = all_first_batch
        .iter()
        .copied()
        .filter(|&id| id > final_after)
        .collect();
    assert_eq!(received, expected);

    for i in 0..40 {
        script.delta("m", &format!("live-{i}-{chunk}")).await;
    }
    let target_last = EventId(all_first_batch.last().expect("last").0 + 40);
    wire.thread_when(&thread, |s| s.last_event >= Some(target_last))
        .await;

    let all_with_live: Vec<EventId> = wire.history(&thread).await.iter().map(|e| e.id).collect();
    let live_mid = all_with_live[all_with_live.len() - 15];

    let replace_live_req = client.request_id();
    client
        .send(&ClientMessage::Subscribe {
            request_id: replace_live_req,
            thread_id: thread.clone(),
            after: Some(live_mid),
        })
        .await;

    loop {
        match client.read().await.expect("open") {
            ServerMessage::Response { request_id, result } if request_id == replace_live_req => {
                assert!(matches!(
                    result,
                    atrium_protocol::wire::CommandResult::Ok { .. }
                ));
                break;
            }
            ServerMessage::Event { event } => {
                assert!(event.id > *all_first_batch.last().unwrap());
            }
            other => panic!("unexpected before live-replace response: {other:?}"),
        }
    }

    let mut after_live_replace = Vec::new();
    loop {
        match client.read().await.expect("open") {
            ServerMessage::Event { event } => {
                assert!(
                    event.id > live_mid,
                    "live event {:?} <= {:?} leaked after replacement Subscribe response!",
                    event.id,
                    live_mid
                );
                after_live_replace.push(event.id);
            }
            ServerMessage::ReplayDone { thread_id, last } => {
                assert_eq!(thread_id, thread);
                assert_eq!(last, all_with_live.last().copied());
                break;
            }
            other => panic!("unexpected after live-replace response: {other:?}"),
        }
    }
    let expected_live: Vec<EventId> = all_with_live
        .iter()
        .copied()
        .filter(|&id| id > live_mid)
        .collect();
    assert_eq!(after_live_replace, expected_live);

    script.turn_done(1).await;
    script.ended(Some(0)).await;
    rig.stop(true).await;
}

#[tokio::test]
async fn failing_store_read_during_replay_closes_the_subscriber_connection() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;
    let thread = wire.start_thread(native(), Some("go")).await.thread_id;
    let mut script = rig.script().await;
    script.command().await;
    script.started("native-corrupt").await;
    script.turn(1).await;
    script.delta("m", "hello").await;
    script.turn_done(1).await;
    wire.thread_when(&thread, |s| s.state == ThreadState::Idle)
        .await;

    const CORRUPT_SQL: &[Migration] = &[
        Migration {
            version: 1,
            sql: "",
        },
        Migration {
            version: 2,
            sql: "UPDATE events SET payload = '{not-valid-json' WHERE event_id = 2; \
                  CREATE TEMP TRIGGER keep_v1 AFTER INSERT ON main.schema_migrations \
                  BEGIN DELETE FROM schema_migrations WHERE version > 1; END;",
        },
    ];
    let temp_store = SqliteStore::open_with(&rig.paths.store, StoreOptions::default(), CORRUPT_SQL)
        .await
        .expect("applies corruption SQL");
    drop(temp_store);

    let mut sub_client = rig.dev().await;
    let req_id = sub_client.request_id();
    sub_client
        .send(&ClientMessage::Subscribe {
            request_id: req_id,
            thread_id: thread.clone(),
            after: None,
        })
        .await;

    assert!(sub_client.closed().await);

    assert!(wire.ok(DaemonCommand::ListThreads).await.is_array());
    script.ended(Some(0)).await;
    rig.stop(true).await;
}

struct SlowProbeAdapter {
    entered: AtomicUsize,
    gate: Arc<Notify>,
}

impl AgentAdapter for SlowProbeAdapter {
    fn provider(&self) -> Provider {
        Provider::Claude
    }

    fn tested_versions(&self) -> &'static [&'static str] {
        &[]
    }

    fn probe<'a>(&'a self, _: &'a SessionConfig) -> BoxFuture<'a, EnforcementDescriptor> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        let gate = self.gate.clone();
        Box::pin(async move {
            gate.notified().await;
            EnforcementDescriptor::evaluate(
                Provider::Claude,
                Some("0.0.0-slow".into()),
                false,
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
        })
    }

    fn start(
        &self,
        _: SessionConfig,
        _: Transport,
    ) -> BoxFuture<'_, Result<Session, AdapterError>> {
        Box::pin(async { Err(AdapterError::Handshake("unused".into())) })
    }
}

#[tokio::test]
async fn slow_command_limit_runs_eight_beside_readers_and_cancels_on_disconnect() {
    let gate = Arc::new(Notify::new());
    let slow_adapter = Arc::new(SlowProbeAdapter {
        entered: AtomicUsize::new(0),
        gate: gate.clone(),
    });

    let mut rig = Rig::start_with(|options| {
        options.adapters = vec![slow_adapter.clone()];
    })
    .await;

    let mut holders = Vec::new();
    for _ in 0..8 {
        let mut w = rig.dev().await;
        let req_id = w.request_id();
        w.send(&ClientMessage::Command {
            request_id: req_id,
            command: DaemonCommand::ProbeProviders {
                preset: Preset::Home,
            },
        })
        .await;
        holders.push((w, req_id));
    }
    until(|| slow_adapter.entered.load(Ordering::SeqCst) == 8).await;

    for (w, _) in &mut holders {
        assert!(w.ok(DaemonCommand::ListThreads).await.is_array());
    }

    let mut ninth = rig.dev().await;
    let ninth_probe_req = ninth.request_id();
    ninth
        .send(&ClientMessage::Command {
            request_id: ninth_probe_req,
            command: DaemonCommand::ProbeProviders {
                preset: Preset::Home,
            },
        })
        .await;
    until(|| slow_adapter.entered.load(Ordering::SeqCst) == 9).await;

    let ninth_fast_req = ninth.request_id();
    ninth
        .send(&ClientMessage::Command {
            request_id: ninth_fast_req,
            command: DaemonCommand::ListThreads,
        })
        .await;
    let waited = tokio::time::timeout(Duration::from_millis(100), ninth.read()).await;
    assert!(
        waited.is_err(),
        "9th slow command must block its own reader"
    );

    let mut tenth = rig.dev().await;
    assert!(tenth.ok(DaemonCommand::ListThreads).await.is_array());

    let (dropped_conn, _) = holders.pop().expect("a holder");
    drop(dropped_conn);

    gate.notify_waiters();
    for (mut w, req_id) in holders {
        let res = w.response(req_id).await.expect("probe ok");
        assert!(res.is_array());
    }
    let res9 = ninth.response(ninth_probe_req).await.expect("9th probe ok");
    assert!(res9.is_array());
    let fast9 = ninth.response(ninth_fast_req).await.expect("9th fast ok");
    assert!(fast9.is_array());

    rig.stop(true).await;
}
