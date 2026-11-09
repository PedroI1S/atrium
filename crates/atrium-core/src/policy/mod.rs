//! Policy engine, command parsing and the approval ledger (contract §5, §9).
//! **Owned by M1-03.** Public items named in `lib.rs` are contract.

use std::sync::Arc;

use atrium_agents::ToolCheck;
use atrium_protocol::agent::ToolKind;
use serde_json::Value;

use crate::{ApprovalLedger, PolicyEngine, RequestStore};

/// Canonical bytes of a tool call's input for binding approvals: JSON with
/// object keys sorted, no insignificant whitespace, numbers as serde_json
/// prints them, prefixed by `kind` and `name`. Two calls hash equal iff an
/// approval for one may authorize the other.
pub fn canonical_input(kind: ToolKind, name: &str, input: &Value) -> Vec<u8> {
    let _ = (kind, name, input);
    unimplemented!("M1-03")
}

/// Lowercase hex SHA-256 of [`canonical_input`].
pub fn input_sha256(check: &ToolCheck) -> String {
    let _ = check;
    unimplemented!("M1-03")
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

pub fn parse_command(line: &str, cwd: &std::path::Path) -> ParsedCommand {
    let _ = (line, cwd);
    unimplemented!("M1-03")
}

/// The default [`PolicyEngine`]: hard-deny defaults, rules, presets.
#[derive(Debug, Default)]
pub struct DefaultPolicy {
    _private: (),
}

/// The ledger over a [`crate::RequestStore`].
pub struct Ledger {
    _store: Arc<dyn RequestStore>,
}

impl Ledger {
    pub fn new(store: Arc<dyn RequestStore>) -> Self {
        Self { _store: store }
    }
}

/// One session's [`atrium_agents::ToolGate`]: evaluates the policy, opens and
/// redeems ledger entries. Built by the daemon for each incarnation.
///
/// - `check`: resolves paths (deepest existing ancestor canonicalized, the
///   rest appended; this is the gate's only filesystem access), evaluates
///   the policy, and for `Ask` computes the "always…" matcher and opens the
///   ledger entry. Any internal error → `Deny` (fail closed).
///   - Paths resolved: a `Paths` subject's paths; for a command, every
///     non-flag argument, the value of every `--flag=value`, and every
///     redirect target, of every segment, against the segment's cwd. The
///     cwd is `ToolSubject::Command.cwd` (or the thread's), updated by a
///     `cd` to a literal path for later segments; any other directory
///     change (`cd -`, `cd $X`, `pushd`) makes the line `Ask`.
///   - "Always…" matcher: one simple command with no assignments, harmless
///     redirects only and no `$`, globs or braces → `CommandPrefix` of its
///     argv; a file tool with exactly one path → `PathGlob` of that resolved
///     path with glob metacharacters escaped; anything else → none (not
///     rememberable).
/// - `redeem`: rebuilds the binding from the given check and re-resolves its
///   paths; always re-checks the hard-deny list (a symlink may have been
///   swapped since the approval); if the rules revision or preset changed,
///   re-evaluates the whole policy (excluding the rule this approval itself
///   created); refuses on `Deny`/`HardDeny`; then calls the ledger's
///   `redeem`. Anything but success → `Deny`.
pub struct SessionGate {
    _policy: Arc<dyn PolicyEngine>,
    _ledger: Arc<dyn ApprovalLedger>,
}
