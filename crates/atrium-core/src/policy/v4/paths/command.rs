//! Command-line target resolution (`resolve_command_targets`) and file-tool /
//! search-tool subject path resolution (`resolve_subject_paths`) (contract §5, §8).

use std::collections::HashMap;

use crate::policy::limits;
use crate::policy::model::{
    AccessMode, ByteProvenance, Cap, CodeExecution, CommandUnit, ComputeBudget,
    DerivedDestinations, Exhausted, FileType, FilterDialect, FsCaps, Hit, HitMatch, HitSource,
    ModelRedirect, OperandRole, PartId, PartOutcome, PartResult, PipelineId, ProtectedSet,
    ResolveError, ResolvedCallPaths, SearchFilter, Target, TargetOrigin, TargetResolution,
    ToolEffects, UnitIndex, UnreadReason, ValueRole, Word,
};
use crate::policy::v4::paths::PathResolver;
use crate::policy::v4::paths::glob::expand_shell_glob_impl;
use crate::policy::v4::paths::recursive::{check_filter_on_root, check_recursive_root_impl};
use crate::policy::v4::paths::resolve::{join_bytes, resolve_literal_impl, resolve_word_literal};
use crate::policy::v4::paths::sensitive::{
    SharedSensitive, check_target_impl, fold_path_listed, origin_to_part, push_charged, under_bytes,
};

#[derive(Debug, Clone)]
enum CwdState {
    Known(Vec<u8>),
    Unknown,
}

/// Checks `target` against `protected` and records the outcome. Write-only
/// identities come from `protected.write_only_hard_deny_identities`, which
/// `build_protected_set` fills: there is one identity collection, so the
/// resolver-backed flows and a standalone `check_target` always agree (§5,
/// D-142).
fn push_checked(
    target: &Target,
    protected: &ProtectedSet,
    budget: &mut ComputeBudget,
    outcomes: &mut Vec<PartResult>,
) -> Result<(), Exhausted> {
    let checked = check_target_impl(target, protected, budget)?;
    push_charged(outcomes, checked, budget)?;
    Ok(())
}

fn finish_call_exhausted(
    mut out: ResolvedCallPaths,
    part: PartId,
    e: Exhausted,
) -> Result<ResolvedCallPaths, Exhausted> {
    if out.outcomes.is_empty() && !out.multiply_linked_file {
        return Err(e);
    }
    if !out
        .outcomes
        .iter()
        .any(|r| matches!(r.outcome, PartOutcome::Exhausted(_)))
    {
        // Recording the exhaustion itself cannot be charged (the budget is
        // spent): it is one bounded record.
        out.outcomes.push(PartResult {
            part,
            outcome: PartOutcome::Exhausted(e.reason),
        });
    }
    Ok(out)
}

fn ensure_exhausted_recorded(out: &mut ResolvedCallPaths, part: PartId, budget: &ComputeBudget) {
    if let Some(reason) = budget.exhausted_reason()
        && !out
            .outcomes
            .iter()
            .any(|r| matches!(r.outcome, PartOutcome::Exhausted(_)))
    {
        // Recording the exhaustion itself cannot be charged (the budget is
        // spent): it is one bounded record.
        out.outcomes.push(PartResult {
            part,
            outcome: PartOutcome::Exhausted(reason),
        });
    }
}

fn unit_is_possible_writer(unit: &CommandUnit, eff: Option<&ToolEffects>) -> bool {
    if unit
        .redirects
        .iter()
        .any(ModelRedirect::writes_outside_dev_null)
    {
        return true;
    }
    match eff {
        None => true,
        Some(e) => e.writes || e.tool_name.is_none() || !matches!(e.code, CodeExecution::None),
    }
}

fn is_path_role(role: OperandRole) -> bool {
    matches!(
        role,
        OperandRole::Path
            | OperandRole::DestinationRoot
            | OperandRole::ProgramFile
            | OperandRole::FlagValue(
                ValueRole::Path | ValueRole::DestinationRoot | ValueRole::ProgramFile
            )
    )
}

fn is_recursive_root_role(role: OperandRole) -> bool {
    matches!(
        role,
        OperandRole::Path | OperandRole::FlagValue(ValueRole::Path)
    )
}

fn is_absolute_or_tilde(word: &Word) -> bool {
    word.literal_bytes().starts_with(b"/") || word.starts_with_unquoted_tilde()
}

fn record_multiply_linked(target: &Target, flag: &mut bool) {
    if let TargetResolution::Resolved {
        multiply_linked: true,
        ..
    } = &target.resolution
    {
        *flag = true;
    }
}

fn compute_subtree_writer(
    pos: usize,
    units: &[CommandUnit],
    effects: &[ToolEffects],
    children_by_parent: &HashMap<Option<UnitIndex>, Vec<usize>>,
    memo: &mut [Option<Option<UnitIndex>>],
    visiting: &mut [bool],
) -> Option<UnitIndex> {
    if let Some(cached) = memo[pos] {
        return cached;
    }
    if visiting[pos] {
        return None;
    }
    visiting[pos] = true;
    let unit = &units[pos];
    let mut writer = if unit_is_possible_writer(unit, effects.get(pos)) {
        Some(unit.index)
    } else {
        None
    };
    if writer.is_none()
        && let Some(children) = children_by_parent.get(&Some(unit.index))
    {
        for &child_pos in children {
            if let Some(w) = compute_subtree_writer(
                child_pos,
                units,
                effects,
                children_by_parent,
                memo,
                visiting,
            ) {
                writer = Some(w);
                break;
            }
        }
    }
    visiting[pos] = false;
    memo[pos] = Some(writer);
    writer
}

struct ExecCtx<'a> {
    units: &'a [CommandUnit],
    effects: &'a [ToolEffects],
    start_cwd: &'a [u8],
    protected: &'a ProtectedSet,
    resolver: &'a dyn PathResolver,
    caps: &'a mut FsCaps,
    budget: &'a mut ComputeBudget,
    children_by_parent: &'a HashMap<Option<UnitIndex>, Vec<usize>>,
    pipeline_writers: &'a HashMap<PipelineId, UnitIndex>,
    subtree_writers: &'a [Option<UnitIndex>],
    visited: &'a mut [bool],
    out: &'a mut ResolvedCallPaths,
    stopped_too_large: bool,
    exhausted_err: Option<(PartId, Exhausted)>,
}

