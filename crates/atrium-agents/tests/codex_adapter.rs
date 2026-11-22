#![doc = "Offline integration tests for CodexAdapter using examples/codex-fake.rs."]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use atrium_agents::codex::{CodexAdapter, CodexOptions};
use atrium_agents::{
    AdapterError, AdapterInput, AgentAdapter, ApprovalTicket, BoxFuture, GateDecision,
    MAX_QUEUED_MESSAGES, McpServer, PermissionPolicy, SHUTDOWN_GRACE, Session, SessionConfig,
    ToolCheck, ToolGate, Transport,
};
use atrium_protocol::agent::{
    AgentEvent, ApprovalDecision, MessageBody, MessageKind, QuestionAnswer, RequestOutcome,
    SessionCommand, ToolKind, ToolStatus, TurnOutcome,
};
use atrium_protocol::limits::MAX_PENDING_REQUESTS;
use atrium_protocol::policy::{ConfigSourceKind, EnforcementLevel, ReasonCode, ToolSubject};
use serde_json::Value;

fn fake() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let exe = std::env::current_exe().expect("current_exe");
            let dir = exe.parent().expect("deps").parent().expect("profile");
            let path = dir.join("examples").join("codex-fake");
            if !path.exists() {
                let status = std::process::Command::new(env!("CARGO"))
                    .args([
                        "build",
                        "-p",
                        "atrium-agents",
                        "--example",
                        "codex-fake",
                        "--offline",
                    ])
                    .status()
                    .expect("build codex-fake");
                assert!(status.success(), "codex-fake build failed");
            }
            assert!(path.exists(), "missing {}", path.display());
            path
        })
        .clone()
}

type CheckFn = dyn Fn(&ToolCheck) -> GateDecision + Send + Sync;
type RedeemFn = dyn Fn(&ApprovalTicket, &ToolCheck) -> GateDecision + Send + Sync;

struct TestGate {
    check: Box<CheckFn>,
    redeem: Box<RedeemFn>,
    question: Option<ApprovalTicket>,
    checks: Mutex<Vec<ToolCheck>>,
    redeems: Mutex<Vec<(ApprovalTicket, ToolCheck)>>,
}

impl TestGate {
    fn new(check: impl Fn(&ToolCheck) -> GateDecision + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            check: Box::new(check),
            redeem: Box::new(|_, _| GateDecision::Allow),
            question: Some(ApprovalTicket {
                request_id: "Q1".into(),
                rememberable: false,
            }),
            checks: Mutex::default(),
            redeems: Mutex::default(),
        })
    }

    fn asking(redeem: GateDecision) -> Arc<Self> {
        Arc::new(Self {
            check: Box::new(|_| GateDecision::Ask {
                ticket: ApprovalTicket {
                    request_id: "T1".into(),
                    rememberable: true,
                },
            }),
            redeem: Box::new(move |_, _| redeem.clone()),
            question: Some(ApprovalTicket {
                request_id: "Q1".into(),
                rememberable: false,
            }),
            checks: Mutex::default(),
            redeems: Mutex::default(),
        })
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
        ticket: &'a ApprovalTicket,
        check: &'a ToolCheck,
    ) -> BoxFuture<'a, GateDecision> {
        self.redeems
            .lock()
            .unwrap()
            .push((ticket.clone(), check.clone()));
        let d = (self.redeem)(ticket, check);
        Box::pin(async move { d })
    }

    fn question_ticket(&self, _turn: Option<u32>) -> BoxFuture<'_, Option<ApprovalTicket>> {
        let q = self.question.clone();
        Box::pin(async move { q })
    }
}

