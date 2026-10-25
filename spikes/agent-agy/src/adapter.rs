use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use atrium_agents::fixture::FixtureLine;
use atrium_agents::{
    AdapterError, AdapterInput, AgentAdapter, BoxFuture, COMMAND_BUFFER, EVENT_BUFFER,
    Interception, MAX_QUEUED_MESSAGES, Normalizer, PermissionPolicy, SHUTDOWN_GRACE, Session,
    SessionConfig, Transport,
};
use atrium_protocol::agent::{
    AgentEvent, AgentEventEnvelope, Provider, SessionCommand, ToolStatus,
};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

use crate::bridge::{
    ApprovalMode, CwdHookInstall, GateBridge, GateSetup, HookConfigError, HookConfigExpectation,
    create_private_session_dir, install_cwd_hook, write_session_hook_config,
    write_session_mcp_config,
};
use crate::normalizer::AgyNormalizer;

/// Shape of user messages written to `agy`'s stdin under `--input-format stream-json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StdinFormat {
    /// Verified on `agy` 1.4.7: `{"event":"user","message":{"content":"..."}}`
    #[default]
    EventUserMessage,
}

impl StdinFormat {
    pub fn format_message(self, text: &str) -> String {
        match self {
            Self::EventUserMessage => json!({
                "event": "user",
                "message": {
                    "content": text,
                }
            })
            .to_string(),
        }
    }
}

/// Rust [`AgentAdapter`] implementation for Google's Antigravity CLI (`agy`).
#[derive(Debug, Clone)]
pub struct AgyAdapter {
    pub helper_bin: Option<PathBuf>,
    pub session_base_dir: PathBuf,
    pub approval_mode: ApprovalMode,
    pub stdin_format: StdinFormat,
    pub hook_timeout_secs: u64,
    pub shutdown_grace: Duration,
    pub use_default_cli_args: bool,
    pub pass_add_dir: bool,
    /// Install the session's hook config as `<cwd>/<agents dir>/hooks.json`, the file `agy`
    /// reads. If a hook config is already there the adapter **refuses to start** (`agy` would
    /// read that one and run tools ungated). Set to `false` only if the caller installs the
    /// gate hook itself; the runtime coverage check below is then the only safety net.
    pub install_cwd_hook_if_absent: bool,
    /// Abort the session (fatal `Error`, child killed, every later hook denied) when a tool
    /// finishes that no `allow` hook reply covers, or when a hook config stops validating.
    /// This is the fail-open tripwire: `agy` ignores an unreadable or malformed hook config.
    pub enforce_gate_coverage: bool,
    pub extra_args: Vec<String>,
    pub recorder: Option<mpsc::UnboundedSender<FixtureLine>>,
}

impl Default for AgyAdapter {
    fn default() -> Self {
        Self {
            helper_bin: None,
            session_base_dir: std::env::temp_dir().join("agy"),
            approval_mode: ApprovalMode::Block,
            stdin_format: StdinFormat::EventUserMessage,
            hook_timeout_secs: 300,
            shutdown_grace: SHUTDOWN_GRACE,
            use_default_cli_args: true,
            pass_add_dir: false,
            install_cwd_hook_if_absent: true,
            enforce_gate_coverage: true,
            extra_args: Vec::new(),
            recorder: None,
        }
    }
}

