//! M1-28 follow-ups for `atrium-agents` (M1-26 gate-withdraw minors & nit).
//!
//! Never runs a real provider CLI: uses minimal local Python/shell scripts via
//! `SessionConfig.program` and a real `SessionGate` + `SqliteStore`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use atrium_agents::claude::{ClaudeAdapter, ClaudeOptions};
use atrium_agents::codex::{CodexAdapter, CodexOptions};
use atrium_agents::{
    AgentAdapter, ApprovalTicket, BoxFuture, GateDecision, PermissionPolicy, SHUTDOWN_GRACE,
    Session, SessionConfig, ToolCheck, ToolGate, Transport,
};
use atrium_core::policy::{DefaultPolicy, Ledger, SessionGate, SessionGateConfig, StoreRules};
use atrium_core::store::{SqliteStore, StoreOptions};
use atrium_core::{NewThread, Signal, Store};
use atrium_protocol::agent::{AgentEvent, Provider, SessionCommand};
use atrium_protocol::policy::Preset;
use atrium_protocol::thread::{Location, ThreadId};
use tokio::sync::Notify;

fn scratch(name: &str) -> PathBuf {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "atrium-m1-28-agents-{}-{}-{n}",
        std::process::id(),
        name
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write script");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("chmod 0700");
}

struct SpyGate {
    inner: Arc<SessionGate>,
    asked: Mutex<Vec<ApprovalTicket>>,
    withdrawn: Mutex<Vec<ApprovalTicket>>,
    hang_withdraw: bool,
    on_asked: Option<Arc<Notify>>,
}

impl SpyGate {
    fn new(inner: Arc<SessionGate>, hang_withdraw: bool) -> Arc<Self> {
        Arc::new(Self {
            inner,
            asked: Mutex::default(),
            withdrawn: Mutex::default(),
            hang_withdraw,
            on_asked: None,
        })
    }

    fn with_notify(inner: Arc<SessionGate>, on_asked: Arc<Notify>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            asked: Mutex::default(),
            withdrawn: Mutex::default(),
            hang_withdraw: false,
            on_asked: Some(on_asked),
        })
    }

    fn asked(&self) -> Vec<String> {
        self.asked
            .lock()
            .unwrap()
            .iter()
            .map(|t| t.request_id.clone())
            .collect()
    }

    fn withdrawn(&self) -> Vec<String> {
        self.withdrawn
            .lock()
            .unwrap()
            .iter()
            .map(|t| t.request_id.clone())
            .collect()
    }
}

impl ToolGate for SpyGate {
    fn check<'a>(&'a self, check: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        Box::pin(async move {
            let d = self.inner.check(check).await;
            if let GateDecision::Ask { ticket } = &d {
                self.asked.lock().unwrap().push(ticket.clone());
                if let Some(n) = &self.on_asked {
                    n.notify_one();
                }
            }
            d
        })
    }

    fn redeem<'a>(
        &'a self,
        ticket: &'a ApprovalTicket,
        check: &'a ToolCheck,
    ) -> BoxFuture<'a, GateDecision> {
        self.inner.redeem(ticket, check)
    }

    fn question_ticket(&self, turn: Option<u32>) -> BoxFuture<'_, Option<ApprovalTicket>> {
        Box::pin(async move {
            let t = self.inner.question_ticket(turn).await;
            if let Some(ticket) = &t {
                self.asked.lock().unwrap().push(ticket.clone());
                if let Some(n) = &self.on_asked {
                    n.notify_one();
                }
            }
            t
        })
    }

    fn withdraw<'a>(&'a self, ticket: &'a ApprovalTicket) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.withdrawn.lock().unwrap().push(ticket.clone());
            if self.hang_withdraw {
                std::future::pending::<()>().await;
            } else {
                self.inner.withdraw(ticket).await;
            }
        })
    }
}

struct RealLedger {
    dir: PathBuf,
    store: Arc<SqliteStore>,
    thread: ThreadId,
    inc: u32,
    gate: Arc<SpyGate>,
}

