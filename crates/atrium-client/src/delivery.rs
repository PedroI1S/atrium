//! What the client hands to windows, and to callers of commands.

use std::sync::Arc;

use atrium_protocol::thread::{InboxItem, ThreadId, ThreadSummary};
use atrium_protocol::wire::{ErrorCode, EventId, StoredEvent};
use serde::{Deserialize, Serialize};

/// Where the connection to the daemon stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConnectionState {
    /// Not connected: starting up, or reconnecting after the connection
    /// dropped. `attempt` counts from 1.
    Connecting { attempt: u32 },
    Connected {
        daemon_version: String,
        daemon_pid: u32,
    },
    /// The daemon refused this client: another protocol version (the app was
    /// upgraded and the daemon wasn't, contract §10), an unverified app, or
    /// too many clients. Not tried again until [`crate::Client::reconnect`].
    Incompatible {
        protocol_version: u32,
        message: String,
        daemon_version: String,
        daemon_pid: u32,
        running_turns: u32,
    },
    /// The socket isn't safe to use, or the daemon broke the protocol. Not
    /// tried again until [`crate::Client::reconnect`].
    Failed { message: String },
    /// The client was shut down.
    Closed,
}

/// One message to a window. The JSON of `Event`, `ThreadUpdated` and
/// `InboxUpdated` is the daemon's own `ServerMessage`, passed through
/// unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Delivery {
    /// The connection's state: the first delivery to a window, and one for
    /// each change.
    Connection {
        state: ConnectionState,
    },
    /// Every thread and the whole inbox, replacing what the window has. Sent
    /// when the window asks for the inbox, after a reconnect and after a
    /// resync.
    Inbox {
        threads: Vec<ThreadSummary>,
        inbox: Vec<InboxItem>,
    },
    ThreadUpdated {
        summary: ThreadSummary,
    },
    /// The whole ranked inbox, replacing the previous one.
    InboxUpdated {
        items: Vec<InboxItem>,
    },
    /// The thread was archived from this app.
    ThreadRemoved {
        thread_id: ThreadId,
    },
    /// A view of a thread, replacing what the window has for it: the newest
    /// part of its history, folded (a message's deltas are one delta, a tool
    /// call's updates one update), starting where no item is open. `Event`s
    /// after `last` follow.
    Snapshot {
        thread_id: ThreadId,
        events: Vec<StoredEvent>,
        /// Older history can be paged in ([`crate::WindowHandle::page_before`]).
        has_older: bool,
        /// History older than what the client holds exists, but only in the
        /// daemon.
        truncated: bool,
        /// The last event the snapshot includes; `None` for an empty thread.
        last: Option<EventId>,
    },
    /// A live event of a thread the window views.
    Event {
        event: StoredEvent,
    },
    /// The thread can't be viewed (it doesn't exist, for one).
    ThreadFailed {
        thread_id: ThreadId,
        code: ErrorCode,
        message: String,
    },
}

/// A [`Delivery`] as a window receives it: its JSON, and the number to
/// acknowledge it with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    /// Per window, from 1, rising by one.
    pub seq: u64,
    /// A [`Delivery`] as JSON.
    pub json: Arc<str>,
}

impl Outgoing {
    /// The delivery, parsed (for consumers in Rust; a webview gets the JSON).
    pub fn delivery(&self) -> serde_json::Result<Delivery> {
        serde_json::from_str(&self.json)
    }
}

/// Older history of a thread, for a view that scrolled up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(specta::Type))]
pub struct Page {
    /// Folded like a snapshot's, oldest first, starting where no item is open.
    pub events: Vec<StoredEvent>,
    pub has_older: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandError {
    /// Nothing was sent: there is no connection right now.
    #[error("not connected to the daemon")]
    NotConnected,
    /// Nothing was sent: the command is over `limits::MAX_CLIENT_FRAME_LEN`.
    #[error("the command is too large ({bytes} bytes)")]
    TooLarge { bytes: usize },
    /// The connection dropped before the answer came. The command may or may
    /// not have run; it is never sent again by the client. Read the state
    /// again before deciding to repeat it.
    #[error("the connection dropped before the daemon answered")]
    UnknownOutcome,
    /// The daemon's answer.
    #[error("{message}")]
    Rejected { code: ErrorCode, message: String },
}
