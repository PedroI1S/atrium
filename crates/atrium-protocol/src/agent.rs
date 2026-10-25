//! Normalized agent events and session commands (PLAN §3.2).
//!
//! Every provider (Claude Code, Codex, `agy`, ACP) maps its native protocol
//! into [`AgentEvent`]. The daemon stores events, the attention state machine
//! reads them, and the UI renders them, so nothing downstream ever sees a
//! provider-specific record.
//!
//! # Lifecycles
//!
//! **Established session**: `SessionStarted` is the first event and
//! `SessionEnded` the last. **Pre-session failure** (bad model, auth error,
//! the CLI won't start): `Error { fatal: true }` then `SessionEnded`, with no
//! `SessionStarted`. Adapters never invent a native session id.
//!
//! When a session ends for any reason, the adapter first closes everything
//! still open, in this order: open messages → `MessageCompleted` (text so
//! far); open tool calls → `ToolCallFinished { status: cancelled }`; pending
//! approvals and questions → `RequestResolved { outcome: cancelled }`; an open
//! turn → `TurnCompleted { outcome: interrupted | failed }`. Then
//! `SessionEnded`.
//!
//! # Mapping rules
//!
//! - Text arrives as [`AgentEvent::MessageDelta`]s followed by exactly one
//!   [`AgentEvent::MessageCompleted`] whose text is their concatenation.
//!   Providers that only send whole messages emit the completed event alone.
//! - Every tool call is `ToolCallStarted` → zero or more `ToolCallUpdated` →
//!   exactly one `ToolCallFinished`, all with the same `call_id`. `input` is
//!   always a complete JSON value: partial provider arguments are buffered
//!   until they parse.
//! - Anything that needs a human goes out as `ApprovalRequested` /
//!   `QuestionAsked`, and each one gets exactly one `RequestResolved`. A
//!   command for an unknown or already resolved request is rejected with
//!   `Error { fatal: false }` and changes nothing.
//! - `Usage` is the running total for the current turn and replaces any
//!   earlier `Usage` in that turn. `TurnCompleted::usage` is the final total
//!   for the turn. Session totals are the sum over turns.
//! - `QuotaUpdated` is informational (subscription windows filling up).
//!   `RateLimited` means the provider is refusing work right now.
//! - A record the adapter can't map yet becomes [`AgentEvent::Unmapped`],
//!   never a silent drop, so spike fixtures show the gaps.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The native agent behind a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Claude,
    Codex,
    Agy,
    Acp,
}

