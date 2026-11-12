//! `ClaudeAdapter`: runs Claude Code headless and routes every tool call
//! through the [`ToolGate`].
//!
//! # How the gate sees every tool call
//!
//! The session starts `claude -p --input-format stream-json --output-format
//! stream-json --permission-prompt-tool stdio` and sends an `initialize`
//! control request that registers a `PreToolUse` **hook callback**. Claude
//! Code then sends a `hook_callback` control request before every tool call
//! (built-ins, MCP tools, subagent tools), and we answer it from the gate:
//!
//! - `Allow` → `permissionDecision: allow`;
//! - `Deny` → `permissionDecision: deny` (the tool never runs);
//! - `Ask { ticket }` → `permissionDecision: ask`, which makes Claude Code
//!   send a `can_use_tool` request for the same `tool_use_id`; that becomes
//!   [`AgentEvent::ApprovalRequested`] (with the ticket's id) and waits for
//!   the client.
//!
//! Anything that goes wrong on our side (gate panics, times out, malformed
//! request) answers `deny`. Claude Code fails **open** on an error response
//! or an invalid decision, so the adapter must always send an explicit one;
//! it fails closed only on its own hook timeout.
//!
//! # Approvals are redeemed (D-046)
//!
//! `ResolveApproval { Allow }` calls [`ToolGate::redeem`] immediately before
//! answering `can_use_tool`, with a [`ToolCheck`] built from that request's
//! own payload. Only an `Allow` from `redeem` lets the tool run; anything
//! else, and every other decision, answers deny. An approval whose request
//! was cancelled or whose turn ended is gone: a late `Allow` changes nothing.
//!
//! # Interrupts and shutdown
//!
//! `SessionCommand::Interrupt` sends the `interrupt` control request. Claude
//! Code ends the turn with a `result` (`error_during_execution`) and stays
//! alive for the next message. If the turn doesn't end within
//! [`SHUTDOWN_GRACE`], the session ends (the adapter never spawns a second
//! process; the daemon resumes in a new incarnation). `Shutdown` closes
//! stdin, waits [`SHUTDOWN_GRACE`], then `atrium_host::supervise::terminate`
//! kills the process group. The closing events go through a bounded tail and
//! are delivered when the receiver drains.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use atrium_host::supervise::{self, ProcessIdentity, SupervisedChild};
use atrium_protocol::agent::{
    AgentEvent, AgentEventEnvelope, ApprovalDecision, Provider, QuestionAnswer, SessionCommand,
};
use atrium_protocol::limits::{
    MAX_DIAGNOSTIC_LINE_BYTES, MAX_PARTIAL_ARGS_BYTES, MAX_PROVIDER_RECORD_BYTES,
    TERMINAL_TAIL_EVENTS,
};
use atrium_protocol::policy::{
    ClassCoverage, Coverage, EnforcementDescriptor, EnforcementReason, ReasonCode, ToolClass,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, JoinHandle, JoinSet};
use tokio::time::{Instant as TokioInstant, sleep_until, timeout};

use super::normalizer::{ClaudeNormalizer, DEADLINE_INTERRUPT, DEADLINE_RECORD_TOO_LARGE};
use super::settings::{InspectInput, Inspection, inspect};
use super::tools::{gate_key, tool_check};
use crate::fixture::FixtureLine;
use crate::{
    AdapterError, AdapterInput, AgentAdapter, ApprovalTicket, BoxFuture, COMMAND_BUFFER,
    EVENT_BUFFER, GateDecision, Normalizer, PermissionPolicy, SHUTDOWN_GRACE, STARTUP_TIMEOUT,
    Session, SessionConfig, ToolCheck, ToolGate, Transport,
};

const INIT_REQUEST_ID: &str = "atrium-init";
const HOOK_CALLBACK_ID: &str = "atrium-pre-tool-use";
const ASK_USER_QUESTION: &str = "AskUserQuestion";
/// Events held back when the client is not reading, before the adapter stops
/// reading the provider (the pipe then applies backpressure to Claude Code).
const PENDING_LIMIT: usize = 4096;
/// How often a provider that closed nothing is checked for being gone (a
/// grandchild can keep its pipes open after it exited).
const ALIVE_POLL: Duration = Duration::from_millis(500);
/// `claude --version` must answer within this.
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest stderr line kept (the rest of the line is dropped).
const STDERR_LINE_BYTES: usize = MAX_DIAGNOSTIC_LINE_BYTES;

/// The CLI versions the fixtures were recorded with (and so the only ones a
/// session can be Full on).
pub const TESTED_VERSIONS: &[&str] = &["2.1.308"];

/// Knobs that are not part of the shared [`SessionConfig`].
#[derive(Clone)]
pub struct ClaudeOptions {
    /// `--setting-sources` value. `None` leaves Claude Code's default (user,
    /// project and local settings). The recorder uses an empty list so
    /// fixtures don't carry the recorder's personal configuration.
    pub setting_sources: Option<String>,
    /// Extra CLI arguments, appended last. `--settings` and `--plugin-dir`
    /// are inspected for the enforcement descriptor.
    pub extra_args: Vec<String>,
    /// How long the gate has to answer a call before it is denied.
    pub gate_deadline: Duration,
    /// Managed settings files to inspect; `None` = this platform's defaults.
    pub managed_settings: Option<Vec<PathBuf>>,
    /// Receives every [`AdapterInput`] with the time since the session
    /// started, for the fixture recorder.
    pub tap: Option<mpsc::UnboundedSender<FixtureLine>>,
}

impl Default for ClaudeOptions {
    fn default() -> Self {
        Self {
            setting_sources: None,
            extra_args: Vec::new(),
            gate_deadline: Duration::from_secs(20),
            managed_settings: None,
            tap: None,
        }
    }
}

#[derive(Default)]
pub struct ClaudeAdapter {
    options: ClaudeOptions,
}

impl ClaudeAdapter {
    pub fn new(options: ClaudeOptions) -> Self {
        Self { options }
    }
}

fn program_of(config: &SessionConfig) -> PathBuf {
    config
        .program
        .clone()
        .unwrap_or_else(|| PathBuf::from("claude"))
}

