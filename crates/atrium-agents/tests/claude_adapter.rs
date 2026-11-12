//! Adapter tests against the offline stand-in for the CLI
//! (`examples/claude-fake.rs`): no network, no real `claude`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use atrium_agents::claude::{ClaudeAdapter, ClaudeOptions};
use atrium_agents::{
    AdapterError, AdapterInput, AgentAdapter, ApprovalTicket, BoxFuture, GateDecision,
    MAX_QUEUED_MESSAGES, McpServer, PermissionPolicy, SHUTDOWN_GRACE, Session, SessionConfig,
    ToolCheck, ToolGate, Transport,
};
use atrium_protocol::agent::{
    AgentEvent, ApprovalDecision, MessageBody, MessageKind, QuestionAnswer, RequestOutcome,
    SessionCommand, ToolKind, ToolStatus, TurnOutcome,
};
use atrium_protocol::policy::{ConfigSourceKind, EnforcementLevel, ReasonCode, ToolSubject};
use serde_json::Value;

fn fake() -> PathBuf {
    // Examples are built next to the test binaries (`target/<profile>/examples`).
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().parent().unwrap();
    let path = dir.join("examples/claude-fake");
    assert!(
        path.exists(),
        "build the examples first: {}",
        path.display()
    );
    path
}

type CheckFn = dyn Fn(&ToolCheck) -> GateDecision + Send + Sync;
type RedeemFn = dyn Fn(&ApprovalTicket, &ToolCheck) -> GateDecision + Send + Sync;

/// A scripted gate that remembers what it was asked.
struct TestGate {
    check: Box<CheckFn>,
    redeem: Box<RedeemFn>,
    question: Option<ApprovalTicket>,
    checks: Mutex<Vec<ToolCheck>>,
    redeems: Mutex<Vec<(ApprovalTicket, ToolCheck)>>,
    question_turns: Mutex<Vec<Option<u32>>>,
}

impl TestGate {
    fn new(check: impl Fn(&ToolCheck) -> GateDecision + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self::build(check, GateDecision::Allow))
    }

    fn build(
        check: impl Fn(&ToolCheck) -> GateDecision + Send + Sync + 'static,
        redeem: GateDecision,
    ) -> Self {
        Self {
            check: Box::new(check),
            redeem: Box::new(move |_, _| redeem.clone()),
            question: Some(ticket("Q1", false)),
            checks: Mutex::default(),
            redeems: Mutex::default(),
            question_turns: Mutex::default(),
        }
    }

    /// Asks for every call; redeem answers `redeem`.
    fn asking(redeem: GateDecision) -> Arc<Self> {
        Arc::new(Self::build(|_| ask(), redeem))
    }

    fn redeems(&self) -> Vec<(ApprovalTicket, ToolCheck)> {
        self.redeems.lock().unwrap().clone()
    }
}

fn ticket(id: &str, rememberable: bool) -> ApprovalTicket {
    ApprovalTicket {
        request_id: id.into(),
        rememberable,
    }
}

fn ask() -> GateDecision {
    GateDecision::Ask {
        ticket: ticket("T1", true),
    }
}

impl ToolGate for TestGate {
    fn check<'a>(&'a self, check: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        self.checks.lock().unwrap().push(check.clone());
        let d = (self.check)(check);
        Box::pin(async move { d })
    }

    fn redeem<'a>(
        &'a self,
        t: &'a ApprovalTicket,
        check: &'a ToolCheck,
    ) -> BoxFuture<'a, GateDecision> {
        self.redeems
            .lock()
            .unwrap()
            .push((t.clone(), check.clone()));
        let d = (self.redeem)(t, check);
        Box::pin(async move { d })
    }

    fn question_ticket(&self, turn: Option<u32>) -> BoxFuture<'_, Option<ApprovalTicket>> {
        self.question_turns.lock().unwrap().push(turn);
        let t = self.question.clone();
        Box::pin(async move { t })
    }
}

struct PanicGate;

impl ToolGate for PanicGate {
    fn check<'a>(&'a self, _: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        Box::pin(async { panic!("gate bug") })
    }
    fn redeem<'a>(
        &'a self,
        _: &'a ApprovalTicket,
        _: &'a ToolCheck,
    ) -> BoxFuture<'a, GateDecision> {
        Box::pin(async { panic!("gate bug") })
    }
    fn question_ticket(&self, _: Option<u32>) -> BoxFuture<'_, Option<ApprovalTicket>> {
        Box::pin(async { panic!("gate bug") })
    }
}

