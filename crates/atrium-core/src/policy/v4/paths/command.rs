//! Command-line target resolution (`resolve_command_targets`) and file-tool /
//! search-tool subject path resolution (`resolve_subject_paths`) (contract §5, §8).

use std::collections::HashMap;

use crate::policy::limits;
use crate::policy::model::{
    AccessMode, ByteProvenance, Cap, CodeExecution, CommandUnit, ComputeBudget,
    DerivedDestinations, Exhausted, FileType, FilterDialect, FsCaps, ModelRedirect, OperandRole,
    PartId, PartOutcome, PartResult, PipelineId, ProtectedSet, ResolveError, ResolvedCallPaths,
    SearchFilter, Target, TargetOrigin, TargetResolution, ToolEffects, UnitIndex, UnreadReason,
    ValueRole, Word,
};
use crate::policy::v4::paths::PathResolver;
use crate::policy::v4::paths::glob::expand_shell_glob_impl;
use crate::policy::v4::paths::recursive::{check_filter_on_root, check_recursive_root_impl};
use crate::policy::v4::paths::resolve::{resolve_literal_impl, resolve_word_literal};
use crate::policy::v4::paths::sensitive::{
    SharedSensitive, check_target_impl, fold_path_listed, under_bytes,
};

#[derive(Debug, Clone)]
enum CwdState {
    Known(Vec<u8>),
    Unknown,
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

    let mut out = ResolvedCallPaths::default();

    // Pre-scan pipelines so every stage of a pipeline containing a possible
    // writer — including stages BEFORE the writer — is tainted (§5, §10 row 21).
    let mut pipeline_writers: HashMap<PipelineId, UnitIndex> = HashMap::new();
    for (idx, unit) in units.iter().enumerate() {
        if let Some(pid) = unit.pipeline_id
            && unit_is_possible_writer(unit, effects.get(idx))
        {
            pipeline_writers.entry(pid).or_insert(unit.index);
        }
    }

    let mut cwd_state = CwdState::Known(start_cwd.to_vec());
    let mut earlier_writer: Option<UnitIndex> = None;
    let mut active_pipeline: Option<PipelineId> = None;
    let mut active_pipeline_writer: Option<UnitIndex> = None;

    for (idx, unit) in units.iter().enumerate() {
        let eff = effects.get(idx);

        // If we just left a pipeline that had a writer, promote it to `earlier_writer`.
        if unit.pipeline_id != active_pipeline {
            if let Some(pw) = active_pipeline_writer.take() {
                earlier_writer.get_or_insert(pw);
            }
            active_pipeline = unit.pipeline_id;
        }

        let pipeline_taint = unit
            .pipeline_id
            .and_then(|pid| pipeline_writers.get(&pid).copied());
        let after_writer_taint = if pipeline_taint.is_some() {
            None
        } else {
            earlier_writer
        };

        let mut unit_target_parts: Vec<PartId> = Vec::new();
        let mut saw_explicit_recursive_root = false;
        let mut mv_source_is_dir = false;

        if matches!(cwd_state, CwdState::Unknown) {
            out.outcomes.push(PartResult {
                part: PartId::Unit(unit.index),
                outcome: PartOutcome::Unread(UnreadReason::UnknownCwd),
            });
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

            if out.targets.len() >= limits::MAX_PATHS {
                out.outcomes.push(PartResult {
                    part: PartId::WholeCall,
                    outcome: PartOutcome::TooLarge(Cap::Paths),
                });
                return Ok(out);
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
                        out.outcomes.push(PartResult {
                            part,
                            outcome: PartOutcome::Unread(UnreadReason::UnknownCwd),
                        });
                        out.targets.push(unread_t);
                        continue;
                    }
                    start_cwd
                }
            };

            let is_rec_root =
                eff.is_some_and(|e| e.reads_recursively) && is_recursive_root_role(role);
            if is_rec_root {
                saw_explicit_recursive_root = true;
            }