/// Executes a sibling sequence of command units with its own lexically-scoped
/// `cwd_state` (so nested-shell `cd` never leaks back into the parent scope,
/// while child units inherit the exact cwd their parent unit had when it ran,
/// and child writers still propagate into `earlier_writer`; §5).
fn exec_unit_sequence(
    indices: &[usize],
    initial_cwd: CwdState,
    inherited_pipeline_taint: Option<UnitIndex>,
    earlier_writer: &mut Option<UnitIndex>,
    ctx: &mut ExecCtx<'_>,
) {
    let mut cwd_state = initial_cwd;
    let mut active_pipeline: Option<PipelineId> = None;
    let mut active_pipeline_writer: Option<UnitIndex> = None;

    for &idx in indices {
        if ctx.stopped_too_large || ctx.exhausted_err.is_some() {
            return;
        }
        if ctx.visited[idx] {
            continue;
        }
        ctx.visited[idx] = true;

        let unit = &ctx.units[idx];
        let eff = ctx.effects.get(idx);

        // If we just left a pipeline that had a writer, promote it to `earlier_writer`.
        if unit.pipeline_id != active_pipeline {
            if let Some(pw) = active_pipeline_writer.take() {
                earlier_writer.get_or_insert(pw);
            }
            active_pipeline = unit.pipeline_id;
        }

        let own_pipeline_taint = unit
            .pipeline_id
            .and_then(|pid| ctx.pipeline_writers.get(&pid).copied());
        let pipeline_taint = own_pipeline_taint.or(inherited_pipeline_taint);
        let after_writer_taint = if pipeline_taint.is_some() {
            None
        } else {
            *earlier_writer
        };

        let mut unit_target_parts: Vec<PartId> = Vec::new();
        let mut saw_explicit_recursive_root = false;
        let mut mv_source_is_dir = false;

        if matches!(cwd_state, CwdState::Unknown)
            && let Err(err) = push_charged(
                &mut ctx.out.outcomes,
                PartResult {
                    part: PartId::Unit(unit.index),
                    outcome: PartOutcome::Unread(UnreadReason::UnknownCwd),
                },
                ctx.budget,
            )
        {
            ctx.exhausted_err = Some((PartId::WholeCall, err));
            return;
        }

        // Resolve operands according to ToolEffects roles & access modes.
        for (argv_index, word) in unit.argv.iter().enumerate() {
            let (role, access) = match eff {
                Some(e) => {
                    let r = e
                        .roles
                        .get(argv_index)
                        .copied()
                        .unwrap_or(if argv_index == 0 {
                            OperandRole::Command
                        } else {
                            OperandRole::Path
                        });
                    let a = e
                        .operand_access
                        .get(argv_index)
                        .copied()
                        .unwrap_or(AccessMode::ReadWrite);
                    (r, a)
                }
                None => (
                    if argv_index == 0 {
                        OperandRole::Command
                    } else {
                        OperandRole::Path
                    },
                    AccessMode::ReadWrite,
                ),
            };

            if !is_path_role(role) {
                continue;
            }

            if ctx.out.targets.len() >= limits::MAX_PATHS {
                if let Err(err) = push_charged(
                    &mut ctx.out.outcomes,
                    PartResult {
                        part: PartId::WholeCall,
                        outcome: PartOutcome::TooLarge(Cap::Paths),
                    },
                    ctx.budget,
                ) {
                    ctx.exhausted_err = Some((PartId::WholeCall, err));
                    return;
                }
                ctx.stopped_too_large = true;
                return;
            }

            let part = PartId::Operand {
                unit: unit.index,
                argv_index,
            };
            let origin = TargetOrigin::Operand {
                unit: unit.index,
                argv_index,
            };
            unit_target_parts.push(part.clone());

            let cur_cwd: &[u8] = match &cwd_state {
                CwdState::Known(c) => c.as_slice(),
                CwdState::Unknown => {
                    if !is_absolute_or_tilde(word) {
                        let unread_t = Target {
                            origin,
                            access,
                            resolution: TargetResolution::Unread(UnreadReason::UnknownCwd),
                        };
                        if let Err(err) = push_charged(
                            &mut ctx.out.outcomes,
                            PartResult {
                                part,
                                outcome: PartOutcome::Unread(UnreadReason::UnknownCwd),
                            },
                            ctx.budget,
                        ) {
                            ctx.exhausted_err = Some((PartId::WholeCall, err));
                            return;
                        }
                        ctx.out.targets.push(unread_t);
                        continue;
                    }
                    ctx.start_cwd
                }
            };

            let is_rec_root =
                eff.is_some_and(|e| e.reads_recursively) && is_recursive_root_role(role);
            if is_rec_root {
                saw_explicit_recursive_root = true;
            }

            if word.has_unquoted_glob() {
                let exp = match expand_shell_glob_impl(
                    word,
                    cur_cwd,
                    &ctx.protected.home,
                    part.clone(),
                    access,
                    ctx.resolver,
                    ctx.caps,
                    ctx.budget,
                ) {
                    Ok(exp) => exp,
                    Err(e) => {
                        ctx.exhausted_err = Some((part, e));
                        return;
                    }
                };
                for unread in exp.unread {
                    if let Err(err) = push_charged(
                        &mut ctx.out.outcomes,
                        PartResult {
                            part: part.clone(),
                            outcome: PartOutcome::Unread(unread),
                        },
                        ctx.budget,
                    ) {
                        ctx.exhausted_err = Some((PartId::WholeCall, err));
                        return;
                    }
                }
                for target in exp.exact_matches {
                    if ctx.out.targets.len() >= limits::MAX_PATHS {
                        if let Err(err) = push_charged(
                            &mut ctx.out.outcomes,
                            PartResult {
                                part: PartId::WholeCall,
                                outcome: PartOutcome::TooLarge(Cap::Paths),
                            },
                            ctx.budget,
                        ) {
                            ctx.exhausted_err = Some((PartId::WholeCall, err));
                            return;
                        }
                        ctx.stopped_too_large = true;
                        return;
                    }
                    record_multiply_linked(&target, &mut ctx.out.multiply_linked_file);
                    if is_rec_root && let Some(e) = eff {
                        match check_recursive_root_impl(
                            &target,
                            e.links,
                            &e.filters,
                            ctx.protected,
                            ctx.resolver,
                            ctx.caps,
                            ctx.budget,
                        ) {
                            Ok(rec) => {
                                ctx.out.multiply_linked_file |= rec.multiply_linked_file;
                                ctx.out.outcomes.extend(rec.outcomes);
                                if ctx.budget.is_exhausted() {
                                    ensure_exhausted_recorded(ctx.out, part.clone(), ctx.budget);
                                    ctx.out.targets.push(target);
                                    return;
                                }
                            }
                            Err(err) => {
                                ctx.exhausted_err = Some((part, err));
                                return;
                            }
                        }
                    } else if let Err(err) =
                        push_checked(&target, ctx.protected, ctx.budget, &mut ctx.out.outcomes)
                    {
                        ctx.exhausted_err = Some((part, err));
                        return;
                    }
                    if eff.is_some_and(|e| {
                        e.derived_destinations == DerivedDestinations::WhenSourceIsDir
                    }) && role == OperandRole::Path
                    {
                        match source_is_dir_or_unknown(&target, ctx.resolver, ctx.caps, ctx.budget)
                        {
                            Ok(is_dir) => {
                                if is_dir {
                                    mv_source_is_dir = true;
                                }
                            }
                            Err(err) => {
                                ctx.exhausted_err = Some((part, err));
                                return;
                            }
                        }
                    }
                    ctx.out.targets.push(target);
                }
                let mut emitted_variant = false;
                for target in exp.variant_only_matches {
                    if ctx.out.targets.len() >= limits::MAX_PATHS {
                        if let Err(err) = push_charged(
                            &mut ctx.out.outcomes,
                            PartResult {
                                part: PartId::WholeCall,
                                outcome: PartOutcome::TooLarge(Cap::Paths),
                            },
                            ctx.budget,
                        ) {
                            ctx.exhausted_err = Some((PartId::WholeCall, err));
                            return;
                        }
                        ctx.stopped_too_large = true;
                        return;
                    }
                    if !emitted_variant {
                        emitted_variant = true;
                        if let Err(err) = push_charged(
                            &mut ctx.out.outcomes,
                            PartResult {
                                part: part.clone(),
                                outcome: PartOutcome::Unread(UnreadReason::VariantOnlyGlobMatch {
                                    pattern: word.literal_bytes().to_vec(),
                                }),
                            },
                            ctx.budget,
                        ) {
                            ctx.exhausted_err = Some((PartId::WholeCall, err));
                            return;
                        }
                    }
                    ctx.out.targets.push(target);
                }
            } else {
                let target = match resolve_word_literal(
                    word,
                    cur_cwd,
                    &ctx.protected.home,
                    origin,
                    access,
                    ctx.resolver,
                    ctx.caps,
                    ctx.budget,
                ) {
                    Ok(t) => t,
                    Err(e) => {
                        ctx.exhausted_err = Some((part, e));
                        return;
                    }
                };
                record_multiply_linked(&target, &mut ctx.out.multiply_linked_file);
                if is_rec_root && let Some(e) = eff {
                    match check_recursive_root_impl(
                        &target,
                        e.links,
                        &e.filters,
                        ctx.protected,
                        ctx.resolver,
                        ctx.caps,
                        ctx.budget,
                    ) {
                        Ok(rec) => {
                            ctx.out.multiply_linked_file |= rec.multiply_linked_file;
                            ctx.out.outcomes.extend(rec.outcomes);
                            if ctx.budget.is_exhausted() {
                                ensure_exhausted_recorded(ctx.out, part.clone(), ctx.budget);
                                ctx.out.targets.push(target);
                                return;
                            }
                        }
                        Err(err) => {
                            ctx.exhausted_err = Some((part, err));
                            return;
                        }
                    }
                } else if let Err(err) =
                    push_checked(&target, ctx.protected, ctx.budget, &mut ctx.out.outcomes)
                {
                    ctx.exhausted_err = Some((part, err));
                    return;
                }
                if eff
                    .is_some_and(|e| e.derived_destinations == DerivedDestinations::WhenSourceIsDir)
                    && role == OperandRole::Path
                {
                    match source_is_dir_or_unknown(&target, ctx.resolver, ctx.caps, ctx.budget) {
                        Ok(is_dir) => {
                            if is_dir {
                                mv_source_is_dir = true;
                            }
                        }
                        Err(err) => {
                            ctx.exhausted_err = Some((part, err));
                            return;
                        }
                    }
                }
                ctx.out.targets.push(target);
            }
        }

        // Implicit cwd root for recursive readers with no explicit path operand
        // (e.g. `rg pat`, `grep -r pat`, `find`).
        if let Some(e) = eff
            && e.reads_recursively
            && !saw_explicit_recursive_root
        {
            let implicit_part = PartId::Unit(unit.index);
            unit_target_parts.push(implicit_part.clone());
            match &cwd_state {
                CwdState::Known(cur) => {
                    let target = match resolve_literal_impl(
                        cur,
                        cur,
                        &ctx.protected.home,
                        TargetOrigin::ImplicitCwd {
                            unit: Some(unit.index),
                        },
                        AccessMode::Read,
                        ctx.resolver,
                        ctx.caps,
                        ctx.budget,
                    ) {
                        Ok(t) => t,
                        Err(err) => {
                            ctx.exhausted_err = Some((implicit_part, err));
                            return;
                        }
                    };
                    record_multiply_linked(&target, &mut ctx.out.multiply_linked_file);
                    match check_recursive_root_impl(
                        &target,
                        e.links,
                        &e.filters,
                        ctx.protected,
                        ctx.resolver,
                        ctx.caps,
                        ctx.budget,
                    ) {
                        Ok(rec) => {
                            ctx.out.multiply_linked_file |= rec.multiply_linked_file;
                            ctx.out.outcomes.extend(rec.outcomes);
                            ctx.out.targets.push(target);
                            if ctx.budget.is_exhausted() {
                                ensure_exhausted_recorded(ctx.out, implicit_part, ctx.budget);
                                return;
                            }
                        }
                        Err(err) => {
                            ctx.exhausted_err = Some((implicit_part, err));
                            return;
                        }
                    }
                }
                CwdState::Unknown => {
                    ctx.out.targets.push(Target {
                        origin: TargetOrigin::ImplicitCwd {
                            unit: Some(unit.index),
                        },
                        access: AccessMode::Read,
                        resolution: TargetResolution::Unread(UnreadReason::UnknownCwd),
                    });
                }
            }
        }

        // Redirects (§3, §5).
        for (redirect_index, redir) in unit.redirects.iter().enumerate() {
            let Some(target_word) = redir.target() else {
                continue;
            };
            if ctx.out.targets.len() >= limits::MAX_PATHS {
                if let Err(err) = push_charged(
                    &mut ctx.out.outcomes,
                    PartResult {
                        part: PartId::WholeCall,
                        outcome: PartOutcome::TooLarge(Cap::Paths),
                    },
                    ctx.budget,
                ) {
                    ctx.exhausted_err = Some((PartId::WholeCall, err));
                    return;
                }
                ctx.stopped_too_large = true;
                return;
            }
            let access = match redir {
                ModelRedirect::Read { .. } => AccessMode::Read,
                ModelRedirect::Write { .. }
                | ModelRedirect::Append { .. }
                | ModelRedirect::ErrWrite { .. }
                | ModelRedirect::ErrAppend { .. } => AccessMode::Write,
                ModelRedirect::Dup { .. } | ModelRedirect::Close { .. } => continue,
            };
            let part = PartId::Redirect {
                unit: unit.index,
                redirect_index,
            };
            let origin = TargetOrigin::Redirect {
                unit: unit.index,
                redirect_index,
            };
            unit_target_parts.push(part.clone());

            let cur_cwd: &[u8] = match &cwd_state {
                CwdState::Known(c) => c.as_slice(),
                CwdState::Unknown => {
                    if !is_absolute_or_tilde(target_word) {
                        if let Err(err) = push_charged(
                            &mut ctx.out.outcomes,
                            PartResult {
                                part,
                                outcome: PartOutcome::Unread(UnreadReason::UnknownCwd),
                            },
                            ctx.budget,
                        ) {
                            ctx.exhausted_err = Some((PartId::WholeCall, err));
                            return;
                        }
                        ctx.out.targets.push(Target {
                            origin,
                            access,
                            resolution: TargetResolution::Unread(UnreadReason::UnknownCwd),
                        });
                        continue;
                    }
                    ctx.start_cwd
                }
            };

            let target = match resolve_word_literal(
                target_word,
                cur_cwd,
                &ctx.protected.home,
                origin,
                access,
                ctx.resolver,
                ctx.caps,
                ctx.budget,
            ) {
                Ok(t) => t,
                Err(err) => {
                    ctx.exhausted_err = Some((part, err));
                    return;
                }
            };
            record_multiply_linked(&target, &mut ctx.out.multiply_linked_file);
            if let Err(err) =
                push_checked(&target, ctx.protected, ctx.budget, &mut ctx.out.outcomes)
            {
                ctx.exhausted_err = Some((part, err));
                return;
            }
            ctx.out.targets.push(target);
        }

        // Derived destinations (§5, §10 row 18): makes the writer itself Unread,
        // while any destination-root hit checked above still stands.
        if let Some(e) = eff {
            let derived = match e.derived_destinations {
                DerivedDestinations::None => false,
                DerivedDestinations::Always => true,
                DerivedDestinations::WhenSourceIsDir => mv_source_is_dir,
            };
            if derived
                && let Err(err) = push_charged(
                    &mut ctx.out.outcomes,
                    PartResult {
                        part: PartId::Unit(unit.index),
                        outcome: PartOutcome::Unread(UnreadReason::DerivedDestinations {
                            tool: e.tool_name.unwrap_or("unknown").to_string(),
                        }),
                    },
                    ctx.budget,
                )
            {
                ctx.exhausted_err = Some((PartId::WholeCall, err));
                return;
            }
        }

        // Intra-call writer effects (§5, §10 row 21): targets of every unit
        // after a possible writer, and every stage of a pipeline containing a
        // possible writer, are Unread (hard-deny hits already pushed still stand).
        if let Some(writer_unit) = pipeline_taint {
            if unit_target_parts.is_empty() {
                if unit.index != writer_unit
                    && let Err(err) = push_charged(
                        &mut ctx.out.outcomes,
                        PartResult {
                            part: PartId::Unit(unit.index),
                            outcome: PartOutcome::Unread(
                                UnreadReason::PipelineWithPossibleWriter { writer_unit },
                            ),
                        },
                        ctx.budget,
                    )
                {
                    ctx.exhausted_err = Some((PartId::WholeCall, err));
                    return;
                }
            } else {
                for p in &unit_target_parts {
                    if let Err(err) = push_charged(
                        &mut ctx.out.outcomes,
                        PartResult {
                            part: p.clone(),
                            outcome: PartOutcome::Unread(
                                UnreadReason::PipelineWithPossibleWriter { writer_unit },
                            ),
                        },
                        ctx.budget,
                    ) {
                        ctx.exhausted_err = Some((PartId::WholeCall, err));
                        return;
                    }
                }
            }
        } else if let Some(writer_unit) = after_writer_taint {
            if unit_target_parts.is_empty() {
                if let Err(err) = push_charged(
                    &mut ctx.out.outcomes,
                    PartResult {
                        part: PartId::Unit(unit.index),
                        outcome: PartOutcome::Unread(UnreadReason::AfterPossibleWriter {
                            writer_unit,
                        }),
                    },
                    ctx.budget,
                ) {
                    ctx.exhausted_err = Some((PartId::WholeCall, err));
                    return;
                }
            } else {
                for p in &unit_target_parts {
                    if let Err(err) = push_charged(
                        &mut ctx.out.outcomes,
                        PartResult {
                            part: p.clone(),
                            outcome: PartOutcome::Unread(UnreadReason::AfterPossibleWriter {
                                writer_unit,
                            }),
                        },
                        ctx.budget,
                    ) {
                        ctx.exhausted_err = Some((PartId::WholeCall, err));
                        return;
                    }
                }
            }
        }

        // Execute nested-shell children at the EXACT cwd `unit` had when it ran,
        // isolating any `cd` inside the child from `unit`'s parent scope (§5).
        if let Some(children) = ctx.children_by_parent.get(&Some(unit.index)) {
            let child_entry_cwd = cwd_state.clone();
            let mut child_earlier_writer = *earlier_writer;
            exec_unit_sequence(
                children,
                child_entry_cwd,
                pipeline_taint,
                &mut child_earlier_writer,
                ctx,
            );
            if ctx.stopped_too_large || ctx.exhausted_err.is_some() || ctx.budget.is_exhausted() {
                return;
            }
            // If a child inside a non-pipelined `-c` script wrote to disk, later
            // outer units run after that writer.
            if let Some(cw) = child_earlier_writer
                && earlier_writer.is_none()
            {
                if unit.pipeline_id.is_some() {
                    active_pipeline_writer.get_or_insert(cw);
                } else {
                    *earlier_writer = Some(cw);
                }
            }
        }

        // Update `cd` state for subsequent sibling units in THIS scope only (§5).
        if let Some(e) = eff
            && e.tool_name == Some("cd")
        {
            cwd_state = if earlier_writer.is_some() {
                CwdState::Unknown
            } else {
                match next_cwd_after_cd(
                    unit,
                    e,
                    &cwd_state,
                    ctx.protected,
                    ctx.resolver,
                    ctx.caps,
                    ctx.budget,
                ) {
                    Ok(next) => next,
                    Err(err) => {
                        ctx.exhausted_err = Some((PartId::Unit(unit.index), err));
                        return;
                    }
                }
            };
        }

        // Record possible-writer status for later units.
        if let Some(w) = ctx.subtree_writers[idx] {
            if unit.pipeline_id.is_some() {
                active_pipeline_writer.get_or_insert(w);
            } else {
                earlier_writer.get_or_insert(w);
            }
        }
    }

    if let Some(pw) = active_pipeline_writer.take() {
        earlier_writer.get_or_insert(pw);
    }
}

