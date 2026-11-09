//! Normalized agent events and session commands (PLAN §3.2).
//!
//! Every provider (Claude Code, Codex, `agy`, ACP) maps its native protocol
//! into [`AgentEvent`]. The daemon stores events, the attention state machine
//! reads them, and the UI renders them, so nothing downstream ever sees a
//! provider-specific record.
//!
//! # Lifecycles
//!
//! A thread runs one provider session at a time. Each start or resume of a
//! provider process is an **incarnation** (from 1, per thread, carried by the
//! envelope). Sequences, turn numbers and request ids are scoped to
//! `(thread, incarnation)`: a resumed session may restart them.
//!
//! **Established session**: `SessionStarted` is the first event and
//! `SessionEnded` the last. **Pre-session failure** (bad model, auth error,
//! the CLI won't start): `Error { fatal: true }` then `SessionEnded`, with no
//! `SessionStarted`, and nothing before them: diagnostics (stderr tail,
//! startup records the adapter couldn't parse) go in `Error::details`, never
//! in `Unmapped` events. Adapters never invent a native session id.
//!
//! When a session ends for any reason, the adapter first closes everything
//! still open, in this order: open messages → `MessageCompleted` (text so
//! far, see [`MessageBody`]); open tool calls → `ToolCallFinished { status: cancelled }`; pending
//! approvals and questions → `RequestResolved { outcome: cancelled }`; an open
//! turn → `TurnCompleted { outcome: interrupted | failed }`. Then
//! `SessionEnded`.
//!
//! # Mapping rules
//!
//! - Text arrives as [`AgentEvent::MessageDelta`]s followed by exactly one
//!   [`AgentEvent::MessageCompleted`] with [`MessageBody::Streamed`], which
//!   describes their concatenation without repeating it (CR-03, amended).
//!   Providers that only send whole messages emit the completed event alone
//!   with [`MessageBody::Inline`], if the text is at most
//!   [`crate::limits::MAX_INLINE_MESSAGE_BYTES`]; a longer one is first split
//!   into deltas of at most that size, on UTF-8 character boundaries, so it is
//!   `Streamed`.
//! - Every tool call is `ToolCallStarted` → zero or more `ToolCallUpdated` →
//!   exactly one `ToolCallFinished`, all with the same `call_id`. `input` is
//!   always a complete JSON value: partial provider arguments are buffered
//!   until they parse, up to [`crate::limits::MAX_PARTIAL_ARGS_BYTES`].
//! - Tool output (`output_delta` + `output`) is capped per call at
//!   [`crate::limits::MAX_TOOL_OUTPUT_BYTES`]. Past the cap the adapter drops
//!   the rest and reports how much in `ToolCallFinished::truncated_bytes`.
//! - A provider record larger than
//!   [`crate::limits::MAX_PROVIDER_RECORD_BYTES`] ends the session with a fatal
//!   `Error`.
//! - Anything that needs a human goes out as `ApprovalRequested` /
//!   `QuestionAsked`, and each one gets exactly one `RequestResolved`. A
//!   command for an unknown or already resolved request is rejected with
//!   `Error { fatal: false }` and changes nothing.
//! - `Usage` is the running total for the current turn and replaces any
//!   earlier `Usage` in that turn. `TurnCompleted::usage` is the final total
//!   for the turn. Session totals are the sum over turns. How full the
//!   context window is goes in [`AgentEvent::ContextUpdated`], never in
//!   `Usage`.
//! - `QuotaUpdated` is informational (subscription windows filling up).
//!   `RateLimited` means the provider is refusing work right now.
//! - A record the adapter can't map yet becomes [`AgentEvent::Unmapped`],
//!   never a silent drop. Its `raw` is cut to
//!   [`crate::limits::MAX_UNMAPPED_RAW_BYTES`] (replaced by a JSON string of
//!   the truncated text when longer).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::policy::EnforcementDescriptor;

/// The native agent behind a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Claude,
    Codex,
    Agy,
    Acp,
}

