//! Spike M0-01: Claude Code adapter (stream-json + control protocol).
//! See `docs/spikes/M0-01-claude-adapter.md`.

mod adapter;
mod normalizer;

pub use adapter::{ClaudeAdapter, ClaudeOptions};
pub use normalizer::{ClaudeNormalizer, questions_of, tool_check, tool_kind, tool_title};