/// `claude --version`, e.g. `2.1.308 (Claude Code)` → `2.1.308`.
async fn cli_version(config: &SessionConfig) -> Option<String> {
    let mut cmd = Command::new(program_of(config));
    cmd.arg("--version")
        .current_dir(&config.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    cmd.envs(config.env.iter().map(|(k, v)| (k, v)));
    let out = timeout(VERSION_TIMEOUT, cmd.output()).await.ok()?.ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let first = text.split_whitespace().next()?;
    first
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit())
        .then(|| first.to_owned())
}

/// What the evidence supports, per tool class. Every class is intercepted by
/// the `PreToolUse` hook, which fires for built-ins, MCP tools, subagent
/// tools and Claude's own tools in every permission mode.
fn coverage() -> Vec<ClassCoverage> {
    let ev = |class: ToolClass, evidence: &str| ClassCoverage {
        class,
        coverage: Coverage::Intercepted,
        evidence: Some(format!("fixtures/agents/claude/{evidence}")),
    };
    vec![
        ev(ToolClass::Read, "gate-deny"),
        ev(ToolClass::Edit, "gate-deny-edit"),
        // Claude Code has no delete or move tool: `rm` and `mv` run through
        // Bash, which the hook intercepts.
        ev(ToolClass::Delete, "gate-deny-delete"),
        ev(ToolClass::Move, "gate-deny-delete"),
        ev(ToolClass::Search, "gate-deny"),
        ev(ToolClass::Execute, "gate-deny-exec"),
        ev(ToolClass::Fetch, "gate-deny-fetch"),
        ev(ToolClass::Mcp, "gate-deny-mcp"),
        ev(ToolClass::Subagent, "gate-deny-subagent"),
        // The hook is generic: Claude's own tools (ToolSearch, Skill,
        // Monitor...) pass the same gate.
        ev(ToolClass::ProviderSpecific, "mcp-allow"),
    ]
}

/// What the session knows to build its descriptor from.
#[derive(Clone)]
struct DescriptorInputs {
    cwd: PathBuf,
    env: Vec<(String, String)>,
    setting_sources: Option<String>,
    extra_args: Vec<String>,
    managed: Option<Vec<PathBuf>>,
    version: Option<String>,
    gate_unreachable: bool,
}

impl DescriptorInputs {
    fn descriptor(&self) -> EnforcementDescriptor {
        let Inspection {
            sources,
            mut reasons,
        } = inspect(&InspectInput {
            cwd: &self.cwd,
            env: &self.env,
            setting_sources: self.setting_sources.as_deref(),
            extra_args: &self.extra_args,
            managed: self.managed.as_deref(),
        });
        if self.gate_unreachable {
            reasons.push(EnforcementReason {
                code: ReasonCode::GateUnreachable,
                message: "The policy gate didn't answer; calls were denied".to_owned(),
            });
        }
        let tested = self
            .version
            .as_deref()
            .is_some_and(|v| TESTED_VERSIONS.contains(&v));
        EnforcementDescriptor::evaluate(
            Provider::Claude,
            self.version.clone(),
            tested,
            coverage(),
            sources,
            reasons,
        )
    }
}

impl AgentAdapter for ClaudeAdapter {
    fn provider(&self) -> Provider {
        Provider::Claude
    }

    fn tested_versions(&self) -> &'static [&'static str] {
        TESTED_VERSIONS
    }

    fn probe<'a>(&'a self, config: &'a SessionConfig) -> BoxFuture<'a, EnforcementDescriptor> {
        Box::pin(async move {
            let version = cli_version(config).await;
            self.inputs(config, version).descriptor()
        })
    }

    fn start(
        &self,
        config: SessionConfig,
        transport: Transport,
    ) -> BoxFuture<'_, Result<Session, AdapterError>> {
        let options = self.options.clone();
        Box::pin(async move {
            match transport {
                Transport::Local => {}
                #[allow(unreachable_patterns)]
                _ => return Err(AdapterError::Unsupported("transport".into())),
            }
            let version = cli_version(&config).await;
            let inputs = self.inputs(&config, version);
            let descriptor = inputs.descriptor();
            let (events_tx, events_rx) = mpsc::channel(EVENT_BUFFER);
            let (commands_tx, commands_rx) = mpsc::channel(COMMAND_BUFFER);
            let mut actor = Actor::new(config, options, inputs, descriptor.clone(), events_tx, commands_rx)?;
            let identity = actor.launch()?;
            if let Err(e) = actor.handshake().await {
                actor.discard();
                return Err(e);
            }
            tokio::spawn(actor.run());
            Ok(Session {
                descriptor,
                process: Some(identity),
                events: events_rx,
                commands: commands_tx,
            })
        })
    }
}

impl ClaudeAdapter {
    fn inputs(&self, config: &SessionConfig, version: Option<String>) -> DescriptorInputs {
        DescriptorInputs {
            cwd: config.cwd.clone(),
            env: config.env.clone(),
            setting_sources: self.options.setting_sources.clone(),
            extra_args: self.options.extra_args.clone(),
            managed: self.options.managed_settings.clone(),
            version,
            gate_unreachable: false,
        }
    }
}

// ---------------------------------------------------------------------------

enum Out {
    Stdout(String),
    Stderr(String),
    /// A stdout record longer than [`MAX_PROVIDER_RECORD_BYTES`]; the reader
    /// stops.
    Oversize,
}

enum Ev {
    Command(Option<SessionCommand>),
    Internal(Box<Internal>),
    InterruptDeadline,
    Flushed,
    ConsumerGone,
    Output(Option<Out>),
    AlivePoll,
}

enum Internal {
    Gate {
        reply: Reply,
        check: ToolCheck,
        decision: GateDecision,
        failed: bool,
    },
    Redeem {
        native: String,
        ticket: ApprovalTicket,
        check: ToolCheck,
        input: Value,
        decision: GateDecision,
        failed: bool,
    },
    Question {
        native: String,
        ticket: Option<ApprovalTicket>,
        input: Value,
        failed: bool,
    },
}

