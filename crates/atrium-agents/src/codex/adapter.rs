#![doc = "Codex app-server AgentAdapter implementation on the frozen M1 contract."]

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use atrium_host::supervise::{self, ProcessIdentity, SupervisedChild};
use atrium_protocol::agent::{
    AgentEvent, AgentEventEnvelope, ApprovalDecision, Provider, QuestionAnswer, RequestOutcome,
    SessionCommand,
};
use atrium_protocol::limits::{
    MAX_DIAGNOSTIC_LINE_BYTES, MAX_PARTIAL_ARGS_BYTES, MAX_PENDING_REQUESTS,
    MAX_PROVIDER_RECORD_BYTES, TERMINAL_TAIL_EVENTS,
};
use atrium_protocol::policy::{EnforcementDescriptor, EnforcementReason, ReasonCode};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use tokio::time::Instant as TokioInstant;

use super::normalizer::{CodexNormalizer, DEADLINE_INTERRUPT, DEADLINE_RECORD_TOO_LARGE, coverage};
use super::settings::{self, InspectInput};
use super::tools;
use crate::{
    AdapterError, AdapterInput, AgentAdapter, ApprovalTicket, BoxFuture, COMMAND_BUFFER,
    EVENT_BUFFER, GateDecision, McpServer, Normalizer, PermissionPolicy, SHUTDOWN_GRACE,
    STARTUP_TIMEOUT, Session, SessionConfig, ToolCheck, ToolGate, Transport,
};

/// Codex CLI version the M1 fixtures were recorded and tested against.
pub const TESTED_VERSIONS: &[&str] = &["0.163.0"];

const VERSION_TIMEOUT: Duration = Duration::from_secs(3);
const INTERRUPT_GRACE: Duration = Duration::from_secs(5);
const KILL_GRACE: Duration = Duration::from_secs(3);
const GATE_TIMEOUT: Duration = Duration::from_secs(300);
const REDEEM_TIMEOUT: Duration = Duration::from_secs(10);
const ALIVE_POLL: Duration = Duration::from_millis(200);
const WRITER_BUFFER: usize = 256;
const PENDING_LIMIT: usize = 4096;
/// Maximum number of inner `gate.check` / `gate.question_ticket` calls (and
/// their compensation withdrawals) that may be outstanding at once.
const MAX_IN_FLIGHT_GATE_CALLS: usize = MAX_PENDING_REQUESTS;

/// Codex-specific adapter options.
#[doc = "Telemetry / fixture tap callback receiving every raw [`AdapterInput`]."]
pub type InputTap = Arc<dyn Fn(&AdapterInput) + Send + Sync>;

/// Codex-specific adapter options.
#[derive(Clone, Default)]
pub struct CodexOptions {
    /// Optional Codex config profile (`-p <profile>`).
    pub profile: Option<String>,
    /// Extra CLI arguments placed before `app-server`.
    pub extra_args: Vec<String>,
    /// Override for the handshake startup timeout (defaults to [`STARTUP_TIMEOUT`]).
    pub startup_timeout: Option<Duration>,
    /// Override for the user config directory inspected for enforcement reporting.
    pub user_dir: Option<PathBuf>,
    /// Optional fixture / telemetry tap receiving every raw [`AdapterInput`].
    pub tap: Option<InputTap>,
}

impl std::fmt::Debug for CodexOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexOptions")
            .field("profile", &self.profile)
            .field("extra_args", &self.extra_args)
            .field("startup_timeout", &self.startup_timeout)
            .field("user_dir", &self.user_dir)
            .field("tap", &self.tap.as_ref().map(|_| ".."))
            .finish()
    }
}

/// Production adapter for `codex app-server --listen stdio://`.
#[derive(Debug, Clone, Default)]
pub struct CodexAdapter {
    pub options: CodexOptions,
}

impl CodexAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_options(options: CodexOptions) -> Self {
        Self { options }
    }
}

impl AgentAdapter for CodexAdapter {
    fn provider(&self) -> Provider {
        Provider::Codex
    }

    fn tested_versions(&self) -> &'static [&'static str] {
        TESTED_VERSIONS
    }

    fn probe<'a>(&'a self, config: &'a SessionConfig) -> BoxFuture<'a, EnforcementDescriptor> {
        Box::pin(async move {
            let program = resolve_program(config);
            let version = probe_version(&program, &config.env).await;
            build_descriptor(config, &self.options, version, false)
        })
    }

    fn start(
        &self,
        config: SessionConfig,
        transport: Transport,
    ) -> BoxFuture<'_, Result<Session, AdapterError>> {
        let options = self.options.clone();
        Box::pin(async move { start_session(config, transport, options).await })
    }
}

/// Builds the Codex CLI argument vector for `app-server` over stdio.
pub fn process_args(
    config: &SessionConfig,
    opts: &CodexOptions,
) -> Result<Vec<String>, AdapterError> {
    let mut args = Vec::new();
    if let Some(profile) = &opts.profile
        && !profile.is_empty()
    {
        args.push("-p".to_owned());
        args.push(profile.clone());
    }
    let mut seen_names = HashSet::new();
    for server in &config.mcp_servers {
        validate_mcp_name(&server.name)?;
        if !seen_names.insert(server.name.as_str()) {
            return Err(AdapterError::Unsupported(format!(
                "duplicate MCP server name: {}",
                server.name
            )));
        }
        append_mcp_overrides(&mut args, server)?;
    }
    args.extend(opts.extra_args.iter().cloned());
    args.push("app-server".to_owned());
    args.push("--listen".to_owned());
    args.push("stdio://".to_owned());
    Ok(args)
}

fn expected_thread_policy(permissions: PermissionPolicy) -> (&'static str, &'static str) {
    match permissions {
        PermissionPolicy::ReadOnly => ("read-only", "untrusted"),
        PermissionPolicy::Ask => ("workspace-write", "untrusted"),
        PermissionPolicy::FullAccess => ("danger-full-access", "never"),
    }
}

fn normalize_sandbox_name(s: &str) -> &str {
    match s {
        "readOnly" | "read-only" => "read-only",
        "workspaceWrite" | "workspace-write" => "workspace-write",
        "dangerFullAccess" | "danger-full-access" => "danger-full-access",
        other => other,
    }
}

fn verify_effective_policy(permissions: PermissionPolicy, result: &Value) -> Result<(), String> {
    let (expected_sandbox, expected_approval) = expected_thread_policy(permissions);
    let actual_approval = result
        .get("approvalPolicy")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            format!(
                "codex thread response omitted or non-string approvalPolicy (expected '{expected_approval}')"
            )
        })?;
    if actual_approval != expected_approval {
        return Err(format!(
            "codex effective approvalPolicy mismatch: requested '{expected_approval}', got '{actual_approval}'"
        ));
    }
    let sandbox_raw = result
        .pointer("/sandbox/type")
        .and_then(Value::as_str)
        .or_else(|| result.get("sandbox").and_then(Value::as_str))
        .ok_or_else(|| {
            format!("codex thread response omitted sandbox type (expected '{expected_sandbox}')")
        })?;
    let actual_sandbox = normalize_sandbox_name(sandbox_raw);
    if actual_sandbox != expected_sandbox {
        return Err(format!(
            "codex effective sandbox mismatch: requested '{expected_sandbox}', got '{sandbox_raw}'"
        ));
    }
    Ok(())
}

/// Builds `thread/start` or `thread/resume` parameters implementing D-050 and D-088.
pub fn thread_params(config: &SessionConfig) -> Value {
    let (sandbox, approval_policy) = expected_thread_policy(config.permissions);
    let mut params = json!({
        "cwd": config.cwd.display().to_string(),
        "sandbox": sandbox,
        "approvalPolicy": approval_policy,
        "approvalsReviewer": "user",
    });
    if let Some(model) = &config.model
        && !model.is_empty()
    {
        params["model"] = json!(model);
    }
    if let Some(thread_id) = &config.resume
        && !thread_id.is_empty()
    {
        params["threadId"] = json!(thread_id);
    }
    params
}

fn validate_mcp_name(name: &str) -> Result<(), AdapterError> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(AdapterError::Unsupported(format!(
            "invalid Codex MCP server name '{name}': only ASCII letters, digits, '_' and '-' are supported"
        )));
    }
    Ok(())
}

fn toml_literal<T: serde::Serialize>(val: &T) -> Result<String, AdapterError> {
    toml::to_string(&json!({"v": val}))
        .map_err(|e| {
            AdapterError::Unsupported(format!("cannot encode Codex override as TOML: {e}"))
        })?
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("v = "))
        .map(str::to_owned)
        .ok_or_else(|| AdapterError::Unsupported("unexpected TOML serialization".into()))
}

