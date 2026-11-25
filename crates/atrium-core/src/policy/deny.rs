//! Retired `dev` hard-deny module.
//!
//! Runtime evaluation is now handled by policy rev 4 (`policy/v4/`). Only the
//! static CR-12 store lists (parsed by `app/src/ui/security.test.ts` to verify
//! `Settings -> Security` parity) and `#[cfg(test)]` parity constants required
//! by the frozen P0 unit tests in `limits.rs` and `tools.rs` remain here.

#[allow(dead_code)]
const ASK_FILES: &[&str] = &[
    ".cargo/credentials.toml",
    ".cargo/credentials",
    ".pypirc",
    ".pgpass",
    ".vault-token",
    ".azure/accessTokens.json",
    ".config/git/credentials",
    ".env",
    // shell history files
    ".bash_history",
    ".zsh_history",
    ".zhistory",
    ".sh_history",
    ".ksh_history",
    ".history",
    ".local/share/fish/fish_history",
    ".config/fish/fish_history",
];

#[allow(dead_code)]
const ASK_TREES: &[&str] = &[".aws/sso/cache", ".zsh_sessions", ".bash_sessions"];

#[allow(dead_code)]
const ASK_NAME_PREFIXES: &[(&str, &str)] = &[(".azure", "msal_token_cache.")];

#[cfg(test)]
pub(super) const MAX_BYTES: usize = 128 * 1024;
#[cfg(test)]
pub(super) const DOWNLOADERS: &[&str] = &["curl", "wget", "fetch"];
#[cfg(test)]
pub(super) const PRIV: &[&str] = &["sudo", "sudoedit", "su", "doas", "pkexec", "run0"];
