//! Atrium contract types (PLAN §3.1–§3.4, §3.12).
//!
//! **Contract, M1 revision 3 (draft, `docs/contracts/m1.md`).** Workers
//! implement against these types and never edit them; changes go through
//! `docs/agents/change-requests.md`. M1-12 may add TS-export derives only.
//!
//! - [`agent`]: the normalized event stream every agent adapter produces,
//!   and the commands a session accepts.
//! - [`policy`]: enforcement descriptors, rules, presets, hard-deny defaults.
//! - [`thread`]: thread states, summaries and the inbox.
//! - [`wire`]: the daemon socket framing, roles, handshake, replay, commands.
//! - [`limits`]: every byte, count and time limit.

pub mod agent;
pub mod limits;
pub mod policy;
pub mod thread;
pub mod wire;

/// Bumped on any breaking change to the modules above. App and daemon ship
/// together; a mismatch is `Incompatible`.
pub const PROTOCOL_VERSION: u32 = 1;
