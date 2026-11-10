//! Daemon socket protocol (PLAN §3.1).
//!
//! # Framing
//!
//! Every message on the unix socket is one frame:
//!
//! ```text
//! [len: u32 big-endian][kind: u8][payload: len - 1 bytes]
//! ```
//!
//! - `kind` = [`FrameKind::Control`]: payload is UTF-8 JSON, a
//!   [`ClientMessage`] (client → daemon) or a [`ServerMessage`] (daemon → client).
//! - `kind` = [`FrameKind::Stream`]: payload is `[stream_id: u32 big-endian][bytes]`,
//!   for raw byte streams (PTY output, screencast frames). Stream ids are
//!   announced in a control message before first use.
//! - `len` covers kind + payload and never exceeds [`MAX_FRAME_LEN`]
//!   (daemon → client) or [`crate::limits::MAX_CLIENT_FRAME_LEN`] (client →
//!   daemon); a peer that receives a larger frame, an unknown kind or invalid
//!   JSON closes the connection.
//!
//! # Socket security
//!
//! The socket lives in a directory owned by the user with mode `0700`, and
//! the socket itself is `0600`. The daemon refuses to start if the directory
//! is a symlink or owned by someone else. It takes the lock file *before*
//! removing a stale socket.
//!
//! # Roles and authority (contract §4, D-045)
//!
//! The socket's permissions keep other users out, not agents running as the
//! same user. So each connection asks for a [`ClientRole`] in `Hello`, and the
//! daemon grants it only after checking the peer:
//! - [`ClientRole::App`]: the peer process must be the Atrium app binary
//!   (macOS code signature via the audit token; Linux `/proc/<pid>/exe`).
//! - [`ClientRole::Dev`]: only when a debug daemon runs with `--dev-socket`.
//! - [`ClientRole::Agent`]: reserved for the MCP server; M1 rejects it.
//!
//! A failed check is `Incompatible`, never a lesser role. Every command has a
//! required [`Authority`] ([`DaemonCommand::authority`]) checked before it
//! does anything. Subscriptions need `App` too in M1. An unconfined same-user
//! agent can still modify Atrium's files; peer checks raise the bar, they are
//! not isolation.
//!
//! # Session
//!
//! 1. Client sends [`ClientMessage::Hello`] within
//!    [`crate::limits::HELLO_TIMEOUT`]. The daemon answers
//!    [`ServerMessage::Welcome`] with the granted role, or
//!    [`ServerMessage::Incompatible`] and closes. Any other message before
//!    `Hello` closes the connection.
//! 2. Client sends [`ClientMessage::Subscribe`] with a cursor (see below).
//!    The daemon answers with a [`ServerMessage::Response`]: `Ok` with
//!    `{"replay_until": EventId|null}` (the thread's last event at that
//!    moment), or `Err`: [`ErrorCode::UnknownThread`] for an unknown thread,
//!    [`ErrorCode::Invalid`] for a cursor beyond the thread's last event.
//!    After `Ok` it sends every stored event after the cursor up to
//!    `replay_until`, then [`ServerMessage::ReplayDone`] with `last` equal
//!    to `replay_until`, then live events. No event is skipped or duplicated
//!    across the replay/live boundary.
//!    Subscribing again to the same thread on the same connection replaces
//!    the earlier subscription and replays from the new cursor. **Once the
//!    daemon has answered a `Subscribe` (or processed an `Unsubscribe`), it
//!    sends no `Event`, `ReplayDone` or `SubscriptionLagged` of an earlier
//!    subscription to that thread on that connection, frames it had already
//!    queued included** (CR-07, D-058). So everything a client receives for
//!    a thread after a subscribe response belongs to that subscription.
//! 3. Commands carry a client-chosen `request_id`; the daemon answers each with
//!    exactly one [`ServerMessage::Response`].
//! 4. [`ClientMessage::SubscribeInbox`] is answered with `Response { Ok
//!    {"threads": [ThreadSummary], "inbox": [InboxItem]} }` (the full current
//!    set), then [`ServerMessage::ThreadUpdated`],
//!    [`ServerMessage::ThreadRemoved`] and [`ServerMessage::InboxUpdated`]
//!    for every later change, in order, on the control lane.
//!
//! # Lanes and slow readers
//!
//! The daemon writes each connection's **control lane** (responses,
//! `ThreadUpdated`, `ThreadRemoved`, `InboxUpdated`, `SubscriptionLagged`)
//! before its **event
//! lane** (replay pages and live events), so a command is never stuck behind
//! replay. The event lane holds at most
//! [`crate::limits::MAX_CONNECTION_QUEUE_BYTES`]. When a subscription would
//! overflow it, the daemon drops that subscription and sends
//! [`ServerMessage::SubscriptionLagged`]; the client resubscribes from its
//! applied cursor. Other subscriptions are unaffected.
//!
//! # Cursors and retries
//!
//! - A client's cursor for a thread is the last event its *consumer has
//!   applied*, not the last one read off the socket. On reconnect it
//!   subscribes with that cursor, so events that were read but not applied
//!   are sent again; consumers ignore ids at or below their cursor.
//! - Commands are **never retried automatically**. If the connection drops
//!   before a command's response arrives, the client reports an "unknown
//!   outcome" error to its caller, who re-reads state (e.g. `ListThreads`)
//!   before deciding to send it again.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::{
    AgentEventEnvelope, ApprovalDecision, Provider, QuestionAnswer, SessionCommand,
};
use crate::policy::{EnforcementDescriptor, Preset, RememberScope, RuleDraft, RuleId};
use crate::thread::{InboxItem, ThreadSummary};

