//! Crash recovery (contract §5, §10): a daemon killed with sessions alive,
//! and the next one started in the same home.

mod common;

use std::process::{Command, Stdio};
use std::sync::Arc;

use atrium_daemon::{Daemon, Options};
use atrium_protocol::agent::{AgentEvent, ApprovalDecision, SessionCommand};
use atrium_protocol::thread::{Cleanup, ThreadId, ThreadState};
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