            if word.has_unquoted_glob() {
                let exp = expand_shell_glob_impl(
                    word,
                    cur_cwd,
                    &protected.home,
                    part.clone(),
                    access,
                    resolver,
                    caps,
                    budget,
                )?;
                for unread in exp.unread {
                    out.outcomes.push(PartResult {
                        part: part.clone(),
                        outcome: PartOutcome::Unread(unread),
                    });
                }
                for target in exp.exact_matches {
                    if out.targets.len() >= limits::MAX_PATHS {
                        out.outcomes.push(PartResult {
                            part: PartId::WholeCall,
                            outcome: PartOutcome::TooLarge(Cap::Paths),
                        });
                        return Ok(out);
                    }
                    record_multiply_linked(&target, &mut out.multiply_linked_file);
                    let checked = check_target_impl(&target, protected, budget)?;
                    out.outcomes.push(checked);
                    if is_rec_root && let Some(e) = eff {
                        let rec = check_recursive_root_impl(
                            &target, e.links, &e.filters, protected, resolver, caps, budget,
                        )?;
                        out.multiply_linked_file |= rec.multiply_linked_file;
                        out.outcomes.extend(rec.outcomes);
                    }
                    if eff.is_some_and(|e| {
                        e.derived_destinations == DerivedDestinations::WhenSourceIsDir
                    }) && role == OperandRole::Path
                        && source_is_dir_or_unknown(&target, resolver, caps, budget)?
                    {
                        mv_source_is_dir = true;
                    }
                    out.targets.push(target);
                }
                for target in exp.variant_only_matches {
                    if out.targets.len() >= limits::MAX_PATHS {
                        out.outcomes.push(PartResult {
                            part: PartId::WholeCall,
                            outcome: PartOutcome::TooLarge(Cap::Paths),
                        });
                        return Ok(out);
                    }
                    // Variant-only matches are Unread, never hits (§1, §5).
                    out.outcomes.push(PartResult {
                        part: part.clone(),
                        outcome: PartOutcome::Unread(UnreadReason::VariantOnlyGlobMatch {
                            pattern: word.literal_bytes().to_vec(),
                        }),
                    });
                    out.targets.push(target);
                }
            } else {
                let target = resolve_word_literal(
                    word,
                    cur_cwd,
                    &protected.home,
                    origin,
                    access,
                    resolver,
                    caps,
                    budget,
                )?;
                record_multiply_linked(&target, &mut out.multiply_linked_file);
                let checked = check_target_impl(&target, protected, budget)?;
                out.outcomes.push(checked);
                if is_rec_root && let Some(e) = eff {
                    let rec = check_recursive_root_impl(
                        &target, e.links, &e.filters, protected, resolver, caps, budget,
                    )?;
                    out.multiply_linked_file |= rec.multiply_linked_file;
                    out.outcomes.extend(rec.outcomes);
                }
                if eff
                    .is_some_and(|e| e.derived_destinations == DerivedDestinations::WhenSourceIsDir)
                    && role == OperandRole::Path
                    && source_is_dir_or_unknown(&target, resolver, caps, budget)?
                {
                    mv_source_is_dir = true;
                }
                out.targets.push(target);
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
                    let target = resolve_literal_impl(
                        cur,
                        cur,
                        &protected.home,
                        TargetOrigin::ImplicitCwd {
                            unit: Some(unit.index),
                        },
                        AccessMode::Read,
                        resolver,
                        caps,
                        budget,
                    )?;
                    record_multiply_linked(&target, &mut out.multiply_linked_file);
                    let rec = check_recursive_root_impl(
                        &target, e.links, &e.filters, protected, resolver, caps, budget,
                    )?;
                    out.multiply_linked_file |= rec.multiply_linked_file;
                    out.outcomes.extend(rec.outcomes);
                    out.targets.push(target);
                }
                CwdState::Unknown => {
                    out.targets.push(Target {
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
            if out.targets.len() >= limits::MAX_PATHS {
                out.outcomes.push(PartResult {
                    part: PartId::WholeCall,
                    outcome: PartOutcome::TooLarge(Cap::Paths),
                });
                return Ok(out);
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
                        out.outcomes.push(PartResult {
                            part,
                            outcome: PartOutcome::Unread(UnreadReason::UnknownCwd),
                        });
                        out.targets.push(Target {
                            origin,
                            access,
                            resolution: TargetResolution::Unread(UnreadReason::UnknownCwd),
                        });
                        continue;
                    }
                    start_cwd
                }
            };

            let target = resolve_word_literal(
                target_word,
                cur_cwd,
                &protected.home,
                origin,
                access,
                resolver,
                caps,
                budget,
            )?;
            record_multiply_linked(&target, &mut out.multiply_linked_file);
            let checked = check_target_impl(&target, protected, budget)?;
            out.outcomes.push(checked);
            out.targets.push(target);
        }

        // Derived destinations (§5, §10 row 18): makes the writer itself Unread,
        // while any destination-root hit checked above still stands.
        if let Some(e) = eff {
            let derived = match e.derived_destinations {
                DerivedDestinations::None => false,
                DerivedDestinations::Always => true,
                DerivedDestinations::WhenSourceIsDir => mv_source_is_dir,
            };
            if derived {
                out.outcomes.push(PartResult {
                    part: PartId::Unit(unit.index),
                    outcome: PartOutcome::Unread(UnreadReason::DerivedDestinations {
                        tool: e.tool_name.unwrap_or("unknown").to_string(),
                    }),
                });
            }
        }

