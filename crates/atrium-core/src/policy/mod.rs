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

/// A shell command line, as rules see it (contract §9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedCommand {
    /// One or more simple commands joined by `;`, `&&`, `||` or `|`, each as
    /// argv. `piped` marks segments that receive another's output.
    Segments(Vec<Segment>),
    /// Substitution, backgrounding, here-docs, redirection outside the cwd, or
    /// anything the parser doesn't handle: rules can't allow it.
    Unparseable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub argv: Vec<String>,
    pub piped: bool,
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
pub struct SessionGate {
    _policy: Arc<dyn PolicyEngine>,
    _ledger: Arc<dyn ApprovalLedger>,
}