impl Drop for RealLedger {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

impl RealLedger {
    async fn new(provider: Provider, home: &Path, preset: Preset, hang_withdraw: bool) -> Self {
        let dir = scratch("ledger");
        let store = Arc::new(
            SqliteStore::open(&dir.join("atrium.db"), StoreOptions::default())
                .await
                .unwrap(),
        );
        let ledger = Arc::new(Ledger::new(store.requests()));
        let s = store
            .create_thread(NewThread {
                provider,
                model: None,
                effort: None,
                preset,
                location: Location::Home,
                title: "m1-28-followups".into(),
            })
            .await
            .unwrap();
        let inc = store.begin_incarnation(&s.thread_id).await.unwrap();
        store.signal(&s.thread_id, Signal::Starting).await.unwrap();
        let canon_home = fs::canonicalize(home).unwrap();
        let inner = Arc::new(SessionGate::new(
            SessionGateConfig {
                thread: s.thread_id.clone(),
                incarnation: inc,
                cwd: canon_home.clone(),
                home: canon_home,
                atrium_data_paths: vec![],
                atrium_protected_paths: vec![],
                preset,
                provider,
            },
            Arc::new(DefaultPolicy::new()),
            ledger,
            Arc::new(StoreRules(store.clone())),
        ));
        Self {
            dir,
            store,
            thread: s.thread_id,
            inc,
            gate: SpyGate::new(inner, hang_withdraw),
        }
    }

    async fn with_notify(
        provider: Provider,
        home: &Path,
        preset: Preset,
        on_asked: Arc<Notify>,
    ) -> Self {
        let dir = scratch("ledger-notify");
        let store = Arc::new(
            SqliteStore::open(&dir.join("atrium.db"), StoreOptions::default())
                .await
                .unwrap(),
        );
        let ledger = Arc::new(Ledger::new(store.requests()));
        let s = store
            .create_thread(NewThread {
                provider,
                model: None,
                effort: None,
                preset,
                location: Location::Home,
                title: "m1-28-race".into(),
            })
            .await
            .unwrap();
        let inc = store.begin_incarnation(&s.thread_id).await.unwrap();
        store.signal(&s.thread_id, Signal::Starting).await.unwrap();
        let canon_home = fs::canonicalize(home).unwrap();
        let inner = Arc::new(SessionGate::new(
            SessionGateConfig {
                thread: s.thread_id.clone(),
                incarnation: inc,
                cwd: canon_home.clone(),
                home: canon_home,
                atrium_data_paths: vec![],
                atrium_protected_paths: vec![],
                preset,
                provider,
            },
            Arc::new(DefaultPolicy::new()),
            ledger,
            Arc::new(StoreRules(store.clone())),
        ));
        Self {
            dir,
            store,
            thread: s.thread_id,
            inc,
            gate: SpyGate::with_notify(inner, on_asked),
        }
    }

    async fn open_count(&self) -> u64 {
        self.store
            .requests()
            .open_count(&self.thread, self.inc)
            .await
            .unwrap()
    }
}

fn claude_opts(deadline: Duration) -> ClaudeOptions {
    ClaudeOptions {
        managed_settings: Some(vec![]),
        gate_deadline: deadline,
        ..ClaudeOptions::default()
    }
}

fn claude_cfg(dir: &Path, program: PathBuf, gate: Arc<dyn ToolGate>) -> SessionConfig {
    SessionConfig {
        cwd: dir.to_path_buf(),
        model: Some("haiku".into()),
        effort: None,
        permissions: PermissionPolicy::Ask,
        gate,
        mcp_servers: vec![],
        resume: None,
        env: vec![(
            "CLAUDE_CONFIG_DIR".into(),
            dir.join("user").display().to_string(),
        )],
        program: Some(program),
        on_spawn: None,
    }
}

async fn shutdown(mut session: Session) {
    let _ = session.commands.send(SessionCommand::Shutdown).await;
    let deadline = Instant::now() + SHUTDOWN_GRACE + Duration::from_secs(10);
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, session.events.recv()).await {
            Ok(Some(env)) if matches!(env.event, AgentEvent::SessionEnded { .. }) => return,
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => return,
        }
    }
}

async fn wait_for_approvals(session: &mut Session, count: usize) {
    let mut seen = 0;
    let deadline = Instant::now() + Duration::from_secs(5);
    while seen < count {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("timed out waiting for ApprovalRequested");
        match tokio::time::timeout(remaining, session.events.recv()).await {
            Ok(Some(env)) if matches!(env.event, AgentEvent::ApprovalRequested { .. }) => {
                seen += 1;
            }
            Ok(Some(_)) => {}
            Ok(None) => panic!("session ended after {seen}/{count} ApprovalRequested events"),
            Err(_) => panic!("timed out after {seen}/{count} ApprovalRequested events"),
        }
    }
}