pub use crate::thread::ThreadId;

pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    Control = 0,
    Stream = 1,
}

impl TryFrom<u8> for FrameKind {
    type Error = u8;

    fn try_from(value: u8) -> Result<Self, u8> {
        match value {
            0 => Ok(Self::Control),
            1 => Ok(Self::Stream),
            other => Err(other),
        }
    }
}

/// Global, strictly increasing, assigned by the daemon's store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
#[serde(transparent)]
pub struct EventId(pub u64);

/// An agent event after the daemon has stored it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
pub struct StoredEvent {
    pub id: EventId,
    pub thread_id: ThreadId,
    pub envelope: AgentEventEnvelope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientRole {
    /// The Atrium app. Human-control authority, after peer verification.
    App,
    /// Tests and harnesses against a debug daemon started with `--dev-socket`.
    Dev,
    /// Reserved for the MCP server (per-thread scope). Rejected in M1.
    Agent { token: String },
}

/// The authority a command needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum Authority {
    /// Read and act within one thread the connection is scoped to.
    ThreadScoped,
    /// Human control: approvals, answers, rules, settings, every thread.
    HumanControl,
}

impl ClientRole {
    pub fn authority(&self) -> Authority {
        match self {
            ClientRole::App | ClientRole::Dev => Authority::HumanControl,
            ClientRole::Agent { .. } => Authority::ThreadScoped,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        protocol_version: u32,
        client_name: String,
        role: ClientRole,
    },
    Subscribe {
        request_id: u64,
        thread_id: ThreadId,
        /// The last event the consumer applied; `None` replays from the start.
        after: Option<EventId>,
    },
    Unsubscribe {
        thread_id: ThreadId,
    },
    /// Thread summaries and the inbox, for every thread (see the module docs).
    SubscribeInbox {
        request_id: u64,
    },
    Command {
        request_id: u64,
        command: DaemonCommand,
    },
}

/// Commands. Responses are [`CommandResult::Ok`] with the value given on each
/// variant (`null` when none is given), or [`CommandResult::Err`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonCommand {
    /// Starts a Home thread. Value: [`ThreadSummary`].
    StartThread {
        agent: AgentLaunch,
        /// The first user message, sent once the session starts.
        message: Option<String>,
    },
    /// Value: `Vec<ThreadSummary>`, newest first, archived ones excluded.
    ListThreads,
    /// Runs `AgentAdapter::probe` for every provider with this preset, for
    /// the provider picker and settings. Value: `Vec<ProviderProbe>`.
    ProbeProviders {
        preset: Preset,
    },
    RenameThread {
        thread_id: ThreadId,
        title: String,
    },
    /// Value: [`ThreadSummary`].
    GetThread {
        thread_id: ThreadId,
    },
    /// `SendUserMessage`, `Interrupt` and `Shutdown` only. Approvals and
    /// answers use their own commands so the ledger sees them first.
    ThreadCommand {
        thread_id: ThreadId,
        command: SessionCommand,
    },
    /// Validated by the approval ledger before the session sees it (D-046).
    /// An unknown, resolved, expired or older-incarnation request is
    /// `Err { code: Expired }`. `remember` with `Allow` also creates a
    /// rule in the same transaction. Value: `{"rule_id": RuleId|null}`.
    ResolveApproval {
        thread_id: ThreadId,
        incarnation: u32,
        request_id: String,
        decision: ApprovalDecision,
        remember: Option<RememberScope>,
    },
    /// Same ledger checks as `ResolveApproval`.
    AnswerQuestion {
        thread_id: ThreadId,
        incarnation: u32,
        request_id: String,
        answers: Vec<QuestionAnswer>,
    },
    /// Starts a new incarnation from the last native session id.
    /// Value: [`ThreadSummary`].
    ResumeThread {
        thread_id: ThreadId,
    },
    /// Ends the session (if running) and hides the thread from lists.
    ArchiveThread {
        thread_id: ThreadId,
    },
    MarkRead {
        thread_id: ThreadId,
    },
    /// The thread the user is looking at in some window (`None`: none), for
    /// notification suppression.
    SetFocus {
        thread_id: Option<ThreadId>,
    },
    /// Value: `Vec<InboxItem>`, ranked.
    ListInbox,
    /// Value: `Vec<Rule>`.
    ListRules,
    /// The daemon assigns id, origin (`Settings`) and time. Value: `Rule`.
    CreateRule {
        rule: RuleDraft,
    },
    DeleteRule {
        rule_id: RuleId,
    },
    /// Writes `path` (mode 0600). Value: `{"bytes": u64}`.
    ExportThread {
        thread_id: ThreadId,
        format: ExportFormat,
        path: String,
    },
    /// Value: `{key: value}` for every setting.
    GetSettings,
    SetSetting {
        key: String,
        value: Value,
    },
    /// Upgrades (contract §10). Without `force`: exits now if no turn is
    /// running, else `Err { code: TurnsRunning }` and nothing changes. With
    /// `force`: running turns are interrupted (resumable) and it exits.
    ShutdownDaemon {
        force: bool,
    },
}

