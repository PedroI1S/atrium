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
//! - `len` covers kind + payload and never exceeds [`MAX_FRAME_LEN`]; a peer
//!   that receives a larger frame, an unknown kind or invalid JSON closes the
//!   connection.
//!
//! # Socket security
//!
//! The socket lives in a directory owned by the user with mode `0700`, and
//! the socket itself is `0600`. The daemon refuses to start if the directory
//! is a symlink or owned by someone else. It takes the lock file *before*
//! removing a stale socket.
//!
//! # Session
//!
//! 1. Client sends [`ClientMessage::Hello`]. The daemon answers
//!    [`ServerMessage::Welcome`], or [`ServerMessage::Incompatible`] and closes.
//!    Any other message before `Hello` closes the connection.
//! 2. Client sends [`ClientMessage::Subscribe`] with a cursor (see below).
//!    The daemon answers with a [`ServerMessage::Response`]: `Ok` with
//!    `{"replay_until": EventId|null}` (the thread's last event at that
//!    moment), or `Err` for an unknown thread or a cursor beyond the thread's
//!    last event. After `Ok` it sends every stored event after the cursor up
//!    to `replay_until`, then [`ServerMessage::ReplayDone`], then live events.
//!    No event is skipped or duplicated across the replay/live boundary.
//!    Subscribing again to the same thread on the same connection replaces
//!    the earlier subscription and replays from the new cursor.
//! 3. Commands carry a client-chosen `request_id`; the daemon answers each with
//!    exactly one [`ServerMessage::Response`].
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

use crate::agent::{AgentEventEnvelope, Provider, SessionCommand};

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

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ThreadId(pub String);

/// Global, strictly increasing, assigned by the daemon's store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventId(pub u64);

/// An agent event after the daemon has stored it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredEvent {
    pub id: EventId,
    pub thread_id: ThreadId,
    pub envelope: AgentEventEnvelope,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        protocol_version: u32,
        client_name: String,
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
    Command {
        request_id: u64,
        command: DaemonCommand,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonCommand {
    /// Response value: `{"thread_id": "<id>"}`.
    StartThread {
        agent: AgentLaunch,
        cwd: String,
    },
    /// Response value: `[{"thread_id", "running": bool, "last_event": EventId|null}]`.
    ListThreads,
    /// Forward a command to the thread's agent session.
    ThreadCommand {
        thread_id: ThreadId,
        command: SessionCommand,
    },
    StopThread {
        thread_id: ThreadId,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentLaunch {
    /// **M0 placeholder, not frozen for M1.** M1 replaces it with a full
    /// launch config (permission policy, resume, effort), validated by the
    /// daemon's policy engine. M0 daemons answer it with `CommandResult::Err`.
    Native {
        provider: Provider,
        model: Option<String>,
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
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome {
        protocol_version: u32,
        daemon_version: String,
        daemon_pid: u32,
    },
    Incompatible {
        protocol_version: u32,
        message: String,
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CommandResult {
    Ok { value: Value },
    Err { message: String },
}
