//! Who is told what: live subscribers of each thread, the connections that
//! follow the inbox, and the thread list they are shown.
//!
//! Everything here is behind one mutex and never waits. The thread list is a
//! copy of the store's summaries, refreshed by the daemon when something
//! that shows in a summary changed, so ranking the inbox never reads the
//! store.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use atrium_core::attention;
use atrium_protocol::thread::{InboxItem, ThreadId, ThreadState, ThreadSummary};
use atrium_protocol::wire::{ServerMessage, StoredEvent};
use serde_json::{Value, json};

use crate::codec;
use crate::conn::{Coalesce, Conn, ConnId, Pushed, SubId};

/// A transition the user should hear about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub thread_id: ThreadId,
    pub title: String,
    pub reason: NotifyReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyReason {
    Approval,
    Question,
    Failed,
}

#[derive(Default)]
pub(crate) struct Hub {
    /// Connections that asked for the inbox.
    inbox: HashMap<ConnId, Arc<Conn>>,
    /// Live subscriptions by thread.
    live: HashMap<ThreadId, Vec<(Arc<Conn>, SubId)>>,
    /// Every thread that isn't archived.
    threads: BTreeMap<ThreadId, ThreadSummary>,
    ranked: Vec<InboxItem>,
    /// The thread each connection says the user is looking at.
    focus: HashMap<ConnId, ThreadId>,
    storage_full_since: Option<u64>,
    notified: HashMap<ThreadId, u64>,
    connections: HashMap<ConnId, Arc<Conn>>,
}

impl Hub {
    pub(crate) fn load(&mut self, threads: Vec<ThreadSummary>) {
        self.threads = threads
            .into_iter()
            .map(|summary| (summary.thread_id.clone(), summary))
            .collect();
        self.ranked = self.rank();
    }

    fn rank(&self) -> Vec<InboxItem> {
        let threads: Vec<ThreadSummary> = self.threads.values().cloned().collect();
        attention::rank_inbox(&threads, self.storage_full_since)
    }

    pub(crate) fn add_conn(&mut self, conn: &Arc<Conn>) {
        self.connections.insert(conn.id, conn.clone());
    }

    /// Closes every connection once its answers are written (the daemon is
    /// stopping).
    pub(crate) fn finish_all(&mut self) {
        for conn in self.connections.values() {
            conn.finish();
        }
    }

    /// The connection is gone: nothing more is queued for it.
    pub(crate) fn remove_conn(&mut self, id: ConnId) {
        self.connections.remove(&id);
        self.inbox.remove(&id);
        self.focus.remove(&id);
        self.live.retain(|_, subs| {
            subs.retain(|(conn, _)| conn.id != id);
            !subs.is_empty()
        });
    }

    pub(crate) fn inbox(&self) -> &[InboxItem] {
        &self.ranked
    }

    pub(crate) fn thread(&self, thread: &ThreadId) -> Option<&ThreadSummary> {
        self.threads.get(thread)
    }

    /// Answers `SubscribeInbox` with everything, and sends the changes from
    /// now on. The answer is queued here, under the lock, so no update can
    /// come before it or be missed after it.
    pub(crate) fn subscribe_inbox(&mut self, conn: &Arc<Conn>, request_id: u64) {
        let mut threads: Vec<&ThreadSummary> = self.threads.values().collect();
        threads.sort_by_key(|summary| std::cmp::Reverse(summary.created_at_ms));
        conn.ok(
            request_id,
            json!({ "threads": threads, "inbox": self.ranked }),
        );
        self.inbox.insert(conn.id, conn.clone());
    }

    fn to_inbox(&self, message: &ServerMessage, key: Coalesce) {
        if self.inbox.is_empty() {
            return;
        }
        let Some(frame) = codec::encode(message) else {
            return;
        };
        for conn in self.inbox.values() {
            conn.control_frame(frame.clone(), key.clone());
        }
    }

    fn refresh_inbox(&mut self) {
        let ranked = self.rank();
        if ranked != self.ranked {
            self.ranked = ranked;
            self.to_inbox(
                &ServerMessage::InboxUpdated {
                    items: self.ranked.clone(),
                },
                Coalesce::Inbox,
            );
        }
    }

    /// A thread was created.
    pub(crate) fn thread_added(&mut self, summary: ThreadSummary) {
        self.threads
            .insert(summary.thread_id.clone(), summary.clone());
        self.to_inbox(
            &ServerMessage::ThreadUpdated {
                summary: summary.clone(),
            },
            Coalesce::Thread(summary.thread_id),
        );
        self.refresh_inbox();
    }

