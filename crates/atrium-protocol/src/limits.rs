//! Every byte, count and time limit in M1, in one place (contract §6, G4).
//!
//! A limit that a peer can exceed has a defined behaviour, written next to it.
//! Nothing in Atrium is bounded only by "the channel is bounded": channels
//! bound counts, these bound bytes.

use std::time::Duration;

// ---- Agent events (adapters) ----

/// A completion-only message up to this size is sent `Inline`; a longer one is
/// split into deltas first.
pub const MAX_INLINE_MESSAGE_BYTES: usize = 64 * 1024;
/// One provider record (stdout line, JSON-RPC message, hook payload). Larger
/// ends the session with a fatal `Error`.
pub const MAX_PROVIDER_RECORD_BYTES: usize = 8 * 1024 * 1024;
/// Partial tool arguments buffered until they parse. Larger: the call is
/// finished `failed` with an error, and the gate is never asked to allow it.
pub const MAX_PARTIAL_ARGS_BYTES: usize = 4 * 1024 * 1024;
/// Total output kept per tool call; the rest is dropped and counted.
pub const MAX_TOOL_OUTPUT_BYTES: usize = 1024 * 1024;
/// `Error::details`.
pub const MAX_DIAGNOSTIC_LINES: usize = 50;
pub const MAX_DIAGNOSTIC_LINE_BYTES: usize = 2 * 1024;
/// Pending approvals + questions per session. More: the adapter denies the
/// new request (`Deny { reason: "too many pending requests" }`).
pub const MAX_PENDING_REQUESTS: usize = 64;
/// Closing events an adapter keeps for a receiver that isn't reading, so
/// `SessionEnded` is never lost (contract §10).
pub const TERMINAL_TAIL_EVENTS: usize = 4096;

// ---- Store ----

/// One stored event's JSON payload. Tool output is already capped, so this is
/// only reached by a bug; the event is replaced by a fatal `Error`.
pub const MAX_EVENT_BYTES: usize = 2 * 1024 * 1024;
/// Below this much free space on the data volume, the store goes read-only.
pub const MIN_FREE_DISK_BYTES: u64 = 512 * 1024 * 1024;
/// SQLite page cache per connection (`PRAGMA cache_size = -<KiB>`).
pub const SQLITE_CACHE_KIB: u32 = 2 * 1024;
/// Read-only connections in the store's pool.
pub const STORE_READERS: usize = 2;
/// Daily snapshots kept.
pub const SNAPSHOTS_KEPT: usize = 7;

// ---- Wire ----

/// Client → daemon frames. Larger: the daemon closes the connection.
pub const MAX_CLIENT_FRAME_LEN: u32 = 1024 * 1024;
/// Concurrent connections. More: `Incompatible { message: "too many clients" }`.
pub const MAX_CONNECTIONS: usize = 32;
pub const MAX_SUBSCRIPTIONS_PER_CONNECTION: usize = 256;
/// Bytes queued for one connection's event lane. Past it, the subscription
/// that would overflow is dropped with `SubscriptionLagged`.
pub const MAX_CONNECTION_QUEUE_BYTES: usize = 32 * 1024 * 1024;
/// Events per replay page read from the store.
pub const REPLAY_PAGE_EVENTS: usize = 512;
/// A connection that hasn't sent `Hello` by then is closed.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

// ---- App ----

/// Per window, bytes queued for delivery. Past it the window is marked
/// `needs_resync` and its queue dropped.
pub const MAX_WINDOW_QUEUE_BYTES: usize = 16 * 1024 * 1024;
/// `data:` images rendered inline in agent content.
pub const MAX_INLINE_IMAGE_BYTES: usize = 1024 * 1024;

// ---- Attention ----

/// At most one notification per thread per interval.
pub const NOTIFY_MIN_INTERVAL: Duration = Duration::from_secs(30);