struct HangGate;

impl ToolGate for HangGate {
    fn check<'a>(&'a self, _: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        Box::pin(std::future::pending())
    }
    fn redeem<'a>(
        &'a self,
        _: &'a ApprovalTicket,
        _: &'a ToolCheck,
    ) -> BoxFuture<'a, GateDecision> {
        Box::pin(std::future::pending())
    }
    fn question_ticket(&self, _: Option<u32>) -> BoxFuture<'_, Option<ApprovalTicket>> {
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
            // A user directory with nothing in it: the machine's own
            // settings are not read.
            (
                "CLAUDE_CONFIG_DIR".into(),
                dir.join("user").display().to_string(),
            ),
        ],
        program: Some(fake()),
        on_spawn: None,
    }
}

fn allow() -> Arc<dyn ToolGate> {
    TestGate::new(|_| GateDecision::Allow)
}

fn opts() -> ClaudeOptions {
    ClaudeOptions {
        managed_settings: Some(vec![]),
        ..ClaudeOptions::default()
    }
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

fn group_alive(pgid: u32) -> bool {
    alive(&format!("-{pgid}"))
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

fn turn_done(e: &AgentEvent) -> bool {
    matches!(e, AgentEvent::TurnCompleted { .. })
}

fn ended(e: &AgentEvent) -> bool {
    matches!(e, AgentEvent::SessionEnded { .. })
}

async fn shutdown(mut session: Session) -> Vec<AgentEvent> {
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
    next_matching(&mut session, 10, ended).await
}

/// The saturation test (contract M0 §3.2, brief M1-07): nobody reads the
/// events, yet Interrupt and Shutdown are obeyed within SHUTDOWN_GRACE, the
/// whole process group is gone, and `SessionEnded` is delivered once the
/// receiver drains.
#[tokio::test]
async fn saturation_unread_events_still_obey_interrupt_and_shutdown() {
    let dir = scratch("saturation");
    let mut cfg = config(&dir, "flood", allow());
    cfg.env.push(("FAKE_GRANDCHILD".into(), "1".into()));
    let session = start(cfg, opts()).await.unwrap();
    let process = session.process.clone().expect("the process is reported");
    let Session {
        mut events,
        commands,
        ..
    } = session;
    commands.send(say("go")).await.unwrap();

    // Let the flood fill the event channel and the adapter's backlog.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(events.len(), events.max_capacity(), "the channel is full");
    let pid = wait_file(&dir.join("pids.txt"), 5).await;
    let grandchild = wait_file(&dir.join("grandchild.pid"), 5).await;
    assert!(alive(&grandchild) && group_alive(process.pgid));

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
        .expect("the session stopped taking commands");
    assert!(
        t.elapsed() < SHUTDOWN_GRACE + Duration::from_secs(1),
        "shutdown took {:?}",
        t.elapsed()
    );
    assert!(!alive(&pid), "child {pid} was reaped");
    assert!(!alive(&grandchild), "grandchild {grandchild} is gone");
    assert!(!group_alive(process.pgid), "the process group is gone");

    // Now the receiver drains: the closing events and `SessionEnded` arrive,
    // exactly once, last.
    let mut last = None;
    let mut ended_count = 0;
    let mut seqs = Vec::new();
    while let Ok(Some(env)) = tokio::time::timeout(Duration::from_secs(10), events.recv()).await {
        seqs.push(env.seq);
        if matches!(env.event, AgentEvent::SessionEnded { .. }) {
            ended_count += 1;
        }
        last = Some(env.event);
    }
    assert_eq!(ended_count, 1);
    assert!(matches!(last, Some(AgentEvent::SessionEnded { .. })));
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "no gaps");

    // No task left behind.
    let metrics = tokio::runtime::Handle::current().metrics();
    let end = Instant::now() + Duration::from_secs(2);
    while metrics.num_alive_tasks() > 0 && Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(metrics.num_alive_tasks(), 0, "tasks left behind");
}

/// A child that ignores the interrupt: after SHUTDOWN_GRACE the session ends
/// (the turn interrupted, the process gone) and no second process is ever
/// started; the daemon resumes in a new incarnation.
#[tokio::test]
async fn an_interrupt_the_child_ignores_ends_the_session() {
    let dir = scratch("stuck");
    let mut session = start(config(&dir, "stuck", allow()), opts()).await.unwrap();
    session.commands.send(say("one")).await.unwrap();
    next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::TurnStarted { .. })
    })
    .await;
    let pid = wait_file(&dir.join("pids.txt"), 5).await;
    let pgid = session.process.as_ref().unwrap().pgid;

    session
        .commands
        .send(SessionCommand::Interrupt)
        .await
        .unwrap();
    let t = Instant::now();
    let evs = next_matching(&mut session, 15, ended).await;
    assert!(t.elapsed() < SHUTDOWN_GRACE + Duration::from_secs(2));
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Interrupted,
            ..
        }
    )));
    let Some(AgentEvent::SessionEnded { reason, .. }) = evs.last() else {
        panic!("{evs:?}")
    };
    assert!(reason.as_deref().unwrap().contains("interrupt"));
    assert!(!alive(&pid) && !group_alive(pgid));
    let argv = fs::read_to_string(dir.join("argv.txt")).unwrap();
    assert_eq!(argv.lines().count(), 1, "a second process was started");
    assert!(session.events.recv().await.is_none());
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
            ..opts()
        },
    )
    .await
    .unwrap();
    session.commands.send(say("read x")).await.unwrap();
    let text = wait_file(&dir.join("hook_response.json"), 10).await;
    let v: Value = serde_json::from_str(&text).unwrap();
    let mut evs = next_matching(&mut session, 10, turn_done).await;
    evs.extend(shutdown(session).await);
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

    let deny: Arc<dyn ToolGate> = TestGate::new(|c| {
        assert_eq!(c.name, "Read");
        assert_eq!(c.call_id.as_deref(), Some("toolu_1"));
        GateDecision::Deny {
            reason: "nope".into(),
        }
    });
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

    let (v, _) = hook_answer(
        TestGate::asking(GateDecision::Allow),
        Duration::from_secs(5),
        "hook-ask",
    )
    .await;
    assert_eq!(decision(&v).0, "ask");
}

