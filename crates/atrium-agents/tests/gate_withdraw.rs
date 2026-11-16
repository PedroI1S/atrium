//! CR-18 / P2: both Claude and Codex adapters call `ToolGate::withdraw` when a
//! request is withdrawn (provider cancellation, turn end, interrupt/shutdown,
//! saturation decline, or an `Ask` never followed by `can_use_tool`).
//!
//! Runs against `examples/claude-fake.rs` and `examples/codex-fake.rs` (never a
//! real provider) backed by a real `SessionGate` + `SqliteStore`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use atrium_agents::claude::{ClaudeAdapter, ClaudeOptions};
use atrium_agents::codex::{CodexAdapter, CodexOptions};
use atrium_agents::{
    AgentAdapter, ApprovalTicket, BoxFuture, GateDecision, PermissionPolicy, Session,
    SessionConfig, ToolCheck, ToolGate, Transport,
};
use atrium_core::policy::{DefaultPolicy, Ledger, SessionGate, SessionGateConfig, StoreRules};
use atrium_core::store::{SqliteStore, StoreOptions};
use atrium_core::{NewThread, Signal, Store};
use atrium_protocol::agent::{AgentEvent, Provider, SessionCommand};
use atrium_protocol::policy::Preset;
use atrium_protocol::thread::{Location, ThreadId};

fn fake_bin(example: &str) -> PathBuf {
    static CLAUDE_BUILT: OnceLock<PathBuf> = OnceLock::new();
    static CODEX_BUILT: OnceLock<PathBuf> = OnceLock::new();
    let cell = match example {
        "claude-fake" => &CLAUDE_BUILT,
        "codex-fake" => &CODEX_BUILT,
        other => panic!("unexpected fake example {other}"),
    };
    cell.get_or_init(|| {
        let exe = std::env::current_exe().expect("current_exe");
        let dir = exe.parent().expect("deps").parent().expect("profile");
        let path = dir.join("examples").join(example);
        if !path.exists() {
            let status = std::process::Command::new(env!("CARGO"))
                .args([
                    "build",
                    "-p",
                    "atrium-agents",
                    "--example",
                    example,
                    "--offline",
                ])
                .status()
                .expect("build fake example");
            assert!(status.success(), "{example} build failed");
        }
        assert!(path.exists(), "missing {}", path.display());
        path
    })
    .clone()
}

