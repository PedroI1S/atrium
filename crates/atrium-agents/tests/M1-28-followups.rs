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
use atrium_core::{ApprovalLedger, NewThread, RequestKey, Signal, Store};
use atrium_protocol::agent::{AgentEvent, Provider, SessionCommand};
use atrium_protocol::limits::MAX_PENDING_REQUESTS;
use atrium_protocol::policy::Preset;
use atrium_protocol::thread::{Location, ThreadId};
use tokio::sync::{Notify, watch};

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
    check_barrier: Option<(Arc<Notify>, Arc<Notify>)>,
}

impl SpyGate {
    fn new(inner: Arc<SessionGate>, hang_withdraw: bool) -> Arc<Self> {
        Arc::new(Self {
            inner,
            asked: Mutex::default(),
            withdrawn: Mutex::default(),
            hang_withdraw,
            on_asked: None,
            check_barrier: None,
        })
    }

    fn with_notify(inner: Arc<SessionGate>, on_asked: Arc<Notify>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            asked: Mutex::default(),
            withdrawn: Mutex::default(),
            hang_withdraw: false,
            on_asked: Some(on_asked),
            check_barrier: None,
        })
    }

    fn with_check_barrier(
        inner: Arc<SessionGate>,
        on_inserted: Arc<Notify>,
        allow_return: Arc<Notify>,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner,
            asked: Mutex::default(),
            withdrawn: Mutex::default(),
            hang_withdraw: false,
            on_asked: None,
            check_barrier: Some((on_inserted, allow_return)),
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
            if let Some((on_inserted, allow_return)) = &self.check_barrier {
                on_inserted.notify_one();
                allow_return.notified().await;
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

    async fn with_check_barrier(
        provider: Provider,
        home: &Path,
        preset: Preset,
        on_inserted: Arc<Notify>,
        allow_return: Arc<Notify>,
    ) -> Self {
        let dir = scratch("ledger-barrier");
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
                title: "m1-28-barrier".into(),
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
            gate: SpyGate::with_check_barrier(inner, on_inserted, allow_return),
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
    // Keep `gate_deadline` at its default (20 s): it also bounds real SQLite
    // checks and tickets, so shrinking it causes spurious timeouts under load;
    // bound only `drain_withdrawals` via `with_withdraw_timeout`.
    let mut session = ClaudeAdapter::new(claude_opts(ClaudeOptions::default().gate_deadline))
        .with_withdraw_timeout(deadline)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_clear_requests_withdraws_pending_insert_before_check_returns() {
    for mode in ["result", "shutdown"] {
        let dir = scratch(&format!("claude-pending-insert-{mode}"));
        let script = dir.join("fake-claude-pending.py");
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
sid = "fake-pending"
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

        let on_inserted = Arc::new(Notify::new());
        let allow_return = Arc::new(Notify::new());
        let ledger = RealLedger::with_check_barrier(
            Provider::Claude,
            &dir,
            Preset::Careful,
            on_inserted.clone(),
            allow_return.clone(),
        )
        .await;
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

        // Wait until the SQLite approval insert has committed, while `gate.check`
        // has NOT yet returned to `ClaudeAdapter`.
        tokio::time::timeout(Duration::from_secs(5), on_inserted.notified())
            .await
            .expect("approval insert committed");
        assert_eq!(ledger.open_count().await, 1, "{mode}: row inserted");

        if mode == "result" {
            fs::write(&trigger, "now").unwrap();
            // Wait for `result` -> `TurnCompleted` so `clear_requests()` runs
            // before `gate.check` returns.
            let end_deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < end_deadline {
                if let Ok(Some(env)) =
                    tokio::time::timeout(Duration::from_secs(2), session.events.recv()).await
                    && matches!(env.event, AgentEvent::TurnCompleted { .. })
                {
                    break;
                }
            }
            allow_return.notify_one();
            tokio::time::sleep(Duration::from_millis(20)).await;
        } else {
            // Cancel via Shutdown while `gate.check` is still paused; unblock it
            // shortly after `finish()` starts cancelling in-flight gates.
            let release = allow_return.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                release.notify_one();
            });
        }

        shutdown(session).await;

        assert_eq!(ledger.gate.asked().len(), 1, "{mode}");
        assert_eq!(
            ledger.gate.withdrawn(),
            ledger.gate.asked(),
            "{mode} leaked pending ticket inserted before check returned"
        );
        assert_eq!(ledger.open_count().await, 0, "{mode}");
        let _ = fs::remove_dir_all(&dir);
    }
}

/// Round 2, finding 5 (Claude): if `gate.check` commits a ticket and returns
/// *after* the 500 ms `wait_for_in_flight_gates` cap while terminal event
/// delivery is blocked on a full `events_tx`, `OrphanGuard` must still withdraw
/// the ticket directly instead of queuing it into an unread `internal_rx`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_late_orphan_withdrawal_after_cap_with_event_backpressure() {
    let dir = scratch("claude-late-orphan-bp");
    let script = dir.join("fake-claude-late-bp.py");

    let on_inserted = Arc::new(Notify::new());
    let allow_return = Arc::new(Notify::new());
    let ledger = RealLedger::with_check_barrier(
        Provider::Claude,
        &dir,
        Preset::Careful,
        on_inserted.clone(),
        allow_return.clone(),
    )
    .await;

    // Emit enough deltas before `PreToolUse` to fill `session.events` plus
    // leave buffered events in `self.pending` so shutdown blocks on `events_tx`.
    // (Exact capacity is checked below; 1200 exceeds `EVENT_BUFFER` while
    // staying well below `PENDING_LIMIT` = 4096.)
    write_executable(
        &script,
        r#"#!/usr/bin/env python3
import json, sys
if len(sys.argv) > 1 and sys.argv[1] == "--version":
    print("2.1.308 (Claude Code)")
    sys.exit(0)
sid = "fake-late-bp"
for line in sys.stdin:
    v = json.loads(line)
    t = v.get("type")
    if t == "control_request" and v.get("request", {}).get("subtype") == "initialize":
        print(json.dumps({"type":"control_response","response":{"subtype":"success","request_id":v["request_id"],"response":{"commands":[]}}}), flush=True)
    elif t == "user":
        print(json.dumps({"type":"system","subtype":"init","session_id":sid,"model":"claude-fake","tools":["Read"],"cwd":"."}), flush=True)
        for _ in range(1200):
            print(json.dumps({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}},"session_id":sid,"api_message_id":"msg_bp"}), flush=True)
        print(json.dumps({"type":"control_request","request_id":"h1","request":{"subtype":"hook_callback","callback_id":"atrium-pre-tool-use","input":{"hook_event_name":"PreToolUse","tool_name":"Read","tool_input":{"file_path":"x"},"tool_use_id":"toolu_1"}}}), flush=True)
"#,
    );

    let mut session = ClaudeAdapter::new(claude_opts(Duration::from_secs(5)))
        .start(
            claude_cfg(&dir, script, ledger.gate.clone()),
            Transport::Local,
        )
        .await
        .unwrap();
    assert!(
        session.events.max_capacity() < 1200,
        "test flood must exceed session.events capacity"
    );

    session
        .commands
        .send(SessionCommand::SendUserMessage { text: "go".into() })
        .await
        .unwrap();

    // Do NOT drain `session.events`: wait until the approval row is inserted
    // (by which point all 1200 preceding deltas have already filled `events_tx`).
    tokio::time::timeout(Duration::from_secs(5), on_inserted.notified())
        .await
        .expect("approval insert committed");
    assert_eq!(ledger.open_count().await, 1);

    // Trigger Shutdown and let the 500 ms `wait_for_in_flight_gates` cap expire
    // so the actor is parked in the terminal `events_tx.send(ev).await` loop.
    let _ = session.commands.send(SessionCommand::Shutdown).await;
    tokio::time::sleep(Duration::from_millis(750)).await;

    // Release the paused `gate.check` while terminal delivery is still blocked.
    allow_return.notify_one();

    // Drain events so the session can complete cleanly.
    let end_deadline = Instant::now() + Duration::from_secs(10);
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

    let settle = Instant::now() + Duration::from_secs(2);
    while Instant::now() < settle {
        if ledger.gate.withdrawn() == ledger.gate.asked() && ledger.open_count().await == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(ledger.gate.asked().len(), 1);
    assert_eq!(
        ledger.gate.withdrawn(),
        ledger.gate.asked(),
        "late Claude orphan withdrawal entered closed/unread channel instead of withdrawing"
    );
    assert_eq!(ledger.open_count().await, 0);
    let _ = fs::remove_dir_all(&dir);
}

