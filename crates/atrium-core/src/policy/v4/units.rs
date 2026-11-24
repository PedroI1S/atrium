//! Command-unit builder, wrapper unwrapping, and nested-shell expansion
//! (owned by P1; contract §3, §4).

use crate::policy::model::{
    ComputeBudget, Exhausted, FoldRules, ParsedCommandUnits, PartOutcome, RawCommandUnit,
    UnwrapResult,
};

/// Parses a full command line into modeled [`CommandUnit`]s and a separate
/// [`BestEffortRecord`]: checks `MAX_BYTES` / `MAX_UNITS`, strips §4 wrappers
/// up to `MAX_WRAP`, recursively lexes modeled `sh`/`bash`/`dash -c` scripts
/// up to `MAX_DEPTH`, flags command-word globs, and tracks earlier `HOME`
/// modifications.
///
/// **Owned by:** P1 · **Contract:** §2, §3, §4, §8.
pub(crate) fn build_units(
    _line: &str,
    _folds: &FoldRules,
    _budget: &mut ComputeBudget,
) -> Result<ParsedCommandUnits, PartOutcome> {
    todo!("P1: implement §3–§4 unit building and nested-shell expansion")
}

/// Strips §4's closed list of transparent wrappers (`command`, `exec`, `nice`,
/// `timeout`, `time`, `env` in their exact modeled forms) from one raw unit,
/// bounded by `limits::MAX_WRAP`.
///
/// **Owned by:** P1 · **Contract:** §4, §8.
pub(crate) fn unwrap_unit(
    _raw: RawCommandUnit,
    _folds: &FoldRules,
    _budget: &mut ComputeBudget,
) -> Result<UnwrapResult, Exhausted> {
    todo!("P1: implement §4 wrapper unwrapping")
}