fn scratch(name: &str) -> PathBuf {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "atrium-codex-adapter-{}-{}-{n}",
        std::process::id(),
        name
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn config(dir: &Path, mode: &str, gate: Arc<dyn ToolGate>) -> SessionConfig {
    let user_dir = dir.join("empty-codex-home");
    fs::create_dir_all(&user_dir).unwrap();
    SessionConfig {
        cwd: dir.to_path_buf(),
        model: Some("gpt-5".into()),
        effort: None,
        permissions: PermissionPolicy::Ask,
        resume: None,
        mcp_servers: Vec::new(),
        gate,
        env: vec![
            ("FAKE_MODE".into(), mode.into()),
            ("FAKE_DIR".into(), dir.display().to_string()),
            (
                "FAKE_AUDIT_FILE".into(),
                dir.join("audit.json").display().to_string(),
            ),
            ("CODEX_HOME".into(), user_dir.display().to_string()),
        ],
        program: Some(fake()),
        on_spawn: None,
    }
}

fn opts(dir: &Path) -> CodexOptions {
    CodexOptions {
        user_dir: Some(dir.join("empty-codex-home")),
        ..CodexOptions::default()
    }
}

fn allow() -> Arc<dyn ToolGate> {
    TestGate::new(|_| GateDecision::Allow)
}

async fn wait_file_until<T>(
    path: &Path,
    timeout: Duration,
    mut parse: impl FnMut(&str) -> Option<T>,
) -> T {
    let end = Instant::now() + timeout;
    let mut last_seen: Option<String> = None;
    loop {
        if let Ok(text) = fs::read_to_string(path) {
            if let Some(value) = parse(&text) {
                return value;
            }
            last_seen = Some(text);
        }
        assert!(
            Instant::now() < end,
            "{} never became complete (last_seen={last_seen:?})",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_file(path: &Path, secs: u64) -> String {
    wait_file_until(path, Duration::from_secs(secs), |text| {
        (text.ends_with('\n') && !text.trim().is_empty()).then(|| text.to_owned())
    })
    .await
}

async fn wait_json(path: &Path, secs: u64) -> Value {
    wait_file_until(path, Duration::from_secs(secs), |text| {
        serde_json::from_str::<Value>(text).ok()
    })
    .await
}

async fn wait_pid(path: &Path, secs: u64) -> String {
    wait_file_until(path, Duration::from_secs(secs), |text| {
        if !text.ends_with('\n') {
            return None;
        }
        let trimmed = text.trim();
        trimmed
            .parse::<u32>()
            .ok()
            .filter(|&pid| pid > 0)
            .map(|_| trimmed.to_owned())
    })
    .await
}

/// The completed (`\n`-terminated) lines of `responses.jsonl` so far, as JSON.
///
/// An unfinished trailing line (without `\n` yet) is ignored while the writer
/// is still appending. Every completed line must parse as valid JSON.
fn responses(dir: &Path) -> Vec<Value> {
    let path = dir.join("responses.jsonl");
    let Ok(raw) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Some(last_nl) = raw.rfind('\n') else {
        return Vec::new();
    };
    raw[..=last_nl]
        .lines()
        .enumerate()
        .map(|(idx, line)| {
            serde_json::from_str(line).unwrap_or_else(|err| {
                panic!(
                    "malformed completed JSONL line {} in {}: {err}: {line:?}",
                    idx + 1,
                    path.display()
                )
            })
        })
        .collect()
}

fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", "--", pid.trim()])
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

fn ended(e: &AgentEvent) -> bool {
    matches!(e, AgentEvent::SessionEnded { .. })
}

fn say(text: &str) -> SessionCommand {
    SessionCommand::SendUserMessage { text: text.into() }
}

async fn shutdown(mut session: Session) -> Vec<AgentEvent> {
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
    next_matching(&mut session, 10, ended).await
}

/// Saturation test (M0 §3.2): receiver leaves `events` unread while the fake
/// floods output; `Interrupt` and `Shutdown` still take effect within
/// `SHUTDOWN_GRACE`, the entire process group (leader + grandchild) is
/// cleaned up, `SessionEnded` lands once and last, and sequences stay gapless.
#[tokio::test]
async fn saturation_unread_events_still_obey_interrupt_and_shutdown() {
    let dir = scratch("saturation");
    let mut cfg = config(&dir, "flood", allow());
    cfg.env.push(("FAKE_GRANDCHILD".into(), "1".into()));
    let session = CodexAdapter::with_options(opts(&dir))
        .start(cfg, Transport::Local)
        .await
        .unwrap();
    let process = session.process.clone().expect("ProcessIdentity reported");
    let Session {
        mut events,
        commands,
        ..
    } = session;

    commands.send(say("flood")).await.unwrap();

    // Wait for the event channel to saturate completely.
    let fill_end = Instant::now() + Duration::from_secs(5);
    while events.len() < events.max_capacity() && Instant::now() < fill_end {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(events.len(), events.max_capacity(), "event channel is full");
    let pid = wait_pid(&dir.join("pids.txt"), 5).await;
    let grandchild = wait_pid(&dir.join("grandchild.pid"), 5).await;
    assert!(alive(&grandchild) && group_alive(process.pgid));

    let t = Instant::now();
    commands.send(SessionCommand::Interrupt).await.unwrap();
    wait_file(&dir.join("interrupted"), 5).await;
    assert!(
        t.elapsed() < SHUTDOWN_GRACE,
        "interrupt reached fake in {:?}",
        t.elapsed()
    );

    let t = Instant::now();
    commands.send(SessionCommand::Shutdown).await.unwrap();
    tokio::time::timeout(SHUTDOWN_GRACE + Duration::from_secs(2), commands.closed())
        .await
        .expect("driver closed commands channel");
    assert!(
        t.elapsed() < SHUTDOWN_GRACE + Duration::from_secs(1),
        "shutdown took {:?}",
        t.elapsed()
    );
    assert!(!alive(&pid), "leader {pid} must be reaped");
    assert!(!alive(&grandchild), "grandchild {grandchild} must be gone");
    assert!(!group_alive(process.pgid), "process group must be empty");

    let mut last = None;
    let mut ended_count = 0;
    let mut seqs = Vec::new();
    while let Ok(Some(env)) = tokio::time::timeout(Duration::from_secs(5), events.recv()).await {
        seqs.push(env.seq);
        if matches!(env.event, AgentEvent::SessionEnded { .. }) {
            ended_count += 1;
        }
        last = Some(env.event);
    }
    assert_eq!(ended_count, 1);
    assert!(matches!(last, Some(AgentEvent::SessionEnded { .. })));
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "no seq gaps");

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_interrupt_the_child_ignores_ends_the_session() {
    let dir = scratch("stuck");
    let mut session = CodexAdapter::with_options(opts(&dir))
        .start(config(&dir, "stuck", allow()), Transport::Local)
        .await
        .unwrap();
    session.commands.send(say("hang")).await.unwrap();
    next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::TurnStarted { .. })
    })
    .await;
    let pid = wait_pid(&dir.join("pids.txt"), 5).await;
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
    assert!(matches!(evs.last(), Some(AgentEvent::SessionEnded { .. })));
    assert!(!alive(&pid) && !group_alive(pgid));
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn start_dropped_and_leader_exits_kill_process_group() {
    let dir = scratch("leader-exits");
    let spawned = Arc::new(Mutex::new(None));
    let spawned_cb = spawned.clone();
    let mut cfg = config(&dir, "leader-exits", allow());
    cfg.on_spawn = Some(Arc::new(move |id| {
        *spawned_cb.lock().unwrap() = Some(id.clone());
    }));
    let mut options = opts(&dir);
    options.startup_timeout = Some(Duration::from_millis(400));

    let err = CodexAdapter::with_options(options)
        .start(cfg, Transport::Local)
        .await
        .unwrap_err();
    assert!(matches!(err, AdapterError::Handshake(_)));
    let identity = spawned.lock().unwrap().clone().expect("on_spawn called");
    let grandchild = wait_pid(&dir.join("grandchild.pid"), 5).await;
    // Wait briefly for terminate_blocking / terminate to reap.
    let end = Instant::now() + Duration::from_secs(3);
    while alive(&grandchild) && Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(!alive(&grandchild), "grandchild must be dead");
    assert!(!group_alive(identity.pgid), "process group must be empty");
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn startup_hang_rpc_error_and_oversize_fail_cleanly() {
    for (mode, timeout_ms) in [
        ("startup-hang", Some(250u64)),
        ("startup-fail", None),
        ("startup-rpc-error", None),
        ("startup-oversize", None),
    ] {
        let dir = scratch(mode);
        let mut options = opts(&dir);
        options.startup_timeout = timeout_ms.map(Duration::from_millis);
        let tap_inputs = Arc::new(Mutex::new(Vec::new()));
        let tap_ref = tap_inputs.clone();
        options.tap = Some(Arc::new(move |input: &AdapterInput| {
            tap_ref.lock().unwrap().push(input.clone());
        }));
        let err = CodexAdapter::with_options(options)
            .start(config(&dir, mode, allow()), Transport::Local)
            .await
            .unwrap_err();
        assert!(matches!(err, AdapterError::Handshake(_)), "{mode}: {err:?}");
        let pid = fs::read_to_string(dir.join("pids.txt")).ok();
        if let Some(p) = pid
            && !p.trim().is_empty()
        {
            assert!(!alive(&p), "{mode}: child still alive");
        }
        assert!(
            tap_inputs
                .lock()
                .unwrap()
                .iter()
                .any(|i| matches!(i, AdapterInput::Exit { .. })),
            "{mode}: tap saw Exit"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}

#[tokio::test]
async fn oversize_midturn_ends_session_with_fatal_error() {
    let dir = scratch("oversize-midturn");
    let mut session = CodexAdapter::with_options(opts(&dir))
        .start(config(&dir, "oversize-midturn", allow()), Transport::Local)
        .await
        .unwrap();
    session.commands.send(say("big")).await.unwrap();
    let evs = next_matching(&mut session, 10, ended).await;
    assert!(
        evs.iter()
            .any(|e| matches!(e, AgentEvent::Error { fatal: true, .. }))
    );
    assert!(matches!(evs.last(), Some(AgentEvent::SessionEnded { .. })));
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn config_becomes_cli_arguments_and_thread_params() {
    for (policy, expected_sandbox, expected_approval) in [
        (PermissionPolicy::ReadOnly, "read-only", "untrusted"),
        (PermissionPolicy::Ask, "workspace-write", "untrusted"),
        (PermissionPolicy::FullAccess, "danger-full-access", "never"),
    ] {
        let dir = scratch("config-args");
        let mut cfg = config(&dir, "simple", allow());
        cfg.permissions = policy;
        cfg.effort = Some("high".into());
        cfg.resume = Some("thread-prev".into());
        cfg.mcp_servers.push(McpServer {
            name: "docs".into(),
            command: PathBuf::from("docs-mcp"),
            args: vec!["--stdio".into()],
            env: vec![("API_KEY".into(), "secret".into())],
        });
        let mut options = opts(&dir);
        options.profile = Some("work".into());

        let mut session = CodexAdapter::with_options(options)
            .start(cfg, Transport::Local)
            .await
            .unwrap();
        session.commands.send(say("hello")).await.unwrap();
        let _ = next_matching(&mut session, 5, |e| {
            matches!(e, AgentEvent::TurnCompleted { .. })
        })
        .await;
        let _ = shutdown(session).await;

        let audit: Value =
            serde_json::from_str(&fs::read_to_string(dir.join("audit.json")).unwrap()).unwrap();
        let args: Vec<&str> = audit["args"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(args.windows(2).any(|w| w == ["-p", "work"]));
        assert!(args.iter().any(|a| a.contains("mcp_servers.docs.command=")));
        assert!(
            args.windows(3)
                .any(|w| w == ["app-server", "--listen", "stdio://"])
        );

        let records = responses(&dir);
        let thread_req: Value = records
            .iter()
            .find(|v| v.get("method").and_then(Value::as_str) == Some("thread/resume"))
            .cloned()
            .expect("thread/resume sent");
        assert_eq!(thread_req["params"]["sandbox"], expected_sandbox);
        assert_eq!(thread_req["params"]["approvalPolicy"], expected_approval);
        assert_eq!(thread_req["params"]["approvalsReviewer"], "user");
        assert_eq!(thread_req["params"]["threadId"], "thread-prev");

        let turn_req: Value = records
            .iter()
            .find(|v| v.get("method").and_then(Value::as_str) == Some("turn/start"))
            .cloned()
            .expect("turn/start sent");
        assert_eq!(turn_req["params"]["effort"], "high");

        let _ = fs::remove_dir_all(&dir);
    }
}

#[tokio::test]
async fn gate_allow_deny_and_redeem_flow() {
    // 1) Ask -> Allow -> Redeem Allow => tool succeeds
    let dir = scratch("gate-redeem-allow");
    let gate = TestGate::asking(GateDecision::Allow);
    let mut session = CodexAdapter::with_options(opts(&dir))
        .start(config(&dir, "gate", gate.clone()), Transport::Local)
        .await
        .unwrap();
    session.commands.send(say("run")).await.unwrap();
    let req_id = {
        let evs = next_matching(&mut session, 5, |e| {
            matches!(e, AgentEvent::ApprovalRequested { .. })
        })
        .await;
        let (id, kind) = evs
            .iter()
            .find_map(|e| match e {
                AgentEvent::ApprovalRequested {
                    request_id, kind, ..
                } => Some((request_id.clone(), *kind)),
                _ => None,
            })
            .unwrap();
        assert_eq!(kind, ToolKind::Execute);
        id
    };
    session
        .commands
        .send(SessionCommand::ResolveApproval {
            request_id: req_id.clone(),
            decision: ApprovalDecision::Allow,
        })
        .await
        .unwrap();
    let evs = next_matching(&mut session, 5, |e| {
        matches!(e, AgentEvent::TurnCompleted { .. })
    })
    .await;
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::RequestResolved {
            request_id,
            outcome: RequestOutcome::Approved,
        } if *request_id == req_id
    )));
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Succeeded,
            ..
        }
    )));
    {
        let checks = gate.checks.lock().unwrap();
        let redeems = gate.redeems.lock().unwrap();
        assert_eq!(checks.len(), 1);
        assert_eq!(redeems.len(), 1);
        assert_eq!(
            redeems[0].1, checks[0],
            "redeem check must equal check built from stored request"
        );
        assert_ne!(redeems[0].1.input, serde_json::json!({}));
    }
    let _ = shutdown(session).await;
    let reply1: Value = responses(&dir)
        .into_iter()
        .find(|v| v.get("id").and_then(Value::as_str) == Some("srv-cmd-1"))
        .expect("srv-cmd-1 reply written");
    assert_eq!(reply1["result"]["decision"], "accept");
    let _ = fs::remove_dir_all(&dir);

    // 2) Ask -> Allow -> Redeem Deny => tool denied and Codex receives decline
    let dir2 = scratch("gate-redeem-deny");
    let gate2 = TestGate::asking(GateDecision::Deny {
        reason: "policy updated".into(),
    });
    let mut session2 = CodexAdapter::with_options(opts(&dir2))
        .start(config(&dir2, "gate", gate2.clone()), Transport::Local)
        .await
        .unwrap();
    session2.commands.send(say("run")).await.unwrap();
    let evs2 = next_matching(&mut session2, 5, |e| {
        matches!(e, AgentEvent::ApprovalRequested { .. })
    })
    .await;
    let req_id2 = evs2
        .iter()
        .find_map(|e| match e {
            AgentEvent::ApprovalRequested { request_id, .. } => Some(request_id.clone()),
            _ => None,
        })
        .unwrap();
    session2
        .commands
        .send(SessionCommand::ResolveApproval {
            request_id: req_id2.clone(),
            decision: ApprovalDecision::Allow,
        })
        .await
        .unwrap();
    let done2 = next_matching(&mut session2, 5, |e| {
        matches!(e, AgentEvent::TurnCompleted { .. })
    })
    .await;
    assert!(done2.iter().any(|e| matches!(
        e,
        AgentEvent::RequestResolved {
            request_id,
            outcome: RequestOutcome::Approved,
        } if *request_id == req_id2
    )));
    assert!(done2.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Denied,
            ..
        }
    )));
    assert!(!done2.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Succeeded,
            ..
        }
    )));
    {
        let checks2 = gate2.checks.lock().unwrap();
        let redeems2 = gate2.redeems.lock().unwrap();
        assert_eq!(checks2.len(), 1);
        assert_eq!(redeems2.len(), 1);
        assert_eq!(redeems2[0].1, checks2[0]);
    }
    let _ = shutdown(session2).await;
    let reply2: Value = responses(&dir2)
        .into_iter()
        .find(|v| v.get("id").and_then(Value::as_str) == Some("srv-cmd-1"))
        .expect("srv-cmd-1 reply written");
    assert_eq!(
        reply2["result"]["decision"], "decline",
        "redeem Deny must answer Codex with decline"
    );
    let _ = fs::remove_dir_all(&dir2);
}