/// Round 2, finding 5 (Codex): same late-return + terminal-backpressure case
/// for `CodexAdapter` (`flush_final` blocked after the 500 ms cap).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_late_orphan_withdrawal_after_cap_with_event_backpressure() {
    let dir = scratch("codex-late-orphan-bp");
    let user_dir = dir.join("empty-codex-home");
    fs::create_dir_all(&user_dir).unwrap();
    let script = dir.join("fake-codex-late-bp.py");
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
        print(json.dumps({"method": "item/started", "params": {"threadId": "th_fake", "turnId": "turn_1", "item": {"type": "agentMessage", "id": "msg_bp", "text": ""}}}), flush=True)
        for _ in range(1200):
            print(json.dumps({"method": "item/agentMessage/delta", "params": {"threadId": "th_fake", "turnId": "turn_1", "itemId": "msg_bp", "delta": "x"}}), flush=True)
        print(json.dumps({"id": "req_1", "method": "item/commandExecution/requestApproval", "params": {"threadId": "th_fake", "turnId": "turn_1", "itemId": "item_1", "command": "ls", "cwd": "."}}), flush=True)
"#,
    );

    let on_inserted = Arc::new(Notify::new());
    let allow_return = Arc::new(Notify::new());
    let ledger = RealLedger::with_check_barrier(
        Provider::Codex,
        &dir,
        Preset::Careful,
        on_inserted.clone(),
        allow_return.clone(),
    )
    .await;
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
    assert!(
        session.events.max_capacity() < 1200,
        "test flood must exceed session.events capacity"
    );

    session
        .commands
        .send(SessionCommand::SendUserMessage { text: "go".into() })
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(5), on_inserted.notified())
        .await
        .expect("approval insert committed");
    assert_eq!(ledger.open_count().await, 1);

    let _ = session.commands.send(SessionCommand::Shutdown).await;
    tokio::time::sleep(Duration::from_millis(750)).await;

    // Release the paused check while `flush_final()` is blocked on `events_tx`.
    allow_return.notify_one();

    let end_deadline = Instant::now() + Duration::from_secs(10);
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

    let settle = Instant::now() + Duration::from_secs(2);
    while Instant::now() < settle {
        if ledger.gate.withdrawn() == ledger.gate.asked() && ledger.open_count().await == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(ledger.gate.asked().len(), 1);
    assert_eq!(
        ledger.gate.withdrawn(),
        ledger.gate.asked(),
        "late Codex orphan withdrawal entered closed/unread channel instead of withdrawing"
    );
    assert_eq!(ledger.open_count().await, 0);
    let _ = fs::remove_dir_all(&dir);
}

/// Gate wrapper that pauses `check()` after the real `SessionGate` inserts its
/// approval row until `release` turns `true`, while counting total `check()`
/// invocations and withdrawals.
struct BoundedBarrierGate {
    inner: Arc<SessionGate>,
    check_calls: AtomicUsize,
    check_entered: Arc<Notify>,
    release: watch::Receiver<bool>,
    asked: Mutex<Vec<ApprovalTicket>>,
    withdrawn: Mutex<Vec<ApprovalTicket>>,
}

impl BoundedBarrierGate {
    fn new(inner: Arc<SessionGate>) -> (Arc<Self>, watch::Sender<bool>) {
        let (tx, rx) = watch::channel(false);
        let gate = Arc::new(Self {
            inner,
            check_calls: AtomicUsize::new(0),
            check_entered: Arc::new(Notify::new()),
            release: rx,
            asked: Mutex::default(),
            withdrawn: Mutex::default(),
        });
        (gate, tx)
    }

    fn check_calls(&self) -> usize {
        self.check_calls.load(Ordering::Acquire)
    }

    fn asked_len(&self) -> usize {
        self.asked.lock().unwrap().len()
    }

    fn withdrawn_len(&self) -> usize {
        self.withdrawn.lock().unwrap().len()
    }
}

impl ToolGate for BoundedBarrierGate {
    fn check<'a>(&'a self, check: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        Box::pin(async move {
            self.check_calls.fetch_add(1, Ordering::AcqRel);
            let d = self.inner.check(check).await;
            if let GateDecision::Ask { ticket } = &d {
                self.asked.lock().unwrap().push(ticket.clone());
            }
            self.check_entered.notify_waiters();
            let mut rx = self.release.clone();
            while !*rx.borrow() {
                if rx.changed().await.is_err() {
                    break;
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
        self.inner.question_ticket(turn)
    }

    fn withdraw<'a>(&'a self, ticket: &'a ApprovalTicket) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.withdrawn.lock().unwrap().push(ticket.clone());
            self.inner.withdraw(ticket).await;
        })
    }
}

