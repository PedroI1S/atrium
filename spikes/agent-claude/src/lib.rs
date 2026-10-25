//! Spike M0-01: Claude Code adapter (stream-json + control protocol).
//! See `docs/spikes/M0-01-claude-adapter.md`.

mod adapter;
mod normalizer;
mod settings;

pub use adapter::{ClaudeAdapter, ClaudeOptions};
pub use normalizer::{ClaudeNormalizer, questions_of, tool_check, tool_kind, tool_title};
pub use settings::{
    Enforcement, ExternalHook, default_locations, detect_external_hooks, settings_files,
};
