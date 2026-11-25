//! Retired `dev` hard-deny module.
//!
//! Runtime evaluation is now handled by policy rev 4 (`policy/v4/`). Only the
//! `#[cfg(test)]` parity constants required by the frozen P0 unit tests in
//! `limits.rs` and `tools.rs` remain here.

pub(super) const MAX_BYTES: usize = 128 * 1024;
pub(super) const DOWNLOADERS: &[&str] = &["curl", "wget", "fetch"];
pub(super) const PRIV: &[&str] = &["sudo", "sudoedit", "su", "doas", "pkexec", "run0"];
