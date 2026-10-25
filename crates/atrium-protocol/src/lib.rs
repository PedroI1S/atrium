//! Atrium contract types (PLAN §3.2, §3.1).
//!
//! **Contract, frozen for M0 (revision 2, D-014).** Workers implement against these types and
//! never edit them; changes go through `docs/agents/change-requests.md`.
//!
//! - [`agent`]: the normalized event stream every agent adapter produces,
//!   and the commands a session accepts.
//! - [`wire`]: the daemon socket framing, handshake and replay messages.

pub mod agent;
pub mod wire;

/// Bumped on any breaking change to [`agent`] or [`wire`].
pub const PROTOCOL_VERSION: u32 = 0;