fn scratch(name: &str) -> PathBuf {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "atrium-gate-withdraw-{}-{}-{n}",
        std::process::id(),
        name
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

struct SpyGate {
    inner: Arc<SessionGate>,
    asked: Mutex<Vec<ApprovalTicket>>,
    withdrawn: Mutex<Vec<ApprovalTicket>>,
    hold: Option<
        Arc<(
            std::sync::atomic::AtomicBool,
            tokio::sync::Notify,
            AtomicUsize,
        )>,
    >,
}

impl SpyGate {
    fn new(inner: Arc<SessionGate>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            asked: Mutex::default(),
            withdrawn: Mutex::default(),
            hold: None,
        })
    }

    fn holding(
        inner: Arc<SessionGate>,
    ) -> (
        Arc<Self>,
        Arc<(
            std::sync::atomic::AtomicBool,
            tokio::sync::Notify,
            AtomicUsize,
        )>,
    ) {
        let hold = Arc::new((
            std::sync::atomic::AtomicBool::new(false),
            tokio::sync::Notify::new(),
            AtomicUsize::new(0),
        ));
        let gate = Arc::new(Self {
            inner,
            asked: Mutex::default(),
            withdrawn: Mutex::default(),
            hold: Some(hold.clone()),
        });
        (gate, hold)
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
            if let Some(h) = &self.hold {
                h.2.fetch_add(1, Ordering::AcqRel);
                while !h.0.load(Ordering::Acquire) {
                    h.1.notified().await;
                }
            }
            let d = self.inner.check(check).await;
            if let GateDecision::Ask { ticket } = &d {
                self.asked.lock().unwrap().push(ticket.clone());
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
            if let Some(h) = &self.hold {
                h.2.fetch_add(1, Ordering::AcqRel);
                while !h.0.load(Ordering::Acquire) {
                    h.1.notified().await;
                }
            }
            let t = self.inner.question_ticket(turn).await;
            if let Some(ticket) = &t {
                self.asked.lock().unwrap().push(ticket.clone());
            }
            t
        })
    }

    fn withdraw<'a>(&'a self, ticket: &'a ApprovalTicket) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.withdrawn.lock().unwrap().push(ticket.clone());
            self.inner.withdraw(ticket).await;
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
    async fn new(provider: Provider, home: &Path, preset: Preset) -> Self {
        Self::build(provider, home, preset, false).await.0
    }

    async fn holding(
        provider: Provider,
        home: &Path,
        preset: Preset,
    ) -> (
        Self,
        Arc<(
            std::sync::atomic::AtomicBool,
            tokio::sync::Notify,
            AtomicUsize,
        )>,
    ) {
        let (me, hold) = Self::build(provider, home, preset, true).await;
        (me, hold.unwrap())
    }

    async fn build(
        provider: Provider,
        home: &Path,
        preset: Preset,
        with_hold: bool,
    ) -> (
        Self,
        Option<
            Arc<(
                std::sync::atomic::AtomicBool,
                tokio::sync::Notify,
                AtomicUsize,
            )>,
        >,
    ) {
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
                title: "adapter-withdraw".into(),
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
                home: canon_home.clone(),
                atrium_data_paths: vec![canon_home.join(".local/share/atrium")],
                atrium_protected_paths: vec![canon_home.join(".config/atrium")],
                preset,
                provider,
            },
            Arc::new(DefaultPolicy::new()),
            ledger,
            Arc::new(StoreRules(store.clone())),
        ));
        let (gate, hold) = if with_hold {
            let (g, h) = SpyGate::holding(inner);
            (g, Some(h))
        } else {
            (SpyGate::new(inner), None)
        };
        (
            Self {
                dir,
                store,
                thread: s.thread_id,
                inc,
                gate,
            },
            hold,
        )
    }

    async fn open_count(&self) -> u64 {
        self.store
            .requests()
            .open_count(&self.thread, self.inc)
            .await
            .unwrap()
    }
}

fn say(text: &str) -> SessionCommand {
    SessionCommand::SendUserMessage { text: text.into() }
}

async fn next_matching(
    session: &mut Session,
    secs: u64,
    pred: impl Fn(&AgentEvent) -> bool,
) -> Vec<AgentEvent> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    let mut out = Vec::new();
    loop {
        let ev = tokio::time::timeout_at(deadline, session.events.recv())
            .await
            .expect("timed out waiting for event")
            .expect("event stream ended early");
        let matched = pred(&ev.event);
        out.push(ev.event);
        if matched {
            return out;
        }
    }
}

async fn shutdown(mut session: Session) {
    let _ = session.commands.send(SessionCommand::Shutdown).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(2), session.events.recv()).await {
            Ok(Some(env)) if matches!(env.event, AgentEvent::SessionEnded { .. }) => return,
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => return,
        }
    }
}

fn claude_config(dir: &Path, mode: &str, gate: Arc<dyn ToolGate>) -> SessionConfig {
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
            (
                "CLAUDE_CONFIG_DIR".into(),
                dir.join("user").display().to_string(),
            ),
        ],
        program: Some(fake_bin("claude-fake")),
        on_spawn: None,
    }
}

fn claude_opts() -> ClaudeOptions {
    ClaudeOptions {
        managed_settings: Some(vec![]),
        ..ClaudeOptions::default()
    }
}

fn codex_config(dir: &Path, mode: &str, gate: Arc<dyn ToolGate>) -> SessionConfig {
    let user_dir = dir.join("empty-codex-home");
    fs::create_dir_all(&user_dir).unwrap();
    SessionConfig {
        cwd: dir.to_path_buf(),
        model: Some("gpt-5".into()),
        effort: None,
        permissions: PermissionPolicy::Ask,
        resume: None,
        mcp_servers: vec![],
        gate,
        env: vec![
            ("FAKE_MODE".into(), mode.into()),
            ("FAKE_DIR".into(), dir.display().to_string()),
            ("CODEX_HOME".into(), user_dir.display().to_string()),
        ],
        program: Some(fake_bin("codex-fake")),
        on_spawn: None,
    }
}

