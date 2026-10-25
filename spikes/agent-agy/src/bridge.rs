use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use atrium_agents::{AdapterInput, GateDecision, McpServer, ToolCheck, ToolGate};
use atrium_protocol::agent::{ApprovalDecision, Question, QuestionAnswer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::normalizer::{
    canonical_json, classify_tool, extract_tool_title, format_question_answers, parse_questions,
};

/// Default client-side timeout in `spike-agy-hook`, shorter than `agy`'s hook timeout
/// so the helper fails closed before `agy` kills it.
pub const DEFAULT_HOOK_CLIENT_TIMEOUT: Duration = Duration::from_secs(25);

/// How the gate bridge handles `GateDecision::Ask`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApprovalMode {
    /// Hold the `PreToolUse` hook connection open until `ResolveApproval` arrives.
    #[default]
    Block,
    /// Immediately deny the hook call with a pending-approval message, emit
    /// `ApprovalRequested`, and upon `ResolveApproval(Allow)` register a one-time
    /// [`RetryGrant`] bound to `(session_id, tool_name, canonical_args)`.
    DenyAndRetry,
}

/// JSON stdout response expected by `agy` from a `PreToolUse` hook command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookDecisionOutput {
    pub decision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl HookDecisionOutput {
    pub fn allow(reason: Option<String>) -> Self {
        Self {
            decision: "allow".to_owned(),
            reason,
        }
    }

    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            decision: "deny".to_owned(),
            reason: Some(reason.into()),
        }
    }

    pub fn is_allow(&self) -> bool {
        self.decision == "allow"
    }

    pub fn is_deny(&self) -> bool {
        self.decision == "deny"
    }
}

/// A single-use grant bound to exact `(session_id, tool_name, canonical_args)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RetryGrant {
    pub session_id: String,
    pub tool_name: String,
    pub canonical_args: String,
}

impl RetryGrant {
    pub fn new(session_id: impl Into<String>, tool_name: impl Into<String>, args: &Value) -> Self {
        Self {
            session_id: session_id.into(),
            tool_name: tool_name.into(),
            canonical_args: canonical_json(args),
        }
    }
}

/// Stores one-time retry grants for the `DenyAndRetry` fallback.
/// Each grant is bound to `session_id + tool_name + canonical_args` and consumed once.
#[derive(Debug, Default, Clone)]
pub struct RetryGrantStore {
    grants: Arc<Mutex<Vec<RetryGrant>>>,
}