        // Intra-call writer effects (§5, §10 row 21): targets of every unit
        // after a possible writer, and every stage of a pipeline containing a
        // possible writer, are Unread (hard-deny hits already pushed still stand).
        if let Some(writer_unit) = pipeline_taint {
            if unit_target_parts.is_empty() {
                if unit.index != writer_unit {
                    out.outcomes.push(PartResult {
                        part: PartId::Unit(unit.index),
                        outcome: PartOutcome::Unread(UnreadReason::PipelineWithPossibleWriter {
                            writer_unit,
                        }),
                    });
                }
            } else {
                for p in &unit_target_parts {
                    out.outcomes.push(PartResult {
                        part: p.clone(),
                        outcome: PartOutcome::Unread(UnreadReason::PipelineWithPossibleWriter {
                            writer_unit,
                        }),
                    });
                }
            }
        } else if let Some(writer_unit) = after_writer_taint {
            if unit_target_parts.is_empty() {
                out.outcomes.push(PartResult {
                    part: PartId::Unit(unit.index),
                    outcome: PartOutcome::Unread(UnreadReason::AfterPossibleWriter { writer_unit }),
                });
            } else {
                for p in &unit_target_parts {
                    out.outcomes.push(PartResult {
                        part: p.clone(),
                        outcome: PartOutcome::Unread(UnreadReason::AfterPossibleWriter {
                            writer_unit,
                        }),
                    });
                }
            }
        }

        // Update `cd` state for subsequent units (§5).
        if let Some(e) = eff
            && e.tool_name == Some("cd")
        {
            cwd_state = next_cwd_after_cd(unit, e, &cwd_state, protected, resolver, caps, budget)?;
        }

