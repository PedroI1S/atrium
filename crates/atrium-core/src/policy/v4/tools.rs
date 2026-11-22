//! Tool-table application and Git name-selection checks (owned by P3;
//! contract §6).

use crate::policy::model::{
    CommandUnit, ComputeBudget, Exhausted, FoldRules, PartResult, ToolEffects,
};

/// Applies `policy::tools`'s normative table to one modeled [`CommandUnit`],
/// assigning per-position [`OperandRole`]s, detecting unknown flags,
/// computing write/recursive/link/code effects, extracting search filters, and
/// checking Home read-only eligibility on the command word as written.
///
/// **Owned by:** P3 · **Contract:** §6, §8.
pub(crate) fn classify_unit(
    _unit: &CommandUnit,
    _folds: &FoldRules,
    _budget: &mut ComputeBudget,
) -> Result<ToolEffects, Exhausted> {
    todo!("P3: implement §6 tool-table classification")
}

/// Runs CR-12 Git selection checks on `git show`/`cat-file`/`log`/`diff`:
/// committed credential names, credential-ancestor pathspecs, pathspec
/// magic/exclusion (`StoreHit`), and active wildcard pathspecs
/// (`Unread(GitWildcardPathspec)`).
///
/// **Owned by:** P3 · **Contract:** §6, §8.
pub(crate) fn check_git_selection(
    _unit: &CommandUnit,
    _effects: &ToolEffects,
    _folds: &FoldRules,
    _budget: &mut ComputeBudget,
) -> Result<Vec<PartResult>, Exhausted> {
    todo!("P3: implement §6 git name-selection and wildcard-pathspec checks")
}