/// Every check carries the extracted subject, the kind and the adapter's turn.
#[tokio::test]
async fn checks_carry_subject_kind_and_turn() {
    let dir = scratch("subject");
    let gate = TestGate::new(|_| GateDecision::Allow);
    let g = gate.clone();
    let mut session = start(config(&dir, "hook", gate), opts()).await.unwrap();
    session.commands.send(say("read x")).await.unwrap();
    wait_file(&dir.join("hook_response.json"), 10).await;
    next_matching(&mut session, 10, turn_done).await;
    shutdown(session).await;
    let checks = g.checks.lock().unwrap().clone();
    assert_eq!(checks.len(), 1);
    let c = &checks[0];
    assert_eq!(c.kind, ToolKind::Read);
    assert_eq!(c.turn, 1);
    assert_eq!(
        c.subject,
        ToolSubject::Paths {
            paths: vec!["x".into()],
            glob: None,
            recursive: false
        }
    );
}

/// Fail closed: a gate that panics or never answers is a deny, never an error
/// response (Claude Code treats those as "no objection"). The descriptor then
/// says the gate was unreachable.
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
    let changed = evs.iter().find_map(|e| match e {
        AgentEvent::EnforcementChanged { enforcement } => Some(enforcement),
        _ => None,
    });
    let changed = changed.expect("EnforcementChanged after a gate failure");
    assert!(
        changed
            .reasons
            .iter()
            .any(|r| r.code == ReasonCode::GateUnreachable)
    );

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
    let err = start(cfg, opts()).await.unwrap_err();
    assert!(matches!(err, AdapterError::Spawn(_)), "{err}");

    let err = start(config(&dir, "exit", allow()), opts())
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
    let mut session = start(config(&dir, "stuck", allow()), opts()).await.unwrap();
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
    shutdown(session).await;
}

#[tokio::test]
async fn config_becomes_cli_arguments() {
    let dir = scratch("args");
    let mut cfg = config(&dir, "echo", allow());
    cfg.permissions = PermissionPolicy::FullAccess;
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
            ..opts()
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
    shutdown(session).await;

    // Ask and ReadOnly leave Claude's normal permission mode alone.
    let dir = scratch("args-ask");
    let session = start(config(&dir, "echo", allow()), opts()).await.unwrap();
    let argv = fs::read_to_string(dir.join("argv.txt")).unwrap();
    assert!(!argv.contains("--permission-mode"), "{argv}");
    shutdown(session).await;
}