    /// A listed thread's summary as the store has it now. Nothing happens
    /// for a thread that isn't listed (archived meanwhile) or didn't change.
    /// Returns the notification to post, if the change calls for one.
    pub(crate) fn thread_changed(
        &mut self,
        summary: ThreadSummary,
        now_ms: u64,
    ) -> Option<Notification> {
        let id = summary.thread_id.clone();
        let previous = self.threads.get(&id)?;
        if *previous == summary {
            return None;
        }
        let reason = match &summary.state {
            ThreadState::NeedsApproval => Some(NotifyReason::Approval),
            ThreadState::NeedsInput => Some(NotifyReason::Question),
            ThreadState::Failed { .. } => Some(NotifyReason::Failed),
            _ => None,
        };
        let focused = self.focus.values().any(|focused| *focused == id);
        let notify = reason.filter(|_| {
            attention::should_notify(
                &previous.state,
                &summary.state,
                focused,
                self.notified.get(&id).copied(),
                now_ms,
            )
        });
        self.threads.insert(id.clone(), summary.clone());
        self.to_inbox(
            &ServerMessage::ThreadUpdated {
                summary: summary.clone(),
            },
            Coalesce::Thread(id.clone()),
        );
        self.refresh_inbox();
        notify.map(|reason| {
            self.notified.insert(id.clone(), now_ms);
            Notification {
                thread_id: id,
                title: summary.title,
                reason,
            }
        })
    }

    /// A thread was archived: it leaves the list for everyone.
    pub(crate) fn thread_removed(&mut self, thread: &ThreadId) {
        if self.threads.remove(thread).is_none() {
            return;
        }
        self.notified.remove(thread);
        self.focus.retain(|_, focused| focused != thread);
        self.to_inbox(
            &ServerMessage::ThreadRemoved {
                thread_id: thread.clone(),
            },
            Coalesce::Thread(thread.clone()),
        );
        self.refresh_inbox();
    }

    /// Since when the store is read-only, for the inbox's storage-full item.
    pub(crate) fn set_storage_full(&mut self, since_ms: Option<u64>) {
        if self.storage_full_since.is_some() == since_ms.is_some() {
            return;
        }
        self.storage_full_since = since_ms;
        self.refresh_inbox();
    }

    pub(crate) fn set_focus(&mut self, conn: ConnId, thread: Option<ThreadId>) {
        match thread {
            Some(thread) => {
                self.focus.insert(conn, thread);
            }
            None => {
                self.focus.remove(&conn);
            }
        }
    }

    /// A subscription has everything stored and is pushed to from now on.
    /// Called with the publish lock held, so no event falls between what it
    /// read from the store and what is pushed.
    pub(crate) fn go_live(&mut self, conn: &Arc<Conn>, sub: SubId, thread: &ThreadId) -> bool {
        if !conn.go_live(sub) {
            return false;
        }
        let subs = self.live.entry(thread.clone()).or_default();
        // A connection has one subscription per thread: what it had before
        // is over, and would otherwise stay listed until the next event.
        subs.retain(|(other, _)| other.id != conn.id);
        subs.push((conn.clone(), sub));
        true
    }

    /// Sends newly stored events to the thread's live subscribers.
    pub(crate) fn publish(&mut self, thread: &ThreadId, events: &[StoredEvent]) {
        let Some(subs) = self.live.get_mut(thread) else {
            return;
        };
        for event in events {
            if subs.is_empty() {
                break;
            }
            // Encoded once, whatever the number of subscribers.
            let Some(frame) = encode_event(event) else {
                continue;
            };
            subs.retain(|(conn, sub)| conn.push_live(*sub, event.id, &frame) == Pushed::Queued);
        }
        if subs.is_empty() {
            self.live.remove(thread);
        }
    }
}

/// `ServerMessage::Event` for a stored event, without cloning it.
pub(crate) fn encode_event(event: &StoredEvent) -> Option<codec::Frame> {
    use atrium_protocol::wire::{FrameKind, MAX_FRAME_LEN};
    #[derive(serde::Serialize)]
    struct Event<'a> {
        r#type: &'static str,
        event: &'a StoredEvent,
    }
    let mut frame = vec![0, 0, 0, 0, FrameKind::Control as u8];
    serde_json::to_writer(
        &mut frame,
        &Event {
            r#type: "event",
            event,
        },
    )
    .ok()?;
    let len = frame.len() - 4;
    if len > MAX_FRAME_LEN as usize {
        return None;
    }
    frame[..4].copy_from_slice(&(len as u32).to_be_bytes());
    Some(frame.into())
}

/// The value of a `ListInbox` response.
pub(crate) fn inbox_value(items: &[InboxItem]) -> Value {
    serde_json::to_value(items).unwrap_or(Value::Null)
}
