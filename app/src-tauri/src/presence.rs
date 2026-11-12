//! What the tray item shows (PLAN §3.1, "Presence"): how many threads are
//! running and how many things need the user, while the window is closed.
//!
//! The counts come from the same deliveries a window gets: the tray has its
//! own client window (`TRAY_WINDOW`), consumed here in Rust.

use std::collections::HashMap;

use atrium_client::{ConnectionState, Delivery};
use atrium_protocol::thread::{ThreadId, ThreadState, ThreadSummary};

/// The client window the tray reads. Not a webview label: those are `main`
/// and `view-<n>`.
pub const TRAY_WINDOW: &str = "@tray";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Presence {
    pub connected: bool,
    /// Threads that are starting or have a turn running.
    pub running: u32,
    /// Items in the inbox.
    pub needs_you: u32,
}

impl Presence {
    /// One line for the tray's tooltip and its first menu row.
    pub fn label(&self) -> String {
        if !self.connected {
            return "Atrium: not connected".into();
        }
        match (self.running, self.needs_you) {
            (0, 0) => "Atrium: nothing running".into(),
            (running, 0) => format!("Atrium: {running} running"),
            (0, needs) => format!("Atrium: {needs} need you"),
            (running, needs) => format!("Atrium: {running} running, {needs} need you"),
        }
    }
}

#[derive(Debug, Default)]
pub struct Tracker {
    running: HashMap<ThreadId, bool>,
    inbox: usize,
    connected: bool,
}

fn is_running(summary: &ThreadSummary) -> bool {
    matches!(summary.state, ThreadState::Starting | ThreadState::Running)
}

impl Tracker {
    pub fn apply(&mut self, delivery: &Delivery) -> Presence {
        match delivery {
            Delivery::Connection { state } => {
                self.connected = matches!(state, ConnectionState::Connected { .. });
            }
            Delivery::Inbox { threads, inbox } => {
                self.running = threads
                    .iter()
                    .map(|summary| (summary.thread_id.clone(), is_running(summary)))
                    .collect();
                self.inbox = inbox.len();
            }
            Delivery::ThreadUpdated { summary } => {
                self.running
                    .insert(summary.thread_id.clone(), is_running(summary));
            }
            Delivery::ThreadRemoved { thread_id } => {
                self.running.remove(thread_id);
            }
            Delivery::InboxUpdated { items } => self.inbox = items.len(),
            Delivery::Snapshot { .. } | Delivery::Event { .. } | Delivery::ThreadFailed { .. } => {}
        }
        self.presence()
    }

    pub fn presence(&self) -> Presence {
        let count = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
        Presence {
            connected: self.connected,
            running: count(self.running.values().filter(|running| **running).count()),
            needs_you: count(self.inbox),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn delivery(value: serde_json::Value) -> Delivery {
        serde_json::from_value(value).expect("a delivery")
    }

    fn summary(id: &str, state: serde_json::Value) -> serde_json::Value {
        json!({
            "thread_id": id, "title": id, "provider": "claude", "model": null,
            "location": {"type": "home"}, "state": state, "enforcement": null,
            "incarnation": 1, "native_session_id": null, "last_event": null,
            "unread": false, "pending_approvals": 0, "pending_questions": 0,
            "attention_since_ms": null, "attention_detail": null,
            "created_at_ms": 1, "updated_at_ms": 1
        })
    }

    fn item(id: &str) -> serde_json::Value {
        json!({"thread_id": id, "reason": "approval", "since_ms": 1, "title": id, "detail": null})
    }

    #[test]
    fn counts_follow_the_deliveries() {
        let mut tracker = Tracker::default();
        assert_eq!(tracker.presence().label(), "Atrium: not connected");
        tracker.apply(&delivery(json!({"type": "connection", "state": {"type": "connected", "daemon_version": "1", "daemon_pid": 1}})));
        assert_eq!(tracker.presence().label(), "Atrium: nothing running");
        let presence = tracker.apply(&delivery(json!({
            "type": "inbox",
            "threads": [summary("a", json!({"type": "running"})), summary("b", json!({"type": "starting"})), summary("c", json!({"type": "idle"}))],
            "inbox": [item("c")]
        })));
        assert_eq!(
            presence,
            Presence {
                connected: true,
                running: 2,
                needs_you: 1
            }
        );
        assert_eq!(presence.label(), "Atrium: 2 running, 1 need you");
        let presence = tracker.apply(&delivery(json!({"type": "thread_updated", "summary": summary("a", json!({"type": "needs_approval"}))})));
        assert_eq!(presence.running, 1);
        let presence = tracker.apply(&delivery(
            json!({"type": "inbox_updated", "items": [item("a"), item("c")]}),
        ));
        assert_eq!(presence.needs_you, 2);
        let presence = tracker.apply(&delivery(
            json!({"type": "thread_removed", "thread_id": "b"}),
        ));
        assert_eq!(
            presence,
            Presence {
                connected: true,
                running: 0,
                needs_you: 2
            }
        );
        assert_eq!(presence.label(), "Atrium: 2 need you");
    }

    #[test]
    fn a_dropped_connection_shows_as_not_connected_and_keeps_the_counts_for_the_return() {
        let mut tracker = Tracker::default();
        tracker.apply(&delivery(json!({"type": "connection", "state": {"type": "connected", "daemon_version": "1", "daemon_pid": 1}})));
        tracker.apply(&delivery(json!({"type": "inbox", "threads": [summary("a", json!({"type": "running"}))], "inbox": []})));
        let presence = tracker.apply(&delivery(
            json!({"type": "connection", "state": {"type": "connecting", "attempt": 1}}),
        ));
        assert!(!presence.connected);
        assert_eq!(presence.label(), "Atrium: not connected");
        let presence = tracker.apply(&delivery(json!({"type": "connection", "state": {"type": "connected", "daemon_version": "1", "daemon_pid": 1}})));
        assert_eq!(presence.running, 1);
    }
}