impl AgyAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_recorder(mut self, tx: mpsc::UnboundedSender<FixtureLine>) -> Self {
        self.recorder = Some(tx);
        self
    }

    pub fn with_approval_mode(mut self, mode: ApprovalMode) -> Self {
        self.approval_mode = mode;
        self
    }

    pub fn with_shutdown_grace(mut self, grace: Duration) -> Self {
        self.shutdown_grace = grace;
        self
    }

    /// Resolves the path to the `spike-agy-hook` helper binary.
    pub fn resolve_helper_bin(&self) -> PathBuf {
        if let Some(p) = &self.helper_bin {
            return p.clone();
        }
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            let direct = dir.join("spike-agy-hook");
            if direct.exists() {
                return direct;
            }
            if let Some(parent) = dir.parent() {
                let parent_bin = parent.join("spike-agy-hook");
                if parent_bin.exists() {
                    return parent_bin;
                }
            }
        }
        PathBuf::from("spike-agy-hook")
    }

    /// Computes the CLI arguments passed to `agy` for a given [`SessionConfig`].
    pub fn build_cli_args(&self, config: &SessionConfig, session_dir: &Path) -> Vec<String> {
        let mut args = Vec::new();
        if self.use_default_cli_args {
            args.push("--print=".to_owned());
            args.push("--input-format".to_owned());
            args.push("stream-json".to_owned());
            args.push("--output-format".to_owned());
            args.push("stream-json".to_owned());
            args.push("--print-timeout".to_owned());
            args.push("0s".to_owned());

            let mode = match config.permissions {
                PermissionPolicy::Ask => "accept-edits",
                PermissionPolicy::ReadOnly => "plan",
                PermissionPolicy::FullAccess => "auto",
            };
            args.push("--mode".to_owned());
            args.push(mode.to_owned());
            args.push("--dangerously-skip-permissions".to_owned());

            if let Some(model) = &config.model {
                args.push("--model".to_owned());
                args.push(model.clone());
            }
            if let Some(effort) = &config.effort {
                args.push("--effort".to_owned());
                args.push(effort.clone());
            }
            if let Some(resume) = &config.resume {
                args.push("--conversation".to_owned());
                args.push(resume.clone());
            }
            if self.pass_add_dir {
                args.push("--add-dir".to_owned());
                args.push(session_dir.display().to_string());
            }
        }
        args.extend(self.extra_args.iter().cloned());
        args
    }
}

impl AgentAdapter for AgyAdapter {
    fn provider(&self) -> Provider {
        Provider::Agy
    }

    fn interception(&self) -> Interception {
        // Under `--dangerously-skip-permissions` + a PreToolUse hook with matcher `"*"`,
        // every tool call (including Read, Edit, Execute, Search, Fetch, MCP, and subagents)
        // passes through `SessionConfig::gate` before execution.
        Interception::AllToolCalls
    }

