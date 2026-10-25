use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    ClientCapabilities, ContentBlock, CreateElicitationRequest, CreateElicitationResponse,
    CreateTerminalRequest, ElicitationAcceptAction, ElicitationAction, ElicitationCapabilities,
    ElicitationContentValue, ElicitationFormCapabilities, ElicitationMode,
    ElicitationPropertySchema, EnvVariable, FileSystemCapabilities, Implementation,
    InitializeRequest, InitializeResponse, LoadSessionRequest, McpServer as AcpMcpServer,
    McpServerStdio, NewSessionRequest, NewSessionResponse, PermissionOption, PermissionOptionKind,
    PromptRequest, ReadTextFileRequest, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionId, SessionModeState, TextContent,
    ToolKind as AcpToolKind, WriteTextFileRequest,
};
use atrium_agents::fixture::FixtureLine;
use atrium_agents::{
    AdapterError, AdapterInput, AgentAdapter, BoxFuture, COMMAND_BUFFER, EVENT_BUFFER,
    GateDecision, Interception, MAX_QUEUED_MESSAGES, Normalizer, PermissionPolicy, SHUTDOWN_GRACE,
    STARTUP_TIMEOUT, Session, SessionConfig, ToolCheck, Transport,
};
use atrium_protocol::agent::{
    AgentEvent, AgentEventEnvelope, ApprovalDecision, Provider, QuestionAnswer, SessionCommand,
    ToolKind,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, mpsc, watch};

use crate::model::{ModelVia, find_select_option, parse_model_state};
use crate::normalizer::{
    AcpNormalizer, extract_tool_name, map_tool_kind, normalize_tool_input, rpc_id_to_string,
};
use crate::opencode_config::InjectedConfig;

/// Client capability advertisement configuration for [`AcpAdapter`].
///
/// By default (`ClientCapabilityConfig::all()`), the adapter advertises
/// `fs.read_text_file`, `fs.write_text_file`, `terminal`, and `elicitation`
/// so that an ACP agent routes file reads/writes and command execution through
/// the client adapter where [`SessionConfig::gate`] inspects every call before
/// it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientCapabilityConfig {
    pub read_text_file: bool,
    pub write_text_file: bool,
    pub terminal: bool,
    pub elicitation: bool,
}

impl Default for ClientCapabilityConfig {
    fn default() -> Self {
        Self::all()
    }
}

impl ClientCapabilityConfig {
    /// Advertise all client capabilities (`fs`, `terminal`, `elicitation`).
    pub const fn all() -> Self {
        Self {
            read_text_file: true,
            write_text_file: true,
            terminal: true,
            elicitation: true,
        }
    }

    /// Do not advertise `fs` or `terminal` client capabilities (only `elicitation`).
    /// Used to compare provider-only permission requests vs client-capability interception.
    pub const fn without_fs_and_terminal() -> Self {
        Self {
            read_text_file: false,
            write_text_file: false,
            terminal: false,
            elicitation: true,
        }
    }

    fn to_acp(self) -> ClientCapabilities {
        let mut caps = ClientCapabilities::new()
            .fs(FileSystemCapabilities::new()
                .read_text_file(self.read_text_file)
                .write_text_file(self.write_text_file))
            .terminal(self.terminal);
        if self.elicitation {
            caps = caps.elicitation(
                ElicitationCapabilities::new().form(ElicitationFormCapabilities::new()),
            );
        }
        caps
    }
}

/// ACP (`agent-client-protocol`) client adapter, defaulting to `opencode acp`.
#[derive(Debug, Clone)]
pub struct AcpAdapter {
    program: PathBuf,
    args: Vec<String>,
    capabilities: ClientCapabilityConfig,
    recorder: Option<mpsc::UnboundedSender<FixtureLine>>,
    /// Spawn the agent with a per-session OpenCode config that makes it ask
    /// for every action (see [`crate::opencode_config`]). On for the default
    /// `opencode acp` command, off for [`AcpAdapter::with_command`].
    inject_opencode_config: bool,
    startup_timeout: Duration,
}

impl Default for AcpAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl AcpAdapter {
    /// Creates an `AcpAdapter` configured to run `opencode acp` with all client
    /// capabilities (`fs`, `terminal`, `elicitation`) enabled.
    pub fn new() -> Self {
        Self {
            program: PathBuf::from("opencode"),
            args: vec!["acp".to_owned()],
            capabilities: ClientCapabilityConfig::all(),
            recorder: None,
            inject_opencode_config: true,
            startup_timeout: STARTUP_TIMEOUT,
        }
    }

    /// Overrides the default agent command and arguments.
    pub fn with_command<P, I, S>(program: P, args: I) -> Self
    where
        P: Into<PathBuf>,
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            capabilities: ClientCapabilityConfig::all(),
            recorder: None,
            inject_opencode_config: false,
            startup_timeout: STARTUP_TIMEOUT,
        }
    }

    /// Turns the per-session OpenCode config injection on or off. It is on for
    /// [`AcpAdapter::new`] (`opencode acp`) and off for other commands.
    pub fn with_opencode_config(mut self, inject: bool) -> Self {
        self.inject_opencode_config = inject;
        self
    }

    /// Bounds `initialize` + session creation + model selection. Defaults to
    /// [`STARTUP_TIMEOUT`]; tests use a shorter one.
    pub fn with_startup_timeout(mut self, timeout: Duration) -> Self {
        self.startup_timeout = timeout;
        self
    }

    /// Configures which ACP client capabilities (`fs`, `terminal`, `elicitation`)
    /// are advertised during `initialize`.
    pub fn with_client_capabilities(mut self, capabilities: ClientCapabilityConfig) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Attaches a [`FixtureLine`] recorder channel that receives every
    /// [`AdapterInput`] in the exact order it crosses the adapter boundary.
    pub fn with_recorder(mut self, recorder: mpsc::UnboundedSender<FixtureLine>) -> Self {
        self.recorder = Some(recorder);
        self
    }
}

impl AgentAdapter for AcpAdapter {
    fn provider(&self) -> Provider {
        Provider::Acp
    }

