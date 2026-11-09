//! History export and message reconstruction (contract §13).
//! **Owned by M1-11.** Public items named in `lib.rs` are contract.

use std::io::Write;

use atrium_agents::fixture::Sanitizer;
use atrium_protocol::agent::MessageBody;
use atrium_protocol::wire::{ExportFormat, StoredEvent};

#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    /// The rebuilt text doesn't match `chars`/`sha256`: a delta was lost or
    /// reordered. Never patched up.
    #[error("message {message_id} doesn't match its completion")]
    Integrity { message_id: String },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Rebuilds a message from its deltas, checking the completion.
pub fn reconstruct_message(
    message_id: &str,
    deltas: &[&str],
    body: &MessageBody,
) -> Result<String, HistoryError> {
    let _ = (message_id, deltas, body);
    unimplemented!("M1-11")
}

/// Writes one thread's events as Markdown or JSONL, redacted with
/// `sanitizer` on decoded, reconstructed content. Returns bytes written.
pub fn export_thread(
    events: &mut dyn Iterator<Item = StoredEvent>,
    format: ExportFormat,
    sanitizer: &Sanitizer,
    out: &mut dyn Write,
) -> Result<u64, HistoryError> {
    let _ = (events, format, sanitizer, out);
    unimplemented!("M1-11")
}
