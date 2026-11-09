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
mod shell;
mod words;

pub use gate::{RulesSource, SessionGateConfig, StoreRules, fs_resolve};

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
    let lexed = shell::lex(line);
    if !lexed.reasons.is_empty() {
        return ParsedCommand::Unparseable { reason: lexed.reasons.join(", ") };
    }
    // Top-level commands only: payloads are the hard-deny list's business.
    ParsedCommand::Segments(
        lexed.units.into_iter().filter(|u| !u.derived).map(|u| u.seg).collect(),
    )
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