    fn interception(&self) -> Interception {
        // `ProviderGated` on purpose. OpenCode does not use the client fs and
        // terminal capabilities (live finding), and with its default
        // permissions it asks about nothing. The injected `ask` rule makes it
        // send `session/request_permission` for the tool classes its docs list,
        // but that is the provider's own enforcement: not every class has been
        // proven live (MCP tools and subagent internals are open), saved
        // "always" approvals and user-defined agents can override it, and other
        // ACP agents choose for themselves. See the enforcement matrix in
        // `docs/spikes/M0-04-acp-adapter.md`.
        Interception::ProviderGated
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
                    return Err(AdapterError::Unsupported(format!(
                        "unsupported transport: {transport:?}"
                    )));
                }
            }

            let program = config
                .program
                .clone()
                .unwrap_or_else(|| self.program.clone());
            let injected = if self.inject_opencode_config {
                Some(
                    InjectedConfig::create(config.model.as_deref(), &config.mcp_servers)
                        .map_err(AdapterError::Spawn)?,
                )
            } else {
                None
            };
            let mut cmd = Command::new(&program);
            cmd.args(&self.args)
                .current_dir(&config.cwd)
                .envs(config.env.iter().cloned());
            if let Some(injected) = &injected {
                // Ours go last, so they win over anything in `config.env`.
                let lookup = |name: &str| {
                    config
                        .env
                        .iter()
                        .rev()
                        .find(|(k, _)| k == name)
                        .map(|(_, v)| v.clone())
                        .or_else(|| std::env::var(name).ok())
                };
                cmd.envs(injected.env(lookup));
            }
            cmd.stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);

            let mut child = cmd.spawn().map_err(AdapterError::Spawn)?;
            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| AdapterError::Protocol("missing child stdin".into()))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| AdapterError::Protocol("missing child stdout".into()))?;
            let stderr = child
                .stderr
                .take()
                .ok_or_else(|| AdapterError::Protocol("missing child stderr".into()))?;

            let (io_tx, mut io_rx) = mpsc::unbounded_channel::<ChildIo>();
            let stdout_tx = io_tx.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if stdout_tx.send(ChildIo::Stdout(line)).is_err() {
                        break;
                    }
                }
            });

            let stderr_tx = io_tx.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if stderr_tx.send(ChildIo::Stderr(line)).is_err() {
                        break;
                    }
                }
            });

            let started_at = Instant::now();
            let normalizer = AcpNormalizer::new().with_model(config.model.clone());

            let (events_tx, events_rx) = mpsc::channel::<AgentEventEnvelope>(EVENT_BUFFER);
            let (commands_tx, commands_rx) = mpsc::channel::<SessionCommand>(COMMAND_BUFFER);
            let (internal_ev_tx, mut internal_ev_rx) =
                mpsc::unbounded_channel::<AgentEventEnvelope>();
            let (abort_fwd_tx, mut abort_fwd_rx) = watch::channel(false);
            let (events_closed_tx, events_closed_rx) = watch::channel(false);

            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        maybe_env = internal_ev_rx.recv() => {
                            let Some(env) = maybe_env else {
                                break;
                            };
                            let is_end = matches!(env.event, AgentEvent::SessionEnded { .. });
                            match events_tx.try_send(env) {
                                Ok(()) => {
                                    if is_end {
                                        break;
                                    }
                                }
                                Err(mpsc::error::TrySendError::Closed(_)) => {
                                    let _ = events_closed_tx.send(true);
                                    break;
                                }
                                Err(mpsc::error::TrySendError::Full(env)) => {
                                    if *abort_fwd_rx.borrow() {
                                        break;
                                    }
                                    tokio::select! {
                                        res = events_tx.send(env) => {
                                            if res.is_err() || is_end {
                                                break;
                                            }
                                        }
                                        res = abort_fwd_rx.changed() => {
                                            if res.is_ok() && *abort_fwd_rx.borrow() {
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        _ = events_tx.closed() => {
                            let _ = events_closed_tx.send(true);
                            break;
                        }
                    }
                }
                if events_tx.is_closed() {
                    let _ = events_closed_tx.send(true);
                }
            });

            let mut runtime = SessionRuntime {
                config,
                capabilities: self.capabilities,
                recorder: self.recorder.clone(),
                started_at,
                normalizer,
                mcp_via_config: injected.is_some(),
                _injected_config: injected,
                next_seq: 0,
                next_rpc_id: 0,
                session_id: String::new(),
                stdin: Some(stdin),
                internal_ev_tx,
                abort_fwd_tx,
                pending_approvals: BTreeMap::new(),
                pending_questions: BTreeMap::new(),
                queued_messages: VecDeque::new(),
                active_prompt_rpc_id: None,
                terminals: BTreeMap::new(),
                next_terminal_seq: 0,
            };

            // Perform the ACP handshake (`initialize` + `session/new` or `session/load`)
            // bounded by `STARTUP_TIMEOUT`.
            let handshake_res = tokio::time::timeout(
                self.startup_timeout,
                runtime.handshake(&mut child, &mut io_rx),
            )
            .await;

            match handshake_res {
                Err(_) => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    let _ = runtime.abort_fwd_tx.send(true);
                    return Err(AdapterError::Handshake(
                        "ACP startup handshake timed out".into(),
                    ));
                }
                Ok(Err(e)) => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    let _ = runtime.abort_fwd_tx.send(true);
                    return Err(e);
                }
                Ok(Ok(HandshakeOutcome::PreSessionExit { exit_code })) => {
                    runtime.record_and_normalize(AdapterInput::Exit { code: exit_code });
                    return Ok(Session {
                        events: events_rx,
                        commands: commands_tx,
                    });
                }
                Ok(Ok(HandshakeOutcome::Established)) => {}
            }

            // Spawn the main session event loop.
            tokio::spawn(async move {
                runtime
                    .run(child, io_rx, commands_rx, events_closed_rx)
                    .await;
            });

            Ok(Session {
                events: events_rx,
                commands: commands_tx,
            })
        })
    }
}

#[derive(Debug)]
enum ChildIo {
    Stdout(String),
    Stderr(String),
}

#[derive(Debug)]
enum HandshakeOutcome {
    Established,
    PreSessionExit { exit_code: Option<i32> },
}

#[derive(Debug)]
enum PendingClientOp {
    Permission {
        rpc_id: Value,
        options: Vec<PermissionOption>,
    },
    FsRead {
        rpc_id: Value,
        resolved_path: PathBuf,
        line: Option<u32>,
        limit: Option<u32>,
    },
    FsWrite {
        rpc_id: Value,
        resolved_path: PathBuf,
        content: String,
    },
    TerminalCreate {
        rpc_id: Value,
        params: CreateTerminalRequest,
    },
}

#[derive(Debug)]
struct PendingQuestionRpc {
    rpc_id: Value,
    req: CreateElicitationRequest,
}

struct ManagedTerminal {
    child: Arc<Mutex<Child>>,
    output: Arc<Mutex<Vec<u8>>>,
    output_byte_limit: Option<usize>,
}

struct SessionRuntime {
    config: SessionConfig,
    capabilities: ClientCapabilityConfig,
    recorder: Option<mpsc::UnboundedSender<FixtureLine>>,
    started_at: Instant,
    normalizer: AcpNormalizer,
    /// Removes the generated config directory when the session ends.
    _injected_config: Option<InjectedConfig>,
    /// MCP servers travel in the injected config (with `codemode: false`), not
    /// in `session/new`.
    mcp_via_config: bool,
    next_seq: u64,
    next_rpc_id: i64,
    session_id: String,
    stdin: Option<ChildStdin>,
    internal_ev_tx: mpsc::UnboundedSender<AgentEventEnvelope>,
    abort_fwd_tx: watch::Sender<bool>,
    pending_approvals: BTreeMap<String, PendingClientOp>,
    pending_questions: BTreeMap<String, PendingQuestionRpc>,
    queued_messages: VecDeque<String>,
    active_prompt_rpc_id: Option<String>,
    terminals: BTreeMap<String, ManagedTerminal>,
    next_terminal_seq: u64,
}

