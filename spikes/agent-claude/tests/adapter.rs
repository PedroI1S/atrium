//! Adapter tests against `fake_claude` (no network, no real CLI).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use atrium_agents::{
    AdapterError, AgentAdapter, BoxFuture, GateDecision, MAX_QUEUED_MESSAGES, McpServer,
    PermissionPolicy, SHUTDOWN_GRACE, Session, SessionConfig, ToolCheck, ToolGate, Transport,
};
use atrium_protocol::agent::{AgentEvent, SessionCommand, ToolStatus, TurnOutcome};
use serde_json::Value;
use spike_agent_claude::{ClaudeAdapter, ClaudeOptions};

const FAKE: &str = env!("CARGO_BIN_EXE_fake_claude");

struct FnGate<F>(F);

impl<F> ToolGate for FnGate<F>
where
    F: Fn(&ToolCheck) -> GateDecision + Send + Sync,
{
    fn check<'a>(&'a self, check: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        let d = (self.0)(check);
        Box::pin(async move { d })
    }
}

struct PanicGate;

impl ToolGate for PanicGate {
    fn check<'a>(&'a self, _: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        Box::pin(async { panic!("gate bug") })
    }
}

struct HangGate;

impl ToolGate for HangGate {
    fn check<'a>(&'a self, _: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        Box::pin(std::future::pending())
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("atrium-claude-test-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn config(dir: &Path, mode: &str, gate: Arc<dyn ToolGate>) -> SessionConfig {
    SessionConfig {
        cwd: dir.to_path_buf(),
        model: Some("haiku".into()),
        effort: None,
        permissions: PermissionPolicy::Ask,
        gate,
        mcp_servers: vec![],
        resume: None,
        env: vec![
            ("FAKE_MODE".into(), mode.into()),
            ("FAKE_DIR".into(), dir.display().to_string()),
        ],
        program: Some(PathBuf::from(FAKE)),
    }
}

fn allow() -> Arc<dyn ToolGate> {
    Arc::new(FnGate(|_: &ToolCheck| GateDecision::Allow))
}

async fn start(cfg: SessionConfig, opts: ClaudeOptions) -> Result<Session, AdapterError> {
    ClaudeAdapter::new(opts).start(cfg, Transport::Local).await
}

async fn wait_file(path: &Path, secs: u64) -> String {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Ok(text) = fs::read_to_string(path) {
            return text;
        }
        assert!(Instant::now() < end, "{} never appeared", path.display());
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn next_matching(
    session: &mut Session,
    secs: u64,
    pred: impl Fn(&AgentEvent) -> bool,
) -> Vec<AgentEvent> {
    let mut seen = Vec::new();
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        let left = end.saturating_duration_since(Instant::now());
        let ev = tokio::time::timeout(left, session.events.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out; saw {seen:#?}"))
            .unwrap_or_else(|| panic!("events closed; saw {seen:#?}"));
        let hit = pred(&ev.event);
        seen.push(ev.event);
        if hit {
            return seen;
        }
    }
}

fn say(text: &str) -> SessionCommand {
    SessionCommand::SendUserMessage { text: text.into() }
}

/// The saturation test (docs/contracts/m0.md §3.2): nobody reads the events,
/// yet Interrupt and Shutdown are obeyed within SHUTDOWN_GRACE, the child is
/// reaped and nothing is left running.
#[tokio::test]
async fn saturation_unread_events_still_obey_interrupt_and_shutdown() {
    let dir = scratch("saturation");
    let session = start(config(&dir, "flood", allow()), ClaudeOptions::default())
        .await
        .unwrap();
    let Session { events, commands } = session;
    commands.send(say("go")).await.unwrap();

    // Let the flood fill the event channel and the adapter's backlog.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(events.len(), events.max_capacity(), "the channel is full");
    let pid = wait_file(&dir.join("pids.txt"), 5).await;

    let t = Instant::now();
    commands.send(SessionCommand::Interrupt).await.unwrap();
    wait_file(&dir.join("interrupted"), 5).await;
    assert!(
        t.elapsed() < SHUTDOWN_GRACE,
        "interrupt reached the child in {:?}",
        t.elapsed()
    );

    let t = Instant::now();
    commands.send(SessionCommand::Shutdown).await.unwrap();
    tokio::time::timeout(SHUTDOWN_GRACE + Duration::from_secs(2), commands.closed())
        .await
        .expect("the session task ended");
    assert!(
        t.elapsed() < SHUTDOWN_GRACE + Duration::from_secs(1),
        "shutdown took {:?}",
        t.elapsed()
    );
    assert!(!alive(&pid), "child {pid} was reaped");

    // No task left behind: the reader, writer and gate tasks are gone too.
    let metrics = tokio::runtime::Handle::current().metrics();
    let end = Instant::now() + Duration::from_secs(2);
    while metrics.num_alive_tasks() > 0 && Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(metrics.num_alive_tasks(), 0, "tasks left behind");
    drop(events);
}

/// A child that ignores the interrupt is killed after SHUTDOWN_GRACE, the turn
/// ends interrupted, and the next message resumes the session in a new process.
#[tokio::test]
async fn stuck_child_is_killed_after_interrupt_and_resumed() {
    let dir = scratch("stuck");
    let mut session = start(config(&dir, "stuck", allow()), ClaudeOptions::default())
        .await
        .unwrap();
    session.commands.send(say("one")).await.unwrap();
    next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::TurnStarted { .. })
    })
    .await;
    let first_pid = wait_file(&dir.join("pids.txt"), 5).await;

