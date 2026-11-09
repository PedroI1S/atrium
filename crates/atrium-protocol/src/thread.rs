//! Threads and attention (PLAN §3.3, §3.4; contract §8).
//!
//! `ThreadState` is the M1 part of PLAN §3.4's state machine. The states for
//! checks, review and merge arrive with M2 (a protocol version bump).
//! Transitions are computed by `atrium-core::attention::next_state`, a pure
//! function; nothing else sets a thread's state.

use serde::{Deserialize, Serialize};

use crate::agent::Provider;
use crate::policy::EnforcementDescriptor;
use crate::wire::EventId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ThreadId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cleanup {
    /// The provider's process group was killed and reaped.
    Verified,
    /// The recorded process couldn't be identified (pid reused, executable
    /// changed), so Atrium didn't kill it and can't say it stopped.
    Unverified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThreadState {
    /// The provider is starting (before `SessionStarted`).
    Starting,
    /// A turn is running.
    Running,
    /// At least one approval is pending.
    NeedsApproval,
    /// At least one question is pending (and no approval).
    NeedsInput,
    /// The session is alive and waiting for the user's next message.
    Idle,
    /// The provider refused work because of a limit.
    Paused { resets_at_ms: Option<u64> },
    /// The session ended with a fatal error.
    Failed { message: String },
    /// The session was cut off: daemon restart, crash, the provider died, or
    /// the store went read-only. Resumable by native session id.
    Interrupted { cleanup: Cleanup },
    /// The session ended normally (user stop or provider exit 0).
    Ended,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Location {
    /// Home: cwd `~`, policy "read freely, approve changes".
    Home,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadSummary {
    pub thread_id: ThreadId,
    /// From the first user message until the user renames it.
    pub title: String,
    pub provider: Provider,
    pub model: Option<String>,
    pub location: Location,
    pub state: ThreadState,
    /// The current incarnation's, once it started.
    pub enforcement: Option<EnforcementDescriptor>,
    pub incarnation: u32,
    /// The current incarnation's native session id, once known.
    pub native_session_id: Option<String>,
    pub last_event: Option<EventId>,
    /// New assistant output since the user last looked.
    pub unread: bool,
    pub pending_approvals: u32,
    pub pending_questions: u32,
    /// Since when the thread has needed the user (pending request, failure,
    /// unverified cleanup, unread reply); `None` when it doesn't.
    pub attention_since_ms: Option<u64>,
    /// The ask itself, for the inbox: the oldest pending approval's title,
    /// the question's first prompt, the error message.
    pub attention_detail: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxReason {
    // Declaration order is the ranking (contract §8): earlier ranks higher.
    StorageFull,
    Approval,
    Question,
    Failed,
    CleanupUnverified,
    Reply,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxItem {
    /// `None` only for `StorageFull`.
    pub thread_id: Option<ThreadId>,
    pub reason: InboxReason,
    /// When the item started needing the user (oldest first within a reason).
    pub since_ms: u64,
    pub title: String,
    /// The ask itself: the approval's title, the question's first prompt, the
    /// error message.
    pub detail: Option<String>,
}
