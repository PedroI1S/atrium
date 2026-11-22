//! Command-unit builder, wrapper unwrapping, and nested-shell expansion
//! (owned by P1; contract §2, §3, §4, §8).

use crate::policy::limits;
use crate::policy::model::{
    Cap, CommandUnit, ComputeBudget, Exhausted, FoldRules, ParsedCommandUnits, PartId, PartOutcome,
    PartResult, PipelineId, RawCommandUnit, RemovedWrapper, UnitIndex, UnreadReason, UnwrapResult,
    ValueRole, Word, WrapperKind,
};
use crate::policy::tools::{self, FlagKind, ToolSpec};

use super::lex::{fold_command_word, leading_assignment_name, lex_best_effort_into, lex_exact};

#[cfg(test)]
mod tests;

/// Hook called after wrapper unwrapping for each modeled unit so P3's code-runner
/// classifier can attach additional [`UnreadReason`]s without editing P1
/// (contract §4, §6).
pub(crate) type CodeRunnerHook<'a> = &'a mut dyn FnMut(
    &CommandUnit,
    &str,
    &mut ComputeBudget,
) -> Result<Vec<UnreadReason>, Exhausted>;

/// Parses a full command line into modeled [`CommandUnit`]s and a separate
/// [`BestEffortRecord`]: checks `MAX_BYTES` / `MAX_UNITS`, strips §4 wrappers
/// up to `MAX_WRAP`, recursively lexes modeled `sh`/`bash`/`dash -c` scripts
/// up to `MAX_DEPTH`, flags command-word globs, and tracks earlier `HOME`
/// modifications.
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
            Err(PartOutcome::Exhausted(reason)) => {
                parsed.outcomes.push(PartResult {
                    part: PartId::WholeCall,
                    outcome: PartOutcome::Exhausted(reason),
                });
            }
            Err(other) => {
                parsed.outcomes.push(PartResult {
                    part: PartId::WholeCall,
                    outcome: other,
                });
            }
        }
    }

    Ok(parsed)
}

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
    let lex_out = lex_exact(text, depth, parent_unit, budget)?;

    // Track per-local-unit mapping to global `UnitIndex` values so lex outcomes
    // are attached to the exact final unit indices.
    let base_idx = parsed.units.len();
    if base_idx + lex_out.units.len() > limits::MAX_UNITS {
        return Err(PartOutcome::TooLarge(Cap::Units));
    }

    let local_pipe_offset = *next_pipe_offset;
    let mut max_local_pipe = 0u32;
    for raw in &lex_out.units {
        if let Some(PipelineId(id)) = raw.pipeline_id {
            max_local_pipe = max_local_pipe.max(id);
        }
    }
    *next_pipe_offset = next_pipe_offset.saturating_add(max_local_pipe);

    // Pre-assign global indices for this level's raw units (nested `-c` units
    // are appended after this level's units, keeping their `parent_unit`).
    let mut local_to_global: Vec<UnitIndex> = Vec::with_capacity(lex_out.units.len());
    for i in 0..lex_out.units.len() {
        local_to_global.push(UnitIndex(base_idx + i));
    }

    // Remap outcomes emitted by `lex_exact`.
    for pr in lex_out.outcomes {
        let mapped_part = match pr.part {
            PartId::Unit(UnitIndex(local)) => local_to_global
                .get(local)
                .copied()
                .map_or(PartId::WholeCall, PartId::Unit),
            PartId::WholeCall => parent_unit.map_or(PartId::WholeCall, PartId::Unit),
            other => other,
        };
        parsed.outcomes.push(PartResult {
            part: mapped_part,
            outcome: pr.outcome,
        });
    }

    // If `lex_exact` already exhausted the budget, move all raw units produced
    // before exhaustion into `parsed.units` without allocating so earlier hits
    // are never lost (§8).
    if budget.is_exhausted() {
        for (i, raw) in lex_out.units.into_iter().enumerate() {
            let global_idx = local_to_global[i];
            let pipeline_id = raw
                .pipeline_id
                .map(|PipelineId(id)| PipelineId(local_pipe_offset + id));
            parsed.units.push(CommandUnit {
                index: global_idx,
                argv: raw.argv,
                removed_wrappers: Vec::new(),
                redirects: raw.redirects,
                separator_before: raw.separator_before,
                pipeline_id,
                depth: raw.depth,
                parent_unit: raw.parent_unit,
            });
        }
        return Ok(());
    }

    let mut home_modified = inherited_home_modified;
    let mut nested_scripts: Vec<(UnitIndex, String, bool)> = Vec::new();
    let mut raw_iter = lex_out.units.into_iter().enumerate();

    let flush_remaining_raw =
        |raw_iter: &mut std::iter::Enumerate<std::vec::IntoIter<RawCommandUnit>>,
         parsed: &mut ParsedCommandUnits| {
            for (j, rem) in raw_iter.by_ref() {
                let g_idx = local_to_global[j];
                let pipeline_id = rem
                    .pipeline_id
                    .map(|PipelineId(id)| PipelineId(local_pipe_offset + id));
                parsed.units.push(CommandUnit {
                    index: g_idx,
                    argv: rem.argv,
                    removed_wrappers: Vec::new(),
                    redirects: rem.redirects,
                    separator_before: rem.separator_before,
                    pipeline_id,
                    depth: rem.depth,
                    parent_unit: rem.parent_unit,
                });
            }
        };

    while let Some((i, mut raw)) = raw_iter.next() {
        let global_idx = local_to_global[i];
        raw.index = global_idx;
        raw.pipeline_id = raw
            .pipeline_id
            .map(|PipelineId(id)| PipelineId(local_pipe_offset + id));

        // Detect leading `HOME=...` assignment before unwrapping.
        let mut unit_sets_home = has_leading_home_assignment(&raw.argv);

        let unwrapped = match unwrap_unit_preserving(raw, folds, budget) {
            Ok(u) => u,
            Err((partial_unit, e)) => {
                parsed.units.push(partial_unit);
                flush_remaining_raw(&mut raw_iter, parsed);
                parsed.outcomes.push(PartResult {
                    part: PartId::Unit(global_idx),
                    outcome: PartOutcome::Exhausted(e.reason),
                });
                return Ok(());
            }
        };

        let unit = unwrapped.unit;
        if let Some(reason) = unwrapped.unread {
            parsed.outcomes.push(PartResult {
                part: PartId::Unit(global_idx),
                outcome: PartOutcome::Unread(reason),
            });
        }

        // Check if this unit uses an unquoted `~` after an earlier unit
        // modified `HOME` (§3).
        if home_modified && unit_uses_unquoted_tilde(&unit) {
            parsed.outcomes.push(PartResult {
                part: PartId::Unit(global_idx),
                outcome: PartOutcome::Unread(UnreadReason::HomeSetEarlier),
            });
        }

        // Check if a command after leading assignments (`FOO=1 export HOME=...`,
        // `FOO=1 unset HOME`, `FOO=1 env -u HOME ...`) modifies `HOME` (§3).
        let assign_prefix = leading_assignment_count(&unit.argv);
        if assign_prefix > 0
            && let Some(cmd_after_assign) = unit.argv.get(assign_prefix)
            && !cmd_after_assign.has_unquoted_glob()
        {
            match fold_command_word(cmd_after_assign, folds, budget) {
                Ok(folded_after) => {
                    if command_modifies_home(&folded_after, &unit.argv[assign_prefix..]) {
                        unit_sets_home = true;
                    }
                }
                Err(e) => {
                    parsed.units.push(unit);
                    flush_remaining_raw(&mut raw_iter, parsed);
                    parsed.outcomes.push(PartResult {
                        part: PartId::Unit(global_idx),
                        outcome: PartOutcome::Exhausted(e.reason),
                    });
                    return Ok(());
                }
            }
        }

        // Inspect effective command word: command-word glob (§4), nested shell
        // (§4), HOME-modifying commands (§3), and optional P3 code-runner hook.
        if let Some(first) = unit.argv.first() {
            if first.has_unquoted_glob() {
                parsed.outcomes.push(PartResult {
                    part: PartId::Unit(global_idx),
                    outcome: PartOutcome::Unread(UnreadReason::CommandWordGlob),
                });
            } else {
                let folded = match fold_command_word(first, folds, budget) {
                    Ok(f) => f,
                    Err(e) => {
                        parsed.units.push(unit);
                        flush_remaining_raw(&mut raw_iter, parsed);
                        parsed.outcomes.push(PartResult {
                            part: PartId::Unit(global_idx),
                            outcome: PartOutcome::Exhausted(e.reason),
                        });
                        return Ok(());
                    }
                };

                if command_modifies_home(&folded, &unit.argv) {
                    unit_sets_home = true;
                }

                // Check modeled and outside-model nested shells (§4).
                if let Some(spec) = tools::lookup(&folded) {
                    if spec.nested_shell {
                        match classify_nested_shell(&unit, &folded, spec, budget) {
                            Ok(NestedShellOutcome::ModeledScript(script)) => {
                                if unit.depth >= limits::MAX_DEPTH {
                                    parsed.outcomes.push(PartResult {
                                        part: PartId::Unit(global_idx),
                                        outcome: PartOutcome::Unread(
                                            UnreadReason::MaxNestingDepthReached,
                                        ),
                                    });
                                } else {
                                    nested_scripts.push((global_idx, script, home_modified));
                                }
                            }
                            Ok(NestedShellOutcome::Unread(reason)) => {
                                parsed.outcomes.push(PartResult {
                                    part: PartId::Unit(global_idx),
                                    outcome: PartOutcome::Unread(reason),
                                });
                            }
                            Err(e) => {
                                parsed.units.push(unit);
                                flush_remaining_raw(&mut raw_iter, parsed);
                                parsed.outcomes.push(PartResult {
                                    part: PartId::Unit(global_idx),
                                    outcome: PartOutcome::Exhausted(e.reason),
                                });
                                return Ok(());
                            }
                        }
                    }
                } else if let Some(out_entry) = tools::lookup_outside_model(&folded)
                    && out_entry.contract_section == "§4"
                    && !out_entry.reason.starts_with("Wrapper")
                {
                    // Shell outside §4's sh/bash/dash subset (`zsh`, `ksh`, `fish`, …).
                    parsed.outcomes.push(PartResult {
                        part: PartId::Unit(global_idx),
                        outcome: PartOutcome::Unread(UnreadReason::UnsupportedNestedShellForm {
                            shell: folded.clone(),
                        }),
                    });
                }

                // Invoke optional P3 code-runner hook.
                if let Some(hook) = code_runner_hook.as_deref_mut() {
                    match hook(&unit, &folded, budget) {
                        Ok(reasons) => {
                            for r in reasons {
                                parsed.outcomes.push(PartResult {
                                    part: PartId::Unit(global_idx),
                                    outcome: PartOutcome::Unread(r),
                                });
                            }
                        }
                        Err(e) => {
                            parsed.units.push(unit);
                            flush_remaining_raw(&mut raw_iter, parsed);
                            parsed.outcomes.push(PartResult {
                                part: PartId::Unit(global_idx),
                                outcome: PartOutcome::Exhausted(e.reason),
                            });
                            return Ok(());
                        }
                    }
                }
            }
        }

        if unit_sets_home {
            home_modified = true;
        }
        parsed.units.push(unit);
    }

    // Recursively lex modeled `sh`/`bash`/`dash -c SCRIPT` payloads.
    for (parent_idx, script, child_home_modified) in nested_scripts {
        if budget.is_exhausted() {
            break;
        }
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
    }

    Ok(())
}