impl SessionRuntime {
    fn elapsed_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }

    fn now_unix_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn alloc_rpc_id(&mut self) -> i64 {
        self.next_rpc_id += 1;
        self.next_rpc_id
    }

    fn emit_events(&mut self, events: Vec<AgentEvent>) {
        let at_ms = Self::now_unix_ms();
        for event in events {
            let seq = self.next_seq;
            self.next_seq += 1;
            let _ = self
                .internal_ev_tx
                .send(AgentEventEnvelope { seq, at_ms, event });
        }
    }

    fn record_and_normalize(&mut self, input: AdapterInput) -> Vec<AgentEvent> {
        if let Some(rec) = &self.recorder {
            let _ = rec.send(FixtureLine {
                ms: self.elapsed_ms(),
                input: input.clone(),
            });
        }
        let events = self.normalizer.on_input(&input);
        self.emit_events(events.clone());
        events
    }

    async fn send_json(&mut self, msg: &Value) -> Result<Vec<AgentEvent>, AdapterError> {
        let line = serde_json::to_string(msg)
            .map_err(|e| AdapterError::Protocol(format!("serializing JSON-RPC: {e}")))?;
        if let Some(stdin) = &mut self.stdin {
            let mut payload = line.clone();
            payload.push('\n');
            stdin
                .write_all(payload.as_bytes())
                .await
                .map_err(AdapterError::Spawn)?;
            stdin.flush().await.map_err(AdapterError::Spawn)?;
        }
        Ok(self.record_and_normalize(AdapterInput::Stdin { line }))
    }

    async fn handshake(
        &mut self,
        child: &mut Child,
        io_rx: &mut mpsc::UnboundedReceiver<ChildIo>,
    ) -> Result<HandshakeOutcome, AdapterError> {
        // 1. Send `initialize`
        let init_id = self.alloc_rpc_id();
        let init_req = InitializeRequest::new(ProtocolVersion::V1)
            .client_capabilities(self.capabilities.to_acp())
            .client_info(Implementation::new("atrium", "0.0.0"));
        let init_msg = json!({
            "jsonrpc": "2.0",
            "id": init_id,
            "method": "initialize",
            "params": init_req,
        });
        self.send_json(&init_msg).await?;

        let init_resp_val = match self.wait_for_rpc_response(init_id, child, io_rx).await? {
            WaitOutcome::Response(v) => v,
            WaitOutcome::RpcError => {
                let code = terminate_and_reap(&mut self.stdin, child).await;
                return Ok(HandshakeOutcome::PreSessionExit { exit_code: code });
            }
            WaitOutcome::Exited(code) => {
                let _ = terminate_and_reap(&mut self.stdin, child).await;
                return Ok(HandshakeOutcome::PreSessionExit { exit_code: code });
            }
        };

        let init_resp = serde_json::from_value::<InitializeResponse>(init_resp_val).ok();

        // 2. Build MCP server configs and optional _meta
        let mcp_servers: Vec<AcpMcpServer> = self
            .config
            .mcp_servers
            .iter()
            .filter(|_| !self.mcp_via_config)
            .map(|s| {
                let env_vars = s
                    .env
                    .iter()
                    .map(|(k, v)| EnvVariable::new(k.clone(), v.clone()))
                    .collect();
                AcpMcpServer::Stdio(
                    McpServerStdio::new(s.name.clone(), s.command.clone())
                        .args(s.args.clone())
                        .env(env_vars),
                )
            })
            .collect();

        let mut meta_map = serde_json::Map::new();
        if let Some(model) = &self.config.model {
            meta_map.insert("model".into(), Value::String(model.clone()));
        }
        if let Some(effort) = &self.config.effort {
            meta_map.insert("effort".into(), Value::String(effort.clone()));
        }

        // 3. Send `session/load` (when resuming) or `session/new`
        let sess_rpc_id = self.alloc_rpc_id();
        let sess_msg = if let Some(resume_id) = &self.config.resume {
            let supports_load = init_resp
                .as_ref()
                .map(|r| r.agent_capabilities.load_session)
                .unwrap_or(true);
            let supports_resume = init_resp
                .as_ref()
                .map(|r| r.agent_capabilities.session_capabilities.resume.is_some())
                .unwrap_or(false);
            if !supports_load && !supports_resume {
                let _ = terminate_and_reap(&mut self.stdin, child).await;
                return Err(AdapterError::Unsupported(
                    "ACP agent does not advertise loadSession or session/resume capability".into(),
                ));
            }
            self.session_id = resume_id.clone();
            let mut req =
                LoadSessionRequest::new(SessionId::new(resume_id.clone()), self.config.cwd.clone())
                    .mcp_servers(mcp_servers);
            if !meta_map.is_empty() {
                req = req.meta(meta_map);
            }
            json!({
                "jsonrpc": "2.0",
                "id": sess_rpc_id,
                "method": "session/load",
                "params": req,
            })
        } else {
            let mut req = NewSessionRequest::new(self.config.cwd.clone()).mcp_servers(mcp_servers);
            if !meta_map.is_empty() {
                req = req.meta(meta_map);
            }
            json!({
                "jsonrpc": "2.0",
                "id": sess_rpc_id,
                "method": "session/new",
                "params": req,
            })
        };
        self.send_json(&sess_msg).await?;

        let sess_resp_val = match self
            .wait_for_rpc_response(sess_rpc_id, child, io_rx)
            .await?
        {
            WaitOutcome::Response(v) => v,
            WaitOutcome::RpcError => {
                let code = terminate_and_reap(&mut self.stdin, child).await;
                return Ok(HandshakeOutcome::PreSessionExit { exit_code: code });
            }
            WaitOutcome::Exited(code) => {
                let _ = terminate_and_reap(&mut self.stdin, child).await;
                return Ok(HandshakeOutcome::PreSessionExit { exit_code: code });
            }
        };

        if self.session_id.is_empty() {
            if let Ok(new_resp) =
                serde_json::from_value::<NewSessionResponse>(sess_resp_val.clone())
            {
                self.session_id = new_resp.session_id.0.to_string();
            } else if let Some(sid) = sess_resp_val.get("sessionId").and_then(Value::as_str) {
                self.session_id = sid.to_owned();
            }
        }

        // 4. Select the model. OpenCode ignores `_meta.model` on session/new, so
        //    the model is verified from the response and, if the agent is not
        //    running it, selected over ACP. The normalizer holds back
        //    `SessionStarted` until this succeeds, so a model the agent rejects
        //    (or silently does not apply) is a pre-session failure.
        if let Some(model) = self.config.model.clone() {
            let state = parse_model_state(&sess_resp_val);
            if !state.as_ref().is_some_and(|s| s.confirms(&model)) {
                let (method, params) = match state.map(|s| s.via) {
                    Some(ModelVia::ModelsState) => (
                        "session/set_model",
                        json!({ "sessionId": self.session_id, "modelId": model }),
                    ),
                    Some(ModelVia::ConfigOption { config_id }) => (
                        "session/set_config_option",
                        json!({ "sessionId": self.session_id, "configId": config_id, "value": model }),
                    ),
                    // The agent reports no model state at all: try the
                    // conventional id and let the agent answer.
                    None => (
                        "session/set_config_option",
                        json!({ "sessionId": self.session_id, "configId": "model", "value": model }),
                    ),
                };
                match self.request(method, params, child, io_rx).await? {
                    WaitOutcome::Response(_) => {}
                    WaitOutcome::RpcError => {
                        let code = terminate_and_reap(&mut self.stdin, child).await;
                        return Ok(HandshakeOutcome::PreSessionExit { exit_code: code });
                    }
                    WaitOutcome::Exited(code) => {
                        let _ = terminate_and_reap(&mut self.stdin, child).await;
                        return Ok(HandshakeOutcome::PreSessionExit { exit_code: code });
                    }
                }
                if !self.normalizer.session_started() {
                    // The agent answered but the normalizer did not accept the
                    // model (it reported another one): fail before the session.
                    let code = terminate_and_reap(&mut self.stdin, child).await;
                    return Ok(HandshakeOutcome::PreSessionExit { exit_code: code });
                }
            }
        }

        // 5. Apply the PermissionPolicy and effort where the agent offers them.
        //    Failures here are reported by the normalizer as non-fatal errors.
        let mut options_source = sess_resp_val.clone();
        if let Some(modes_val) = sess_resp_val.get("modes") {
            if let Ok(modes) = serde_json::from_value::<SessionModeState>(modes_val.clone()) {
                if let Some(target_mode) = select_mode_for_policy(self.config.permissions, &modes) {
                    if target_mode != modes.current_mode_id.0.as_ref() {
                        let params = json!({ "sessionId": self.session_id, "modeId": target_mode });
                        let _ = self
                            .request("session/set_mode", params, child, io_rx)
                            .await?;
                    }
                }
            }
        } else if let Some((id, current, offered)) =
            find_select_option(&options_source, "mode", "mode")
        {
            if let Some(target) = select_mode_value(self.config.permissions, &offered) {
                if target != current {
                    let params =
                        json!({ "sessionId": self.session_id, "configId": id, "value": target });
                    if let WaitOutcome::Response(v) = self
                        .request("session/set_config_option", params, child, io_rx)
                        .await?
                    {
                        if v.get("configOptions").is_some() {
                            options_source = v;
                        }
                    }
                }
            }
        }
        if let Some(effort) = self.config.effort.clone() {
            match find_select_option(&options_source, "thought_level", "effort") {
                Some((id, current, offered))
                    if current != effort && (offered.is_empty() || offered.contains(&effort)) =>
                {
                    let params =
                        json!({ "sessionId": self.session_id, "configId": id, "value": effort });
                    let _ = self
                        .request("session/set_config_option", params, child, io_rx)
                        .await?;
                }
                Some((_, current, _)) if current == effort => {}
                // Not offered (or not a listed level): ask anyway so the agent's
                // refusal is recorded and reported, not silently ignored.
                _ => {
                    let params = json!({ "sessionId": self.session_id, "configId": "effort", "value": effort });
                    let _ = self
                        .request("session/set_config_option", params, child, io_rx)
                        .await?;
                }
            }
        }

        Ok(HandshakeOutcome::Established)
    }

    /// Sends a request and waits for its response.
    async fn request(
        &mut self,
        method: &str,
        params: Value,
        child: &mut Child,
        io_rx: &mut mpsc::UnboundedReceiver<ChildIo>,
    ) -> Result<WaitOutcome, AdapterError> {
        let id = self.alloc_rpc_id();
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.send_json(&msg).await?;
        self.wait_for_rpc_response(id, child, io_rx).await
    }

    async fn wait_for_rpc_response(
        &mut self,
        expected_id: i64,
        child: &mut Child,
        io_rx: &mut mpsc::UnboundedReceiver<ChildIo>,
    ) -> Result<WaitOutcome, AdapterError> {
        let expected_id_str = expected_id.to_string();
        loop {
            tokio::select! {
                maybe_io = io_rx.recv() => {
                    match maybe_io {
                        Some(ChildIo::Stdout(line)) => {
                            self.record_and_normalize(AdapterInput::Stdout { line: line.clone() });
                            if let Ok(val) = serde_json::from_str::<Value>(&line) {
                                if val.get("method").is_none() {
                                    if let Some(id_str) = val.get("id").and_then(rpc_id_to_string) {
                                        if id_str == expected_id_str {
                                            if val.get("error").is_some() {
                                                return Ok(WaitOutcome::RpcError);
                                            }
                                            let result = val.get("result").cloned().unwrap_or(Value::Null);
                                            return Ok(WaitOutcome::Response(result));
                                        }
                                    }
                                }
                            }
                        }
                        Some(ChildIo::Stderr(line)) => {
                            self.record_and_normalize(AdapterInput::Stderr { line });
                        }
                        None => {
                            let status = child.wait().await.ok();
                            return Ok(WaitOutcome::Exited(status.and_then(|s| s.code())));
                        }
                    }
                }
                status = child.wait() => {
                    // Drain any remaining buffered lines before returning exit.
                    while let Ok(io) = io_rx.try_recv() {
                        match io {
                            ChildIo::Stdout(line) => {
                                self.record_and_normalize(AdapterInput::Stdout { line: line.clone() });
                                if let Ok(val) = serde_json::from_str::<Value>(&line) {
                                    if val.get("method").is_none() {
                                        if let Some(id_str) = val.get("id").and_then(rpc_id_to_string) {
                                            if id_str == expected_id_str {
                                                if val.get("error").is_some() {
                                                    return Ok(WaitOutcome::RpcError);
                                                }
                                                let result = val.get("result").cloned().unwrap_or(Value::Null);
                                                return Ok(WaitOutcome::Response(result));
                                            }
                                        }
                                    }
                                }
                            }
                            ChildIo::Stderr(line) => {
                                self.record_and_normalize(AdapterInput::Stderr { line });
                            }
                        }
                    }
                    let code = status.ok().and_then(|s| s.code());
                    return Ok(WaitOutcome::Exited(code));
                }
            }
        }
    }

    async fn run(
        mut self,
        mut child: Child,
        mut io_rx: mpsc::UnboundedReceiver<ChildIo>,
        mut commands_rx: mpsc::Receiver<SessionCommand>,
        mut events_closed_rx: watch::Receiver<bool>,
    ) {
        let mut commands_closed = false;
        let mut events_closed = *events_closed_rx.borrow();

        loop {
            if commands_closed && events_closed {
                self.shutdown_and_reap(&mut child, &mut io_rx).await;
                return;
            }

            tokio::select! {
                maybe_cmd = commands_rx.recv(), if !commands_closed => {
                    match maybe_cmd {
                        Some(cmd) => {
                            let is_shutdown = matches!(cmd, SessionCommand::Shutdown);
                            self.handle_command(cmd).await;
                            if is_shutdown {
                                self.shutdown_and_reap(&mut child, &mut io_rx).await;
                                return;
                            }
                        }
                        None => {
                            commands_closed = true;
                        }
                    }
                }
                res = events_closed_rx.changed(), if !events_closed => {
                    if res.is_ok() {
                        events_closed = *events_closed_rx.borrow();
                    } else {
                        events_closed = true;
                    }
                }
                maybe_io = io_rx.recv() => {
                    match maybe_io {
                        Some(ChildIo::Stdout(line)) => {
                            self.handle_stdout_line(line).await;
                        }
                        Some(ChildIo::Stderr(line)) => {
                            self.record_and_normalize(AdapterInput::Stderr { line });
                        }
                        None => {
                            let code = child.wait().await.ok().and_then(|s| s.code());
                            self.cleanup_terminals().await;
                            self.record_and_normalize(AdapterInput::Exit { code });
                            return;
                        }
                    }
                }
                status = child.wait() => {
                    while let Ok(io) = io_rx.try_recv() {
                        match io {
                            ChildIo::Stdout(line) => self.handle_stdout_line(line).await,
                            ChildIo::Stderr(line) => {
                                self.record_and_normalize(AdapterInput::Stderr { line });
                            }
                        }
                    }
                    let code = status.ok().and_then(|s| s.code());
                    self.cleanup_terminals().await;
                    self.record_and_normalize(AdapterInput::Exit { code });
                    return;
                }
            }
        }
    }

    async fn handle_command(&mut self, command: SessionCommand) {
        self.record_and_normalize(AdapterInput::Command {
            command: command.clone(),
        });

        match command {
            SessionCommand::SendUserMessage { text } => {
                if self.active_prompt_rpc_id.is_some() {
                    if self.queued_messages.len() < MAX_QUEUED_MESSAGES {
                        self.queued_messages.push_back(text);
                    }
                } else {
                    self.send_prompt(text).await;
                }
            }
            SessionCommand::ResolveApproval {
                request_id,
                decision,
            } => {
                if let Some(op) = self.pending_approvals.remove(&request_id) {
                    self.complete_pending_approval(op, decision).await;
                }
            }
            SessionCommand::AnswerQuestion {
                request_id,
                answers,
            } => {
                if let Some(pq) = self.pending_questions.remove(&request_id) {
                    self.complete_pending_question(pq, answers).await;
                }
            }
            SessionCommand::Interrupt => {
                // Cancel any pending permission requests on the wire.
                let pending = std::mem::take(&mut self.pending_approvals);
                for (_, op) in pending {
                    self.cancel_pending_op(op).await;
                }
                let cancel_msg = json!({
                    "jsonrpc": "2.0",
                    "method": "session/cancel",
                    "params": {
                        "sessionId": self.session_id,
                    }
                });
                let _ = self.send_json(&cancel_msg).await;
            }
            SessionCommand::Shutdown => {
                if self.active_prompt_rpc_id.is_some() {
                    let cancel_msg = json!({
                        "jsonrpc": "2.0",
                        "method": "session/cancel",
                        "params": {
                            "sessionId": self.session_id,
                        }
                    });
                    let _ = self.send_json(&cancel_msg).await;
                }
            }
        }
    }

    async fn send_prompt(&mut self, text: String) {
        let rpc_id = self.alloc_rpc_id();
        self.active_prompt_rpc_id = Some(rpc_id.to_string());
        let req = PromptRequest::new(
            SessionId::new(self.session_id.clone()),
            vec![ContentBlock::Text(TextContent::new(text))],
        );
        let msg = json!({
            "jsonrpc": "2.0",
            "id": rpc_id,
            "method": "session/prompt",
            "params": req,
        });
        let _ = self.send_json(&msg).await;
    }

    async fn handle_stdout_line(&mut self, line: String) {
        self.record_and_normalize(AdapterInput::Stdout { line: line.clone() });

        let Ok(val) = serde_json::from_str::<Value>(&line) else {
            return;
        };
        let Some(obj) = val.as_object() else {
            return;
        };

        // Check if a prompt turn just completed so we can dequeue the next message.
        if obj.get("method").is_none() {
            if let Some(id_str) = obj.get("id").and_then(rpc_id_to_string) {
                if self.active_prompt_rpc_id.as_deref() == Some(id_str.as_str()) {
                    self.active_prompt_rpc_id = None;
                    if let Some(next_text) = self.queued_messages.pop_front() {
                        self.send_prompt(next_text).await;
                    }
                }
            }
            return;
        }

        // Agent -> Client request requiring a response (has both "method" and "id").
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            return;
        };
        let Some(raw_id) = obj.get("id").cloned() else {
            return;
        };
        let Some(rpc_id_str) = rpc_id_to_string(&raw_id) else {
            return;
        };
        let params = obj.get("params").cloned().unwrap_or(Value::Null);

        match method {
            "session/request_permission" => {
                self.on_agent_request_permission(raw_id, rpc_id_str, params)
                    .await;
            }
            "fs/read_text_file" => {
                self.on_agent_fs_read(raw_id, rpc_id_str, params).await;
            }
            "fs/write_text_file" => {
                self.on_agent_fs_write(raw_id, rpc_id_str, params).await;
            }
            "terminal/create" => {
                self.on_agent_terminal_create(raw_id, rpc_id_str, params)
                    .await;
            }
            "terminal/output" => {
                self.on_agent_terminal_output(raw_id, params).await;
            }
            "terminal/wait_for_exit" => {
                self.on_agent_terminal_wait(raw_id, params).await;
            }
            "terminal/kill" => {
                self.on_agent_terminal_kill(raw_id, params).await;
            }
            "terminal/release" => {
                self.on_agent_terminal_release(raw_id, params).await;
            }
            "elicitation/create" => {
                if let Ok(req) = serde_json::from_value::<CreateElicitationRequest>(params) {
                    self.pending_questions.insert(
                        rpc_id_str,
                        PendingQuestionRpc {
                            rpc_id: raw_id,
                            req,
                        },
                    );
                }
            }
            _ => {
                let err_resp = json!({
                    "jsonrpc": "2.0",
                    "id": raw_id,
                    "error": {
                        "code": -32601,
                        "message": format!("Method not found: {method}"),
                    }
                });
                let _ = self.send_json(&err_resp).await;
            }
        }
    }

    async fn on_agent_request_permission(
        &mut self,
        raw_id: Value,
        rpc_id_str: String,
        params: Value,
    ) {
        let mut params = params;
        crate::normalizer::hoist_tool_name(&mut params);
        let Ok(req) = serde_json::from_value::<RequestPermissionRequest>(params) else {
            let err_resp = json!({
                "jsonrpc": "2.0",
                "id": raw_id,
                "error": { "code": -32602, "message": "Invalid RequestPermissionRequest" }
            });
            let _ = self.send_json(&err_resp).await;
            return;
        };

        let call_id = req.tool_call.tool_call_id.0.to_string();
        let fields = &req.tool_call.fields;
        let acp_kind = fields.kind.unwrap_or(AcpToolKind::Other);
        let title_str = fields.title.as_deref().unwrap_or_default();
        let kind = map_tool_kind(acp_kind, req.tool_call.meta.as_ref(), title_str);
        let name = extract_tool_name(req.tool_call.meta.as_ref(), acp_kind, title_str);
        let title = fields.title.clone();
        let locations = fields.locations.clone().unwrap_or_default();
        let input = normalize_tool_input(fields.raw_input.as_ref(), &locations);

        let check = ToolCheck {
            call_id: Some(call_id),
            name,
            kind,
            title,
            input,
        };
        let decision = self.config.gate.check(&check).await;
        self.record_and_normalize(AdapterInput::Gate {
            check,
            decision: decision.clone(),
        });

        match decision {
            GateDecision::Allow => {
                let resp = build_permission_response(&req.options, true);
                let msg = json!({
                    "jsonrpc": "2.0",
                    "id": raw_id,
                    "result": resp,
                });
                let _ = self.send_json(&msg).await;
            }
            GateDecision::Deny { .. } => {
                let resp = build_permission_response(&req.options, false);
                let msg = json!({
                    "jsonrpc": "2.0",
                    "id": raw_id,
                    "result": resp,
                });
                let _ = self.send_json(&msg).await;
            }
            GateDecision::Ask => {
                self.pending_approvals.insert(
                    rpc_id_str,
                    PendingClientOp::Permission {
                        rpc_id: raw_id,
                        options: req.options,
                    },
                );
            }
        }
    }

    async fn on_agent_fs_read(&mut self, raw_id: Value, rpc_id_str: String, params: Value) {
        let Ok(req) = serde_json::from_value::<ReadTextFileRequest>(params) else {
            let err_resp = json!({
                "jsonrpc": "2.0",
                "id": raw_id,
                "error": { "code": -32602, "message": "Invalid ReadTextFileRequest" }
            });
            let _ = self.send_json(&err_resp).await;
            return;
        };

        let resolved_path = resolve_path(&self.config.cwd, &req.path);
        let call_id = self.normalizer.call_id_for_rpc(&rpc_id_str);
        let check = ToolCheck {
            call_id,
            name: "fs/read_text_file".into(),
            kind: ToolKind::Read,
            title: Some(format!("Read {}", req.path.display())),
            input: json!({ "path": req.path }),
        };
        let decision = self.config.gate.check(&check).await;
        self.record_and_normalize(AdapterInput::Gate {
            check,
            decision: decision.clone(),
        });

        match decision {
            GateDecision::Allow => {
                self.execute_fs_read(raw_id, &resolved_path, req.line, req.limit)
                    .await;
            }
            GateDecision::Deny { reason } => {
                let msg = json!({
                    "jsonrpc": "2.0",
                    "id": raw_id,
                    "error": {
                        "code": -32000,
                        "message": format!("Permission denied: {reason}"),
                    }
                });
                let _ = self.send_json(&msg).await;
            }
            GateDecision::Ask => {
                self.pending_approvals.insert(
                    rpc_id_str,
                    PendingClientOp::FsRead {
                        rpc_id: raw_id,
                        resolved_path,
                        line: req.line,
                        limit: req.limit,
                    },
                );
            }
        }
    }

    async fn execute_fs_read(
        &mut self,
        raw_id: Value,
        resolved_path: &Path,
        line: Option<u32>,
        limit: Option<u32>,
    ) {
        match tokio::fs::read_to_string(resolved_path).await {
            Ok(raw_content) => {
                let content = slice_lines(&raw_content, line, limit);
                let msg = json!({
                    "jsonrpc": "2.0",
                    "id": raw_id,
                    "result": { "content": content },
                });
                let _ = self.send_json(&msg).await;
            }
            Err(e) => {
                let msg = json!({
                    "jsonrpc": "2.0",
                    "id": raw_id,
                    "error": {
                        "code": -32000,
                        "message": format!("Failed to read file: {e}"),
                    }
                });
                let _ = self.send_json(&msg).await;
            }
        }
    }

    async fn on_agent_fs_write(&mut self, raw_id: Value, rpc_id_str: String, params: Value) {
        let Ok(req) = serde_json::from_value::<WriteTextFileRequest>(params) else {
            let err_resp = json!({
                "jsonrpc": "2.0",
                "id": raw_id,
                "error": { "code": -32602, "message": "Invalid WriteTextFileRequest" }
            });
            let _ = self.send_json(&err_resp).await;
            return;
        };

        let resolved_path = resolve_path(&self.config.cwd, &req.path);
        let call_id = self.normalizer.call_id_for_rpc(&rpc_id_str);
        let check = ToolCheck {
            call_id,
            name: "fs/write_text_file".into(),
            kind: ToolKind::Edit,
            title: Some(format!("Write {}", req.path.display())),
            input: json!({ "path": req.path }),
        };
        let decision = self.config.gate.check(&check).await;
        self.record_and_normalize(AdapterInput::Gate {
            check,
            decision: decision.clone(),
        });

        match decision {
            GateDecision::Allow => {
                self.execute_fs_write(raw_id, &resolved_path, &req.content)
                    .await;
            }
            GateDecision::Deny { reason } => {
                let msg = json!({
                    "jsonrpc": "2.0",
                    "id": raw_id,
                    "error": {
                        "code": -32000,
                        "message": format!("Permission denied: {reason}"),
                    }
                });
                let _ = self.send_json(&msg).await;
            }
            GateDecision::Ask => {
                self.pending_approvals.insert(
                    rpc_id_str,
                    PendingClientOp::FsWrite {
                        rpc_id: raw_id,
                        resolved_path,
                        content: req.content,
                    },
                );
            }
        }
    }

    async fn execute_fs_write(&mut self, raw_id: Value, resolved_path: &Path, content: &str) {
        if let Some(parent) = resolved_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        match tokio::fs::write(resolved_path, content).await {
            Ok(()) => {
                let msg = json!({
                    "jsonrpc": "2.0",
                    "id": raw_id,
                    "result": {},
                });
                let _ = self.send_json(&msg).await;
            }
            Err(e) => {
                let msg = json!({
                    "jsonrpc": "2.0",
                    "id": raw_id,
                    "error": {
                        "code": -32000,
                        "message": format!("Failed to write file: {e}"),
                    }
                });
                let _ = self.send_json(&msg).await;
            }
        }
    }

    async fn on_agent_terminal_create(&mut self, raw_id: Value, rpc_id_str: String, params: Value) {
        let Ok(req) = serde_json::from_value::<CreateTerminalRequest>(params) else {
            let err_resp = json!({
                "jsonrpc": "2.0",
                "id": raw_id,
                "error": { "code": -32602, "message": "Invalid CreateTerminalRequest" }
            });
            let _ = self.send_json(&err_resp).await;
            return;
        };

        let title = if req.args.is_empty() {
            req.command.clone()
        } else {
            format!("{} {}", req.command, req.args.join(" "))
        };
        let call_id = self.normalizer.call_id_for_rpc(&rpc_id_str);
        let check = ToolCheck {
            call_id,
            name: "terminal/create".into(),
            kind: ToolKind::Execute,
            title: Some(title),
            input: json!({ "command": req.command, "args": req.args }),
        };
        let decision = self.config.gate.check(&check).await;
        self.record_and_normalize(AdapterInput::Gate {
            check,
            decision: decision.clone(),
        });

        match decision {
            GateDecision::Allow => {
                self.execute_terminal_create(raw_id, req).await;
            }
            GateDecision::Deny { reason } => {
                let msg = json!({
                    "jsonrpc": "2.0",
                    "id": raw_id,
                    "error": {
                        "code": -32000,
                        "message": format!("Permission denied: {reason}"),
                    }
                });
                let _ = self.send_json(&msg).await;
            }
            GateDecision::Ask => {
                self.pending_approvals.insert(
                    rpc_id_str,
                    PendingClientOp::TerminalCreate {
                        rpc_id: raw_id,
                        params: req,
                    },
                );
            }
        }
    }

    async fn execute_terminal_create(&mut self, raw_id: Value, req: CreateTerminalRequest) {
        let cwd = req
            .cwd
            .as_deref()
            .map(|p| resolve_path(&self.config.cwd, p))
            .unwrap_or_else(|| self.config.cwd.clone());
        let mut cmd = Command::new(&req.command);
        cmd.args(&req.args)
            .current_dir(cwd)
            .envs(req.env.iter().map(|e| (e.name.clone(), e.value.clone())))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        match cmd.spawn() {
            Ok(mut term_child) => {
                self.next_terminal_seq += 1;
                let term_id = format!("term-{}", self.next_terminal_seq);
                let output_buf = Arc::new(Mutex::new(Vec::<u8>::new()));

                if let Some(mut out) = term_child.stdout.take() {
                    let buf_clone = Arc::clone(&output_buf);
                    tokio::spawn(async move {
                        let mut tmp = [0u8; 4096];
                        while let Ok(n) = out.read(&mut tmp).await {
                            if n == 0 {
                                break;
                            }
                            buf_clone.lock().await.extend_from_slice(&tmp[..n]);
                        }
                    });
                }
                if let Some(mut err) = term_child.stderr.take() {
                    let buf_clone = Arc::clone(&output_buf);
                    tokio::spawn(async move {
                        let mut tmp = [0u8; 4096];
                        while let Ok(n) = err.read(&mut tmp).await {
                            if n == 0 {
                                break;
                            }
                            buf_clone.lock().await.extend_from_slice(&tmp[..n]);
                        }
                    });
                }

                self.terminals.insert(
                    term_id.clone(),
                    ManagedTerminal {
                        child: Arc::new(Mutex::new(term_child)),
                        output: output_buf,
                        output_byte_limit: req.output_byte_limit.map(|n| n as usize),
                    },
                );

                let msg = json!({
                    "jsonrpc": "2.0",
                    "id": raw_id,
                    "result": { "terminalId": term_id },
                });
                let _ = self.send_json(&msg).await;
            }
            Err(e) => {
                let msg = json!({
                    "jsonrpc": "2.0",
                    "id": raw_id,
                    "error": {
                        "code": -32000,
                        "message": format!("Failed to spawn terminal command: {e}"),
                    }
                });
                let _ = self.send_json(&msg).await;
            }
        }
    }

    async fn on_agent_terminal_output(&mut self, raw_id: Value, params: Value) {
        let term_id = params
            .get("terminalId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(term) = self.terminals.get(term_id) else {
            let msg = json!({
                "jsonrpc": "2.0",
                "id": raw_id,
                "error": { "code": -32602, "message": format!("Unknown terminalId: {term_id}") }
            });
            let _ = self.send_json(&msg).await;
            return;
        };

        let exit_status = {
            let mut ch = term.child.lock().await;
            ch.try_wait()
                .ok()
                .flatten()
                .map(|s| json!({ "exitCode": s.code().unwrap_or(1), "signal": Value::Null }))
        };
        let (out_str, truncated) = {
            let buf = term.output.lock().await;
            if let Some(limit) = term.output_byte_limit {
                if buf.len() > limit {
                    let start = buf.len() - limit;
                    (String::from_utf8_lossy(&buf[start..]).into_owned(), true)
                } else {
                    (String::from_utf8_lossy(&buf).into_owned(), false)
                }
            } else {
                (String::from_utf8_lossy(&buf).into_owned(), false)
            }
        };

        let msg = json!({
            "jsonrpc": "2.0",
            "id": raw_id,
            "result": {
                "output": out_str,
                "truncated": truncated,
                "exitStatus": exit_status,
            }
        });
        let _ = self.send_json(&msg).await;
    }

    async fn on_agent_terminal_wait(&mut self, raw_id: Value, params: Value) {
        let term_id = params
            .get("terminalId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(term) = self.terminals.get(term_id) else {
            let msg = json!({
                "jsonrpc": "2.0",
                "id": raw_id,
                "error": { "code": -32602, "message": format!("Unknown terminalId: {term_id}") }
            });
            let _ = self.send_json(&msg).await;
            return;
        };

        let code = {
            let mut ch = term.child.lock().await;
            ch.wait().await.ok().and_then(|s| s.code()).unwrap_or(0)
        };
        let msg = json!({
            "jsonrpc": "2.0",
            "id": raw_id,
            "result": {
                "exitStatus": {
                    "exitCode": code,
                    "signal": Value::Null,
                }
            }
        });
        let _ = self.send_json(&msg).await;
    }

    async fn on_agent_terminal_kill(&mut self, raw_id: Value, params: Value) {
        let term_id = params
            .get("terminalId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Some(term) = self.terminals.get(term_id) {
            let mut ch = term.child.lock().await;
            let _ = ch.kill().await;
        }
        let msg = json!({
            "jsonrpc": "2.0",
            "id": raw_id,
            "result": {},
        });
        let _ = self.send_json(&msg).await;
    }

    async fn on_agent_terminal_release(&mut self, raw_id: Value, params: Value) {
        let term_id = params
            .get("terminalId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Some(term) = self.terminals.remove(term_id) {
            let mut ch = term.child.lock().await;
            let _ = ch.kill().await;
            let _ = ch.wait().await;
        }
        let msg = json!({
            "jsonrpc": "2.0",
            "id": raw_id,
            "result": {},
        });
        let _ = self.send_json(&msg).await;
    }

    async fn complete_pending_approval(&mut self, op: PendingClientOp, decision: ApprovalDecision) {
        match (op, decision) {
            (PendingClientOp::Permission { rpc_id, options }, ApprovalDecision::Allow) => {
                let resp = build_permission_response(&options, true);
                let msg = json!({
                    "jsonrpc": "2.0",
                    "id": rpc_id,
                    "result": resp,
                });
                let _ = self.send_json(&msg).await;
            }
            (PendingClientOp::Permission { rpc_id, options }, ApprovalDecision::Deny { .. }) => {
                let resp = build_permission_response(&options, false);
                let msg = json!({
                    "jsonrpc": "2.0",
                    "id": rpc_id,
                    "result": resp,
                });
                let _ = self.send_json(&msg).await;
            }
            (
                PendingClientOp::FsRead {
                    rpc_id,
                    resolved_path,
                    line,
                    limit,
                },
                ApprovalDecision::Allow,
            ) => {
                self.execute_fs_read(rpc_id, &resolved_path, line, limit)
                    .await;
            }
            (PendingClientOp::FsRead { rpc_id, .. }, ApprovalDecision::Deny { message }) => {
                let reason = message.unwrap_or_else(|| "denied by user".into());
                let msg = json!({
                    "jsonrpc": "2.0",
                    "id": rpc_id,
                    "error": { "code": -32000, "message": format!("Permission denied: {reason}") }
                });
                let _ = self.send_json(&msg).await;
            }
            (
                PendingClientOp::FsWrite {
                    rpc_id,
                    resolved_path,
                    content,
                },
                ApprovalDecision::Allow,
            ) => {
                self.execute_fs_write(rpc_id, &resolved_path, &content)
                    .await;
            }
            (PendingClientOp::FsWrite { rpc_id, .. }, ApprovalDecision::Deny { message }) => {
                let reason = message.unwrap_or_else(|| "denied by user".into());
                let msg = json!({
                    "jsonrpc": "2.0",
                    "id": rpc_id,
                    "error": { "code": -32000, "message": format!("Permission denied: {reason}") }
                });
                let _ = self.send_json(&msg).await;
            }
            (PendingClientOp::TerminalCreate { rpc_id, params }, ApprovalDecision::Allow) => {
                self.execute_terminal_create(rpc_id, params).await;
            }
            (
                PendingClientOp::TerminalCreate { rpc_id, .. },
                ApprovalDecision::Deny { message },
            ) => {
                let reason = message.unwrap_or_else(|| "denied by user".into());
                let msg = json!({
                    "jsonrpc": "2.0",
                    "id": rpc_id,
                    "error": { "code": -32000, "message": format!("Permission denied: {reason}") }
                });
                let _ = self.send_json(&msg).await;
            }
        }
    }

    async fn cancel_pending_op(&mut self, op: PendingClientOp) {
        match op {
            PendingClientOp::Permission { rpc_id, .. } => {
                let resp = RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled);
                let msg = json!({
                    "jsonrpc": "2.0",
                    "id": rpc_id,
                    "result": resp,
                });
                let _ = self.send_json(&msg).await;
            }
            PendingClientOp::FsRead { rpc_id, .. }
            | PendingClientOp::FsWrite { rpc_id, .. }
            | PendingClientOp::TerminalCreate { rpc_id, .. } => {
                let msg = json!({
                    "jsonrpc": "2.0",
                    "id": rpc_id,
                    "error": { "code": -32800, "message": "Request cancelled" }
                });
                let _ = self.send_json(&msg).await;
            }
        }
    }

    async fn complete_pending_question(
        &mut self,
        pq: PendingQuestionRpc,
        answers: Vec<QuestionAnswer>,
    ) {
        let mut content_map = BTreeMap::new();
        for ans in answers {
            let is_array = match &pq.req.mode {
                ElicitationMode::Form(f) => matches!(
                    f.requested_schema.properties.get(&ans.question_id),
                    Some(ElicitationPropertySchema::Array(_))
                ),
                _ => ans.selected.len() > 1,
            };
            if is_array {
                content_map.insert(
                    ans.question_id,
                    ElicitationContentValue::StringArray(ans.selected),
                );
            } else if let Some(first) = ans.selected.into_iter().next() {
                content_map.insert(ans.question_id, ElicitationContentValue::String(first));
            } else if let Some(text) = ans.text {
                content_map.insert(ans.question_id, ElicitationContentValue::String(text));
            }
        }
        let resp = CreateElicitationResponse::new(ElicitationAction::Accept(
            ElicitationAcceptAction::new().content(Some(content_map)),
        ));
        let msg = json!({
            "jsonrpc": "2.0",
            "id": pq.rpc_id,
            "result": resp,
        });
        let _ = self.send_json(&msg).await;
    }

    async fn cleanup_terminals(&mut self) {
        let terminals = std::mem::take(&mut self.terminals);
        for (_, term) in terminals {
            let mut ch = term.child.lock().await;
            let _ = ch.kill().await;
            let _ = ch.wait().await;
        }
    }

    async fn shutdown_and_reap(
        &mut self,
        child: &mut Child,
        io_rx: &mut mpsc::UnboundedReceiver<ChildIo>,
    ) {
        self.cleanup_terminals().await;
        let code = terminate_and_reap(&mut self.stdin, child).await;
        while let Ok(io) = io_rx.try_recv() {
            match io {
                ChildIo::Stdout(line) => {
                    self.record_and_normalize(AdapterInput::Stdout { line });
                }
                ChildIo::Stderr(line) => {
                    self.record_and_normalize(AdapterInput::Stderr { line });
                }
            }
        }
        self.record_and_normalize(AdapterInput::Exit { code });
        let _ = self.abort_fwd_tx.send(true);
    }
}

