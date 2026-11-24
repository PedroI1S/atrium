//! Exact and best-effort shell lexers (owned by P1; contract §2, §3).

use crate::policy::model::{
    BestEffortRecord, ComputeBudget, Exhausted, FoldRules, LexExactOutput, PartOutcome, UnitIndex,
    Word,
};

/// Lexes one command text into [`RawCommandUnit`]s with per-byte provenance
/// under the intersection of POSIX `sh`, `bash`, and `zsh`, recording every
/// §3 construct outside the model as an `UnreadReason`.
///
/// **Owned by:** P1 · **Contract:** §3, §8.
pub(crate) fn lex_exact(
    _line: &str,
    _depth: u8,
    _parent_unit: Option<UnitIndex>,
    _budget: &mut ComputeBudget,
) -> Result<LexExactOutput, PartOutcome> {
    todo!("P1: implement §3 exact lexer")
}

/// Runs the tolerant best-effort lexer over `raw_texts`, producing
/// [`BestEffortRecord`] (including units derived from `-c` scripts and
/// substitutions) under the shared compute budget.
///
/// **Owned by:** P1 · **Contract:** §2, §8.
pub(crate) fn lex_best_effort(
    _raw_texts: &[&str],
    _budget: &mut ComputeBudget,
) -> Result<BestEffortRecord, PartOutcome> {
    todo!("P1: implement §2 best-effort lexer")
}

/// Folds a command word according to CR-19 / `FoldRules` (path basename +
/// listed case folds, no Unicode normalization).
///
/// **Owned by:** P1 · **Contract:** §4, §8.
pub(crate) fn fold_command_word(
    _word: &Word,
    _folds: &FoldRules,
    _budget: &mut ComputeBudget,
) -> Result<String, Exhausted> {
    todo!("P1: implement CR-19 command-word folding")
}
