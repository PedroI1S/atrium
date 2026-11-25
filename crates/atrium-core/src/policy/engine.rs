//! `DefaultPolicy`: pure rev 4 policy engine (`PolicyEngine` implementation).
//!
//! Never touches the filesystem (contract §5): `SessionGate` evaluates with
//! `v4::paths::FsResolver` and `build_protected_set`, while `DefaultPolicy`
//! evaluates lexically over a pure [`VirtualResolver`].

use std::path::Path;

use atrium_protocol::policy::Rule;

use super::DefaultPolicy;
use super::model::{
    ComputeBudget, DirEntry, FoldRules, FsCaps, PathMetadata, ProtectedSet, ResolveError,
};
use super::v4;
use crate::{PolicyEngine, PolicyInput, PolicyVerdict};

/// Pure, non-filesystem [`v4::paths::PathResolver`] used by
/// [`DefaultPolicy::evaluate`] (contract §5).
struct VirtualResolver;

impl v4::paths::PathResolver for VirtualResolver {
    fn stat(
        &self,
        _path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<PathMetadata, ResolveError> {
        caps.charge_stat()?;
        budget.charge(1)?;
        Err(ResolveError::NotFound)
    }

    fn lstat(
        &self,
        _path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<PathMetadata, ResolveError> {
        caps.charge_stat()?;
        budget.charge(1)?;
        Err(ResolveError::NotFound)
    }

    fn read_link(
        &self,
        _path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<Vec<u8>, ResolveError> {
        caps.charge_link_read()?;
        budget.charge(1)?;
        Err(ResolveError::NotFound)
    }

    fn canonicalize(
        &self,
        _path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<Vec<u8>, ResolveError> {
        caps.charge_canonicalize()?;
        budget.charge(1)?;
        Err(ResolveError::NotFound)
    }

    fn read_dir(
        &self,
        _path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<Vec<DirEntry>, ResolveError> {
        caps.charge_dir_open()?;
        budget.charge(1)?;
        Err(ResolveError::NotFound)
    }
}

#[cfg(unix)]
fn engine_path_bytes(p: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    p.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn engine_path_bytes(p: &Path) -> Vec<u8> {
    p.as_os_str().as_encoded_bytes().to_vec()
}

impl PolicyEngine for DefaultPolicy {
    fn evaluate(&self, input: &PolicyInput<'_>, rules: &[Rule]) -> PolicyVerdict {
        let protected = ProtectedSet {
            home: engine_path_bytes(input.home),
            cwd: engine_path_bytes(input.cwd),
            atrium_data_paths: input
                .atrium_data_paths
                .iter()
                .map(|p| engine_path_bytes(p))
                .collect(),
            atrium_protected_paths: input
                .atrium_protected_paths
                .iter()
                .map(|p| engine_path_bytes(p))
                .collect(),
            hard_deny_identities: Vec::new(),
            write_only_hard_deny_identities: Vec::new(),
            identity_gaps: Vec::new(),
            store_identities: Vec::new(),
            ssh_linked_dirs: Vec::new(),
            folds: FoldRules::REV4,
        };
        v4::evaluate::evaluate_call(input, rules, &protected, &VirtualResolver).verdict
    }
}

// Parity constants retained only for frozen P0 tests in `policy/tools.rs`
// (see CR in handoff `M1-41-rev4-p5-integration.md`).
#[cfg(test)]
pub(crate) const RG_BOOL_S: &str = "iSsnNHIlcovwxuaUFPq0pb";
#[cfg(test)]
pub(crate) const RG_VAL_S: &str = "egtTmABCdjMEr";
#[cfg(test)]
pub(crate) const RG_BOOL_L: &[&str] = &[
    "hidden",
    "no-ignore",
    "no-ignore-vcs",
    "no-ignore-dot",
    "no-ignore-parent",
    "no-ignore-global",
    "no-ignore-files",
    "no-messages",
    "no-heading",
    "heading",
    "line-number",
    "no-line-number",
    "with-filename",
    "no-filename",
    "ignore-case",
    "smart-case",
    "case-sensitive",
    "count",
    "count-matches",
    "files-with-matches",
    "files-without-match",
    "only-matching",
    "invert-match",
    "word-regexp",
    "line-regexp",
    "fixed-strings",
    "multiline",
    "multiline-dotall",
    "text",
    "binary",
    "unrestricted",
    "files",
    "type-list",
    "json",
    "column",
    "no-column",
    "vimgrep",
    "trim",
    "pretty",
    "quiet",
    "stats",
    "null",
    "no-config",
    "version",
    "help",
    "pcre2",
    "no-pcre2",
    "crlf",
    "include-zero",
    "byte-offset",
    "glob-case-insensitive",
    "no-glob-case-insensitive",
];
#[cfg(test)]
pub(crate) const RG_VAL_L: &[&str] = &[
    "regexp",
    "glob",
    "iglob",
    "type",
    "type-not",
    "max-count",
    "after-context",
    "before-context",
    "context",
    "max-depth",
    "maxdepth",
    "threads",
    "max-columns",
    "encoding",
    "replace",
    "colors",
    "color",
    "sort",
    "sortr",
    "max-filesize",
    "path-separator",
    "engine",
    "context-separator",
    "type-add",
    "type-clear",
    "field-context-separator",
    "field-match-separator",
    "dfa-size-limit",
    "regex-size-limit",
];
#[cfg(test)]
pub(crate) const GREP_BOOL_S: &str = "ivnclLoqswxhHEFGPaIrRzZbTUy0123456789";
#[cfg(test)]
pub(crate) const GREP_VAL_S: &str = "emABCdD";
#[cfg(test)]
pub(crate) const GREP_BOOL_L: &[&str] = &[
    "ignore-case",
    "no-ignore-case",
    "invert-match",
    "line-number",
    "count",
    "files-with-matches",
    "files-without-match",
    "only-matching",
    "quiet",
    "silent",
    "no-messages",
    "word-regexp",
    "line-regexp",
    "no-filename",
    "with-filename",
    "extended-regexp",
    "fixed-strings",
    "basic-regexp",
    "perl-regexp",
    "text",
    "null-data",
    "null",
    "byte-offset",
    "initial-tab",
    "recursive",
    "dereference-recursive",
    "no-group-separator",
];
#[cfg(test)]
pub(crate) const GREP_VAL_L: &[&str] = &[
    "regexp",
    "max-count",
    "after-context",
    "before-context",
    "context",
    "directories",
    "devices",
    "include",
    "exclude",
    "exclude-dir",
    "label",
    "color",
    "colour",
    "binary-files",
    "group-separator",
];
#[cfg(test)]
pub(super) const GIT_CRED_SUFFIXES: &[&str] = &[
    ".aws/credentials",
    ".docker/config.json",
    ".kube/config",
    ".config/gh/hosts.yml",
    ".claude/.credentials.json",
    ".codex/auth.json",
    ".cargo/credentials",
    ".cargo/credentials.toml",
    ".config/git/credentials",
    ".azure/accesstokens.json",
];
#[cfg(test)]
pub(super) const GIT_CRED_NAMES: &[&str] = &[
    ".netrc",
    ".git-credentials",
    ".npmrc",
    ".claude.json",
    ".pypirc",
    ".pgpass",
    ".vault-token",
    ".env",
    ".bash_history",
    ".zsh_history",
    ".zhistory",
    ".sh_history",
    ".ksh_history",
    ".history",
    "fish_history",
    "accesstokens.json",
];
#[cfg(test)]
pub(super) const GIT_CRED_DIRS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".config/gcloud",
    ".gemini",
    ".aws/sso/cache",
    ".zsh_sessions",
    ".bash_sessions",
    "library/keychains",
];