/// Strips §4's closed list of transparent wrappers (`command`, `exec`, `nice`,
/// `timeout`, `time`, `env` in their exact modeled forms) from one raw unit,
/// bounded by `limits::MAX_WRAP`.
///
/// **Owned by:** P1 · **Contract:** §4, §8.
pub(crate) fn unwrap_unit(
    raw: RawCommandUnit,
    folds: &FoldRules,
    budget: &mut ComputeBudget,
) -> Result<UnwrapResult, Exhausted> {
    unwrap_unit_preserving(raw, folds, budget).map_err(|(_, e)| e)
}

/// Internal helper that preserves the partially-unwrapped [`CommandUnit`] on
/// budget exhaustion so `build_units` never drops a unit found before running
/// out of budget (§8).
fn unwrap_unit_preserving(
    raw: RawCommandUnit,
    folds: &FoldRules,
    budget: &mut ComputeBudget,
) -> Result<UnwrapResult, (CommandUnit, Exhausted)> {
    let mut argv = raw.argv;
    let mut removed_wrappers: Vec<RemovedWrapper> = Vec::new();
    let mut unread: Option<UnreadReason> = None;

    let finish_err =
        |argv: Vec<Word>, removed_wrappers: Vec<RemovedWrapper>, redirects, e: Exhausted| {
            (
                CommandUnit {
                    index: raw.index,
                    argv,
                    removed_wrappers,
                    redirects,
                    separator_before: raw.separator_before,
                    pipeline_id: raw.pipeline_id,
                    depth: raw.depth,
                    parent_unit: raw.parent_unit,
                },
                e,
            )
        };

    if let Err(e) = budget.charge(1) {
        return Err(finish_err(argv, removed_wrappers, raw.redirects, e));
    }

    while let Some(first) = argv.first() {
        // Never treat a globbed command word as a wrapper (§4).
        if first.has_unquoted_glob() {
            break;
        }

        let folded = match fold_command_word(first, folds, budget) {
            Ok(f) => f,
            Err(e) => return Err(finish_err(argv, removed_wrappers, raw.redirects, e)),
        };
        let Some(spec) = tools::lookup(&folded) else {
            // Check if `folded` is an explicit outside-model wrapper in P0's table.
            if let Some(out) = tools::lookup_outside_model(&folded)
                && out.contract_section == "§4"
                && out.reason.starts_with("Wrapper")
            {
                unread = Some(if removed_wrappers.len() >= limits::MAX_WRAP {
                    UnreadReason::MaxWrapReached
                } else {
                    UnreadReason::UnsupportedWrapperForm { wrapper: folded }
                });
            }
            break;
        };

        let Some(kind) = spec.wrapper else {
            break;
        };

        if removed_wrappers.len() >= limits::MAX_WRAP {
            unread = Some(UnreadReason::MaxWrapReached);
            break;
        }

        match parse_wrapper_prefix(kind, spec, &argv, budget) {
            Ok(Some(strip_count)) => {
                if let Err(e) = budget.charge(strip_count as u64).and_then(|()| {
                    budget.charge_alloc(
                        std::mem::size_of::<RemovedWrapper>()
                            + strip_count * std::mem::size_of::<Word>(),
                    )
                }) {
                    return Err(finish_err(argv, removed_wrappers, raw.redirects, e));
                }
                let words: Vec<Word> = argv.drain(..strip_count).collect();
                removed_wrappers.push(RemovedWrapper { kind, words });
            }
            Ok(None) => {
                unread = Some(UnreadReason::UnsupportedWrapperForm { wrapper: folded });
                break;
            }
            Err(e) => return Err(finish_err(argv, removed_wrappers, raw.redirects, e)),
        }
    }

    Ok(UnwrapResult {
        unit: CommandUnit {
            index: raw.index,
            argv,
            removed_wrappers,
            redirects: raw.redirects,
            separator_before: raw.separator_before,
            pipeline_id: raw.pipeline_id,
            depth: raw.depth,
            parent_unit: raw.parent_unit,
        },
        unread,
    })
}