/// What to do with a gate decision.
enum Reply {
    /// A `PreToolUse` hook callback.
    Hook { request_id: String },
    /// A `can_use_tool` request the CLI asked directly.
    CanUse { request_id: String, input: Value },
}

/// A `can_use_tool` approval waiting for the client, by ticket id.
struct PendingApproval {
    native: String,
    ticket: ApprovalTicket,
    tool_use_id: Option<String>,
    name: String,
    input: Value,
}

struct AbortOnDrop(AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Proc {
    child: SupervisedChild,
    identity: ProcessIdentity,
    stdin_tx: Option<mpsc::UnboundedSender<String>>,
    writer: JoinHandle<()>,
    readers: Vec<JoinHandle<()>>,
}

struct Actor {
    config: SessionConfig,
    options: ClaudeOptions,
    gate: Arc<dyn ToolGate>,
    norm: ClaudeNormalizer,
    started: Instant,
    seq: u64,

    inputs: DescriptorInputs,
    descriptor: EnforcementDescriptor,

    events_tx: mpsc::Sender<AgentEventEnvelope>,
    commands_rx: Option<mpsc::Receiver<SessionCommand>>,
    internal_tx: mpsc::UnboundedSender<Internal>,
    internal_rx: mpsc::UnboundedReceiver<Internal>,
    pending: VecDeque<AgentEventEnvelope>,
    /// Closing events queued since the session began ending.
    closing: usize,
    finishing: bool,

    proc: Option<Proc>,
    out_rx: Option<mpsc::Receiver<Out>>,
    interrupt_deadline: Option<TokioInstant>,
    queue: VecDeque<String>,
    shutting_down: bool,
    /// In-flight gate calls; aborted when the session ends.
    gates: JoinSet<()>,

    /// Gate keys the hook answered `ask` for, with the gate's ticket.
    hook_asked: HashMap<String, ApprovalTicket>,
    /// `can_use_tool` requests not answered yet (Claude's own ids).
    live: HashSet<String>,
    approvals: HashMap<String, PendingApproval>,
    /// Questions waiting for an answer, by ticket id: (native id, input).
    questions: HashMap<String, (String, Value)>,

    mcp_dir: Option<TempDir>,
    mcp_path: PathBuf,
}

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

impl Actor {
    fn new(
        config: SessionConfig,
        options: ClaudeOptions,
        inputs: DescriptorInputs,
        descriptor: EnforcementDescriptor,
        events_tx: mpsc::Sender<AgentEventEnvelope>,
        commands_rx: mpsc::Receiver<SessionCommand>,
    ) -> Result<Self, AdapterError> {
        let dir = std::env::temp_dir().join(format!(
            "atrium-claude-{}-{}-{}",
            std::process::id(),
            SESSION_COUNTER.fetch_add(1, Ordering::Relaxed),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        private_dir(&dir)?;
        let mcp_dir = TempDir(dir.clone());
        let mcp_path = dir.join("mcp.json");
        write_private(&mcp_path, mcp_config(&config).to_string().as_bytes())?;
        let (internal_tx, internal_rx) = mpsc::unbounded_channel();
        Ok(Self {
            gate: config.gate.clone(),
            config,
            options,
            norm: ClaudeNormalizer::new(),
            started: Instant::now(),
            seq: 0,
            inputs,
            descriptor,
            events_tx,
            commands_rx: Some(commands_rx),
            internal_tx,
            internal_rx,
            pending: VecDeque::new(),
            closing: 0,
            finishing: false,
            proc: None,
            out_rx: None,
            interrupt_deadline: None,
            queue: VecDeque::new(),
            shutting_down: false,
            gates: JoinSet::new(),
            hook_asked: HashMap::new(),
            live: HashSet::new(),
            approvals: HashMap::new(),
            questions: HashMap::new(),
            mcp_dir: Some(mcp_dir),
            mcp_path,
        })
    }

    // ---- process ---------------------------------------------------------

    fn args(&self) -> Vec<String> {
        let mut a: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
            "--strict-mcp-config",
            "--mcp-config",
        ]
        .map(String::from)
        .to_vec();
        a.push(self.mcp_path.display().to_string());
        // Ask and ReadOnly both use Claude's normal mode with every decision
        // routed to the gate; Claude has no separate read-only mode.
        if self.config.permissions == PermissionPolicy::FullAccess {
            a.extend(["--permission-mode".into(), "bypassPermissions".into()]);
        }
        if let Some(s) = &self.options.setting_sources {
            a.extend(["--setting-sources".into(), s.clone()]);
        }
        if let Some(m) = &self.config.model {
            a.extend(["--model".into(), m.clone()]);
        }
        if let Some(e) = &self.config.effort {
            a.extend(["--effort".into(), e.clone()]);
        }
        if let Some(r) = &self.config.resume {
            a.extend(["--resume".into(), r.clone()]);
        }
        a.extend(self.options.extra_args.iter().cloned());
        a
    }

    /// Starts the process (as the leader of its own group), reports it
    /// through `on_spawn`, and sends `initialize`.
    fn launch(&mut self) -> Result<ProcessIdentity, AdapterError> {
        let mut cmd = Command::new(program_of(&self.config));
        cmd.args(self.args())
            .current_dir(&self.config.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Only this session's MCP servers; no claude.ai connectors.
        cmd.env("ENABLE_CLAUDEAI_MCP_SERVERS", "false");
        cmd.envs(self.config.env.iter().map(|(k, v)| (k, v)));
        let mut child = supervise::spawn_group(cmd)?;
        let identity = child.identity.clone();
        if let Some(on_spawn) = &self.config.on_spawn {
            on_spawn(&identity);
        }
        let stdin = child
            .child
            .stdin
            .take()
            .ok_or_else(|| AdapterError::Handshake("no stdin pipe".into()))?;
        let stdout = child
            .child
            .stdout
            .take()
            .ok_or_else(|| AdapterError::Handshake("no stdout pipe".into()))?;
        let stderr = child
            .child
            .stderr
            .take()
            .ok_or_else(|| AdapterError::Handshake("no stderr pipe".into()))?;

        let (out_tx, out_rx) = mpsc::channel::<Out>(64);
        let readers = vec![
            tokio::spawn(pump(stdout, out_tx.clone(), false)),
            tokio::spawn(pump(stderr, out_tx, true)),
        ];
        let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<String>();
        // Writes never block the session loop: a provider that doesn't read
        // its stdin holds up this task only.
        let writer = tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(line) = stdin_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err()
                    || stdin.write_all(b"\n").await.is_err()
                    || stdin.flush().await.is_err()
                {
                    break;
                }
            }
            // Dropping `stdin` closes the pipe: Claude Code exits on EOF.
        });
        self.proc = Some(Proc {
            child,
            identity: identity.clone(),
            stdin_tx: Some(stdin_tx),
            writer,
            readers,
        });
        self.out_rx = Some(out_rx);

        self.feed(AdapterInput::Enforcement {
            descriptor: self.descriptor.clone(),
        });
        self.write_value(&json!({
            "type": "control_request",
            "request_id": INIT_REQUEST_ID,
            "request": {
                "subtype": "initialize",
                "hooks": {
                    "PreToolUse": [{
                        "matcher": null,
                        "hookCallbackIds": [HOOK_CALLBACK_ID],
                        "timeout": self.options.gate_deadline.as_secs() + 10,
                    }]
                }
            }
        }));
        Ok(identity)
    }

    async fn handshake(&mut self) -> Result<(), AdapterError> {
        let deadline = TokioInstant::now() + STARTUP_TIMEOUT;
        let mut tail: VecDeque<String> = VecDeque::new();
        loop {
            let next = {
                let Some(rx) = self.out_rx.as_mut() else {
                    return Err(AdapterError::Handshake("no process".into()));
                };
                tokio::time::timeout_at(deadline, rx.recv()).await
            };
            match next {
                Ok(Some(Out::Stdout(line))) => {
                    let is_init = is_init_response(&line);
                    self.feed(AdapterInput::Stdout { line });
                    if is_init {
                        return Ok(());
                    }
                }
                Ok(Some(Out::Stderr(line))) => {
                    if tail.len() >= 3 {
                        tail.pop_front();
                    }
                    tail.push_back(line.clone());
                    self.feed(AdapterInput::Stderr { line });
                }
                Ok(Some(Out::Oversize)) => {
                    self.stop_process(Duration::ZERO).await;
                    return Err(AdapterError::Handshake(format!(
                        "claude sent a record over {MAX_PROVIDER_RECORD_BYTES} bytes"
                    )));
                }
                Ok(None) => {
                    let code = self.stop_process(Duration::ZERO).await;
                    let tail: Vec<String> = tail.into_iter().collect();
                    return Err(AdapterError::Handshake(format!(
                        "claude exited before answering initialize (code {code:?}): {}",
                        tail.join(" | ")
                    )));
                }
                Err(_) => {
                    self.stop_process(Duration::ZERO).await;
                    return Err(AdapterError::Handshake(format!(
                        "claude did not answer initialize within {STARTUP_TIMEOUT:?}"
                    )));
                }
            }
        }
    }

    /// A failed `start` returns an error instead of events.
    fn discard(&mut self) {
        self.pending.clear();
        self.mcp_dir = None;
    }

    /// Closes stdin (the soft stop), keeps draining the provider's output so
    /// a full pipe can't hold it up, and, after `grace`, kills its whole
    /// process group and reaps it (`atrium_host::supervise::terminate`).
    /// Returns the exit code.
    async fn stop_process(&mut self, grace: Duration) -> Option<i32> {
        let mut p = self.proc.take()?;
        p.stdin_tx = None;
        let mut rx = self.out_rx.take();
        let result = {
            let term = supervise::terminate(&mut p.child, grace);
            tokio::pin!(term);
            loop {
                tokio::select! {
                    r = &mut term => break r,
                    out = recv_out(&mut rx) => match out {
                        Some(out) => self.on_out(out, false),
                        None => rx = None,
                    },
                }
            }
        };
        let code = match result {
            Ok(t) => {
                if t.killed {
                    self.feed(AdapterInput::Deadline {
                        what: "shutdown_grace".into(),
                    });
                }
                t.exit_code
            }
            Err(e) => {
                self.feed(AdapterInput::Stderr {
                    line: format!("atrium: could not verify the provider's process group: {e}"),
                });
                None
            }
        };
        // Whatever was written last.
        if let Some(mut rx) = rx {
            let end = TokioInstant::now() + Duration::from_millis(200);
            while let Ok(Some(out)) = tokio::time::timeout_at(end, rx.recv()).await {
                self.on_out(out, false);
            }
        }
        p.writer.abort();
        for r in p.readers {
            r.abort();
        }
        code
    }

    // ---- inputs and events -------------------------------------------------

    fn feed(&mut self, input: AdapterInput) {
        if let Some(tap) = &self.options.tap {
            let _ = tap.send(FixtureLine {
                ms: self.started.elapsed().as_millis() as u64,
                input: input.clone(),
            });
        }
        let events = self.norm.on_input(&input);
        for event in events {
            self.push_event(event);
        }
    }

    fn push_event(&mut self, event: AgentEvent) {
        if self.finishing {
            // The tail is bounded; `SessionEnded` is always kept.
            if self.closing >= TERMINAL_TAIL_EVENTS
                && !matches!(event, AgentEvent::SessionEnded { .. })
            {
                return;
            }
            self.closing += 1;
        }
        let at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.pending.push_back(AgentEventEnvelope {
            incarnation: 0,
            seq: self.seq,
            at_ms,
            event,
        });
        self.seq += 1;
    }

    fn write_value(&mut self, v: &Value) {
        let line = v.to_string();
        self.feed(AdapterInput::Stdin { line: line.clone() });
        if let Some(tx) = self.proc.as_ref().and_then(|p| p.stdin_tx.as_ref())
            && tx.send(line).is_err()
        {
            self.feed(AdapterInput::WriteFailed {
                error: "the provider's stdin is closed".into(),
            });
        }
    }

    fn respond(&mut self, request_id: &str, response: Value) {
        self.write_value(&json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": request_id,
                "response": response,
            }
        }));
    }

    fn respond_hook(&mut self, request_id: &str, decision: &str, reason: Option<&str>) {
        let mut out = json!({
            "hookEventName": "PreToolUse",
            "permissionDecision": decision,
        });
        if let Some(r) = reason {
            out["permissionDecisionReason"] = json!(r);
        }
        self.respond(request_id, json!({ "hookSpecificOutput": out }));
    }

    fn deny_can_use(&mut self, request_id: &str, message: &str) {
        self.live.remove(request_id);
        self.respond(request_id, json!({"behavior": "deny", "message": message}));
    }

    // ---- main loop -------------------------------------------------------------

    async fn run(mut self) {
        let mut consumer_gone = false;
        let mut alive = tokio::time::interval(ALIVE_POLL);
        alive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            if self.shutting_down {
                break;
            }
            if self.norm.pre_session_failed() || self.norm.wants_end() {
                // Nothing useful will come out of this process.
                self.finish(false, SHUTDOWN_GRACE).await;
                break;
            }
            let flush = !self.pending.is_empty() && !consumer_gone;
            let can_read = self.pending.len() < PENDING_LIMIT && self.out_rx.is_some();
            let deadline = self.interrupt_deadline;
            let ev = {
                let Some(commands_rx) = self.commands_rx.as_mut() else {
                    break;
                };
                tokio::select! {
                    biased;
                    cmd = commands_rx.recv() => Ev::Command(cmd),
                    Some(msg) = self.internal_rx.recv() => Ev::Internal(Box::new(msg)),
                    _ = sleep_until(deadline.unwrap_or_else(TokioInstant::now)), if deadline.is_some() => Ev::InterruptDeadline,
                    permit = self.events_tx.reserve(), if flush => match permit {
                        Ok(p) => {
                            if let Some(ev) = self.pending.pop_front() {
                                p.send(ev);
                            }
                            Ev::Flushed
                        }
                        Err(_) => Ev::ConsumerGone,
                    },
                    out = recv_out(&mut self.out_rx), if can_read => Ev::Output(out),
                    _ = alive.tick(), if self.proc.is_some() => Ev::AlivePoll,
                }
            };
            match ev {
                Ev::Command(Some(c)) => self.on_command(c).await,
                Ev::Command(None) => {
                    self.finish(true, SHUTDOWN_GRACE).await;
                    break;
                }
                Ev::Internal(msg) => self.on_internal(*msg),
                Ev::InterruptDeadline => {
                    // The child ignored the interrupt: the session ends and
                    // the daemon resumes it in a new incarnation.
                    self.interrupt_deadline = None;
                    self.feed(AdapterInput::Deadline {
                        what: DEADLINE_INTERRUPT.into(),
                    });
                    self.finish(false, Duration::ZERO).await;
                    break;
                }
                Ev::Flushed => {}
                Ev::ConsumerGone => {
                    consumer_gone = true;
                    self.pending.clear();
                }
                Ev::Output(Some(out)) => self.on_out(out, true),
                // Both pipes closed.
                Ev::Output(None) => {
                    self.finish(false, Duration::ZERO).await;
                    break;
                }
                Ev::AlivePoll => {
                    let gone = self
                        .proc
                        .as_ref()
                        .is_some_and(|p| matches!(supervise::identify(p.identity.pid), Ok(None)));
                    if gone {
                        self.finish(false, Duration::ZERO).await;
                        break;
                    }
                }
            }
            if consumer_gone {
                self.pending.clear();
            }
        }
        self.gates.abort_all();
        self.commands_rx = None;
        // Closing events reach a receiver that was not reading as soon as it
        // drains; the task ends when everything is delivered or the receiver
        // is dropped.
        if !consumer_gone {
            while let Some(ev) = self.pending.pop_front() {
                if self.events_tx.send(ev).await.is_err() {
                    break;
                }
            }
        }
    }

    /// Ends the session: stop the provider (grace, then kill the group, reap),
    /// feed the closing inputs, and let `run` flush the events. `requested`
    /// is false for endings the adapter or the provider itself caused.
    async fn finish(&mut self, requested: bool, grace: Duration) {
        self.shutting_down = true;
        self.finishing = true;
        self.gates.abort_all();
        if requested {
            self.feed(AdapterInput::Command {
                command: SessionCommand::Shutdown,
            });
        }
        self.interrupt_deadline = None;
        let code = self.stop_process(grace).await;
        self.out_rx = None;
        self.clear_requests();
        self.feed(AdapterInput::Exit { code });
        // The generated MCP config (it can hold credentials) goes with the
        // process, not with the delivery of the last events.
        self.mcp_dir = None;
    }

    fn clear_requests(&mut self) {
        self.hook_asked.clear();
        self.live.clear();
        self.approvals.clear();
        self.questions.clear();
    }

    // ---- provider output ---------------------------------------------------------

    /// `active` is false while the provider is winding down: its requests
    /// are recorded but not answered.
    fn on_out(&mut self, out: Out, active: bool) {
        match out {
            Out::Stdout(line) => {
                if active || self.pending.len() < PENDING_LIMIT {
                    self.on_stdout(line, active);
                }
            }
            Out::Stderr(line) => self.feed(AdapterInput::Stderr { line }),
            Out::Oversize => {
                self.feed(AdapterInput::Deadline {
                    what: DEADLINE_RECORD_TOO_LARGE.into(),
                });
            }
        }
    }

    fn on_stdout(&mut self, line: String, active: bool) {
        let parsed: Option<Value> = serde_json::from_str(&line).ok();
        let big = line.len() > MAX_PARTIAL_ARGS_BYTES;
        self.feed(AdapterInput::Stdout { line });
        let Some(v) = parsed.filter(|_| active) else {
            return;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("control_request") => self.on_control_request(&v, big),
            Some("control_cancel_request") => {
                if let Some(rid) = v.get("request_id").and_then(Value::as_str) {
                    self.forget_native(rid);
                }
            }
            Some("result") => {
                // The turn is over: nothing it was waiting for can run.
                self.interrupt_deadline = None;
                self.clear_requests();
                self.dispatch_next();
            }
            _ => {}
        }
    }

    /// The provider withdrew a request.
    fn forget_native(&mut self, native: &str) {
        self.live.remove(native);
        self.approvals.retain(|_, a| a.native != native);
        self.questions.retain(|_, (n, _)| n != native);
    }

    fn on_control_request(&mut self, v: &Value, big: bool) {
        let Some(request_id) = v
            .get("request_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return;
        };
        let req = v.get("request").cloned().unwrap_or(Value::Null);
        match req.get("subtype").and_then(Value::as_str) {
            Some("hook_callback") => self.on_hook(request_id, &req, big),
            Some("can_use_tool") => self.on_can_use(request_id, &req),
            _ => {
                // Unknown request: tell Claude Code we don't handle it.
                self.write_value(&json!({
                    "type": "control_response",
                    "response": {
                        "subtype": "error",
                        "request_id": request_id,
                        "error": "unsupported control request",
                    }
                }));
            }
        }
    }

    fn on_hook(&mut self, request_id: String, req: &Value, big: bool) {
        let input = req.get("input").cloned().unwrap_or(Value::Null);
        let name = input.get("tool_name").and_then(Value::as_str);
        let callback = req.get("callback_id").and_then(Value::as_str);
        let (Some(name), true) = (name, callback == Some(HOOK_CALLBACK_ID)) else {
            self.respond_hook(
                &request_id,
                "deny",
                Some("Denied by Atrium: malformed hook call"),
            );
            return;
        };
        let tool_input = input.get("tool_input").cloned().unwrap_or(Value::Null);
        if big && tool_input.to_string().len() > MAX_PARTIAL_ARGS_BYTES {
            // Never shown to the gate (contract §6: oversize arguments).
            self.respond_hook(
                &request_id,
                "deny",
                Some("Denied by Atrium: the tool arguments are too large"),
            );
            return;
        }
        if name == ASK_USER_QUESTION {
            // The question is the interaction: its ticket comes with the
            // `can_use_tool` request that follows, and the user answers it.
            self.respond_hook(&request_id, "allow", None);
            return;
        }
        let call_id = input.get("tool_use_id").and_then(Value::as_str);
        let check = tool_check(call_id, name, &tool_input, self.norm.turn_number());
        self.spawn_gate(Reply::Hook { request_id }, check);
    }

    fn on_can_use(&mut self, request_id: String, req: &Value) {
        let name = req.get("tool_name").and_then(Value::as_str).unwrap_or("");
        let input = req.get("input").cloned().unwrap_or(Value::Null);
        let tool_use_id = req
            .get("tool_use_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        self.live.insert(request_id.clone());
        if name == ASK_USER_QUESTION {
            self.spawn_question(request_id, input);
            return;
        }
        let key = gate_key(tool_use_id.as_deref(), name, &input);
        if let Some(ticket) = self.hook_asked.remove(&key) {
            self.approvals.insert(
                ticket.request_id.clone(),
                PendingApproval {
                    native: request_id,
                    ticket,
                    tool_use_id,
                    name: name.to_owned(),
                    input,
                },
            );
            return;
        }
        // Nobody asked the gate about this one: it does now.
        let check = tool_check(tool_use_id.as_deref(), name, &input, self.norm.turn_number());
        self.spawn_gate(Reply::CanUse { request_id, input }, check);
    }

    fn spawn_gate(&mut self, reply: Reply, check: ToolCheck) {
        let gate = self.gate.clone();
        let tx = self.internal_tx.clone();
        let deadline = self.options.gate_deadline;
        self.gates.spawn(async move {
            let for_gate = check.clone();
            // The call runs in its own task so a panic is contained, and is
            // aborted if this task is (session end) or the deadline passes.
            let inner = tokio::spawn(async move { gate.check(&for_gate).await });
            let (decision, failed) = match guarded(deadline, inner).await {
                Ok(d) => (d, false),
                Err(why) => (
                    GateDecision::Deny {
                        reason: why.reason().into(),
                    },
                    true,
                ),
            };
            let _ = tx.send(Internal::Gate {
                reply,
                check,
                decision,
                failed,
            });
        });
    }

    fn spawn_question(&mut self, native: String, input: Value) {
        let gate = self.gate.clone();
        let tx = self.internal_tx.clone();
        let deadline = self.options.gate_deadline;
        let turn = Some(self.norm.turn_number());
        self.gates.spawn(async move {
            let inner = tokio::spawn(async move { gate.question_ticket(turn).await });
            let (ticket, failed) = match guarded(deadline, inner).await {
                Ok(t) => (t, false),
                Err(_) => (None, true),
            };
            let _ = tx.send(Internal::Question {
                native,
                ticket,
                input,
                failed,
            });
        });
    }

    fn spawn_redeem(&mut self, a: PendingApproval) {
        let gate = self.gate.clone();
        let tx = self.internal_tx.clone();
        let deadline = self.options.gate_deadline;
        // Built from the provider's own approval request, now.
        let check = tool_check(
            a.tool_use_id.as_deref(),
            &a.name,
            &a.input,
            self.norm.turn_number(),
        );
        self.gates.spawn(async move {
            let (for_gate, ticket) = (check.clone(), a.ticket.clone());
            let inner = tokio::spawn(async move { gate.redeem(&ticket, &for_gate).await });
            let (decision, failed) = match guarded(deadline, inner).await {
                Ok(d) => (d, false),
                Err(why) => (
                    GateDecision::Deny {
                        reason: why.reason().into(),
                    },
                    true,
                ),
            };
            let _ = tx.send(Internal::Redeem {
                native: a.native,
                ticket: a.ticket,
                check,
                input: a.input,
                decision,
                failed,
            });
        });
    }

    fn on_internal(&mut self, msg: Internal) {
        while self.gates.try_join_next().is_some() {}
        let failed = match &msg {
            Internal::Gate { failed, .. }
            | Internal::Redeem { failed, .. }
            | Internal::Question { failed, .. } => *failed,
        };
        match msg {
            Internal::Gate {
                reply,
                check,
                decision,
                ..
            } => {
                self.feed(AdapterInput::Gate {
                    check: check.clone(),
                    decision: decision.clone(),
                });
                self.on_gate_decision(reply, check, decision);
            }
            Internal::Redeem {
                native,
                ticket,
                check,
                input,
                decision,
                ..
            } => {
                self.feed(AdapterInput::Redeem {
                    ticket,
                    check,
                    decision: decision.clone(),
                });
                // The request may have been withdrawn while the gate decided.
                if !self.live.remove(&native) || self.proc.is_none() {
                    return;
                }
                let response = match decision {
                    GateDecision::Allow => json!({"behavior": "allow", "updatedInput": input}),
                    GateDecision::Deny { reason } => {
                        json!({"behavior": "deny", "message": deny_text(&reason)})
                    }
                    GateDecision::Ask { .. } => {
                        json!({"behavior": "deny", "message": deny_text("approval needs another decision")})
                    }
                };
                self.respond(&native, response);
            }
            Internal::Question {
                native,
                ticket,
                input,
                ..
            } => {
                self.feed(AdapterInput::QuestionTicket {
                    id: native.clone(),
                    ticket: ticket.clone(),
                });
                if !self.live.contains(&native) || self.proc.is_none() {
                    return;
                }
                match ticket {
                    Some(t) => {
                        self.questions.insert(t.request_id, (native, input));
                    }
                    None => self.deny_can_use(&native, "Atrium declined the question"),
                }
            }
        }
        if failed {
            self.gate_failed();
        }
    }

    fn on_gate_decision(&mut self, reply: Reply, check: ToolCheck, decision: GateDecision) {
        // A gate answer for a process that is gone has nobody to hear it.
        if self.proc.is_none() {
            return;
        }
        match reply {
            Reply::Hook { request_id } => match decision {
                GateDecision::Allow => self.respond_hook(&request_id, "allow", None),
                GateDecision::Deny { reason } => {
                    self.respond_hook(&request_id, "deny", Some(&deny_text(&reason)));
                }
                GateDecision::Ask { ticket } => {
                    let key = gate_key(check.call_id.as_deref(), &check.name, &check.input);
                    self.hook_asked.insert(key, ticket);
                    self.respond_hook(&request_id, "ask", None);
                }
            },
            Reply::CanUse { request_id, input } => match decision {
                GateDecision::Allow => {
                    self.live.remove(&request_id);
                    self.respond(
                        &request_id,
                        json!({"behavior": "allow", "updatedInput": input}),
                    );
                }
                GateDecision::Deny { reason } => {
                    self.deny_can_use(&request_id, &deny_text(&reason));
                }
                GateDecision::Ask { ticket } => {
                    self.approvals.insert(
                        ticket.request_id.clone(),
                        PendingApproval {
                            native: request_id,
                            ticket,
                            tool_use_id: check.call_id,
                            name: check.name,
                            input,
                        },
                    );
                }
            },
        }
    }

    /// The gate failed to answer: calls were denied, and the descriptor says so.
    fn gate_failed(&mut self) {
        self.feed(AdapterInput::Deadline {
            what: "hook".into(),
        });
        if !self.inputs.gate_unreachable {
            self.inputs.gate_unreachable = true;
            self.refresh_enforcement();
        }
    }

    /// Rebuilds the descriptor (a settings file or plugin may have changed)
    /// and records it when it differs.
    fn refresh_enforcement(&mut self) {
        let descriptor = self.inputs.descriptor();
        if descriptor != self.descriptor {
            self.descriptor = descriptor.clone();
            self.feed(AdapterInput::Enforcement { descriptor });
        }
    }

    // ---- client commands -----------------------------------------------------------------

    async fn on_command(&mut self, command: SessionCommand) {
        match command {
            SessionCommand::Shutdown => {
                self.finish(true, SHUTDOWN_GRACE).await;
            }
            SessionCommand::Interrupt => {
                self.feed(AdapterInput::Command {
                    command: SessionCommand::Interrupt,
                });
                if self.proc.is_some() && self.norm.turn_in_flight() {
                    self.write_value(&json!({
                        "type": "control_request",
                        "request_id": format!("atrium-interrupt-{}", self.seq),
                        "request": {"subtype": "interrupt"},
                    }));
                    self.interrupt_deadline = Some(TokioInstant::now() + SHUTDOWN_GRACE);
                }
            }
            SessionCommand::SendUserMessage { text } => {
                // The normalizer owns the limit, so a replay refuses the same
                // messages; this only keeps the texts.
                let busy = self.norm.turn_in_flight() || !self.queue.is_empty();
                self.feed(AdapterInput::Command {
                    command: SessionCommand::SendUserMessage { text: text.clone() },
                });
                if self.norm.take_rejected() {
                    return;
                }
                self.queue.push_back(text);
                if !busy {
                    self.dispatch_next();
                }
            }
            SessionCommand::ResolveApproval {
                request_id,
                decision,
            } => {
                let approval = self.approvals.remove(&request_id);
                self.feed(AdapterInput::Command {
                    command: SessionCommand::ResolveApproval {
                        request_id: request_id.clone(),
                        decision: decision.clone(),
                    },
                });
                // Unknown, expired or withdrawn: nothing happens.
                let Some(a) = approval else {
                    return;
                };
                match decision {
                    ApprovalDecision::Allow => self.spawn_redeem(a),
                    ApprovalDecision::Deny { message } => {
                        let text = message.unwrap_or_else(|| "Denied by the user".into());
                        self.deny_can_use(&a.native, &text);
                    }
                }
            }
            SessionCommand::AnswerQuestion {
                request_id,
                answers,
            } => {
                let question = self.questions.remove(&request_id);
                self.feed(AdapterInput::Command {
                    command: SessionCommand::AnswerQuestion {
                        request_id: request_id.clone(),
                        answers: answers.clone(),
                    },
                });
                if let Some((native, input)) = question {
                    self.live.remove(&native);
                    let updated = answered_input(&input, &answers);
                    self.respond(
                        &native,
                        json!({"behavior": "allow", "updatedInput": updated}),
                    );
                }
            }
        }
    }

    /// Writes the next queued message unless a turn is running.
    fn dispatch_next(&mut self) {
        if self.norm.turn_in_flight() || self.queue.is_empty() || self.proc.is_none() {
            return;
        }
        if let Some(text) = self.queue.pop_front() {
            // A settings file or plugin may have changed since the last turn.
            self.refresh_enforcement();
            self.write_value(&json!({
                "type": "user",
                "message": {"role": "user", "content": text},
            }));
        }
    }
}

