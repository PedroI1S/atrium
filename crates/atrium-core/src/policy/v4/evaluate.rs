//! Rev 4 decision procedure and end-to-end call evaluation (owned by P5;
//! contract §1, §2).

use atrium_protocol::policy::{Preset, Rule};

use crate::policy::model::{
    BestEffortRecord, CallEvaluation, CallOutcome, ComputeBudget, Exhausted, PartResult,
    ProtectedSet, RuleCoverage,
};
use crate::policy::v4::paths::PathResolver;
use crate::{PolicyInput, PolicyVerdict};

/// Runs the §2 best-effort pass over raw text, tolerant-lexer units (including
/// derived `-c` script units), and hard-deny glob/search-filter probes.
///
/// **Owned by:** P5 · **Contract:** §2, §8.
pub(crate) fn run_best_effort(
    _record: &BestEffortRecord,
    _protected: &ProtectedSet,
    _budget: &mut ComputeBudget,
) -> Result<Vec<PartResult>, Exhausted> {
    todo!("P5: implement §2 best-effort pass")
}

/// Pure §1 decision procedure combining every part's outcome in `outcome` with
/// `rule_coverage`, `preset`, and Home read-only eligibility:
/// 1. `TooLarge` → `Deny` ("too large to check");
/// 2. Any `HardDenyHit` (model or best-effort) → `HardDeny`;
/// 3. Winning Deny rule → `Deny`;
/// 4. Multiply-linked regular file → `Ask`;
/// 5. Any `Exhausted` part → `Ask`;
/// 6. Every part authorized by §2's table under `preset` and covering rules →
///    `Allow`, otherwise `Ask`.
///
/// **Owned by:** P5 · **Contract:** §1, §2.
pub(crate) fn decide(
    _outcome: &CallOutcome,
    _rule_coverage: &RuleCoverage,
    _preset: Preset,
    _home_readonly_all: bool,
) -> PolicyVerdict {
    todo!("P5: implement §1–§2 decision procedure")
}

/// Full rev 4 evaluation entry point wiring P1 (lex/units), P3 (tool effects),
/// P2 (paths/globs/filters), best-effort pass, P3 (rules), and [`decide`].
///
/// **Owned by:** P5 · **Contract:** §1, §2, §13.
pub(crate) fn evaluate_call(
    _input: &PolicyInput<'_>,
    _rules: &[Rule],
    _protected: &ProtectedSet,
    _resolver: &dyn PathResolver,
) -> CallEvaluation {
    todo!("P5: wire P1–P3 into rev 4 evaluator")
}