/// The generated MCP config can hold credentials: private directory and file,
/// removed when the process is gone (not only when the events are drained).
#[tokio::test]
async fn temp_mcp_config_is_private_and_removed_when_the_session_ends() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("tmpclean");
    let mut session = start(config(&dir, "echo", allow()), opts()).await.unwrap();
    let argv = fs::read_to_string(dir.join("argv.txt")).unwrap();
    let path = argv
        .split_whitespace()
        .skip_while(|a| *a != "--mcp-config")
        .nth(1)
        .unwrap()
        .to_owned();
    let file = Path::new(&path);
    assert!(file.exists());
    assert_eq!(
        fs::metadata(file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(file.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
    next_matching(&mut session, 10, ended).await;
    assert!(!file.exists(), "{path} should be removed");
}

// ---- malformed requests, can_use_tool, redeem ----

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
            ..opts()
        },
    )
    .await
    .unwrap();
    session.commands.send(say("go")).await.unwrap();
    let text = wait_file(&dir.join(file), 10).await;
    let mut evs = next_matching(&mut session, 10, turn_done).await;
    evs.extend(shutdown(session).await);
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
        let called = Arc::new(AtomicBool::new(false));
        let c = called.clone();
        let gate: Arc<dyn ToolGate> = TestGate::new(move |_| {
            c.store(true, Ordering::SeqCst);
            GateDecision::Allow
        });
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
            !called.load(Ordering::SeqCst),
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

    let deny: Arc<dyn ToolGate> = TestGate::new(|_| GateDecision::Deny {
        reason: "no".into(),
    });
    let (v, evs) = go("cu-deny", deny, Duration::from_secs(5)).await;
    assert_eq!(behavior(&v).0, "deny");
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Denied,
            ..
        }
    )));

    let (v, _) = go("cu-panic", Arc::new(PanicGate), Duration::from_secs(5)).await;
    assert_eq!(behavior(&v).0, "deny");
    let (v, _) = go("cu-hang", Arc::new(HangGate), Duration::from_millis(300)).await;
    assert_eq!(behavior(&v).0, "deny");
}

/// An `Ask` on a request the hook didn't see still reaches the client, with
/// or without a tool_use_id; the approval carries the gate's ticket.
#[tokio::test]
async fn an_ask_on_can_use_tool_reaches_the_client_with_or_without_an_id() {
    for (mode, name) in [("canuse", "cu-ask"), ("canuse-noid", "cu-ask-noid")] {
        let dir = scratch(name);
        let gate = TestGate::asking(GateDecision::Allow);
        let g = gate.clone();
        let mut session = start(config(&dir, mode, gate), opts()).await.unwrap();
        session.commands.send(say("go")).await.unwrap();
        let evs = next_matching(&mut session, 10, |e| {
            matches!(e, AgentEvent::ApprovalRequested { .. })
        })
        .await;
        let Some(AgentEvent::ApprovalRequested {
            request_id,
            call_id,
            rememberable,
            ..
        }) = evs.last()
        else {
            unreachable!()
        };
        assert_eq!(request_id, "T1", "the gate's ticket, not Claude's id");
        assert!(*rememberable);
        assert_eq!(call_id.is_some(), mode == "canuse");
        session
            .commands
            .send(SessionCommand::ResolveApproval {
                request_id: request_id.clone(),
                decision: ApprovalDecision::Allow,
            })
            .await
            .unwrap();
        let v: Value =
            serde_json::from_str(&wait_file(&dir.join("canuse_response.json"), 10).await).unwrap();
        assert_eq!(behavior(&v).0, "allow");
        assert_eq!(g.redeems().len(), 1, "{mode}: redeemed once");
        next_matching(&mut session, 10, turn_done).await;
        shutdown(session).await;
    }
}