/// One event, as the adapter emits it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
pub struct AgentEventEnvelope {
    /// The provider process this event came from (see the module docs).
    /// Adapters emit 0; the daemon sets it when it stores the event.
    pub incarnation: u32,
    /// Per incarnation, starting at 0, no gaps.
    pub seq: u64,
    /// Unix time in milliseconds when the adapter produced the event.
    pub at_ms: u64,
    pub event: AgentEvent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
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
        /// What Atrium can enforce for this session (D-044).
        enforcement: EnforcementDescriptor,
    },
    /// The session's enforcement changed (a hook appeared, the gate became
    /// unreachable, a configuration source changed). Replaces the earlier one.
    EnforcementChanged {
        enforcement: EnforcementDescriptor,
    },
    /// A user message was accepted and the agent is working on it. The
    /// message itself follows as a `MessageKind::User` message.
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
        body: MessageBody,
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
        /// Output bytes dropped past [`crate::limits::MAX_TOOL_OUTPUT_BYTES`].
        #[serde(default)]
        truncated_bytes: u64,
    },
    /// The agent wants to do something Atrium's policy said to ask about.
    /// Answer with [`SessionCommand::ResolveApproval`]. `request_id` is the
    /// approval ticket's id from the gate (contract §5), never the
    /// provider's own id.
    ApprovalRequested {
        request_id: String,
        /// The tool call this approval gates, when the provider links them.
        call_id: Option<String>,
        name: String,
        kind: ToolKind,
        title: Option<String>,
        input: Value,
        reason: Option<String>,
        /// The gate can turn this into a rule ("always…"); when `false` the UI
        /// offers only allow/deny once.
        #[serde(default)]
        rememberable: bool,
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
    /// How full the model's context window is. Replaces any earlier value.
    ContextUpdated {
        used_tokens: u64,
        window_tokens: Option<u64>,
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
        /// Diagnostics: stderr tail, startup records that didn't parse. At
        /// most [`crate::limits::MAX_DIAGNOSTIC_LINES`] lines of at most
        /// [`crate::limits::MAX_DIAGNOSTIC_LINE_BYTES`] bytes, sanitized.
        #[serde(default)]
        details: Vec<String>,
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

/// The text of a completed message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageBody {
    /// The whole text, for completion-only messages up to
    /// [`crate::limits::MAX_INLINE_MESSAGE_BYTES`].
    Inline { text: String },
    /// The text is the in-order concatenation of this message's
    /// `MessageDelta`s in the same incarnation. A consumer that rebuilds it
    /// checks both `chars` and `sha256`; a mismatch is a hard error (a lost or
    /// reordered delta), never patched up. The store keeps the deltas as long
    /// as the completion.
    Streamed {
        /// Length in Unicode scalar values.
        chars: u64,
        /// Number of deltas.
        deltas: u64,
        /// Lowercase hex SHA-256 of the UTF-8 text.
        sha256: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    /// The user's message for this turn, emitted by the adapter right after
    /// `TurnStarted` from the `SendUserMessage` it sent (after
    /// `SessionStarted` for providers that start lazily), with the same
    /// delta/inline rules, so replay, paging and export see the whole
    /// conversation. Never sets `ThreadSummary::unread`.
    User,
    Assistant,
    /// Thinking / reasoning text, when the provider exposes it.
    Reasoning,
}

/// Coarse tool category, used by the policy engine and the UI. Mapped from
/// the provider's tool name; `Other` when unsure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
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
#[cfg_attr(feature = "ts", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Succeeded,
    Failed,
    /// Blocked by an approval decision or policy.
    Denied,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    Interrupted,
    Failed,
    /// The provider stopped on a turn or token limit.
    LimitReached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
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
#[cfg_attr(feature = "ts", derive(specta::Type))]
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
#[cfg_attr(feature = "ts", derive(specta::Type))]
pub struct QuestionOption {
    /// Stable within the question. The adapter makes one up if the provider
    /// has none (e.g. the option's index).
    pub id: String,
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
pub struct QuestionAnswer {
    pub question_id: String,
    /// [`QuestionOption::id`]s, in the order chosen.
    #[serde(default)]
    pub selected: Vec<String>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
pub struct PlanEntry {
    pub content: String,
    pub status: PlanStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

/// Token counts. Fields a provider doesn't report stay `None`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
pub struct QuotaWindow {
    /// The provider's name for the window, e.g. `5h`, `weekly`, `primary`.
    pub name: String,
    pub used_percent: Option<f64>,
    pub resets_at_ms: Option<u64>,
}

/// What a client can tell a running session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
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
#[cfg_attr(feature = "ts", derive(specta::Type))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApprovalDecision {
    Allow,
    /// `message` is passed to the agent as the reason, when the provider supports it.
    Deny {
        message: Option<String>,
    },
}