    fn start(
        &self,
        config: SessionConfig,
        transport: Transport,
    ) -> BoxFuture<'_, Result<Session, AdapterError>> {
        Box::pin(async move {
            match transport {
                Transport::Local => {}
                _ => {
                    return Err(AdapterError::Unsupported(
                        "only Transport::Local is supported in M0".into(),
                    ));
                }
            }

            let session_dir = create_private_session_dir(&self.session_base_dir, "s")?;
            // Removes what this call created if it returns early; disarmed once the actor owns it.
            let mut guard = StartGuard {
                session_dir: session_dir.clone(),
                cwd_install: None,
                armed: true,
            };
            let socket_path = session_dir.join("g.sock");
            let helper_bin = self.resolve_helper_bin();

            let session_hooks = write_session_hook_config(
                &session_dir,
                &helper_bin,
                &socket_path,
                self.hook_timeout_secs,
            )?;

            if !config.mcp_servers.is_empty() {
                write_session_mcp_config(&session_dir, &config.mcp_servers)?;
            }

            let mut cwd_hooks = None;
            if self.install_cwd_hook_if_absent {
                let install =
                    install_cwd_hook(&config.cwd, &session_hooks).map_err(gate_config_error)?;
                cwd_hooks = Some(install.hooks_path.clone());
                guard.cwd_install = Some(install);
            }

            let gate_setup = GateSetup {
                session_dir: session_dir.clone(),
                cwd_hooks,
                expectation: HookConfigExpectation {
                    helper_bin: helper_bin.clone(),
                    socket_path: socket_path.clone(),
                    timeout_secs: self.hook_timeout_secs,
                },
            };

            let (bridge_inputs_tx, bridge_inputs_rx) = mpsc::unbounded_channel();
            let initial_session_id = config
                .resume
                .clone()
                .unwrap_or_else(|| "pending-session".to_owned());
            let bridge = GateBridge::start(
                socket_path.clone(),
                initial_session_id,
                config.gate.clone(),
                self.approval_mode,
                bridge_inputs_tx,
            )?;

            // `agy` fails OPEN on an unreadable or malformed hook config (it ignores it and runs
            // tools under --dangerously-skip-permissions). Read everything back right before
            // spawning and refuse to start unless it is exactly what `agy` must see.
            gate_setup.verify().map_err(gate_config_error)?;

            let program = config
                .program
                .clone()
                .unwrap_or_else(|| PathBuf::from("agy"));
            let cli_args = self.build_cli_args(&config, &session_dir);

            let mut cmd = Command::new(&program);
            cmd.args(&cli_args)
                .current_dir(&config.cwd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .env("ATRIUM_AGY_SESSION_DIR", &session_dir)
                .env("ATRIUM_AGY_GATE_SOCKET", &socket_path);

            for (k, v) in &config.env {
                cmd.env(k, v);
            }

            let child = cmd.spawn().map_err(AdapterError::Spawn)?;
            guard.armed = false;
            let created_cwd_hook = guard.cwd_install.take();

            let (events_tx, events_rx) = mpsc::channel(EVENT_BUFFER);
            let (commands_tx, commands_rx) = mpsc::channel(COMMAND_BUFFER);

            let state = ActorState {
                bridge,
                normalizer: AgyNormalizer::new(),
                stdin_format: self.stdin_format,
                shutdown_grace: self.shutdown_grace,
                session_dir,
                cwd_install: created_cwd_hook,
                gate_setup,
                enforce_gate_coverage: self.enforce_gate_coverage,
                allowed_hooks: 0,
                executed_tools: 0,
                open_tool_names: HashMap::new(),
                abort_reason: None,
                events_tx,
                recorder: self.recorder.clone(),
                started_at: Instant::now(),
                seq: 0,
                in_turn: false,
                queued_messages: VecDeque::new(),
                pending_out: VecDeque::new(),
            };

            tokio::spawn(run_session_actor(
                state,
                child,
                commands_rx,
                bridge_inputs_rx,
            ));

            Ok(Session {
                events: events_rx,
                commands: commands_tx,
            })
        })
    }
}

fn gate_config_error(e: HookConfigError) -> AdapterError {
    AdapterError::Handshake(format!(
        "refusing to start agy, gate hook config check failed: {e}"
    ))
}

/// Cleans up a half-started session on every early return from `start`.
struct StartGuard {
    session_dir: PathBuf,
    cwd_install: Option<CwdHookInstall>,
    armed: bool,
}

impl Drop for StartGuard {
    fn drop(&mut self) {
        if self.armed {
            if let Some(install) = &self.cwd_install {
                install.remove();
            }
            let _ = fs::remove_dir_all(&self.session_dir);
        }
    }
}

struct ActorState {
    bridge: GateBridge,
    normalizer: AgyNormalizer,
    stdin_format: StdinFormat,
    shutdown_grace: Duration,
    session_dir: PathBuf,
    cwd_install: Option<CwdHookInstall>,
    gate_setup: GateSetup,
    enforce_gate_coverage: bool,
    /// Hook requests answered `allow` so far.
    allowed_hooks: u64,
    /// Tool calls that finished `Succeeded` or `Failed` (that is, that actually ran).
    executed_tools: u64,
    /// `call_id` -> tool name of calls that started and have not finished.
    open_tool_names: HashMap<String, String>,
    /// Set when a gate bypass is suspected; the actor stops and kills the child at once.
    abort_reason: Option<String>,
    events_tx: mpsc::Sender<AgentEventEnvelope>,
    recorder: Option<mpsc::UnboundedSender<FixtureLine>>,
    started_at: Instant,
    seq: u64,
    in_turn: bool,
    queued_messages: VecDeque<String>,
    pending_out: VecDeque<AgentEventEnvelope>,
}

