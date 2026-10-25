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
//! - `Ask` → `permissionDecision: ask`, which makes Claude Code send a
//!   `can_use_tool` request for the same `tool_use_id`; that becomes
//!   [`AgentEvent::ApprovalRequested`] and waits for the client.
//!
//! Anything that goes wrong on our side (gate panics, times out, malformed
//! request) answers `deny`. Claude Code fails **open** on an error response
//! or an invalid decision, so the adapter must always send an explicit one;
//! it fails closed only on its own hook timeout.
//!
//! `can_use_tool` requests that were not preceded by a hook `ask` go through
//! the gate again (belt and braces; this is also how `AskUserQuestion` and
//! other interactive tools arrive).
//!
//! # Interrupts
//!
//! `SessionCommand::Interrupt` sends the `interrupt` control request. Claude
//! Code ends the turn with a `result` (`error_during_execution`) and stays
//! alive for the next message. If it neither answers nor exits within
//! [`SHUTDOWN_GRACE`], the adapter kills it, reaps it and resumes the session
//! (`--resume <id>`) with the next message.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use atrium_agents::fixture::FixtureLine;
use atrium_agents::{
    AdapterError, AdapterInput, AgentAdapter, BoxFuture, COMMAND_BUFFER, EVENT_BUFFER,
    GateDecision, Interception, MAX_QUEUED_MESSAGES, Normalizer, PermissionPolicy, SHUTDOWN_GRACE,
    STARTUP_TIMEOUT, Session, SessionConfig, ToolGate, Transport,
};
use atrium_protocol::agent::{
    AgentEvent, AgentEventEnvelope, ApprovalDecision, Provider, QuestionAnswer, SessionCommand,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, JoinHandle, JoinSet};
use tokio::time::{Instant as TokioInstant, sleep_until, timeout};

use crate::normalizer::{ClaudeNormalizer, tool_check};

const INIT_REQUEST_ID: &str = "atrium-init";
const HOOK_CALLBACK_ID: &str = "atrium-pre-tool-use";
const ASK_USER_QUESTION: &str = "AskUserQuestion";
/// Events held back when the client is not reading, before the adapter stops
/// reading the provider (the pipe then applies backpressure to Claude Code).
const PENDING_LIMIT: usize = 4096;
/// How long the closing events get to reach a client that is not reading.
const FLUSH_BUDGET: Duration = Duration::from_millis(300);

/// Knobs that are not part of the shared [`SessionConfig`].
#[derive(Clone)]
pub struct ClaudeOptions {
    /// `--setting-sources` value. `None` leaves Claude Code's default (user,
    /// project and local settings). The recorder uses `project,local` so
    /// fixtures don't carry the recorder's personal configuration.
    pub setting_sources: Option<String>,
    /// Extra CLI arguments, appended last.
    pub extra_args: Vec<String>,
    /// How long the gate has to answer a hook call before it is denied.
    pub gate_deadline: Duration,
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

impl AgentAdapter for ClaudeAdapter {
    fn provider(&self) -> Provider {
        Provider::Claude
    }

    /// `AllToolCalls`: the `PreToolUse` hook fires for built-in, MCP and
    /// subagent tools in every permission mode (see the enforcement matrix).
    fn interception(&self) -> Interception {
        Interception::AllToolCalls
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
            let (events_tx, events_rx) = mpsc::channel(EVENT_BUFFER);
            let (commands_tx, commands_rx) = mpsc::channel(COMMAND_BUFFER);
            let mut actor = Actor::new(config, options, events_tx, commands_rx)?;
            actor.spawn_proc(true).await?;
            tokio::spawn(actor.run());
            Ok(Session {
                events: events_rx,
                commands: commands_tx,
            })
        })
    }
}

// ---------------------------------------------------------------------------

enum Out {
    Stdout(String),
    Stderr(String),
}

enum Ev {
    Command(Option<SessionCommand>),
    Internal(Internal),
    InterruptDeadline,
    Flushed,
    ConsumerGone,
    Output(Option<Out>),
    ChildExited,
}

