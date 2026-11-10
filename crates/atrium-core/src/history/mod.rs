//! History export and message reconstruction (contract §13).
//! **Owned by M1-11.** Public items named in `lib.rs` are contract.
//!
//! - [`reconstruct_message`] rebuilds a message from its deltas and checks it
//!   against its completion.
//! - [`Redactor`] removes secrets from exported text (`redact.rs`).
//! - [`export_thread`] writes one thread as Markdown or JSONL (`export.rs`).
//! - [`create_export_file`] creates the private file an export goes into
//!   (`file.rs`).

use atrium_protocol::agent::MessageBody;
use sha2::{Digest, Sha256};

mod export;
mod file;
mod redact;

pub use export::export_thread;
pub use file::create_export_file;
pub use redact::Redactor;

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
///
/// A `Streamed` body must match the deltas in number, in length (Unicode
/// scalar values) and in SHA-256. An `Inline` body is the whole text, so it
/// must come with no deltas. Anything else is [`HistoryError::Integrity`].
pub fn reconstruct_message(
    message_id: &str,
    deltas: &[&str],
    body: &MessageBody,
) -> Result<String, HistoryError> {
    verify_message(message_id, deltas.concat(), deltas.len() as u64, body)
}

/// [`reconstruct_message`] for deltas that were already joined into `text`
/// (the export joins them as they arrive, so a message of millions of deltas
/// costs one string).
fn verify_message(
    message_id: &str,
    text: String,
    deltas: u64,
    body: &MessageBody,
) -> Result<String, HistoryError> {
    let integrity = || HistoryError::Integrity {
        message_id: message_id.to_owned(),
    };
    match body {
        MessageBody::Inline { text: whole } => {
            if deltas != 0 {
                return Err(integrity());
            }
            Ok(whole.clone())
        }
        MessageBody::Streamed {
            chars,
            deltas: count,
            sha256,
        } => {
            if deltas != *count
                || text.chars().count() as u64 != *chars
                || hex(&Sha256::digest(text.as_bytes())) != *sha256
            {
                return Err(integrity());
            }
            Ok(text)
        }
    }
}

/// Lowercase hex, as `MessageBody::Streamed::sha256` is written.
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 0xf)]));
    }
    out
}