/// The main approval path: the hook asks, `can_use_tool` follows, the user
/// allows, the adapter redeems with a check built from the `can_use_tool`
/// payload, and only then answers.
#[tokio::test]
async fn an_allowed_approval_is_redeemed_before_the_effect() {
    let dir = scratch("redeem");
    let gate = TestGate::asking(GateDecision::Allow);
    let g = gate.clone();
    let mut session = start(config(&dir, "hook-ask", gate), opts()).await.unwrap();
    session.commands.send(say("go")).await.unwrap();
    let evs = next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::ApprovalRequested { .. })
    })
    .await;
    let Some(AgentEvent::ApprovalRequested {
        request_id,
        reason,
        title,
        ..
    }) = evs.last()
    else {
        unreachable!()
    };
    assert_eq!(request_id, "T1");
    assert_eq!(reason.as_deref(), Some("asked"));
    assert_eq!(title.as_deref(), Some("echo hi"));
    // Nothing is answered before the user decides.
    assert!(!dir.join("canuse_response.json").exists());
    assert!(g.redeems().is_empty());

    session
        .commands
        .send(SessionCommand::ResolveApproval {
            request_id: "T1".into(),
            decision: ApprovalDecision::Allow,
        })
        .await
        .unwrap();
    let v: Value =
        serde_json::from_str(&wait_file(&dir.join("canuse_response.json"), 10).await).unwrap();
    assert_eq!(behavior(&v).0, "allow");
    let redeems = g.redeems();
    assert_eq!(redeems.len(), 1);
    let (t, check) = &redeems[0];
    assert_eq!(t.request_id, "T1");
    assert_eq!(check.name, "Bash");
    assert_eq!(check.kind, ToolKind::Execute);
    assert_eq!(check.call_id.as_deref(), Some("toolu_1"));
    assert_eq!(
        check.subject,
        ToolSubject::Command {
            line: "echo hi".into(),
            cwd: None
        }
    );
    assert_eq!(check.turn, 1);
    let mut evs = next_matching(&mut session, 10, turn_done).await;
    evs.extend(shutdown(session).await);
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::RequestResolved {
            outcome: RequestOutcome::Approved,
            ..
        }
    )));
}

/// A redeem that refuses (the rules changed, the input differs...) means the
/// tool does not run, even though the user said yes.
#[tokio::test]
async fn a_refused_redeem_denies_the_tool() {
    let dir = scratch("redeem-deny");
    let gate = TestGate::asking(GateDecision::Deny {
        reason: "a deny rule was added".into(),
    });
    let mut session = start(config(&dir, "hook-ask", gate), opts()).await.unwrap();
    session.commands.send(say("go")).await.unwrap();
    next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::ApprovalRequested { .. })
    })
    .await;
    session
        .commands
        .send(SessionCommand::ResolveApproval {
            request_id: "T1".into(),
            decision: ApprovalDecision::Allow,
        })
        .await
        .unwrap();
    let v: Value =
        serde_json::from_str(&wait_file(&dir.join("canuse_response.json"), 10).await).unwrap();
    let (b, r) = behavior(&v);
    assert_eq!(b, "deny");
    assert!(r["message"].as_str().unwrap().contains("deny rule"));
    let mut evs = next_matching(&mut session, 10, turn_done).await;
    evs.extend(shutdown(session).await);
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Denied,
            ..
        }
    )));
}

/// A denial never redeems; it answers deny at once.
#[tokio::test]
async fn a_denied_approval_is_never_redeemed() {
    let dir = scratch("approval-deny");
    let gate = TestGate::asking(GateDecision::Allow);
    let g = gate.clone();
    let mut session = start(config(&dir, "hook-ask", gate), opts()).await.unwrap();
    session.commands.send(say("go")).await.unwrap();
    next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::ApprovalRequested { .. })
    })
    .await;
    session
        .commands
        .send(SessionCommand::ResolveApproval {
            request_id: "T1".into(),
            decision: ApprovalDecision::Deny {
                message: Some("not now".into()),
            },
        })
        .await
        .unwrap();
    let v: Value =
        serde_json::from_str(&wait_file(&dir.join("canuse_response.json"), 10).await).unwrap();
    let (b, r) = behavior(&v);
    assert_eq!(b, "deny");
    assert_eq!(r["message"], "not now");
    assert!(g.redeems().is_empty());
    next_matching(&mut session, 10, turn_done).await;
    shutdown(session).await;
}

