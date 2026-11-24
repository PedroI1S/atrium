//! Command-unit builder, wrapper unwrapping, and nested-shell expansion
//! (owned by P1; contract §2, §3, §4, §8).
//!
//! Follows the budget discipline in `v4::lex`'s module docs: charge before
//! work, charge before allocation, and reserve storage before the work that
//! discovers the value stored in it.

use crate::policy::limits;
use crate::policy::model::{
    ByteProvenance, Cap, CommandUnit, ComputeBudget, Exhausted, FoldRules, ParsedCommandUnits,
    PartId, PartOutcome, PartResult, PipelineId, RawCommandUnit, RemovedWrapper, UnitIndex,
    UnreadReason, UnwrapResult, ValueRole, Word, WrapperKind,
};
use crate::policy::tools::{FlagKind, ToolSpec};

use super::lex::{
    CoreStop, ExactSink, RawUnitLexMeta, charge_vec_growth, flag_charged, flag_short_charged,
    fold_command_word, lex_best_effort_into, lex_exact_core, lookup_outside, lookup_tool,
    push_reserved, word_as_str, word_has_unquoted_glob,
};

#[cfg(test)]
mod tests;

/// Where a code-runner hook records the reasons it attaches to one unit
/// (contract §4, §6). Reasons go straight into the call's outcomes.
pub(crate) struct UnitReasonSink<'a> {
    outcomes: &'a mut Vec<PartResult>,
    part: PartId,
}

impl UnitReasonSink<'_> {
    /// Reserves room for `n` reasons, plus the slot that records a later
    /// `Exhausted`. A hook that must not lose a reason it builds reserves
    /// before building it; [`Self::push`] then cannot fail.
    pub(crate) fn reserve(
        &mut self,
        n: usize,
        budget: &mut ComputeBudget,
    ) -> Result<(), Exhausted> {
        charge_vec_growth(self.outcomes, n.saturating_add(1), budget)
    }

    /// Records one reason for the unit, charging its slot first (a no-op when
    /// [`Self::reserve`] already made room).
    pub(crate) fn push(
        &mut self,
        reason: UnreadReason,
        budget: &mut ComputeBudget,
    ) -> Result<(), Exhausted> {
        if self.outcomes.capacity() - self.outcomes.len() < 2 {
            self.reserve(1, budget)?;
        }
        push_reserved(
            self.outcomes,
            PartResult {
                part: self.part.clone(),
                outcome: PartOutcome::Unread(reason),
            },
        );
        Ok(())
    }
}

/// Hook called after wrapper unwrapping for each modeled unit so P3's code-runner
/// classifier can attach additional [`UnreadReason`]s without editing P1
/// (contract §4, §6). It receives the unit, its folded command word, the
/// unit's reason sink and the shared budget.
pub(crate) type CodeRunnerHook<'a> = &'a mut dyn FnMut(
    &CommandUnit,
    &str,
    &mut UnitReasonSink<'_>,
    &mut ComputeBudget,
) -> Result<(), Exhausted>;

/// Parses a full command line into modeled [`CommandUnit`]s and a separate
/// [`BestEffortRecord`]: checks `MAX_BYTES` / `MAX_UNITS`, strips §4 wrappers
/// up to `MAX_WRAP`, recursively lexes modeled `sh`/`bash`/`dash -c` scripts
/// up to `MAX_DEPTH`, flags command-word globs, and tracks earlier `HOME`
/// modifications.
///
/// Returns `Err(PartOutcome::Exhausted)` only when the budget is already
/// spent on entry. Exhaustion later records `Exhausted` among the outcomes
/// and keeps every unit and reason found before it.
///
/// **Owned by:** P1 · **Contract:** §2, §3, §4, §8.
pub(crate) fn build_units(
    line: &str,
    folds: &FoldRules,
    budget: &mut ComputeBudget,
) -> Result<ParsedCommandUnits, PartOutcome> {
    build_units_with_hook(line, folds, None, budget)
}