/// Implementation of `v4::paths::resolve_command_targets` (contract §5, §8).
pub(crate) fn resolve_command_targets_impl(
    units: &[CommandUnit],
    effects: &[ToolEffects],
    start_cwd: &[u8],
    protected: &ProtectedSet,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
) -> Result<ResolvedCallPaths, Exhausted> {
    budget.charge((units.len() + start_cwd.len() + 1) as u64)?;
    budget.charge_alloc(
        start_cwd
            .len()
            .saturating_add(units.len() * std::mem::size_of::<usize>()),
    )?;

    let mut out = ResolvedCallPaths::default();

    // Build the execution tree from `parent_unit` and `depth` so nested-shell
    // children resolve at their parent's entry cwd regardless of flattening
    // order, and child `cd` changes stay isolated from the parent scope (§5).
    let mut index_to_pos: HashMap<UnitIndex, usize> = HashMap::with_capacity(units.len());
    for (pos, u) in units.iter().enumerate() {
        index_to_pos.insert(u.index, pos);
    }

    let mut children_by_parent: HashMap<Option<UnitIndex>, Vec<usize>> = HashMap::new();
    let mut orphans: Vec<usize> = Vec::new();
    for (pos, u) in units.iter().enumerate() {
        match u.parent_unit {
            None if u.depth == 0 => {
                children_by_parent.entry(None).or_default().push(pos);
            }
            Some(parent_idx) if index_to_pos.contains_key(&parent_idx) => {
                children_by_parent
                    .entry(Some(parent_idx))
                    .or_default()
                    .push(pos);
            }
            _ => {
                // Detached or depth>0 unit with missing parent: resolve fail-closed
                // under `Unknown` cwd so it is never silently dropped.
                orphans.push(pos);
            }
        }
    }

    // Compute per-unit subtree writer status bottom-up so pipelines whose stage
    // invokes a writing nested shell taint every stage of the pipeline (§5).
    let mut memo: Vec<Option<Option<UnitIndex>>> = vec![None; units.len()];
    let mut visiting: Vec<bool> = vec![false; units.len()];
    let mut subtree_writers: Vec<Option<UnitIndex>> = Vec::with_capacity(units.len());
    for pos in 0..units.len() {
        let w = compute_subtree_writer(
            pos,
            units,
            effects,
            &children_by_parent,
            &mut memo,
            &mut visiting,
        );
        subtree_writers.push(w);
    }

    // Pre-scan pipelines so every stage of a pipeline containing a possible
    // writer — including stages BEFORE the writer — is tainted (§5, §10 row 21).
    let mut pipeline_writers: HashMap<PipelineId, UnitIndex> = HashMap::new();
    for (pos, unit) in units.iter().enumerate() {
        if let Some(pid) = unit.pipeline_id
            && let Some(w) = subtree_writers[pos]
        {
            pipeline_writers.entry(pid).or_insert(w);
        }
    }

    let mut visited = vec![false; units.len()];
    let mut earlier_writer: Option<UnitIndex> = None;

    let top_level: Vec<usize> = children_by_parent.get(&None).cloned().unwrap_or_default();

    let mut ctx = ExecCtx {
        units,
        effects,
        start_cwd,
        protected,
        resolver,
        caps,
        budget,
        children_by_parent: &children_by_parent,
        pipeline_writers: &pipeline_writers,
        subtree_writers: &subtree_writers,
        visited: &mut visited,
        out: &mut out,
        stopped_too_large: false,
        exhausted_err: None,
    };

    exec_unit_sequence(
        &top_level,
        CwdState::Known(start_cwd.to_vec()),
        None,
        &mut earlier_writer,
        &mut ctx,
    );

    if !ctx.stopped_too_large && ctx.exhausted_err.is_none() && !ctx.budget.is_exhausted() {
        // Resolve any detached orphans or cycle members under Unknown cwd.
        let remaining: Vec<usize> = orphans
            .into_iter()
            .chain((0..units.len()).filter(|&i| !ctx.visited[i]))
            .collect();
        if !remaining.is_empty() {
            exec_unit_sequence(
                &remaining,
                CwdState::Unknown,
                None,
                &mut earlier_writer,
                &mut ctx,
            );
        }
    }

    let exhausted_err = ctx.exhausted_err.take();
    if let Some((part, err)) = exhausted_err {
        return finish_call_exhausted(out, part, err);
    }

    Ok(out)
}