/// One event, as the adapter emits it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEventEnvelope {
    /// Per session, starting at 0, no gaps.
    pub seq: u64,
    /// Unix time in milliseconds when the adapter produced the event.
    pub at_ms: u64,
    pub event: AgentEvent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// The provider's session exists. `native_session_id` is what resume uses.
    SessionStarted {
        provider: Provider,
        native_session_id: String,
        model: Option<String>,
        /// Tool names the provider advertises, if it says.
        #[serde(default)]
        tools: Vec<String>,
    },
    /// A user message was accepted and the agent is working on it.
    TurnStarted {
        turn: u32,
    },
    MessageDelta {
        message_id: String,
        kind: MessageKind,
        text: String,
    },
    MessageCompleted {
        message_id: String,
        kind: MessageKind,
        text: String,
    },
    ToolCallStarted {
        call_id: String,
        /// The provider's own tool name, e.g. `Bash`, `run_command`, `shell`.
        name: String,
        kind: ToolKind,
        /// Short human-readable summary, e.g. the command line or file path.
        title: Option<String>,
        input: Value,
    },
    /// Every field except `call_id` is optional; `None` means unchanged.
    ToolCallUpdated {
        call_id: String,
        title: Option<String>,
        kind: Option<ToolKind>,
        /// Replaces the whole input.
        input: Option<Value>,
        /// Appended to the output so far (e.g. streamed command output).
        output_delta: Option<String>,
        /// Replaces the whole output so far.
        output: Option<String>,
    },
    ToolCallFinished {
        call_id: String,
        status: ToolStatus,
        output: Option<String>,
        error: Option<String>,
    },
    /// The agent wants to do something Atrium's policy said to ask about.
    /// Answer with [`SessionCommand::ResolveApproval`].
    ApprovalRequested {
        request_id: String,
        /// The tool call this approval gates, when the provider links them.
        call_id: Option<String>,
        name: String,
        kind: ToolKind,
        title: Option<String>,
        input: Value,
        reason: Option<String>,
    },
    /// The agent asked the user something. Answer with
    /// [`SessionCommand::AnswerQuestion`].
    QuestionAsked {
        request_id: String,
        questions: Vec<Question>,
    },
    /// Closes an `ApprovalRequested` or `QuestionAsked`. Exactly once each.
    RequestResolved {
        request_id: String,
        outcome: RequestOutcome,
    },
    /// The agent's full current plan (replaces any earlier one).
    PlanUpdated {
        entries: Vec<PlanEntry>,
    },
    Usage {
        usage: Usage,
    },
    /// Subscription or quota windows, as the provider reports them.
    QuotaUpdated {
        windows: Vec<QuotaWindow>,
    },
    /// The provider is refusing work because of a limit. The scheduler pauses
    /// the thread.
    RateLimited {
        /// Unix ms when the limit resets, if known.
        resets_at_ms: Option<u64>,
        message: Option<String>,
    },
    TurnCompleted {
        turn: u32,
        outcome: TurnOutcome,
        usage: Option<Usage>,
    },
    Error {
        message: String,
        /// The session can't continue.
        fatal: bool,
    },
    /// A native record the adapter doesn't map (yet).
    Unmapped {
        record_type: String,
        raw: Value,
    },
    /// The provider process or connection ended. Always the last event.
    SessionEnded {
        exit_code: Option<i32>,
        reason: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Assistant,
    /// Thinking / reasoning text, when the provider exposes it.
    Reasoning,
}

/// Coarse tool category, used by the policy engine and the UI. Mapped from
/// the provider's tool name; `Other` when unsure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Fetch,
    Think,
    /// A tool from an MCP server.
    Mcp,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Succeeded,
    Failed,
    /// Blocked by an approval decision or policy.
    Denied,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    Interrupted,
    Failed,
    /// The provider stopped on a turn or token limit.
    LimitReached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestOutcome {
    Approved,
    Denied,
    Answered,
    /// The turn or session ended first, or the provider withdrew the request.
    Cancelled,
    /// The provider's own timeout fired first.
    Expired,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    pub id: String,
    pub prompt: String,
    /// Empty for free-text-only questions.
    #[serde(default)]
    pub options: Vec<QuestionOption>,
    #[serde(default)]
    pub multi_select: bool,
    /// Whether a free-text answer is accepted (alone or with options).
    #[serde(default)]
    pub allow_text: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionOption {
    /// Stable within the question. The adapter makes one up if the provider
    /// has none (e.g. the option's index).
    pub id: String,
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionAnswer {
    pub question_id: String,
    /// [`QuestionOption::id`]s, in the order chosen.
    #[serde(default)]
    pub selected: Vec<String>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanEntry {
    pub content: String,
    pub status: PlanStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

/// Token counts. Fields a provider doesn't report stay `None`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaWindow {
    /// The provider's name for the window, e.g. `5h`, `weekly`, `primary`.
    pub name: String,
    pub used_percent: Option<f64>,
    pub resets_at_ms: Option<u64>,
}

/// What a client can tell a running session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionCommand {
    /// Starts a new turn. Sent while a turn is running, it waits in a bounded
    /// queue (see `atrium-agents`); a full queue rejects it with
    /// `Error { fatal: false }`.
    SendUserMessage { text: String },
    ResolveApproval {
        request_id: String,
        decision: ApprovalDecision,
    },
    AnswerQuestion {
        request_id: String,
        answers: Vec<QuestionAnswer>,
    },
    /// Stop the current turn; the session stays usable.
    Interrupt,
    /// End the session and the provider process.
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApprovalDecision {
    Allow,
    /// `message` is passed to the agent as the reason, when the provider supports it.
    Deny {
        message: Option<String>,
    },
}