#[tokio::test]
async fn claude_tool_result_without_can_use_tool_withdraws_hook_ticket_before_turn_end() {
    let dir = scratch("claude-tool-result");
    let go_result = dir.join("go_result");
    let script = dir.join("fake-claude.py");
    write_executable(
        &script,
        &format!(
            r#"#!/usr/bin/env python3
import json, os, sys, time
if len(sys.argv) > 1 and sys.argv[1] == "--version":
    print("2.1.308 (Claude Code)")
    sys.exit(0)
go_result = {go_result:?}
sid = "fake-s1"
for line in sys.stdin:
    v = json.loads(line)
    t = v.get("type")
    if t == "control_request" and v.get("request", {{}}).get("subtype") == "initialize":
        print(json.dumps({{"type":"control_response","response":{{"subtype":"success","request_id":v["request_id"],"response":{{"commands":[]}}}}}}), flush=True)
    elif t == "user":
        print(json.dumps({{"type":"system","subtype":"init","session_id":sid,"model":"claude-fake","tools":["Read"],"cwd":"."}}), flush=True)
        print(json.dumps({{"type":"assistant","message":{{"model":"claude-fake","id":"m1","content":[{{"type":"tool_use","id":"toolu_1","name":"Read","input":{{"file_path":"x"}}}}]}},"session_id":sid}}), flush=True)
        print(json.dumps({{"type":"control_request","request_id":"h1","request":{{"subtype":"hook_callback","callback_id":"atrium-pre-tool-use","input":{{"hook_event_name":"PreToolUse","tool_name":"Read","tool_input":{{"file_path":"x"}},"tool_use_id":"toolu_1"}}}}}}), flush=True)
    elif t == "control_response" and v.get("response", {{}}).get("request_id") == "h1":
        # Emit tool_result directly without ever emitting can_use_tool, then wait for
        # the test to verify withdrawal BEFORE emitting result.
        print(json.dumps({{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_1","content":"ok","is_error":False}}]}},"session_id":sid}}), flush=True)
        deadline = time.monotonic() + 5.0
        while not os.path.exists(go_result) and time.monotonic() < deadline:
            time.sleep(0.01)
        print(json.dumps({{"type":"result","subtype":"success","session_id":sid,"is_error":False,"num_turns":1,"duration_ms":1}}), flush=True)
"#,
            go_result = go_result.display().to_string(),
        ),
    );

    let ledger = RealLedger::new(Provider::Claude, &dir, Preset::Careful, false).await;
    let mut session = ClaudeAdapter::new(claude_opts(Duration::from_secs(5)))
        .start(
            claude_cfg(&dir, script, ledger.gate.clone()),
            Transport::Local,
        )
        .await
        .unwrap();

    session
        .commands
        .send(SessionCommand::SendUserMessage { text: "go".into() })
        .await
        .unwrap();

    // Wait until the hook Ask ticket was opened AND withdrawn by `tool_result`,
    // before `result` has been emitted.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if ledger.gate.asked().len() == 1
            && ledger.gate.withdrawn() == ledger.gate.asked()
            && ledger.open_count().await == 0
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(ledger.gate.asked().len(), 1);
    assert_eq!(ledger.gate.withdrawn(), ledger.gate.asked());
    assert_eq!(ledger.open_count().await, 0);

    fs::write(&go_result, "ok").unwrap();
    let end_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end_deadline {
        if let Ok(Some(env)) =
            tokio::time::timeout(Duration::from_secs(2), session.events.recv()).await
            && matches!(env.event, AgentEvent::TurnCompleted { .. })
        {
            break;
        }
    }
    shutdown(session).await;
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn claude_hung_withdrawals_finish_within_one_deadline() {
    let dir = scratch("claude-hung-withdraw");
    let script = dir.join("fake-claude-5.py");
    write_executable(
        &script,
        r#"#!/usr/bin/env python3
import json, sys
if len(sys.argv) > 1 and sys.argv[1] == "--version":
    print("2.1.308 (Claude Code)")
    sys.exit(0)
sid = "fake-s2"
for line in sys.stdin:
    v = json.loads(line)
    t = v.get("type")
    if t == "control_request" and v.get("request", {}).get("subtype") == "initialize":
        print(json.dumps({"type":"control_response","response":{"subtype":"success","request_id":v["request_id"],"response":{"commands":[]}}}), flush=True)
    elif t == "user":
        print(json.dumps({"type":"system","subtype":"init","session_id":sid,"model":"claude-fake","tools":["Bash"],"cwd":"."}), flush=True)
        for i in range(1, 6):
            print(json.dumps({"type":"control_request","request_id":f"c{i}","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":f"tu_{i}","input":{"command":"ls"}}}), flush=True)
"#,
    );

    let ledger = RealLedger::new(Provider::Claude, &dir, Preset::Careful, true).await;
    let deadline = Duration::from_millis(150);
    let mut session = ClaudeAdapter::new(claude_opts(deadline))
        .start(
            claude_cfg(&dir, script, ledger.gate.clone()),
            Transport::Local,
        )
        .await
        .unwrap();

    session
        .commands
        .send(SessionCommand::SendUserMessage { text: "go".into() })
        .await
        .unwrap();

    wait_for_approvals(&mut session, 5).await;
    assert_eq!(ledger.gate.asked().len(), 5);

    let started = Instant::now();
    shutdown(session).await;
    let elapsed = started.elapsed();

    assert_eq!(ledger.gate.withdrawn().len(), 5);
    assert!(
        elapsed < Duration::from_millis(450),
        "5 hung withdrawals took {elapsed:?}; expected <= ~1 deadline (150ms), not 5 * 150ms"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn codex_hung_withdrawals_finish_within_one_deadline() {
    let dir = scratch("codex-hung-withdraw");
    let user_dir = dir.join("empty-codex-home");
    fs::create_dir_all(&user_dir).unwrap();
    let script = dir.join("fake-codex-3.py");
    write_executable(
        &script,
        r#"#!/usr/bin/env python3
import json, sys
if len(sys.argv) > 1 and sys.argv[1] == "--version":
    print("codex-cli 0.163.0")
    sys.exit(0)
for line in sys.stdin:
    v = json.loads(line)
    m = v.get("method")
    rid = v.get("id")
    if m == "initialize":
        print(json.dumps({"id": rid, "result": {"userAgent": "codex-fake/0.163.0", "codexHome": "/scratch/.codex"}}), flush=True)
    elif m == "thread/start":
        print(json.dumps({"id": rid, "result": {"thread": {"id": "th_fake", "cliVersion": "0.163.0"}, "approvalPolicy": "untrusted", "sandbox": {"type": "workspaceWrite", "networkAccess": False}}}), flush=True)
    elif m == "turn/start":
        print(json.dumps({"id": rid, "result": {"turn": {"id": "turn_1", "items": [], "status": "inProgress"}}}), flush=True)
        print(json.dumps({"method": "turn/started", "params": {"threadId": "th_fake", "turn": {"id": "turn_1", "status": "inProgress"}}}), flush=True)
        for i in range(1, 4):
            print(json.dumps({"id": f"req_{i}", "method": "item/commandExecution/requestApproval", "params": {"threadId": "th_fake", "turnId": "turn_1", "itemId": f"item_{i}", "command": "ls", "cwd": "."}}), flush=True)
"#,
    );

    let ledger = RealLedger::new(Provider::Codex, &dir, Preset::Careful, true).await;
    let cfg = SessionConfig {
        cwd: dir.clone(),
        model: Some("gpt-5".into()),
        effort: None,
        permissions: PermissionPolicy::Ask,
        resume: None,
        mcp_servers: vec![],
        gate: ledger.gate.clone(),
        env: vec![("CODEX_HOME".into(), user_dir.display().to_string())],
        program: Some(script),
        on_spawn: None,
    };
    let opts = CodexOptions {
        user_dir: Some(user_dir),
        ..CodexOptions::default()
    };
    let mut session = CodexAdapter::with_options(opts)
        .start(cfg, Transport::Local)
        .await
        .unwrap();

    session
        .commands
        .send(SessionCommand::SendUserMessage { text: "go".into() })
        .await
        .unwrap();

    wait_for_approvals(&mut session, 3).await;
    assert_eq!(ledger.gate.asked().len(), 3);

    // Shutdown cancels the 3 open requests and drains their hung withdrawals under
    // one REDEEM_TIMEOUT (10s), instead of 3 * 10s = 30s.
    let started = Instant::now();
    let _ = session.commands.send(SessionCommand::Shutdown).await;
    let end_deadline = Instant::now() + Duration::from_secs(20);
    while let Some(remaining) = end_deadline.checked_duration_since(Instant::now()) {
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, session.events.recv()).await {
            Ok(Some(env)) if matches!(env.event, AgentEvent::SessionEnded { .. }) => break,
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => break,
        }
    }
    let elapsed = started.elapsed();
    assert_eq!(ledger.gate.withdrawn().len(), 3);
    assert!(
        elapsed < Duration::from_secs(16),
        "3 hung Codex withdrawals took {elapsed:?}; expected ~10s, not 30s"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_clear_requests_withdraws_in_flight_gate_ticket() {
    for round in 0..8 {
        let dir = scratch(&format!("claude-clear-race-{round}"));
        let script = dir.join("fake-claude-race.py");
        let trigger = dir.join("emit_result");
        write_executable(
            &script,
            &format!(
                r#"#!/usr/bin/env python3
import json, os, sys, time
if len(sys.argv) > 1 and sys.argv[1] == "--version":
    print("2.1.308 (Claude Code)")
    sys.exit(0)
trigger = {trigger:?}
sid = "fake-race"
for line in sys.stdin:
    v = json.loads(line)
    t = v.get("type")
    if t == "control_request" and v.get("request", {{}}).get("subtype") == "initialize":
        print(json.dumps({{"type":"control_response","response":{{"subtype":"success","request_id":v["request_id"],"response":{{"commands":[]}}}}}}), flush=True)
    elif t == "user":
        print(json.dumps({{"type":"system","subtype":"init","session_id":sid,"model":"claude-fake","tools":["Read"],"cwd":"."}}), flush=True)
        print(json.dumps({{"type":"control_request","request_id":"h1","request":{{"subtype":"hook_callback","callback_id":"atrium-pre-tool-use","input":{{"hook_event_name":"PreToolUse","tool_name":"Read","tool_input":{{"file_path":"x"}},"tool_use_id":"toolu_1"}}}}}}), flush=True)
        deadline = time.monotonic() + 5.0
        while not os.path.exists(trigger) and time.monotonic() < deadline:
            time.sleep(0.002)
        print(json.dumps({{"type":"result","subtype":"success","session_id":sid,"is_error":False,"num_turns":1,"duration_ms":1}}), flush=True)
"#,
                trigger = trigger.display().to_string(),
            ),
        );

        let notify = Arc::new(Notify::new());
        let ledger =
            RealLedger::with_notify(Provider::Claude, &dir, Preset::Careful, notify.clone()).await;
        let mut session = ClaudeAdapter::new(claude_opts(Duration::from_secs(5)))
            .start(
                claude_cfg(&dir, script, ledger.gate.clone()),
                Transport::Local,
            )
            .await
            .unwrap();

        session
            .commands
            .send(SessionCommand::SendUserMessage { text: "go".into() })
            .await
            .unwrap();

        // As soon as `gate.check` opens a ticket, immediately trigger `result`
        // (and Shutdown on alternate rounds) to race `clear_requests` against
        // delivering `Internal::Gate`.
        tokio::time::timeout(Duration::from_secs(5), notify.notified())
            .await
            .expect("gate asked");
        fs::write(&trigger, "now").unwrap();
        if round % 2 == 1 {
            let _ = session.commands.send(SessionCommand::Shutdown).await;
        } else {
            let end_deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < end_deadline {
                if let Ok(Some(env)) =
                    tokio::time::timeout(Duration::from_secs(2), session.events.recv()).await
                    && matches!(env.event, AgentEvent::TurnCompleted { .. })
                {
                    break;
                }
            }
        }
        shutdown(session).await;

        assert_eq!(ledger.gate.asked().len(), 1, "round {round}");
        assert_eq!(
            ledger.gate.withdrawn(),
            ledger.gate.asked(),
            "round {round} leaked ticket"
        );
        assert_eq!(ledger.open_count().await, 0, "round {round}");
        let _ = fs::remove_dir_all(&dir);
    }
}
