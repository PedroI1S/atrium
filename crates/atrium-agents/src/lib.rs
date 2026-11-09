//! Agent adapter contract (PLAN §3.2, §3.12).
//!
//! **Contract, M1 revision 3 (draft, `docs/contracts/m1.md`).** Each
//! production adapter (`src/claude`, `src/codex`) implements [`AgentAdapter`]
//! and [`Normalizer`] for one provider. Changes go through
//! `docs/agents/change-requests.md`.
//!
//! An adapter has two layers:
//! - a pure [`Normalizer`] that turns everything that crosses the adapter
//!   boundary ([`AdapterInput`]) into [`AgentEvent`]s, tested offline against
//!   recorded [`fixture`]s;
//! - the live [`Session`], which owns the process, writes the provider's
//!   input format, routes tool calls through the [`ToolGate`], answers the
//!   provider's control requests from [`SessionCommand`]s, and feeds every
//!   input through the same normalizer, so live runs and replays can't drift.
//!
//! # Policy enforcement
//!
//! Atrium, not the provider, decides what runs. Wherever the provider offers
//! a pre-execution hook, **every** tool call (including MCP tools) goes
//! through [`SessionConfig::gate`] before it runs: `Allow` lets it run,
//! `Deny` blocks it (`ToolCallFinished { status: denied }`), `Ask { ticket }`
//! emits [`AgentEvent::ApprovalRequested`] with the ticket's id and waits.
//! If the gate can't be reached, or the hook fails in any way, the call is
//! denied (fail closed).
//!
//! **Approvals are redeemed, not trusted (D-046).** When an approval comes
//! back as `ResolveApproval { Allow }`, the adapter calls
//! [`ToolGate::redeem`] *immediately before* performing the effect (answering
//! the provider's control request, writing a file, retrying a hook-denied
//! call). Only `Allow` from `redeem` lets the effect happen; it is single use
//! and checks that the call's input still matches what was approved. Every
//! redeem is recorded as [`AdapterInput::Redeem`].
//!
//! # Enforcement per session (D-044)
//!
//! What Atrium can enforce depends on the session (CLI version, settings
//! files, plugins, hooks, mode), so it is reported per session as an
//! [`EnforcementDescriptor`]: [`AgentAdapter::probe`] before starting,
//! [`Session::descriptor`] and `SessionStarted.enforcement` once started,
//! `EnforcementChanged` if it changes. Build it only with
//! [`EnforcementDescriptor::evaluate`]. Report conservatively: anything not
//! verified is Partial. Enforcement levels are shown, never used to restrict
//! (D-012).
//!
//! # Backpressure and lifecycle
//!
//! - `events` holds at most [`EVENT_BUFFER`] envelopes and `commands` at most
//!   [`COMMAND_BUFFER`]. A full event channel must never stop the session
//!   from receiving commands: `Interrupt` and `Shutdown` take effect even if
//!   nobody is reading events.
//! - At most [`MAX_QUEUED_MESSAGES`] user messages wait while a turn runs;
//!   more are rejected with `Error { fatal: false }`.
//! - `start` fails with [`AdapterError::Handshake`] after [`STARTUP_TIMEOUT`].
//! - `Shutdown`, or dropping both halves of the [`Session`], asks the
//!   provider to stop, waits [`SHUTDOWN_GRACE`], then kills its whole process
//!   group (`atrium_host::supervise`), reaps it, and emits the closing events (see
//!   `atrium_protocol::agent`).
//! - Cleanup is bounded; delivery of the closing events is not dropped. If
//!   the receiver isn't reading, the adapter keeps up to
//!   [`atrium_protocol::limits::TERMINAL_TAIL_EVENTS`] closing events and
//!   delivers them when it drains, or until the receiver is dropped.
//! - Byte limits: see [`atrium_protocol::limits`].

use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use atrium_host::supervise::ProcessIdentity;
use atrium_protocol::agent::{AgentEvent, AgentEventEnvelope, Provider, SessionCommand, ToolKind};
use atrium_protocol::policy::{EnforcementDescriptor, ToolSubject};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;

pub mod fixture;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// See [`SessionConfig::on_spawn`].
pub type OnSpawn = Arc<dyn Fn(&ProcessIdentity) + Send + Sync>;

pub const EVENT_BUFFER: usize = 1024;
pub const COMMAND_BUFFER: usize = 64;
pub const MAX_QUEUED_MESSAGES: usize = 8;
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

pub trait AgentAdapter: Send + Sync {
    fn provider(&self) -> Provider;

