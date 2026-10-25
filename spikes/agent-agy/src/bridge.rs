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
    /// Set by [`GateBridge::lock_down`]: every later hook request is denied.
    locked_down: bool,
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
            locked_down: false,
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

    /// Denies every hook request from now on and cancels the pending ones. Used when the
    /// adapter detects that the gate may be bypassed and is about to kill the child.
    pub fn lock_down(&self) {
        {
            let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            guard.locked_down = true;
        }
        self.cancel_all();
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

    let locked_down = state.lock().unwrap_or_else(|e| e.into_inner()).locked_down;
    if locked_down {
        let reply =
            HookDecisionOutput::deny("session locked down by Atrium (gate bypass suspected)");
        let _ = send_hook_reply(&mut write_half, &reply).await;
        return;
    }

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

    let cmd = hook_command(helper_bin, socket_path);
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
    fs::set_permissions(&direct_path, fs::Permissions::from_mode(HOOK_FILE_MODE))?;

    let agents_dir = session_dir.join(agents_dir_name());
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

// ---------------------------------------------------------------------------------------------
// Hook config hardening (Fix round 2)
//
// Live finding on `agy` 1.4.7 (hook-failure harness): if the hook config is unreadable (mode
// 0000) or malformed JSON, `agy` silently ignores it and runs the tool under
// `--dangerously-skip-permissions`. That is fail-OPEN. The helper-side failures (helper
// missing, non-zero exit, invalid JSON output, timeout) fail closed. So the adapter must prove,
// before it spawns `agy`, that the config `agy` will read is present, private, parseable and
// really registers the gate hook, and must keep checking while the session runs.
// ---------------------------------------------------------------------------------------------

/// Required permission bits (`mode & 0o777`) of every hook config file the adapter writes.
pub const HOOK_FILE_MODE: u32 = 0o600;
/// Required permission bits of the session-private directories.
pub const HOOK_DIR_MODE: u32 = 0o700;

/// Name of the per-directory config folder `agy` reads hook configs from.
pub fn agents_dir_name() -> String {
    [".", "agents"].concat()
}

/// The shell command registered in `hooks.json` for the gate helper.
pub fn hook_command(helper_bin: &Path, socket_path: &Path) -> String {
    format!(
        "'{}' --socket '{}'",
        helper_bin.display(),
        socket_path.display()
    )
}

/// What a valid gate hook config must register.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookConfigExpectation {
    pub helper_bin: PathBuf,
    pub socket_path: PathBuf,
    pub timeout_secs: u64,
}

/// Which mode a directory holding a hook config must have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirRule {
    /// Exactly `0700` (directories the adapter created inside its session dir).
    Private,
    /// Not writable by group or others (a project's own config directory; it may be `0755`).
    NoGroupOtherWrite,
}

/// Why a hook config (or the helper it points to) was rejected. Every variant means
/// "refuse to start / abort the session": none of them is recoverable by trying `agy` anyway.
#[derive(Debug)]
pub enum HookConfigError {
    /// The file or directory cannot be stat'ed (missing, no search permission on the parent).
    Missing { path: PathBuf, source: io::Error },
    /// The path is a symlink or something other than a regular file.
    NotRegularFile { path: PathBuf },
    /// The path is a symlink or something other than a directory.
    NotADirectory { path: PathBuf },
    /// The file mode is not exactly [`HOOK_FILE_MODE`] (covers mode 0000 and group/world access).
    BadFileMode {
        path: PathBuf,
        actual: u32,
        expected: u32,
    },
    /// The directory mode violates `rule`.
    BadDirMode {
        path: PathBuf,
        actual: u32,
        rule: DirRule,
    },
    /// The file could not be read although its mode looked right.
    Unreadable { path: PathBuf, source: io::Error },
    /// The file is not valid JSON or has the wrong top-level shape.
    Malformed { path: PathBuf, detail: String },
    /// The JSON parses but does not register the expected `PreToolUse` command hook.
    GateHookAbsent { path: PathBuf, detail: String },
    /// A hook config already exists in the working directory, so ours would not be used.
    CwdHooksPresent { path: PathBuf },
    /// The helper binary is missing or not executable.
    HelperUnusable { path: PathBuf, detail: String },
}

impl std::fmt::Display for HookConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing { path, source } => {
                write!(f, "{} cannot be inspected: {source}", path.display())
            }
            Self::NotRegularFile { path } => {
                write!(f, "{} is not a regular file (symlink?)", path.display())
            }
            Self::NotADirectory { path } => {
                write!(f, "{} is not a directory (symlink?)", path.display())
            }
            Self::BadFileMode {
                path,
                actual,
                expected,
            } => write!(
                f,
                "{} has mode {actual:04o}, expected {expected:04o}",
                path.display()
            ),
            Self::BadDirMode { path, actual, rule } => write!(
                f,
                "{} has mode {actual:04o}, which violates the {rule:?} rule",
                path.display()
            ),
            Self::Unreadable { path, source } => {
                write!(f, "{} cannot be read: {source}", path.display())
            }
            Self::Malformed { path, detail } => {
                write!(f, "{} is malformed: {detail}", path.display())
            }
            Self::GateHookAbsent { path, detail } => write!(
                f,
                "{} does not register the Atrium gate hook: {detail}",
                path.display()
            ),
            Self::CwdHooksPresent { path } => write!(
                f,
                "{} already exists, so Atrium's gate hook would not be installed (agy ignores a \
                 hook config it does not read, and runs the tool ungated)",
                path.display()
            ),
            Self::HelperUnusable { path, detail } => {
                write!(f, "gate helper {} is unusable: {detail}", path.display())
            }
        }
    }
}

