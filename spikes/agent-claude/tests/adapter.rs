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

// ---- findings from the review: malformed requests, can_use_tool, groups ----

async fn run_mode(
    mode: &str,
    name: &str,
    gate: Arc<dyn ToolGate>,
    deadline: Duration,
    file: &str,
) -> (Value, Vec<AgentEvent>) {
    let dir = scratch(name);
    let mut session = start(
        config(&dir, mode, gate),
        ClaudeOptions {
            gate_deadline: deadline,
            ..ClaudeOptions::default()
        },
    )
    .await
    .unwrap();
    session.commands.send(say("go")).await.unwrap();
    let text = wait_file(&dir.join(file), 10).await;
    let evs = next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::TurnCompleted { .. })
    })
    .await;
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
    (serde_json::from_str(&text).unwrap(), evs)
}

fn behavior(v: &Value) -> (&str, &Value) {
    let r = &v["response"]["response"];
    (r["behavior"].as_str().unwrap(), r)
}

/// A hook request we can't make sense of is a deny, never silence or an error.
#[tokio::test]
async fn malformed_and_foreign_hook_requests_are_denied() {
    for mode in ["hook-malformed", "hook-badcb"] {
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let c = called.clone();
        let gate: Arc<dyn ToolGate> = Arc::new(FnGate(move |_: &ToolCheck| {
            c.store(true, std::sync::atomic::Ordering::SeqCst);
            GateDecision::Allow
        }));
        let (v, _) = run_mode(
            mode,
            mode,
            gate,
            Duration::from_secs(5),
            "hook_response.json",
        )
        .await;
        let (d, reason) = decision(&v);
        assert_eq!(d, "deny", "{mode}");
        assert!(reason.unwrap().contains("malformed"), "{mode}");
        assert!(
            !called.load(std::sync::atomic::Ordering::SeqCst),
            "{mode}: the gate is not asked"
        );
    }
}

/// `can_use_tool` that no hook preceded goes through the gate, and every
/// failure of the gate is a deny.
#[tokio::test]
async fn can_use_tool_follows_the_gate_and_fails_closed() {
    let go = |name: &str, gate: Arc<dyn ToolGate>, deadline| {
        let name = name.to_owned();
        async move { run_mode("canuse", &name, gate, deadline, "canuse_response.json").await }
    };
    let (v, _) = go("cu-allow", allow(), Duration::from_secs(5)).await;
    let (b, r) = behavior(&v);
    assert_eq!(b, "allow");
    assert_eq!(r["updatedInput"]["command"], "ls");

    let deny: Arc<dyn ToolGate> = Arc::new(FnGate(|_: &ToolCheck| GateDecision::Deny {
        reason: "no".into(),
    }));
    let (v, evs) = go("cu-deny", deny, Duration::from_secs(5)).await;
    assert_eq!(behavior(&v).0, "deny");
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Denied,
            ..
        }
    )));

    // The failure branch of this path: a panicking and a hanging gate.
    let (v, _) = go("cu-panic", Arc::new(PanicGate), Duration::from_secs(5)).await;
    assert_eq!(behavior(&v).0, "deny");
    let (v, _) = go("cu-hang", Arc::new(HangGate), Duration::from_millis(300)).await;
    assert_eq!(behavior(&v).0, "deny");
}

async fn approve_via_can_use(mode: &str, name: &str) {
    let dir = scratch(name);
    let ask: Arc<dyn ToolGate> = Arc::new(FnGate(|_: &ToolCheck| GateDecision::Ask));
    let mut session = start(config(&dir, mode, ask), ClaudeOptions::default())
        .await
        .unwrap();
    session.commands.send(say("go")).await.unwrap();
    let evs = next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::ApprovalRequested { .. })
    })
    .await;
    let AgentEvent::ApprovalRequested {
        request_id,
        call_id,
        ..
    } = evs.last().unwrap()
    else {
        unreachable!()
    };
    // Without a tool_use_id the request is still shown (it used to hang).
    assert_eq!(call_id.is_some(), mode == "canuse");
    session
        .commands
        .send(SessionCommand::ResolveApproval {
            request_id: request_id.clone(),
            decision: atrium_protocol::agent::ApprovalDecision::Allow,
        })
        .await
        .unwrap();
    let v: Value =
        serde_json::from_str(&wait_file(&dir.join("canuse_response.json"), 10).await).unwrap();
    assert_eq!(behavior(&v).0, "allow");
    next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::TurnCompleted { .. })
    })
    .await;
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
}

#[tokio::test]
async fn an_ask_on_can_use_tool_reaches_the_client_with_or_without_an_id() {
    approve_via_can_use("canuse", "cu-ask").await;
    approve_via_can_use("canuse-noid", "cu-ask-noid").await;
}

#[tokio::test]
async fn a_question_nobody_let_through_is_not_shown() {
    // The hook never decided this AskUserQuestion; the gate does now.
    let deny: Arc<dyn ToolGate> = Arc::new(FnGate(|_: &ToolCheck| GateDecision::Deny {
        reason: "no questions".into(),
    }));
    let (v, evs) = run_mode(
        "canuse-question",
        "q-deny",
        deny,
        Duration::from_secs(5),
        "canuse_response.json",
    )
    .await;
    assert_eq!(behavior(&v).0, "deny");
    assert!(
        !evs.iter()
            .any(|e| matches!(e, AgentEvent::QuestionAsked { .. }))
    );

    let dir = scratch("q-allow");
    let mut session = start(
        config(&dir, "canuse-question", allow()),
        ClaudeOptions::default(),
    )
    .await
    .unwrap();
    session.commands.send(say("go")).await.unwrap();
    let evs = next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::QuestionAsked { .. })
    })
    .await;
    let AgentEvent::QuestionAsked {
        request_id,
        questions,
    } = evs.last().unwrap()
    else {
        unreachable!()
    };
    session
        .commands
        .send(SessionCommand::AnswerQuestion {
            request_id: request_id.clone(),
            answers: vec![atrium_protocol::agent::QuestionAnswer {
                question_id: questions[0].id.clone(),
                selected: vec!["o1".into()],
                text: None,
            }],
        })
        .await
        .unwrap();
    let v: Value =
        serde_json::from_str(&wait_file(&dir.join("canuse_response.json"), 10).await).unwrap();
    assert_eq!(
        v["response"]["response"]["updatedInput"]["answers"]["Q?"],
        "B"
    );
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
}