impl RetryGrantStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a single-use retry grant.
    pub fn insert(
        &self,
        session_id: impl Into<String>,
        tool_name: impl Into<String>,
        args: &Value,
    ) {
        let grant = RetryGrant::new(session_id, tool_name, args);
        let mut guard = self.grants.lock().unwrap_or_else(|e| e.into_inner());
        guard.push(grant);
    }

    /// Checks and consumes at most one matching retry grant.
    /// Returns `true` if a matching grant existed and was consumed; `false` otherwise.
    pub fn consume(&self, session_id: &str, tool_name: &str, args: &Value) -> bool {
        let target_canonical = canonical_json(args);
        let mut guard = self.grants.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(pos) = guard.iter().position(|g| {
            g.session_id == session_id
                && g.tool_name == tool_name
                && g.canonical_args == target_canonical
        }) {
            guard.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn remaining(&self) -> usize {
        self.grants.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

#[derive(Debug)]
struct PendingHookApproval {
    session_id: String,
    tool_name: String,
    args: Value,
    responder: Option<oneshot::Sender<HookDecisionOutput>>,
}

#[derive(Debug)]
struct PendingHookQuestion {
    questions: Vec<Question>,
    responder: oneshot::Sender<HookDecisionOutput>,
}

#[derive(Default)]
struct BridgeState {
    session_id: String,
    call_id_hint: Option<String>,
    approvals: HashMap<String, PendingHookApproval>,
    questions: HashMap<String, PendingHookQuestion>,
}

/// Per-session Unix socket bridge between `spike-agy-hook` and `SessionConfig::gate`.
pub struct GateBridge {
    socket_path: PathBuf,
    state: Arc<Mutex<BridgeState>>,
    retry_grants: RetryGrantStore,
    task: JoinHandle<()>,
}

impl GateBridge {
    /// Starts the gate bridge listening on `socket_path` (inside a mode 0700 directory).
    pub fn start(
        socket_path: PathBuf,
        session_id: impl Into<String>,
        gate: Arc<dyn ToolGate>,
        approval_mode: ApprovalMode,
        inputs_tx: mpsc::UnboundedSender<AdapterInput>,
    ) -> io::Result<Self> {
        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent)?;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
        if socket_path.exists() {
            let _ = fs::remove_file(&socket_path);
        }

        let std_listener = std::os::unix::net::UnixListener::bind(&socket_path)?;
        std_listener.set_nonblocking(true)?;
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
        let listener = UnixListener::from_std(std_listener)?;

        let state = Arc::new(Mutex::new(BridgeState {
            session_id: session_id.into(),
            call_id_hint: None,
            approvals: HashMap::new(),
            questions: HashMap::new(),
        }));
        let retry_grants = RetryGrantStore::new();
        let req_seq = Arc::new(AtomicU64::new(0));

        let state_clone = Arc::clone(&state);
        let grants_clone = retry_grants.clone();

        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _addr)) = listener.accept().await else {
                    break;
                };
                let seq = req_seq.fetch_add(1, Ordering::Relaxed) + 1;
                let req_id = format!("req-{seq}");
                let gate = Arc::clone(&gate);
                let state = Arc::clone(&state_clone);
                let grants = grants_clone.clone();
                let inputs_tx = inputs_tx.clone();

                tokio::spawn(async move {
                    handle_hook_connection(
                        stream,
                        req_id,
                        gate,
                        approval_mode,
                        state,
                        grants,
                        inputs_tx,
                    )
                    .await;
                });
            }
        });

        Ok(Self {
            socket_path,
            state,
            retry_grants,
            task,
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn retry_grants(&self) -> &RetryGrantStore {
        &self.retry_grants
    }

    pub fn set_session_id(&self, session_id: impl Into<String>) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.session_id = session_id.into();
    }

    pub fn set_call_id_hint(&self, call_id: Option<String>) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.call_id_hint = call_id;
    }

    /// Resolves a pending approval request (`ApprovalMode::Block` or `ApprovalMode::DenyAndRetry`).
    pub fn resolve_approval(&self, request_id: &str, decision: &ApprovalDecision) -> bool {
        let pending = {
            let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            guard.approvals.remove(request_id)
        };
        let Some(pending) = pending else {
            return false;
        };

        match decision {
            ApprovalDecision::Allow => {
                if let Some(tx) = pending.responder {
                    let _ = tx.send(HookDecisionOutput::allow(None));
                } else {
                    // DenyAndRetry mode: bind a one-time retry grant to session + tool + canonical args.
                    self.retry_grants
                        .insert(pending.session_id, pending.tool_name, &pending.args);
                }
            }
            ApprovalDecision::Deny { message } => {
                if let Some(tx) = pending.responder {
                    let reason = message
                        .clone()
                        .unwrap_or_else(|| "denied by user".to_owned());
                    let _ = tx.send(HookDecisionOutput::deny(reason));
                }
            }
        }
        true
    }

    /// Resolves a pending `ask_question` hook request.
    pub fn answer_question(&self, request_id: &str, answers: &[QuestionAnswer]) -> bool {
        let pending = {
            let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            guard.questions.remove(request_id)
        };
        let Some(pending) = pending else {
            return false;
        };
        let formatted = format_question_answers(&pending.questions, answers);
        let _ = pending
            .responder
            .send(HookDecisionOutput::allow(Some(format!(
                "User answered: {formatted}"
            ))));
        true
    }

    /// Cancels all pending approvals and questions, causing blocked hooks to fail closed (`deny`).
    pub fn cancel_all(&self) {
        let (approvals, questions) = {
            let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            (
                std::mem::take(&mut guard.approvals),
                std::mem::take(&mut guard.questions),
            )
        };
        for (_, p) in approvals {
            if let Some(tx) = p.responder {
                let _ = tx.send(HookDecisionOutput::deny("approval cancelled"));
            }
        }
        for (_, q) in questions {
            let _ = q
                .responder
                .send(HookDecisionOutput::deny("question cancelled"));
        }
    }
}