/// Same as [`build_units`], with an optional post-unwrap code-runner hook for
/// P3/P5 integration (§4: classifying code runners like `awk`/`python` is P3's
/// job).
pub(crate) fn build_units_with_hook(
    line: &str,
    folds: &FoldRules,
    mut code_runner_hook: Option<CodeRunnerHook<'_>>,
    budget: &mut ComputeBudget,
) -> Result<ParsedCommandUnits, PartOutcome> {
    if line.len() > limits::MAX_BYTES {
        return Err(PartOutcome::TooLarge(Cap::Bytes));
    }

    let mut parsed = ParsedCommandUnits::default();
    // Charge entry work and reserve the slot that records a later
    // `Exhausted` (invariant: `parsed.outcomes` keeps one free slot until an
    // `Exhausted` is recorded). If even that fails, nothing has been found.
    budget
        .charge(1)
        .and_then(|()| charge_vec_growth(&mut parsed.outcomes, 1, budget))
        .map_err(|e| PartOutcome::Exhausted(e.reason))?;

    let mut next_pipe_offset = 0u32;
    process_text_recursive(
        line,
        0,
        None,
        false,
        folds,
        &mut code_runner_hook,
        &mut next_pipe_offset,
        &mut parsed,
        budget,
    )?;

    // Run the §2 best-effort lexer over the raw line if budget remains.
    // Partial best-effort units found before exhaustion are preserved in
    // `parsed.best_effort`.
    if !budget.is_exhausted() {
        match lex_best_effort_into(&[line], &mut parsed.best_effort, budget) {
            Ok(()) => {}
            Err(PartOutcome::TooLarge(cap)) => return Err(PartOutcome::TooLarge(cap)),
            Err(other) => push_reserved(
                &mut parsed.outcomes,
                PartResult {
                    part: PartId::WholeCall,
                    outcome: other,
                },
            ),
        }
    }

    Ok(parsed)
}

/// [`ExactSink`] that stores the exact lexer's units straight into the
/// call's [`ParsedCommandUnits`] (global indices, offset pipeline IDs), so no
/// unit is copied between lexing and unwrapping.
struct UnitsSink<'p> {
    parsed: &'p mut ParsedCommandUnits,
    base: usize,
    text_part: PartId,
    pipe_offset: u32,
    max_pipe: u32,
    meta: Vec<RawUnitLexMeta>,
}

impl ExactSink for UnitsSink<'_> {
    fn local_units(&self) -> usize {
        self.parsed.units.len() - self.base
    }

    fn at_unit_cap(&self) -> bool {
        self.parsed.units.len() >= limits::MAX_UNITS
    }

    fn reserve_unit(&mut self, budget: &mut ComputeBudget) -> Result<(), Exhausted> {
        charge_vec_growth(&mut self.parsed.units, 1, budget)?;
        charge_vec_growth(&mut self.meta, 1, budget)
    }

    fn reserve_outcomes(
        &mut self,
        free: usize,
        budget: &mut ComputeBudget,
    ) -> Result<(), Exhausted> {
        charge_vec_growth(&mut self.parsed.outcomes, free, budget)
    }

    fn push_unit(&mut self, raw: RawCommandUnit, meta: RawUnitLexMeta) {
        let pipeline_id = raw.pipeline_id.map(|PipelineId(id)| {
            self.max_pipe = self.max_pipe.max(id);
            PipelineId(self.pipe_offset.saturating_add(id))
        });
        push_reserved(
            &mut self.parsed.units,
            CommandUnit {
                index: UnitIndex(self.base + raw.index.0),
                argv: raw.argv,
                removed_wrappers: Vec::new(),
                redirects: raw.redirects,
                separator_before: raw.separator_before,
                pipeline_id,
                depth: raw.depth,
                parent_unit: raw.parent_unit,
            },
        );
        push_reserved(&mut self.meta, meta);
    }

    fn push_unit_outcome(&mut self, local: usize, outcome: PartOutcome) {
        push_reserved(
            &mut self.parsed.outcomes,
            PartResult {
                part: PartId::Unit(UnitIndex(self.base + local)),
                outcome,
            },
        );
    }

    fn push_text_outcome(&mut self, outcome: PartOutcome) {
        push_reserved(
            &mut self.parsed.outcomes,
            PartResult {
                part: self.text_part.clone(),
                outcome,
            },
        );
    }
}

