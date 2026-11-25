//! Policy engine, command parsing and the approval ledger (contract §5, §9).
//! **Owned by M1-03.** Public items named in `lib.rs` are contract.

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use atrium_agents::ToolCheck;
use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::{PolicyContext, Preset};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{ApprovalLedger, PolicyEngine, RequestStore};

mod deny;
mod engine;
mod gate;
mod ledger;
mod limits;
mod model;
mod tools;
mod v4;

// Parity constants retained strictly for frozen P0 tests in `limits.rs` and
// `tools.rs` after deleting `shell.rs` and `words.rs` (see CR in handoff).
#[cfg(test)]
mod shell {
    pub(super) const MAX_UNITS: usize = 512;
    pub(super) const MAX_DEPTH: u8 = 6;
    pub(super) const RESERVED: &[&str] = &[
        "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
        "function", "select", "{", "}", "!", "[[", "coproc",
    ];
    pub(super) const SHELLS: &[&str] = &[
        "sh", "bash", "zsh", "dash", "ksh", "fish", "ash", "csh", "tcsh", "mksh", "yash", "rbash",
        "posh", "elvish", "xonsh",
    ];
    pub(super) const COMMAND_RUNNERS: &[&str] = &[
        "ssh",
        "mosh",
        "autossh",
        "nsenter",
        "unshare",
        "docker",
        "podman",
        "nerdctl",
        "kubectl",
        "oc",
        "lxc",
        "incus",
        "machinectl",
        "systemd-run",
        "systemd-nspawn",
        "firejail",
        "bwrap",
        "flatpak-spawn",
        "toolbox",
        "distrobox",
        "distrobox-enter",
        "multipass",
        "vagrant",
        "limactl",
        "colima",
        "su",
        "runuser",
        "setpriv",
        "parallel",
    ];
}

#[cfg(test)]
mod words {
    pub(super) const MAX_USES: usize = 4096;
    pub(super) const MAX_PATHS: usize = 16384;
    pub(super) const WRITERS: &[&str] = &[
        "rm",
        "mv",
        "cp",
        "tee",
        "touch",
        "chmod",
        "chown",
        "chgrp",
        "ln",
        "truncate",
        "install",
        "mkdir",
        "rmdir",
        "rsync",
        "unlink",
        "shred",
        "patch",
        "ed",
        "ex",
        "wipefs",
        "blkdiscard",
    ];
    pub(super) const READERS: &[&str] = &[
        "cat",
        "head",
        "tail",
        "less",
        "more",
        "wc",
        "ls",
        "stat",
        "file",
        "grep",
        "egrep",
        "fgrep",
        "rg",
        "ag",
        "ack",
        "diff",
        "cmp",
        "md5",
        "md5sum",
        "shasum",
        "sha1sum",
        "sha256sum",
        "du",
        "df",
        "tree",
        "echo",
        "printf",
        "which",
        "whereis",
        "type",
        "readlink",
        "realpath",
        "basename",
        "dirname",
        "test",
        "[",
        "pwd",
        "sort",
        "uniq",
        "cut",
        "tr",
        "nl",
        "od",
        "xxd",
        "hexdump",
        "strings",
        "bat",
        "jq",
        "column",
        "fold",
        "expand",
        "true",
        "false",
        "id",
        "whoami",
        "uname",
        "date",
        "cd",
        "pushd",
        "popd",
        "find",
        "open",
        "spctl",
        "mdls",
        "mdfind",
        "codesign",
        "awk",
        "gawk",
        "mawk",
        "sed",
        "perl",
    ];
}

pub use gate::{RulesSource, SessionGateConfig, StoreRules, fs_resolve};

/// Validates a rule matcher (and optional `path_scope`) against policy rev 4
/// §7's code-runner pinning and glob syntax rules (contract §7, §11 items 4–5,
/// D-153).
///
/// Calls `v4::rules::validate_rule_matcher` with [`model::FoldRules::REV4`]
/// and returns a plain user-facing explanation on error, suitable for daemon
/// `ErrorCode::Invalid` responses and `ListRules` invalid-rule annotations.
pub fn validate_rule_matcher(
    matcher: &atrium_protocol::policy::RuleMatcher,
    decision: atrium_protocol::policy::RuleDecision,
    path_scope: Option<&str>,
) -> Result<(), String> {
    v4::rules::validate_rule_draft_reason(matcher, decision, path_scope, &model::FoldRules::REV4)
}

