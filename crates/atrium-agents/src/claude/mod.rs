//! The Claude Code adapter (M1-07): `claude -p` over stream-json plus the
//! control protocol, folded from the M0-01 spike onto the M1 contract.
//!
//! - [`ClaudeAdapter`] implements [`crate::AgentAdapter`]: `probe` reports the
//!   enforcement a session would get, `start` spawns the CLI as a supervised
//!   process group and routes every tool call through the session's gate.
//! - [`ClaudeNormalizer`] is the pure mapping from [`crate::AdapterInput`] to
//!   events, replayed against `fixtures/agents/claude`.
//! - [`settings`] inspects the configuration sources the CLI would load, for
//!   the descriptor.

mod adapter;
mod normalizer;
pub mod settings;
mod tools;

pub use adapter::{ClaudeAdapter, ClaudeOptions, TESTED_VERSIONS};
pub use normalizer::{ClaudeNormalizer, questions_of};
pub use tools::{tool_check, tool_kind, tool_subject, tool_title};