/// Lexes `text` into `parsed`, then classifies each new unit in place and
/// recurses into modeled `-c` scripts. Returns `Err` only for `TooLarge`;
/// exhaustion is recorded in `parsed.outcomes` (into the reserved slot).
#[allow(clippy::too_many_arguments)]
fn process_text_recursive(
    text: &str,
    depth: u8,
    parent_unit: Option<UnitIndex>,
    inherited_home_modified: bool,
    folds: &FoldRules,
    code_runner_hook: &mut Option<CodeRunnerHook<'_>>,
    next_pipe_offset: &mut u32,
    parsed: &mut ParsedCommandUnits,
    budget: &mut ComputeBudget,
) -> Result<(), PartOutcome> {
    let text_part = parent_unit.map_or(PartId::WholeCall, PartId::Unit);
    let base = parsed.units.len();
    let mut sink = UnitsSink {
        parsed,
        base,
        text_part: text_part.clone(),
        pipe_offset: *next_pipe_offset,
        max_pipe: 0,
        meta: Vec::new(),
    };
    match lex_exact_core(text, depth, parent_unit, &mut sink, budget) {
        Ok(()) => {}
        Err(CoreStop::TooLarge(cap)) => return Err(PartOutcome::TooLarge(cap)),
        Err(CoreStop::Entry(e)) => {
            push_reserved(
                &mut sink.parsed.outcomes,
                PartResult {
                    part: text_part,
                    outcome: PartOutcome::Exhausted(e.reason),
                },
            );
            return Ok(());
        }
    }
    let UnitsSink {
        parsed,
        max_pipe,
        meta,
        ..
    } = sink;
    *next_pipe_offset = next_pipe_offset.saturating_add(max_pipe);
    if budget.is_exhausted() {
        // The lexer recorded `Exhausted` and kept every unit it found.
        return Ok(());
    }

    let end = parsed.units.len();
    let mut home_modified = inherited_home_modified;
    let mut nested_scripts: Vec<(UnitIndex, String, bool)> = Vec::new();
    for k in base..end {
        if let Err(e) = process_unit(
            k,
            meta[k - base],
            &mut home_modified,
            folds,
            code_runner_hook,
            &mut nested_scripts,
            parsed,
            budget,
        ) {
            // Units from `k` on keep their lexed form; all are in `parsed`.
            push_reserved(
                &mut parsed.outcomes,
                PartResult {
                    part: PartId::Unit(UnitIndex(k)),
                    outcome: PartOutcome::Exhausted(e.reason),
                },
            );
            return Ok(());
        }
    }

    // Recursively lex modeled `sh`/`bash`/`dash -c SCRIPT` payloads.
    for (parent_idx, script, child_home_modified) in nested_scripts {
        process_text_recursive(
            &script,
            depth + 1,
            Some(parent_idx),
            child_home_modified,
            folds,
            code_runner_hook,
            next_pipe_offset,
            parsed,
            budget,
        )?;
        if budget.is_exhausted() {
            break;
        }
    }

    Ok(())
}

/// Outcome slots the fixed checks of [`process_unit`] may fill: the wrapper
/// reason, `HomeSetEarlier`, and one of `CommandWordGlob`, a nested-shell
/// reason or `MaxNestingDepthReached`.
const UNIT_FIXED_REASONS: usize = 3;

/// Classifies unit `k` of `parsed` in place: §4 wrappers, `HOME` tracking
/// (§3), command-word globs and nested shells (§4), and the P3 hook.
#[allow(clippy::too_many_arguments)]
fn process_unit(
    k: usize,
    meta: RawUnitLexMeta,
    home_modified: &mut bool,
    folds: &FoldRules,
    code_runner_hook: &mut Option<CodeRunnerHook<'_>>,
    nested_scripts: &mut Vec<(UnitIndex, String, bool)>,
    parsed: &mut ParsedCommandUnits,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    let global_idx = UnitIndex(k);
    let part = PartId::Unit(global_idx);
    // Reserve, before any classification work, the slots its fixed reasons
    // may fill plus the spare that records a later `Exhausted` (§8).
    charge_vec_growth(&mut parsed.outcomes, UNIT_FIXED_REASONS + 1, budget)?;
    let unread = |reason: UnreadReason| PartResult {
        part: part.clone(),
        outcome: PartOutcome::Unread(reason),
    };

    // §4 wrappers, removed in place. A reason found before exhaustion is kept.
    let (wrapper_reason, unwrapped) = unwrap_in_place(&mut parsed.units[k], folds, budget);
    if let Some(reason) = wrapper_reason {
        push_reserved(&mut parsed.outcomes, unread(reason));
    }
    unwrapped?;

    let unit = &parsed.units[k];
    let mut unit_sets_home = meta.sets_home;
    let assign_prefix = meta.leading_assignment_count;

    // A unit using an unquoted `~` after an earlier unit modified `HOME` (§3).
    if *home_modified && unit_uses_unquoted_tilde(unit, budget)? {
        push_reserved(&mut parsed.outcomes, unread(UnreadReason::HomeSetEarlier));
    }

    // A command after leading assignments that modifies `HOME`
    // (`FOO=1 export HOME=...`, `FOO=1 unset HOME`, `FOO=1 env -u HOME ...`).
    if unit.removed_wrappers.is_empty()
        && assign_prefix > 0
        && let Some(cmd_after_assign) = unit.argv.get(assign_prefix)
        && !word_has_unquoted_glob(cmd_after_assign, budget)?
    {
        let folded_after = fold_command_word(cmd_after_assign, folds, budget)?;
        if command_modifies_home(&folded_after, &unit.argv[assign_prefix..], budget)? {
            unit_sets_home = true;
        }
    }

    // Effective command word: command-word glob (§4), nested shell (§4),
    // HOME-modifying commands (§3), and the optional P3 code-runner hook.
    if let Some(first) = unit.argv.first() {
        if word_has_unquoted_glob(first, budget)? {
            push_reserved(&mut parsed.outcomes, unread(UnreadReason::CommandWordGlob));
        } else {
            let folded = fold_command_word(first, folds, budget)?;
            if command_modifies_home(&folded, &unit.argv, budget)? {
                unit_sets_home = true;
            }

            if let Some(spec) = lookup_tool(&folded, budget)? {
                if spec.is_nested_shell() {
                    if unit.depth < limits::MAX_DEPTH {
                        charge_vec_growth(nested_scripts, 1, budget)?;
                    }
                    match classify_nested_shell(unit, &folded, spec, budget)? {
                        NestedShellOutcome::ModeledScript(_) if unit.depth >= limits::MAX_DEPTH => {
                            push_reserved(
                                &mut parsed.outcomes,
                                unread(UnreadReason::MaxNestingDepthReached),
                            );
                        }
                        NestedShellOutcome::ModeledScript(script) => {
                            push_reserved(nested_scripts, (global_idx, script, *home_modified));
                        }
                        NestedShellOutcome::Unread(reason) => {
                            push_reserved(&mut parsed.outcomes, unread(reason));
                        }
                    }
                }
            } else if let Some(out_entry) = lookup_outside(&folded, budget)?
                && out_entry.contract_section == "§4"
                && !out_entry.reason.starts_with("Wrapper")
            {
                // Shell outside §4's sh/bash/dash subset (`zsh`, `ksh`, `fish`, …).
                budget.charge(folded.len().max(1) as u64)?;
                budget.charge_alloc(folded.len())?;
                push_reserved(
                    &mut parsed.outcomes,
                    unread(UnreadReason::UnsupportedNestedShellForm {
                        shell: folded.clone(),
                    }),
                );
            }

            // Optional P3 code-runner hook.
            if let Some(hook) = code_runner_hook.as_deref_mut() {
                let mut sink = UnitReasonSink {
                    outcomes: &mut parsed.outcomes,
                    part: part.clone(),
                };
                hook(unit, &folded, &mut sink, budget)?;
            }
        }
    }

    if unit_sets_home {
        *home_modified = true;
    }
    Ok(())
}