impl DaemonCommand {
    /// Checked by the daemon before anything else. Every M1 command needs
    /// human control; the table exists so later roles can't silently gain
    /// these commands.
    pub fn authority(&self) -> Authority {
        Authority::HumanControl
    }
}

/// One provider's probe result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
pub struct ProviderProbe {
    pub provider: Provider,
    /// `None` when the CLI isn't installed.
    pub enforcement: Option<EnforcementDescriptor>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Markdown,
    Jsonl,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentLaunch {
    /// A real provider session. The daemon validates it (provider enabled,
    /// model known if the provider lists models) and adds its own MCP
    /// servers; clients never supply MCP commands.
    Native {
        provider: Provider,
        model: Option<String>,
        effort: Option<String>,
        /// The user's choice; Home threads default to `Preset::Home`.
        preset: Preset,
    },
    /// A synthetic agent process for transport and rendering tests: it emits
    /// `MessageDelta` events of `chunk_chars` characters at
    /// `tokens_per_second` (1 token ≈ 4 chars) for `duration_ms`, with a tool
    /// call every `tool_call_every_ms` (0 = none), then `TurnCompleted`.
    Fake {
        tokens_per_second: u32,
        chunk_chars: u32,
        duration_ms: u64,
        tool_call_every_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome {
        protocol_version: u32,
        daemon_version: String,
        daemon_pid: u32,
        /// The role granted (always the one asked for; otherwise `Incompatible`).
        role: ClientRole,
    },
    /// Sent before closing. Carries what an upgraded app needs to replace an
    /// older daemon (contract §10): its pid (also in the lock file, with its
    /// process identity) and whether turns are running.
    Incompatible {
        protocol_version: u32,
        message: String,
        daemon_version: String,
        daemon_pid: u32,
        running_turns: u32,
    },
    Event {
        event: StoredEvent,
    },
    /// Everything up to `last` (the subscribe response's `replay_until`) has
    /// been sent; live events follow.
    ReplayDone {
        thread_id: ThreadId,
        last: Option<EventId>,
    },
    Response {
        request_id: u64,
        result: CommandResult,
    },
    /// Control lane, for `SubscribeInbox` connections.
    ThreadUpdated {
        summary: ThreadSummary,
    },
    /// Control lane, for `SubscribeInbox` connections: the thread was
    /// archived and has left the list, whoever archived it (the connection
    /// that sent `ArchiveThread` gets this too). No `ThreadUpdated` for it
    /// follows.
    ThreadRemoved {
        thread_id: ThreadId,
    },
    /// Control lane: the whole ranked inbox, replacing the previous one.
    InboxUpdated {
        items: Vec<InboxItem>,
    },
    /// The thread's subscription was dropped because this connection's event
    /// lane was full. Resubscribe from the applied cursor.
    SubscriptionLagged {
        thread_id: ThreadId,
        last_sent: Option<EventId>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CommandResult {
    Ok { value: Value },
    Err { code: ErrorCode, message: String },
}

/// What clients branch on; `message` is for people.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The approval or question is unknown, resolved, expired, or from an
    /// older incarnation. The UI shows it as expired.
    Expired,
    UnknownThread,
    /// The thread has no live session (resume it first).
    NotRunning,
    StorageReadOnly,
    TurnsRunning,
    /// The connection's role can't send this command.
    Forbidden,
    Invalid,
    Internal,
}