/// What the child started goes with it, and a grandchild that keeps the
/// pipes open can't hold the shutdown up.
#[tokio::test]
async fn shutdown_kills_the_whole_process_group() {
    let dir = scratch("group");
    let session = start(
        config(&dir, "grandchild", allow()),
        ClaudeOptions::default(),
    )
    .await
    .unwrap();
    session.commands.send(say("go")).await.unwrap();
    let grandchild = wait_file(&dir.join("grandchild.pid"), 10).await;
    let child = wait_file(&dir.join("pids.txt"), 5).await;
    assert!(alive(&grandchild) && alive(&child));

    let t = Instant::now();
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
    tokio::time::timeout(
        SHUTDOWN_GRACE + Duration::from_secs(3),
        session.commands.closed(),
    )
    .await
    .expect("the session ended");
    assert!(
        t.elapsed() < SHUTDOWN_GRACE + Duration::from_secs(2),
        "{:?}",
        t.elapsed()
    );
    // Give the kernel a moment to reap.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!alive(&child), "child survived");
    assert!(!alive(&grandchild), "grandchild survived");
}

/// A respawned process that is slow to answer `initialize` must not hold the
/// actor: Shutdown is obeyed at once.
#[tokio::test]
async fn shutdown_during_a_slow_respawn_handshake_is_prompt() {
    let dir = scratch("slow-respawn");
    let mut cfg = config(&dir, "stuck", allow());
    cfg.env.push(("FAKE_SLOW_RESUME".into(), "1".into()));
    let mut session = start(cfg, ClaudeOptions::default()).await.unwrap();
    session.commands.send(say("one")).await.unwrap();
    next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::TurnStarted { .. })
    })
    .await;
    session
        .commands
        .send(SessionCommand::Interrupt)
        .await
        .unwrap();
    next_matching(&mut session, 15, |e| {
        matches!(e, AgentEvent::TurnCompleted { .. })
    })
    .await;

    // The next message starts a new process, which sleeps 20 s before it
    // answers. Shutdown must not wait for that (nor for the 30 s startup timeout).
    session.commands.send(say("two")).await.unwrap();
    let argv_two = loop {
        if let Ok(a) = fs::read_to_string(dir.join("argv.txt"))
            && a.lines().count() == 2
        {
            break a;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(argv_two.contains("--resume"));
    let t = Instant::now();
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
    let evs = next_matching(&mut session, 12, |e| {
        matches!(e, AgentEvent::SessionEnded { .. })
    })
    .await;
    assert!(
        t.elapsed() < SHUTDOWN_GRACE + Duration::from_secs(3),
        "{:?}",
        t.elapsed()
    );
    // The second message was never written, so no second turn started.
    assert!(
        !evs.iter()
            .any(|e| matches!(e, AgentEvent::TurnStarted { .. }))
    );
}

// ---- external hooks in Claude Code's own settings ----

#[tokio::test]
async fn external_hooks_make_the_enforcement_partial_and_say_why() {
    let dir = scratch("enforcement");
    let claude = dir.join(".claude");
    fs::create_dir_all(&claude).unwrap();
    fs::write(
        claude.join("settings.local.json"),
        r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"/opt/audit.sh"}]}]}}"#,
    )
    .unwrap();
    // A user directory with no settings, so the machine's own are not read.
    let user_dir = scratch("enforcement-user");

    let mut cfg = config(&dir, "echo", allow());
    cfg.env
        .push(("CLAUDE_CONFIG_DIR".into(), user_dir.display().to_string()));

    // Native behaviour (the default): the project's hook is found.
    let adapter = ClaudeAdapter::new(ClaudeOptions::default());
    let e = adapter.enforcement_for(&dir, &cfg.env);
    assert_eq!(e.interception, atrium_agents::Interception::ProviderGated);
    assert!(e.reason.as_deref().unwrap().contains("audit.sh"));
    let session = adapter.start(cfg.clone(), Transport::Local).await.unwrap();
    assert_eq!(
        adapter.interception(),
        atrium_agents::Interception::ProviderGated
    );
    assert_eq!(adapter.last_enforcement().unwrap().external_hooks.len(), 1);
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();

    // External settings disabled: full enforcement, nothing to report.
    let adapter = ClaudeAdapter::new(ClaudeOptions {
        setting_sources: Some(String::new()),
        ..ClaudeOptions::default()
    });
    let session = adapter.start(cfg, Transport::Local).await.unwrap();
    assert_eq!(
        adapter.interception(),
        atrium_agents::Interception::AllToolCalls
    );
    assert!(adapter.last_enforcement().unwrap().reason.is_none());
    let argv = fs::read_to_string(dir.join("argv.txt")).unwrap();
    assert!(argv.lines().last().unwrap().contains("--setting-sources"));
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
}