struct PanicGate;
impl ToolGate for PanicGate {
    fn check<'a>(&'a self, _: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        Box::pin(async { panic!("gate exploded") })
    }
    fn redeem<'a>(
        &'a self,
        _: &'a ApprovalTicket,
        _: &'a ToolCheck,
    ) -> BoxFuture<'a, GateDecision> {
        Box::pin(async { GateDecision::Allow })
    }
    fn question_ticket(&self, _: Option<u32>) -> BoxFuture<'_, Option<ApprovalTicket>> {
        Box::pin(async { None })
    }
}

#[tokio::test]
async fn gate_panic_fails_closed_and_emits_gate_unreachable_reason() {
    let dir = scratch("gate-panic");
    let mut session = CodexAdapter::with_options(opts(&dir))
        .start(config(&dir, "gate", Arc::new(PanicGate)), Transport::Local)
        .await
        .unwrap();
    session.commands.send(say("run")).await.unwrap();
    let evs = next_matching(&mut session, 5, |e| {
        matches!(e, AgentEvent::TurnCompleted { .. })
    })
    .await;
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::EnforcementChanged { enforcement }
            if enforcement
                .reasons
                .iter()
                .any(|r| r.code == ReasonCode::GateUnreachable)
    )));
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Denied,
            ..
        }
    )));
    let _ = shutdown(session).await;
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn oversize_server_request_and_flood_ask_decline_without_calling_gate() {
    let dir = scratch("oversize-args");
    let gate = TestGate::new(|_| GateDecision::Allow);
    let mut session = CodexAdapter::with_options(opts(&dir))
        .start(
            config(&dir, "oversize-args", gate.clone()),
            Transport::Local,
        )
        .await
        .unwrap();
    session.commands.send(say("go")).await.unwrap();
    let evs = next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::TurnCompleted { .. })
    })
    .await;
    assert!(gate.checks.lock().unwrap().is_empty());
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Denied,
            ..
        }
    )));
    let _ = shutdown(session).await;
    let _ = fs::remove_dir_all(&dir);

    let dir2 = scratch("flood-ask");
    let gate2 = TestGate::asking(GateDecision::Allow);
    let mut session2 = CodexAdapter::with_options(opts(&dir2))
        .start(config(&dir2, "flood-ask", gate2.clone()), Transport::Local)
        .await
        .unwrap();
    session2.commands.send(say("flood")).await.unwrap();
    let evs2 = next_matching(&mut session2, 10, |e| {
        matches!(e, AgentEvent::TurnCompleted { .. })
    })
    .await;
    let asked_count = evs2
        .iter()
        .filter(|e| matches!(e, AgentEvent::ApprovalRequested { .. }))
        .count();
    assert!(
        asked_count <= MAX_PENDING_REQUESTS,
        "asked {asked_count} must stay <= {MAX_PENDING_REQUESTS}"
    );
    let _ = shutdown(session2).await;
    let _ = fs::remove_dir_all(&dir2);
}