async fn run_session_actor(
    mut state: ActorState,
    mut child: Child,
    mut commands_rx: mpsc::Receiver<SessionCommand>,
    mut bridge_inputs_rx: mpsc::UnboundedReceiver<AdapterInput>,
) {
    let mut child_stdin = child.stdin.take();
    let child_stdout = child.stdout.take().expect("stdout piped");
    let child_stderr = child.stderr.take().expect("stderr piped");

    let mut stdout_lines = BufReader::new(child_stdout).lines();
    let mut stderr_lines = BufReader::new(child_stderr).lines();
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut commands_closed = false;
    let mut exited = false;

    loop {
        if state.abort_reason.is_some() {
            break;
        }
        let has_pending_out = !state.pending_out.is_empty() && !state.events_tx.is_closed();
        let can_read_pipes = state.pending_out.len() < EVENT_BUFFER * 2;
        let events_tx_closed = state.events_tx.clone();
        let events_tx_reserve = state.events_tx.clone();

        tokio::select! {
            biased;

            // 1. Commands always have top priority and are never blocked by a full `events_tx`.
            cmd_opt = commands_rx.recv(), if !commands_closed => {
                match cmd_opt {
                    Some(SessionCommand::Shutdown) => {
                        state.record_and_normalize(AdapterInput::Command {
                            command: SessionCommand::Shutdown,
                        });
                        break;
                    }
                    Some(SessionCommand::Interrupt) => {
                        state.record_and_normalize(AdapterInput::Command {
                            command: SessionCommand::Interrupt,
                        });
                        state.bridge.cancel_all();
                        state.queued_messages.clear();
                        if let Some(pid) = child.id() {
                            send_unix_signal(pid, "-INT");
                        }
                    }
                    Some(SessionCommand::ResolveApproval { request_id, decision }) => {
                        let cmd = SessionCommand::ResolveApproval {
                            request_id: request_id.clone(),
                            decision: decision.clone(),
                        };
                        state.record_and_normalize(AdapterInput::Command { command: cmd });
                        state.bridge.resolve_approval(&request_id, &decision);
                    }
                    Some(SessionCommand::AnswerQuestion { request_id, answers }) => {
                        let cmd = SessionCommand::AnswerQuestion {
                            request_id: request_id.clone(),
                            answers: answers.clone(),
                        };
                        state.record_and_normalize(AdapterInput::Command { command: cmd });
                        state.bridge.answer_question(&request_id, &answers);
                    }
                    Some(SessionCommand::SendUserMessage { text }) => {
                        state.record_and_normalize(AdapterInput::Command {
                            command: SessionCommand::SendUserMessage { text: text.clone() },
                        });
                        if state.in_turn {
                            if state.queued_messages.len() >= MAX_QUEUED_MESSAGES {
                                state.push_event(AgentEvent::Error {
                                    message: format!(
                                        "user message queue full (max {MAX_QUEUED_MESSAGES} queued messages)"
                                    ),
                                    fatal: false,
                                });
                            } else {
                                state.queued_messages.push_back(text);
                            }
                        } else {
                            state.write_user_message(&mut child_stdin, &text).await;
                        }
                    }
                    None => {
                        commands_closed = true;
                        if state.events_tx.is_closed() {
                            break;
                        }
                    }
                }
            }

            // 2. If `commands` was dropped and `events` also closes, shut down immediately.
            () = events_tx_closed.closed(), if commands_closed => {
                break;
            }

            // 3. Drain pending outbound events into `events_tx` whenever capacity is available.
            Ok(permit) = events_tx_reserve.reserve_owned(), if has_pending_out => {
                if let Some(env) = state.pending_out.pop_front() {
                    permit.send(env);
                }
            }

            // 4. Process gate bridge inputs (HookRequest, Gate, HookResponse).
            Some(bridge_input) = bridge_inputs_rx.recv() => {
                state.record_and_normalize(bridge_input);
            }

            // 5. Read child stdout when outbound buffer isn't over-saturated.
            res = stdout_lines.next_line(), if !stdout_done && can_read_pipes => {
                match res {
                    Ok(Some(line)) => {
                        // Hook traffic that happened before this line must be processed
                        // first, so the coverage check sees hook replies before the tool
                        // step they cover finishes.
                        while let Ok(inp) = bridge_inputs_rx.try_recv() {
                            state.record_and_normalize(inp);
                        }
                        state.record_and_normalize(AdapterInput::Stdout { line });
                        if !state.in_turn
                            && let Some(next_msg) = state.queued_messages.pop_front()
                        {
                            state.write_user_message(&mut child_stdin, &next_msg).await;
                        }
                    }
                    Ok(None) | Err(_) => {
                        stdout_done = true;
                    }
                }
            }

            // 6. Read child stderr when outbound buffer isn't over-saturated.
            res = stderr_lines.next_line(), if !stderr_done && can_read_pipes => {
                match res {
                    Ok(Some(line)) => {
                        state.record_and_normalize(AdapterInput::Stderr { line });
                    }
                    Ok(None) | Err(_) => {
                        stderr_done = true;
                    }
                }
            }

            // 7. Once stdout and stderr reach EOF, wait for process exit.
            status_res = child.wait(), if stdout_done && stderr_done => {
                let code = status_res.ok().and_then(|s| s.code());
                exited = true;
                while let Ok(inp) = bridge_inputs_rx.try_recv() {
                    state.record_and_normalize(inp);
                }
                state.record_and_normalize(AdapterInput::Exit { code });
                break;
            }
        }
    }

    if !exited {
        state.bridge.cancel_all();
        drop(child_stdin);

        let code = if let Ok(Some(status)) = child.try_wait() {
            status.code()
        } else if state.abort_reason.is_some() {
            // Gate bypass suspected: no grace period, nothing more may run.
            let _ = child.kill().await;
            child.wait().await.ok().and_then(|s| s.code())
        } else {
            if let Some(pid) = child.id() {
                send_unix_signal(pid, "-TERM");
            }
            match tokio::time::timeout(state.shutdown_grace, child.wait()).await {
                Ok(Ok(status)) => status.code(),
                _ => {
                    let _ = child.kill().await;
                    child.wait().await.ok().and_then(|s| s.code())
                }
            }
        };

        while let Ok(inp) = bridge_inputs_rx.try_recv() {
            state.record_and_normalize(inp);
        }
        state.record_and_normalize(AdapterInput::Exit { code });
    }

    // Remove the session dir and the cwd hook config before the final flush: a consumer that
    // sees `SessionEnded` may start the next session in the same working directory at once.
    state.cleanup();
    state.try_flush_pending();
    if !state.pending_out.is_empty() && !state.events_tx.is_closed() {
        let flush_deadline = Duration::from_millis(50);
        let _ = tokio::time::timeout(flush_deadline, async {
            while let Some(env) = state.pending_out.pop_front() {
                if state.events_tx.send(env).await.is_err() {
                    break;
                }
            }
        })
        .await;
    }
}