impl std::error::Error for HookConfigError {}

fn check_dir(path: &Path, rule: DirRule) -> Result<(), HookConfigError> {
    let meta = fs::symlink_metadata(path).map_err(|source| HookConfigError::Missing {
        path: path.to_path_buf(),
        source,
    })?;
    if !meta.file_type().is_dir() {
        return Err(HookConfigError::NotADirectory {
            path: path.to_path_buf(),
        });
    }
    let actual = meta.permissions().mode() & 0o777;
    let ok = match rule {
        DirRule::Private => actual == HOOK_DIR_MODE,
        DirRule::NoGroupOtherWrite => actual & 0o022 == 0,
    };
    if ok {
        Ok(())
    } else {
        Err(HookConfigError::BadDirMode {
            path: path.to_path_buf(),
            actual,
            rule,
        })
    }
}

fn find_gate_hook(doc: &Value, exp: &HookConfigExpectation) -> Result<(), String> {
    let root = doc
        .as_object()
        .ok_or_else(|| "top-level JSON value is not an object".to_owned())?;
    let want_cmd = hook_command(&exp.helper_bin, &exp.socket_path);
    let mut seen_pre_tool_use = false;
    for group in root.values() {
        let Some(entries) = group.get("PreToolUse").and_then(Value::as_array) else {
            continue;
        };
        seen_pre_tool_use = true;
        for entry in entries {
            if entry.get("matcher").and_then(Value::as_str) != Some("*") {
                continue;
            }
            let Some(hooks) = entry.get("hooks").and_then(Value::as_array) else {
                continue;
            };
            let hit = hooks.iter().any(|h| {
                h.get("type").and_then(Value::as_str) == Some("command")
                    && h.get("command").and_then(Value::as_str) == Some(want_cmd.as_str())
                    && h.get("timeout").and_then(Value::as_u64) == Some(exp.timeout_secs)
            });
            if hit {
                return Ok(());
            }
        }
    }
    Err(if seen_pre_tool_use {
        "no PreToolUse entry with matcher \"*\" runs the expected helper command with the \
         expected timeout"
            .to_owned()
    } else {
        "no PreToolUse entry".to_owned()
    })
}

/// Reads `path` back and proves it is what `agy` must see: a regular file (not a symlink)
/// with mode exactly [`HOOK_FILE_MODE`], inside a directory that satisfies `dir_rule`,
/// readable, valid JSON, registering a `PreToolUse` command hook with matcher `"*"` that runs
/// `exp`'s helper with `exp`'s socket and timeout.
pub fn validate_hook_config(
    path: &Path,
    exp: &HookConfigExpectation,
    dir_rule: DirRule,
) -> Result<(), HookConfigError> {
    if exp.helper_bin.to_string_lossy().contains('\'')
        || exp.socket_path.to_string_lossy().contains('\'')
    {
        return Err(HookConfigError::Malformed {
            path: path.to_path_buf(),
            detail: "helper or socket path contains a single quote and cannot be quoted safely"
                .to_owned(),
        });
    }
    if let Some(parent) = path.parent() {
        check_dir(parent, dir_rule)?;
    }
    let meta = fs::symlink_metadata(path).map_err(|source| HookConfigError::Missing {
        path: path.to_path_buf(),
        source,
    })?;
    if !meta.file_type().is_file() {
        return Err(HookConfigError::NotRegularFile {
            path: path.to_path_buf(),
        });
    }
    let actual = meta.permissions().mode() & 0o777;
    if actual != HOOK_FILE_MODE {
        return Err(HookConfigError::BadFileMode {
            path: path.to_path_buf(),
            actual,
            expected: HOOK_FILE_MODE,
        });
    }
    let text = fs::read_to_string(path).map_err(|source| HookConfigError::Unreadable {
        path: path.to_path_buf(),
        source,
    })?;
    let doc: Value = serde_json::from_str(&text).map_err(|e| HookConfigError::Malformed {
        path: path.to_path_buf(),
        detail: e.to_string(),
    })?;
    find_gate_hook(&doc, exp).map_err(|detail| {
        if doc.is_object() {
            HookConfigError::GateHookAbsent {
                path: path.to_path_buf(),
                detail,
            }
        } else {
            HookConfigError::Malformed {
                path: path.to_path_buf(),
                detail,
            }
        }
    })
}