    session
        .commands
        .send(SessionCommand::Interrupt)
        .await
        .unwrap();
    let t = Instant::now();
    let evs = next_matching(&mut session, 15, |e| {
        matches!(e, AgentEvent::TurnCompleted { .. })
    })
    .await;
    assert!(t.elapsed() < SHUTDOWN_GRACE + Duration::from_secs(2));
    assert!(matches!(
        evs.last(),
        Some(AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Interrupted,
            ..
        })
    ));
    assert!(!alive(&first_pid));

    session.commands.send(say("two")).await.unwrap();
    next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::TurnStarted { turn: 2 })
    })
    .await;
    let argv = fs::read_to_string(dir.join("argv.txt")).unwrap();
    let second = argv.lines().nth(1).expect("a second process was started");
    assert!(
        second.contains("--resume fake-session-1"),
        "second argv: {second}"
    );
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
    let evs = next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::SessionEnded { .. })
    })
    .await;
    assert!(matches!(evs.last(), Some(AgentEvent::SessionEnded { .. })));
}

async fn hook_answer(
    gate: Arc<dyn ToolGate>,
    deadline: Duration,
    name: &str,
) -> (Value, Vec<AgentEvent>) {
    let dir = scratch(name);
    let mut session = start(
        config(&dir, "hook", gate),
        ClaudeOptions {
            gate_deadline: deadline,
            ..ClaudeOptions::default()
        },
    )
    .await
    .unwrap();
    session.commands.send(say("read x")).await.unwrap();
    let text = wait_file(&dir.join("hook_response.json"), 10).await;
    let v: Value = serde_json::from_str(&text).unwrap();
    let evs = next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::TurnCompleted { .. })
    })
    .await;
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
    (v, evs)
}

fn decision(v: &Value) -> (&str, Option<&str>) {
    let out = &v["response"]["response"]["hookSpecificOutput"];
    assert_eq!(out["hookEventName"], "PreToolUse");
    (
        out["permissionDecision"].as_str().unwrap(),
        out["permissionDecisionReason"].as_str(),
    )
}

#[tokio::test]
async fn hook_decisions_reach_the_provider() {
    let (v, _) = hook_answer(allow(), Duration::from_secs(5), "hook-allow").await;
    assert_eq!(decision(&v).0, "allow");

    let deny: Arc<dyn ToolGate> = Arc::new(FnGate(|c: &ToolCheck| {
        assert_eq!(c.name, "Read");
        assert_eq!(c.call_id.as_deref(), Some("toolu_1"));
        GateDecision::Deny {
            reason: "nope".into(),
        }
    }));
    let (v, evs) = hook_answer(deny, Duration::from_secs(5), "hook-deny").await;
    let (d, reason) = decision(&v);
    assert_eq!(d, "deny");
    assert!(reason.unwrap().contains("nope"));
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Denied,
            ..
        }
    )));

    let ask: Arc<dyn ToolGate> = Arc::new(FnGate(|_: &ToolCheck| GateDecision::Ask));
    let (v, _) = hook_answer(ask, Duration::from_secs(5), "hook-ask").await;
    assert_eq!(decision(&v).0, "ask");
}