enum WaitOutcome {
    Response(Value),
    RpcError,
    Exited(Option<i32>),
}

async fn terminate_and_reap(stdin: &mut Option<ChildStdin>, child: &mut Child) -> Option<i32> {
    // Close stdin so the child sees EOF.
    drop(stdin.take());
    match tokio::time::timeout(SHUTDOWN_GRACE, child.wait()).await {
        Ok(Ok(status)) => status.code(),
        _ => {
            let _ = child.kill().await;
            child.wait().await.ok().and_then(|s| s.code())
        }
    }
}

/// Picks the `allow_once` / `reject_once` option. Atrium's "always" rules live
/// in Atrium, so an agent-side `allow_always` is never selected (OpenCode
/// would save it as a durable project rule that later skips the gate). If the
/// agent offers no matching once-option, the request is cancelled, which is a
/// denial (fail closed).
fn build_permission_response(
    options: &[PermissionOption],
    allow: bool,
) -> RequestPermissionResponse {
    let target = if allow {
        PermissionOptionKind::AllowOnce
    } else {
        PermissionOptionKind::RejectOnce
    };
    let chosen = options.iter().find(|o| o.kind == target).or_else(|| {
        // A reject-always is still a denial; an allow-always is not an allow-once.
        (!allow)
            .then(|| {
                options
                    .iter()
                    .find(|o| o.kind == PermissionOptionKind::RejectAlways)
            })
            .flatten()
    });
    match chosen {
        Some(opt) => RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new(opt.option_id.clone()),
        )),
        None => RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled),
    }
}

