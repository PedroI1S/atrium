//! Rule validation, rule coverage, and "always…" matcher generation
//! (owned by P3; contract §7).

use atrium_protocol::policy::{Rule, RuleDecision, RuleMatcher};

use crate::policy::model::{
    CallOutcome, ComputeBudget, Exhausted, FoldRules, RuleCoverage, RuleEvalContext,
    RuleValidationError,
};

/// Validates a rule matcher against §7's code-runner pinning rules (rejects
/// Allow prefixes ending at a code runner with no pinned program, e.g.
/// `["awk"]`, `["python"]`, `["pipx", "run"]`, `["git"]`).
///
/// Used both at rule creation (§11 item 4) and when evaluating stored rules so
/// newly rejected stored rules stop covering anything (§11 item 5).
///
/// **Owned by:** P3 · **Contract:** §7.
pub(crate) fn validate_rule_matcher(
    _matcher: &RuleMatcher,
    _decision: RuleDecision,
    _folds: &FoldRules,
) -> Result<(), RuleValidationError> {
    todo!("P3: implement §7 rule validation")
}

/// Evaluates `rules` against a call under `dev`'s precedence (thread > Home >
/// global, Deny before Allow within each rank): Deny rules match exact and
/// best-effort units; Allow rules cover `Clear`, `StoreHit`, or pinned
/// `Unread (program)` units meeting simple-command and canonical path-scope
/// conditions.
///
/// **Owned by:** P3 · **Contract:** §1 step 3, §7, §8.
pub(crate) fn evaluate_rules(
    _call: &RuleEvalContext<'_>,
    _rules: &[Rule],
    _budget: &mut ComputeBudget,
) -> Result<RuleCoverage, Exhausted> {
    todo!("P3: implement §7 rule coverage")
}

/// Computes the "always…" matcher (`CommandExact` or `PathGlob`) for an Ask
/// call, returning `None` whenever any part of `outcome` is `Unread` or
/// `Exhausted`.
///
/// **Owned by:** P3 · **Contract:** §7, §9.
pub(crate) fn remember_matcher(
    _call: &RuleEvalContext<'_>,
    _outcome: &CallOutcome,
) -> Option<RuleMatcher> {
    todo!("P3: implement §7 remember_matcher")
}
