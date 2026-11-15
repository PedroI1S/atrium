#![doc = "Codex (`codex app-server --listen stdio://`) adapter."]

pub mod adapter;
pub mod normalizer;
pub mod recording;
pub mod settings;
pub mod tools;

pub use adapter::{CodexAdapter, CodexOptions, TESTED_VERSIONS, process_args, thread_params};
pub use normalizer::CodexNormalizer;
pub use tools::{
    extract_command_line, file_change_kind, file_change_paths, tool_check, tool_kind, tool_subject,
    tool_title,
};