/// An Allow that arrives after the turn ended is never executed: no redeem,
/// nothing written to the provider, and the client gets a non-fatal error.
#[tokio::test]
async fn a_stale_allow_after_the_turn_ended_is_never_executed() {
    let dir = scratch("stale");
    let gate = TestGate::asking(GateDecision::Allow);
    let g = gate.clone();
    let mut session = start(config(&dir, "hook-ask", gate), opts()).await.unwrap();
    session.commands.send(say("go")).await.unwrap();
    next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::ApprovalRequested { .. })
    })
    .await;
    session
        .commands
        .send(SessionCommand::Interrupt)
        .await
        .unwrap();
    let evs = next_matching(&mut session, 10, turn_done).await;
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::RequestResolved {
            outcome: RequestOutcome::Cancelled,
            ..
        }
    )));
    session
        .commands
        .send(SessionCommand::ResolveApproval {
            request_id: "T1".into(),
            decision: ApprovalDecision::Allow,
        })
        .await
        .unwrap();
    let evs = next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::Error { fatal: false, .. })
    })
    .await;
    assert!(matches!(
        evs.last(),
        Some(AgentEvent::Error { message, .. }) if message.contains("already resolved")
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(g.redeems().is_empty(), "no redeem for a stale approval");
    assert!(
        !dir.join("canuse_response.json").exists(),
        "nothing was answered to the provider"
    );
    shutdown(session).await;
}

#[tokio::test]
async fn questions_use_the_gates_question_ticket() {
    // Declined: no ticket, nothing shown, the provider is told no.
    let dir = scratch("q-declined");
    let mut gate = TestGate::build(|_| GateDecision::Allow, GateDecision::Allow);
    gate.question = None;
    let gate = Arc::new(gate);
    let g = gate.clone();
    let mut session = start(config(&dir, "canuse-question", gate), opts())
        .await
        .unwrap();
    session.commands.send(say("go")).await.unwrap();
    let v: Value =
        serde_json::from_str(&wait_file(&dir.join("canuse_response.json"), 10).await).unwrap();
    assert_eq!(behavior(&v).0, "deny");
    let mut evs = next_matching(&mut session, 10, turn_done).await;
    evs.extend(shutdown(session).await);
    assert!(
        !evs.iter()
            .any(|e| matches!(e, AgentEvent::QuestionAsked { .. }))
    );
    assert_eq!(g.question_turns.lock().unwrap().as_slice(), &[Some(1)]);

    // Ticketed: asked with the ticket's id, answered with structured answers.
    let dir = scratch("q-allow");
    let mut session = start(config(&dir, "canuse-question", allow()), opts())
        .await
        .unwrap();
    session.commands.send(say("go")).await.unwrap();
    let evs = next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::QuestionAsked { .. })
    })
    .await;
    let Some(AgentEvent::QuestionAsked {
        request_id,
        questions,
    }) = evs.last()
    else {
        unreachable!()
    };
    assert_eq!(request_id, "Q1");
    session
        .commands
        .send(SessionCommand::AnswerQuestion {
            request_id: request_id.clone(),
            answers: vec![QuestionAnswer {
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
    shutdown(session).await;
}

/// The session reports its process before the handshake, and the user
/// message follows `TurnStarted` as a `User` message.
#[tokio::test]
async fn on_spawn_and_the_user_message() {
    let dir = scratch("on-spawn");
    let seen = Arc::new(AtomicUsize::new(0));
    let s = seen.clone();
    let mut cfg = config(&dir, "echo", allow());
    let pids = dir.join("pids.txt");
    cfg.on_spawn = Some(Arc::new(move |id| {
        // Reported before `initialize` was answered: the fake has not even
        // written its pid file yet or just did; the identity is what matters.
        assert!(id.pid > 1 && id.pgid == id.pid);
        s.fetch_add(1, Ordering::SeqCst);
    }));
    let mut session = start(cfg, opts()).await.unwrap();
    assert_eq!(seen.load(Ordering::SeqCst), 1);
    let identity = session.process.clone().unwrap();
    assert_eq!(identity.pgid, identity.pid);
    let _ = pids;
    session.commands.send(say("hello there")).await.unwrap();
    let evs = next_matching(&mut session, 10, turn_done).await;
    let Some(AgentEvent::SessionStarted { enforcement, .. }) = evs.first() else {
        panic!("{evs:?}")
    };
    assert_eq!(enforcement, &session.descriptor);
    assert!(matches!(evs[1], AgentEvent::TurnStarted { turn: 1 }));
    assert!(matches!(
        &evs[2],
        AgentEvent::MessageCompleted {
            kind: MessageKind::User,
            body: MessageBody::Inline { text },
            ..
        } if text == "hello there"
    ));
    shutdown(session).await;
}

/// What the child started goes with it, and a grandchild that keeps the
/// pipes open can't hold the shutdown up.
#[tokio::test]
async fn shutdown_kills_the_whole_process_group() {
    let dir = scratch("group");
    let session = start(config(&dir, "grandchild", allow()), opts())
        .await
        .unwrap();
    let pgid = session.process.as_ref().unwrap().pgid;
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
    assert!(!alive(&child), "child survived");
    assert!(!alive(&grandchild), "grandchild survived");
    assert!(!group_alive(pgid));
}

/// A record past the limit ends the session with a fatal `Error`.
#[tokio::test]
async fn an_oversize_record_ends_the_session() {
    let dir = scratch("oversize");
    let mut session = start(config(&dir, "oversize", allow()), opts())
        .await
        .unwrap();
    session.commands.send(say("go")).await.unwrap();
    let evs = next_matching(&mut session, 20, ended).await;
    assert!(
        evs.iter()
            .any(|e| matches!(e, AgentEvent::Error { fatal: true, message, .. } if message.contains("exceeds")))
    );
    let pgid = session.process.as_ref().unwrap().pgid;
    assert!(!group_alive(pgid));
}

// ---- enforcement descriptor ----

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[tokio::test]
async fn the_descriptor_is_full_only_when_everything_is_verified() {
    let dir = scratch("desc-full");
    let cfg = config(&dir, "echo", allow());
    let adapter = ClaudeAdapter::new(opts());
    let d = adapter.probe(&cfg).await;
    assert_eq!(d.level, EnforcementLevel::Full, "{:?}", d.reasons);
    assert_eq!(d.cli_version.as_deref(), Some("2.1.308"));
    assert!(d.version_tested);
    assert!(adapter.tested_versions().contains(&"2.1.308"));

    // An untested version is Partial.
    let mut cfg2 = cfg.clone();
    cfg2.env.push(("FAKE_VERSION".into(), "9.9.9".into()));
    let d = adapter.probe(&cfg2).await;
    assert_eq!(d.level, EnforcementLevel::Partial);
    assert!(
        d.reasons
            .iter()
            .any(|r| r.code == ReasonCode::UntestedVersion)
    );

    // A CLI that can't be found: version unknown, never Full.
    let mut cfg3 = cfg.clone();
    cfg3.program = Some(dir.join("missing"));
    let d = adapter.probe(&cfg3).await;
    assert_eq!(d.level, EnforcementLevel::Partial);
    assert_eq!(d.cli_version, None);
}

#[tokio::test]
async fn external_hooks_make_the_session_partial_and_say_why() {
    let dir = scratch("desc-hooks");
    write(
        &dir.join(".claude/settings.local.json"),
        r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"/opt/audit.sh"}]}]}}"#,
    );
    let cfg = config(&dir, "echo", allow());
    let adapter = ClaudeAdapter::new(opts());

    // Native behaviour (the default): the project's hook is found, at probe
    // time and in the session.
    let probe = adapter.probe(&cfg).await;
    assert_eq!(probe.level, EnforcementLevel::Partial);
    assert!(
        probe
            .reasons
            .iter()
            .any(|r| r.code == ReasonCode::ExternalHooks && r.message.contains("audit.sh"))
    );
    assert!(probe.config_sources.iter().any(|s| {
        s.kind == ConfigSourceKind::LocalSettings && s.inspected && s.sha256.is_some()
    }));
    let mut session = adapter.start(cfg.clone(), Transport::Local).await.unwrap();
    assert_eq!(session.descriptor.level, EnforcementLevel::Partial);
    session.commands.send(say("go")).await.unwrap();
    let evs = next_matching(&mut session, 10, turn_done).await;
    let Some(AgentEvent::SessionStarted { enforcement, .. }) = evs.first() else {
        panic!("{evs:?}")
    };
    assert_eq!(enforcement.level, EnforcementLevel::Partial);
    shutdown(session).await;

    // External settings disabled: nothing is loaded, Full.
    let adapter = ClaudeAdapter::new(ClaudeOptions {
        setting_sources: Some(String::new()),
        ..opts()
    });
    let session = adapter.start(cfg, Transport::Local).await.unwrap();
    assert_eq!(session.descriptor.level, EnforcementLevel::Full);
    let argv = fs::read_to_string(dir.join("argv.txt")).unwrap();
    assert!(argv.lines().last().unwrap().contains("--setting-sources"));
    shutdown(session).await;
}

