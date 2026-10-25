//! Agent adapter contract (PLAN §3.2, §3.12).
//!
//! **Contract, frozen for M0 (revision 2, D-014).** Each adapter spike
//! (`spikes/agent-*`) implements [`AgentAdapter`] and [`Normalizer`] for one
//! provider. Changes go through `docs/agents/change-requests.md`.
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
//! `Deny` blocks it (`ToolCallFinished { status: denied }`), `Ask` emits
//! [`AgentEvent::ApprovalRequested`] and waits for the client. If the gate
//! can't be reached, or the hook fails in any way, the call is denied (fail
//! closed). An adapter that can only see calls the provider itself chooses to
//! gate reports [`Interception::ProviderGated`]; the daemon decides what such
//! a provider may do.
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
//!   provider to stop, waits [`SHUTDOWN_GRACE`], then kills it, reaps it, and
//!   emits the closing events (see `atrium_protocol::agent`).

use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use atrium_protocol::agent::{AgentEvent, AgentEventEnvelope, Provider, SessionCommand, ToolKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;

pub mod fixture;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub const EVENT_BUFFER: usize = 1024;
pub const COMMAND_BUFFER: usize = 64;
pub const MAX_QUEUED_MESSAGES: usize = 8;
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

pub trait AgentAdapter: Send + Sync {
    fn provider(&self) -> Provider;

    /// How much of the provider's activity this adapter can put through the gate.
    fn interception(&self) -> Interception;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interception {
    /// Every tool call, MCP tools included, passes the gate before it runs.
    AllToolCalls,
    /// Only calls the provider itself asks about pass the gate; the rest run
    /// unchecked.
    ProviderGated,
}

/// A running provider session. Dropping both halves shuts it down.
#[derive(Debug)]
pub struct Session {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServer {
    pub name: String,
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// Where the provider process runs. Sandbox (vsock) transport comes in M4.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Transport {
    /// A child process on the host, talking over stdio.
    Local,
}

/// Atrium's policy engine, as an adapter sees it.
pub trait ToolGate: Send + Sync {
    fn check<'a>(&'a self, check: &'a ToolCheck) -> BoxFuture<'a, GateDecision>;
}

/// A tool call the provider is about to run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCheck {
    pub call_id: Option<String>,
    pub name: String,
    pub kind: ToolKind,
    pub title: Option<String>,
    pub input: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GateDecision {
    Allow,
    Deny {
        reason: String,
    },
    /// Ask the user: emit `ApprovalRequested` and wait for `ResolveApproval`.
    Ask,
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