/// Checks that the helper `agy` will run exists and is executable. A bare name is looked up
/// on `PATH`, as the shell running the hook command would.
pub fn check_helper_binary(helper: &Path) -> Result<(), HookConfigError> {
    let unusable = |detail: &str| HookConfigError::HelperUnusable {
        path: helper.to_path_buf(),
        detail: detail.to_owned(),
    };
    let candidates: Vec<PathBuf> = if helper.components().count() > 1 || helper.is_absolute() {
        vec![helper.to_path_buf()]
    } else {
        std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).map(|d| d.join(helper)).collect())
            .unwrap_or_default()
    };
    for cand in &candidates {
        if let Ok(meta) = fs::metadata(cand)
            && meta.is_file()
        {
            return if meta.permissions().mode() & 0o111 != 0 {
                Ok(())
            } else {
                Err(unusable("not executable"))
            };
        }
    }
    Err(unusable(
        "not found (build it with `cargo build -p spike-agent-agy --bins`)",
    ))
}

/// Everything the adapter verifies about the gate hook set-up, before spawning `agy` and again
/// while the session runs.
#[derive(Debug, Clone)]
pub struct GateSetup {
    pub session_dir: PathBuf,
    /// The copy in the working directory (the one `agy` actually reads), if the adapter
    /// installed one.
    pub cwd_hooks: Option<PathBuf>,
    pub expectation: HookConfigExpectation,
}

impl GateSetup {
    /// Reads every hook config back and checks it. Any error means: do not start / abort.
    pub fn verify(&self) -> Result<(), HookConfigError> {
        check_dir(&self.session_dir, DirRule::Private)?;
        validate_hook_config(
            &self.session_dir.join("hooks.json"),
            &self.expectation,
            DirRule::Private,
        )?;
        validate_hook_config(
            &self.session_dir.join(agents_dir_name()).join("hooks.json"),
            &self.expectation,
            DirRule::Private,
        )?;
        if let Some(cwd_hooks) = &self.cwd_hooks {
            validate_hook_config(cwd_hooks, &self.expectation, DirRule::NoGroupOtherWrite)?;
        }
        check_helper_binary(&self.expectation.helper_bin)
    }
}

/// What [`install_cwd_hook`] created, so cleanup removes exactly that and nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CwdHookInstall {
    pub hooks_path: PathBuf,
    /// The config directory, if the adapter created it.
    pub created_dir: Option<PathBuf>,
}

impl CwdHookInstall {
    /// Removes the installed file, and the directory if the adapter created it and it is empty.
    pub fn remove(&self) {
        let _ = fs::remove_file(&self.hooks_path);
        if let Some(dir) = &self.created_dir {
            let _ = fs::remove_dir(dir);
        }
    }
}

/// Installs a copy of the session's hook config as `<cwd>/<agents dir>/hooks.json` (mode 0600),
/// which is where `agy` looks for it. Refuses (and changes nothing) if a hook config is already
/// there: `agy` would read that one, not ours, and run tools without the Atrium gate.
pub fn install_cwd_hook(
    cwd: &Path,
    session_hooks: &Path,
) -> Result<CwdHookInstall, HookConfigError> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

    let agents_dir = cwd.join(agents_dir_name());
    let hooks_path = agents_dir.join("hooks.json");
    match fs::symlink_metadata(&hooks_path) {
        Ok(_) => return Err(HookConfigError::CwdHooksPresent { path: hooks_path }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(HookConfigError::Missing {
                path: hooks_path,
                source,
            });
        }
    }
    let contents =
        fs::read_to_string(session_hooks).map_err(|source| HookConfigError::Unreadable {
            path: session_hooks.to_path_buf(),
            source,
        })?;

    let mut created_dir = None;
    match fs::DirBuilder::new()
        .mode(HOOK_DIR_MODE)
        .create(&agents_dir)
    {
        Ok(()) => created_dir = Some(agents_dir.clone()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(source) => {
            return Err(HookConfigError::Missing {
                path: agents_dir,
                source,
            });
        }
    }
    let install = CwdHookInstall {
        hooks_path: hooks_path.clone(),
        created_dir,
    };
    let write = || -> io::Result<()> {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(HOOK_FILE_MODE)
            .open(&hooks_path)?;
        f.write_all(contents.as_bytes())?;
        f.flush()?;
        fs::set_permissions(&hooks_path, fs::Permissions::from_mode(HOOK_FILE_MODE))
    };
    if let Err(source) = write() {
        if source.kind() == io::ErrorKind::AlreadyExists {
            // Lost a race with someone else creating the file: it is not ours, leave it.
            if let Some(dir) = &install.created_dir {
                let _ = fs::remove_dir(dir);
            }
            return Err(HookConfigError::CwdHooksPresent { path: hooks_path });
        }
        install.remove();
        return Err(HookConfigError::Unreadable {
            path: hooks_path,
            source,
        });
    }
    Ok(install)
}