fn append_mcp_overrides(args: &mut Vec<String>, server: &McpServer) -> Result<(), AdapterError> {
    let prefix = format!("mcp_servers.{}", server.name);
    let command = server.command.to_string_lossy();
    if command.trim().is_empty() {
        return Err(AdapterError::Unsupported(format!(
            "MCP server '{}' has an empty command",
            server.name
        )));
    }
    args.push("-c".into());
    args.push(format!(
        "{prefix}.command={}",
        toml_literal(&command.as_ref())?
    ));
    if !server.args.is_empty() {
        args.push("-c".into());
        args.push(format!("{prefix}.args={}", toml_literal(&server.args)?));
    }
    if !server.env.is_empty() {
        let env_map: BTreeMap<&str, &str> = server
            .env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let mut pairs = Vec::with_capacity(env_map.len());
        for (k, v) in env_map {
            pairs.push(format!("{} = {}", toml_literal(&k)?, toml_literal(&v)?));
        }
        args.push("-c".into());
        args.push(format!("{prefix}.env={{ {} }}", pairs.join(", ")));
    }
    Ok(())
}

fn resolve_program(config: &SessionConfig) -> PathBuf {
    if let Some(p) = &config.program {
        return p.clone();
    }
    PathBuf::from("codex")
}

async fn probe_version(program: &Path, env: &[(String, String)]) -> Option<String> {
    let mut cmd = Command::new(program);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = tokio::time::timeout(VERSION_TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_version_output(&text)
}

fn parse_version_output(text: &str) -> Option<String> {
    for token in text.split_whitespace() {
        let trimmed = token
            .trim_start_matches('v')
            .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-');
        let mut parts = trimmed.split('.');
        if let (Some(a), Some(b)) = (parts.next(), parts.next())
            && !a.is_empty()
            && !b.is_empty()
            && a.chars().all(|c| c.is_ascii_digit())
            && b.chars().all(|c| c.is_ascii_digit())
        {
            return Some(trimmed.to_owned());
        }
    }
    None
}

fn build_descriptor(
    config: &SessionConfig,
    opts: &CodexOptions,
    cli_version: Option<String>,
    gate_failed: bool,
) -> EnforcementDescriptor {
    let user_dir_str = opts.user_dir.as_ref().map(|p| p.display().to_string());
    let inspect = settings::inspect(InspectInput {
        cwd: &config.cwd,
        env: &config.env,
        profile: opts.profile.as_deref(),
        args: &opts.extra_args,
        user_dir: user_dir_str.as_deref(),
    });
    let mut reasons = inspect.reasons;
    if gate_failed {
        reasons.push(EnforcementReason {
            code: ReasonCode::GateUnreachable,
            message: "Atrium policy gate timed out or panicked during the session".into(),
        });
    }
    let version_tested = cli_version
        .as_deref()
        .is_some_and(|v| TESTED_VERSIONS.contains(&v));
    EnforcementDescriptor::evaluate(
        Provider::Codex,
        cli_version,
        version_tested,
        coverage(),
        inspect.sources,
        reasons,
    )
}

// ---- transport & session runtime ----

enum Out {
    Stdout(String),
    Stderr(String),
    Oversize,
}

enum Internal {
    WriteFailed(String),
    Gate {
        native: String,
        check: ToolCheck,
        decision: GateDecision,
        failed: bool,
    },
    Redeem {
        native: String,
        ticket: ApprovalTicket,
        check: ToolCheck,
        decision: GateDecision,
        failed: bool,
    },
    Question {
        native: String,
        ticket: Option<ApprovalTicket>,
        questions: Value,
        failed: bool,
    },
}

struct InFlightCheck {
    count: Arc<AtomicUsize>,
    settled: Arc<Notify>,
    permit: Option<OwnedSemaphorePermit>,
}

impl InFlightCheck {
    fn try_enter(
        permits: &Arc<Semaphore>,
        count: &Arc<AtomicUsize>,
        settled: &Arc<Notify>,
    ) -> Option<Self> {
        let permit = permits.clone().try_acquire_owned().ok()?;
        count.fetch_add(1, Ordering::AcqRel);
        Some(Self {
            count: count.clone(),
            settled: settled.clone(),
            permit: Some(permit),
        })
    }

    fn take_permit(&mut self) -> Option<OwnedSemaphorePermit> {
        self.permit.take()
    }
}

impl Drop for InFlightCheck {
    fn drop(&mut self) {
        drop(self.permit.take());
        if self.count.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.settled.notify_waiters();
        }
    }
}

struct OrphanGuard {
    ticket: Option<ApprovalTicket>,
    withdraw_tx: mpsc::Sender<(ApprovalTicket, Option<OwnedSemaphorePermit>)>,
    gate: Arc<dyn ToolGate>,
    in_flight: Option<InFlightCheck>,
}

impl OrphanGuard {
    fn arm(
        withdraw_tx: mpsc::Sender<(ApprovalTicket, Option<OwnedSemaphorePermit>)>,
        gate: Arc<dyn ToolGate>,
        in_flight: Option<InFlightCheck>,
    ) -> Self {
        Self {
            ticket: None,
            withdraw_tx,
            gate,
            in_flight,
        }
    }

    fn set_ticket(&mut self, ticket: Option<ApprovalTicket>) {
        self.ticket = ticket;
    }

    fn disarm(&mut self) {
        self.ticket = None;
    }
}

impl Drop for OrphanGuard {
    fn drop(&mut self) {
        let mut in_flight = self.in_flight.take();
        if let Some(ticket) = self.ticket.take() {
            let permit = in_flight.as_mut().and_then(InFlightCheck::take_permit);
            match self.withdraw_tx.try_send((ticket, permit)) {
                Ok(()) => {}
                Err(
                    mpsc::error::TrySendError::Full((ticket, permit))
                    | mpsc::error::TrySendError::Closed((ticket, permit)),
                ) => {
                    if let Ok(handle) = tokio::runtime::Handle::try_current() {
                        let gate = self.gate.clone();
                        handle.spawn(async move {
                            let _keep_inflight = in_flight;
                            let _keep_permit = permit;
                            gate.withdraw(&ticket).await;
                        });
                    }
                }
            }
        }
    }
}