/// Strips §4's closed list of transparent wrappers (`command`, `exec`, `nice`,
/// `timeout`, `time`, `env` in their exact modeled forms) from one raw unit,
/// bounded by `limits::MAX_WRAP`.
///
/// P0's signature takes the unit by value and returns only `Exhausted` on
/// exhaustion, so a caller that needs the unit afterwards keeps its own copy
/// (`build_units` unwraps in place and loses nothing).
///
/// **Owned by:** P1 · **Contract:** §4, §8.
pub(crate) fn unwrap_unit(
    raw: RawCommandUnit,
    folds: &FoldRules,
    budget: &mut ComputeBudget,
) -> Result<UnwrapResult, Exhausted> {
    let mut unit = CommandUnit {
        index: raw.index,
        argv: raw.argv,
        removed_wrappers: Vec::new(),
        redirects: raw.redirects,
        separator_before: raw.separator_before,
        pipeline_id: raw.pipeline_id,
        depth: raw.depth,
        parent_unit: raw.parent_unit,
    };
    let (unread, res) = unwrap_in_place(&mut unit, folds, budget);
    res?;
    Ok(UnwrapResult { unit, unread })
}

/// Most words one §4 wrapper form strips (`timeout -s SIG -k D D`).
const MAX_WRAPPER_WORDS: usize = 6;