/// Canonical bytes of a tool call's input for binding approvals: JSON with
/// object keys sorted, no insignificant whitespace, numbers as serde_json
/// prints them, prefixed by `kind` and `name`. Two calls hash equal iff an
/// approval for one may authorize the other.
pub fn canonical_input(kind: ToolKind, name: &str, input: &Value) -> Vec<u8> {
    fn write(v: &Value, out: &mut String) {
        match v {
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                out.push('{');
                for (i, k) in keys.into_iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&Value::String(k.clone()).to_string());
                    out.push(':');
                    write(&m[k], out);
                }
                out.push('}');
            }
            Value::Array(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write(x, out);
                }
                out.push(']');
            }
            other => out.push_str(&other.to_string()),
        }
    }
    let kind = serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    let mut body = String::new();
    write(input, &mut body);
    // Length-prefixed, so `kind`/`name`/input can't run into one another.
    format!("{}:{kind}\n{}:{name}\n{body}", kind.len(), name.len()).into_bytes()
}

/// Lowercase hex SHA-256 of [`canonical_input`].
pub fn input_sha256(check: &ToolCheck) -> String {
    let digest = Sha256::digest(canonical_input(check.kind, &check.name, &check.input));
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// A shell command line, as rules see it (contract §9). Separators are `;`,
/// `&&`, `||`, `|` and newline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedCommand {
    /// One or more simple commands joined by `;`, `&&`, `||` or `|`, each as
    /// argv. `piped` marks segments that receive another's output.
    Segments(Vec<Segment>),
    /// Substitution, backgrounding, here-docs, subshells, or anything the
    /// parser doesn't handle: rules and Home's read-only list can't allow it.
    Unparseable { reason: String },
}

/// One simple command. Rules and `HOME_READONLY_COMMANDS` match a segment
/// only if it has no `assignments` and every redirect is
/// [`Redirect::harmless`]; the hard-deny list looks through assignments and
/// checks redirect targets as paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Leading `NAME=value` words (`LD_PRELOAD=… ls`, `PAGER=… git log`).
    pub assignments: Vec<(String, String)>,
    pub argv: Vec<String>,
    pub redirects: Vec<Redirect>,
    pub piped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    /// The file descriptor redirected (`2>` is 2; `>` is 1, `<` is 0).
    pub fd: u32,
    pub op: RedirectOp,
    /// A file path, or the fd number for `Dup`.
    pub target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectOp {
    /// `<`
    Read,
    /// `>`, `>|`, `&>`
    Write,
    /// `>>`, `&>>`
    Append,
    /// `<>`
    ReadWrite,
    /// `>&N`, `<&N`, `N>&-`
    Dup,
}

impl Redirect {
    /// fd duplication, or output to `/dev/null`. Anything else that writes
    /// is a write, wherever it points (in Home the cwd is `~`).
    pub fn harmless(&self) -> bool {
        match self.op {
            RedirectOp::Dup => true,
            RedirectOp::Write | RedirectOp::Append => self.target == "/dev/null",
            RedirectOp::Read | RedirectOp::ReadWrite => false,
        }
    }
}