    /// CLI versions this adapter was verified against (fixtures and live
    /// enforcement checks). Any other version makes sessions Partial.
    fn tested_versions(&self) -> &'static [&'static str];

    /// The enforcement a session with this config would get, without starting
    /// it: finds the CLI and its version, and inspects the configuration
    /// sources it would load. For the provider picker and settings.
    fn probe<'a>(&'a self, config: &'a SessionConfig) -> BoxFuture<'a, EnforcementDescriptor>;

    /// Spawns the provider and completes its handshake. On success the first
    /// event is [`AgentEvent::SessionStarted`] (possibly only once the first
    /// turn starts, for providers that create sessions lazily). Returns
    /// [`AdapterError::Unsupported`] only for a combination the provider
    /// itself can't do; every [`PermissionPolicy`] is valid on every
    /// [`Transport`].
    fn start(
        &self,
        config: SessionConfig,
        transport: Transport,
    ) -> BoxFuture<'_, Result<Session, AdapterError>>;
}

/// A running provider session. Dropping both halves shuts it down.
#[derive(Debug)]
pub struct Session {
    /// The enforcement this session actually got (same as
    /// `SessionStarted.enforcement`).
    pub descriptor: EnforcementDescriptor,
    /// The provider process (also reported earlier through
    /// [`SessionConfig::on_spawn`]). `None` only for transports without a
    /// local process.
    pub process: Option<ProcessIdentity>,
    /// Closed after [`AgentEvent::SessionEnded`].
    pub events: mpsc::Receiver<AgentEventEnvelope>,
    pub commands: mpsc::Sender<SessionCommand>,
}

#[derive(Clone)]
pub struct SessionConfig {
    pub cwd: PathBuf,
    pub model: Option<String>,
    /// Provider-specific effort / reasoning level, passed through as is.
    pub effort: Option<String>,
    pub permissions: PermissionPolicy,
    /// Atrium's policy engine. Consulted for every intercepted tool call.
    pub gate: Arc<dyn ToolGate>,
    /// MCP servers injected into this session only (never global config).
    pub mcp_servers: Vec<McpServer>,
    /// Native session id to resume.
    pub resume: Option<String>,
    pub env: Vec<(String, String)>,
    /// Override the CLI binary (default: found on `PATH`).
    pub program: Option<PathBuf>,
    /// Called once, right after the provider process is spawned and before
    /// the handshake, so the daemon records it for crash recovery. An adapter
    /// never spawns a second provider process inside one `Session`: if an
    /// interrupt has to kill the process, the session ends and the daemon
    /// resumes in a new incarnation.
    pub on_spawn: Option<OnSpawn>,
}

impl fmt::Debug for SessionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionConfig")
            .field("cwd", &self.cwd)
            .field("model", &self.model)
            .field("effort", &self.effort)
            .field("permissions", &self.permissions)
            .field("mcp_servers", &self.mcp_servers)
            .field("resume", &self.resume)
            .field("program", &self.program)
            .finish_non_exhaustive()
    }
}

/// The provider-side mode a session runs in, chosen by the user. Safe modes
/// are the defaults, but Atrium never refuses a mode the user picks (D-011).
/// What actually runs is decided by the [`ToolGate`]; this only picks the
/// provider's own guard rails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionPolicy {
    /// Workspace threads: the provider's normal mode, with its own prompts
    /// routed to the gate.
    Ask,
    /// Home: additionally use the provider's own read-only mode where it has
    /// one (Codex `sandbox=read-only`).
    ReadOnly,
    /// The provider's full-access mode: no provider prompts. The default for
    /// sandbox threads, and available on the host when the user chooses it.
    /// Intercepted calls still pass the gate, which applies whatever rules
    /// the user has set (PLAN §3.12).
    FullAccess,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServer {
    pub name: String,
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// Prints `args` and `env` values redacted: they can hold credentials
/// (gate review, credential-safe `Debug`).
impl fmt::Debug for McpServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let env_keys: Vec<&str> = self.env.iter().map(|(k, _)| k.as_str()).collect();
        f.debug_struct("McpServer")
            .field("name", &self.name)
            .field("command", &self.command)
            .field("args", &format_args!("[{} redacted]", self.args.len()))
            .field("env_keys", &env_keys)
            .finish()
    }
}

/// Where the provider process runs. Sandbox (vsock) transport comes in M4.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Transport {
    /// A child process on the host, talking over stdio.
    Local,
}