/// The mode value (from a `mode` select option) for a [`PermissionPolicy`]:
/// only `ReadOnly` changes anything (OpenCode's `plan` agent denies edits).
/// `Ask` and `FullAccess` keep the agent's default; the injected `ask` rule
/// applies in every mode, so the gate sees calls either way.
fn select_mode_value(policy: PermissionPolicy, offered: &[String]) -> Option<String> {
    if policy != PermissionPolicy::ReadOnly {
        return None;
    }
    ["plan", "read-only", "readonly", "architect"]
        .iter()
        .find_map(|c| offered.iter().find(|o| o.eq_ignore_ascii_case(c)).cloned())
}

fn select_mode_for_policy(policy: PermissionPolicy, modes: &SessionModeState) -> Option<String> {
    let candidates: &[&str] = match policy {
        PermissionPolicy::Ask => &["default", "ask", "code", "normal"],
        PermissionPolicy::ReadOnly => &["plan", "read-only", "readonly", "architect", "ask"],
        PermissionPolicy::FullAccess => &[
            "bypassPermissions",
            "full_access",
            "full-access",
            "auto",
            "yolo",
            "dontAsk",
        ],
    };
    for cand in candidates {
        if let Some(found) = modes
            .available_modes
            .iter()
            .find(|m| m.id.0.as_ref().eq_ignore_ascii_case(cand))
        {
            return Some(found.id.0.to_string());
        }
    }
    None
}