/// Removes §4 wrappers from the front of `unit.argv` into
/// `unit.removed_wrappers`, in place.
///
/// The wrapper words are moved once, at the end, with one shift of the
/// argv, instead of one shift per wrapper. The shift is paid for before the
/// first wrapper is parsed, and each wrapper's storage (its
/// `removed_wrappers` slot and up to `MAX_WRAPPER_WORDS` words) before it is
/// parsed. So on exhaustion every wrapper already recognised is still moved
/// out and kept, along with any wrapper reason already found (review round
/// 4: no finished result is dropped on an error path).
fn unwrap_in_place(
    unit: &mut CommandUnit,
    folds: &FoldRules,
    budget: &mut ComputeBudget,
) -> (Option<UnreadReason>, Result<(), Exhausted>) {
    let mut unread: Option<UnreadReason> = None;
    let mut plan = [(WrapperKind::Command, 0usize); limits::MAX_WRAP];
    let mut planned = 0usize;
    let mut pos = 0usize;

    let scan = (|| -> Result<(), Exhausted> {
        budget.charge(1)?;
        while let Some(first) = unit.argv.get(pos) {
            // Never treat a globbed command word as a wrapper (§4).
            if word_has_unquoted_glob(first, budget)? {
                break;
            }
            let folded = fold_command_word(first, folds, budget)?;
            let Some(spec) = lookup_tool(&folded, budget)? else {
                // Check if `folded` is an explicit outside-model wrapper in P0's table.
                if let Some(out) = lookup_outside(&folded, budget)?
                    && out.contract_section == "§4"
                    && out.reason.starts_with("Wrapper")
                {
                    unread = Some(UnreadReason::UnsupportedWrapperForm { wrapper: folded });
                }
                break;
            };
            let Some(kind) = spec.wrapper() else {
                break;
            };
            // Pay, before parsing, for storing this wrapper once recognised.
            if planned == 0 {
                budget.charge(unit.argv.len() as u64)?;
            }
            budget.charge(MAX_WRAPPER_WORDS as u64)?;
            budget.charge_alloc(MAX_WRAPPER_WORDS * std::mem::size_of::<Word>())?;
            charge_vec_growth(&mut unit.removed_wrappers, planned + 1, budget)?;
            match parse_wrapper_prefix(kind, spec, &unit.argv[pos..], budget)? {
                Some(strip_count) => {
                    debug_assert!(strip_count <= MAX_WRAPPER_WORDS);
                    plan[planned] = (kind, strip_count);
                    planned += 1;
                    pos += strip_count;
                    // Reaching `MAX_WRAP` (8 removals) makes the unit Unread (§4).
                    if planned >= limits::MAX_WRAP {
                        unread = Some(UnreadReason::MaxWrapReached);
                        break;
                    }
                }
                None => {
                    unread = Some(UnreadReason::UnsupportedWrapperForm { wrapper: folded });
                    break;
                }
            }
        }
        Ok(())
    })();

    // Move the recognised wrappers out; all of it was paid for above.
    if planned > 0 {
        let mut drained = unit.argv.drain(..pos);
        for &(kind, count) in &plan[..planned] {
            let mut words = Vec::with_capacity(count);
            words.extend(drained.by_ref().take(count));
            push_reserved(&mut unit.removed_wrappers, RemovedWrapper { kind, words });
        }
    }
    (unread, scan)
}

/// Validates a single §4 wrapper at the front of `argv` using `spec.flags()`
/// from P0's `tools.rs`. Returns `Ok(Some(word_count_to_strip))` when the
/// invocation matches §4's closed form, or `Ok(None)` for any near miss.
/// Every scan of an operand is charged before it runs (review round 3,
/// finding 2).
fn parse_wrapper_prefix(
    kind: WrapperKind,
    spec: &'static ToolSpec,
    argv: &[Word],
    budget: &mut ComputeBudget,
) -> Result<Option<usize>, Exhausted> {
    budget.charge(1)?;
    match kind {
        WrapperKind::Command => {
            // `command [--] CMD ...`
            let Some(next) = argv.get(1) else {
                return Ok(None);
            };
            let nb = next.literal_bytes();
            if nb == b"--" {
                Ok((argv.len() >= 3).then_some(2))
            } else if nb.starts_with(b"-") {
                Ok(None)
            } else {
                Ok(Some(1))
            }
        }
        WrapperKind::Exec | WrapperKind::Time => {
            // `exec CMD ...`, `time CMD ...` (no options)
            let Some(next) = argv.get(1) else {
                return Ok(None);
            };
            if next.literal_bytes().starts_with(b"-") {
                Ok(None)
            } else {
                Ok(Some(1))
            }
        }
        WrapperKind::Env => {
            // Bare `env CMD ...` (no options, no assignments — even quoted `=`
            // is interpreted as an assignment by `env(1)`).
            let Some(next) = argv.get(1) else {
                return Ok(None);
            };
            let nb = next.literal_bytes();
            if nb.starts_with(b"-") {
                return Ok(None);
            }
            budget.charge(nb.len().max(1) as u64)?;
            if nb.contains(&b'=') {
                Ok(None)
            } else {
                Ok(Some(1))
            }
        }
        WrapperKind::Nice => {
            // `nice CMD ...` or `nice -n N CMD ...`
            let Some(next) = argv.get(1) else {
                return Ok(None);
            };
            if !next.literal_bytes().starts_with(b"-") {
                return Ok(Some(1));
            }
            let Some(tok) = word_as_str(next, budget)? else {
                return Ok(None);
            };
            let Some(fspec) = flag_charged(spec.flags(), tok, budget)? else {
                return Ok(None);
            };
            if fspec.short != Some('n') {
                return Ok(None);
            }
            let (Some(n_word), Some(cmd_word)) = (argv.get(2), argv.get(3)) else {
                return Ok(None);
            };
            if !is_signed_integer(n_word, budget)? || cmd_word.literal_bytes().starts_with(b"-") {
                return Ok(None);
            }
            Ok(Some(3))
        }
        WrapperKind::Timeout => {
            // `timeout [-s SIG] [-k D] D CMD ...`
            let mut j = 1usize;
            let mut seen_s = false;
            let mut seen_k = false;
            while let Some(w) = argv.get(j) {
                budget.charge(1)?;
                if !w.literal_bytes().starts_with(b"-") {
                    break;
                }
                let Some(tok) = word_as_str(w, budget)? else {
                    return Ok(None);
                };
                let Some(fspec) = flag_charged(spec.flags(), tok, budget)? else {
                    return Ok(None);
                };
                match fspec.short {
                    Some('s') if !seen_s => {
                        let Some(sig) = argv.get(j + 1) else {
                            return Ok(None);
                        };
                        if !is_timeout_signal(sig, budget)? {
                            return Ok(None);
                        }
                        seen_s = true;
                        j += 2;
                    }
                    Some('k') if !seen_k => {
                        let Some(dur) = argv.get(j + 1) else {
                            return Ok(None);
                        };
                        if !is_timeout_duration(dur, budget)? {
                            return Ok(None);
                        }
                        seen_k = true;
                        j += 2;
                    }
                    _ => return Ok(None),
                }
            }
            // Required duration `D` followed by `CMD ...`.
            let (Some(dur), Some(cmd)) = (argv.get(j), argv.get(j + 1)) else {
                return Ok(None);
            };
            if !is_timeout_duration(dur, budget)? || cmd.literal_bytes().starts_with(b"-") {
                return Ok(None);
            }
            Ok(Some(j + 1))
        }
    }
}