fn source_is_dir_or_unknown(
    target: &Target,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    let TargetResolution::Resolved { canonical, .. } = &target.resolution else {
        return Ok(true);
    };
    // `mv` moves symlinks themselves rather than their target directories, so
    // `lstat` is the exact check for whether a source is a directory.
    match resolver.lstat(canonical, caps, budget) {
        Ok(md) => Ok(md.file_type == FileType::Dir),
        Err(ResolveError::NotFound) => Ok(false),
        Err(ResolveError::Io(_) | ResolveError::CapReached(_)) => Ok(true),
        Err(ResolveError::Exhausted(e)) => Err(e),
    }
}

fn next_cwd_after_cd(
    unit: &CommandUnit,
    eff: &ToolEffects,
    current: &CwdState,
    protected: &ProtectedSet,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
) -> Result<CwdState, Exhausted> {
    // `cd` inside a pipeline differs across shells (subshell vs zsh last-stage).
    if unit.pipeline_id.is_some() || !eff.unread.is_empty() {
        return Ok(CwdState::Unknown);
    }
    let CwdState::Known(cur) = current else {
        return Ok(CwdState::Unknown);
    };
    match eff.cd_target_operand {
        Some(idx) => {
            let Some(word) = unit.argv.get(idx) else {
                return Ok(CwdState::Unknown);
            };
            if word.has_unquoted_glob() || word.literal_bytes() == b"-" {
                return Ok(CwdState::Unknown);
            }
            let resolved = resolve_word_literal(
                word,
                cur,
                &protected.home,
                TargetOrigin::Operand {
                    unit: unit.index,
                    argv_index: idx,
                },
                AccessMode::Read,
                resolver,
                caps,
                budget,
            )?;
            match resolved.resolution {
                TargetResolution::Resolved { canonical, .. } => {
                    // Only move cwd when the destination is an existing directory;
                    // otherwise `cd` fails at runtime and later units run in the
                    // prior directory, so treat cwd as Unknown (fail closed).
                    match resolver.stat(&canonical, caps, budget) {
                        Ok(md) if md.file_type == FileType::Dir => Ok(CwdState::Known(canonical)),
                        Ok(_)
                        | Err(
                            ResolveError::NotFound
                            | ResolveError::Io(_)
                            | ResolveError::CapReached(_),
                        ) => Ok(CwdState::Unknown),
                        Err(ResolveError::Exhausted(e)) => Err(e),
                    }
                }
                TargetResolution::Unread(_) => Ok(CwdState::Unknown),
            }
        }
        None => {
            // Bare `cd` (or `cd -L` / `cd -P` / `cd --` with no positional operand) moves to `home`.
            let only_flags = unit.argv.len() == 1
                || (eff.roles.len() == unit.argv.len()
                    && eff.roles[1..].iter().all(|r| *r == OperandRole::Flag));
            if only_flags {
                budget.charge_alloc(protected.home.len())?;
                Ok(CwdState::Known(protected.home.clone()))
            } else {
                Ok(CwdState::Unknown)
            }
        }
    }
}