fn codex_opts(dir: &Path) -> CodexOptions {
    CodexOptions {
        user_dir: Some(dir.join("empty-codex-home")),
        ..CodexOptions::default()
    }
}

#[tokio::test]
async fn claude_adapter_withdraws_cancelled_and_turn_ended_requests() {
    // 1. Provider cancels an open `can_use_tool` request (`control_cancel_request`).
    let dir = scratch("claude-cancel");
    let ledger = RealLedger::new(Provider::Claude, &dir, Preset::Careful).await;
    let mut session = ClaudeAdapter::new(claude_opts())
        .start(
            claude_config(&dir, "canuse-cancel", ledger.gate.clone()),
            Transport::Local,
        )
        .await
        .unwrap();
    session.commands.send(say("go")).await.unwrap();
    next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::RequestResolved { .. })
    })
    .await;
    let asked = ledger.gate.asked();
    let withdrawn = ledger.gate.withdrawn();
    assert_eq!(asked.len(), 1);
    assert_eq!(withdrawn, asked);
    assert_eq!(ledger.open_count().await, 0);
    shutdown(session).await;
    let _ = fs::remove_dir_all(&dir);

    // 2. Turn ends (`result`) while a `can_use_tool` Ask is still pending.
    let dir = scratch("claude-result");
    let ledger = RealLedger::new(Provider::Claude, &dir, Preset::Careful).await;
    let mut session = ClaudeAdapter::new(claude_opts())
        .start(
            claude_config(&dir, "canuse-result", ledger.gate.clone()),
            Transport::Local,
        )
        .await
        .unwrap();
    session.commands.send(say("go")).await.unwrap();
    next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::TurnCompleted { .. })
    })
    .await;
    let asked = ledger.gate.asked();
    let withdrawn = ledger.gate.withdrawn();
    assert_eq!(asked.len(), 1);
    assert_eq!(withdrawn, asked);
    assert_eq!(ledger.open_count().await, 0);
    shutdown(session).await;
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn claude_adapter_withdraws_unmatched_hook_ask_when_input_changes() {
    let dir = scratch("claude-hook-changed");
    let ledger = RealLedger::new(Provider::Claude, &dir, Preset::Home).await;
    let mut session = ClaudeAdapter::new(claude_opts())
        .start(
            claude_config(&dir, "hook-ask-changed", ledger.gate.clone()),
            Transport::Local,
        )
        .await
        .unwrap();
    session.commands.send(say("go")).await.unwrap();
    // Wait until the second Ask (for the changed `can_use_tool`) is emitted:
    // the unmatched hook Ask ticket must already have been withdrawn.
    next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::ApprovalRequested { .. })
    })
    .await;
    let asked = ledger.gate.asked();
    assert_eq!(asked.len(), 2, "expected hook Ask + can_use_tool Ask");
    let withdrawn = ledger.gate.withdrawn();
    assert_eq!(
        withdrawn,
        vec![asked[0].clone()],
        "unmatched hook Ask must be withdrawn immediately"
    );
    assert_eq!(ledger.open_count().await, 1);

    // Interrupt/shutdown withdraws the remaining open approval too.
    shutdown(session).await;
    assert_eq!(ledger.open_count().await, 0);
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn codex_adapter_withdraws_interrupted_approval_and_question_and_saturation() {
    // 1. Interrupted turn with an open approval (`Bash` command `touch x.txt`).
    let dir = scratch("codex-interrupt-approval");
    let ledger = RealLedger::new(Provider::Codex, &dir, Preset::Home).await;
    let mut cfg = codex_config(&dir, "gate", ledger.gate.clone());
    cfg.env.push(("FAKE_CMD".into(), "touch x.txt".into()));
    let mut session = CodexAdapter::with_options(codex_opts(&dir))
        .start(cfg, Transport::Local)
        .await
        .unwrap();
    session.commands.send(say("go")).await.unwrap();
    next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::ApprovalRequested { .. })
    })
    .await;
    assert_eq!(ledger.open_count().await, 1);
    session
        .commands
        .send(SessionCommand::Interrupt)
        .await
        .unwrap();
    next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::TurnCompleted { .. })
    })
    .await;
    let asked = ledger.gate.asked();
    let withdrawn = ledger.gate.withdrawn();
    assert_eq!(asked.len(), 1);
    assert_eq!(withdrawn, asked);
    assert_eq!(ledger.open_count().await, 0);
    shutdown(session).await;
    let _ = fs::remove_dir_all(&dir);

    // 2. Interrupted turn with an open question.
    let dir = scratch("codex-interrupt-question");
    let ledger = RealLedger::new(Provider::Codex, &dir, Preset::Home).await;
    let mut session = CodexAdapter::with_options(codex_opts(&dir))
        .start(
            codex_config(&dir, "question", ledger.gate.clone()),
            Transport::Local,
        )
        .await
        .unwrap();
    session.commands.send(say("ask")).await.unwrap();
    next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::QuestionAsked { .. })
    })
    .await;
    assert_eq!(ledger.open_count().await, 1);
    session
        .commands
        .send(SessionCommand::Interrupt)
        .await
        .unwrap();
    next_matching(&mut session, 10, |e| {
        matches!(e, AgentEvent::TurnCompleted { .. })
    })
    .await;
    let asked = ledger.gate.asked();
    let withdrawn = ledger.gate.withdrawn();
    assert_eq!(asked.len(), 1);
    assert_eq!(withdrawn, asked);
    assert_eq!(ledger.open_count().await, 0);
    shutdown(session).await;
    let _ = fs::remove_dir_all(&dir);

    // 3. Saturation (`flood-then-ask`) opens tickets then declines them on
    //    saturation: must withdraw so no ledger slot leaks.
    let dir = scratch("codex-saturation");
    let (ledger, hold) = RealLedger::holding(Provider::Codex, &dir, Preset::Careful).await;
    let mut cfg = codex_config(&dir, "flood-then-ask", ledger.gate.clone());
    cfg.env.push(("FAKE_FLOOD_COUNT".into(), "5500".into()));
    let stdout_count = Arc::new(AtomicUsize::new(0));
    let sc = stdout_count.clone();
    let mut options = codex_opts(&dir);
    options.tap = Some(Arc::new(move |input: &atrium_agents::AdapterInput| {
        if matches!(input, atrium_agents::AdapterInput::Stdout { .. }) {
            sc.fetch_add(1, Ordering::AcqRel);
        }
    }));
    let session = CodexAdapter::with_options(options)
        .start(cfg, Transport::Local)
        .await
        .unwrap();
    session.commands.send(say("flood")).await.unwrap();
    let wait_end = Instant::now() + Duration::from_secs(5);
    while (session.events.len() < session.events.max_capacity()
        || hold.2.load(Ordering::Acquire) < 2
        || stdout_count.load(Ordering::Acquire) < 4096 + session.events.max_capacity() + 5)
        && Instant::now() < wait_end
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(session.events.len(), session.events.max_capacity());
    assert_eq!(hold.2.load(Ordering::Acquire), 2);
    hold.0.store(true, Ordering::Release);
    hold.1.notify_waiters();

    // Wait until both tickets were opened and withdrawn without draining `events`.
    let wait_withdraw = Instant::now() + Duration::from_secs(5);
    while (ledger.gate.asked().len() < 2 || ledger.gate.withdrawn().len() < 2)
        && Instant::now() < wait_withdraw
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let asked = ledger.gate.asked();
    let withdrawn = ledger.gate.withdrawn();
    assert_eq!(asked.len(), 2);
    for id in &asked {
        assert!(
            withdrawn.contains(id),
            "opened ticket {id} must be withdrawn on saturation: {withdrawn:?}"
        );
    }
    assert_eq!(ledger.open_count().await, 0);
    shutdown(session).await;
    let _ = fs::remove_dir_all(&dir);
}