/// Parses a shell line into simple commands. `Unparseable` for anything a
/// rule must not trust: substitution, globs, `$` expansion, brace expansion,
/// backgrounding, here-docs, subshells and compound commands.
pub fn parse_command(line: &str, cwd: &std::path::Path) -> ParsedCommand {
    let _ = cwd;
    if line.len() > limits::MAX_BYTES {
        return ParsedCommand::Unparseable {
            reason: "too large".into(),
        };
    }
    let mut budget = model::ComputeBudget::with_default_limits();
    let lexed = match v4::lex::lex_exact(line, 0, None, &mut budget) {
        Ok(l) => l,
        Err(outcome) => {
            return ParsedCommand::Unparseable {
                reason: format!("{outcome:?}"),
            };
        }
    };
    if let Some(first) = lexed.outcomes.first() {
        return ParsedCommand::Unparseable {
            reason: format!("{:?}", first.outcome),
        };
    }
    let mut segments = Vec::with_capacity(lexed.units.len());
    for u in lexed.units {
        if u.argv.iter().any(model::Word::has_unquoted_glob) {
            return ParsedCommand::Unparseable {
                reason: "glob".into(),
            };
        }
        let mut argv = Vec::with_capacity(u.argv.len());
        for w in &u.argv {
            match std::str::from_utf8(w.literal_bytes()) {
                Ok(s) => argv.push(s.to_owned()),
                Err(_) => {
                    return ParsedCommand::Unparseable {
                        reason: "non-utf8".into(),
                    };
                }
            }
        }
        let mut redirects = Vec::with_capacity(u.redirects.len());
        for r in &u.redirects {
            let (fd, op, target) = match r {
                model::ModelRedirect::Read { target } => {
                    let Ok(s) = std::str::from_utf8(target.literal_bytes()) else {
                        return ParsedCommand::Unparseable {
                            reason: "non-utf8".into(),
                        };
                    };
                    (0u32, RedirectOp::Read, s.to_owned())
                }
                model::ModelRedirect::Write { target } => {
                    let Ok(s) = std::str::from_utf8(target.literal_bytes()) else {
                        return ParsedCommand::Unparseable {
                            reason: "non-utf8".into(),
                        };
                    };
                    (1u32, RedirectOp::Write, s.to_owned())
                }
                model::ModelRedirect::Append { target } => {
                    let Ok(s) = std::str::from_utf8(target.literal_bytes()) else {
                        return ParsedCommand::Unparseable {
                            reason: "non-utf8".into(),
                        };
                    };
                    (1u32, RedirectOp::Append, s.to_owned())
                }
                model::ModelRedirect::ErrWrite { target } => {
                    let Ok(s) = std::str::from_utf8(target.literal_bytes()) else {
                        return ParsedCommand::Unparseable {
                            reason: "non-utf8".into(),
                        };
                    };
                    (2u32, RedirectOp::Write, s.to_owned())
                }
                model::ModelRedirect::ErrAppend { target } => {
                    let Ok(s) = std::str::from_utf8(target.literal_bytes()) else {
                        return ParsedCommand::Unparseable {
                            reason: "non-utf8".into(),
                        };
                    };
                    (2u32, RedirectOp::Append, s.to_owned())
                }
                model::ModelRedirect::Dup { from, to } => {
                    (*from as u32, RedirectOp::Dup, (*to as u8).to_string())
                }
                model::ModelRedirect::Close { fd } => (*fd as u32, RedirectOp::Dup, "-".to_owned()),
            };
            redirects.push(Redirect { fd, op, target });
        }
        segments.push(Segment {
            assignments: Vec::new(),
            argv,
            redirects,
            piped: u.separator_before == model::Separator::Pipe,
        });
    }
    ParsedCommand::Segments(segments)
}

/// The default [`PolicyEngine`]: hard-deny defaults, rules, presets.
#[derive(Debug, Default)]
pub struct DefaultPolicy {
    _private: (),
}

impl DefaultPolicy {
    pub fn new() -> Self {
        Self::default()
    }
}

/// The ledger over a [`crate::RequestStore`].
pub struct Ledger {
    store: Arc<dyn RequestStore>,
    open_lock: tokio::sync::Mutex<()>,
    counter: AtomicU64,
    salt: u64,
}

/// One session's [`atrium_agents::ToolGate`]: evaluates the policy, opens and
/// redeems ledger entries. Built by the daemon for each incarnation with
/// [`SessionGate::new`].
///
/// - `check`: resolves paths (deepest existing ancestor canonicalized, the
///   rest appended; this is the gate's only filesystem access), evaluates
///   the policy, and for `Ask` computes the "always…" matcher and opens the
///   ledger entry. Any internal error → `Deny` (fail closed).
/// - `redeem`: re-resolves the call's paths, always re-checks the hard-deny
///   list (a symlink may have been swapped since the approval), re-evaluates
///   the policy without the rule this approval itself created, refuses on
///   `Deny`/`HardDeny`, then calls the ledger's `redeem`. Anything but
///   success → `Deny`.
pub struct SessionGate {
    config: SessionGateConfig,
    preset: Mutex<Preset>,
    policy: Arc<dyn PolicyEngine>,
    ledger: Arc<dyn ApprovalLedger>,
    rules: Arc<dyn RulesSource>,
    /// The policy context each open ticket was evaluated under.
    opened: Mutex<HashMap<String, PolicyContext>>,
}