struct Proc {
    child: Option<SupervisedChild>,
    stdin_tx: Option<mpsc::Sender<String>>,
    writer: tokio::task::JoinHandle<()>,
    readers: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Proc {
    fn drop(&mut self) {
        self.stdin_tx = None;
        self.writer.abort();
        for r in self.readers.drain(..) {
            r.abort();
        }
        if let Some(mut child) = self.child.take() {
            if tokio::runtime::Handle::try_current().is_ok() {
                let _ = std::thread::Builder::new()
                    .name("atrium-codex-drop-reap".into())
                    .spawn(move || {
                        let _ = supervise::terminate_blocking(&mut child, KILL_GRACE);
                    });
            } else {
                let _ = supervise::terminate_blocking(&mut child, KILL_GRACE);
            }
        }
    }
}

#[derive(Clone)]
struct PendingApproval {
    raw_id: Value,
    native: String,
    method: String,
    call_id: Option<String>,
    name: String,
    payload: Value,
    ticket: ApprovalTicket,
}

#[derive(Clone)]
struct PendingQuestion {
    raw_id: Value,
    native: String,
    questions: Value,
    ticket: Option<ApprovalTicket>,
}

async fn start_session(
    config: SessionConfig,
    transport: Transport,
    options: CodexOptions,
) -> Result<Session, AdapterError> {
    match transport {
        Transport::Local => {}
    }

    let program = resolve_program(&config);
    let args = process_args(&config, &options)?;
    let initial_version = probe_version(&program, &config.env).await;
    let initial_descriptor = build_descriptor(&config, &options, initial_version.clone(), false);

    let mut cmd = Command::new(&program);
    cmd.args(&args)
        .current_dir(&config.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in &config.env {
        cmd.env(k, v);
    }

    let mut supervised = supervise::spawn_group(cmd).map_err(AdapterError::Spawn)?;
    let identity: ProcessIdentity = supervised.identity.clone();
    if let Some(cb) = &config.on_spawn {
        cb(&identity);
    }

    let stdin = supervised
        .child
        .stdin
        .take()
        .ok_or_else(|| AdapterError::Handshake("child has no stdin pipe".into()))?;
    let stdout = supervised
        .child
        .stdout
        .take()
        .ok_or_else(|| AdapterError::Handshake("child has no stdout pipe".into()))?;
    let stderr = supervised
        .child
        .stderr
        .take()
        .ok_or_else(|| AdapterError::Handshake("child has no stderr pipe".into()))?;

    let (out_tx, out_rx) = mpsc::channel(256);
    let (stdin_tx, stdin_rx) = mpsc::channel(WRITER_BUFFER);
    let (internal_tx, internal_rx) = mpsc::channel(128);
    let (withdraw_tx, withdraw_rx) = mpsc::channel(MAX_IN_FLIGHT_GATE_CALLS);

    let writer = tokio::spawn(run_writer(stdin, stdin_rx, internal_tx.clone()));
    let r_out = tokio::spawn(read_lines(stdout, out_tx.clone(), true));
    let r_err = tokio::spawn(read_lines(stderr, out_tx, false));

    let proc = Proc {
        child: Some(supervised),
        stdin_tx: Some(stdin_tx),
        writer,
        readers: vec![r_out, r_err],
    };

    let (events_tx, events_rx) = mpsc::channel(EVENT_BUFFER);
    let (cmd_tx, cmd_rx) = mpsc::channel(COMMAND_BUFFER);

    let mut driver = Driver {
        config: config.clone(),
        options,
        gate: config.gate.clone(),
        cli_version: initial_version,
        descriptor: initial_descriptor.clone(),
        norm: CodexNormalizer::new(),
        start: Instant::now(),
        seq: 0,
        pending: VecDeque::new(),
        closing_tail: VecDeque::new(),
        dropped_events: 0,
        events_tx,
        cmd_rx: Some(cmd_rx),
        out_rx: Some(out_rx),
        internal_tx,
        internal_rx,
        withdraw_tx,
        withdraw_rx,
        proc: Some(proc),
        items: HashMap::new(),
        queued_msgs: VecDeque::new(),
        approvals: HashMap::new(),
        questions: HashMap::new(),
        live: HashSet::new(),
        inflight: HashMap::new(),
        pending_withdrawals: Vec::new(),
        gate_permits: Arc::new(Semaphore::new(MAX_IN_FLIGHT_GATE_CALLS)),
        gate_in_flight: Arc::new(AtomicUsize::new(0)),
        gate_settled: Arc::new(Notify::new()),
        turn_seq: 0,
        interrupt_deadline: None,
        shutdown_deadline: None,
        shutting_down: false,
        eof_reached: false,
        gate_failed: false,
    };

    driver.feed(AdapterInput::Enforcement {
        descriptor: initial_descriptor,
    });

    if let Err(err) = driver.handshake().await {
        driver.discard();
        return Err(err);
    }

    let final_descriptor = driver.descriptor.clone();
    tokio::spawn(driver.run());

    Ok(Session {
        descriptor: final_descriptor,
        process: Some(identity),
        events: events_rx,
        commands: cmd_tx,
    })
}

async fn run_writer(
    mut stdin: ChildStdin,
    mut rx: mpsc::Receiver<String>,
    internal_tx: mpsc::Sender<Internal>,
) {
    while let Some(line) = rx.recv().await {
        if let Err(e) = stdin.write_all(line.as_bytes()).await {
            let _ = internal_tx.try_send(Internal::WriteFailed(e.to_string()));
            return;
        }
        if let Err(e) = stdin.write_all(b"\n").await {
            let _ = internal_tx.try_send(Internal::WriteFailed(e.to_string()));
            return;
        }
        if let Err(e) = stdin.flush().await {
            let _ = internal_tx.try_send(Internal::WriteFailed(e.to_string()));
            return;
        }
    }
    let _ = stdin.shutdown().await;
}

async fn read_lines<R: AsyncRead + Unpin>(reader: R, tx: mpsc::Sender<Out>, is_stdout: bool) {
    let mut buf = BufReader::new(reader);
    let max_bytes = if is_stdout {
        MAX_PROVIDER_RECORD_BYTES
    } else {
        MAX_DIAGNOSTIC_LINE_BYTES
    };
    let mut line_buf = Vec::new();
    loop {
        line_buf.clear();
        match read_capped_line(&mut buf, &mut line_buf, max_bytes).await {
            Ok(ReadLine::Eof) => break,
            Ok(ReadLine::Oversize) => {
                if is_stdout {
                    let _ = tx.send(Out::Oversize).await;
                    break;
                } else {
                    let s = String::from_utf8_lossy(&line_buf).into_owned();
                    if tx.send(Out::Stderr(s)).await.is_err() {
                        break;
                    }
                }
            }
            Ok(ReadLine::Line) => {
                let s = String::from_utf8_lossy(&line_buf).into_owned();
                let msg = if is_stdout {
                    Out::Stdout(s)
                } else {
                    Out::Stderr(s)
                };
                if tx.send(msg).await.is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

enum ReadLine {
    Eof,
    Line,
    Oversize,
}

async fn read_capped_line<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
    out: &mut Vec<u8>,
    max_bytes: usize,
) -> std::io::Result<ReadLine> {
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(if out.is_empty() {
                ReadLine::Eof
            } else {
                ReadLine::Line
            });
        }
        if let Some(pos) = available.iter().position(|&b| b == b'\n') {
            let take = pos;
            if out.len() + take > max_bytes {
                let space = max_bytes.saturating_sub(out.len());
                out.extend_from_slice(&available[..space]);
                reader.consume(pos + 1);
                return Ok(ReadLine::Oversize);
            }
            out.extend_from_slice(&available[..take]);
            if out.last() == Some(&b'\r') {
                out.pop();
            }
            reader.consume(pos + 1);
            return Ok(ReadLine::Line);
        }
        let len = available.len();
        if out.len() + len > max_bytes {
            let space = max_bytes.saturating_sub(out.len());
            out.extend_from_slice(&available[..space]);
            reader.consume(len);
            // Drain until newline or EOF so we know boundary cleanly, then report Oversize.
            loop {
                let more = reader.fill_buf().await?;
                if more.is_empty() {
                    break;
                }
                if let Some(p) = more.iter().position(|&b| b == b'\n') {
                    reader.consume(p + 1);
                    break;
                }
                let n = more.len();
                reader.consume(n);
            }
            return Ok(ReadLine::Oversize);
        }
        out.extend_from_slice(available);
        reader.consume(len);
    }
}

struct Driver {
    config: SessionConfig,
    options: CodexOptions,
    gate: Arc<dyn ToolGate>,
    cli_version: Option<String>,
    descriptor: EnforcementDescriptor,
    norm: CodexNormalizer,
    start: Instant,
    seq: u64,
    pending: VecDeque<AgentEventEnvelope>,
    closing_tail: VecDeque<AgentEventEnvelope>,
    dropped_events: u64,
    events_tx: mpsc::Sender<AgentEventEnvelope>,
    cmd_rx: Option<mpsc::Receiver<SessionCommand>>,
    out_rx: Option<mpsc::Receiver<Out>>,
    internal_tx: mpsc::Sender<Internal>,
    internal_rx: mpsc::Receiver<Internal>,
    withdraw_tx: mpsc::Sender<(ApprovalTicket, Option<OwnedSemaphorePermit>)>,
    withdraw_rx: mpsc::Receiver<(ApprovalTicket, Option<OwnedSemaphorePermit>)>,
    proc: Option<Proc>,
    items: HashMap<String, Value>,
    queued_msgs: VecDeque<String>,
    approvals: HashMap<String, PendingApproval>,
    questions: HashMap<String, PendingQuestion>,
    live: HashSet<String>,
    inflight: HashMap<String, tokio::task::JoinHandle<()>>,
    pending_withdrawals: Vec<(ApprovalTicket, Option<OwnedSemaphorePermit>)>,
    gate_permits: Arc<Semaphore>,
    gate_in_flight: Arc<AtomicUsize>,
    gate_settled: Arc<Notify>,
    turn_seq: u32,
    interrupt_deadline: Option<TokioInstant>,
    shutdown_deadline: Option<TokioInstant>,
    shutting_down: bool,
    eof_reached: bool,
    gate_failed: bool,
}

impl Driver {
    fn feed(&mut self, input: AdapterInput) {
        if let Some(tap) = &self.options.tap {
            tap(&input);
        }
        let events = self.norm.on_input(&input);
        self.enqueue_events(events);
    }

    fn is_pending_saturated(&self) -> bool {
        self.pending.len() >= PENDING_LIMIT || !self.closing_tail.is_empty()
    }

    fn take_dropped_error(&mut self) -> Option<AgentEvent> {
        if self.dropped_events == 0 {
            return None;
        }
        let count = std::mem::take(&mut self.dropped_events);
        Some(AgentEvent::Error {
            message: format!(
                "dropped {count} non-terminal event(s) because event buffer exceeded PENDING_LIMIT ({PENDING_LIMIT})"
            ),
            fatal: false,
            details: Vec::new(),
        })
    }

    fn push_tail_event(&mut self, ev: AgentEvent, at_ms: u64) {
        let is_ended = matches!(ev, AgentEvent::SessionEnded { .. });
        // Reserve one slot in `closing_tail` so `SessionEnded` can always be delivered.
        let limit = if is_ended {
            TERMINAL_TAIL_EVENTS
        } else {
            TERMINAL_TAIL_EVENTS.saturating_sub(1)
        };
        if is_ended && self.closing_tail.len() >= TERMINAL_TAIL_EVENTS {
            self.closing_tail.pop_front();
        }
        if self.closing_tail.len() < limit {
            self.closing_tail.push_back(AgentEventEnvelope {
                incarnation: 0,
                seq: self.seq,
                at_ms,
                event: ev,
            });
            self.seq += 1;
        }
    }

    fn emit_saturation_error(&mut self, message: String) {
        let at_ms = self.start.elapsed().as_millis() as u64;
        let ev = AgentEvent::Error {
            message,
            fatal: false,
            details: Vec::new(),
        };
        if !self.is_pending_saturated() {
            self.pending.push_back(AgentEventEnvelope {
                incarnation: 0,
                seq: self.seq,
                at_ms,
                event: ev,
            });
            self.seq += 1;
        } else {
            self.push_tail_event(ev, at_ms);
        }
    }

    fn cancel_overflow_request(&mut self, request_id: String, is_question: bool, at_ms: u64) {
        if is_question {
            if let Some(pending) = self.questions.remove(&request_id) {
                self.live.remove(&pending.native);
                self.decline_question_request(&pending.raw_id);
                if let Some(ticket) = pending.ticket {
                    self.queue_withdraw(ticket);
                }
            }
        } else if let Some(pending) = self.approvals.remove(&request_id) {
            self.live.remove(&pending.native);
            self.decline_server_request(&pending.raw_id, &pending.method);
            if !pending.ticket.request_id.is_empty() {
                self.queue_withdraw(pending.ticket);
            }
        }
        let kind = if is_question { "question" } else { "approval" };
        self.push_tail_event(
            AgentEvent::RequestResolved {
                request_id,
                outcome: RequestOutcome::Cancelled,
            },
            at_ms,
        );
        self.push_tail_event(
            AgentEvent::Error {
                message: format!(
                    "declined {kind} request because event buffer exceeded PENDING_LIMIT ({PENDING_LIMIT})"
                ),
                fatal: false,
                details: Vec::new(),
            },
            at_ms,
        );
    }

    fn enqueue_events(&mut self, events: Vec<AgentEvent>) {
        let at_ms = self.start.elapsed().as_millis() as u64;
        let mut overflow_requests: Vec<(String, bool)> = Vec::new();
        for ev in events {
            if !self.is_pending_saturated() {
                if let Some(err) = self.take_dropped_error() {
                    self.pending.push_back(AgentEventEnvelope {
                        incarnation: 0,
                        seq: self.seq,
                        at_ms,
                        event: err,
                    });
                    self.seq += 1;
                    if self.is_pending_saturated() {
                        match ev {
                            AgentEvent::ApprovalRequested { request_id, .. } => {
                                overflow_requests.push((request_id, false));
                            }
                            AgentEvent::QuestionAsked { request_id, .. } => {
                                overflow_requests.push((request_id, true));
                            }
                            other if is_closing_event(&other) => {
                                self.push_tail_event(other, at_ms);
                            }
                            _ => {
                                self.dropped_events += 1;
                            }
                        }
                        continue;
                    }
                }
                self.pending.push_back(AgentEventEnvelope {
                    incarnation: 0,
                    seq: self.seq,
                    at_ms,
                    event: ev,
                });
                self.seq += 1;
            } else {
                match ev {
                    AgentEvent::ApprovalRequested { request_id, .. } => {
                        overflow_requests.push((request_id, false));
                    }
                    AgentEvent::QuestionAsked { request_id, .. } => {
                        overflow_requests.push((request_id, true));
                    }
                    other if is_closing_event(&other) => {
                        if matches!(
                            other,
                            AgentEvent::TurnCompleted { .. } | AgentEvent::SessionEnded { .. }
                        ) && let Some(err) = self.take_dropped_error()
                        {
                            self.push_tail_event(err, at_ms);
                        }
                        self.push_tail_event(other, at_ms);
                    }
                    _ => {
                        self.dropped_events += 1;
                    }
                }
            }
        }
        for (req_id, is_question) in overflow_requests {
            self.cancel_overflow_request(req_id, is_question, at_ms);
        }
    }

    fn write_value(&mut self, value: &Value) {
        let line = value.to_string();
        self.feed(AdapterInput::Stdin { line: line.clone() });
        if let Some(proc) = &self.proc
            && let Some(tx) = &proc.stdin_tx
            && tx.try_send(line).is_err()
        {
            let _ = self
                .internal_tx
                .try_send(Internal::WriteFailed("stdin channel closed or full".into()));
        }
    }

    async fn handshake(&mut self) -> Result<(), AdapterError> {
        let timeout = self.options.startup_timeout.unwrap_or(STARTUP_TIMEOUT);
        let deadline = TokioInstant::now() + timeout;
        self.write_value(&json!({
            "id": "atrium-init",
            "method": "initialize",
            "params": {
                "clientInfo": {
                    "name": "atrium",
                    "title": "Atrium",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": {
                    "experimentalApi": true,
                }
            }
        }));
        self.wait_handshake_response("atrium-init", deadline)
            .await?;

        self.write_value(&json!({
            "method": "initialized",
        }));

        let method = if self.config.resume.is_some() {
            "thread/resume"
        } else {
            "thread/start"
        };
        let params = thread_params(&self.config);
        self.write_value(&json!({
            "id": "atrium-thread",
            "method": method,
            "params": params,
        }));
        self.wait_handshake_response("atrium-thread", deadline)
            .await?;

        // Update descriptor if `userAgent` or `cliVersion` resolved a version when `--version` did not.
        if self.descriptor.cli_version != self.cli_version && self.cli_version.is_some() {
            let updated = build_descriptor(
                &self.config,
                &self.options,
                self.cli_version.clone(),
                self.gate_failed,
            );
            if updated != self.descriptor {
                self.descriptor = updated.clone();
                self.feed(AdapterInput::Enforcement {
                    descriptor: updated,
                });
            }
        }

        Ok(())
    }

    async fn wait_handshake_response(
        &mut self,
        expected_id: &str,
        deadline: TokioInstant,
    ) -> Result<(), AdapterError> {
        let mut tail: VecDeque<String> = VecDeque::new();
        loop {
            let next = {
                let Some(rx) = self.out_rx.as_mut() else {
                    return Err(AdapterError::Handshake(
                        "codex stdout channel closed during handshake".into(),
                    ));
                };
                tokio::time::timeout_at(deadline, rx.recv()).await
            };
            match next {
                Ok(Some(Out::Stdout(line))) => {
                    let parsed = serde_json::from_str::<Value>(&line).ok();
                    if let Some(v) = &parsed
                        && let Some(ua) = v.pointer("/result/userAgent").and_then(Value::as_str)
                        && self.cli_version.is_none()
                    {
                        self.cli_version = parse_version_output(ua);
                    }
                    if let Some(v) = &parsed
                        && let Some(cv) = v
                            .pointer("/result/thread/cliVersion")
                            .and_then(Value::as_str)
                        && self.cli_version.is_none()
                    {
                        self.cli_version = Some(cv.to_owned());
                    }
                    let id_matches = parsed
                        .as_ref()
                        .and_then(|v| v.get("id"))
                        .and_then(Value::as_str)
                        == Some(expected_id);
                    let rpc_err = parsed
                        .as_ref()
                        .filter(|_| id_matches)
                        .and_then(|v| v.get("error"))
                        .filter(|e| !e.is_null())
                        .and_then(|e| e.get("message").and_then(Value::as_str))
                        .map(str::to_owned);
                    let policy_err =
                        if id_matches && rpc_err.is_none() && expected_id == "atrium-thread" {
                            parsed
                                .as_ref()
                                .and_then(|v| v.get("result"))
                                .map(|res| verify_effective_policy(self.config.permissions, res))
                                .unwrap_or_else(|| {
                                    Err("codex thread response missing result object".into())
                                })
                                .err()
                        } else {
                            None
                        };
                    if let Some(msg) = policy_err {
                        let code = self.stop_process(Duration::ZERO).await;
                        self.feed(AdapterInput::Stderr {
                            line: format!("atrium: {msg}"),
                        });
                        self.feed(AdapterInput::Exit { code });
                        return Err(AdapterError::Handshake(msg));
                    }
                    self.feed(AdapterInput::Stdout { line });
                    if let Some(err_msg) = rpc_err {
                        let code = self.stop_process(Duration::ZERO).await;
                        self.feed(AdapterInput::Exit { code });
                        return Err(AdapterError::Handshake(err_msg));
                    }
                    if id_matches {
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
                    let code = self.stop_process(Duration::ZERO).await;
                    self.feed(AdapterInput::Deadline {
                        what: DEADLINE_RECORD_TOO_LARGE.into(),
                    });
                    self.feed(AdapterInput::Exit { code });
                    return Err(AdapterError::Handshake(format!(
                        "codex sent a record over {MAX_PROVIDER_RECORD_BYTES} bytes"
                    )));
                }
                Ok(None) => {
                    let code = self.stop_process(Duration::ZERO).await;
                    self.feed(AdapterInput::Exit { code });
                    let msg: Vec<String> = tail.into_iter().collect();
                    return Err(AdapterError::Handshake(format!(
                        "codex exited before answering {expected_id} (code {code:?}): {}",
                        msg.join(" | ")
                    )));
                }
                Err(_) => {
                    let code = self.stop_process(Duration::ZERO).await;
                    self.feed(AdapterInput::Deadline {
                        what: "startup".into(),
                    });
                    self.feed(AdapterInput::Exit { code });
                    return Err(AdapterError::Handshake(format!(
                        "codex did not answer {expected_id} within {:?}",
                        self.options.startup_timeout.unwrap_or(STARTUP_TIMEOUT)
                    )));
                }
            }
        }
    }

    fn discard(&mut self) {
        self.pending.clear();
        self.closing_tail.clear();
        self.dropped_events = 0;
        self.clear_requests();
    }

    async fn run(mut self) {
        loop {
            if self.norm.has_ended() {
                // Break before flushing so `SessionEnded` is only delivered in
                // `flush_final` after `clear_requests` and `drain_withdrawals`
                // have settled all pending tickets.
                break;
            }

            self.drain_withdrawals().await;
            self.flush_pending();

            if self.norm.wants_end() && !self.shutting_down {
                self.begin_shutdown(Duration::ZERO).await;
            }

            if self.norm.has_ended() {
                break;
            }

            let paused_stdout = self.is_pending_saturated() && !self.shutting_down;
            let has_pending = !self.pending.is_empty() || !self.closing_tail.is_empty();
            let events_tx = self.events_tx.clone();
            let next_deadline = earliest_deadline(self.interrupt_deadline, self.shutdown_deadline);

            tokio::select! {
                biased;

                // Drain events channel capacity whenever events are waiting.
                Ok(permit) = events_tx.reserve(), if has_pending => {
                    if let Some(ev) = self.pending.pop_front().or_else(|| self.closing_tail.pop_front()) {
                        permit.send(ev);
                    }
                    self.flush_pending();
                }

                cmd = recv_cmd(&mut self.cmd_rx) => {
                    match cmd {
                        Some(c) => self.on_command(c).await,
                        None => {
                            // All Session::commands senders dropped: wait cleanly unless already stopping,
                            // or terminate if nothing else keeps the session.
                            self.cmd_rx = None;
                            if !self.shutting_down {
                                self.begin_shutdown(SHUTDOWN_GRACE).await;
                            }
                        }
                    }
                }

                internal = self.internal_rx.recv() => {
                    if let Some(msg) = internal {
                        self.on_internal(msg).await;
                    }
                }

                withdraw = self.withdraw_rx.recv() => {
                    if let Some((ticket, permit)) = withdraw {
                        self.queue_withdraw_with_permit(ticket, permit);
                    }
                }

                out = recv_out(&mut self.out_rx), if !paused_stdout => {
                    match out {
                        Some(msg) => self.on_out(msg),
                        None => {
                            self.out_rx = None;
                            self.eof_reached = true;
                            let code = self.stop_process(Duration::ZERO).await;
                            self.feed(AdapterInput::Exit { code });
                        }
                    }
                }

                _ = wait_until(next_deadline), if next_deadline.is_some() => {
                    self.on_deadline().await;
                }

                _ = tokio::time::sleep(ALIVE_POLL), if self.eof_reached && self.proc.is_some() => {
                    let code = self.stop_process(Duration::ZERO).await;
                    self.feed(AdapterInput::Exit { code });
                }
            }
        }

        self.cmd_rx = None;
        self.clear_requests();
        self.wait_for_in_flight_gates().await;
        // Close `internal_rx` and `withdraw_rx` before the final drain so late
        // `OrphanGuard` drops during `flush_final` take the direct withdrawal
        // fallback instead of queuing into a channel that will never be read again.
        self.internal_rx.close();
        self.withdraw_rx.close();
        self.drain_internal_withdrawals();
        self.drain_withdrawals().await;
        self.flush_final().await;
    }

    fn flush_pending(&mut self) {
        loop {
            if self.pending.is_empty()
                && self.closing_tail.is_empty()
                && let Some(err) = self.take_dropped_error()
            {
                let at_ms = self.start.elapsed().as_millis() as u64;
                self.pending.push_back(AgentEventEnvelope {
                    incarnation: 0,
                    seq: self.seq,
                    at_ms,
                    event: err,
                });
                self.seq += 1;
            }
            let Some(front) = self.pending.front().or_else(|| self.closing_tail.front()) else {
                break;
            };
            let is_main = !self.pending.is_empty();
            if self.events_tx.try_send(front.clone()).is_ok() {
                if is_main {
                    self.pending.pop_front();
                } else {
                    self.closing_tail.pop_front();
                }
            } else {
                break;
            }
        }
    }

    async fn flush_final(&mut self) {
        // Send any buffered normal events and terminal tail without blocking indefinitely if receiver closed.
        while let Some(ev) = self
            .pending
            .pop_front()
            .or_else(|| self.closing_tail.pop_front())
        {
            if self.events_tx.send(ev).await.is_err() {
                return;
            }
        }
        if let Some(err) = self.take_dropped_error() {
            let at_ms = self.start.elapsed().as_millis() as u64;
            let env = AgentEventEnvelope {
                incarnation: 0,
                seq: self.seq,
                at_ms,
                event: err,
            };
            self.seq += 1;
            let _ = self.events_tx.send(env).await;
        }
    }

    fn on_out(&mut self, msg: Out) {
        match msg {
            Out::Stdout(line) => {
                let parsed = serde_json::from_str::<Value>(&line).ok();
                self.feed(AdapterInput::Stdout { line });
                if let Some(v) = parsed {
                    self.handle_provider_json(&v);
                }
            }
            Out::Stderr(line) => {
                self.feed(AdapterInput::Stderr { line });
            }
            Out::Oversize => {
                self.feed(AdapterInput::Deadline {
                    what: DEADLINE_RECORD_TOO_LARGE.into(),
                });
            }
        }
    }

    fn handle_provider_json(&mut self, v: &Value) {
        let Some(method) = v.get("method").and_then(Value::as_str) else {
            return;
        };
        if let Some(id_val) = v.get("id").filter(|i| !i.is_null()) {
            self.on_server_request(id_val, method, &v["params"]);
            return;
        }
        match method {
            "item/started" => {
                if let Some(item) = v.pointer("/params/item")
                    && let Some(id) = item.get("id").and_then(Value::as_str)
                {
                    self.items.insert(id.to_owned(), item.clone());
                }
            }
            "item/completed" => {
                if let Some(id) = v.pointer("/params/item/id").and_then(Value::as_str) {
                    self.items.remove(id);
                }
            }
            "serverRequest/resolved" => {
                if let Some(req_id) = v.pointer("/params/requestId") {
                    let native = id_key(req_id);
                    self.forget_native(&native);
                }
            }
            "turn/completed" => {
                self.interrupt_deadline = None;
                self.clear_requests();
                self.dispatch_next();
            }
            _ => {}
        }
    }

    fn on_server_request(&mut self, id_val: &Value, method: &str, params: &Value) {
        let native = id_key(id_val);
        if self.live.contains(&native)
            || self.live.len() + self.approvals.len() + self.questions.len() >= MAX_PENDING_REQUESTS
        {
            self.decline_native_request(id_val, method);
            return;
        }

        if matches!(
            method,
            "execCommandApproval" | "applyPatchApproval" | "item/permissions/requestApproval"
        ) {
            // Decline unmapped legacy / turn-scoped permission requests until verified live (X3).
            self.decline_server_request(id_val, method);
            return;
        }

        if self.is_pending_saturated() {
            self.decline_native_request(id_val, method);
            self.emit_saturation_error(format!(
                "declined {method} because event buffer exceeded PENDING_LIMIT ({PENDING_LIMIT})"
            ));
            return;
        }

        if method == "item/tool/requestUserInput" {
            let turn = self.norm.turn_number();
            let turn_arg = (turn > 0).then_some(turn);
            let questions = params.get("questions").cloned().unwrap_or(Value::Null);
            self.live.insert(native.clone());
            self.spawn_question(id_val.clone(), native, questions, turn_arg);
            return;
        }

        let Some((name, call_id)) = approval_method_name(method, params) else {
            // Unsupported server request: answer with JSON-RPC Method not found error cleanly.
            self.write_value(&json!({
                "id": id_val,
                "error": {
                    "code": -32601,
                    "message": format!("unsupported server request: {method}"),
                }
            }));
            return;
        };

        let mut payload = params.clone();
        if let Some(id) = call_id.as_deref()
            && let Some(item) = self.items.get(id)
            && !payload.get("item").is_some_and(|i| !i.is_null())
        {
            payload["item"] = item.clone();
        }

        if method == "item/fileChange/requestApproval" && has_grant_root(&payload) {
            // Decline writable-root expansion requests until X1/X3 verifies `accept` semantics.
            self.decline_server_request(id_val, method);
            return;
        }

        if payload.to_string().len() > MAX_PARTIAL_ARGS_BYTES {
            self.decline_server_request(id_val, method);
            return;
        }

        let turn = self.norm.turn_number();
        let cwd = self.config.cwd.display().to_string();
        let check = tools::tool_check(call_id, name, &payload, &cwd, turn);

        self.live.insert(native.clone());
        self.spawn_gate(id_val.clone(), native, method.to_owned(), payload, check);
    }

    fn spawn_gate(
        &mut self,
        raw_id: Value,
        native: String,
        method: String,
        payload: Value,
        check: ToolCheck,
    ) {
        let Some(in_flight) =
            InFlightCheck::try_enter(&self.gate_permits, &self.gate_in_flight, &self.gate_settled)
        else {
            self.live.remove(&native);
            self.feed(AdapterInput::Gate {
                check,
                decision: GateDecision::Deny {
                    reason: "too many pending gate checks".into(),
                },
            });
            self.decline_server_request(&raw_id, &method);
            return;
        };
        let gate = self.gate.clone();
        let tx = self.internal_tx.clone();
        let withdraw_tx = self.withdraw_tx.clone();
        let key = native.clone();
        let check_gate = check.clone();
        let handle = tokio::spawn(async move {
            let gate_orphan = gate.clone();
            let (res_tx, res_rx) = oneshot::channel();
            // Run `gate.check` to completion without aborting on cancellation
            // so an already-inserted ledger ticket cannot leak.
            tokio::spawn(async move {
                let mut guard = OrphanGuard::arm(withdraw_tx, gate_orphan, Some(in_flight));
                let d = gate.check(&check_gate).await;
                if let GateDecision::Ask { ticket } = &d {
                    guard.set_ticket(Some(ticket.clone()));
                }
                let _ = res_tx.send((d, guard));
            });
            let (decision, failed) = match tokio::time::timeout(GATE_TIMEOUT, res_rx).await {
                Ok(Ok((d, mut guard))) => {
                    if tx
                        .send(Internal::Gate {
                            native: key,
                            check,
                            decision: d,
                            failed: false,
                        })
                        .await
                        .is_ok()
                    {
                        guard.disarm();
                    }
                    return;
                }
                Ok(Err(_)) | Err(_) => (
                    GateDecision::Deny {
                        reason: "Atrium policy gate timed out or panicked".into(),
                    },
                    true,
                ),
            };
            let _ = tx
                .send(Internal::Gate {
                    native: key,
                    check,
                    decision,
                    failed,
                })
                .await;
        });
        // Keep metadata ready so `on_internal(Internal::Gate)` can inspect raw_id/method/payload.
        self.approvals.insert(
            format!("pending:{native}"),
            PendingApproval {
                raw_id,
                native: native.clone(),
                method,
                call_id: None,
                name: String::new(),
                payload,
                ticket: ApprovalTicket {
                    request_id: String::new(),
                    rememberable: false,
                },
            },
        );
        self.inflight.insert(format!("gate:{native}"), handle);
    }

    fn spawn_redeem(&mut self, pending: PendingApproval) {
        let cwd = self.config.cwd.display().to_string();
        let turn = self.norm.turn_number();
        let check = tools::tool_check(
            pending.call_id.clone(),
            pending.name.clone(),
            &pending.payload,
            &cwd,
            turn,
        );
        let gate = self.gate.clone();
        let tx = self.internal_tx.clone();
        let native = pending.native.clone();
        let ticket = pending.ticket.clone();
        let t_gate = ticket.clone();
        let c_gate = check.clone();
        let key = format!("redeem:{native}");
        let handle = tokio::spawn(async move {
            let (decision, failed) = match guarded(REDEEM_TIMEOUT, async move {
                gate.redeem(&t_gate, &c_gate).await
            })
            .await
            {
                Some(d) => (d, false),
                None => (
                    GateDecision::Deny {
                        reason: "Atrium policy gate failed during approval redemption".into(),
                    },
                    true,
                ),
            };
            let _ = tx
                .send(Internal::Redeem {
                    native,
                    ticket,
                    check,
                    decision,
                    failed,
                })
                .await;
        });
        self.inflight.insert(key, handle);
    }

    fn spawn_question(
        &mut self,
        raw_id: Value,
        native: String,
        questions: Value,
        turn: Option<u32>,
    ) {
        let Some(in_flight) =
            InFlightCheck::try_enter(&self.gate_permits, &self.gate_in_flight, &self.gate_settled)
        else {
            self.live.remove(&native);
            self.feed(AdapterInput::QuestionTicket {
                id: native,
                ticket: None,
            });
            self.decline_question_request(&raw_id);
            return;
        };
        let gate = self.gate.clone();
        let tx = self.internal_tx.clone();
        let withdraw_tx = self.withdraw_tx.clone();
        let key = native.clone();
        let q_clone = questions.clone();
        let handle = tokio::spawn(async move {
            let gate_orphan = gate.clone();
            let (res_tx, res_rx) = oneshot::channel();
            tokio::spawn(async move {
                let mut guard = OrphanGuard::arm(withdraw_tx, gate_orphan, Some(in_flight));
                let t = gate.question_ticket(turn).await;
                guard.set_ticket(t.clone());
                let _ = res_tx.send((t, guard));
            });
            let (ticket, failed) = match tokio::time::timeout(GATE_TIMEOUT, res_rx).await {
                Ok(Ok((t, mut guard))) => {
                    if tx
                        .send(Internal::Question {
                            native: key,
                            ticket: t,
                            questions: q_clone,
                            failed: false,
                        })
                        .await
                        .is_ok()
                    {
                        guard.disarm();
                    }
                    return;
                }
                Ok(Err(_)) | Err(_) => (None, true),
            };
            let _ = tx
                .send(Internal::Question {
                    native: key,
                    ticket,
                    questions: q_clone,
                    failed,
                })
                .await;
        });
        self.questions.insert(
            format!("pending:{native}"),
            PendingQuestion {
                raw_id,
                native: native.clone(),
                questions,
                ticket: None,
            },
        );
        self.inflight.insert(format!("question:{native}"), handle);
    }

    async fn on_internal(&mut self, msg: Internal) {
        match msg {
            Internal::WriteFailed(error) => {
                self.feed(AdapterInput::WriteFailed { error });
                self.begin_shutdown(Duration::ZERO).await;
            }
            Internal::Gate {
                native,
                check,
                decision,
                failed,
            } => {
                self.inflight.remove(&format!("gate:{native}"));
                let meta = self.approvals.remove(&format!("pending:{native}"));
                if failed {
                    self.mark_gate_failed();
                }
                if !self.live.contains(&native) {
                    if let GateDecision::Ask { ticket } = decision {
                        self.queue_withdraw(ticket);
                    }
                    return;
                }
                let Some(mut meta) = meta else {
                    if let GateDecision::Ask { ticket } = decision {
                        self.queue_withdraw(ticket);
                    }
                    return;
                };
                let (decision, saturated_decline) = if let GateDecision::Ask { ticket } = &decision
                    && (self.is_pending_saturated() || self.pending.len() + 2 > PENDING_LIMIT)
                {
                    self.queue_withdraw(ticket.clone());
                    self.live.remove(&native);
                    self.decline_server_request(&meta.raw_id, &meta.method);
                    self.emit_saturation_error(format!(
                        "declined {} because event buffer exceeded PENDING_LIMIT ({PENDING_LIMIT})",
                        meta.method
                    ));
                    (
                        GateDecision::Deny {
                            reason: format!(
                                "event buffer exceeded PENDING_LIMIT ({PENDING_LIMIT})"
                            ),
                        },
                        true,
                    )
                } else {
                    (decision, false)
                };
                self.feed(AdapterInput::Gate {
                    check: check.clone(),
                    decision: decision.clone(),
                });
                match decision {
                    GateDecision::Allow => {
                        self.live.remove(&native);
                        self.accept_server_request(&meta.raw_id, &meta.method, &meta.payload);
                    }
                    GateDecision::Deny { .. } => {
                        self.live.remove(&native);
                        if !saturated_decline {
                            self.decline_server_request(&meta.raw_id, &meta.method);
                        }
                    }
                    GateDecision::Ask { ticket } => {
                        meta.call_id = check.call_id;
                        meta.name = check.name;
                        meta.ticket = ticket.clone();
                        self.approvals.insert(ticket.request_id, meta);
                    }
                }
            }
            Internal::Redeem {
                native,
                ticket,
                check,
                decision,
                failed,
            } => {
                self.inflight.remove(&format!("redeem:{native}"));
                let meta = self.approvals.remove(&format!("redeeming:{native}"));
                if failed {
                    self.mark_gate_failed();
                }
                if !self.live.remove(&native) {
                    self.queue_withdraw(ticket);
                    return;
                }
                let Some(meta) = meta else {
                    self.queue_withdraw(ticket);
                    return;
                };
                self.feed(AdapterInput::Redeem {
                    ticket: ticket.clone(),
                    check,
                    decision: decision.clone(),
                });
                match decision {
                    GateDecision::Allow => {
                        self.accept_server_request(&meta.raw_id, &meta.method, &meta.payload);
                    }
                    GateDecision::Deny { .. } | GateDecision::Ask { .. } => {
                        self.queue_withdraw(ticket);
                        self.decline_server_request(&meta.raw_id, &meta.method);
                    }
                }
            }
            Internal::Question {
                native,
                ticket,
                questions,
                failed,
            } => {
                self.inflight.remove(&format!("question:{native}"));
                let meta = self.questions.remove(&format!("pending:{native}"));
                if failed {
                    self.mark_gate_failed();
                }
                if !self.live.contains(&native) {
                    if let Some(t) = ticket {
                        self.queue_withdraw(t);
                    }
                    return;
                }
                let Some(meta) = meta else {
                    if let Some(t) = ticket {
                        self.queue_withdraw(t);
                    }
                    return;
                };
                let ticket = match ticket {
                    Some(t)
                        if self.is_pending_saturated()
                            || self.pending.len() + 2 > PENDING_LIMIT =>
                    {
                        self.queue_withdraw(t);
                        self.live.remove(&native);
                        self.decline_question_request(&meta.raw_id);
                        self.emit_saturation_error(format!(
                            "declined question request {native} because event buffer exceeded PENDING_LIMIT ({PENDING_LIMIT})"
                        ));
                        None
                    }
                    other => other,
                };
                self.feed(AdapterInput::QuestionTicket {
                    id: native.clone(),
                    ticket: ticket.clone(),
                });
                match ticket {
                    Some(t) => {
                        self.questions.insert(
                            t.request_id.clone(),
                            PendingQuestion {
                                raw_id: meta.raw_id,
                                native: meta.native,
                                questions,
                                ticket: Some(t),
                            },
                        );
                    }
                    None => {
                        if self.live.remove(&native) {
                            self.decline_question_request(&meta.raw_id);
                        }
                    }
                }
            }
        }
    }

    async fn on_command(&mut self, command: SessionCommand) {
        self.feed(AdapterInput::Command {
            command: command.clone(),
        });
        match command {
            SessionCommand::SendUserMessage { text } => {
                if self.norm.take_rejected() {
                    return;
                }
                self.queued_msgs.push_back(text);
                if !self.norm.turn_in_flight() {
                    self.dispatch_next();
                }
            }
            SessionCommand::ResolveApproval {
                request_id,
                decision,
            } => {
                let Some(pending) = self.approvals.remove(&request_id) else {
                    return;
                };
                match decision {
                    ApprovalDecision::Allow => {
                        let native = pending.native.clone();
                        self.approvals
                            .insert(format!("redeeming:{native}"), pending.clone());
                        self.spawn_redeem(pending);
                    }
                    ApprovalDecision::Deny { .. } => {
                        self.live.remove(&pending.native);
                        self.decline_server_request(&pending.raw_id, &pending.method);
                    }
                }
            }
            SessionCommand::AnswerQuestion {
                request_id,
                answers,
            } => {
                let Some(pending) = self.questions.remove(&request_id) else {
                    return;
                };
                self.live.remove(&pending.native);
                let payload = build_question_response(&pending.questions, &answers);
                self.write_value(&json!({
                    "id": pending.raw_id,
                    "result": payload,
                }));
            }
            SessionCommand::DeclineQuestion { request_id } => {
                let Some(pending) = self.questions.remove(&request_id) else {
                    return;
                };
                if let Some(ticket) = pending.ticket {
                    self.queue_withdraw(ticket);
                }
                self.live.remove(&pending.native);
                self.write_value(&json!({
                    "id": pending.raw_id,
                    "result": { "answers": {} },
                }));
            }
            SessionCommand::Interrupt => {
                if self.norm.turn_in_flight() {
                    if let Some(tid) = self.norm.native_session_id().map(str::to_owned) {
                        self.turn_seq += 1;
                        self.write_value(&json!({
                            "id": format!("atrium-interrupt-{}", self.turn_seq),
                            "method": "turn/interrupt",
                            "params": {
                                "threadId": tid,
                            }
                        }));
                    }
                    self.interrupt_deadline = Some(TokioInstant::now() + INTERRUPT_GRACE);
                }
            }
            SessionCommand::Shutdown => {
                self.begin_shutdown(SHUTDOWN_GRACE).await;
            }
        }
    }

    fn dispatch_next(&mut self) {
        if self.shutting_down || self.norm.turn_in_flight() {
            return;
        }
        let Some(text) = self.queued_msgs.pop_front() else {
            return;
        };
        let Some(thread_id) = self.norm.native_session_id().map(str::to_owned) else {
            return;
        };
        self.turn_seq += 1;
        let effort = self.config.effort.as_deref();
        self.write_value(&json!({
            "id": format!("atrium-turn-{}", self.turn_seq),
            "method": "turn/start",
            "params": {
                "threadId": thread_id,
                "input": [{ "type": "text", "text": text }],
                "effort": effort,
            }
        }));
    }

    async fn on_deadline(&mut self) {
        let now = TokioInstant::now();
        if let Some(d) = self.interrupt_deadline
            && now >= d
        {
            self.interrupt_deadline = None;
            self.feed(AdapterInput::Deadline {
                what: DEADLINE_INTERRUPT.into(),
            });
            self.begin_shutdown(Duration::ZERO).await;
            return;
        }
        if let Some(d) = self.shutdown_deadline
            && now >= d
        {
            self.shutdown_deadline = None;
            let code = self.stop_process(Duration::ZERO).await;
            self.feed(AdapterInput::Exit { code });
        }
    }

    async fn begin_shutdown(&mut self, grace: Duration) {
        self.shutting_down = true;
        self.interrupt_deadline = None;
        self.shutdown_deadline = None;
        if let Some(proc) = &mut self.proc {
            proc.stdin_tx = None;
        }
        if grace.is_zero() {
            let code = self.stop_process(Duration::ZERO).await;
            self.feed(AdapterInput::Exit { code });
        } else {
            // Wait up to `grace` for EOF before forcing kill in `on_deadline` (while still draining stdout).
            self.shutdown_deadline = Some(TokioInstant::now() + grace);
        }
    }

    async fn stop_process(&mut self, grace: Duration) -> Option<i32> {
        let mut p = self.proc.take()?;
        let mut child = p.child.take()?;
        p.stdin_tx = None;
        let mut rx = self.out_rx.take();
        let term_res = {
            let term = supervise::terminate(&mut child, grace);
            tokio::pin!(term);
            loop {
                tokio::select! {
                    r = &mut term => break r,
                    out = recv_out(&mut rx) => match out {
                        Some(msg) => self.on_out(msg),
                        None => rx = None,
                    },
                }
            }
        };
        let code = match term_res {
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
                    line: format!("atrium: could not verify codex process group: {e}"),
                });
                None
            }
        };
        if let Some(mut rx) = rx {
            let end = TokioInstant::now() + Duration::from_millis(200);
            while let Ok(Some(msg)) = tokio::time::timeout_at(end, rx.recv()).await {
                self.on_out(msg);
            }
        }
        p.writer.abort();
        for r in std::mem::take(&mut p.readers) {
            r.abort();
        }
        code
    }

    fn accept_server_request(&mut self, raw_id: &Value, method: &str, _payload: &Value) {
        let result = match method {
            "mcpServer/elicitation/request" => json!({ "action": "accept" }),
            _ => json!({ "decision": "accept" }),
        };
        self.write_value(&json!({ "id": raw_id, "result": result }));
    }

    fn decline_server_request(&mut self, raw_id: &Value, method: &str) {
        let result = match method {
            "mcpServer/elicitation/request" => json!({ "action": "decline" }),
            "item/permissions/requestApproval" => json!({ "permissions": {}, "scope": "turn" }),
            "execCommandApproval" | "applyPatchApproval" => json!({ "decision": "denied" }),
            _ => json!({ "decision": "decline" }),
        };
        self.write_value(&json!({ "id": raw_id, "result": result }));
    }

    fn decline_native_request(&mut self, raw_id: &Value, method: &str) {
        if method == "item/tool/requestUserInput" {
            self.decline_question_request(raw_id);
        } else {
            self.decline_server_request(raw_id, method);
        }
    }

    fn decline_question_request(&mut self, raw_id: &Value) {
        self.write_value(&json!({
            "id": raw_id,
            "result": { "answers": {} },
        }));
    }

    fn queue_withdraw(&mut self, ticket: ApprovalTicket) {
        self.pending_withdrawals.push((ticket, None));
    }

    fn queue_withdraw_with_permit(
        &mut self,
        ticket: ApprovalTicket,
        permit: Option<OwnedSemaphorePermit>,
    ) {
        self.pending_withdrawals.push((ticket, permit));
    }

    async fn wait_for_in_flight_gates(&self) {
        let cap = REDEEM_TIMEOUT.min(Duration::from_millis(500));
        let deadline = TokioInstant::now() + cap;
        while self.gate_in_flight.load(Ordering::Acquire) > 0 {
            let notified = self.gate_settled.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.gate_in_flight.load(Ordering::Acquire) == 0 {
                break;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                break;
            }
        }
    }

    async fn drain_withdrawals(&mut self) {
        while let Ok((ticket, permit)) = self.withdraw_rx.try_recv() {
            self.pending_withdrawals.push((ticket, permit));
        }
        let tickets = std::mem::take(&mut self.pending_withdrawals);
        if tickets.is_empty() {
            return;
        }
        let mut handles = Vec::with_capacity(tickets.len());
        for (ticket, permit) in tickets {
            let gate = self.gate.clone();
            handles.push(tokio::spawn(async move {
                let _permit = permit;
                gate.withdraw(&ticket).await;
            }));
        }
        let deadline = TokioInstant::now() + REDEEM_TIMEOUT;
        for handle in handles {
            if tokio::time::timeout_at(deadline, handle).await.is_err() {
                // Keep unfinished withdrawals running in the background with
                // their permits so compensation settles and capacity stays
                // reserved until `gate.withdraw` completes.
                break;
            }
        }
    }

    fn drain_internal_withdrawals(&mut self) {
        while let Ok(msg) = self.internal_rx.try_recv() {
            match msg {
                Internal::Gate {
                    decision: GateDecision::Ask { ticket },
                    ..
                } => self.queue_withdraw(ticket),
                Internal::Question {
                    ticket: Some(ticket),
                    ..
                } => self.queue_withdraw(ticket),
                Internal::Redeem {
                    ticket, decision, ..
                } if !matches!(decision, GateDecision::Allow) => self.queue_withdraw(ticket),
                _ => {}
            }
        }
    }

    fn forget_native(&mut self, native: &str) {
        self.live.remove(native);
        for prefix in ["gate:", "redeem:", "question:"] {
            if let Some(h) = self.inflight.remove(&format!("{prefix}{native}")) {
                h.abort();
            }
        }
        let mut withdrawn = Vec::new();
        self.approvals.retain(|_, a| {
            if a.native == native {
                if !a.ticket.request_id.is_empty() {
                    withdrawn.push(a.ticket.clone());
                }
                false
            } else {
                true
            }
        });
        self.questions.retain(|_, q| {
            if q.native == native {
                if let Some(t) = q.ticket.clone() {
                    withdrawn.push(t);
                }
                false
            } else {
                true
            }
        });
        for ticket in withdrawn {
            self.queue_withdraw(ticket);
        }
    }

    fn clear_requests(&mut self) {
        self.abort_inflight();
        self.live.clear();
        for (_, a) in self.approvals.drain() {
            if !a.ticket.request_id.is_empty() {
                self.pending_withdrawals.push((a.ticket, None));
            }
        }
        for (_, q) in self.questions.drain() {
            if let Some(ticket) = q.ticket {
                self.pending_withdrawals.push((ticket, None));
            }
        }
    }

    fn abort_inflight(&mut self) {
        for (_, h) in self.inflight.drain() {
            h.abort();
        }
    }

    fn mark_gate_failed(&mut self) {
        if self.gate_failed {
            return;
        }
        self.gate_failed = true;
        let updated = build_descriptor(&self.config, &self.options, self.cli_version.clone(), true);
        if updated != self.descriptor {
            self.descriptor = updated.clone();
            self.feed(AdapterInput::Enforcement {
                descriptor: updated,
            });
        }
    }
}

fn has_grant_root(params: &Value) -> bool {
    [
        "/grantRoot",
        "/grant_root",
        "/item/grantRoot",
        "/item/grant_root",
    ]
    .iter()
    .any(|ptr| match params.pointer(ptr) {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    })
}

fn id_key(v: &Value) -> String {
    match v.as_str() {
        Some(s) => s.to_owned(),
        None => v.to_string(),
    }
}

fn approval_method_name(method: &str, params: &Value) -> Option<(String, Option<String>)> {
    let (name, id_field) = match method {
        "item/commandExecution/requestApproval" => ("commandExecution", "itemId"),
        "item/fileChange/requestApproval" => ("fileChange", "itemId"),
        "item/permissions/requestApproval" => ("item/permissions/requestApproval", "itemId"),
        "mcpServer/elicitation/request" => ("mcpServer/elicitation/request", "elicitationId"),
        "execCommandApproval" => ("execCommandApproval", "callId"),
        "applyPatchApproval" => ("applyPatchApproval", "callId"),
        _ => return None,
    };
    let cid = params
        .get(id_field)
        .or_else(|| params.get("itemId"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    Some((name.to_owned(), cid))
}

fn build_question_response(questions: &Value, answers: &[QuestionAnswer]) -> Value {
    let mut map = serde_json::Map::new();
    let empty = Vec::new();
    let q_list = questions.as_array().unwrap_or(&empty);
    for ans in answers {
        let mut resolved = Vec::new();
        let q_opts = q_list
            .iter()
            .find(|q| q.get("id").and_then(Value::as_str) == Some(ans.question_id.as_str()))
            .and_then(|q| q.get("options").and_then(Value::as_array));

        for sel in &ans.selected {
            let mut mapped = sel.clone();
            if let Ok(idx) = sel.parse::<usize>()
                && let Some(opts) = q_opts
                && let Some(label) = opts
                    .get(idx)
                    .and_then(|o| o.get("label").and_then(Value::as_str))
            {
                mapped = label.to_owned();
            }
            resolved.push(Value::String(mapped));
        }
        if let Some(txt) = &ans.text
            && !txt.is_empty()
        {
            resolved.push(Value::String(txt.clone()));
        }
        map.insert(ans.question_id.clone(), json!({ "answers": resolved }));
    }
    json!({ "answers": map })
}

fn is_closing_event(ev: &AgentEvent) -> bool {
    matches!(
        ev,
        AgentEvent::Error { fatal: true, .. }
            | AgentEvent::MessageCompleted { .. }
            | AgentEvent::ToolCallFinished { .. }
            | AgentEvent::RequestResolved { .. }
            | AgentEvent::TurnCompleted { .. }
            | AgentEvent::SessionEnded { .. }
    )
}

fn earliest_deadline(a: Option<TokioInstant>, b: Option<TokioInstant>) -> Option<TokioInstant> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

async fn wait_until(deadline: Option<TokioInstant>) {
    match deadline {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending().await,
    }
}

async fn recv_cmd(rx: &mut Option<mpsc::Receiver<SessionCommand>>) -> Option<SessionCommand> {
    match rx.as_mut() {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

async fn recv_out(rx: &mut Option<mpsc::Receiver<Out>>) -> Option<Out> {
    match rx.as_mut() {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

/// Runs a gate future with panic + timeout containment so a broken gate can't
/// wedge or crash the adapter driver task.
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn guarded<T>(
    limit: Duration,
    fut: impl std::future::Future<Output = T> + Send + 'static,
) -> Option<T>
where
    T: Send + 'static,
{
    let joined = tokio::spawn(fut);
    let _guard = AbortOnDrop(joined.abort_handle());
    tokio::time::timeout(limit, joined)
        .await
        .ok()
        .and_then(Result::ok)
}