enum GuardFail {
    Panicked,
    TimedOut,
}

impl GuardFail {
    fn reason(&self) -> &'static str {
        match self {
            GuardFail::Panicked => "policy gate failed",
            GuardFail::TimedOut => "policy gate timed out",
        }
    }
}

/// Waits for a gate call that runs in its own task, so a panic is contained;
/// the task is aborted when the deadline passes or this future is dropped.
async fn guarded<T: Send + 'static>(
    deadline: Duration,
    inner: JoinHandle<T>,
) -> Result<T, GuardFail> {
    let _guard = AbortOnDrop(inner.abort_handle());
    match timeout(deadline, inner).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(_)) => Err(GuardFail::Panicked),
        Err(_) => Err(GuardFail::TimedOut),
    }
}

async fn recv_out(rx: &mut Option<mpsc::Receiver<Out>>) -> Option<Out> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Reads lines from a pipe with a length limit: a stdout record over
/// [`MAX_PROVIDER_RECORD_BYTES`] ends the reader with [`Out::Oversize`]; a
/// stderr line is cut to [`STDERR_LINE_BYTES`].
async fn pump<R: AsyncRead + Unpin>(reader: R, tx: mpsc::Sender<Out>, stderr: bool) {
    let limit = if stderr {
        STDERR_LINE_BYTES
    } else {
        MAX_PROVIDER_RECORD_BYTES
    };
    let mut reader = BufReader::new(reader);
    let mut line: Vec<u8> = Vec::new();
    let mut over = false;
    loop {
        let chunk = match reader.fill_buf().await {
            Ok(c) if !c.is_empty() => c,
            _ => {
                if !line.is_empty() && !over {
                    let _ = tx.send(out_of(&line, stderr)).await;
                }
                return;
            }
        };
        let (piece, rest, ends) = match chunk.iter().position(|b| *b == b'\n') {
            Some(i) => (&chunk[..i], i + 1, true),
            None => (chunk, chunk.len(), false),
        };
        if !over {
            let room = limit.saturating_sub(line.len());
            if piece.len() > room {
                if stderr {
                    line.extend_from_slice(&piece[..room]);
                }
                over = true;
            } else {
                line.extend_from_slice(piece);
            }
        }
        reader.consume(rest);
        if ends {
            let sent = if over && !stderr {
                let _ = tx.send(Out::Oversize).await;
                return;
            } else {
                tx.send(out_of(&line, stderr)).await
            };
            if sent.is_err() {
                return;
            }
            line.clear();
            over = false;
        }
    }
}