/// Fail closed: a gate that panics or never answers is a deny, never an error
/// response (Claude Code treats those as "no objection").
#[tokio::test]
async fn gate_failures_deny() {
    let (v, evs) = hook_answer(Arc::new(PanicGate), Duration::from_secs(5), "hook-panic").await;
    assert_eq!(decision(&v).0, "deny");
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Denied,
            ..
        }
    )));

    let t = Instant::now();
    let (v, _) = hook_answer(Arc::new(HangGate), Duration::from_millis(300), "hook-hang").await;
    assert_eq!(decision(&v).0, "deny");
    assert!(t.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn start_failures_are_errors_not_sessions() {
    let dir = scratch("start-fail");
    let mut cfg = config(&dir, "echo", allow());
    cfg.program = Some(dir.join("no-such-claude"));
    let err = start(cfg, ClaudeOptions::default()).await.unwrap_err();
    assert!(matches!(err, AdapterError::Spawn(_)), "{err}");

    let err = start(config(&dir, "exit", allow()), ClaudeOptions::default())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AdapterError::Handshake(m) if m.contains("refusing to start")),
        "{err}"
    );
}

#[tokio::test]
async fn messages_beyond_the_queue_limit_are_rejected() {
    let dir = scratch("queue");
    let mut session = start(config(&dir, "stuck", allow()), ClaudeOptions::default())
        .await
        .unwrap();
    // One running turn plus MAX_QUEUED_MESSAGES waiting, then one too many.
    for i in 0..(MAX_QUEUED_MESSAGES + 2) {
        session.commands.send(say(&format!("m{i}"))).await.unwrap();
    }
    let evs = next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::Error { fatal: false, .. })
    })
    .await;
    assert!(matches!(
        evs.last(),
        Some(AgentEvent::Error { message, .. }) if message.contains("rejected")
    ));
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
}

#[tokio::test]
async fn config_becomes_cli_arguments() {
    let dir = scratch("args");
    let mut cfg = config(&dir, "echo", allow());
    cfg.permissions = PermissionPolicy::FullAccess;
    cfg.model = Some("haiku".into());
    cfg.effort = Some("low".into());
    cfg.resume = Some("abc-123".into());
    cfg.mcp_servers = vec![McpServer {
        name: "echo".into(),
        command: PathBuf::from("/bin/echo"),
        args: vec!["hi".into()],
        env: vec![("K".into(), "V".into())],
    }];
    let session = start(
        cfg,
        ClaudeOptions {
            setting_sources: Some("project,local".into()),
            ..ClaudeOptions::default()
        },
    )
    .await
    .unwrap();
    let argv = fs::read_to_string(dir.join("argv.txt")).unwrap();
    for want in [
        "-p",
        "--input-format stream-json",
        "--output-format stream-json",
        "--permission-prompt-tool stdio",
        "--permission-mode bypassPermissions",
        "--strict-mcp-config",
        "--setting-sources project,local",
        "--model haiku",
        "--effort low",
        "--resume abc-123",
    ] {
        assert!(argv.contains(want), "missing {want:?} in {argv}");
    }
    let mcp: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("mcp-copy.json")).unwrap()).unwrap();
    assert_eq!(mcp["mcpServers"]["echo"]["command"], "/bin/echo");
    assert_eq!(mcp["mcpServers"]["echo"]["env"]["K"], "V");
    // The init request registers the PreToolUse hook callback.
    let init = fs::read_to_string(dir.join("initialize.json")).unwrap();
    assert!(init.contains("PreToolUse") && init.contains("hookCallbackIds"));

    // Ask and ReadOnly leave Claude's normal permission mode alone.
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
    let dir = scratch("args-ask");
    let session = start(config(&dir, "echo", allow()), ClaudeOptions::default())
        .await
        .unwrap();
    let argv = fs::read_to_string(dir.join("argv.txt")).unwrap();
    assert!(!argv.contains("--permission-mode"), "{argv}");
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
}

#[tokio::test]
async fn temp_mcp_config_is_removed_when_the_session_ends() {
    let dir = scratch("tmpclean");
    let mut session = start(config(&dir, "echo", allow()), ClaudeOptions::default())
        .await
        .unwrap();
    let argv = fs::read_to_string(dir.join("argv.txt")).unwrap();
    let path = argv
        .split_whitespace()
        .skip_while(|a| *a != "--mcp-config")
        .nth(1)
        .unwrap()
        .to_owned();
    assert!(Path::new(&path).exists());
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
    next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::SessionEnded { .. })
    })
    .await;
    // The channel closes with the actor, which owns the temp dir.
    assert!(session.events.recv().await.is_none());
    assert!(!Path::new(&path).exists(), "{path} should be removed");
}
