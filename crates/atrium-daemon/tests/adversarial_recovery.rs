#[path = "common/mod.rs"]
mod common;

use std::process::{Command, Stdio};
use std::sync::Arc;

use atrium_core::Store;
use atrium_core::store::{Migration, SqliteStore, StoreOptions};
use atrium_daemon::{Daemon, Options, StartError};
use atrium_protocol::agent::{AgentEvent, ApprovalDecision};
use atrium_protocol::thread::{Cleanup, InboxItem, InboxReason, ThreadId, ThreadState};
use atrium_protocol::wire::{DaemonCommand, ErrorCode};
use common::{Rig, Scripted, Start, Wire, kinds, native, until};

const CHILD_HOME: &str = "ATRIUM_TEST_ADV_CRASH_HOME";
const PID_FILE: &str = "provider.pid";

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
            *adapter.start.lock().expect("lock") = Start::Script;
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

fn kill_group(pid: u32) {
    let _ = Command::new("kill")
        .args(["-9", &format!("-{pid}")])
        .stderr(Stdio::null())
        .status();
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
async fn recovery_with_mismatched_recorded_start_time_is_unverified_and_spares_process() {
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
    assert!(alive(pid));

    // Rewrite `start_time` in the store through `SqliteStore::set_process` so `kill_recorded`
    // sees a PID/start_time mismatch and returns `Cleanup::Unverified` without killing `pid`.
    {
        let store = SqliteStore::open(&paths.store, StoreOptions::default())
            .await
            .expect("opens store");
        let procs = store.live_processes().await.expect("live_processes");
        assert_eq!(procs.len(), 1);
        let (rec_thread, rec_inc, mut identity) = procs.into_iter().next().unwrap();
        assert_eq!(rec_thread, thread);
        assert_eq!(rec_inc, 1);
        assert_eq!(identity.pid, pid);
        identity.start_time = identity.start_time.wrapping_add(1);
        store
            .set_process(&thread, 1, Some(identity))
            .await
            .expect("rewrites start_time");
    }

    let mut rig = Rig::in_folder(folder);
    rig.boot(|_| {}).await;

    // Because `start_time` did not match, recovery spared the process and marked cleanup Unverified.
    assert!(alive(pid), "mismatched start_time must not be killed");
    kill_group(pid);
    until(|| !alive(pid)).await;

    let mut wire = rig.dev().await;
    let summary = wire.thread(&thread).await;
    assert_eq!(
        summary.state,
        ThreadState::Interrupted {
            cleanup: Cleanup::Unverified
        }
    );
    assert_eq!(summary.pending_approvals, 0);
    assert_eq!(kinds(&wire.history(&thread).await), RECOVERED);

    let items: Vec<InboxItem> =
        serde_json::from_value(wire.ok(DaemonCommand::ListInbox).await).expect("inbox");
    assert!(items.iter().any(|item| {
        item.reason == InboxReason::CleanupUnverified && item.thread_id.as_ref() == Some(&thread)
    }));

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

    rig.stop(true).await;
}

#[tokio::test]
async fn failed_closure_in_recovery_fails_start_and_next_start_closes_it() {
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
    assert!(alive(pid));

    // Stage 1: Fail `Signal::Recovered` (`UPDATE OF state ON threads`) so recovery fails before
    // `ended_ms` is set; the process record stays in `live_processes()` for the next start.
    const FAIL_SIGNAL_SQL: &[Migration] = &[
        Migration {
            version: 1,
            sql: "",
        },
        Migration {
            version: 2,
            sql: "CREATE TRIGGER fail_recovery_signal BEFORE UPDATE OF state ON threads \
                  BEGIN SELECT RAISE(ABORT, 'simulated recovery signal failure'); END; \
                  CREATE TEMP TRIGGER keep_v1 AFTER INSERT ON main.schema_migrations \
                  BEGIN DELETE FROM schema_migrations WHERE version > 1; END;",
        },
    ];
    let temp_store = SqliteStore::open_with(&paths.store, StoreOptions::default(), FAIL_SIGNAL_SQL)
        .await
        .expect("installs signal trigger");
    drop(temp_store);

    let mut rig = Rig::in_folder(folder);
    let first_start = Daemon::start(rig.options()).await;
    assert!(
        matches!(first_start, Err(StartError::Store(_))),
        "expected StartError::Store, got {:?}",
        first_start.err()
    );
    assert!(!alive(pid));
    {
        let check_store = SqliteStore::open(&rig.paths.store, StoreOptions::default())
            .await
            .expect("opens store");
        assert_eq!(
            check_store
                .live_processes()
                .await
                .expect("live_processes")
                .len(),
            1,
            "process record must be kept in live_processes when recovery fails before ended_ms"
        );
    }

    // Stage 2: Swap the trigger to fail `close_incarnation`'s `INSERT ON events` (after `Signal::Recovered`).
    const SWAP_TO_APPEND_FAIL_SQL: &[Migration] = &[
        Migration {
            version: 1,
            sql: "",
        },
        Migration {
            version: 2,
            sql: "DROP TRIGGER fail_recovery_signal; \
                  CREATE TRIGGER fail_recovery_append BEFORE INSERT ON events \
                  BEGIN SELECT RAISE(ABORT, 'simulated recovery append failure'); END; \
                  CREATE TEMP TRIGGER keep_v1 AFTER INSERT ON main.schema_migrations \
                  BEGIN DELETE FROM schema_migrations WHERE version > 1; END;",
        },
    ];
    let temp_store = SqliteStore::open_with(
        &rig.paths.store,
        StoreOptions::default(),
        SWAP_TO_APPEND_FAIL_SQL,
    )
    .await
    .expect("swaps trigger");
    drop(temp_store);

    let second_start = Daemon::start(rig.options()).await;
    assert!(
        matches!(second_start, Err(StartError::Store(_))),
        "expected StartError::Store on append failure, got {:?}",
        second_start.err()
    );

    // Stage 3: Remove the trigger and start the daemon again: it closes the incarnation.
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
    let temp_store =
        SqliteStore::open_with(&rig.paths.store, StoreOptions::default(), DROP_TRIGGER_SQL)
            .await
            .expect("drops trigger");
    drop(temp_store);

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

    rig.stop(true).await;
}
