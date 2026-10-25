//! Unix socket spike. See [`Client`] for the small consumer API.
pub mod client;
pub mod codec;
pub mod daemon;
pub mod store;
pub use atrium_protocol::{agent, wire};
pub use client::{
    Client, ClientHandle, CommandError, ConnectionState, EventStream, UnknownOutcome,
};
use std::{
    io,
    time::{SystemTime, UNIX_EPOCH},
};
pub const PROTOCOL_VERSION: u32 = 1;
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub(crate) fn error(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}
