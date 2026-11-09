//! Atrium's domain (PLAN §3.3, §3.4, §3.12; contract §5, §7–§9, §13).
//!
//! **Contract, M1 revision 1 (draft, `docs/contracts/m1.md`).** This file and
//! `schema/` are the orchestrator's. Each module directory is owned by one
//! task, which implements the items this file names and may add private code
//! freely, but never changes the public items listed here:
//!
//! | Module | Task | Implements |
//! |---|---|---|
//! | [`store`] | M1-02 | [`Store`] as `store::SqliteStore` |
//! | [`policy`] | M1-03 | [`PolicyEngine`], [`ApprovalLedger`], `policy::SessionGate` ([`atrium_agents::ToolGate`]), `policy::canonical_input`, `policy::parse_command` |
//! | [`attention`] | M1-04 | `attention::next_state`, `attention::rank_inbox`, `attention::should_notify` |
//! | [`history`] | M1-11 | `history::reconstruct_message`, `history::export_thread`, snapshots scheduling |

use std::path::{Path, PathBuf};
use std::sync::Arc;

use atrium_agents::{ApprovalTicket, BoxFuture, ToolCheck};
use atrium_host::supervise::ProcessIdentity;
use atrium_protocol::agent::{AgentEventEnvelope, ApprovalDecision, Provider};
use atrium_protocol::policy::{
    EnforcementDescriptor, PolicyContext, Preset, RememberScope, Rule, RuleId,
};
use atrium_protocol::thread::{Cleanup, Location, ThreadId, ThreadSummary};
use atrium_protocol::wire::{EventId, StoredEvent};
use serde_json::Value;

pub mod attention;
pub mod history;
pub mod policy;
pub mod store;

// ---------------------------------------------------------------------------
// Store (contract §7)
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Free space fell below `limits::MIN_FREE_DISK_BYTES`; writes are refused
    /// until it recovers.
    #[error("storage is full ({free_bytes} bytes free)")]
    ReadOnly { free_bytes: u64 },
    #[error("unknown thread {0:?}")]
    UnknownThread(ThreadId),
    /// A migration failed and was rolled back; the daemon must not start.
    #[error("migration {version} failed: {message}")]
    Migration { version: u32, message: String },
    #[error("event too large ({bytes} bytes)")]
    EventTooLarge { bytes: usize },
    #[error("sqlite: {0}")]
    Sqlite(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewThread {
    pub provider: Provider,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub preset: Preset,
    pub location: Location,
    pub title: String,
}

/// Inputs to the thread state machine that aren't agent events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    /// The provider process for this incarnation is being started.
    Starting,
    /// The provider process exited (after its closing events were stored).
    Exited { code: Option<i32> },
    /// The daemon restarted and found this incarnation's process.
    Recovered { cleanup: Cleanup },
    /// The store went read-only while a turn ran.
    StorageReadOnly,
}