#[tokio::test]
async fn question_and_queue_limit_round_trip() {
    let dir = scratch("question");
    let mut session = CodexAdapter::with_options(opts(&dir))
        .start(config(&dir, "question", allow()), Transport::Local)
        .await
        .unwrap();
    session.commands.send(say("ask")).await.unwrap();
    let evs = next_matching(&mut session, 5, |e| {
        matches!(e, AgentEvent::QuestionAsked { .. })
    })
    .await;
    let req_id = evs
        .iter()
        .find_map(|e| match e {
            AgentEvent::QuestionAsked { request_id, .. } => Some(request_id.clone()),
            _ => None,
        })
        .unwrap();
    session
        .commands
        .send(SessionCommand::AnswerQuestion {
            request_id: req_id,
            answers: vec![QuestionAnswer {
                question_id: "color".into(),
                selected: vec!["1".into()],
                text: None,
            }],
        })
        .await
        .unwrap();
    let done = next_matching(&mut session, 5, |e| {
        matches!(e, AgentEvent::TurnCompleted { .. })
    })
    .await;
    assert!(done.iter().any(|e| matches!(
        e,
        AgentEvent::MessageDelta {
            kind: MessageKind::Assistant,
            text,
            ..
        } if text.contains("Blue")
    ) || matches!(
        e,
        AgentEvent::MessageCompleted {
            kind: MessageKind::Assistant,
            body: MessageBody::Inline { text },
            ..
        } if text.contains("Blue")
    )));
    let _ = shutdown(session).await;
    let _ = fs::remove_dir_all(&dir);

    let dir2 = scratch("queue");
    let mut cfg = config(&dir2, "delayed-turn", allow());
    cfg.env.push(("FAKE_TURN_DELAY_MS".into(), "200".into()));
    let mut session2 = CodexAdapter::with_options(opts(&dir2))
        .start(cfg, Transport::Local)
        .await
        .unwrap();
    // 1 active + MAX_QUEUED_MESSAGES accepted, next rejected
    for i in 0..=MAX_QUEUED_MESSAGES {
        session2.commands.send(say(&format!("m{i}"))).await.unwrap();
    }
    session2.commands.send(say("overflow")).await.unwrap();
    let evs2 = next_matching(&mut session2, 5, |e| {
        matches!(e, AgentEvent::Error { fatal: false, .. })
    })
    .await;
    assert!(
        evs2.iter()
            .any(|e| matches!(e, AgentEvent::Error { fatal: false, .. }))
    );
    let _ = shutdown(session2).await;
    let _ = fs::remove_dir_all(&dir2);
}