fn resolve_path(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn slice_lines(content: &str, line: Option<u32>, limit: Option<u32>) -> String {
    if line.is_none() && limit.is_none() {
        return content.to_owned();
    }
    let start_idx = line.unwrap_or(1).saturating_sub(1) as usize;
    let max_lines = limit.map(|l| l as usize).unwrap_or(usize::MAX);
    content
        .lines()
        .skip(start_idx)
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opt(id: &str, kind: PermissionOptionKind) -> PermissionOption {
        PermissionOption::new(id.to_owned(), id.to_owned(), kind)
    }

    fn selected(resp: &RequestPermissionResponse) -> Option<String> {
        match &resp.outcome {
            RequestPermissionOutcome::Selected(s) => Some(s.option_id.0.to_string()),
            _ => None,
        }
    }

    #[test]
    fn allow_is_always_allow_once_never_allow_always() {
        let both = [
            opt("always", PermissionOptionKind::AllowAlways),
            opt("once", PermissionOptionKind::AllowOnce),
            opt("reject", PermissionOptionKind::RejectOnce),
        ];
        assert_eq!(
            selected(&build_permission_response(&both, true)).as_deref(),
            Some("once")
        );
        assert_eq!(
            selected(&build_permission_response(&both, false)).as_deref(),
            Some("reject")
        );
        let only_always = [opt("always", PermissionOptionKind::AllowAlways)];
        assert_eq!(
            selected(&build_permission_response(&only_always, true)),
            None
        );
        assert_eq!(
            selected(&build_permission_response(&only_always, false)),
            None
        );
    }

    #[test]
    fn only_read_only_policy_changes_the_mode() {
        let offered = vec!["build".to_owned(), "plan".to_owned()];
        assert_eq!(
            select_mode_value(PermissionPolicy::ReadOnly, &offered).as_deref(),
            Some("plan")
        );
        assert_eq!(select_mode_value(PermissionPolicy::Ask, &offered), None);
        assert_eq!(
            select_mode_value(PermissionPolicy::FullAccess, &offered),
            None
        );
    }
}