/// Lexically normalizes an absolute byte path containing `.` / `..` for
/// best-effort selector probe matching on absent targets.
fn lexical_normalize_selector(base: &[u8], glob: &str, home: &[u8]) -> Vec<u8> {
    let raw = glob.as_bytes();
    let abs = if raw == b"~" {
        home.to_vec()
    } else if let Some(rest) = raw.strip_prefix(b"~/") {
        join_bytes(home, rest)
    } else if raw.starts_with(b"/") {
        raw.to_vec()
    } else {
        join_bytes(base, raw)
    };
    let mut segs: Vec<&[u8]> = Vec::new();
    for seg in abs.split(|&b| b == b'/').filter(|s| !s.is_empty()) {
        if seg == b"." {
            continue;
        }
        if seg == b".." {
            segs.pop();
        } else {
            segs.push(seg);
        }
    }
    if segs.is_empty() {
        return b"/".to_vec();
    }
    let mut out = Vec::with_capacity(abs.len());
    for seg in segs {
        out.push(b'/');
        out.extend_from_slice(seg);
    }
    out
}

/// Best-effort probe check for selector globs (`../.aws/credentia?s`,
/// `~/.azure/msal*`) so absent protected paths still produce their probe hits
/// (§2, §5).
fn check_selector_probes(
    base: &[u8],
    glob: &str,
    protected: &ProtectedSet,
    shared: &SharedSensitive,
    budget: &mut ComputeBudget,
    outcomes: &mut Vec<PartResult>,
) -> Result<(), Exhausted> {
    let est_len = base.len().max(protected.home.len()) + glob.len() + 2;
    budget.charge(est_len as u64)?;
    budget.charge_alloc(est_len.saturating_mul(2))?;
    let norm = lexical_normalize_selector(base, glob, &protected.home);
    let folded = fold_path_listed(&norm, protected.folds, budget)?;
    let Ok(pat_str) = std::str::from_utf8(&folded) else {
        return Ok(());
    };
    budget.charge((pat_str.len() as u64).saturating_mul(8).max(8))?;
    budget.charge_alloc(pat_str.len().saturating_mul(16).max(64))?;
    let Ok(g) = globset::GlobBuilder::new(pat_str)
        .literal_separator(true)
        .build()
    else {
        return Ok(());
    };
    let matcher = g.compile_matcher();
    let filter_part = PartId::Filter { unit: None };
    for (probe_bytes, hard_id) in shared.hard_deny_probes(budget)? {
        budget.charge(probe_bytes.len().max(1) as u64)?;
        if let Ok(ps) = std::str::from_utf8(&probe_bytes)
            && matcher.is_match(ps)
        {
            push_charged(
                outcomes,
                PartResult {
                    part: filter_part.clone(),
                    outcome: PartOutcome::HardDenyHit(Hit {
                        hard_deny_id: Some(hard_id),
                        source: HitSource::BestEffort,
                        matched: HitMatch::ProbeName {
                            probe: ps.to_string(),
                        },
                        part: filter_part.clone(),
                    }),
                },
                budget,
            )?;
            break;
        }
    }
    for (probe_bytes, _family) in shared.store_probes(budget)? {
        budget.charge(probe_bytes.len().max(1) as u64)?;
        if let Ok(ps) = std::str::from_utf8(&probe_bytes)
            && matcher.is_match(ps)
        {
            push_charged(
                outcomes,
                PartResult {
                    part: filter_part.clone(),
                    outcome: PartOutcome::StoreHit(Hit {
                        hard_deny_id: None,
                        source: HitSource::Model,
                        matched: HitMatch::ProbeName {
                            probe: ps.to_string(),
                        },
                        part: filter_part.clone(),
                    }),
                },
                budget,
            )?;
            break;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// File-tool and search-tool subject resolution (`resolve_subject_paths`, §5)
// ---------------------------------------------------------------------------

fn is_selector_glob(glob: &str) -> bool {
    glob.starts_with('/') || glob.starts_with('~') || glob.split('/').any(|seg| seg == "..")
}

/// Implementation of `v4::paths::resolve_subject_paths` (contract §5, §8).
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_subject_paths_impl(
    paths: &[String],
    glob: Option<&str>,
    recursive: bool,
    access: AccessMode,
    protected: &ProtectedSet,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
) -> Result<ResolvedCallPaths, Exhausted> {
    budget.charge((paths.len() + 1) as u64)?;
    let mut out = ResolvedCallPaths::default();

    if paths.len() > limits::MAX_USES {
        if let Err(e) = push_charged(
            &mut out.outcomes,
            PartResult {
                part: PartId::WholeCall,
                outcome: PartOutcome::TooLarge(Cap::Uses),
            },
            budget,
        ) {
            return finish_call_exhausted(out, PartId::WholeCall, e);
        }
        return Ok(out);
    }

    let selector_glob = glob.filter(|g| is_selector_glob(g));
    let non_selector_filter = glob.filter(|g| !is_selector_glob(g)).map(|g| SearchFilter {
        dialect: FilterDialect::SearchGlob,
        pattern: Word::derived(g.as_bytes().to_vec()),
        case_insensitive: false,
        argv_index: None,
    });
    let filter_part = PartId::Filter { unit: None };

    // Helper: check a single resolved root immediately (so any earlier hit is
    // already stored in `out.outcomes` if a later root exhausts the budget; §8).
    let mut roots: Vec<Target> = Vec::with_capacity(paths.len().max(1));

    // File-tool paths are ALWAYS literal (M1-32 round 4 finding 3; §5; §10 row 11).
    for (path_index, raw) in paths.iter().enumerate() {
        if out.targets.len() >= limits::MAX_PATHS {
            if let Err(e) = push_charged(
                &mut out.outcomes,
                PartResult {
                    part: PartId::WholeCall,
                    outcome: PartOutcome::TooLarge(Cap::Paths),
                },
                budget,
            ) {
                return finish_call_exhausted(out, PartId::WholeCall, e);
            }
            return Ok(out);
        }
        let part = PartId::SubjectPath { path_index };
        let target = match resolve_literal_impl(
            raw.as_bytes(),
            &protected.cwd,
            &protected.home,
            TargetOrigin::SubjectPath { path_index },
            access,
            resolver,
            caps,
            budget,
        ) {
            Ok(t) => t,
            Err(e) => return finish_call_exhausted(out, part, e),
        };
        record_multiply_linked(&target, &mut out.multiply_linked_file);

        if selector_glob.is_some() {
            if let Err(e) = push_checked(&target, protected, budget, &mut out.outcomes) {
                return finish_call_exhausted(out, part, e);
            }
            roots.push(target.clone());
        } else if recursive {
            let filters_slice = non_selector_filter.as_slice();
            match check_recursive_root_impl(
                &target,
                Default::default(),
                filters_slice,
                protected,
                resolver,
                caps,
                budget,
            ) {
                Ok(rec) => {
                    out.multiply_linked_file |= rec.multiply_linked_file;
                    out.outcomes.extend(rec.outcomes);
                    out.targets.push(target);
                    if budget.is_exhausted() {
                        ensure_exhausted_recorded(&mut out, part, budget);
                        return Ok(out);
                    }
                    continue;
                }
                Err(e) => return finish_call_exhausted(out, part, e),
            }
        } else {
            if let Err(e) = push_checked(&target, protected, budget, &mut out.outcomes) {
                return finish_call_exhausted(out, part, e);
            }
            if let Some(filter) = &non_selector_filter
                && let TargetResolution::Resolved { canonical, .. } = &target.resolution
            {
                let shared = match SharedSensitive::for_protected_budgeted(protected, budget) {
                    Ok(s) => s,
                    Err(e) => return finish_call_exhausted(out, filter_part, e),
                };
                if let Err(e) = budget
                    .charge(canonical.len().max(1) as u64)
                    .and_then(|()| budget.charge_alloc(canonical.len().max(1)))
                {
                    return finish_call_exhausted(out, filter_part, e);
                }
                let folded_root = match fold_path_listed(canonical, protected.folds, budget) {
                    Ok(f) => f,
                    Err(e) => return finish_call_exhausted(out, filter_part, e),
                };
                if let Err(e) = check_filter_on_root(
                    canonical,
                    &folded_root,
                    filter,
                    false,
                    &filter_part,
                    protected,
                    &shared,
                    resolver,
                    caps,
                    budget,
                    &mut out.outcomes,
                    &mut out.multiply_linked_file,
                ) {
                    return finish_call_exhausted(out, filter_part, e);
                }
            }
        }
        out.targets.push(target);
    }

    // Implicit cwd root when `paths` is empty and a recursive or glob search runs.
    if paths.is_empty() && (recursive || glob.is_some()) {
        let part = PartId::WholeCall;
        let implicit = match resolve_literal_impl(
            &protected.cwd,
            &protected.cwd,
            &protected.home,
            TargetOrigin::ImplicitCwd { unit: None },
            access,
            resolver,
            caps,
            budget,
        ) {
            Ok(t) => t,
            Err(e) => return finish_call_exhausted(out, part, e),
        };
        record_multiply_linked(&implicit, &mut out.multiply_linked_file);

        if selector_glob.is_some() {
            if let Err(e) = push_checked(&implicit, protected, budget, &mut out.outcomes) {
                return finish_call_exhausted(out, part, e);
            }
            roots.push(implicit.clone());
        } else if recursive {
            let filters_slice = non_selector_filter.as_slice();
            match check_recursive_root_impl(
                &implicit,
                Default::default(),
                filters_slice,
                protected,
                resolver,
                caps,
                budget,
            ) {
                Ok(rec) => {
                    out.multiply_linked_file |= rec.multiply_linked_file;
                    out.outcomes.extend(rec.outcomes);
                    out.targets.push(implicit);
                    if budget.is_exhausted() {
                        ensure_exhausted_recorded(&mut out, part, budget);
                    }
                    return Ok(out);
                }
                Err(e) => return finish_call_exhausted(out, part, e),
            }
        } else {
            if let Err(e) = push_checked(&implicit, protected, budget, &mut out.outcomes) {
                return finish_call_exhausted(out, part, e);
            }
            if let Some(filter) = &non_selector_filter
                && let TargetResolution::Resolved { canonical, .. } = &implicit.resolution
            {
                let shared = match SharedSensitive::for_protected_budgeted(protected, budget) {
                    Ok(s) => s,
                    Err(e) => return finish_call_exhausted(out, filter_part, e),
                };
                if let Err(e) = budget
                    .charge(canonical.len().max(1) as u64)
                    .and_then(|()| budget.charge_alloc(canonical.len().max(1)))
                {
                    return finish_call_exhausted(out, filter_part, e);
                }
                let folded_root = match fold_path_listed(canonical, protected.folds, budget) {
                    Ok(f) => f,
                    Err(e) => return finish_call_exhausted(out, filter_part, e),
                };
                if let Err(e) = check_filter_on_root(
                    canonical,
                    &folded_root,
                    filter,
                    false,
                    &filter_part,
                    protected,
                    &shared,
                    resolver,
                    caps,
                    budget,
                    &mut out.outcomes,
                    &mut out.multiply_linked_file,
                ) {
                    return finish_call_exhausted(out, filter_part, e);
                }
            }
        }
        out.targets.push(implicit);
    }

    // Selector glob (`../...`, `/...`, `~...`): expand like shell glob + D-100 escaping Unread.
    if let Some(g) = selector_glob {
        if let Err(e) = budget.charge_alloc(g.len().saturating_mul(2)) {
            return finish_call_exhausted(out, filter_part, e);
        }
        let mut word = Word::new();
        word.extend_with_provenance(g.as_bytes(), ByteProvenance::Unquoted);
        let shared = match SharedSensitive::for_protected_budgeted(protected, budget) {
            Ok(s) => s,
            Err(e) => return finish_call_exhausted(out, filter_part, e),
        };
        let anchored = g.starts_with('/') || g.starts_with('~');
        let root_bases: Vec<Vec<u8>> = if anchored {
            vec![protected.cwd.clone()]
        } else {
            roots
                .iter()
                .filter_map(|r| match &r.resolution {
                    TargetResolution::Resolved { canonical, .. } => Some(canonical.clone()),
                    TargetResolution::Unread(_) => None,
                })
                .collect()
        };
        let mut root_folded: Vec<Vec<u8>> = Vec::with_capacity(roots.len());
        for r in &roots {
            if let TargetResolution::Resolved { canonical, .. } = &r.resolution {
                match fold_path_listed(canonical, protected.folds, budget) {
                    Ok(f) => root_folded.push(f),
                    Err(e) => return finish_call_exhausted(out, filter_part, e),
                }
            }
        }

        let mut escaped = root_bases.is_empty() && !anchored;
        for base in &root_bases {
            let exp = match expand_shell_glob_impl(
                &word,
                base,
                &protected.home,
                filter_part.clone(),
                access,
                resolver,
                caps,
                budget,
            ) {
                Ok(exp) => exp,
                Err(e) => return finish_call_exhausted(out, filter_part, e),
            };
            for unread in exp.unread {
                if let Err(e) = push_charged(
                    &mut out.outcomes,
                    PartResult {
                        part: filter_part.clone(),
                        outcome: PartOutcome::Unread(unread),
                    },
                    budget,
                ) {
                    return finish_call_exhausted(out, PartId::WholeCall, e);
                }
            }
            for target in exp.exact_matches {
                if out.targets.len() >= limits::MAX_PATHS {
                    if let Err(e) = push_charged(
                        &mut out.outcomes,
                        PartResult {
                            part: PartId::WholeCall,
                            outcome: PartOutcome::TooLarge(Cap::Paths),
                        },
                        budget,
                    ) {
                        return finish_call_exhausted(out, PartId::WholeCall, e);
                    }
                    return Ok(out);
                }
                if let TargetResolution::Resolved { canonical, .. } = &target.resolution {
                    let tf = match fold_path_listed(canonical, protected.folds, budget) {
                        Ok(f) => f,
                        Err(e) => return finish_call_exhausted(out, filter_part, e),
                    };
                    if !root_folded.iter().any(|rf| under_bytes(&tf, rf).is_some()) {
                        escaped = true;
                    }
                }
                record_multiply_linked(&target, &mut out.multiply_linked_file);
                if let Err(e) = push_checked(&target, protected, budget, &mut out.outcomes) {
                    return finish_call_exhausted(out, filter_part, e);
                }
                out.targets.push(target);
            }
            let mut emitted_variant = false;
            for target in exp.variant_only_matches {
                escaped = true;
                if !emitted_variant {
                    emitted_variant = true;
                    if let Err(e) = push_charged(
                        &mut out.outcomes,
                        PartResult {
                            part: filter_part.clone(),
                            outcome: PartOutcome::Unread(UnreadReason::VariantOnlyGlobMatch {
                                pattern: g.as_bytes().to_vec(),
                            }),
                        },
                        budget,
                    ) {
                        return finish_call_exhausted(out, PartId::WholeCall, e);
                    }
                }
                out.targets.push(target);
            }
            if let Err(e) =
                check_selector_probes(base, g, protected, &shared, budget, &mut out.outcomes)
            {
                return finish_call_exhausted(out, filter_part, e);
            }
        }
        if escaped || anchored {
            // D-100: an escaping selector cannot be allowed by Home or
            // scoped/prefix rules; only unscoped Any or FullAccess allows
            // it (unless a HardDeny hit was already recorded above).
            if let Err(e) = push_charged(
                &mut out.outcomes,
                PartResult {
                    part: filter_part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::UnsupportedFilterForm),
                },
                budget,
            ) {
                return finish_call_exhausted(out, PartId::WholeCall, e);
            }
        }
        // If recursive is also set on the subject, run S8 / hard-link checks on roots.
        if recursive {
            for root in &roots {
                match check_recursive_root_impl(
                    root,
                    Default::default(),
                    &[],
                    protected,
                    resolver,
                    caps,
                    budget,
                ) {
                    Ok(rec) => {
                        out.multiply_linked_file |= rec.multiply_linked_file;
                        out.outcomes.extend(rec.outcomes);
                        if budget.is_exhausted() {
                            ensure_exhausted_recorded(
                                &mut out,
                                origin_to_part(&root.origin),
                                budget,
                            );
                            return Ok(out);
                        }
                    }
                    Err(e) => return finish_call_exhausted(out, origin_to_part(&root.origin), e),
                }
            }
        }
    }

    Ok(out)
}