/// Iterates a word's bytes with a flag for "unquoted glob character",
/// for single-pass validators.
fn bytes_with_glob(word: &Word) -> impl Iterator<Item = (u8, bool)> + '_ {
    word.literal_bytes()
        .iter()
        .zip(word.provenance())
        .map(|(&b, &p)| {
            (
                b,
                p == ByteProvenance::Unquoted && matches!(b, b'*' | b'?' | b'['),
            )
        })
}

/// `[+-]DIGITS` with no unquoted glob and no leading unquoted `~`: one pass,
/// charged by the word's length before it runs.
fn is_signed_integer(word: &Word, budget: &mut ComputeBudget) -> Result<bool, Exhausted> {
    budget.charge(word.len().max(1) as u64)?;
    if word.starts_with_unquoted_tilde() {
        return Ok(false);
    }
    let mut digits = 0usize;
    for (i, (b, glob)) in bytes_with_glob(word).enumerate() {
        if glob {
            return Ok(false);
        }
        if i == 0 && matches!(b, b'+' | b'-') {
            continue;
        }
        if !b.is_ascii_digit() {
            return Ok(false);
        }
        digits += 1;
    }
    Ok(digits > 0)
}

/// A `timeout -s` signal name or number: one pass, charged by the word's
/// length before it runs.
fn is_timeout_signal(word: &Word, budget: &mut ComputeBudget) -> Result<bool, Exhausted> {
    budget.charge(word.len().max(1) as u64)?;
    if word.starts_with_unquoted_tilde() {
        return Ok(false);
    }
    let b = word.literal_bytes();
    if b.is_empty() || b.starts_with(b"-") {
        return Ok(false);
    }
    for (c, glob) in bytes_with_glob(word) {
        if glob || !(c.is_ascii_alphanumeric() || matches!(c, b'_' | b'+' | b'-')) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// A `timeout` duration (`N`, `N.N`, optional `s`/`m`/`h`/`d` suffix): one
/// pass, charged by the word's length before it runs.
fn is_timeout_duration(word: &Word, budget: &mut ComputeBudget) -> Result<bool, Exhausted> {
    budget.charge(word.len().max(1) as u64)?;
    if word.starts_with_unquoted_tilde() {
        return Ok(false);
    }
    let b = word.literal_bytes();
    if b.is_empty() || b.starts_with(b"-") {
        return Ok(false);
    }
    let num_len = match b.last() {
        Some(b's' | b'm' | b'h' | b'd') => b.len() - 1,
        _ => b.len(),
    };
    if num_len == 0 {
        return Ok(false);
    }
    let mut dots = 0usize;
    let mut digits = 0usize;
    for (i, (c, glob)) in bytes_with_glob(word).enumerate() {
        if glob {
            return Ok(false);
        }
        if i >= num_len {
            continue;
        }
        if c == b'.' {
            dots += 1;
            if dots > 1 {
                return Ok(false);
            }
        } else if c.is_ascii_digit() {
            digits += 1;
        } else {
            return Ok(false);
        }
    }
    Ok(digits > 0)
}

// ---------------------------------------------------------------------------
// Nested-shell classification (§4)
// ---------------------------------------------------------------------------

enum NestedShellOutcome {
    ModeledScript(String),
    Unread(UnreadReason),
}

fn unsupported_nested_shell(
    folded_shell: &str,
    budget: &mut ComputeBudget,
) -> Result<NestedShellOutcome, Exhausted> {
    budget.charge(folded_shell.len().max(1) as u64)?;
    budget.charge_alloc(folded_shell.len())?;
    Ok(NestedShellOutcome::Unread(
        UnreadReason::UnsupportedNestedShellForm {
            shell: folded_shell.to_string(),
        },
    ))
}

/// Classifies `NAME [-e|-u|-x]... -c SCRIPT` for `sh`/`bash`/`dash` (§4).
/// Every scan of an operand and every flag lookup is charged before it runs.
fn classify_nested_shell(
    unit: &CommandUnit,
    folded_shell: &str,
    spec: &'static ToolSpec,
    budget: &mut ComputeBudget,
) -> Result<NestedShellOutcome, Exhausted> {
    let argv = &unit.argv;
    let Some(first_arg) = argv.get(1) else {
        // Stdin shell (`sh`, `bash`, `dash`).
        return unsupported_nested_shell(folded_shell, budget);
    };

    let fb = first_arg.literal_bytes();

    // Script file: `sh x.sh [args...]` -> `Unread (program)` (§4, §7).
    if !fb.is_empty() && !fb.starts_with(b"-") && !fb.starts_with(b"+") {
        if !word_has_unquoted_glob(first_arg, budget)?
            && let Some(cmd_s) = word_as_str(&argv[0], budget)?
            && let Some(script_s) = word_as_str(first_arg, budget)?
        {
            let alloc = cmd_s.len() + script_s.len() + 2 * std::mem::size_of::<String>();
            budget.charge(alloc as u64)?;
            budget.charge_alloc(alloc)?;
            return Ok(NestedShellOutcome::Unread(UnreadReason::Program {
                program_words: vec![cmd_s.to_string(), script_s.to_string()],
            }));
        }
        return unsupported_nested_shell(folded_shell, budget);
    }

    // Modeled `-c` form: `NAME [-e|-u|-x]... -c SCRIPT` (nothing after SCRIPT).
    let mut j = 1usize;
    let mut saw_c = false;
    while let Some(w) = argv.get(j) {
        budget.charge(1)?;
        let wb = w.literal_bytes();
        if !wb.starts_with(b"-") || wb == b"-" || wb.starts_with(b"--") {
            return unsupported_nested_shell(folded_shell, budget);
        }
        let letters = &wb[1..];
        if letters.is_empty() {
            return unsupported_nested_shell(folded_shell, budget);
        }
        for (k, &lb) in letters.iter().enumerate() {
            if !lb.is_ascii() {
                return unsupported_nested_shell(folded_shell, budget);
            }
            let Some(fspec) = flag_short_charged(spec.flags(), lb as char, budget)? else {
                return unsupported_nested_shell(folded_shell, budget);
            };
            match fspec.kind {
                FlagKind::Bool => {}
                FlagKind::Value(ValueRole::Code) => {
                    // `-c` must be the final flag letter (`-ec` ok; `-ce` rejected).
                    if k + 1 != letters.len() {
                        return unsupported_nested_shell(folded_shell, budget);
                    }
                    saw_c = true;
                }
                _ => {
                    return unsupported_nested_shell(folded_shell, budget);
                }
            }
        }
        j += 1;
        if saw_c {
            break;
        }
    }

    // Must have `-c`, followed by EXACTLY ONE `SCRIPT` word and nothing after it.
    if !saw_c || j + 1 != argv.len() {
        return unsupported_nested_shell(folded_shell, budget);
    }

    let script_word = &argv[j];
    if script_word.starts_with_unquoted_tilde() || word_has_unquoted_glob(script_word, budget)? {
        return unsupported_nested_shell(folded_shell, budget);
    }
    let Some(script) = word_as_str(script_word, budget)? else {
        return unsupported_nested_shell(folded_shell, budget);
    };

    budget.charge(script.len().max(1) as u64)?;
    budget.charge_alloc(script.len())?;
    Ok(NestedShellOutcome::ModeledScript(script.to_string()))
}

// ---------------------------------------------------------------------------
// HOME modification tracking (§3)
// ---------------------------------------------------------------------------

/// Bytes compared against constants per argument by `export`-style and
/// `printf` checks (`HOME`, `HOME=`, `HOME+=`; `--`, `-v`, `-vHOME`, `HOME`).
const HOME_CONST_BYTES: u64 = 16;

/// Bytes compared against constants per argument by the `env` check (the sum
/// of its option spellings is 97 bytes).
const ENV_CONST_BYTES: u64 = 128;

fn word_targets_home_var(w: &Word) -> bool {
    let b = w.literal_bytes();
    b == b"HOME" || b.starts_with(b"HOME=") || b.starts_with(b"HOME+=")
}

/// One pass over an `env` argument: whether it contains `=`, and in the
/// option letters after the leading `-`, whether `i` or `S` occur and where
/// the first `u` is.
fn scan_env_word(b: &[u8]) -> (bool, bool, bool, Option<usize>) {
    let mut has_eq = false;
    let mut has_i = false;
    let mut has_s = false;
    let mut first_u = None;
    for (idx, &c) in b.iter().enumerate() {
        has_eq |= c == b'=';
        if idx >= 1 {
            has_i |= c == b'i';
            has_s |= c == b'S';
            if c == b'u' && first_u.is_none() {
                first_u = Some(idx - 1);
            }
        }
    }
    (has_eq, has_i, has_s, first_u)
}

/// Whether `argv` (starting at the folded command word `folded`) sets or
/// clears `HOME`. Each argument inspected is charged before it is inspected.
fn command_modifies_home(
    folded: &str,
    argv: &[Word],
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    let args = &argv[1..];
    let charge_const = |budget: &mut ComputeBudget, w: &Word| {
        budget.charge(1 + (w.len() as u64).min(HOME_CONST_BYTES))
    };
    match folded {
        "export" | "declare" | "typeset" | "local" | "readonly" | "read" => {
            for w in args {
                charge_const(budget, w)?;
                if word_targets_home_var(w) {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        "unset" => {
            for w in args {
                charge_const(budget, w)?;
                if w.literal_bytes() == b"HOME" {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        "printf" => {
            // `printf -v HOME ...` or `printf -vHOME ...` sets `HOME` in bash.
            let mut i = 0usize;
            while i < args.len() {
                charge_const(budget, &args[i])?;
                let b = args[i].literal_bytes();
                if b == b"--" {
                    break;
                }
                if b == b"-v"
                    && args
                        .get(i + 1)
                        .is_some_and(|w| w.literal_bytes() == b"HOME")
                {
                    return Ok(true);
                }
                if b == b"-vHOME" {
                    return Ok(true);
                }
                i += 1;
            }
            Ok(false)
        }
        "env" => {
            // Bare `env` was already stripped by `unwrap_unit`; any remaining
            // `env` unit may clear (`-i`), unset (`-u HOME`), split-string
            // (`-S`), or assign `HOME=...` (including after `--`).
            let mut i = 0usize;
            let mut after_double_dash = false;
            while i < args.len() {
                let b = args[i].literal_bytes();
                budget.charge(ENV_CONST_BYTES + b.len() as u64)?;
                let (has_eq, has_i, has_s, first_u) = scan_env_word(b);
                if !after_double_dash && b == b"--" {
                    after_double_dash = true;
                    i += 1;
                    continue;
                }
                if b.starts_with(b"HOME=") {
                    return Ok(true);
                }
                if after_double_dash {
                    if !has_eq {
                        break;
                    }
                    i += 1;
                    continue;
                }
                if b == b"-i"
                    || b == b"-"
                    || b == b"--ignore-environment"
                    || b == b"-S"
                    || b.starts_with(b"-S")
                    || b == b"--split-string"
                    || b.starts_with(b"--split-string=")
                    || b == b"--unset=HOME"
                    || b == b"-uHOME"
                {
                    return Ok(true);
                }
                if (b == b"-u" || b == b"--unset")
                    && args
                        .get(i + 1)
                        .is_some_and(|w| w.literal_bytes() == b"HOME")
                {
                    return Ok(true);
                }
                if b.starts_with(b"-") && !b.starts_with(b"--") {
                    if has_i || has_s {
                        return Ok(true);
                    }
                    if let Some(pos) = first_u {
                        let tail = &b[1..][pos + 1..];
                        if tail == b"HOME"
                            || (tail.is_empty()
                                && args
                                    .get(i + 1)
                                    .is_some_and(|w| w.literal_bytes() == b"HOME"))
                        {
                            return Ok(true);
                        }
                    }
                }
                i += 1;
            }
            Ok(false)
        }
        _ => Ok(false),
    }
}

/// Whether any word of the unit as written, or any redirect target, starts
/// with an unquoted `~`. One unit of work per word, charged before the walk.
fn unit_uses_unquoted_tilde(
    unit: &CommandUnit,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    budget.charge(1 + unit.removed_wrappers.len() as u64)?;
    let words = unit
        .removed_wrappers
        .iter()
        .map(|w| w.words.len())
        .sum::<usize>()
        + unit.argv.len()
        + unit.redirects.len();
    budget.charge(words as u64)?;
    Ok(unit.original_argv().any(Word::starts_with_unquoted_tilde)
        || unit
            .redirects
            .iter()
            .any(|r| r.target().is_some_and(Word::starts_with_unquoted_tilde)))
}