/// Validates a single §4 wrapper at the front of `argv` using `spec.flags`
/// from P0's `tools.rs`. Returns `Ok(Some(word_count_to_strip))` when the
/// invocation matches §4's closed form, or `Ok(None)` for any near miss.
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
            if nb.starts_with(b"-") || nb.contains(&b'=') {
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
            let nb = next.literal_bytes();
            if !nb.starts_with(b"-") {
                return Ok(Some(1));
            }
            let Some(tok) = next.as_str() else {
                return Ok(None);
            };
            let Some(fspec) = tools::flag(spec.flags, tok) else {
                return Ok(None);
            };
            if fspec.short != Some('n') {
                return Ok(None);
            }
            let (Some(n_word), Some(cmd_word)) = (argv.get(2), argv.get(3)) else {
                return Ok(None);
            };
            if !is_signed_integer(n_word) || cmd_word.literal_bytes().starts_with(b"-") {
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
                let wb = w.literal_bytes();
                if !wb.starts_with(b"-") {
                    break;
                }
                let Some(tok) = w.as_str() else {
                    return Ok(None);
                };
                let Some(fspec) = tools::flag(spec.flags, tok) else {
                    return Ok(None);
                };
                match fspec.short {
                    Some('s') if !seen_s => {
                        let Some(sig) = argv.get(j + 1) else {
                            return Ok(None);
                        };
                        if !is_timeout_signal(sig) {
                            return Ok(None);
                        }
                        seen_s = true;
                        j += 2;
                    }
                    Some('k') if !seen_k => {
                        let Some(dur) = argv.get(j + 1) else {
                            return Ok(None);
                        };
                        if !is_timeout_duration(dur) {
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
            if !is_timeout_duration(dur) || cmd.literal_bytes().starts_with(b"-") {
                return Ok(None);
            }
            Ok(Some(j + 1))
        }
    }
}

fn is_signed_integer(word: &Word) -> bool {
    if word.has_unquoted_glob() || word.starts_with_unquoted_tilde() {
        return false;
    }
    let b = word.literal_bytes();
    let digits = match b.first() {
        Some(b'+' | b'-') => &b[1..],
        _ => b,
    };
    !digits.is_empty() && digits.iter().all(u8::is_ascii_digit)
}

fn is_timeout_signal(word: &Word) -> bool {
    if word.has_unquoted_glob() || word.starts_with_unquoted_tilde() {
        return false;
    }
    let b = word.literal_bytes();
    if b.is_empty() || b.starts_with(b"-") {
        return false;
    }
    b.iter()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'+' | b'-'))
}