impl ActorState {
    async fn write_user_message(&mut self, child_stdin: &mut Option<ChildStdin>, text: &str) {
        let line = self.stdin_format.format_message(text);
        self.record_and_normalize(AdapterInput::Stdin { line: line.clone() });
        if let Some(stdin) = child_stdin.as_mut() {
            let mut payload = line;
            payload.push('\n');
            if stdin.write_all(payload.as_bytes()).await.is_err() {
                *child_stdin = None;
            } else {
                let _ = stdin.flush().await;
            }
        }
    }

    fn record_and_normalize(&mut self, input: AdapterInput) {
        if let AdapterInput::HookResponse { payload, .. } = &input
            && payload.get("decision").and_then(|d| d.as_str()) == Some("allow")
        {
            self.allowed_hooks += 1;
        }
        if let Some(rec) = &self.recorder {
            let ms = u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
            let _ = rec.send(FixtureLine {
                ms,
                input: input.clone(),
            });
        }
        let events = self.normalizer.on_input(&input);
        let mut violation: Option<String> = None;
        for ev in events {
            match &ev {
                AgentEvent::SessionStarted {
                    native_session_id, ..
                } => {
                    self.bridge.set_session_id(native_session_id.clone());
                }
                AgentEvent::TurnStarted { .. } => {
                    self.in_turn = true;
                }
                AgentEvent::TurnCompleted { .. } => {
                    self.in_turn = false;
                    self.bridge.set_call_id_hint(None);
                }
                AgentEvent::ToolCallStarted { call_id, name, .. } => {
                    self.bridge.set_call_id_hint(Some(call_id.clone()));
                    self.open_tool_names.insert(call_id.clone(), name.clone());
                    // A tool is about to run: the hook config must still be exactly what we
                    // wrote (a tool, or anything else, may have changed it since the last check).
                    if self.enforce_gate_coverage
                        && violation.is_none()
                        && let Err(e) = self.gate_setup.verify()
                    {
                        violation = Some(format!("hook config no longer valid: {e}"));
                    }
                }
                AgentEvent::ToolCallFinished {
                    call_id, status, ..
                } => {
                    self.bridge.set_call_id_hint(None);
                    let tool = self
                        .open_tool_names
                        .remove(call_id)
                        .unwrap_or_else(|| "unknown".to_owned());
                    if matches!(status, ToolStatus::Succeeded | ToolStatus::Failed) {
                        self.executed_tools += 1;
                        if self.enforce_gate_coverage
                            && violation.is_none()
                            && self.executed_tools > self.allowed_hooks
                        {
                            violation = Some(format!(
                                "tool '{tool}' ({call_id}) finished {status:?} without a \
                                 preceding allowed hook request ({} tools ran, {} hook \
                                 requests were allowed)",
                                self.executed_tools, self.allowed_hooks
                            ));
                        }
                    }
                }
                _ => {}
            }
            self.push_event(ev);
        }
        if let Some(reason) = violation {
            self.abort_session(&reason);
        }
    }