/// A settings file that appears mid-session shows up as `EnforcementChanged`
/// when the next turn starts.
#[tokio::test]
async fn a_changed_settings_file_emits_enforcement_changed() {
    let dir = scratch("desc-change");
    let cfg = config(&dir, "echo", allow());
    let mut session = start(cfg, opts()).await.unwrap();
    assert_eq!(session.descriptor.level, EnforcementLevel::Full);
    session.commands.send(say("one")).await.unwrap();
    next_matching(&mut session, 10, turn_done).await;
    write(
        &dir.join(".claude/settings.json"),
        r#"{"apiKeyHelper":"/opt/key.sh"}"#,
    );
    session.commands.send(say("two")).await.unwrap();
    let evs = next_matching(&mut session, 10, turn_done).await;
    let changed = evs.iter().find_map(|e| match e {
        AgentEvent::EnforcementChanged { enforcement } => Some(enforcement),
        _ => None,
    });
    let changed = changed.expect("EnforcementChanged");
    assert_eq!(changed.level, EnforcementLevel::Partial);
    assert!(
        changed
            .reasons
            .iter()
            .any(|r| r.code == ReasonCode::ExternalHooks)
    );
    shutdown(session).await;
}

/// The plugin-hook case with `--plugin-dir`: the plugin is uninspected and its
/// hooks are reported.
#[tokio::test]
async fn a_plugin_with_a_hook_is_partial() {
    let dir = scratch("desc-plugin");
    let plugin = dir.join("my-plugin");
    write(
        &plugin.join("hooks/hooks.json"),
        r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"echo hooked"}]}]}}"#,
    );
    let cfg = config(&dir, "echo", allow());
    let adapter = ClaudeAdapter::new(ClaudeOptions {
        setting_sources: Some(String::new()),
        extra_args: vec!["--plugin-dir".into(), plugin.display().to_string()],
        ..opts()
    });
    let d = adapter.probe(&cfg).await;
    assert_eq!(d.level, EnforcementLevel::Partial);
    let codes: Vec<_> = d.reasons.iter().map(|r| r.code).collect();
    assert!(codes.contains(&ReasonCode::ExternalHooks), "{codes:?}");
    assert!(codes.contains(&ReasonCode::UninspectedConfig), "{codes:?}");
    assert!(
        d.config_sources
            .iter()
            .any(|s| s.kind == ConfigSourceKind::Plugin && !s.inspected)
    );
}

