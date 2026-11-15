//! Crash recovery (contract §5, §10): a daemon killed with sessions alive,
//! and the next one started in the same home.

mod common;

use std::process::{Command, Stdio};
use std::sync::Arc;

use atrium_core::store::{Migration, SqliteStore, StoreOptions};
use atrium_core::{NewThread, OpenItems, Store};
use atrium_daemon::{Daemon, Options, StartError};
use atrium_protocol::agent::{
    AgentEvent, AgentEventEnvelope, ApprovalDecision, MessageKind, Provider, SessionCommand,
    ToolKind,
};
use atrium_protocol::policy::Preset;
use atrium_protocol::thread::{Cleanup, Location, ThreadId, ThreadState};
use atrium_protocol::wire::{DaemonCommand, ErrorCode};
use common::{Rig, Scripted, Start, Wire, kinds, native, soon, until};

/// Set for the daemon process that the test kills.
const CHILD_HOME: &str = "ATRIUM_TEST_CRASH_HOME";
const PID_FILE: &str = "provider.pid";

/// Not a test of its own: run by `a_killed_daemon_is_recovered_at_the_next_
/// start` as a separate process, it is the daemon that gets killed. Its
/// first session has a real provider process; the second has none. Each
/// is left mid-turn with an open message and a pending approval.
#[test]
fn crash_child() {
    let Some(root) = std::env::var_os(CHILD_HOME) else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    runtime.block_on(async {
        let home = root.join("home");
        std::fs::create_dir_all(&home).expect("home");
        let (adapter, mut scripts) = Scripted::new();
        *adapter.start.lock().expect("lock") = Start::Process;
        let mut options = Options::new(common::paths(&root), home);
        options.dev_socket = true;
        options.adapters = vec![adapter.clone() as Arc<dyn atrium_agents::AgentAdapter>];
        let _daemon = Daemon::start(options).await.expect("the daemon starts");
        let mut sessions = Vec::new();
        while let Some(mut script) = scripts.recv().await {
            if let Some(process) = &script.process {
                std::fs::write(root.join(PID_FILE), process.identity.pid.to_string())
                    .expect("the pid file");
            }
            // The fourth never finishes starting.
            *adapter.start.lock().expect("lock") = if sessions.len() < 2 {
                Start::Script
            } else {
                Start::Hang
            };
            script.command().await;
            script
                .started(&format!("native-{}", sessions.len() + 1))
                .await;
            script.turn(1).await;
            script.delta("m", "partial").await;
            script.ask(1, "x.txt").await;
            sessions.push(script);
        }
    });
}

fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

async fn request_of(wire: &mut Wire, thread: &ThreadId) -> String {
    wire.history(thread)
        .await
        .iter()
        .find_map(|event| match &event.envelope.event {
            AgentEvent::ApprovalRequested { request_id, .. } => Some(request_id.clone()),
            _ => None,
        })
        .expect("an approval")
}

const RECOVERED: [&str; 8] = [
    "session_started",
    "turn_started",
    "message_delta",
    "approval_requested",
    "message_completed",
    "request_resolved",
    "turn_completed",
    "session_ended",
];