        // Record possible-writer status for later units.
        if unit_is_possible_writer(unit, eff) {
            if unit.pipeline_id.is_some() {
                active_pipeline_writer.get_or_insert(unit.index);
            } else {
                earlier_writer.get_or_insert(unit.index);
            }
        }
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
                TargetResolution::Resolved { canonical, .. } => Ok(CwdState::Known(canonical)),
                TargetResolution::Unread(_) => Ok(CwdState::Unknown),
            }
        }
        None => {
            // Bare `cd` (no positional operand) moves to `home`.
            if unit.argv.len() == 1 {
                Ok(CwdState::Known(protected.home.clone()))
            } else {
                Ok(CwdState::Unknown)
            }
        }
    }
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
        out.outcomes.push(PartResult {
            part: PartId::WholeCall,
            outcome: PartOutcome::TooLarge(Cap::Uses),
        });
        return Ok(out);
    }

    let mut roots: Vec<Target> = Vec::with_capacity(paths.len().max(1));

    // File-tool paths are ALWAYS literal (M1-32 round 4 finding 3; §5; §10 row 11).
    for (path_index, raw) in paths.iter().enumerate() {
        if out.targets.len() >= limits::MAX_PATHS {
            out.outcomes.push(PartResult {
                part: PartId::WholeCall,
                outcome: PartOutcome::TooLarge(Cap::Paths),
            });
            return Ok(out);
        }
        let target = resolve_literal_impl(
            raw.as_bytes(),
            &protected.cwd,
            &protected.home,
            TargetOrigin::SubjectPath { path_index },
            access,
            resolver,
            caps,
            budget,
        )?;
        record_multiply_linked(&target, &mut out.multiply_linked_file);
        let checked = check_target_impl(&target, protected, budget)?;
        out.outcomes.push(checked);
        roots.push(target.clone());
        out.targets.push(target);
    }

    if roots.is_empty() && (recursive || glob.is_some()) {
        let implicit = resolve_literal_impl(
            &protected.cwd,
            &protected.cwd,
            &protected.home,
            TargetOrigin::ImplicitCwd { unit: None },
            access,
            resolver,
            caps,
            budget,
        )?;
        record_multiply_linked(&implicit, &mut out.multiply_linked_file);
        let checked = check_target_impl(&implicit, protected, budget)?;
        out.outcomes.push(checked);
        roots.push(implicit.clone());
        out.targets.push(implicit);
    }

    match glob {
        Some(g) if is_selector_glob(g) => {
            // Selector (§5): expands like a shell glob, and D-100 escaping-glob
            // verdicts stay (Unread when escaping roots, HardDeny if a hit).
            let mut word = Word::new();
            word.extend_with_provenance(g.as_bytes(), ByteProvenance::Unquoted);
            let shared = SharedSensitive::for_protected(protected);
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
            let root_folded: Vec<Vec<u8>> = roots
                .iter()
                .filter_map(|r| match &r.resolution {
                    TargetResolution::Resolved { canonical, .. } => {
                        Some(fold_path_listed(canonical, protected.folds))
                    }
                    TargetResolution::Unread(_) => None,
                })
                .collect();

            let mut escaped = false;
            for base in &root_bases {
                let exp = expand_shell_glob_impl(
                    &word,
                    base,
                    &protected.home,
                    PartId::Filter { unit: None },
                    access,
                    resolver,
                    caps,
                    budget,
                )?;
                for unread in exp.unread {
                    out.outcomes.push(PartResult {
                        part: PartId::Filter { unit: None },
                        outcome: PartOutcome::Unread(unread),
                    });
                }
                for target in exp.exact_matches {
                    if out.targets.len() >= limits::MAX_PATHS {
                        out.outcomes.push(PartResult {
                            part: PartId::WholeCall,
                            outcome: PartOutcome::TooLarge(Cap::Paths),
                        });
                        return Ok(out);
                    }
                    if let TargetResolution::Resolved { canonical, .. } = &target.resolution {
                        let tf = fold_path_listed(canonical, protected.folds);
                        if !root_folded.iter().any(|rf| under_bytes(&tf, rf).is_some()) {
                            escaped = true;
                        }
                    }
                    record_multiply_linked(&target, &mut out.multiply_linked_file);
                    let checked = check_target_impl(&target, protected, budget)?;
                    out.outcomes.push(checked);
                    out.targets.push(target);
                }
                for target in exp.variant_only_matches {
                    escaped = true;
                    out.outcomes.push(PartResult {
                        part: PartId::Filter { unit: None },
                        outcome: PartOutcome::Unread(UnreadReason::VariantOnlyGlobMatch {
                            pattern: g.as_bytes().to_vec(),
                        }),
                    });
                    out.targets.push(target);
                }
            }
            if escaped || anchored {
                // D-100: an escaping selector cannot be allowed by Home or
                // scoped/prefix rules; only unscoped Any or FullAccess allows
                // it (unless a HardDeny hit was already recorded above).
                out.outcomes.push(PartResult {
                    part: PartId::Filter { unit: None },
                    outcome: PartOutcome::Unread(UnreadReason::UnsupportedFilterForm),
                });
            }
            // If recursive is also set on the subject, run S8 / hard-link checks on roots.
            let _ = &shared;
            if recursive {
                for root in &roots {
                    let rec = check_recursive_root_impl(
                        root,
                        Default::default(),
                        &[],
                        protected,
                        resolver,
                        caps,
                        budget,
                    )?;
                    out.multiply_linked_file |= rec.multiply_linked_file;
                    out.outcomes.extend(rec.outcomes);
                }
            }
        }
        Some(g) => {
            let filter = SearchFilter {
                dialect: FilterDialect::SearchGlob,
                pattern: Word::derived(g.as_bytes().to_vec()),
                case_insensitive: false,
                argv_index: None,
            };
            if recursive {
                for root in &roots {
                    let rec = check_recursive_root_impl(
                        root,
                        Default::default(),
                        std::slice::from_ref(&filter),
                        protected,
                        resolver,
                        caps,
                        budget,
                    )?;
                    out.multiply_linked_file |= rec.multiply_linked_file;
                    out.outcomes.extend(rec.outcomes);
                }
            } else {
                let shared = SharedSensitive::for_protected(protected);
                let filter_part = PartId::Filter { unit: None };
                for root in &roots {
                    let TargetResolution::Resolved { canonical, .. } = &root.resolution else {
                        continue;
                    };
                    let folded_root = fold_path_listed(canonical, protected.folds);
                    check_filter_on_root(
                        canonical,
                        &folded_root,
                        &filter,
                        false,
                        &filter_part,
                        protected,
                        &shared,
                        resolver,
                        caps,
                        budget,
                        &mut out.outcomes,
                        &mut out.multiply_linked_file,
                    )?;
                }
            }
        }
        None => {
            if recursive {
                for root in &roots {
                    let rec = check_recursive_root_impl(
                        root,
                        Default::default(),
                        &[],
                        protected,
                        resolver,
                        caps,
                        budget,
                    )?;
                    out.multiply_linked_file |= rec.multiply_linked_file;
                    out.outcomes.extend(rec.outcomes);
                }
            }
        }
    }

    Ok(out)
}