#[tokio::test]
async fn decline_question_sends_empty_answers_and_continues_turn() {
    let dir = scratch("decline-question");
    let mut session = CodexAdapter::with_options(opts(&dir))
        .start(config(&dir, "question", allow()), Transport::Local)
        .await
        .unwrap();
    session.commands.send(say("ask")).await.unwrap();
    let evs = next_matching(&mut session, 5, |e| {
        matches!(e, AgentEvent::QuestionAsked { .. })
    })
    .await;
    let req_id = evs
        .iter()
        .find_map(|e| match e {
            AgentEvent::QuestionAsked { request_id, .. } => Some(request_id.clone()),
            _ => None,
        })
        .unwrap();

    session
        .commands
        .send(SessionCommand::DeclineQuestion {
            request_id: req_id.clone(),
        })
        .await
        .unwrap();
    let done = next_matching(&mut session, 5, |e| {
        matches!(e, AgentEvent::TurnCompleted { .. })
    })
    .await;
    assert!(done.iter().any(|e| matches!(
        e,
        AgentEvent::RequestResolved {
            request_id,
            outcome: RequestOutcome::Cancelled,
        } if *request_id == req_id
    )));
    assert!(done.iter().any(|e| matches!(
        e,
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Completed,
            ..
        }
    )));

    // Unknown request_id produces a non-fatal Error without failing the session.
    session
        .commands
        .send(SessionCommand::DeclineQuestion {
            request_id: "unknown-q".into(),
        })
        .await
        .unwrap();
    let err_evs = next_matching(&mut session, 5, |e| {
        matches!(e, AgentEvent::Error { fatal: false, .. })
    })
    .await;
    assert!(
        err_evs
            .iter()
            .any(|e| matches!(e, AgentEvent::Error { fatal: false, .. }))
    );

    let _ = shutdown(session).await;

    let reply: Value = responses(&dir)
        .into_iter()
        .find(|v| v.get("id").and_then(Value::as_str) == Some("srv-q-1"))
        .expect("question reply sent to fake server");
    assert_eq!(reply["result"], serde_json::json!({ "answers": {} }));

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn enforcement_inspects_user_dir_and_untested_version() {
    let dir = scratch("enforcement");
    let user_dir = dir.join("custom-codex");
    fs::create_dir_all(&user_dir).unwrap();
    fs::write(user_dir.join("config.toml"), "model = \"gpt-5\"\n").unwrap();
    fs::write(
        user_dir.join("hooks.json"),
        "{\"hooks\":{\"PreToolUse\":[{\"command\":\"echo hi\"}]}}",
    )
    .unwrap();

    let mut cfg = config(&dir, "simple", allow());
    cfg.env
        .push(("FAKE_VERSION".into(), "codex-cli 0.999.0".into()));
    let mut options = opts(&dir);
    options.user_dir = Some(user_dir.clone());

    let seen_spawn = Arc::new(AtomicBool::new(false));
    let sp = seen_spawn.clone();
    cfg.on_spawn = Some(Arc::new(move |_| sp.store(true, Ordering::Release)));

    let session = CodexAdapter::with_options(options)
        .start(cfg, Transport::Local)
        .await
        .unwrap();
    assert!(seen_spawn.load(Ordering::Acquire));
    assert_eq!(session.descriptor.level, EnforcementLevel::Partial);
    assert!(
        session
            .descriptor
            .reasons
            .iter()
            .any(|r| r.code == ReasonCode::UntestedVersion)
    );
    assert!(
        session
            .descriptor
            .reasons
            .iter()
            .any(|r| r.code == ReasonCode::ExternalHooks)
    );
    assert!(
        session
            .descriptor
            .config_sources
            .iter()
            .any(|s| s.kind == ConfigSourceKind::UserSettings && s.inspected)
    );

    let _ = shutdown(session).await;
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn codex_tool_check_extracts_subjects_uniformly() {
    let payload = serde_json::json!({
        "item": {
            "id": "call-1",
            "type": "fileChange",
            "changes": [{
                "path": "src/main.rs",
                "kind": { "type": "delete" }
            }]
        }
    });
    let check = atrium_agents::codex::tools::tool_check(
        Some("call-1".into()),
        "fileChange".into(),
        &payload,
        "/scratch",
        1,
    );
    assert_eq!(check.kind, ToolKind::Delete);
    assert_eq!(
        check.subject,
        ToolSubject::Paths {
            paths: vec!["src/main.rs".into()],
            glob: None,
            recursive: false,
        }
    );
}

#[tokio::test]
async fn unverified_legacy_permissions_and_grant_root_requests_are_declined() {
    let dir = scratch("unverified-approvals");
    let gate = TestGate::new(|_| GateDecision::Allow);
    let mut session = CodexAdapter::with_options(opts(&dir))
        .start(
            config(&dir, "unverified-approvals", gate.clone()),
            Transport::Local,
        )
        .await
        .unwrap();
    session.commands.send(say("go")).await.unwrap();
    let evs = next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::TurnCompleted { .. })
    })
    .await;

    assert!(
        gate.checks.lock().unwrap().is_empty(),
        "none of the unverified approval shapes may reach the gate"
    );
    assert!(
        !evs.iter()
            .any(|e| matches!(e, AgentEvent::ApprovalRequested { .. }))
    );
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished {
            call_id,
            status: ToolStatus::Denied,
            ..
        } if call_id == "unv-fc-1"
    )));

    let _ = shutdown(session).await;

    let replies: std::collections::HashMap<String, Value> = responses(&dir)
        .into_iter()
        .filter_map(|v| {
            let id = v.get("id")?.as_str()?.to_owned();
            let res = v.get("result")?.clone();
            Some((id, res))
        })
        .collect();
    assert_eq!(
        replies.get("srv-unv-exec"),
        Some(&serde_json::json!({ "decision": "denied" }))
    );
    assert_eq!(
        replies.get("srv-unv-patch"),
        Some(&serde_json::json!({ "decision": "denied" }))
    );
    assert_eq!(
        replies.get("srv-unv-perm"),
        Some(&serde_json::json!({ "permissions": {}, "scope": "turn" }))
    );
    assert_eq!(
        replies.get("srv-unv-grantroot"),
        Some(&serde_json::json!({ "decision": "decline" }))
    );

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn thread_start_effective_policy_mismatch_refuses_start() {
    for (env_key, env_val, expected_substr) in [
        (
            "FAKE_EFFECTIVE_APPROVAL",
            "never",
            "approvalPolicy mismatch",
        ),
        (
            "FAKE_EFFECTIVE_SANDBOX",
            "dangerFullAccess",
            "sandbox mismatch",
        ),
    ] {
        let dir = scratch("policy-mismatch");
        let mut cfg = config(&dir, "simple", allow());
        cfg.permissions = PermissionPolicy::ReadOnly;
        cfg.env.push((env_key.into(), env_val.into()));

        let tap_inputs = Arc::new(Mutex::new(Vec::new()));
        let tap_ref = tap_inputs.clone();
        let mut options = opts(&dir);
        options.tap = Some(Arc::new(move |input: &AdapterInput| {
            tap_ref.lock().unwrap().push(input.clone());
        }));

        let err = CodexAdapter::with_options(options)
            .start(cfg, Transport::Local)
            .await
            .unwrap_err();
        let AdapterError::Handshake(msg) = err else {
            panic!("expected Handshake error, got {err:?}");
        };
        assert!(
            msg.contains(expected_substr),
            "unexpected error message: {msg}"
        );
        let pid = fs::read_to_string(dir.join("pids.txt")).ok();
        if let Some(p) = pid
            && !p.trim().is_empty()
        {
            assert!(!alive(&p), "child still alive after policy mismatch");
        }
        let inputs = tap_inputs.lock().unwrap().clone();
        let evs = atrium_agents::fixture::normalize(
            &inputs
                .iter()
                .enumerate()
                .map(|(i, input)| atrium_agents::fixture::FixtureLine {
                    ms: i as u64,
                    input: input.clone(),
                })
                .collect::<Vec<_>>(),
            &mut atrium_agents::codex::CodexNormalizer::new(),
        );
        assert!(matches!(
            evs.as_slice(),
            [
                AgentEvent::Error { fatal: true, .. },
                AgentEvent::SessionEnded { .. }
            ]
        ));
        let _ = fs::remove_dir_all(&dir);
    }
}