/// The recorded inputs replay to the same events the session emitted.
#[tokio::test]
async fn a_recorded_session_replays_to_the_same_events() {
    use atrium_agents::claude::ClaudeNormalizer;
    use atrium_agents::fixture;
    let dir = scratch("replay");
    let (tap, mut taps) = tokio::sync::mpsc::unbounded_channel();
    let mut session = start(
        config(&dir, "hook-ask", TestGate::asking(GateDecision::Allow)),
        ClaudeOptions {
            tap: Some(tap),
            ..opts()
        },
    )
    .await
    .unwrap();
    session.commands.send(say("go")).await.unwrap();
    let mut live = next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::ApprovalRequested { .. })
    })
    .await;
    session
        .commands
        .send(SessionCommand::ResolveApproval {
            request_id: "T1".into(),
            decision: ApprovalDecision::Allow,
        })
        .await
        .unwrap();
    live.extend(next_matching(&mut session, 10, turn_done).await);
    live.extend(shutdown(session).await);
    let mut lines = Vec::new();
    while let Ok(l) = taps.try_recv() {
        lines.push(l);
    }
    assert!(
        lines
            .iter()
            .any(|l| matches!(l.input, AdapterInput::Redeem { .. }))
    );
    assert!(
        lines
            .iter()
            .any(|l| matches!(l.input, AdapterInput::Enforcement { .. }))
    );
    let replayed = fixture::normalize(&lines, &mut ClaudeNormalizer::new());
    assert_eq!(replayed, live);
}