    /// Fail-open tripwire: deny every later hook request, tell the consumer, and make the actor
    /// kill the child immediately.
    fn abort_session(&mut self, reason: &str) {
        if self.abort_reason.is_some() {
            return;
        }
        self.abort_reason = Some(reason.to_owned());
        self.bridge.lock_down();
        self.push_event(AgentEvent::Error {
            message: format!("gate bypass suspected, session aborted: {reason}"),
            fatal: true,
        });
    }

    fn push_event(&mut self, event: AgentEvent) {
        let env = AgentEventEnvelope {
            seq: self.seq,
            at_ms: now_epoch_ms(),
            event,
        };
        self.seq += 1;
        self.pending_out.push_back(env);
        self.try_flush_pending();
    }

    fn try_flush_pending(&mut self) {
        while let Some(env) = self.pending_out.pop_front() {
            match self.events_tx.try_send(env) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(returned)) => {
                    self.pending_out.push_front(returned);
                    break;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    self.pending_out.clear();
                    break;
                }
            }
        }
    }

    fn cleanup(&self) {
        if let Some(install) = &self.cwd_install {
            install.remove();
        }
        let _ = fs::remove_dir_all(&self.session_dir);
    }
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn send_unix_signal(pid: u32, sig_arg: &str) {
    let _ = std::process::Command::new("kill")
        .args([sig_arg, &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}