#[tokio::test]
async fn saturation_declines_approvals_questions_and_reports_dropped_events() {
    use tokio::sync::Notify;

    const PENDING_LIMIT: usize = 4096;

    struct SlowAskGate {
        started: Arc<AtomicUsize>,
        release: Arc<AtomicBool>,
        notify: Arc<Notify>,
        seen_asks: Arc<AtomicUsize>,
    }

    impl ToolGate for SlowAskGate {
        fn check<'a>(&'a self, _tool: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
            let started = self.started.clone();
            let release = self.release.clone();
            let notify = self.notify.clone();
            let seen_asks = self.seen_asks.clone();
            Box::pin(async move {
                started.fetch_add(1, Ordering::AcqRel);
                while !release.load(Ordering::Acquire) {
                    notify.notified().await;
                }
                let n = seen_asks.fetch_add(1, Ordering::AcqRel);
                GateDecision::Ask {
                    ticket: ApprovalTicket {
                        request_id: format!("sat-req-{n}"),
                        rememberable: false,
                    },
                }
            })
        }

        fn redeem<'a>(
            &'a self,
            _ticket: &'a ApprovalTicket,
            _tool: &'a ToolCheck,
        ) -> BoxFuture<'a, GateDecision> {
            Box::pin(async {
                panic!("redeem should not be called when approval was declined at PENDING_LIMIT");
            })
        }

        fn question_ticket(
            &self,
            _turn: Option<u32>,
        ) -> BoxFuture<'static, Option<ApprovalTicket>> {
            let started = self.started.clone();
            let release = self.release.clone();
            let notify = self.notify.clone();
            let seen_asks = self.seen_asks.clone();
            Box::pin(async move {
                started.fetch_add(1, Ordering::AcqRel);
                while !release.load(Ordering::Acquire) {
                    notify.notified().await;
                }
                let n = seen_asks.fetch_add(1, Ordering::AcqRel);
                Some(ApprovalTicket {
                    request_id: format!("sat-q-{n}"),
                    rememberable: false,
                })
            })
        }
    }

    let dir = scratch("saturation-requests");
    let gate = Arc::new(SlowAskGate {
        started: Arc::new(AtomicUsize::new(0)),
        release: Arc::new(AtomicBool::new(false)),
        notify: Arc::new(Notify::new()),
        seen_asks: Arc::new(AtomicUsize::new(0)),
    });
    let stdout_count = Arc::new(AtomicUsize::new(0));
    let sc = stdout_count.clone();

    let mut cfg = config(&dir, "flood-then-ask", gate.clone());
    cfg.env.push(("FAKE_FLOOD_COUNT".into(), "5500".into()));
    let mut options = opts(&dir);
    options.tap = Some(Arc::new(move |input: &AdapterInput| {
        if matches!(input, AdapterInput::Stdout { .. }) {
            sc.fetch_add(1, Ordering::AcqRel);
        }
    }));

    let session = CodexAdapter::with_options(options)
        .start(cfg, Transport::Local)
        .await
        .unwrap();

    session.commands.send(say("saturate")).await.unwrap();

    let wait_end = Instant::now() + Duration::from_secs(5);
    let min_stdout = PENDING_LIMIT + session.events.max_capacity() + 5;
    while (session.events.len() < session.events.max_capacity()
        || gate.started.load(Ordering::Acquire) < 2
        || stdout_count.load(Ordering::Acquire) < min_stdout)
        && Instant::now() < wait_end
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        session.events.len(),
        session.events.max_capacity(),
        "event channel should be full"
    );
    assert_eq!(
        gate.started.load(Ordering::Acquire),
        2,
        "both approval and question gates started"
    );

    // Release both slow gate calls while Driver::pending is sitting at PENDING_LIMIT.
    gate.release.store(true, Ordering::Release);
    gate.notify.notify_waiters();

    // Wait for Codex fake server to record both declines in responses.jsonl.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let records = responses(&dir);
        let has_cmd_decline = records.iter().any(|v| {
            v.get("id").and_then(Value::as_str) == Some("srv-sat-cmd")
                && v.pointer("/result/decision").and_then(Value::as_str) == Some("decline")
        });
        let has_q_decline = records.iter().any(|v| {
            v.get("id").and_then(Value::as_str) == Some("srv-sat-q")
                && v.get("result") == Some(&serde_json::json!({ "answers": {} }))
        });
        if has_cmd_decline && has_q_decline {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for saturation declines in responses.jsonl: {records:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Trigger shutdown while `pending` is still full so remaining buffered deltas
    // arrive during shutdown, get dropped past PENDING_LIMIT, and are counted/reported.
    let evs = shutdown(session).await;

    assert!(
        !evs.iter()
            .any(|e| matches!(e, AgentEvent::ApprovalRequested { .. })),
        "saturated approval must be declined rather than emitted unread"
    );
    assert!(
        !evs.iter()
            .any(|e| matches!(e, AgentEvent::QuestionAsked { .. })),
        "saturated question must be declined rather than emitted unread"
    );
    let non_fatal_errors: Vec<&str> = evs
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Error {
                fatal: false,
                message,
                ..
            } => Some(message.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        non_fatal_errors
            .iter()
            .any(|m| m.contains("declined") && m.contains("PENDING_LIMIT")),
        "expected decline saturation errors, got {non_fatal_errors:?}"
    );
    assert!(
        non_fatal_errors
            .iter()
            .any(|m| m.contains("dropped") && m.contains("non-terminal")),
        "expected dropped non-terminal event report, got {non_fatal_errors:?}"
    );
    assert!(matches!(evs.last(), Some(AgentEvent::SessionEnded { .. })));

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn wait_helpers_ignore_empty_and_half_written_files() {
    let dir = scratch("wait-partial");

    // 1. JSON reader waits past empty and truncated files until valid JSON lands.
    let json_path = dir.join("response.json");
    fs::write(&json_path, "").unwrap();
    let mut json_fut = Box::pin(wait_json(&json_path, 5));
    assert!(
        tokio::time::timeout(Duration::from_millis(60), &mut json_fut)
            .await
            .is_err(),
        "wait_json returned on an empty file"
    );
    fs::write(&json_path, r#"{"result":{"decision":"#).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(60), &mut json_fut)
            .await
            .is_err(),
        "wait_json returned on truncated JSON"
    );
    fs::write(&json_path, r#"{"result":{"decision":"accept"}}"#).unwrap();
    let value = json_fut.await;
    assert_eq!(value["result"]["decision"], "accept");

    // 2. PID and marker readers wait past empty and unterminated partial writes.
    let pid_path = dir.join("grandchild.pid");
    fs::write(&pid_path, "").unwrap();
    let mut pid_fut = Box::pin(wait_pid(&pid_path, 5));
    assert!(
        tokio::time::timeout(Duration::from_millis(60), &mut pid_fut)
            .await
            .is_err(),
        "wait_pid returned on an empty file"
    );
    fs::write(&pid_path, "4242").unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(60), &mut pid_fut)
            .await
            .is_err(),
        "wait_pid returned before the trailing newline"
    );
    fs::write(&pid_path, "4242\n").unwrap();
    assert_eq!(pid_fut.await, "4242");

    let marker_path = dir.join("interrupted");
    fs::write(&marker_path, "").unwrap();
    let mut marker_fut = Box::pin(wait_file(&marker_path, 5));
    assert!(
        tokio::time::timeout(Duration::from_millis(60), &mut marker_fut)
            .await
            .is_err(),
        "wait_file returned on an empty file"
    );
    fs::write(&marker_path, "1").unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(60), &mut marker_fut)
            .await
            .is_err(),
        "wait_file returned before the trailing newline"
    );
    fs::write(&marker_path, "1\n").unwrap();
    assert_eq!(marker_fut.await, "1\n");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn responses_fails_on_malformed_completed_jsonl_line() {
    let dir = scratch("responses-malformed");
    let jsonl_path = dir.join("responses.jsonl");

    // 1. A completed malformed line before a valid record must panic with the
    // malformed line in the diagnostic.
    fs::write(
        &jsonl_path,
        "not-json-at-all\n{\"id\":\"srv-1\",\"result\":{}}\n",
    )
    .unwrap();
    let payload = std::panic::catch_unwind(|| responses(&dir))
        .expect_err("malformed completed JSONL line must fail responses()");
    let msg = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("<non-string panic>");
    assert!(
        msg.contains("not-json-at-all") && msg.contains("line 1"),
        "diagnostic must show the malformed line and line number, got: {msg}"
    );

    // 2. A completed blank line before a valid record must also fail.
    fs::write(&jsonl_path, "\n{\"id\":\"srv-1\",\"result\":{}}\n").unwrap();
    let blank_payload = std::panic::catch_unwind(|| responses(&dir))
        .expect_err("blank completed JSONL line must fail responses()");
    let blank_msg = blank_payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| blank_payload.downcast_ref::<&str>().copied())
        .unwrap_or("<non-string panic>");
    assert!(
        blank_msg.contains("\"\"") && blank_msg.contains("line 1"),
        "diagnostic must show the blank line and line number, got: {blank_msg}"
    );

    // 3. An unfinished trailing line (no newline yet) is ignored.
    fs::write(&jsonl_path, "{\"truncated").unwrap();
    assert!(responses(&dir).is_empty());

    fs::write(
        &jsonl_path,
        "{\"id\":\"srv-1\",\"result\":{}}\n{\"truncated",
    )
    .unwrap();
    let parsed = responses(&dir);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0]["id"], "srv-1");

    let _ = fs::remove_dir_all(&dir);
}
