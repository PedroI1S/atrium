//! History export and message reconstruction (contract §13).
//! **Owned by M1-11.** Public items named in `lib.rs` are contract.

use std::io::Write;

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

/// Redaction for exports of real sessions (secrets aren't known canaries).
/// Works on decoded content: reconstructed messages, and JSON strings nested
/// inside tool input and output, after `\uXXXX` and escape decoding.
/// - Token patterns: Anthropic (`sk-ant-…`), OpenAI (`sk-…`), GitHub
///   (`gh[pousr]_…`, `github_pat_…`), AWS access keys (`AKIA…`, `ASIA…`), Google
///   API keys (`AIza…`), Slack (`xox[abpr]-…`), PEM private-key blocks,
///   `Authorization: Bearer …`, and `KEY=value` lines whose key ends in
///   `TOKEN`, `SECRET`, `PASSWORD` or `API_KEY`.
/// - The home directory becomes `~`. The user name is kept (it's their own
///   history).
/// - Extra literal secrets can be added (tests use canaries).
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    _private: (),
}

impl Redactor {
    pub fn new(home: &str, extra_secrets: &[&str]) -> Self {
        let _ = (home, extra_secrets);
        unimplemented!("M1-11")
    }

    pub fn text(&self, text: &str) -> String {
        let _ = text;
        unimplemented!("M1-11")
    }
}

/// Writes one thread's events as Markdown or JSONL, redacted with `redactor`.
/// Returns bytes written. The daemon opens `out` with mode 0600 and
/// `O_NOFOLLOW`.
pub fn export_thread(
    events: &mut dyn Iterator<Item = StoredEvent>,
    format: ExportFormat,
    redactor: &Redactor,
    out: &mut dyn Write,
) -> Result<u64, HistoryError> {
    let _ = (events, format, redactor, out);
    unimplemented!("M1-11")
}