impl Drop for GateBridge {
    fn drop(&mut self) {
        self.task.abort();
        let _ = fs::remove_file(&self.socket_path);
    }
}

async fn handle_hook_connection(
    stream: UnixStream,
    req_id: String,
    gate: Arc<dyn ToolGate>,
    approval_mode: ApprovalMode,
    state: Arc<Mutex<BridgeState>>,
    grants: RetryGrantStore,
    inputs_tx: mpsc::UnboundedSender<AdapterInput>,
) {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    if reader.read_line(&mut line).await.is_err() || line.trim().is_empty() {
        let reply = HookDecisionOutput::deny("empty or unreadable hook request");
        let _ = send_hook_reply(&mut write_half, &reply).await;
        return;
    }

    let Ok(payload) = serde_json::from_str::<Value>(line.trim()) else {
        let reply = HookDecisionOutput::deny("malformed JSON in hook request");
        let _ = send_hook_reply(&mut write_half, &reply).await;
        return;
    };

    let _ = inputs_tx.send(AdapterInput::HookRequest {
        id: req_id.clone(),
        payload: payload.clone(),
    });

    let tool_call = payload.get("toolCall");
    let Some(tool_name) = tool_call
        .and_then(|tc| tc.get("name"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
    else {
        let reply = HookDecisionOutput::deny("malformed hook request: missing toolCall.name");
        let _ = inputs_tx.send(AdapterInput::HookResponse {
            id: req_id,
            payload: serde_json::to_value(&reply).unwrap_or_default(),
        });
        let _ = send_hook_reply(&mut write_half, &reply).await;
        return;
    };

    let args = tool_call
        .and_then(|tc| tc.get("args").or_else(|| tc.get("parameters")))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let (session_id, call_id_hint) = {
        let guard = state.lock().unwrap_or_else(|e| e.into_inner());
        (guard.session_id.clone(), guard.call_id_hint.clone())
    };

    // Handle `ask_question` via QuestionAsked -> AnswerQuestion.
    if tool_name == "ask_question"
        && let Some(questions) = parse_questions(&args)
    {
        let (tx, rx) = oneshot::channel();
        {
            let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
            guard.questions.insert(
                req_id.clone(),
                PendingHookQuestion {
                    questions,
                    responder: tx,
                },
            );
        }
        let reply = rx
            .await
            .unwrap_or_else(|_| HookDecisionOutput::deny("question cancelled or session ended"));
        let _ = inputs_tx.send(AdapterInput::HookResponse {
            id: req_id,
            payload: serde_json::to_value(&reply).unwrap_or_default(),
        });
        let _ = send_hook_reply(&mut write_half, &reply).await;
        return;
    }

    // Check if a single-use RetryGrant authorizes this exact (session_id, tool_name, canonical_args).
    if grants.consume(&session_id, &tool_name, &args) {
        let kind = classify_tool(&tool_name);
        let title = extract_tool_title(&tool_name, &args);
        let check = ToolCheck {
            call_id: call_id_hint,
            name: tool_name,
            kind,
            title,
            input: args,
        };
        let _ = inputs_tx.send(AdapterInput::Gate {
            check,
            decision: GateDecision::Allow,
        });
        let reply = HookDecisionOutput::allow(Some("allowed by one-time retry grant".to_owned()));
        let _ = inputs_tx.send(AdapterInput::HookResponse {
            id: req_id,
            payload: serde_json::to_value(&reply).unwrap_or_default(),
        });
        let _ = send_hook_reply(&mut write_half, &reply).await;
        return;
    }

    let kind = classify_tool(&tool_name);
    let title = extract_tool_title(&tool_name, &args);
    let check = ToolCheck {
        call_id: call_id_hint,
        name: tool_name.clone(),
        kind,
        title,
        input: args.clone(),
    };

    let decision = gate.check(&check).await;
    let _ = inputs_tx.send(AdapterInput::Gate {
        check,
        decision: decision.clone(),
    });

    let reply = match decision {
        GateDecision::Allow => HookDecisionOutput::allow(None),
        GateDecision::Deny { reason } => HookDecisionOutput::deny(reason),
        GateDecision::Ask => match approval_mode {
            ApprovalMode::Block => {
                let (tx, rx) = oneshot::channel();
                {
                    let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
                    guard.approvals.insert(
                        req_id.clone(),
                        PendingHookApproval {
                            session_id,
                            tool_name,
                            args,
                            responder: Some(tx),
                        },
                    );
                }
                rx.await.unwrap_or_else(|_| {
                    HookDecisionOutput::deny("approval cancelled or adapter dropped")
                })
            }
            ApprovalMode::DenyAndRetry => {
                {
                    let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
                    guard.approvals.insert(
                        req_id.clone(),
                        PendingHookApproval {
                            session_id,
                            tool_name,
                            args,
                            responder: None,
                        },
                    );
                }
                HookDecisionOutput::deny(format!(
                    "approval required ({req_id}): waiting for user approval before retry"
                ))
            }
        },
    };

    let _ = inputs_tx.send(AdapterInput::HookResponse {
        id: req_id,
        payload: serde_json::to_value(&reply).unwrap_or_default(),
    });
    let _ = send_hook_reply(&mut write_half, &reply).await;
}

async fn send_hook_reply(
    write_half: &mut tokio::net::unix::OwnedWriteHalf,
    reply: &HookDecisionOutput,
) -> io::Result<()> {
    let mut line = serde_json::to_string(reply).unwrap_or_else(|_| {
        r#"{"decision":"deny","reason":"failed to serialize hook reply"}"#.to_owned()
    });
    line.push('\n');
    write_half.write_all(line.as_bytes()).await?;
    write_half.flush().await?;
    Ok(())
}

/// Client implementation executed by `spike-agy-hook`.
/// Connects to `socket_path`, sends `stdin_bytes`, waits up to `timeout`, and returns
/// a [`HookDecisionOutput`]. **Every error path fails closed (`deny`).**
pub async fn run_hook_client(
    socket_path: &Path,
    stdin_bytes: &[u8],
    timeout: Duration,
) -> HookDecisionOutput {
    match tokio::time::timeout(timeout, run_hook_client_inner(socket_path, stdin_bytes)).await {
        Ok(decision) => decision,
        Err(_) => HookDecisionOutput::deny("hook client timed out waiting for Atrium gate bridge"),
    }
}

async fn run_hook_client_inner(socket_path: &Path, stdin_bytes: &[u8]) -> HookDecisionOutput {
    let Ok(input_str) = std::str::from_utf8(stdin_bytes) else {
        return HookDecisionOutput::deny("hook stdin is not valid UTF-8");
    };
    let trimmed = input_str.trim();
    if trimmed.is_empty() {
        return HookDecisionOutput::deny("hook stdin is empty");
    }
    let Ok(parsed) = serde_json::from_str::<Value>(trimmed) else {
        return HookDecisionOutput::deny("hook stdin is not valid JSON");
    };
    let has_tool_name = parsed
        .get("toolCall")
        .and_then(|tc| tc.get("name"))
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty());
    if !has_tool_name {
        return HookDecisionOutput::deny("malformed hook request: missing toolCall.name");
    }

    let mut stream = match UnixStream::connect(socket_path).await {
        Ok(s) => s,
        Err(e) => {
            return HookDecisionOutput::deny(format!(
                "failed to connect to Atrium gate socket: {e}"
            ));
        }
    };

    let mut wire_line = parsed.to_string();
    wire_line.push('\n');
    if let Err(e) = stream.write_all(wire_line.as_bytes()).await {
        return HookDecisionOutput::deny(format!("failed to write to Atrium gate socket: {e}"));
    }
    if let Err(e) = stream.flush().await {
        return HookDecisionOutput::deny(format!("failed to flush Atrium gate socket: {e}"));
    }

    let mut reader = BufReader::new(stream);
    let mut reply_line = String::new();
    match reader.read_line(&mut reply_line).await {
        Ok(0) => {
            return HookDecisionOutput::deny(
                "Atrium gate bridge closed connection before replying",
            );
        }
        Err(e) => {
            return HookDecisionOutput::deny(format!(
                "failed to read reply from Atrium gate socket: {e}"
            ));
        }
        Ok(_) => {}
    }

    let Ok(reply) = serde_json::from_str::<HookDecisionOutput>(reply_line.trim()) else {
        return HookDecisionOutput::deny("invalid JSON reply from Atrium gate bridge");
    };

    match reply.decision.as_str() {
        "allow" | "deny" => reply,
        other => HookDecisionOutput::deny(format!(
            "unexpected decision '{other}' from Atrium gate bridge"
        )),
    }
}

/// Creates a unique session-private directory with mode `0700`.
pub fn create_private_session_dir(base_dir: &Path, prefix: &str) -> io::Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    fs::create_dir_all(base_dir)?;
    let pid = std::process::id();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
    let short_prefix: String = prefix.chars().take(6).collect();
    let dir = base_dir.join(format!("{short_prefix}-{pid:x}-{seq:x}"));
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

/// Writes the per-session `PreToolUse` hook config (matcher `"*"`) into `session_dir`
/// (both `session_dir/hooks.json` and `session_dir/<agents_dir>/hooks.json`), with mode `0600`.
pub fn write_session_hook_config(
    session_dir: &Path,
    helper_bin: &Path,
    socket_path: &Path,
    timeout_secs: u64,
) -> io::Result<PathBuf> {
    fs::create_dir_all(session_dir)?;
    fs::set_permissions(session_dir, fs::Permissions::from_mode(0o700))?;

    let cmd = format!(
        "'{}' --socket '{}'",
        helper_bin.display(),
        socket_path.display()
    );
    let config_json = json!({
        "atrium-gate-bridge": {
            "PreToolUse": [
                {
                    "matcher": "*",
                    "hooks": [
                        {
                            "type": "command",
                            "command": cmd,
                            "timeout": timeout_secs
                        }
                    ]
                }
            ]
        }
    });
    let pretty = serde_json::to_string_pretty(&config_json).map_err(io::Error::other)?;

    let direct_path = session_dir.join("hooks.json");
    fs::write(&direct_path, &pretty)?;
    fs::set_permissions(&direct_path, fs::Permissions::from_mode(0o600))?;

    let agents_subdir_name = [".", "agents"].concat();
    let agents_dir = session_dir.join(agents_subdir_name);
    fs::create_dir_all(&agents_dir)?;
    fs::set_permissions(&agents_dir, fs::Permissions::from_mode(0o700))?;
    let nested_path = agents_dir.join("hooks.json");
    fs::write(&nested_path, &pretty)?;
    fs::set_permissions(&nested_path, fs::Permissions::from_mode(0o600))?;

    Ok(direct_path)
}

/// Writes per-session MCP server definitions into `session_dir` without touching global `~/.gemini`.
pub fn write_session_mcp_config(session_dir: &Path, servers: &[McpServer]) -> io::Result<PathBuf> {
    fs::create_dir_all(session_dir)?;
    fs::set_permissions(session_dir, fs::Permissions::from_mode(0o700))?;

    let mut mcp_servers_map = serde_json::Map::new();
    for srv in servers {
        let env_map: serde_json::Map<String, Value> = srv
            .env
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        mcp_servers_map.insert(
            srv.name.clone(),
            json!({
                "command": srv.command,
                "args": srv.args,
                "env": env_map,
            }),
        );
    }

    let mcp_doc = json!({
        "mcpServers": mcp_servers_map
    });
    let pretty = serde_json::to_string_pretty(&mcp_doc).map_err(io::Error::other)?;

    let mcp_path = session_dir.join("mcp.json");
    fs::write(&mcp_path, &pretty)?;
    fs::set_permissions(&mcp_path, fs::Permissions::from_mode(0o600))?;

    let agents_subdir_name = [".", "agents"].concat();
    let agents_dir = session_dir.join(agents_subdir_name);
    fs::create_dir_all(&agents_dir)?;
    let nested_mcp = agents_dir.join("mcp.json");
    fs::write(&nested_mcp, &pretty)?;
    fs::set_permissions(&nested_mcp, fs::Permissions::from_mode(0o600))?;

    let settings_path = agents_dir.join("settings.json");
    fs::write(&settings_path, &pretty)?;
    fs::set_permissions(&settings_path, fs::Permissions::from_mode(0o600))?;

    Ok(mcp_path)
}