/// What is still open in an incarnation: what the closing events are built
/// from, without reading the history.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenItems {
    /// `(message_id, kind)`; `chars`/`deltas`/`sha256` come from the store.
    pub messages: Vec<OpenMessage>,
    pub tool_calls: Vec<String>,
    pub requests: Vec<String>,
    pub turn: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenMessage {
    pub message_id: String,
    pub kind: atrium_protocol::agent::MessageKind,
    pub chars: u64,
    pub deltas: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageStatus {
    Ok { free_bytes: u64 },
    ReadOnly { free_bytes: u64 },
}

/// The daemon's only persistence. One writer task; every write method is one
/// SQLite transaction that updates the events and every projection it
/// touches together.
pub trait Store: Send + Sync {
    fn create_thread(&self, new: NewThread) -> BoxFuture<'_, StoreResult<ThreadSummary>>;

    /// Starts the next incarnation (the first is 1) and records the process
    /// once known. Returns the new incarnation.
    fn begin_incarnation(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<u32>>;
    fn set_process(
        &self,
        thread: &ThreadId,
        incarnation: u32,
        process: Option<ProcessIdentity>,
    ) -> BoxFuture<'_, StoreResult<()>>;

    /// Appends one adapter batch for `(thread, incarnation)`, sets the
    /// envelopes' `incarnation`, assigns `EventId`s, and updates the thread
    /// (state via `attention::next_state`), session, turn, open-item and
    /// request projections in the same transaction. A `MessageCompleted` is
    /// checked against the open message's chars/deltas/hash; a mismatch is
    /// stored as given but logged as an integrity error.
    fn append(
        &self,
        thread: &ThreadId,
        incarnation: u32,
        events: Vec<AgentEventEnvelope>,
    ) -> BoxFuture<'_, StoreResult<Vec<StoredEvent>>>;

    /// Applies a [`Signal`] to the thread's state in one transaction.
    fn signal(
        &self,
        thread: &ThreadId,
        signal: Signal,
    ) -> BoxFuture<'_, StoreResult<ThreadSummary>>;

    /// Replay paging: events after `after`, oldest first, at most `limit`.
    fn events_after(
        &self,
        thread: &ThreadId,
        after: Option<EventId>,
        limit: usize,
    ) -> BoxFuture<'_, StoreResult<Vec<StoredEvent>>>;
    fn last_event(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<Option<EventId>>>;

    fn thread(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<Option<ThreadSummary>>>;
    /// Not archived, newest first.
    fn threads(&self) -> BoxFuture<'_, StoreResult<Vec<ThreadSummary>>>;
    fn archive(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<()>>;
    fn mark_read(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<()>>;
    fn set_enforcement(
        &self,
        thread: &ThreadId,
        incarnation: u32,
        descriptor: EnforcementDescriptor,
    ) -> BoxFuture<'_, StoreResult<()>>;
    /// The native session id of the latest incarnation that has one.
    fn resume_id(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<Option<String>>>;

    fn open_items(
        &self,
        thread: &ThreadId,
        incarnation: u32,
    ) -> BoxFuture<'_, StoreResult<OpenItems>>;
    /// Incarnations with a recorded process and no end: crash recovery.
    fn live_processes(&self) -> BoxFuture<'_, StoreResult<Vec<(ThreadId, u32, ProcessIdentity)>>>;

    fn rules(&self) -> BoxFuture<'_, StoreResult<(u64, Vec<Rule>)>>;
    fn create_rule(&self, rule: Rule) -> BoxFuture<'_, StoreResult<Rule>>;
    fn delete_rule(&self, rule: &RuleId) -> BoxFuture<'_, StoreResult<()>>;

    fn settings(&self) -> BoxFuture<'_, StoreResult<Vec<(String, Value)>>>;
    fn set_setting(&self, key: &str, value: Value) -> BoxFuture<'_, StoreResult<()>>;

    fn storage_status(&self) -> StorageStatus;
    /// `VACUUM INTO` a dated file in `dir` (mode 0600), pruning to
    /// `limits::SNAPSHOTS_KEPT`. Returns the new file.
    fn snapshot(&self, dir: &Path) -> BoxFuture<'_, StoreResult<PathBuf>>;

    /// The ledger's persistence (see [`ApprovalLedger`]).
    fn requests(&self) -> Arc<dyn RequestStore>;
}

// ---------------------------------------------------------------------------
// Policy and approvals (contract §5, §9)
// ---------------------------------------------------------------------------

/// Everything a policy decision needs about the call's context.
#[derive(Debug, Clone)]
pub struct PolicyInput<'a> {
    pub check: &'a ToolCheck,
    pub thread: &'a ThreadId,
    pub provider: Provider,
    /// Absolute, symlinks resolved.
    pub cwd: &'a Path,
    pub preset: Preset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyVerdict {
    Allow {
        rule: Option<RuleId>,
    },
    Deny {
        reason: String,
        rule: Option<RuleId>,
    },
    /// One of `HARD_DENY_DEFAULTS`. Never turned into an `Ask`, never
    /// overridden.
    HardDeny {
        id: &'static str,
    },
    Ask,
}

/// Pure: no I/O, no clock. Order: hard deny, then thread rules, Home rules,
/// global rules (deny before allow within a scope), then the preset.
pub trait PolicyEngine: Send + Sync {
    fn evaluate(&self, input: &PolicyInput<'_>, rules: &[Rule]) -> PolicyVerdict;
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestKey {
    pub thread: ThreadId,
    pub incarnation: u32,
    pub request_id: String,
}

/// What an approval is bound to (D-046).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub turn: u32,
    pub tool_name: String,
    pub tool_kind: atrium_protocol::agent::ToolKind,
    /// SHA-256 of `policy::canonical_input`.
    pub input_sha256: String,
    pub context: PolicyContext,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LedgerError {
    #[error("unknown request")]
    Unknown,
    #[error("already resolved")]
    AlreadyResolved,
    /// Shown to the user as "expired".
    #[error("expired")]
    Expired,
    #[error("from an older session")]
    StaleIncarnation,
    /// `redeem`: the call's input no longer matches what was approved.
    #[error("doesn't match the approved call")]
    Mismatch,
    #[error("store unavailable")]
    Store,
}

/// The ledger's rows (`requests` table). Implemented by the store (M1-02),
/// used only by the ledger (M1-03). Each method is one transaction.
pub trait RequestStore: Send + Sync {
    fn open_approval(&self, key: &RequestKey, binding: &Binding) -> BoxFuture<'_, StoreResult<()>>;
    fn open_question(&self, key: &RequestKey, turn: Option<u32>) -> BoxFuture<'_, StoreResult<()>>;
    /// Compare-and-set on `status`; returns the status found.
    fn transition(
        &self,
        key: &RequestKey,
        from: &str,
        to: &str,
    ) -> BoxFuture<'_, StoreResult<Option<String>>>;
    /// Approves and creates the rule in one transaction.
    fn approve_with_rule(
        &self,
        key: &RequestKey,
        rule: Option<Rule>,
    ) -> BoxFuture<'_, StoreResult<Option<String>>>;
    fn binding(&self, key: &RequestKey) -> BoxFuture<'_, StoreResult<Option<(String, Binding)>>>;
    /// Marks every `pending`/`approved` row matching the filter `expired`.
    /// `turn: None` expires the whole incarnation; `thread: None` everything
    /// (daemon startup).
    fn expire(
        &self,
        thread: Option<&ThreadId>,
        incarnation: Option<u32>,
        turn: Option<u32>,
    ) -> BoxFuture<'_, StoreResult<u64>>;
}

/// The single authority over approvals and questions (D-046). States:
/// `pending → approved → redeemed`, `pending → denied | answered |
/// cancelled | expired`, `approved → expired`. Nothing leaves `redeemed`,
/// `denied`, `answered`, `cancelled` or `expired`.
pub trait ApprovalLedger: Send + Sync {
    /// Opens a pending approval bound to the call. The ticket's id is new
    /// and unique per `(thread, incarnation)`.
    fn open_approval(
        &self,
        thread: &ThreadId,
        incarnation: u32,
        binding: Binding,
    ) -> BoxFuture<'_, Result<ApprovalTicket, LedgerError>>;
    fn open_question(
        &self,
        thread: &ThreadId,
        incarnation: u32,
        turn: Option<u32>,
    ) -> BoxFuture<'_, Result<ApprovalTicket, LedgerError>>;

    /// From an App connection, before the session sees anything. `Allow`
    /// moves to `approved` (and creates the rule for `remember`); `Deny` to
    /// `denied`. Returns the created rule's id.
    fn resolve(
        &self,
        key: &RequestKey,
        decision: &ApprovalDecision,
        remember: Option<RememberScope>,
    ) -> BoxFuture<'_, Result<Option<RuleId>, LedgerError>>;
    /// `pending → answered`.
    fn answer(&self, key: &RequestKey) -> BoxFuture<'_, Result<(), LedgerError>>;

    /// `approved → redeemed`, once, if `check` still hashes to the binding.
    fn redeem(&self, key: &RequestKey, check: &ToolCheck)
    -> BoxFuture<'_, Result<(), LedgerError>>;

    /// The provider withdrew or timed out a request (`RequestResolved`
    /// cancelled/expired was stored).
    fn withdraw(&self, key: &RequestKey) -> BoxFuture<'_, Result<(), LedgerError>>;
    fn expire_turn(&self, thread: &ThreadId, incarnation: u32, turn: u32) -> BoxFuture<'_, u64>;
    fn expire_incarnation(&self, thread: &ThreadId, incarnation: u32) -> BoxFuture<'_, u64>;
    /// Daemon startup, before any session starts.
    fn expire_all(&self) -> BoxFuture<'_, u64>;
}