async fn make_session_gate(
    provider: Provider,
    home: &Path,
    preset: Preset,
) -> (PathBuf, Arc<SqliteStore>, ThreadId, u32, Arc<SessionGate>) {
    let dir = scratch("ledger-bounded");
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
            title: "m1-33-bounded-gate".into(),
        })
        .await
        .unwrap();
    let inc = store.begin_incarnation(&s.thread_id).await.unwrap();
    store.signal(&s.thread_id, Signal::Starting).await.unwrap();
    let canon_home = fs::canonicalize(home).unwrap();
    let gate = Arc::new(SessionGate::new(
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
    (dir, store, s.thread_id, inc, gate)
}

/// M1-33 finding 1 (Claude): repeatedly submitting and cancelling native
/// requests while `gate.check` is blocked must not accumulate unbounded inner
/// tasks; excess requests are declined explicitly and capacity recovers once
/// cancelled calls settle and withdraw.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_bounded_inner_gate_calls_decline_excess_and_recover() {
    let dir = scratch("claude-bounded-gate");
    let script = dir.join("fake-claude-bounded.py");
    let recovery_trigger = dir.join("send_recovery");
    let denied_log = dir.join("denied.txt");
    let batch_size: usize = 16;
    let batches: usize = MAX_PENDING_REQUESTS / batch_size;
    assert_eq!(batches * batch_size, MAX_PENDING_REQUESTS);
    let excess_count: usize = 8;

    write_executable(
        &script,
        &format!(
            r#"#!/usr/bin/env python3
import json, os, sys, threading, time
if len(sys.argv) > 1 and sys.argv[1] == "--version":
    print("2.1.308 (Claude Code)")
    sys.exit(0)
sid = "fake-bounded"
batch_size = {batch_size}
batches = {batches}
excess = {excess}
trigger_dir = {trigger_dir:?}
recovery_trigger = {recovery_trigger:?}
denied_log = {denied_log:?}

def wait_for(path):
    deadline = time.monotonic() + 10.0
    while not os.path.exists(path) and time.monotonic() < deadline:
        time.sleep(0.005)

def emit_flow():
    print(json.dumps({{"type":"system","subtype":"init","session_id":sid,"model":"claude-fake","tools":["Read"],"cwd":"."}}), flush=True)
    # Submit in batches, wait until each batch has entered the blocked gate,
    # then cancel every request in the batch so outer bookkeeping is cleared
    # while all `MAX_PENDING_REQUESTS` inner tasks remain blocked.
    for b in range(batches):
        base = b * batch_size
        for i in range(base, base + batch_size):
            rid = f"cancel_{{i}}"
            print(json.dumps({{"type":"control_request","request_id":rid,"request":{{"subtype":"can_use_tool","tool_name":"Read","tool_use_id":f"tu_c_{{i}}","input":{{"file_path":"x"}}}}}}), flush=True)
        wait_for(os.path.join(trigger_dir, f"cancel_{{b}}"))
        for i in range(base, base + batch_size):
            print(json.dumps({{"type":"control_cancel_request","request_id":f"cancel_{{i}}"}}), flush=True)
    # Pipe FIFO guarantees all cancellations are processed before these excess requests.
    for i in range(excess):
        rid = f"excess_{{i}}"
        print(json.dumps({{"type":"control_request","request_id":rid,"request":{{"subtype":"can_use_tool","tool_name":"Read","tool_use_id":f"tu_e_{{i}}","input":{{"file_path":"x"}}}}}}), flush=True)
    wait_for(recovery_trigger)
    print(json.dumps({{"type":"control_request","request_id":"recover_1","request":{{"subtype":"can_use_tool","tool_name":"Read","tool_use_id":"tu_rec","input":{{"file_path":"x"}}}}}}), flush=True)

for line in sys.stdin:
    v = json.loads(line)
    t = v.get("type")
    if t == "control_request" and v.get("request", {{}}).get("subtype") == "initialize":
        print(json.dumps({{"type":"control_response","response":{{"subtype":"success","request_id":v["request_id"],"response":{{"commands":[]}}}}}}), flush=True)
    elif t == "user":
        threading.Thread(target=emit_flow, daemon=True).start()
    elif t == "control_response":
        resp = v.get("response", {{}})
        rid = resp.get("request_id", "")
        beh = resp.get("response", {{}}).get("behavior")
        if rid.startswith("excess_") and beh == "deny":
            with open(denied_log, "a", encoding="utf-8") as f:
                f.write(rid + "\n")
"#,
            batch_size = batch_size,
            batches = batches,
            excess = excess_count,
            trigger_dir = dir.display().to_string(),
            recovery_trigger = recovery_trigger.display().to_string(),
            denied_log = denied_log.display().to_string(),
        ),
    );

    let (ledger_dir, store, thread, inc, session_gate) =
        make_session_gate(Provider::Claude, &dir, Preset::Careful).await;
    let (gate, release) = BoundedBarrierGate::new(session_gate);

    let mut session = ClaudeAdapter::new(claude_opts(Duration::from_secs(30)))
        .start(claude_cfg(&dir, script, gate.clone()), Transport::Local)
        .await
        .unwrap();

    session
        .commands
        .send(SessionCommand::SendUserMessage { text: "go".into() })
        .await
        .unwrap();

    // Let each batch commit into SQLite and park on the gate barrier before cancelling it.
    for b in 0..batches {
        let target = (b + 1) * batch_size;
        let deadline = Instant::now() + Duration::from_secs(10);
        while gate.asked_len() < target && Instant::now() < deadline {
            let notified = gate.check_entered.notified();
            let _ = tokio::time::timeout(Duration::from_millis(20), notified).await;
        }
        assert_eq!(gate.asked_len(), target, "batch {b} did not reach gate");
        fs::write(dir.join(format!("cancel_{b}")), "go").unwrap();
    }

    // All `excess_count` requests sent right after the final batch's cancellations
    // must be declined explicitly without entering `gate.check`.
    let deny_deadline = Instant::now() + Duration::from_secs(10);
    let mut denied: Vec<String> = Vec::new();
    while Instant::now() < deny_deadline {
        if let Ok(text) = fs::read_to_string(&denied_log) {
            denied = text.lines().map(str::to_owned).collect();
            if denied.len() == excess_count {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        denied.len(),
        excess_count,
        "expected all {excess_count} excess Claude requests to be explicitly denied, got {denied:?}"
    );
    assert_eq!(
        gate.check_calls(),
        MAX_PENDING_REQUESTS,
        "excess requests must not enter inner gate.check while cancelled calls hold capacity"
    );

    // Release the blocked inner checks so they settle and withdraw their tickets.
    let _ = release.send(true);
    let settle_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < settle_deadline {
        let open = store.requests().open_count(&thread, inc).await.unwrap();
        if gate.withdrawn_len() == MAX_PENDING_REQUESTS && open == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(gate.asked_len(), MAX_PENDING_REQUESTS);
    assert_eq!(gate.withdrawn_len(), MAX_PENDING_REQUESTS);
    assert_eq!(store.requests().open_count(&thread, inc).await.unwrap(), 0);

    // After capacity is returned, a new request is admitted normally.
    fs::write(&recovery_trigger, "go").unwrap();
    wait_for_approvals(&mut session, 1).await;
    assert_eq!(gate.check_calls(), MAX_PENDING_REQUESTS + 1);

    shutdown(session).await;
    let _ = fs::remove_dir_all(&ledger_dir);
    let _ = fs::remove_dir_all(&dir);
}

/// M1-33 finding 1 (Codex): repeatedly submitting and cancelling native
/// requests (`serverRequest/resolved`) while `gate.check` is blocked must not
/// accumulate unbounded inner tasks; excess requests are declined explicitly
/// and capacity recovers once cancelled calls settle and withdraw.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_bounded_inner_gate_calls_decline_excess_and_recover() {
    let dir = scratch("codex-bounded-gate");
    let user_dir = dir.join("empty-codex-home");
    fs::create_dir_all(&user_dir).unwrap();
    let script = dir.join("fake-codex-bounded.py");
    let recovery_trigger = dir.join("send_recovery");
    let declined_log = dir.join("declined.txt");
    let batch_size: usize = 16;
    let batches: usize = MAX_PENDING_REQUESTS / batch_size;
    assert_eq!(batches * batch_size, MAX_PENDING_REQUESTS);
    let excess_count: usize = 8;

    write_executable(
        &script,
        &format!(
            r#"#!/usr/bin/env python3
import json, os, sys, threading, time
if len(sys.argv) > 1 and sys.argv[1] == "--version":
    print("codex-cli 0.163.0")
    sys.exit(0)
batch_size = {batch_size}
batches = {batches}
excess = {excess}
trigger_dir = {trigger_dir:?}
recovery_trigger = {recovery_trigger:?}
declined_log = {declined_log:?}

def wait_for(path):
    deadline = time.monotonic() + 10.0
    while not os.path.exists(path) and time.monotonic() < deadline:
        time.sleep(0.005)

def emit_flow():
    # Submit in batches of 16 (well below the outer live+approvals bound),
    # wait until each batch has entered the blocked gate, then cancel via
    # `serverRequest/resolved` so outer bookkeeping clears while inner tasks stay blocked.
    for b in range(batches):
        base = b * batch_size
        for i in range(base, base + batch_size):
            rid = f"cancel_{{i}}"
            print(json.dumps({{"id": rid, "method": "item/commandExecution/requestApproval", "params": {{"threadId": "th_fake", "turnId": "turn_1", "itemId": f"item_c_{{i}}", "command": "ls", "cwd": "."}}}}), flush=True)
        wait_for(os.path.join(trigger_dir, f"cancel_{{b}}"))
        for i in range(base, base + batch_size):
            print(json.dumps({{"method": "serverRequest/resolved", "params": {{"threadId": "th_fake", "requestId": f"cancel_{{i}}"}}}}), flush=True)
    for i in range(excess):
        rid = f"excess_{{i}}"
        print(json.dumps({{"id": rid, "method": "item/commandExecution/requestApproval", "params": {{"threadId": "th_fake", "turnId": "turn_1", "itemId": f"item_e_{{i}}", "command": "ls", "cwd": "."}}}}), flush=True)
    wait_for(recovery_trigger)
    print(json.dumps({{"id": "recover_1", "method": "item/commandExecution/requestApproval", "params": {{"threadId": "th_fake", "turnId": "turn_1", "itemId": "item_rec", "command": "ls", "cwd": "."}}}}), flush=True)

for line in sys.stdin:
    v = json.loads(line)
    m = v.get("method")
    rid = v.get("id")
    if m == "initialize":
        print(json.dumps({{"id": rid, "result": {{"userAgent": "codex-fake/0.163.0", "codexHome": "/scratch/.codex"}}}}), flush=True)
    elif m == "thread/start":
        print(json.dumps({{"id": rid, "result": {{"thread": {{"id": "th_fake", "cliVersion": "0.163.0"}}, "approvalPolicy": "untrusted", "sandbox": {{"type": "workspaceWrite", "networkAccess": False}}}}}}), flush=True)
    elif m == "turn/start":
        print(json.dumps({{"id": rid, "result": {{"turn": {{"id": "turn_1", "items": [], "status": "inProgress"}}}}}}), flush=True)
        print(json.dumps({{"method": "turn/started", "params": {{"threadId": "th_fake", "turn": {{"id": "turn_1", "status": "inProgress"}}}}}}), flush=True)
        threading.Thread(target=emit_flow, daemon=True).start()
    elif m is None and isinstance(rid, str) and rid.startswith("excess_"):
        if v.get("result", {{}}).get("decision") == "decline":
            with open(declined_log, "a", encoding="utf-8") as f:
                f.write(rid + "\n")
"#,
            batch_size = batch_size,
            batches = batches,
            excess = excess_count,
            trigger_dir = dir.display().to_string(),
            recovery_trigger = recovery_trigger.display().to_string(),
            declined_log = declined_log.display().to_string(),
        ),
    );

    let (ledger_dir, store, thread, inc, session_gate) =
        make_session_gate(Provider::Codex, &dir, Preset::Careful).await;
    let (gate, release) = BoundedBarrierGate::new(session_gate);

    let cfg = SessionConfig {
        cwd: dir.clone(),
        model: Some("gpt-5".into()),
        effort: None,
        permissions: PermissionPolicy::Ask,
        resume: None,
        mcp_servers: vec![],
        gate: gate.clone(),
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

    for b in 0..batches {
        let target = (b + 1) * batch_size;
        let deadline = Instant::now() + Duration::from_secs(10);
        while gate.asked_len() < target && Instant::now() < deadline {
            let notified = gate.check_entered.notified();
            let _ = tokio::time::timeout(Duration::from_millis(20), notified).await;
        }
        assert_eq!(gate.asked_len(), target, "batch {b} did not reach gate");
        fs::write(dir.join(format!("cancel_{b}")), "go").unwrap();
    }

    let decline_deadline = Instant::now() + Duration::from_secs(10);
    let mut declined: Vec<String> = Vec::new();
    while Instant::now() < decline_deadline {
        if let Ok(text) = fs::read_to_string(&declined_log) {
            declined = text.lines().map(str::to_owned).collect();
            if declined.len() == excess_count {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        declined.len(),
        excess_count,
        "expected all {excess_count} excess Codex requests to be explicitly declined, got {declined:?}"
    );
    assert_eq!(
        gate.check_calls(),
        MAX_PENDING_REQUESTS,
        "excess requests must not enter inner gate.check while cancelled calls hold capacity"
    );

    let _ = release.send(true);
    let settle_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < settle_deadline {
        let open = store.requests().open_count(&thread, inc).await.unwrap();
        if gate.withdrawn_len() == MAX_PENDING_REQUESTS && open == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(gate.asked_len(), MAX_PENDING_REQUESTS);
    assert_eq!(gate.withdrawn_len(), MAX_PENDING_REQUESTS);
    assert_eq!(store.requests().open_count(&thread, inc).await.unwrap(), 0);

    fs::write(&recovery_trigger, "go").unwrap();
    wait_for_approvals(&mut session, 1).await;
    assert_eq!(gate.check_calls(), MAX_PENDING_REQUESTS + 1);

    shutdown(session).await;
    let _ = fs::remove_dir_all(&ledger_dir);
    let _ = fs::remove_dir_all(&dir);
}

/// Gate wrapper for delayed-withdrawal regressions:
/// - The first `filler_count` checks pause before touching `inner` until
///   `filler_release` turns `true`, holding their adapter permits without
///   creating extra ledger rows.
/// - The next check (`filler_count + 1`) commits a real `SessionGate` approval
///   row, signals `delayed_inserted`, and pauses until `delayed_check_release`.
/// - `withdraw()` pauses *before* calling `inner.withdraw()` until
///   `withdraw_release` turns `true`, so tests can exceed `drain_withdrawals`'
///   deadline while the withdrawal is still unfinished.
struct DelayedControls {
    filler: watch::Sender<bool>,
    delayed_check: watch::Sender<bool>,
    withdraw: watch::Sender<bool>,
}

struct DelayedWithdrawGate {
    inner: Arc<SessionGate>,
    filler_count: usize,
    check_calls: AtomicUsize,
    filler_entered: Arc<Notify>,
    delayed_inserted: Arc<Notify>,
    withdraw_entered: Arc<Notify>,
    filler_release: watch::Receiver<bool>,
    delayed_check_release: watch::Receiver<bool>,
    withdraw_release: watch::Receiver<bool>,
    asked: Mutex<Vec<ApprovalTicket>>,
    withdraw_started: AtomicUsize,
    withdraw_finished: AtomicUsize,
}

impl DelayedWithdrawGate {
    fn new(inner: Arc<SessionGate>, filler_count: usize) -> (Arc<Self>, DelayedControls) {
        let (filler_tx, filler_rx) = watch::channel(false);
        let (delayed_tx, delayed_rx) = watch::channel(false);
        let (withdraw_tx, withdraw_rx) = watch::channel(false);
        let gate = Arc::new(Self {
            inner,
            filler_count,
            check_calls: AtomicUsize::new(0),
            filler_entered: Arc::new(Notify::new()),
            delayed_inserted: Arc::new(Notify::new()),
            withdraw_entered: Arc::new(Notify::new()),
            filler_release: filler_rx,
            delayed_check_release: delayed_rx,
            withdraw_release: withdraw_rx,
            asked: Mutex::default(),
            withdraw_started: AtomicUsize::new(0),
            withdraw_finished: AtomicUsize::new(0),
        });
        (
            gate,
            DelayedControls {
                filler: filler_tx,
                delayed_check: delayed_tx,
                withdraw: withdraw_tx,
            },
        )
    }

    fn check_calls(&self) -> usize {
        self.check_calls.load(Ordering::Acquire)
    }

    fn asked_len(&self) -> usize {
        self.asked.lock().unwrap().len()
    }

    fn withdraw_started(&self) -> usize {
        self.withdraw_started.load(Ordering::Acquire)
    }

    fn withdraw_finished(&self) -> usize {
        self.withdraw_finished.load(Ordering::Acquire)
    }
}

impl ToolGate for DelayedWithdrawGate {
    fn check<'a>(&'a self, check: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        Box::pin(async move {
            let seq = self.check_calls.fetch_add(1, Ordering::AcqRel) + 1;
            if seq <= self.filler_count {
                self.filler_entered.notify_waiters();
                let mut rx = self.filler_release.clone();
                while !*rx.borrow() {
                    if rx.changed().await.is_err() {
                        break;
                    }
                }
                return GateDecision::Deny {
                    reason: "test filler".into(),
                };
            }
            if seq == self.filler_count + 1 {
                let d = self.inner.check(check).await;
                if let GateDecision::Ask { ticket } = &d {
                    self.asked.lock().unwrap().push(ticket.clone());
                }
                self.delayed_inserted.notify_waiters();
                let mut rx = self.delayed_check_release.clone();
                while !*rx.borrow() {
                    if rx.changed().await.is_err() {
                        break;
                    }
                }
                return d;
            }
            self.inner.check(check).await
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
        self.inner.question_ticket(turn)
    }

    fn withdraw<'a>(&'a self, ticket: &'a ApprovalTicket) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.withdraw_started.fetch_add(1, Ordering::AcqRel);
            self.withdraw_entered.notify_waiters();
            let mut rx = self.withdraw_release.clone();
            while !*rx.borrow() {
                if rx.changed().await.is_err() {
                    break;
                }
            }
            self.inner.withdraw(ticket).await;
            self.withdraw_finished.fetch_add(1, Ordering::AcqRel);
        })
    }
}

/// M1-33 round 2, finding 1 (Claude): a withdrawal that outlives
/// `drain_withdrawals`' deadline must not be aborted; its permit stays
/// reserved until `SessionGate::withdraw` finishes, and once released the
/// orphan ledger row is cancelled and capacity recovers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_delayed_withdrawal_past_deadline_keeps_permit_and_cancels_orphan() {
    let dir = scratch("claude-delayed-withdraw");
    let script = dir.join("fake-claude-delayed.py");
    let probe_log = dir.join("probe.txt");
    let denied_log = dir.join("denied.txt");
    let filler_count: usize = MAX_PENDING_REQUESTS - 1;
    let batch_size: usize = 16;
    let excess_count: usize = 4;

    write_executable(
        &script,
        &format!(
            r#"#!/usr/bin/env python3
import json, os, sys, threading, time
if len(sys.argv) > 1 and sys.argv[1] == "--version":
    print("2.1.308 (Claude Code)")
    sys.exit(0)
sid = "fake-delayed"
filler_count = {filler_count}
batch_size = {batch_size}
excess = {excess}
trigger_dir = {trigger_dir:?}
probe_log = {probe_log:?}
denied_log = {denied_log:?}

def wait_for(path):
    deadline = time.monotonic() + 30.0
    while not os.path.exists(path) and time.monotonic() < deadline:
        time.sleep(0.005)

def emit_flow():
    print(json.dumps({{"type":"system","subtype":"init","session_id":sid,"model":"claude-fake","tools":["Read"],"cwd":"."}}), flush=True)
    sent = 0
    b = 0
    while sent < filler_count:
        n = min(batch_size, filler_count - sent)
        for i in range(sent, sent + n):
            print(json.dumps({{"type":"control_request","request_id":f"filler_{{i}}","request":{{"subtype":"can_use_tool","tool_name":"Read","tool_use_id":f"tu_f_{{i}}","input":{{"file_path":"x"}}}}}}), flush=True)
        wait_for(os.path.join(trigger_dir, f"cancel_{{b}}"))
        for i in range(sent, sent + n):
            print(json.dumps({{"type":"control_cancel_request","request_id":f"filler_{{i}}"}}), flush=True)
        sent += n
        b += 1
    print(json.dumps({{"type":"control_request","request_id":"delayed_0","request":{{"subtype":"can_use_tool","tool_name":"Read","tool_use_id":"tu_delayed","input":{{"file_path":"x"}}}}}}), flush=True)
    wait_for(os.path.join(trigger_dir, "cancel_delayed"))
    print(json.dumps({{"type":"control_cancel_request","request_id":"delayed_0"}}), flush=True)
    # Sent right after cancellation on the same pipe so observing its deny proves
    # `delayed_0` cancellation was processed before releasing `delayed_0`.
    print(json.dumps({{"type":"control_request","request_id":"probe_0","request":{{"subtype":"can_use_tool","tool_name":"Read","tool_use_id":"tu_probe","input":{{"file_path":"x"}}}}}}), flush=True)
    wait_for(os.path.join(trigger_dir, "send_excess"))
    for i in range(excess):
        print(json.dumps({{"type":"control_request","request_id":f"excess_{{i}}","request":{{"subtype":"can_use_tool","tool_name":"Read","tool_use_id":f"tu_e_{{i}}","input":{{"file_path":"x"}}}}}}), flush=True)
    wait_for(os.path.join(trigger_dir, "send_recovery"))
    print(json.dumps({{"type":"control_request","request_id":"recover_1","request":{{"subtype":"can_use_tool","tool_name":"Read","tool_use_id":"tu_rec","input":{{"file_path":"x"}}}}}}), flush=True)

for line in sys.stdin:
    v = json.loads(line)
    t = v.get("type")
    if t == "control_request" and v.get("request", {{}}).get("subtype") == "initialize":
        print(json.dumps({{"type":"control_response","response":{{"subtype":"success","request_id":v["request_id"],"response":{{"commands":[]}}}}}}), flush=True)
    elif t == "user":
        threading.Thread(target=emit_flow, daemon=True).start()
    elif t == "control_response":
        resp = v.get("response", {{}})
        rid = resp.get("request_id", "")
        beh = resp.get("response", {{}}).get("behavior")
        if rid == "probe_0" and beh == "deny":
            with open(probe_log, "a", encoding="utf-8") as f:
                f.write(rid + "\n")
        elif rid.startswith("excess_") and beh == "deny":
            with open(denied_log, "a", encoding="utf-8") as f:
                f.write(rid + "\n")
"#,
            filler_count = filler_count,
            batch_size = batch_size,
            excess = excess_count,
            trigger_dir = dir.display().to_string(),
            probe_log = probe_log.display().to_string(),
            denied_log = denied_log.display().to_string(),
        ),
    );

    let (ledger_dir, store, thread, inc, session_gate) =
        make_session_gate(Provider::Claude, &dir, Preset::Careful).await;
    let (gate, controls) = DelayedWithdrawGate::new(session_gate, filler_count);

    let wait_deadline = Duration::from_millis(150);
    // Keep `gate_deadline` at its default (20 s): it also bounds real SQLite
    // checks and tickets, so shrinking it causes spurious timeouts under load;
    // bound only `drain_withdrawals` via `with_withdraw_timeout`.
    let mut session = ClaudeAdapter::new(claude_opts(ClaudeOptions::default().gate_deadline))
        .with_withdraw_timeout(wait_deadline)
        .start(claude_cfg(&dir, script, gate.clone()), Transport::Local)
        .await
        .unwrap();

    session
        .commands
        .send(SessionCommand::SendUserMessage { text: "go".into() })
        .await
        .unwrap();

    // Fill `MAX_PENDING_REQUESTS - 1` inner gate permits with cancelled checks.
    let mut sent = 0;
    let mut b = 0;
    while sent < filler_count {
        let n = batch_size.min(filler_count - sent);
        let target = sent + n;
        let deadline = Instant::now() + Duration::from_secs(10);
        while gate.check_calls() < target && Instant::now() < deadline {
            let notified = gate.filler_entered.notified();
            let _ = tokio::time::timeout(Duration::from_millis(20), notified).await;
        }
        assert_eq!(gate.check_calls(), target, "filler batch {b} did not enter");
        fs::write(dir.join(format!("cancel_{b}")), "go").unwrap();
        sent = target;
        b += 1;
    }

    // Wait for the final check (`delayed_0`) to commit its real approval row.
    let insert_deadline = Instant::now() + Duration::from_secs(10);
    while gate.asked_len() < 1 && Instant::now() < insert_deadline {
        let notified = gate.delayed_inserted.notified();
        let _ = tokio::time::timeout(Duration::from_millis(20), notified).await;
    }
    assert_eq!(gate.asked_len(), 1);
    assert_eq!(gate.check_calls(), MAX_PENDING_REQUESTS);
    assert_eq!(store.requests().open_count(&thread, inc).await.unwrap(), 1);

    // Cancel `delayed_0` before it returns and wait until `probe_0` is denied
    // so the cancellation has definitely been processed by the actor.
    fs::write(dir.join("cancel_delayed"), "go").unwrap();
    let probe_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < probe_deadline {
        if fs::read_to_string(&probe_log).is_ok_and(|s| s.lines().any(|l| l == "probe_0")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        fs::read_to_string(&probe_log).is_ok_and(|s| s.lines().any(|l| l == "probe_0")),
        "probe_0 was not denied after cancelling delayed_0"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Let `delayed_0` return `Ask { ticket }` so `OrphanGuard` triggers
    // `drain_withdrawals`, where `withdraw()` pauses past `wait_deadline`.
    let release_at = Instant::now();
    let _ = controls.delayed_check.send(true);
    let withdraw_start_deadline = Instant::now() + Duration::from_secs(10);
    while gate.withdraw_started() < 1 && Instant::now() < withdraw_start_deadline {
        let notified = gate.withdraw_entered.notified();
        let _ = tokio::time::timeout(Duration::from_millis(20), notified).await;
    }
    assert_eq!(gate.withdraw_started(), 1);

    // Submit excess requests: the actor can only answer them *after*
    // `drain_withdrawals` hits its deadline, and all must be explicitly denied
    // because the unfinished withdrawal still holds its permit.
    fs::write(dir.join("send_excess"), "go").unwrap();
    let deny_deadline = Instant::now() + Duration::from_secs(10);
    let mut denied: Vec<String> = Vec::new();
    while Instant::now() < deny_deadline {
        if let Ok(text) = fs::read_to_string(&denied_log) {
            denied = text.lines().map(str::to_owned).collect();
            if denied.len() == excess_count {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        release_at.elapsed() >= Duration::from_millis(120),
        "expected drain_withdrawals to exceed its 150ms deadline, elapsed {:?}",
        release_at.elapsed()
    );
    assert_eq!(
        denied.len(),
        excess_count,
        "expected all {excess_count} excess Claude requests to be denied while delayed withdrawal holds permit, got {denied:?}"
    );
    assert_eq!(
        gate.check_calls(),
        MAX_PENDING_REQUESTS,
        "delayed withdrawal past deadline must retain its permit and block excess checks"
    );
    assert_eq!(gate.withdraw_finished(), 0);
    assert_eq!(store.requests().open_count(&thread, inc).await.unwrap(), 1);

    // Release the delayed withdrawal and assert it was never aborted: it
    // cancels the orphan ledger row and returns the permit.
    let _ = controls.withdraw.send(true);
    let settle_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < settle_deadline {
        let open = store.requests().open_count(&thread, inc).await.unwrap();
        if gate.withdraw_finished() == 1 && open == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        gate.withdraw_finished(),
        1,
        "delayed Claude withdrawal was aborted on drain_withdrawals timeout"
    );
    assert_eq!(
        store.requests().open_count(&thread, inc).await.unwrap(),
        0,
        "orphan Claude ledger row remained pending after delayed withdrawal released"
    );

    // With the 64th permit finally returned (63 fillers still paused), a new
    // request is admitted normally.
    fs::write(dir.join("send_recovery"), "go").unwrap();
    wait_for_approvals(&mut session, 1).await;
    assert_eq!(gate.check_calls(), MAX_PENDING_REQUESTS + 1);

    let _ = controls.filler.send(true);
    shutdown(session).await;
    assert_eq!(
        store.requests().open_count(&thread, inc).await.unwrap(),
        0,
        "open Claude ledger row remained pending after shutdown"
    );
    let _ = fs::remove_dir_all(&ledger_dir);
    let _ = fs::remove_dir_all(&dir);
}

/// M1-33 round 2, finding 1 (Codex): a withdrawal that outlives
/// `drain_withdrawals`' deadline (`REDEEM_TIMEOUT`, 10s) must not be aborted;
/// its permit stays reserved until `SessionGate::withdraw` finishes, and once
/// released the orphan ledger row is cancelled and capacity recovers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_delayed_withdrawal_past_deadline_keeps_permit_and_cancels_orphan() {
    let dir = scratch("codex-delayed-withdraw");
    let user_dir = dir.join("empty-codex-home");
    fs::create_dir_all(&user_dir).unwrap();
    let script = dir.join("fake-codex-delayed.py");
    let probe_log = dir.join("probe.txt");
    let declined_log = dir.join("declined.txt");
    let filler_count: usize = MAX_PENDING_REQUESTS - 1;
    let batch_size: usize = 16;
    let excess_count: usize = 4;

    write_executable(
        &script,
        &format!(
            r#"#!/usr/bin/env python3
import json, os, sys, threading, time
if len(sys.argv) > 1 and sys.argv[1] == "--version":
    print("codex-cli 0.163.0")
    sys.exit(0)
filler_count = {filler_count}
batch_size = {batch_size}
excess = {excess}
trigger_dir = {trigger_dir:?}
probe_log = {probe_log:?}
declined_log = {declined_log:?}

def wait_for(path):
    deadline = time.monotonic() + 30.0
    while not os.path.exists(path) and time.monotonic() < deadline:
        time.sleep(0.005)

def emit_flow():
    sent = 0
    b = 0
    while sent < filler_count:
        n = min(batch_size, filler_count - sent)
        for i in range(sent, sent + n):
            print(json.dumps({{"id": f"filler_{{i}}", "method": "item/commandExecution/requestApproval", "params": {{"threadId": "th_fake", "turnId": "turn_1", "itemId": f"item_f_{{i}}", "command": "ls", "cwd": "."}}}}), flush=True)
        wait_for(os.path.join(trigger_dir, f"cancel_{{b}}"))
        for i in range(sent, sent + n):
            print(json.dumps({{"method": "serverRequest/resolved", "params": {{"threadId": "th_fake", "requestId": f"filler_{{i}}"}}}}), flush=True)
        sent += n
        b += 1
    print(json.dumps({{"id": "delayed_0", "method": "item/commandExecution/requestApproval", "params": {{"threadId": "th_fake", "turnId": "turn_1", "itemId": "item_delayed", "command": "ls", "cwd": "."}}}}), flush=True)
    wait_for(os.path.join(trigger_dir, "cancel_delayed"))
    print(json.dumps({{"method": "serverRequest/resolved", "params": {{"threadId": "th_fake", "requestId": "delayed_0"}}}}), flush=True)
    print(json.dumps({{"id": "probe_0", "method": "item/commandExecution/requestApproval", "params": {{"threadId": "th_fake", "turnId": "turn_1", "itemId": "item_probe", "command": "ls", "cwd": "."}}}}), flush=True)
    wait_for(os.path.join(trigger_dir, "send_excess"))
    for i in range(excess):
        print(json.dumps({{"id": f"excess_{{i}}", "method": "item/commandExecution/requestApproval", "params": {{"threadId": "th_fake", "turnId": "turn_1", "itemId": f"item_e_{{i}}", "command": "ls", "cwd": "."}}}}), flush=True)
    wait_for(os.path.join(trigger_dir, "send_recovery"))
    print(json.dumps({{"id": "recover_1", "method": "item/commandExecution/requestApproval", "params": {{"threadId": "th_fake", "turnId": "turn_1", "itemId": "item_rec", "command": "ls", "cwd": "."}}}}), flush=True)

for line in sys.stdin:
    v = json.loads(line)
    m = v.get("method")
    rid = v.get("id")
    if m == "initialize":
        print(json.dumps({{"id": rid, "result": {{"userAgent": "codex-fake/0.163.0", "codexHome": "/scratch/.codex"}}}}), flush=True)
    elif m == "thread/start":
        print(json.dumps({{"id": rid, "result": {{"thread": {{"id": "th_fake", "cliVersion": "0.163.0"}}, "approvalPolicy": "untrusted", "sandbox": {{"type": "workspaceWrite", "networkAccess": False}}}}}}), flush=True)
    elif m == "turn/start":
        print(json.dumps({{"id": rid, "result": {{"turn": {{"id": "turn_1", "items": [], "status": "inProgress"}}}}}}), flush=True)
        print(json.dumps({{"method": "turn/started", "params": {{"threadId": "th_fake", "turn": {{"id": "turn_1", "status": "inProgress"}}}}}}), flush=True)
        threading.Thread(target=emit_flow, daemon=True).start()
    elif m is None and isinstance(rid, str):
        if rid == "probe_0" and v.get("result", {{}}).get("decision") == "decline":
            with open(probe_log, "a", encoding="utf-8") as f:
                f.write(rid + "\n")
        elif rid.startswith("excess_") and v.get("result", {{}}).get("decision") == "decline":
            with open(declined_log, "a", encoding="utf-8") as f:
                f.write(rid + "\n")
"#,
            filler_count = filler_count,
            batch_size = batch_size,
            excess = excess_count,
            trigger_dir = dir.display().to_string(),
            probe_log = probe_log.display().to_string(),
            declined_log = declined_log.display().to_string(),
        ),
    );

    let (ledger_dir, store, thread, inc, session_gate) =
        make_session_gate(Provider::Codex, &dir, Preset::Careful).await;
    let (gate, controls) = DelayedWithdrawGate::new(session_gate, filler_count);

    let cfg = SessionConfig {
        cwd: dir.clone(),
        model: Some("gpt-5".into()),
        effort: None,
        permissions: PermissionPolicy::Ask,
        resume: None,
        mcp_servers: vec![],
        gate: gate.clone(),
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

    let mut sent = 0;
    let mut b = 0;
    while sent < filler_count {
        let n = batch_size.min(filler_count - sent);
        let target = sent + n;
        let deadline = Instant::now() + Duration::from_secs(10);
        while gate.check_calls() < target && Instant::now() < deadline {
            let notified = gate.filler_entered.notified();
            let _ = tokio::time::timeout(Duration::from_millis(20), notified).await;
        }
        assert_eq!(gate.check_calls(), target, "filler batch {b} did not enter");
        fs::write(dir.join(format!("cancel_{b}")), "go").unwrap();
        sent = target;
        b += 1;
    }

    let insert_deadline = Instant::now() + Duration::from_secs(10);
    while gate.asked_len() < 1 && Instant::now() < insert_deadline {
        let notified = gate.delayed_inserted.notified();
        let _ = tokio::time::timeout(Duration::from_millis(20), notified).await;
    }
    assert_eq!(gate.asked_len(), 1);
    assert_eq!(gate.check_calls(), MAX_PENDING_REQUESTS);
    assert_eq!(store.requests().open_count(&thread, inc).await.unwrap(), 1);

    fs::write(dir.join("cancel_delayed"), "go").unwrap();
    let probe_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < probe_deadline {
        if fs::read_to_string(&probe_log).is_ok_and(|s| s.lines().any(|l| l == "probe_0")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        fs::read_to_string(&probe_log).is_ok_and(|s| s.lines().any(|l| l == "probe_0")),
        "probe_0 was not declined after cancelling delayed_0"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;

    let release_at = Instant::now();
    let _ = controls.delayed_check.send(true);
    let withdraw_start_deadline = Instant::now() + Duration::from_secs(10);
    while gate.withdraw_started() < 1 && Instant::now() < withdraw_start_deadline {
        let notified = gate.withdraw_entered.notified();
        let _ = tokio::time::timeout(Duration::from_millis(20), notified).await;
    }
    assert_eq!(gate.withdraw_started(), 1);

    // Submit excess requests: the driver can only answer them *after*
    // `drain_withdrawals` hits its 10s `REDEEM_TIMEOUT` deadline, and all must
    // be explicitly declined because the unfinished withdrawal still holds its
    // permit.
    fs::write(dir.join("send_excess"), "go").unwrap();
    let decline_deadline = Instant::now() + Duration::from_secs(20);
    let mut declined: Vec<String> = Vec::new();
    while Instant::now() < decline_deadline {
        if let Ok(text) = fs::read_to_string(&declined_log) {
            declined = text.lines().map(str::to_owned).collect();
            if declined.len() == excess_count {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        release_at.elapsed() >= Duration::from_secs(9),
        "expected drain_withdrawals to exceed its 10s REDEEM_TIMEOUT, elapsed {:?}",
        release_at.elapsed()
    );
    assert_eq!(
        declined.len(),
        excess_count,
        "expected all {excess_count} excess Codex requests to be declined while delayed withdrawal holds permit, got {declined:?}"
    );
    assert_eq!(
        gate.check_calls(),
        MAX_PENDING_REQUESTS,
        "delayed withdrawal past deadline must retain its permit and block excess checks"
    );
    assert_eq!(gate.withdraw_finished(), 0);
    assert_eq!(store.requests().open_count(&thread, inc).await.unwrap(), 1);

    // Release the delayed withdrawal and assert it was never aborted: it
    // cancels the orphan ledger row and returns the permit.
    let _ = controls.withdraw.send(true);
    let settle_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < settle_deadline {
        let open = store.requests().open_count(&thread, inc).await.unwrap();
        if gate.withdraw_finished() == 1 && open == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        gate.withdraw_finished(),
        1,
        "delayed Codex withdrawal was aborted on drain_withdrawals timeout"
    );
    assert_eq!(
        store.requests().open_count(&thread, inc).await.unwrap(),
        0,
        "orphan Codex ledger row remained pending after delayed withdrawal released"
    );

    fs::write(dir.join("send_recovery"), "go").unwrap();
    wait_for_approvals(&mut session, 1).await;
    assert_eq!(gate.check_calls(), MAX_PENDING_REQUESTS + 1);

    let _ = controls.filler.send(true);
    shutdown(session).await;
    assert_eq!(
        store.requests().open_count(&thread, inc).await.unwrap(),
        0,
        "open Codex ledger row remained pending after shutdown"
    );
    let _ = fs::remove_dir_all(&ledger_dir);
    let _ = fs::remove_dir_all(&dir);
}

/// Gate wrapper that allows normal ticket delivery through a real `SessionGate`
/// while blocking `withdraw()` until `withdraw_release` turns `true`.
struct BlockedDeliveredWithdrawGate {
    inner: Arc<SessionGate>,
    question_calls: AtomicUsize,
    withdraw_started: AtomicUsize,
    withdraw_finished: AtomicUsize,
    withdraw_entered: Arc<Notify>,
    withdraw_release: watch::Receiver<bool>,
}

impl BlockedDeliveredWithdrawGate {
    fn new(inner: Arc<SessionGate>) -> (Arc<Self>, watch::Sender<bool>) {
        let (tx, rx) = watch::channel(false);
        let gate = Arc::new(Self {
            inner,
            question_calls: AtomicUsize::new(0),
            withdraw_started: AtomicUsize::new(0),
            withdraw_finished: AtomicUsize::new(0),
            withdraw_entered: Arc::new(Notify::new()),
            withdraw_release: rx,
        });
        (gate, tx)
    }

    fn question_calls(&self) -> usize {
        self.question_calls.load(Ordering::Acquire)
    }

    fn withdraw_started(&self) -> usize {
        self.withdraw_started.load(Ordering::Acquire)
    }

    fn withdraw_finished(&self) -> usize {
        self.withdraw_finished.load(Ordering::Acquire)
    }
}

impl ToolGate for BlockedDeliveredWithdrawGate {
    fn check<'a>(&'a self, check: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        self.inner.check(check)
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
            self.question_calls.fetch_add(1, Ordering::AcqRel);
            self.inner.question_ticket(turn).await
        })
    }

    fn withdraw<'a>(&'a self, ticket: &'a ApprovalTicket) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.withdraw_started.fetch_add(1, Ordering::AcqRel);
            self.withdraw_entered.notify_waiters();
            let mut rx = self.withdraw_release.clone();
            while !*rx.borrow() {
                if rx.changed().await.is_err() {
                    break;
                }
            }
            self.inner.withdraw(ticket).await;
            self.withdraw_finished.fetch_add(1, Ordering::AcqRel);
        })
    }
}

async fn make_session_gate_with_ledger(
    provider: Provider,
    home: &Path,
    preset: Preset,
) -> (
    PathBuf,
    Arc<SqliteStore>,
    Arc<Ledger>,
    ThreadId,
    u32,
    Arc<SessionGate>,
) {
    let dir = scratch("ledger-delivered-withdraw");
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
            title: "m1-33-delivered-withdraw".into(),
        })
        .await
        .unwrap();
    let inc = store.begin_incarnation(&s.thread_id).await.unwrap();
    store.signal(&s.thread_id, Signal::Starting).await.unwrap();
    let canon_home = fs::canonicalize(home).unwrap();
    let gate = Arc::new(SessionGate::new(
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
        ledger.clone(),
        Arc::new(StoreRules(store.clone())),
    ));
    (dir, store, ledger, s.thread_id, inc, gate)
}

async fn wait_for_question(session: &mut Session) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("timed out waiting for QuestionAsked");
        match tokio::time::timeout(remaining, session.events.recv()).await {
            Ok(Some(env)) => {
                if let AgentEvent::QuestionAsked { request_id, .. } = env.event {
                    return request_id;
                }
            }
            Ok(None) => panic!("session ended while waiting for QuestionAsked"),
            Err(_) => panic!("timed out waiting for QuestionAsked"),
        }
    }
}

/// M1-33 round 3, finding 2 (Claude): after normal question delivery, freeing
/// each ledger slot independently (as the daemon does before `DeclineQuestion`)
/// while adapter `gate.withdraw()` is blocked must keep detached withdrawals
/// bounded by `MAX_PENDING_REQUESTS`, explicitly decline excess questions, and
/// complete all blocked withdrawals without aborting them once unblocked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_delivered_question_blocked_withdrawals_stay_bounded_and_complete() {
    let dir = scratch("claude-delivered-withdraw-bound");
    let script = dir.join("fake-claude-delivered-withdraw.py");
    let recovery_trigger = dir.join("send_recovery");
    let denied_log = dir.join("denied.txt");
    let total: usize = MAX_PENDING_REQUESTS;
    let excess_count: usize = 6;

    write_executable(
        &script,
        &format!(
            r#"#!/usr/bin/env python3
import json, os, sys, threading, time
if len(sys.argv) > 1 and sys.argv[1] == "--version":
    print("2.1.308 (Claude Code)")
    sys.exit(0)
sid = "fake-delivered-withdraw"
total = {total}
excess = {excess}
recovery_trigger = {recovery_trigger:?}
denied_log = {denied_log:?}
out_lock = threading.Lock()

def emit(obj):
    with out_lock:
        print(json.dumps(obj), flush=True)

def emit_question(rid):
    emit({{
        "type": "control_request",
        "request_id": rid,
        "request": {{
            "subtype": "can_use_tool",
            "tool_name": "AskUserQuestion",
            "tool_use_id": f"tu_{{rid}}",
            "input": {{"questions": [{{"question": "Proceed?", "options": [{{"label": "Yes"}}, {{"label": "No"}}]}}]}}
        }}
    }})

def wait_recovery():
    deadline = time.monotonic() + 60.0
    while not os.path.exists(recovery_trigger) and time.monotonic() < deadline:
        time.sleep(0.005)
    if os.path.exists(recovery_trigger):
        emit_question("recover_1")

for line in sys.stdin:
    v = json.loads(line)
    t = v.get("type")
    if t == "control_request" and v.get("request", {{}}).get("subtype") == "initialize":
        emit({{"type": "control_response", "response": {{"subtype": "success", "request_id": v["request_id"], "response": {{"commands": []}}}}}})
    elif t == "user":
        emit({{"type": "system", "subtype": "init", "session_id": sid, "model": "claude-fake", "tools": ["AskUserQuestion"], "cwd": "."}})
        threading.Thread(target=wait_recovery, daemon=True).start()
        emit_question("q_0")
    elif t == "control_response":
        resp = v.get("response", {{}})
        rid = resp.get("request_id", "")
        beh = resp.get("response", {{}}).get("behavior")
        if rid.startswith("q_") and beh == "deny":
            idx = int(rid[2:])
            if idx + 1 < total:
                emit_question(f"q_{{idx + 1}}")
            else:
                for i in range(excess):
                    emit_question(f"excess_{{i}}")
        elif rid.startswith("excess_") and beh == "deny":
            with open(denied_log, "a", encoding="utf-8") as f:
                f.write(rid + "\n")
"#,
            total = total,
            excess = excess_count,
            recovery_trigger = recovery_trigger.display().to_string(),
            denied_log = denied_log.display().to_string(),
        ),
    );

    let (ledger_dir, store, ledger, thread, inc, session_gate) =
        make_session_gate_with_ledger(Provider::Claude, &dir, Preset::Careful).await;
    let (gate, release_withdraw) = BlockedDeliveredWithdrawGate::new(session_gate);

    // Keep `gate_deadline` at its default (20 s): it also bounds real SQLite
    // checks and tickets, so shrinking it causes spurious timeouts under load;
    // bound only `drain_withdrawals` via `with_withdraw_timeout`.
    let mut session = ClaudeAdapter::new(claude_opts(ClaudeOptions::default().gate_deadline))
        .with_withdraw_timeout(Duration::from_millis(20))
        .start(claude_cfg(&dir, script, gate.clone()), Transport::Local)
        .await
        .unwrap();

    session
        .commands
        .send(SessionCommand::SendUserMessage { text: "go".into() })
        .await
        .unwrap();

    // Deliver `MAX_PENDING_REQUESTS` questions normally, free each ledger slot
    // independently (modeling daemon `DeclineQuestion`), and decline each one
    // while adapter `gate.withdraw()` remains blocked.
    for _ in 0..total {
        let request_id = wait_for_question(&mut session).await;
        ledger
            .withdraw(&RequestKey {
                thread: thread.clone(),
                incarnation: inc,
                request_id: request_id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(store.requests().open_count(&thread, inc).await.unwrap(), 0);
        session
            .commands
            .send(SessionCommand::DeclineQuestion { request_id })
            .await
            .unwrap();
    }

    // All excess questions sent once all `MAX_PENDING_REQUESTS` permits are held
    // by blocked withdrawals must be declined explicitly without opening new tickets.
    let deny_deadline = Instant::now() + Duration::from_secs(15);
    let mut denied: Vec<String> = Vec::new();
    while Instant::now() < deny_deadline {
        if let Ok(text) = fs::read_to_string(&denied_log) {
            denied = text.lines().map(str::to_owned).collect();
            if denied.len() == excess_count && gate.withdraw_started() == total {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        denied.len(),
        excess_count,
        "expected all {excess_count} excess Claude questions to be explicitly declined while delivered-ticket withdrawals are blocked, got {denied:?}"
    );
    assert_eq!(
        gate.question_calls(),
        total,
        "excess Claude questions must not enter question_ticket while blocked delivered-ticket withdrawals hold all permits"
    );
    assert_eq!(gate.withdraw_started(), total);
    assert_eq!(gate.withdraw_finished(), 0);
    assert_eq!(store.requests().open_count(&thread, inc).await.unwrap(), 0);

    // Unblock withdrawals: every single blocked withdrawal must complete (none aborted).
    let _ = release_withdraw.send(true);
    let settle_deadline = Instant::now() + Duration::from_secs(15);
    while gate.withdraw_finished() < total && Instant::now() < settle_deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        gate.withdraw_finished(),
        total,
        "blocked Claude delivered-ticket withdrawals must all eventually complete"
    );

    // Capacity recovers: a subsequent question is admitted and delivered normally.
    fs::write(&recovery_trigger, "go").unwrap();
    let _recovered_id = wait_for_question(&mut session).await;
    assert_eq!(gate.question_calls(), total + 1);

    shutdown(session).await;
    assert_eq!(
        store.requests().open_count(&thread, inc).await.unwrap(),
        0,
        "open Claude ledger row remained pending after shutdown"
    );
    let _ = fs::remove_dir_all(&ledger_dir);
    let _ = fs::remove_dir_all(&dir);
}

/// M1-33 round 3, finding 2 (Codex): after normal question delivery, freeing
/// each ledger slot independently (as the daemon does before `DeclineQuestion`)
/// while adapter `gate.withdraw()` is blocked must keep detached withdrawals
/// bounded by `MAX_PENDING_REQUESTS`, explicitly decline excess questions, and
/// complete all blocked withdrawals without aborting them once unblocked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_delivered_question_blocked_withdrawals_stay_bounded_and_complete() {
    let dir = scratch("codex-delivered-withdraw-bound");
    let user_dir = dir.join("empty-codex-home");
    fs::create_dir_all(&user_dir).unwrap();
    let script = dir.join("fake-codex-delivered-withdraw.py");
    let recovery_trigger = dir.join("send_recovery");
    let declined_log = dir.join("declined.txt");
    let total: usize = MAX_PENDING_REQUESTS;
    let excess_count: usize = 6;

    write_executable(
        &script,
        &format!(
            r#"#!/usr/bin/env python3
import json, os, sys, threading, time
if len(sys.argv) > 1 and sys.argv[1] == "--version":
    print("codex-cli 0.163.0")
    sys.exit(0)
total = {total}
excess = {excess}
recovery_trigger = {recovery_trigger:?}
declined_log = {declined_log:?}
out_lock = threading.Lock()

def emit(obj):
    with out_lock:
        print(json.dumps(obj), flush=True)

def emit_question(rid):
    emit({{
        "id": rid,
        "method": "item/tool/requestUserInput",
        "params": {{
            "threadId": "th_fake",
            "turnId": "turn_1",
            "itemId": f"item_{{rid}}",
            "questions": [{{"id": "q1", "header": "Confirm", "question": "Proceed?", "options": [{{"label": "Yes", "description": "Continue"}}]}}]
        }}
    }})

def wait_recovery():
    deadline = time.monotonic() + 30.0
    while not os.path.exists(recovery_trigger) and time.monotonic() < deadline:
        time.sleep(0.005)
    if os.path.exists(recovery_trigger):
        emit_question("recover_1")

for line in sys.stdin:
    v = json.loads(line)
    m = v.get("method")
    rid = v.get("id")
    if m == "initialize":
        emit({{"id": rid, "result": {{"userAgent": "codex-fake/0.163.0", "codexHome": "/scratch/.codex"}}}})
    elif m == "thread/start":
        emit({{"id": rid, "result": {{"thread": {{"id": "th_fake", "cliVersion": "0.163.0"}}, "approvalPolicy": "untrusted", "sandbox": {{"type": "workspaceWrite", "networkAccess": False}}}}}})
    elif m == "turn/start":
        emit({{"id": rid, "result": {{"turn": {{"id": "turn_1", "items": [], "status": "inProgress"}}}}}})
        emit({{"method": "turn/started", "params": {{"threadId": "th_fake", "turn": {{"id": "turn_1", "status": "inProgress"}}}}}})
        threading.Thread(target=wait_recovery, daemon=True).start()
        emit_question("q_0")
    elif m is None and isinstance(rid, str):
        if rid.startswith("q_") and v.get("result") == {{"answers": {{}}}}:
            idx = int(rid[2:])
            if idx + 1 < total:
                emit_question(f"q_{{idx + 1}}")
            else:
                for i in range(excess):
                    emit_question(f"excess_{{i}}")
        elif rid.startswith("excess_") and v.get("result") == {{"answers": {{}}}}:
            with open(declined_log, "a", encoding="utf-8") as f:
                f.write(rid + "\n")
"#,
            total = total,
            excess = excess_count,
            recovery_trigger = recovery_trigger.display().to_string(),
            declined_log = declined_log.display().to_string(),
        ),
    );

    let (ledger_dir, store, ledger, thread, inc, session_gate) =
        make_session_gate_with_ledger(Provider::Codex, &dir, Preset::Careful).await;
    let (gate, release_withdraw) = BlockedDeliveredWithdrawGate::new(session_gate);

    let cfg = SessionConfig {
        cwd: dir.clone(),
        model: Some("gpt-5".into()),
        effort: None,
        permissions: PermissionPolicy::Ask,
        resume: None,
        mcp_servers: vec![],
        gate: gate.clone(),
        env: vec![("CODEX_HOME".into(), user_dir.display().to_string())],
        program: Some(script),
        on_spawn: None,
    };
    let opts = CodexOptions {
        user_dir: Some(user_dir),
        withdraw_timeout: Some(Duration::from_millis(20)),
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

    for _ in 0..total {
        let request_id = wait_for_question(&mut session).await;
        ledger
            .withdraw(&RequestKey {
                thread: thread.clone(),
                incarnation: inc,
                request_id: request_id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(store.requests().open_count(&thread, inc).await.unwrap(), 0);
        session
            .commands
            .send(SessionCommand::DeclineQuestion { request_id })
            .await
            .unwrap();
    }

    let decline_deadline = Instant::now() + Duration::from_secs(15);
    let mut declined: Vec<String> = Vec::new();
    while Instant::now() < decline_deadline {
        if let Ok(text) = fs::read_to_string(&declined_log) {
            declined = text.lines().map(str::to_owned).collect();
            if declined.len() == excess_count && gate.withdraw_started() == total {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        declined.len(),
        excess_count,
        "expected all {excess_count} excess Codex questions to be explicitly declined while delivered-ticket withdrawals are blocked, got {declined:?}"
    );
    assert_eq!(
        gate.question_calls(),
        total,
        "excess Codex questions must not enter question_ticket while blocked delivered-ticket withdrawals hold all permits"
    );
    assert_eq!(gate.withdraw_started(), total);
    assert_eq!(gate.withdraw_finished(), 0);
    assert_eq!(store.requests().open_count(&thread, inc).await.unwrap(), 0);

    let _ = release_withdraw.send(true);
    let settle_deadline = Instant::now() + Duration::from_secs(15);
    while gate.withdraw_finished() < total && Instant::now() < settle_deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        gate.withdraw_finished(),
        total,
        "blocked Codex delivered-ticket withdrawals must all eventually complete"
    );

    fs::write(&recovery_trigger, "go").unwrap();
    let _recovered_id = wait_for_question(&mut session).await;
    assert_eq!(gate.question_calls(), total + 1);

    shutdown(session).await;
    assert_eq!(
        store.requests().open_count(&thread, inc).await.unwrap(),
        0,
        "open Codex ledger row remained pending after shutdown"
    );
    let _ = fs::remove_dir_all(&ledger_dir);
    let _ = fs::remove_dir_all(&dir);
}