/// Atrium's policy engine and approval ledger, as an adapter sees it. One
/// gate per session; it knows the thread, incarnation, cwd and turn.
pub trait ToolGate: Send + Sync {
    /// Called for every intercepted tool call before it runs. `Ask` has
    /// already opened a ledger entry bound to this call (contract §5).
    fn check<'a>(&'a self, check: &'a ToolCheck) -> BoxFuture<'a, GateDecision>;

    /// Called after `ResolveApproval { Allow }`, immediately before the
    /// effect. Returns `Allow` at most once per ticket, and only if `check`
    /// still matches the binding; otherwise `Deny`. Never `Ask`.
    fn redeem<'a>(
        &'a self,
        ticket: &'a ApprovalTicket,
        check: &'a ToolCheck,
    ) -> BoxFuture<'a, GateDecision>;

    /// Opens a ledger entry for a question the provider asked. `None` if the
    /// gate is unreachable: the adapter then declines the question to the
    /// provider and emits nothing for it.
    fn question_ticket(&self, turn: Option<u32>) -> BoxFuture<'_, Option<ApprovalTicket>>;
}

/// A ledger entry's handle. `request_id` is what the events and commands use.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ApprovalTicket {
    pub request_id: String,
    /// Copied into `ApprovalRequested.rememberable`. Always `false` for
    /// question tickets.
    #[serde(default)]
    pub rememberable: bool,
}

/// A tool call the provider is about to run.
///
/// For [`ToolGate::redeem`], build it from the provider's own approval
/// request (e.g. the `can_use_tool` payload), never from a cached earlier
/// check, so a changed input is caught.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCheck {
    pub call_id: Option<String>,
    pub name: String,
    /// `Execute` whenever `subject` is a command, whatever the provider calls
    /// the tool.
    pub kind: ToolKind,
    pub title: Option<String>,
    pub input: Value,
    /// Extracted by the adapter; `Unknown` when it can't (the policy asks).
    pub subject: ToolSubject,
    /// The adapter's own turn counter for this incarnation, so the binding
    /// never races the event stream.
    pub turn: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GateDecision {
    Allow,
    Deny {
        reason: String,
    },
    /// Ask the user: emit `ApprovalRequested` with `ticket.request_id` and
    /// wait for `ResolveApproval`.
    Ask {
        ticket: ApprovalTicket,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("failed to start the provider: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("handshake failed: {0}")]
    Handshake(String),
    #[error("not supported by this provider: {0}")]
    Unsupported(String),
    #[error("protocol error: {0}")]
    Protocol(String),
}

/// Everything that crosses the adapter boundary, in order. Live sessions
/// feed these to the [`Normalizer`] as they happen; fixtures record them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "dir", rename_all = "snake_case")]
pub enum AdapterInput {
    /// A line the adapter wrote to the provider's stdin.
    Stdin {
        line: String,
    },
    /// A line the provider wrote to stdout (NDJSON / JSON-RPC message).
    Stdout {
        line: String,
    },
    Stderr {
        line: String,
    },
    /// A pre-execution hook call from the provider (e.g. the `agy` hook bridge).
    HookRequest {
        id: String,
        payload: Value,
    },
    /// The adapter's answer to that hook call.
    HookResponse {
        id: String,
        payload: Value,
    },
    /// What the gate decided for a tool call.
    Gate {
        check: ToolCheck,
        decision: GateDecision,
    },
    /// A command the client sent to the session.
    Command {
        command: SessionCommand,
    },
    /// What [`ToolGate::redeem`] returned for an approved call.
    Redeem {
        ticket: ApprovalTicket,
        check: ToolCheck,
        decision: GateDecision,
    },
    /// What [`ToolGate::question_ticket`] returned for the provider's question
    /// `id` (its native id); `None` means the question was declined.
    QuestionTicket {
        id: String,
        ticket: Option<ApprovalTicket>,
    },
    /// A hook connection closed before it was answered (its Ask is cancelled).
    HookDisconnected {
        id: String,
    },
    /// Writing to the provider failed (stdin closed, broken pipe).
    WriteFailed {
        error: String,
    },
    /// A deadline fired: `what` is e.g. `startup`, `shutdown_grace`, `hook`.
    Deadline {
        what: String,
    },
    /// The provider process exited (or the connection closed).
    Exit {
        code: Option<i32>,
    },
}

/// Pure mapping from adapter inputs to normalized events. Holds whatever
/// state the mapping needs (open tool calls, message buffers, pending
/// requests, turn counter), but does no I/O.
pub trait Normalizer {
    /// Returns zero or more events, in order. [`AdapterInput::Exit`] returns
    /// the closing events and `SessionEnded`.
    fn on_input(&mut self, input: &AdapterInput) -> Vec<AgentEvent>;
}