fn out_of(line: &[u8], stderr: bool) -> Out {
    let text = String::from_utf8_lossy(line).into_owned();
    if stderr {
        Out::Stderr(text)
    } else {
        Out::Stdout(text)
    }
}

fn is_init_response(line: &str) -> bool {
    serde_json::from_str::<Value>(line).is_ok_and(|v| {
        v.get("type").and_then(Value::as_str) == Some("control_response")
            && v.pointer("/response/request_id").and_then(Value::as_str) == Some(INIT_REQUEST_ID)
    })
}

fn deny_text(reason: &str) -> String {
    format!("Denied by Atrium policy: {reason}")
}

fn mcp_config(config: &SessionConfig) -> Value {
    let mut servers = serde_json::Map::new();
    for s in &config.mcp_servers {
        let env: serde_json::Map<String, Value> = s
            .env
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        servers.insert(
            s.name.clone(),
            json!({
                "command": s.command,
                "args": s.args,
                "env": env,
            }),
        );
    }
    json!({ "mcpServers": servers })
}

/// A directory only this user can enter (the MCP config can hold credentials).
fn private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().recursive(true).mode(0o700).create(path)
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)
}

/// Builds the `updatedInput` for `AskUserQuestion`: Claude Code reads the
/// answers from an `answers` object keyed by question text, one string per
/// question (labels joined by `, ` for multi-select, or the free text).
fn answered_input(input: &Value, answers: &[QuestionAnswer]) -> Value {
    let mut updated = input.clone();
    let mut map = serde_json::Map::new();
    let questions = input
        .get("questions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for a in answers {
        let Some(qi) = a
            .question_id
            .strip_prefix('q')
            .and_then(|n| n.parse::<usize>().ok())
        else {
            continue;
        };
        let Some(q) = questions.get(qi) else {
            continue;
        };
        let text = q.get("question").and_then(Value::as_str).unwrap_or("");
        let options = q
            .get("options")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut parts: Vec<String> = a
            .selected
            .iter()
            .filter_map(|id| {
                let oi = id.strip_prefix('o')?.parse::<usize>().ok()?;
                options
                    .get(oi)?
                    .get("label")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect();
        if let Some(t) = &a.text {
            parts.push(t.clone());
        }
        map.insert(text.to_owned(), Value::String(parts.join(", ")));
    }
    updated["answers"] = Value::Object(map);
    updated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_keyed_by_question_text_with_labels_joined() {
        let input = json!({"questions":[
            {"question":"Pick some","options":[{"label":"A"},{"label":"B"},{"label":"C"}],"multiSelect":true},
            {"question":"Why?","options":[]}]});
        let answers = [
            QuestionAnswer {
                question_id: "q0".into(),
                selected: vec!["o2".into(), "o0".into()],
                text: None,
            },
            QuestionAnswer {
                question_id: "q1".into(),
                selected: vec![],
                text: Some("because".into()),
            },
            QuestionAnswer {
                question_id: "q9".into(),
                selected: vec![],
                text: Some("ignored".into()),
            },
        ];
        let out = answered_input(&input, &answers);
        assert_eq!(out["answers"]["Pick some"], "C, A");
        assert_eq!(out["answers"]["Why?"], "because");
        assert_eq!(out["answers"].as_object().unwrap().len(), 2);
        assert_eq!(out["questions"], input["questions"]);
    }
}