fn is_timeout_duration(word: &Word) -> bool {
    if word.has_unquoted_glob() || word.starts_with_unquoted_tilde() {
        return false;
    }
    let b = word.literal_bytes();
    if b.is_empty() || b.starts_with(b"-") {
        return false;
    }
    let num = match b.last() {
        Some(b's' | b'm' | b'h' | b'd') => &b[..b.len() - 1],
        _ => b,
    };
    if num.is_empty() {
        return false;
    }
    let mut dots = 0usize;
    let mut digits = 0usize;
    for &c in num {
        if c == b'.' {
            dots += 1;
            if dots > 1 {
                return false;
            }
        } else if c.is_ascii_digit() {
            digits += 1;
        } else {
            return false;
        }
    }
    digits > 0
}

// ---------------------------------------------------------------------------
// Nested-shell classification (§4)
// ---------------------------------------------------------------------------

enum NestedShellOutcome {
    ModeledScript(String),
    Unread(UnreadReason),
}

fn classify_nested_shell(
    unit: &CommandUnit,
    folded_shell: &str,
    spec: &'static ToolSpec,
    budget: &mut ComputeBudget,
) -> Result<NestedShellOutcome, Exhausted> {
    let argv = &unit.argv;
    let Some(first_arg) = argv.get(1) else {
        // Stdin shell (`sh`, `bash`, `dash`).
        return Ok(NestedShellOutcome::Unread(
            UnreadReason::UnsupportedNestedShellForm {
                shell: folded_shell.to_string(),
            },
        ));
    };

    let fb = first_arg.literal_bytes();

    // Script file: `sh x.sh [args...]` -> `Unread (program)` (§4, §7).
    if !fb.is_empty() && !fb.starts_with(b"-") && !fb.starts_with(b"+") {
        if !first_arg.has_unquoted_glob()
            && let (Some(cmd_s), Some(script_s)) = (argv[0].as_str(), first_arg.as_str())
        {
            let alloc = cmd_s.len() + script_s.len() + 2 * std::mem::size_of::<String>();
            budget.charge(alloc as u64)?;
            budget.charge_alloc(alloc)?;
            return Ok(NestedShellOutcome::Unread(UnreadReason::Program {
                program_words: vec![cmd_s.to_string(), script_s.to_string()],
            }));
        }
        return Ok(NestedShellOutcome::Unread(
            UnreadReason::UnsupportedNestedShellForm {
                shell: folded_shell.to_string(),
            },
        ));
    }

    // Modeled `-c` form: `NAME [-e|-u|-x]... -c SCRIPT` (nothing after SCRIPT).
    let mut j = 1usize;
    let mut saw_c = false;
    while let Some(w) = argv.get(j) {
        budget.charge(1)?;
        let wb = w.literal_bytes();
        if !wb.starts_with(b"-") || wb == b"-" || wb.starts_with(b"--") {
            return Ok(NestedShellOutcome::Unread(
                UnreadReason::UnsupportedNestedShellForm {
                    shell: folded_shell.to_string(),
                },
            ));
        }
        let letters = &wb[1..];
        if letters.is_empty() {
            return Ok(NestedShellOutcome::Unread(
                UnreadReason::UnsupportedNestedShellForm {
                    shell: folded_shell.to_string(),
                },
            ));
        }
        for (k, &lb) in letters.iter().enumerate() {
            if !lb.is_ascii() {
                return Ok(NestedShellOutcome::Unread(
                    UnreadReason::UnsupportedNestedShellForm {
                        shell: folded_shell.to_string(),
                    },
                ));
            }
            let Some(fspec) = tools::flag_short(spec.flags, lb as char) else {
                return Ok(NestedShellOutcome::Unread(
                    UnreadReason::UnsupportedNestedShellForm {
                        shell: folded_shell.to_string(),
                    },
                ));
            };
            match fspec.kind {
                FlagKind::Bool => {}
                FlagKind::Value(ValueRole::Code) => {
                    // `-c` must be the final flag letter (`-ec` ok; `-ce` rejected).
                    if k + 1 != letters.len() {
                        return Ok(NestedShellOutcome::Unread(
                            UnreadReason::UnsupportedNestedShellForm {
                                shell: folded_shell.to_string(),
                            },
                        ));
                    }
                    saw_c = true;
                }
                _ => {
                    return Ok(NestedShellOutcome::Unread(
                        UnreadReason::UnsupportedNestedShellForm {
                            shell: folded_shell.to_string(),
                        },
                    ));
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
        return Ok(NestedShellOutcome::Unread(
            UnreadReason::UnsupportedNestedShellForm {
                shell: folded_shell.to_string(),
            },
        ));
    }

    let script_word = &argv[j];
    if script_word.has_unquoted_glob() || script_word.starts_with_unquoted_tilde() {
        return Ok(NestedShellOutcome::Unread(
            UnreadReason::UnsupportedNestedShellForm {
                shell: folded_shell.to_string(),
            },
        ));
    }
    let Some(script) = script_word.as_str() else {
        return Ok(NestedShellOutcome::Unread(
            UnreadReason::UnsupportedNestedShellForm {
                shell: folded_shell.to_string(),
            },
        ));
    };

    budget.charge(script.len().max(1) as u64)?;
    budget.charge_alloc(script.len())?;
    Ok(NestedShellOutcome::ModeledScript(script.to_string()))
}

// ---------------------------------------------------------------------------
// HOME modification tracking (§3)
// ---------------------------------------------------------------------------

fn leading_assignment_count(argv: &[Word]) -> usize {
    argv.iter()
        .take_while(|w| leading_assignment_name(w).is_some())
        .count()
}

fn has_leading_home_assignment(argv: &[Word]) -> bool {
    for w in argv {
        match leading_assignment_name(w) {
            Some(b"HOME") => return true,
            Some(_) => continue,
            None => break,
        }
    }
    false
}

fn word_targets_home_var(w: &Word) -> bool {
    let b = w.literal_bytes();
    b == b"HOME" || b.starts_with(b"HOME=") || b.starts_with(b"HOME+=")
}

fn command_modifies_home(folded: &str, argv: &[Word]) -> bool {
    let args = &argv[1..];
    match folded {
        "export" | "declare" | "typeset" | "local" | "readonly" | "read" => {
            args.iter().any(word_targets_home_var)
        }
        "unset" => args.iter().any(|w| w.literal_bytes() == b"HOME"),
        "printf" => {
            // `printf -v HOME ...` or `printf -vHOME ...` sets `HOME` in bash.
            let mut i = 0usize;
            while i < args.len() {
                let b = args[i].literal_bytes();
                if b == b"--" {
                    break;
                }
                if b == b"-v"
                    && args
                        .get(i + 1)
                        .is_some_and(|w| w.literal_bytes() == b"HOME")
                {
                    return true;
                }
                if b == b"-vHOME" {
                    return true;
                }
                i += 1;
            }
            false
        }
        "env" => {
            // Bare `env` was already stripped by `unwrap_unit`; any remaining
            // `env` unit may clear (`-i`), unset (`-u HOME`), split-string
            // (`-S`), or assign `HOME=...` (including after `--`).
            let mut i = 0usize;
            let mut after_double_dash = false;
            while i < args.len() {
                let b = args[i].literal_bytes();
                if !after_double_dash && b == b"--" {
                    after_double_dash = true;
                    i += 1;
                    continue;
                }
                if b.starts_with(b"HOME=") {
                    return true;
                }
                if after_double_dash {
                    if !b.contains(&b'=') {
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
                    return true;
                }
                if (b == b"-u" || b == b"--unset")
                    && args
                        .get(i + 1)
                        .is_some_and(|w| w.literal_bytes() == b"HOME")
                {
                    return true;
                }
                if b.starts_with(b"-") && !b.starts_with(b"--") {
                    let rest = &b[1..];
                    if rest.contains(&b'i') || rest.contains(&b'S') {
                        return true;
                    }
                    if let Some(pos) = rest.iter().position(|&c| c == b'u') {
                        let tail = &rest[pos + 1..];
                        if tail == b"HOME"
                            || (tail.is_empty()
                                && args
                                    .get(i + 1)
                                    .is_some_and(|w| w.literal_bytes() == b"HOME"))
                        {
                            return true;
                        }
                    }
                }
                i += 1;
            }
            false
        }
        _ => false,
    }
}

fn unit_uses_unquoted_tilde(unit: &CommandUnit) -> bool {
    unit.original_argv().any(Word::starts_with_unquoted_tilde)
        || unit
            .redirects
            .iter()
            .any(|r| r.target().is_some_and(Word::starts_with_unquoted_tilde))
}
