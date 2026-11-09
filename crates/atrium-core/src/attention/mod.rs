//! Thread state machine, inbox ranking, notification policy (contract §8).
//! **Owned by M1-04.** Public items named in `lib.rs` are contract. All pure.

use atrium_protocol::agent::AgentEvent;
use atrium_protocol::limits::NOTIFY_MIN_INTERVAL;
use atrium_protocol::thread::{
    Cleanup, InboxItem, InboxReason, ThreadId, ThreadState, ThreadSummary,
};

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
/// - `Signal::Starting` → `Starting`, from any state (a new incarnation).
/// - `Signal::Stopped` (the user stopped it) → `Ended` at once.
/// - Sticky until the next `Signal::Starting`: `Failed` (after a fatal
///   `Error`), `Ended` and `Interrupted`; the `TurnCompleted`,
///   `SessionEnded` and `Signal::Exited` that follow keep them. `Paused`
///   (after `RateLimited`) is kept by those closing events too.
/// - Otherwise, the end of a session (`SessionEnded`, or `Signal::Exited`
///   if it comes first): exit code `Some(0)` → `Ended`; any other code or
///   `None` → `Interrupted { cleanup: Verified }`. (The closing
///   `TurnCompleted` arrives first, so whether a turn was open isn't
///   visible here; a provider that exits 0 mid-turn shows as `Ended`, and
///   can still be resumed.)
/// - `Signal::Recovered { cleanup }` → `Interrupted { cleanup }`;
///   `Signal::StorageReadOnly` → `Interrupted { cleanup: Verified }`.
/// - While alive: pending approvals → `NeedsApproval`; else pending
///   questions → `NeedsInput`; else a turn running → `Running`; else
///   `Idle`.
pub fn next_state(
    current: &ThreadState,
    pending: Pending,
    input: AttentionInput<'_>,
) -> ThreadState {
    // A new incarnation, or the user stopping the session, beats everything.
    if let AttentionInput::Signal(signal) = input {
        match signal {
            Signal::Starting => return ThreadState::Starting,
            Signal::Stopped => return ThreadState::Ended,
            _ => {}
        }
    }

    // Failed, Ended and Interrupted stay until the next start.
    if matches!(
        current,
        ThreadState::Failed { .. } | ThreadState::Ended | ThreadState::Interrupted { .. }
    ) {
        return current.clone();
    }

    let paused = matches!(current, ThreadState::Paused { .. });
    match input {
        AttentionInput::Signal(Signal::Recovered { cleanup }) => {
            ThreadState::Interrupted { cleanup: *cleanup }
        }
        AttentionInput::Signal(Signal::StorageReadOnly) => ThreadState::Interrupted {
            cleanup: Cleanup::Verified,
        },
        AttentionInput::Signal(Signal::Exited { code }) => exit_state(current, *code),
        AttentionInput::Signal(Signal::Starting | Signal::Stopped) => current.clone(),
        AttentionInput::Event(event) => match event {
            AgentEvent::SessionEnded { exit_code, .. } => exit_state(current, *exit_code),
            AgentEvent::Error {
                message,
                fatal: true,
                ..
            } => ThreadState::Failed {
                message: message.clone(),
            },
            AgentEvent::RateLimited { resets_at_ms, .. } => ThreadState::Paused {
                resets_at_ms: *resets_at_ms,
            },
            // Closing events keep a pause.
            AgentEvent::TurnCompleted { .. } if paused => current.clone(),
            AgentEvent::TurnCompleted { .. } => alive(current, pending, false),
            AgentEvent::TurnStarted { .. } => alive(current, pending, true),
            AgentEvent::SessionStarted { .. } if matches!(current, ThreadState::Starting) => {
                alive(&ThreadState::Idle, pending, false)
            }
            _ => alive(current, pending, turn_open(current)),
        },
    }
}

fn turn_open(state: &ThreadState) -> bool {
    matches!(
        state,
        ThreadState::Running | ThreadState::NeedsApproval | ThreadState::NeedsInput
    )
}

/// The end of a session: exit code 0 is `Ended`, anything else `Interrupted`.
/// A pause is kept.
fn exit_state(current: &ThreadState, code: Option<i32>) -> ThreadState {
    if matches!(current, ThreadState::Paused { .. }) {
        return current.clone();
    }
    match code {
        Some(0) => ThreadState::Ended,
        _ => ThreadState::Interrupted {
            cleanup: Cleanup::Verified,
        },
    }
}

/// The state of a live session. `Starting` and `Paused` are kept unless
/// something is pending (or, for `Paused`, a turn starts).
fn alive(current: &ThreadState, pending: Pending, turn_open: bool) -> ThreadState {
    if pending.approvals > 0 {
        return ThreadState::NeedsApproval;
    }
    if pending.questions > 0 {
        return ThreadState::NeedsInput;
    }
    match current {
        ThreadState::Starting | ThreadState::Paused { .. } if !turn_open => current.clone(),
        _ if turn_open => ThreadState::Running,
        _ => ThreadState::Idle,
    }
}

fn reason_of(thread: &ThreadSummary) -> Option<InboxReason> {
    match &thread.state {
        ThreadState::NeedsApproval => Some(InboxReason::Approval),
        ThreadState::NeedsInput => Some(InboxReason::Question),
        ThreadState::Failed { .. } => Some(InboxReason::Failed),
        ThreadState::Interrupted {
            cleanup: Cleanup::Unverified,
        } => Some(InboxReason::CleanupUnverified),
        _ if thread.unread => Some(InboxReason::Reply),
        _ => None,
    }
}

/// The ranked inbox (contract §8). `storage_full_since_ms` adds the
/// storage-full item first.
pub fn rank_inbox(threads: &[ThreadSummary], storage_full_since_ms: Option<u64>) -> Vec<InboxItem> {
    let mut ranked: Vec<(InboxReason, u64, &ThreadId, InboxItem)> = threads
        .iter()
        .filter_map(|t| {
            let reason = reason_of(t)?;
            let since_ms = t.attention_since_ms.unwrap_or(t.updated_at_ms);
            Some((
                reason,
                since_ms,
                &t.thread_id,
                InboxItem {
                    thread_id: Some(t.thread_id.clone()),
                    reason,
                    since_ms,
                    title: t.title.clone(),
                    detail: t.attention_detail.clone(),
                },
            ))
        })
        .collect();
    ranked.sort_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));

    let mut items = Vec::with_capacity(ranked.len() + 1);
    if let Some(since_ms) = storage_full_since_ms {
        items.push(InboxItem {
            thread_id: None,
            reason: InboxReason::StorageFull,
            since_ms,
            title: "Storage is full".to_string(),
            detail: None,
        });
    }
    items.extend(ranked.into_iter().map(|r| r.3));
    items
}

/// Whether a transition posts a notification (contract §8).
pub fn should_notify(
    previous: &ThreadState,
    next: &ThreadState,
    focused: bool,
    last_notified_ms: Option<u64>,
    now_ms: u64,
) -> bool {
    if focused || std::mem::discriminant(previous) == std::mem::discriminant(next) {
        return false;
    }
    if !matches!(
        next,
        ThreadState::NeedsApproval | ThreadState::NeedsInput | ThreadState::Failed { .. }
    ) {
        return false;
    }
    match last_notified_ms {
        Some(last) => now_ms.saturating_sub(last) >= NOTIFY_MIN_INTERVAL.as_millis() as u64,
        None => true,
    }
}