#[tokio::test]
async fn a_killed_daemon_is_recovered_at_the_next_start() {
    let folder = common::folder();
    let paths = common::paths(folder.path());
    let mut child = Command::new(std::env::current_exe().expect("this binary"))
        .args(["crash_child", "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD_HOME, folder.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .expect("the daemon process starts");
    until(|| paths.socket.exists()).await;

    let mut wire = Wire::dev(&paths.socket).await;
    let with_process = wire.start_thread(native(), Some("one")).await.thread_id;
    wire.thread_when(&with_process, |summary| summary.pending_approvals == 1)
        .await;
    let plain = wire.start_thread(native(), Some("two")).await.thread_id;
    wire.thread_when(&plain, |summary| summary.pending_approvals == 1)
        .await;
    // A third thread, which the user stops: it is `Ended` at once, and the
    // daemon is killed before its session has closed anything.
    let stopped = wire.start_thread(native(), Some("three")).await.thread_id;
    wire.thread_when(&stopped, |summary| summary.pending_approvals == 1)
        .await;
    wire.ok(DaemonCommand::ThreadCommand {
        thread_id: stopped.clone(),
        command: SessionCommand::Shutdown,
    })
    .await;
    assert_eq!(wire.thread(&stopped).await.state, ThreadState::Ended);
    let stopped_request = request_of(&mut wire, &stopped).await;
    // A fourth, whose provider never finishes starting.
    let starting = wire.start_thread(native(), Some("four")).await.thread_id;
    assert_eq!(wire.thread(&starting).await.state, ThreadState::Starting);
    let requests = [
        request_of(&mut wire, &with_process).await,
        request_of(&mut wire, &plain).await,
    ];
    let pid: u32 = std::fs::read_to_string(folder.path().join(PID_FILE))
        .expect("the pid file")
        .parse()
        .expect("a pid");
    assert!(alive(pid));

    // `kill -9`: nothing is closed, nothing is cleaned up.
    child.kill().expect("kills");
    child.wait().expect("reaps");
    drop(wire);
    assert!(alive(pid), "the provider outlived its daemon");
    assert!(paths.socket.exists(), "the stale socket is still there");

    // The next daemon recovers before it accepts anyone.
    let mut rig = Rig::in_folder(folder);
    rig.boot(|_| {}).await;
    assert!(!alive(pid), "the provider's group was killed");
    let mut wire = rig.dev().await;
    for (thread, request) in [(&with_process, &requests[0]), (&plain, &requests[1])] {
        let summary = wire.thread(thread).await;
        assert_eq!(
            summary.state,
            ThreadState::Interrupted {
                cleanup: Cleanup::Verified
            }
        );
        assert_eq!(summary.pending_approvals, 0);
        assert_eq!(kinds(&wire.history(thread).await), RECOVERED);
        // Every request of before the restart is over.
        assert_eq!(
            wire.refused(DaemonCommand::ResolveApproval {
                thread_id: thread.clone(),
                incarnation: 1,
                request_id: request.clone(),
                decision: ApprovalDecision::Allow,
                remember: None,
            })
            .await,
            ErrorCode::Expired
        );
    }

    // The thread the user had stopped keeps its state, and is closed like
    // the others.
    assert_eq!(wire.thread(&stopped).await.state, ThreadState::Ended);
    assert_eq!(wire.thread(&stopped).await.pending_approvals, 0);
    assert_eq!(kinds(&wire.history(&stopped).await), RECOVERED);
    assert_eq!(
        wire.refused(DaemonCommand::ResolveApproval {
            thread_id: stopped.clone(),
            incarnation: 1,
            request_id: stopped_request,
            decision: ApprovalDecision::Allow,
            remember: None,
        })
        .await,
        ErrorCode::Expired
    );

    // Nothing says whether the fourth thread's provider had been started:
    // it is interrupted, and not called stopped.
    assert_eq!(
        wire.thread(&starting).await.state,
        ThreadState::Interrupted {
            cleanup: Cleanup::Unverified
        }
    );
    assert_eq!(kinds(&wire.history(&starting).await), ["session_ended"]);

    // The closing events are stored once: another start adds nothing.
    drop(wire);
    rig.stop(true).await;
    rig.boot(|_| {}).await;
    let mut wire = rig.dev().await;
    for thread in [&with_process, &plain, &stopped] {
        assert_eq!(kinds(&wire.history(thread).await), RECOVERED);
    }
    assert_eq!(kinds(&wire.history(&starting).await), ["session_ended"]);

    // And the thread can be resumed from its native session.
    let resumed = wire
        .ok(DaemonCommand::ResumeThread {
            thread_id: with_process.clone(),
        })
        .await;
    assert_eq!(resumed["incarnation"], 2);
    let mut script = rig.script().await;
    assert_eq!(script.config.resume.as_deref(), Some("native-1"));
    script.started("native-1").await;
    wire.thread_when(&with_process, |summary| summary.state == ThreadState::Idle)
        .await;
    wire.ok(DaemonCommand::ThreadCommand {
        thread_id: with_process.clone(),
        command: SessionCommand::SendUserMessage {
            text: "carry on".into(),
        },
    })
    .await;
    assert_eq!(
        soon(script.commands.recv()).await,
        Some(SessionCommand::SendUserMessage {
            text: "carry on".into()
        })
    );
}

#[tokio::test]
async fn a_second_daemon_in_the_same_home_is_refused() {
    let mut rig = Rig::start().await;
    let second = Daemon::start(rig.options()).await;
    assert!(
        matches!(
            second,
            Err(atrium_daemon::StartError::Lifecycle(
                atrium_host::lifecycle::LifecycleError::AlreadyRunning(Some(pid))
            )) if pid == std::process::id()
        ),
        "{:?}",
        second.err().map(|error| error.to_string())
    );
    // The first is untouched: its socket still answers.
    let mut wire = rig.dev().await;
    assert!(wire.ok(DaemonCommand::ListThreads).await.is_array());
    rig.stop(true).await;
}

async fn assert_recorded_processes_count(store_path: &std::path::Path, expected: usize) {
    const CHECK_ZERO: &[Migration] = &[
        Migration {
            version: 1,
            sql: "",
        },
        Migration {
            version: 2,
            sql: "CREATE TEMP TABLE _check_proc (x INTEGER CHECK (x = 0)); \
                  INSERT INTO _check_proc VALUES ((SELECT COUNT(*) FROM sessions WHERE process IS NOT NULL)); \
                  CREATE TEMP TRIGGER keep_v1 AFTER INSERT ON main.schema_migrations \
                  BEGIN DELETE FROM schema_migrations WHERE version > 1; END;",
        },
    ];
    const CHECK_ONE: &[Migration] = &[
        Migration {
            version: 1,
            sql: "",
        },
        Migration {
            version: 2,
            sql: "CREATE TEMP TABLE _check_proc (x INTEGER CHECK (x = 1)); \
                  INSERT INTO _check_proc VALUES ((SELECT COUNT(*) FROM sessions WHERE process IS NOT NULL)); \
                  CREATE TEMP TRIGGER keep_v1 AFTER INSERT ON main.schema_migrations \
                  BEGIN DELETE FROM schema_migrations WHERE version > 1; END;",
        },
    ];
    let migrations = match expected {
        0 => CHECK_ZERO,
        1 => CHECK_ONE,
        other => panic!("unexpected expected process count {other}"),
    };
    let store = SqliteStore::open_with(store_path, StoreOptions::default(), migrations)
        .await
        .expect("process count matches expected");
    drop(store);
}

#[tokio::test]
async fn an_archived_unfinished_thread_is_closed_on_next_start() {
    let folder = common::folder();
    let paths = common::paths(folder.path());
    let mut child = Command::new(std::env::current_exe().expect("this binary"))
        .args(["crash_child", "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD_HOME, folder.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .expect("the daemon process starts");
    until(|| paths.socket.exists()).await;

    let mut wire = Wire::dev(&paths.socket).await;
    let first = wire.start_thread(native(), Some("one")).await.thread_id;
    wire.thread_when(&first, |summary| summary.pending_approvals == 1)
        .await;
    let pid: u32 = std::fs::read_to_string(folder.path().join(PID_FILE))
        .expect("the pid file")
        .parse()
        .expect("a pid");
    assert!(alive(pid));

    // Second session has no process record; add an open message, turn and
    // pending approval, then archive it and kill the daemon before SessionEnded.
    let archived = wire.start_thread(native(), Some("two")).await.thread_id;
    wire.thread_when(&archived, |summary| summary.pending_approvals == 1)
        .await;
    let req_id = request_of(&mut wire, &archived).await;
    wire.ok(DaemonCommand::ArchiveThread {
        thread_id: archived.clone(),
    })
    .await;

    child.kill().expect("kills");
    child.wait().expect("reaps");
    drop(wire);

    // Before recovery: `threads()` omits archived threads, while
    // `unfinished_incarnations()` includes them (including one with an open tool call).
    let with_open_tool = {
        let store = SqliteStore::open(&paths.store, StoreOptions::default())
            .await
            .expect("opens store");
        let tool_thread = store
            .create_thread(NewThread {
                provider: Provider::Claude,
                model: None,
                effort: None,
                preset: Preset::Home,
                location: Location::Home,
                title: "archived-tool".into(),
            })
            .await
            .expect("creates tool thread")
            .thread_id;
        let inc = store
            .begin_incarnation(&tool_thread)
            .await
            .expect("begins inc");
        let env = |seq, event| AgentEventEnvelope {
            incarnation: inc,
            seq,
            at_ms: 1,
            event,
        };
        store
            .append(
                &tool_thread,
                inc,
                vec![
                    env(0, AgentEvent::TurnStarted { turn: 1 }),
                    env(
                        1,
                        AgentEvent::MessageDelta {
                            message_id: "m".into(),
                            kind: MessageKind::Assistant,
                            text: "part".into(),
                        },
                    ),
                    env(
                        2,
                        AgentEvent::ToolCallStarted {
                            call_id: "c1".into(),
                            name: "Bash".into(),
                            kind: ToolKind::Execute,
                            title: None,
                            input: serde_json::json!({}),
                        },
                    ),
                ],
            )
            .await
            .expect("appends");
        store.archive(&tool_thread).await.expect("archives");
        let listed: Vec<_> = store
            .threads()
            .await
            .expect("threads")
            .into_iter()
            .map(|s| s.thread_id)
            .collect();
        assert_eq!(listed, vec![first.clone()]);
        let unfinished = store.unfinished_incarnations().await.expect("unfinished");
        assert!(unfinished.contains(&(archived.clone(), 1)));
        assert!(unfinished.contains(&(tool_thread.clone(), 1)));
        tool_thread
    };

    let mut rig = Rig::in_folder(folder);
    rig.boot(|_| {}).await;
    assert!(!alive(pid));
    let mut wire = rig.dev().await;
    assert_eq!(
        wire.refused(DaemonCommand::ResolveApproval {
            thread_id: archived.clone(),
            incarnation: 1,
            request_id: req_id,
            decision: ApprovalDecision::Allow,
            remember: None,
        })
        .await,
        ErrorCode::Expired
    );
    drop(wire);
    rig.stop(true).await;

    let store = SqliteStore::open(&rig.paths.store, StoreOptions::default())
        .await
        .expect("opens store");
    assert!(
        store
            .unfinished_incarnations()
            .await
            .expect("unfinished")
            .is_empty()
    );
    assert_eq!(
        store.open_items(&archived, 1).await.expect("open_items"),
        OpenItems::default()
    );
    let events = store
        .events_after(&archived, None, 100)
        .await
        .expect("events");
    let event_kinds: Vec<&str> = events
        .iter()
        .map(|stored| match &stored.envelope.event {
            AgentEvent::SessionStarted { .. } => "session_started",
            AgentEvent::TurnStarted { .. } => "turn_started",
            AgentEvent::MessageDelta { .. } => "message_delta",
            AgentEvent::ApprovalRequested { .. } => "approval_requested",
            AgentEvent::MessageCompleted { .. } => "message_completed",
            AgentEvent::RequestResolved { .. } => "request_resolved",
            AgentEvent::TurnCompleted { .. } => "turn_completed",
            AgentEvent::SessionEnded { .. } => "session_ended",
            _ => "other",
        })
        .collect();
    assert_eq!(event_kinds, RECOVERED);
    assert_eq!(
        store
            .open_items(&with_open_tool, 1)
            .await
            .expect("open_items"),
        OpenItems::default()
    );
    let tool_events: Vec<&str> = store
        .events_after(&with_open_tool, None, 100)
        .await
        .expect("events")
        .iter()
        .map(|stored| match &stored.envelope.event {
            AgentEvent::TurnStarted { .. } => "turn_started",
            AgentEvent::MessageDelta { .. } => "message_delta",
            AgentEvent::ToolCallStarted { .. } => "tool_call_started",
            AgentEvent::MessageCompleted { .. } => "message_completed",
            AgentEvent::ToolCallFinished { .. } => "tool_call_finished",
            AgentEvent::TurnCompleted { .. } => "turn_completed",
            AgentEvent::SessionEnded { .. } => "session_ended",
            _ => "other",
        })
        .collect();
    assert_eq!(
        tool_events,
        [
            "turn_started",
            "message_delta",
            "tool_call_started",
            "message_completed",
            "tool_call_finished",
            "turn_completed",
            "session_ended",
        ]
    );
    drop(store);
    assert_recorded_processes_count(&rig.paths.store, 0).await;
}

#[tokio::test]
async fn failed_recovery_closure_retries_and_clears_process_record() {
    let folder = common::folder();
    let paths = common::paths(folder.path());
    let mut child = Command::new(std::env::current_exe().expect("this binary"))
        .args(["crash_child", "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD_HOME, folder.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .expect("the daemon process starts");
    until(|| paths.socket.exists()).await;

    let mut wire = Wire::dev(&paths.socket).await;
    let thread = wire.start_thread(native(), Some("one")).await.thread_id;
    wire.thread_when(&thread, |summary| summary.pending_approvals == 1)
        .await;
    let req_id = request_of(&mut wire, &thread).await;
    let pid: u32 = std::fs::read_to_string(folder.path().join(PID_FILE))
        .expect("the pid file")
        .parse()
        .expect("a pid");
    assert!(alive(pid));

    child.kill().expect("kills");
    child.wait().expect("reaps");
    drop(wire);

    // Fail `close_incarnation`'s INSERT ON events AFTER `Signal::Recovered` commits `ended_ms`.
    const FAIL_APPEND_SQL: &[Migration] = &[
        Migration {
            version: 1,
            sql: "",
        },
        Migration {
            version: 2,
            sql: "CREATE TRIGGER fail_recovery_append BEFORE INSERT ON events \
                  BEGIN SELECT RAISE(ABORT, 'simulated recovery append failure'); END; \
                  CREATE TEMP TRIGGER keep_v1 AFTER INSERT ON main.schema_migrations \
                  BEGIN DELETE FROM schema_migrations WHERE version > 1; END;",
        },
    ];
    let temp = SqliteStore::open_with(&paths.store, StoreOptions::default(), FAIL_APPEND_SQL)
        .await
        .expect("installs trigger");
    drop(temp);

    let mut rig = Rig::in_folder(folder);
    let failed_start = Daemon::start(rig.options()).await;
    assert!(
        matches!(failed_start, Err(StartError::Store(_))),
        "expected StartError::Store, got {:?}",
        failed_start.err()
    );
    assert!(!alive(pid));

    // Because Signal::Recovered set `ended_ms`, `live_processes` skips it,
    // while `unfinished_incarnations` finds it and `sessions.process` stays set.
    {
        let store = SqliteStore::open(&rig.paths.store, StoreOptions::default())
            .await
            .expect("opens store");
        assert!(store.live_processes().await.expect("live").is_empty());
        assert_eq!(
            store.unfinished_incarnations().await.expect("unfinished"),
            vec![(thread.clone(), 1)]
        );
    }
    assert_recorded_processes_count(&rig.paths.store, 1).await;

    // Remove the trigger and start again: recovery finishes closing and clears `sessions.process`.
    const DROP_TRIGGER_SQL: &[Migration] = &[
        Migration {
            version: 1,
            sql: "",
        },
        Migration {
            version: 2,
            sql: "DROP TRIGGER fail_recovery_append; \
                  CREATE TEMP TRIGGER keep_v1 AFTER INSERT ON main.schema_migrations \
                  BEGIN DELETE FROM schema_migrations WHERE version > 1; END;",
        },
    ];
    let temp = SqliteStore::open_with(&rig.paths.store, StoreOptions::default(), DROP_TRIGGER_SQL)
        .await
        .expect("drops trigger");
    drop(temp);

    rig.boot(|_| {}).await;
    let mut wire = rig.dev().await;
    let summary = wire.thread(&thread).await;
    assert_eq!(
        summary.state,
        ThreadState::Interrupted {
            cleanup: Cleanup::Verified
        }
    );
    assert_eq!(summary.pending_approvals, 0);
    assert_eq!(kinds(&wire.history(&thread).await), RECOVERED);
    assert_eq!(
        wire.refused(DaemonCommand::ResolveApproval {
            thread_id: thread.clone(),
            incarnation: 1,
            request_id: req_id,
            decision: ApprovalDecision::Allow,
            remember: None,
        })
        .await,
        ErrorCode::Expired
    );
    drop(wire);
    rig.stop(true).await;

    let store = SqliteStore::open(&rig.paths.store, StoreOptions::default())
        .await
        .expect("opens store");
    assert!(
        store
            .unfinished_incarnations()
            .await
            .expect("unfinished")
            .is_empty()
    );
    drop(store);
    assert_recorded_processes_count(&rig.paths.store, 0).await;
}