enum Internal {
    Gate {
        reply: Reply,
        check: atrium_agents::ToolCheck,
        decision: GateDecision,
    },
}

/// What to do with a gate decision.
enum Reply {
    /// A `PreToolUse` hook callback.
    Hook { request_id: String },
    /// A `can_use_tool` request the CLI asked directly.
    CanUse { request_id: String, input: Value },
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
    child: Child,
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

    events_tx: mpsc::Sender<AgentEventEnvelope>,
    commands_rx: mpsc::Receiver<SessionCommand>,
    internal_tx: mpsc::UnboundedSender<Internal>,
    internal_rx: mpsc::UnboundedReceiver<Internal>,
    pending: VecDeque<AgentEventEnvelope>,

    proc: Option<Proc>,
    out_rx: Option<mpsc::Receiver<Out>>,
    /// The process left after an interrupt; the next message resumes it.
    dormant: bool,
    interrupt_deadline: Option<TokioInstant>,
    queue: VecDeque<String>,
    shutting_down: bool,
    /// In-flight gate checks; aborted when the session ends.
    gates: JoinSet<()>,

    /// `tool_use_id`s the hook answered `ask` for.
    hook_asked: HashSet<String>,
    approvals: HashMap<String, Value>,
    questions: HashMap<String, Value>,

    _mcp_dir: TempDir,
    mcp_path: PathBuf,
}

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

