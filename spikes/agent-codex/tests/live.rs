use atrium_agents::*;
use atrium_protocol::agent::*;
use serde_json::Value;
use spike_agent_codex::{CodexAdapter, process_args, thread_params};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Harness {
    dir: PathBuf,
}
impl Harness {
    fn new() -> Self {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "live/test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }
    fn config(&self, mode: &str, gate: Arc<dyn ToolGate>) -> SessionConfig {
        SessionConfig {
            cwd: self.dir.clone(),
            model: Some("fake-model".into()),
            effort: None,
            permissions: PermissionPolicy::ReadOnly,
            gate,
            mcp_servers: vec![],
            resume: None,
            program: Some(
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/support/server.py"),
            ),
            env: vec![
                ("FAKE_MODE".into(), mode.into()),
                (
                    "FAKE_PID_FILE".into(),
                    self.dir.join("pid").to_string_lossy().into(),
                ),
                (
                    "FAKE_AUDIT_FILE".into(),
                    self.dir.join("audit").to_string_lossy().into(),
                ),
            ],
        }
    }
    fn audit(&self) -> Vec<Value> {
        std::fs::read_to_string(self.dir.join("audit"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
    fn reaped(&self) -> bool {
        let pid = std::fs::read_to_string(self.dir.join("pid")).unwrap();
        !std::process::Command::new("/bin/kill")
            .args(["-0", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
struct Gate(GateDecision);
impl ToolGate for Gate {
    fn check<'a>(&'a self, _: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        Box::pin(async { self.0.clone() })
    }
}
struct PanicGate;
impl ToolGate for PanicGate {
    fn check<'a>(&'a self, _: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        Box::pin(async { panic!("test gate panic") })
    }
}
struct StallGate(Arc<AtomicUsize>);
struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
impl ToolGate for StallGate {
    fn check<'a>(&'a self, _: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        Box::pin(async {
            self.0.fetch_add(1, Ordering::SeqCst);
            let _active = Active(self.0.clone());
            std::future::pending().await
        })
    }
}
async fn collect(session: &mut Session) -> Vec<AgentEvent> {
    tokio::time::timeout(SHUTDOWN_GRACE + Duration::from_secs(1), async {
        let mut result = vec![];
        while let Some(e) = session.events.recv().await {
            result.push(e.event);
        }
        result
    })
    .await
    .expect("session closes")
}
#[tokio::test]
async fn live_uses_same_normalizer_and_reaps() {
    let h = Harness::new();
    let recording = Arc::new(std::sync::Mutex::new(vec![]));
    let mut session = CodexAdapter {
        recording: Some(recording.clone()),
    }
    .start(
        h.config("simple", Arc::new(Gate(GateDecision::Allow))),
        Transport::Local,
    )
    .await
    .unwrap();
    session
        .commands
        .send(SessionCommand::SendUserMessage { text: "hi".into() })
        .await
        .unwrap();
    let mut actual = vec![];
    while let Some(e) = session.events.recv().await {
        let done = matches!(e.event, AgentEvent::TurnCompleted { .. });
        actual.push(e.event);
        if done {
            break;
        }
    }
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
    actual.extend(collect(&mut session).await);
    let expected = atrium_agents::fixture::normalize(
        &recording.lock().unwrap(),
        &mut spike_agent_codex::CodexNormalizer::default(),
    );
    assert_eq!(actual, expected);
    assert!(h.reaped());
}
#[tokio::test]
async fn asks_then_resolves_only_on_server_notification() {
    let h = Harness::new();
    let mut session = CodexAdapter::default()
        .start(
            h.config("gate", Arc::new(Gate(GateDecision::Ask))),
            Transport::Local,
        )
        .await
        .unwrap();
    session
        .commands
        .send(SessionCommand::SendUserMessage { text: "hi".into() })
        .await
        .unwrap();
    while let Some(e) = session.events.recv().await {
        if let AgentEvent::ApprovalRequested { request_id, .. } = e.event {
            assert_eq!(request_id, "77");
            session
                .commands
                .send(SessionCommand::ResolveApproval {
                    request_id,
                    decision: ApprovalDecision::Deny { message: None },
                })
                .await
                .unwrap();
            break;
        }
    }
    let mut saw_resolution = false;
    let mut denied = false;
    while let Some(e) = session.events.recv().await {
        match e.event {
            AgentEvent::RequestResolved {
                outcome: RequestOutcome::Denied,
                ..
            } => saw_resolution = true,
            AgentEvent::ToolCallFinished {
                status: ToolStatus::Denied,
                ..
            } => denied = true,
            AgentEvent::TurnCompleted { .. } => break,
            _ => {}
        }
    }
    assert!(saw_resolution && denied);
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
    collect(&mut session).await;
    assert!(h.reaped());
}
#[tokio::test]
async fn panicking_gate_fails_closed() {
    let h = Harness::new();
    let mut session = CodexAdapter::default()
        .start(h.config("gate", Arc::new(PanicGate)), Transport::Local)
        .await
        .unwrap();
    session
        .commands
        .send(SessionCommand::SendUserMessage { text: "hi".into() })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(e) = session.events.recv().await {
            if matches!(
                e.event,
                AgentEvent::ToolCallFinished {
                    status: ToolStatus::Denied,
                    ..
                }
            ) {
                break;
            }
        }
    })
    .await
    .unwrap();
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
    collect(&mut session).await;
    assert!(
        h.audit()
            .iter()
            .any(|v| v["id"] == 77 && v["result"]["decision"] == "decline")
    );
    assert!(h.reaped());
}
#[tokio::test]
async fn interrupt_cancels_stalled_gate_without_leaving_task() {
    let h = Harness::new();
    let active = Arc::new(AtomicUsize::new(0));
    let mut session = CodexAdapter::default()
        .start(
            h.config("stall-gate", Arc::new(StallGate(active.clone()))),
            Transport::Local,
        )
        .await
        .unwrap();
    session
        .commands
        .send(SessionCommand::SendUserMessage { text: "hi".into() })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while active.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    session
        .commands
        .send(SessionCommand::Interrupt)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(e) = session.events.recv().await {
            if matches!(
                e.event,
                AgentEvent::TurnCompleted {
                    outcome: TurnOutcome::Interrupted,
                    ..
                }
            ) {
                break;
            }
        }
    })
    .await
    .unwrap();
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
    collect(&mut session).await;
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert!(h.reaped());
}
#[tokio::test]
async fn saturation_interrupt_shutdown_and_forced_kill() {
    for mode in ["flood", "ignore-eof"] {
        let h = Harness::new();
        let mut session = CodexAdapter::default()
            .start(
                h.config(mode, Arc::new(Gate(GateDecision::Allow))),
                Transport::Local,
            )
            .await
            .unwrap();
        session
            .commands
            .send(SessionCommand::SendUserMessage {
                text: "flood".into(),
            })
            .await
            .unwrap();
        // Never read events before the owner has terminated. Wait until both
        // output has filled the channel up to its reserved lifecycle slots.
        tokio::time::timeout(Duration::from_secs(2), async {
            while session.events.len() < EVENT_BUFFER - 8 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        session
            .commands
            .send(SessionCommand::Interrupt)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !h.audit().iter().any(|v| v["method"] == "turn/interrupt") {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("provider consumed interrupt under saturation");
        let before = Instant::now();
        session
            .commands
            .send(SessionCommand::Shutdown)
            .await
            .unwrap();
        tokio::time::timeout(SHUTDOWN_GRACE, async {
            while !session.commands.is_closed() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("owner task exits within shutdown grace");
        assert!(before.elapsed() < SHUTDOWN_GRACE);
        assert!(h.reaped());
        // Closed receiver drains to None: no detached event-forwarding task.
        let mut tail = None;
        while let Some(event) = session.events.recv().await {
            tail = Some(event.event);
        }
        assert!(matches!(tail, Some(AgentEvent::SessionEnded { .. })));
    }
}
#[tokio::test]
async fn startup_error_is_reaped() {
    let h = Harness::new();
    let result = CodexAdapter::default()
        .start(
            h.config("startup-fail", Arc::new(Gate(GateDecision::Allow))),
            Transport::Local,
        )
        .await;
    assert!(matches!(result, Err(AdapterError::Handshake(_))));
    assert!(h.reaped());
}
#[test]
fn policies_and_mcp_overrides_are_process_local() {
    let h = Harness::new();
    let mut config = h.config("simple", Arc::new(Gate(GateDecision::Allow)));
    for (policy, sandbox, approval) in [
        (PermissionPolicy::ReadOnly, "read-only", "untrusted"),
        (PermissionPolicy::Ask, "workspace-write", "untrusted"),
        (PermissionPolicy::FullAccess, "danger-full-access", "never"),
    ] {
        config.permissions = policy;
        let params = thread_params(&config);
        assert_eq!(params["sandbox"], sandbox);
        assert_eq!(params["approvalPolicy"], approval);
        assert_eq!(params["approvalsReviewer"], "user");
    }
    config.mcp_servers.push(McpServer {
        name: "quoted.name".into(),
        command: "/tmp/mcp".into(),
        args: vec!["a b".into()],
        env: vec![("KEY".into(), "value".into())],
    });
    let args = process_args(&config);
    assert!(
        args.iter()
            .any(|a| a == "mcp_servers.\"quoted.name\".command=\"/tmp/mcp\"")
    );
    assert!(!args.iter().any(|a| a.contains("config.toml")));
    assert_eq!(
        CodexAdapter::default().interception(),
        Interception::ProviderGated
    );
}

#[tokio::test]
async fn interrupt_during_turn_start_is_not_lost() {
    let h = Harness::new();
    let mut session = CodexAdapter::default()
        .start(
            h.config("delayed-turn", Arc::new(Gate(GateDecision::Ask))),
            Transport::Local,
        )
        .await
        .unwrap();
    session
        .commands
        .send(SessionCommand::SendUserMessage { text: "hi".into() })
        .await
        .unwrap();
    session
        .commands
        .send(SessionCommand::Interrupt)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(e) = session.events.recv().await {
            if matches!(
                e.event,
                AgentEvent::TurnCompleted {
                    outcome: TurnOutcome::Interrupted,
                    ..
                }
            ) {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert!(h.audit().iter().any(|v| v["method"] == "turn/interrupt"));
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .unwrap();
    collect(&mut session).await;
    assert!(h.reaped());
}

#[tokio::test]
async fn dropping_both_halves_shuts_down_and_reaps() {
    let h = Harness::new();
    let session = CodexAdapter::default()
        .start(
            h.config("simple", Arc::new(Gate(GateDecision::Allow))),
            Transport::Local,
        )
        .await
        .unwrap();
    drop(session);
    tokio::time::timeout(SHUTDOWN_GRACE, async {
        while !h.reaped() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
