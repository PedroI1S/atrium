//! Thread state machine, inbox ranking, notification policy (contract §8).
//! **Owned by M1-04.** Public items named in `lib.rs` are contract. All pure.

use atrium_protocol::agent::AgentEvent;
use atrium_protocol::thread::{InboxItem, ThreadState, ThreadSummary};

use crate::Signal;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pending {
    pub approvals: u32,
    pub questions: u32,
}

#[derive(Debug, Clone, Copy)]
pub enum AttentionInput<'a> {
    Event(&'a AgentEvent),
    Signal(&'a Signal),
}

/// The next state. `pending` is the count *after* the input is applied.
///
/// Precedence: `Failed` (after a fatal `Error`) and `Paused` (after
/// `RateLimited`) are sticky: the `TurnCompleted`, `SessionEnded` and
/// `Signal::Exited` that follow keep them. After `Signal::Stopped`, the exit
/// is `Ended` whatever its code. Otherwise an exit with a turn open, or a
/// non-zero code, is `Interrupted { cleanup: Verified }`.
pub fn next_state(
    current: &ThreadState,
    pending: Pending,
    input: AttentionInput<'_>,
) -> ThreadState {
    let _ = (current, pending, input);
    unimplemented!("M1-04")
}

/// The ranked inbox (contract §8). `storage_full_since_ms` adds the
/// storage-full item first.
pub fn rank_inbox(threads: &[ThreadSummary], storage_full_since_ms: Option<u64>) -> Vec<InboxItem> {
    let _ = (threads, storage_full_since_ms);
    unimplemented!("M1-04")
}

/// Whether a transition posts a notification (contract §8).
pub fn should_notify(
    previous: &ThreadState,
    next: &ThreadState,
    focused: bool,
    last_notified_ms: Option<u64>,
    now_ms: u64,
) -> bool {
    let _ = (previous, next, focused, last_notified_ms, now_ms);
    unimplemented!("M1-04")
}