impl Actor {
    fn new(
        config: SessionConfig,
        options: ClaudeOptions,
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
        fs::create_dir_all(&dir)?;
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
            events_tx,
            commands_rx,
            internal_tx,
            internal_rx,
            pending: VecDeque::new(),
            proc: None,
            out_rx: None,
            dormant: false,
            interrupt_deadline: None,
            queue: VecDeque::new(),
            shutting_down: false,
            gates: JoinSet::new(),
            hook_asked: HashSet::new(),
            approvals: HashMap::new(),
            questions: HashMap::new(),
            _mcp_dir: mcp_dir,
            mcp_path,
        })
    }

    // ---- process ---------------------------------------------------------

    fn args(&self, resume: Option<&str>) -> Vec<String> {
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
        if let Some(r) = resume {
            a.extend(["--resume".into(), r.to_owned()]);
        }
        a.extend(self.options.extra_args.iter().cloned());
        a
    }

    /// Starts the process and completes the `initialize` handshake. With
    /// `first` the configured `resume` id applies; later respawns resume the
    /// session this normalizer has seen.
    async fn spawn_proc(&mut self, first: bool) -> Result<(), AdapterError> {
        let resume = if first {
            self.config.resume.clone()
        } else {
            self.norm.native_session_id().map(str::to_owned)
        };
        let program = self
            .config
            .program
            .clone()
            .unwrap_or_else(|| PathBuf::from("claude"));
        let mut cmd = Command::new(program);
        cmd.args(self.args(resume.as_deref()))
            .current_dir(&self.config.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Only this session's MCP servers; no claude.ai connectors.
        cmd.env("ENABLE_CLAUDEAI_MCP_SERVERS", "false");
        cmd.envs(self.config.env.iter().map(|(k, v)| (k, v)));
        let mut child = cmd.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AdapterError::Handshake("no stdin pipe".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AdapterError::Handshake("no stdout pipe".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| AdapterError::Handshake("no stderr pipe".into()))?;

        let (out_tx, out_rx) = mpsc::channel::<Out>(256);
        let mut readers = Vec::new();
        {
            let tx = out_tx.clone();
            readers.push(tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if tx.send(Out::Stdout(line)).await.is_err() {
                        break;
                    }
                }
            }));
            readers.push(tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if out_tx.send(Out::Stderr(line)).await.is_err() {
                        break;
                    }
                }
            }));
        }
        let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<String>();
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
            stdin_tx: Some(stdin_tx),
            writer,
            readers,
        });
        self.out_rx = Some(out_rx);
        self.dormant = false;

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
        self.handshake().await
    }

    async fn handshake(&mut self) -> Result<(), AdapterError> {
        let deadline = TokioInstant::now() + STARTUP_TIMEOUT;
        let mut stderr_tail: VecDeque<String> = VecDeque::new();
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
                    if stderr_tail.len() >= 3 {
                        stderr_tail.pop_front();
                    }
                    stderr_tail.push_back(line.clone());
                    self.feed(AdapterInput::Stderr { line });
                }
                Ok(None) => {
                    let code = self.reap(Duration::from_secs(1)).await;
                    self.discard_pending();
                    let tail: Vec<String> = stderr_tail.into_iter().collect();
                    return Err(AdapterError::Handshake(format!(
                        "claude exited before answering initialize (code {code:?}): {}",
                        tail.join(" | ")
                    )));
                }
                Err(_) => {
                    self.kill_proc().await;
                    return Err(AdapterError::Handshake(format!(
                        "claude did not answer initialize within {STARTUP_TIMEOUT:?}"
                    )));
                }
            }
        }
    }

    fn discard_pending(&mut self) {
        // A failed `start` returns an error instead of events.
        self.pending.clear();
    }

    async fn kill_proc(&mut self) -> Option<i32> {
        let mut code = None;
        if let Some(mut p) = self.proc.take() {
            let _ = p.child.start_kill();
            if let Ok(Ok(status)) = timeout(Duration::from_secs(2), p.child.wait()).await {
                code = status.code();
            }
            p.writer.abort();
            for r in p.readers {
                r.abort();
            }
        }
        code
    }

    /// Waits up to `grace` for the child to exit, then kills it. Returns its
    /// exit code.
    async fn reap(&mut self, grace: Duration) -> Option<i32> {
        let Some(mut p) = self.proc.take() else {
            return None;
        };
        p.stdin_tx = None; // EOF on stdin
        let code = match timeout(grace, p.child.wait()).await {
            Ok(Ok(status)) => status.code(),
            _ => {
                let _ = p.child.start_kill();
                p.child.wait().await.ok().and_then(|s| s.code())
            }
        };
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
        let at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.pending.push_back(AgentEventEnvelope {
            seq: self.seq,
            at_ms,
            event,
        });
        self.seq += 1;
    }

    fn write_value(&mut self, v: &Value) {
        let line = v.to_string();
        self.feed(AdapterInput::Stdin { line: line.clone() });
        if let Some(tx) = self.proc.as_ref().and_then(|p| p.stdin_tx.as_ref()) {
            let _ = tx.send(line);
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

    // ---- main loop -------------------------------------------------------------

    async fn run(mut self) {
        let mut consumer_gone = false;
        loop {
            if self.shutting_down {
                break;
            }
            if self.norm.pre_session_failed() {
                // Nothing useful will come out of this process.
                self.finish(false).await;
                break;
            }
            let flush = !self.pending.is_empty() && !consumer_gone;
            let can_read = self.pending.len() < PENDING_LIMIT && self.out_rx.is_some();
            let deadline = self.interrupt_deadline;
            let ev = tokio::select! {
                biased;
                cmd = self.commands_rx.recv() => Ev::Command(cmd),
                Some(msg) = self.internal_rx.recv() => Ev::Internal(msg),
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
                _ = wait_child(&mut self.proc) => Ev::ChildExited,
            };
            match ev {
                Ev::Command(Some(c)) => self.on_command(c).await,
                Ev::Command(None) => {
                    self.finish(true).await;
                    break;
                }
                Ev::Internal(msg) => self.on_internal(msg),
                Ev::InterruptDeadline => {
                    // The child ignored the interrupt.
                    self.interrupt_deadline = None;
                    self.kill_after_interrupt().await;
                }
                Ev::Flushed => {}
                Ev::ConsumerGone => {
                    consumer_gone = true;
                    self.pending.clear();
                }
                Ev::Output(Some(Out::Stdout(line))) => self.on_stdout(line, true),
                Ev::Output(Some(Out::Stderr(line))) => self.feed(AdapterInput::Stderr { line }),
                Ev::Output(None) | Ev::ChildExited => self.handle_exit().await,
            }
            if consumer_gone {
                self.pending.clear();
            }
        }
        self.cleanup().await;
    }

    async fn cleanup(&mut self) {
        // Closing events: give a reading client a moment, never block on an
        // idle one.
        let deadline = TokioInstant::now() + FLUSH_BUDGET;
        while let Some(ev) = self.pending.pop_front() {
            if tokio::time::timeout_at(deadline, self.events_tx.send(ev))
                .await
                .is_err()
            {
                break;
            }
        }
        // `events_tx` and the temp dir drop with `self`.
    }

    /// Ends the session: stop the provider (grace, then kill, reap), feed the
    /// closing inputs, and let `run` flush the events. `requested` is false
    /// for failures the adapter itself detected.
    async fn finish(&mut self, requested: bool) {
        self.shutting_down = true;
        self.gates.abort_all();
        if requested {
            self.feed(AdapterInput::Command {
                command: SessionCommand::Shutdown,
            });
        }
        self.interrupt_deadline = None;
        let mut code = None;
        if let Some(mut p) = self.proc.take() {
            // Keep draining the provider's output while it winds down, so a
            // full pipe cannot hold it up.
            p.stdin_tx = None;
            let deadline = TokioInstant::now() + SHUTDOWN_GRACE;
            let mut exited = false;
            let mut rx = self.out_rx.take();
            loop {
                tokio::select! {
                    status = p.child.wait() => {
                        code = status.ok().and_then(|s| s.code());
                        exited = true;
                        break;
                    }
                    _ = sleep_until(deadline) => break,
                    out = recv_out(&mut rx) => match out {
                        Some(Out::Stdout(line)) => {
                            if self.pending.len() < PENDING_LIMIT {
                                self.on_stdout(line, false);
                            }
                        }
                        Some(Out::Stderr(line)) => self.feed(AdapterInput::Stderr { line }),
                        None => {
                            // Both pipes closed: wait for the exit status.
                            rx = None;
                        }
                    },
                }
            }
            if !exited {
                let _ = p.child.start_kill();
                code = p.child.wait().await.ok().and_then(|s| s.code());
            }
            p.writer.abort();
            // Anything still buffered.
            if let Some(mut rx) = rx {
                let end = TokioInstant::now() + Duration::from_millis(200);
                while let Ok(Some(out)) = tokio::time::timeout_at(end, rx.recv()).await {
                    match out {
                        Out::Stdout(line) if self.pending.len() < PENDING_LIMIT => {
                            self.on_stdout(line, false)
                        }
                        Out::Stderr(line) => self.feed(AdapterInput::Stderr { line }),
                        Out::Stdout(_) => {}
                    }
                }
            }
            for r in p.readers {
                r.abort();
            }
        }
        self.out_rx = None;
        self.clear_requests();
        self.feed(AdapterInput::Exit { code });
    }

    fn clear_requests(&mut self) {
        self.hook_asked.clear();
        self.approvals.clear();
        self.questions.clear();
    }

    /// The process is gone (pipes closed or `wait` returned): collect what it
    /// wrote last, reap it, and feed the exit.
    async fn handle_exit(&mut self) {
        self.interrupt_deadline = None;
        if let Some(mut rx) = self.out_rx.take() {
            let end = TokioInstant::now() + Duration::from_millis(300);
            while let Ok(Some(out)) = tokio::time::timeout_at(end, rx.recv()).await {
                match out {
                    Out::Stdout(line) if self.pending.len() < PENDING_LIMIT * 2 => {
                        self.on_stdout(line, false);
                    }
                    Out::Stderr(line) => self.feed(AdapterInput::Stderr { line }),
                    Out::Stdout(_) => {}
                }
            }
        }
        let code = self.reap(Duration::from_secs(2)).await;
        self.clear_requests();
        self.feed(AdapterInput::Exit { code });
        self.after_exit().await;
    }

    async fn after_exit(&mut self) {
        if self.norm.has_ended() {
            self.shutting_down = true;
        } else {
            // Left after an interrupt: the session lives on, dormant.
            self.dormant = true;
            self.dispatch_next().await;
        }
    }

    async fn kill_after_interrupt(&mut self) {
        let code = self.kill_proc().await;
        self.out_rx = None;
        self.clear_requests();
        self.feed(AdapterInput::Exit { code });
        self.after_exit().await;
    }

    // ---- provider output ---------------------------------------------------------

    /// `active` is false while the provider is winding down: its requests
    /// are recorded but not answered.
    fn on_stdout(&mut self, line: String, active: bool) {
        let parsed: Option<Value> = serde_json::from_str(&line).ok();
        self.feed(AdapterInput::Stdout { line });
        let Some(v) = parsed.filter(|_| active) else {
            return;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("control_request") => self.on_control_request(&v),
            Some("result") if !self.norm.turn_in_flight() => {
                self.interrupt_deadline = None;
                self.pump_queue();
            }
            _ => {}
        }
    }

    fn on_control_request(&mut self, v: &Value) {
        let Some(request_id) = v
            .get("request_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return;
        };
        let req = v.get("request").cloned().unwrap_or(Value::Null);
        match req.get("subtype").and_then(Value::as_str) {
            Some("hook_callback") => self.on_hook(request_id, &req),
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

    fn on_hook(&mut self, request_id: String, req: &Value) {
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
        let call_id = input.get("tool_use_id").and_then(Value::as_str);
        let check = tool_check(call_id, name, &tool_input);
        self.spawn_gate(Reply::Hook { request_id }, check);
    }

    fn on_can_use(&mut self, request_id: String, req: &Value) {
        let name = req.get("tool_name").and_then(Value::as_str).unwrap_or("");
        let input = req.get("input").cloned().unwrap_or(Value::Null);
        let tool_use_id = req
            .get("tool_use_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if name == ASK_USER_QUESTION {
            // The gate already saw this call at the hook; the question is the
            // interaction, so there is nothing to approve.
            self.questions.insert(request_id, input);
            return;
        }
        if tool_use_id
            .as_ref()
            .is_some_and(|t| self.hook_asked.contains(t))
        {
            self.approvals.insert(request_id, input);
            return;
        }
        let check = tool_check(tool_use_id.as_deref(), name, &input);
        self.spawn_gate(Reply::CanUse { request_id, input }, check);
    }

    fn spawn_gate(&mut self, reply: Reply, check: atrium_agents::ToolCheck) {
        let gate = self.gate.clone();
        let tx = self.internal_tx.clone();
        let deadline = self.options.gate_deadline;
        self.gates.spawn(async move {
            let check_for_gate = check.clone();
            // The check runs in its own task so a panic is contained, and is
            // aborted if this task is (session end) or the deadline passes.
            let inner = tokio::spawn(async move { gate.check(&check_for_gate).await });
            let _guard = AbortOnDrop(inner.abort_handle());
            let decision = match timeout(deadline, inner).await {
                Ok(Ok(d)) => d,
                Ok(Err(_)) => GateDecision::Deny {
                    reason: "policy gate failed".into(),
                },
                Err(_) => GateDecision::Deny {
                    reason: "policy gate timed out".into(),
                },
            };
            let _ = tx.send(Internal::Gate {
                reply,
                check,
                decision,
            });
        });
    }

    fn on_internal(&mut self, msg: Internal) {
        while self.gates.try_join_next().is_some() {}
        let Internal::Gate {
            reply,
            check,
            decision,
        } = msg;
        self.feed(AdapterInput::Gate {
            check: check.clone(),
            decision: decision.clone(),
        });
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
                GateDecision::Ask if check.name == ASK_USER_QUESTION => {
                    self.respond_hook(&request_id, "allow", None);
                }
                GateDecision::Ask => {
                    if let Some(id) = &check.call_id {
                        self.hook_asked.insert(id.clone());
                    }
                    self.respond_hook(&request_id, "ask", None);
                }
            },
            Reply::CanUse { request_id, input } => match decision {
                GateDecision::Allow => {
                    self.respond(
                        &request_id,
                        json!({"behavior": "allow", "updatedInput": input}),
                    );
                }
                GateDecision::Deny { reason } => {
                    self.respond(
                        &request_id,
                        json!({"behavior": "deny", "message": deny_text(&reason)}),
                    );
                }
                GateDecision::Ask => {
                    self.approvals.insert(request_id, input);
                }
            },
        }
    }

    // ---- client commands -----------------------------------------------------------------

    async fn on_command(&mut self, command: SessionCommand) {
        match command {
            SessionCommand::Shutdown => {
                self.finish(true).await;
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
                } else if self.dormant {
                    // Nothing is running.
                }
            }
            SessionCommand::SendUserMessage { text } => {
                self.feed(AdapterInput::Command {
                    command: SessionCommand::SendUserMessage { text: text.clone() },
                });
                let busy = self.norm.turn_in_flight() || !self.queue.is_empty();
                if busy {
                    if self.queue.len() >= MAX_QUEUED_MESSAGES {
                        self.push_event(AgentEvent::Error {
                            message: format!(
                                "message rejected: {MAX_QUEUED_MESSAGES} messages are already waiting"
                            ),
                            fatal: false,
                        });
                    } else {
                        self.queue.push_back(text);
                    }
                } else {
                    self.queue.push_back(text);
                    self.dispatch_next().await;
                }
            }
            SessionCommand::ResolveApproval {
                request_id,
                decision,
            } => {
                let input = self.approvals.remove(&request_id);
                self.feed(AdapterInput::Command {
                    command: SessionCommand::ResolveApproval {
                        request_id: request_id.clone(),
                        decision: decision.clone(),
                    },
                });
                if let Some(input) = input {
                    let response = match decision {
                        ApprovalDecision::Allow => {
                            json!({"behavior": "allow", "updatedInput": input})
                        }
                        ApprovalDecision::Deny { message } => json!({
                            "behavior": "deny",
                            "message": message.unwrap_or_else(|| "Denied by the user".into()),
                        }),
                    };
                    self.respond(&request_id, response);
                }
            }
            SessionCommand::AnswerQuestion {
                request_id,
                answers,
            } => {
                let input = self.questions.remove(&request_id);
                self.feed(AdapterInput::Command {
                    command: SessionCommand::AnswerQuestion {
                        request_id: request_id.clone(),
                        answers: answers.clone(),
                    },
                });
                if let Some(input) = input {
                    let updated = answered_input(&input, &answers);
                    self.respond(
                        &request_id,
                        json!({"behavior": "allow", "updatedInput": updated}),
                    );
                }
            }
        }
    }

    /// Writes the next queued message if no turn is running.
    fn pump_queue(&mut self) {
        // Called from the synchronous reader path; the async dispatch (which
        // may respawn) is done by `dispatch_next` from the loop. Here the
        // process is alive, so writing is enough.
        if self.norm.turn_in_flight() {
            return;
        }
        if let Some(text) = self.queue.pop_front() {
            self.write_user(&text);
        }
    }

    async fn dispatch_next(&mut self) {
        if self.norm.turn_in_flight() {
            return;
        }
        let Some(text) = self.queue.pop_front() else {
            return;
        };
        if self.proc.is_none() {
            if let Err(e) = self.spawn_proc(false).await {
                self.push_event(AgentEvent::Error {
                    message: format!("could not resume the session: {e}"),
                    fatal: true,
                });
                self.feed(AdapterInput::Exit { code: None });
                self.shutting_down = true;
                return;
            }
        }
        self.write_user(&text);
    }

    fn write_user(&mut self, text: &str) {
        self.write_value(&json!({
            "type": "user",
            "message": {"role": "user", "content": text},
        }));
    }
}

async fn wait_child(proc: &mut Option<Proc>) {
    match proc {
        Some(p) => {
            let _ = p.child.wait().await;
        }
        None => std::future::pending().await,
    }
}

async fn recv_out(rx: &mut Option<mpsc::Receiver<Out>>) -> Option<Out> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
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
