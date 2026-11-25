//! Rev 4 decision procedure and end-to-end call evaluation (owned by P5;
//! contract §1, §2, §8, §9).

mod best_effort;
#[cfg(test)]
mod tests;

use std::collections::HashSet;

use atrium_protocol::agent::{ToolKind, UnreadNotice};
use atrium_protocol::policy::{HARD_DENY_DEFAULTS, Preset, Rule, ToolSubject};
use serde_json::Value;

use crate::policy::limits;
use crate::policy::model::{
    AccessMode, BestEffortRecord, CallEvaluation, CallOutcome, Cap, CodeExecution, CommandUnit,
    ComputeBudget, DerivedDestinations, ExhaustReason, Exhausted, FoldRules, FsCaps, LinkPolicy,
    ModelRedirect, OperandRole, PartId, PartOutcome, PartResult, ProtectedSet, RuleCoverage,
    RuleEvalContext, Target, TargetOrigin, TargetResolution, ToolEffects, UnitIndex, UnreadReason,
};
use crate::policy::tools;
use crate::policy::v4::lex::lex_best_effort;
use crate::policy::v4::paths::sensitive::{
    ASK_FILES, ASK_NAME_PREFIXES, ASK_TREES, BROWSER_ROOTS, CRED_FILES, CRED_TREES,
    SharedSensitive, fold_path_listed, join_folded, push_charged, under_bytes,
};
use crate::policy::v4::paths::{
    PathResolver, resolve_command_targets, resolve_literal, resolve_subject_paths,
};
use crate::policy::v4::rules::{evaluate_rules, remember_matcher};
use crate::policy::v4::tools::{check_git_selection_into, classify_unit};
use crate::policy::v4::units::build_units;
use crate::{PolicyInput, PolicyVerdict};

use best_effort::{
    BestEffortOptions, check_search_glob_best_effort, is_home_readonly_tool, raw_text_checks,
    run_best_effort_into,
};

/// Runs the §2 best-effort pass over raw text, tolerant-lexer units (including
/// derived `-c` script units), and hard-deny glob/search-filter probes.
///
/// **Owned by:** P5 · **Contract:** §2, §8.
pub(crate) fn run_best_effort(
    record: &BestEffortRecord,
    protected: &ProtectedSet,
    budget: &mut ComputeBudget,
) -> Result<Vec<PartResult>, Exhausted> {
    let mut out = Vec::new();
    match run_best_effort_into(
        record,
        protected,
        &protected.cwd,
        &BestEffortOptions::default(),
        budget,
        &mut out,
    ) {
        Ok(()) => Ok(out),
        Err(e) => {
            // §8: a hit found before the budget ran out still stands, and
            // exhaustion never creates a hit.
            if out.is_empty() {
                Err(e)
            } else {
                out.push(PartResult {
                    part: PartId::WholeCall,
                    outcome: PartOutcome::Exhausted(e.reason),
                });
                Ok(out)
            }
        }
    }
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
    outcome: &CallOutcome,
    rule_coverage: &RuleCoverage,
    preset: Preset,
    home_readonly_all: bool,
) -> PolicyVerdict {
    // Step 1: TooLarge → Deny ("too large to check").
    if outcome
        .parts
        .iter()
        .any(|p| matches!(p.outcome, PartOutcome::TooLarge(_)))
    {
        return PolicyVerdict::Deny {
            reason: "too large to check".into(),
            rule: None,
        };
    }

    // Step 2: HardDenyHit (model or best-effort) → HardDeny.
    // Pick the first id in `HARD_DENY_DEFAULTS` list order for deterministic precedence (`dev`).
    let mut hard_ids: Vec<&'static str> = Vec::new();
    for p in &outcome.parts {
        if let PartOutcome::HardDenyHit(hit) = &p.outcome {
            hard_ids.push(hit.hard_deny_id.unwrap_or("credential-read"));
        }
    }
    if !hard_ids.is_empty() {
        let id = HARD_DENY_DEFAULTS
            .iter()
            .map(|d| d.id)
            .find(|id| hard_ids.contains(id))
            .unwrap_or(hard_ids[0]);
        return PolicyVerdict::HardDeny { id };
    }

    // Step 3: Winning Deny rule → Deny.
    if let Some((reason, rule_id)) = &rule_coverage.deny {
        return PolicyVerdict::Deny {
            reason: reason.clone(),
            rule: Some(rule_id.clone()),
        };
    }

    // Step 4: Multiply-linked regular file → Ask in every setting.
    if outcome.multiply_linked_file {
        return PolicyVerdict::Ask;
    }

    // Step 5: Any Exhausted part → Ask in every setting.
    if outcome
        .parts
        .iter()
        .any(|p| matches!(p.outcome, PartOutcome::Exhausted(_)))
    {
        return PolicyVerdict::Ask;
    }

    // Step 6: §2 authorization table.
    let all_slots_ruled = !rule_coverage.per_unit.is_empty()
        && rule_coverage
            .per_unit
            .iter()
            .all(|u| u.allow_rule.is_some());
    let first_allow_rule = || {
        rule_coverage.unscoped_any_rule.clone().or_else(|| {
            rule_coverage
                .per_unit
                .iter()
                .find_map(|u| u.allow_rule.clone())
        })
    };

    // Unscoped Any Allow rule authorizes Clear, StoreHit, and Unread in every preset.
    if let Some(rule_id) = &rule_coverage.unscoped_any_rule {
        return PolicyVerdict::Allow {
            rule: Some(rule_id.clone()),
        };
    }

    // FullAccess authorizes Clear, StoreHit, and Unread (after HardDeny, Deny,
    // multiply-linked, and Exhausted above).
    if preset == Preset::FullAccess {
        let rule = if all_slots_ruled && all_parts_rule_authorized(outcome, rule_coverage) {
            first_allow_rule()
        } else {
            None
        };
        return PolicyVerdict::Allow { rule };
    }

    // Home / Careful / Normal / Trusted without an unscoped Any rule:
    // every individual part must be authorized.
    for pr in &outcome.parts {
        match &pr.outcome {
            PartOutcome::Clear => {}
            PartOutcome::StoreHit(_) => {
                // StoreHit is authorized ONLY by a covering Allow rule (D-082),
                // never by Home / Careful / Normal / Trusted alone.
                if !part_has_covering_allow(&pr.part, rule_coverage) {
                    return PolicyVerdict::Ask;
                }
            }
            PartOutcome::Unread(UnreadReason::Program { .. }) => {
                // Unread (program) is authorized ONLY by a covering rule naming
                // that program (or unscoped Any, already handled above; §1 step 6, §7).
                if !part_covers_pinned_program(&pr.part, rule_coverage) {
                    return PolicyVerdict::Ask;
                }
            }
            PartOutcome::Unread(_) => {
                // Plain Unread is NEVER authorized by prefix/exact/scoped rules
                // or Home / Careful / Normal / Trusted (§2).
                return PolicyVerdict::Ask;
            }
            PartOutcome::TooLarge(_) | PartOutcome::HardDenyHit(_) | PartOutcome::Exhausted(_) => {
                // Handled in steps 1, 2, 5.
                return PolicyVerdict::Ask;
            }
        }
    }

    // If every slot is covered by an Allow rule, allow in every preset (including Careful).
    if all_slots_ruled {
        return PolicyVerdict::Allow {
            rule: first_allow_rule(),
        };
    }

    // Otherwise, preset decides for Clear parts not covered by a rule.
    match preset {
        Preset::Careful => PolicyVerdict::Ask,
        Preset::Home | Preset::Normal | Preset::Trusted => {
            if home_readonly_all {
                PolicyVerdict::Allow {
                    rule: first_allow_rule(),
                }
            } else {
                PolicyVerdict::Ask
            }
        }
        Preset::FullAccess => PolicyVerdict::Allow { rule: None },
    }
}

fn part_slot_index(part: &PartId, slot_count: usize) -> Option<usize> {
    match part {
        PartId::Unit(u) | PartId::Operand { unit: u, .. } | PartId::Redirect { unit: u, .. } => {
            Some(u.0)
        }
        PartId::Filter { unit: Some(u) } => Some(u.0),
        PartId::SubjectPath { .. } | PartId::Filter { unit: None } => Some(0),
        PartId::WholeCall => (slot_count == 1).then_some(0),
        PartId::BestEffortUnit(_) => None,
    }
}

fn part_has_covering_allow(part: &PartId, coverage: &RuleCoverage) -> bool {
    let Some(slot) = part_slot_index(part, coverage.per_unit.len()) else {
        return false;
    };
    coverage
        .per_unit
        .get(slot)
        .is_some_and(|u| u.allow_rule.is_some())
}

fn part_covers_pinned_program(part: &PartId, coverage: &RuleCoverage) -> bool {
    let Some(slot) = part_slot_index(part, coverage.per_unit.len()) else {
        return false;
    };
    coverage.per_unit.get(slot).is_some_and(|u| {
        u.unscoped_any_allow || (u.allow_rule.is_some() && u.covers_pinned_program)
    })
}

fn all_parts_rule_authorized(outcome: &CallOutcome, coverage: &RuleCoverage) -> bool {
    outcome.parts.iter().all(|pr| match &pr.outcome {
        PartOutcome::Clear => true,
        PartOutcome::StoreHit(_) => part_has_covering_allow(&pr.part, coverage),
        PartOutcome::Unread(UnreadReason::Program { .. }) => {
            part_covers_pinned_program(&pr.part, coverage)
        }
        PartOutcome::Unread(_)
        | PartOutcome::TooLarge(_)
        | PartOutcome::HardDenyHit(_)
        | PartOutcome::Exhausted(_) => false,
    })
}

/// Computes the UI notice (`ApprovalRequested.unread`) for an evaluation outcome (§9).
pub(crate) fn unread_notice_for_outcome(outcome: &CallOutcome) -> Option<UnreadNotice> {
    if outcome
        .parts
        .iter()
        .any(|p| matches!(p.outcome, PartOutcome::Exhausted(_)))
    {
        Some(UnreadNotice::Exhausted)
    } else if outcome
        .parts
        .iter()
        .any(|p| matches!(p.outcome, PartOutcome::Unread(_)))
    {
        Some(UnreadNotice::Unread)
    } else {
        None
    }
}

/// Full rev 4 evaluation entry point wiring P1 (lex/units), P3 (tool effects),
/// P2 (paths/globs/filters), best-effort pass, P3 (rules), and [`decide`].
///
/// **Owned by:** P5 · **Contract:** §1, §2, §13.
pub(crate) fn evaluate_call(
    input: &PolicyInput<'_>,
    rules: &[Rule],
    protected: &ProtectedSet,
    resolver: &dyn PathResolver,
) -> CallEvaluation {
    let mut budget = ComputeBudget::with_default_limits();
    evaluate_call_with_budget(input, rules, protected, resolver, &mut budget)
}

/// Same as [`evaluate_call`] with a caller-provided [`ComputeBudget`] (used by
/// exhaustion tests and §13 boundary property tests).
pub(crate) fn evaluate_call_with_budget(
    input: &PolicyInput<'_>,
    rules: &[Rule],
    protected: &ProtectedSet,
    resolver: &dyn PathResolver,
    budget: &mut ComputeBudget,
) -> CallEvaluation {
    let mut fs_caps = FsCaps::with_default_limits();
    let folds = protected.folds;
    let mut outcome = CallOutcome::default();

    match &input.check.subject {
        ToolSubject::Command { line, cwd } => evaluate_command(
            input,
            line,
            cwd.as_deref(),
            rules,
            protected,
            resolver,
            folds,
            &mut fs_caps,
            budget,
            &mut outcome,
        ),
        ToolSubject::Paths {
            paths,
            glob,
            recursive,
        } => evaluate_paths(
            input,
            paths,
            glob.as_deref(),
            *recursive,
            rules,
            protected,
            resolver,
            folds,
            &mut fs_caps,
            budget,
            &mut outcome,
        ),
        ToolSubject::Fetch { url } => evaluate_fetch(
            input,
            url,
            rules,
            protected,
            resolver,
            folds,
            &mut fs_caps,
            budget,
            &mut outcome,
        ),
        ToolSubject::Mcp { .. } => evaluate_mcp(
            input,
            rules,
            protected,
            resolver,
            folds,
            &mut fs_caps,
            budget,
            &mut outcome,
        ),
        ToolSubject::None => evaluate_none(input, rules, folds, budget, outcome),
        ToolSubject::Unknown => evaluate_unknown(
            input,
            rules,
            protected,
            resolver,
            folds,
            &mut fs_caps,
            budget,
            &mut outcome,
        ),
    }
}

// ---------------------------------------------------------------------------
// 1. `ToolSubject::Command`
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn evaluate_command(
    input: &PolicyInput<'_>,
    line: &str,
    cwd_override: Option<&str>,
    rules: &[Rule],
    protected: &ProtectedSet,
    resolver: &dyn PathResolver,
    folds: FoldRules,
    fs_caps: &mut FsCaps,
    budget: &mut ComputeBudget,
    outcome: &mut CallOutcome,
) -> CallEvaluation {
    // Structural cap FIRST before any fallible charge (D-150).
    if line.len() > limits::MAX_BYTES {
        outcome.parts.push(PartResult {
            part: PartId::WholeCall,
            outcome: PartOutcome::TooLarge(Cap::Bytes),
        });
        return finalize_too_large(std::mem::take(outcome));
    }

    // 1. P1: exact lex + unwrap + nested shells + best-effort lex.
    let parsed = match build_units(line, &folds, budget) {
        Ok(p) => p,
        Err(PartOutcome::TooLarge(cap)) => {
            outcome.parts.push(PartResult {
                part: PartId::WholeCall,
                outcome: PartOutcome::TooLarge(cap),
            });
            return finalize_too_large(std::mem::take(outcome));
        }
        Err(other) => {
            outcome.parts.push(PartResult {
                part: PartId::WholeCall,
                outcome: other,
            });
            return finalize_exhausted_or_ask(
                input,
                rules,
                folds,
                &[],
                &[],
                &[],
                &BestEffortRecord::default(),
                budget,
                std::mem::take(outcome),
                false,
            );
        }
    };

    // Preserve all P1 outcomes (and remember which units P1 rejected as nested shells).
    let mut p1_rejected_nested: HashSet<UnitIndex> = HashSet::new();
    let mut p1_exhausted = false;
    for pr in &parsed.outcomes {
        if matches!(pr.outcome, PartOutcome::Exhausted(_)) {
            p1_exhausted = true;
        }
        if let PartId::Unit(uid) = pr.part
            && matches!(
                pr.outcome,
                PartOutcome::Unread(
                    UnreadReason::UnsupportedNestedShellForm { .. }
                        | UnreadReason::MaxNestingDepthReached
                        | UnreadReason::Program { .. }
                        | UnreadReason::CommandWordGlob
                )
            )
        {
            p1_rejected_nested.insert(uid);
        }
    }
    outcome.parts.extend(parsed.outcomes);

    // Resolve optional per-call `cwd` override relative to `protected.cwd`.
    let mut effective_start_cwd: Vec<u8> = protected.cwd.clone();
    if let Some(sub_cwd) = cwd_override
        && !budget.is_exhausted()
    {
        match resolve_command_cwd_override(sub_cwd, protected, resolver, fs_caps, budget) {
            Ok(Ok(canon)) => effective_start_cwd = canon,
            Ok(Err(unread)) => {
                record_part_preserving(
                    &mut outcome.parts,
                    PartResult {
                        part: PartId::WholeCall,
                        outcome: PartOutcome::Unread(unread),
                    },
                    budget,
                );
            }
            Err(e) => {
                record_exhausted(&mut outcome.parts, PartId::WholeCall, e.reason);
            }
        }
    }

    // 2. P3: classify each modeled unit + Git name-selection / wildcard checks.
    let mut effects: Vec<ToolEffects> = Vec::with_capacity(parsed.units.len());
    let mut quoted_literal_words: HashSet<Vec<u8>> = HashSet::new();
    for u in &parsed.units {
        if budget.is_exhausted() {
            record_exhausted(
                &mut outcome.parts,
                PartId::Unit(u.index),
                budget
                    .exhausted_reason()
                    .unwrap_or(ExhaustReason::ComputeBudget),
            );
            break;
        }
        // Record exact quoted literal words that have NO unquoted glob chars (§3, §10 row 9).
        for w in &u.argv {
            if !w.has_unquoted_glob()
                && w.literal_bytes()
                    .iter()
                    .any(|&b| matches!(b, b'*' | b'?' | b'['))
            {
                quoted_literal_words.insert(w.literal_bytes().to_vec());
            }
        }

        match classify_unit(u, &folds, budget) {
            Ok(mut eff) => {
                // Wiring: if P1 expanded this `sh`/`bash`/`dash -c SCRIPT` unit
                // into child units (modeled §4 form), give the parent unit
                // non-writing modeled nested-shell effects (matching P2's
                // `p2_tests.rs:3757` expectation) instead of P3's standalone
                // `UnsupportedNestedShellForm`.
                if !p1_exhausted
                    && !p1_rejected_nested.contains(&u.index)
                    && eff
                        .tool_name
                        .and_then(tools::lookup)
                        .is_some_and(tools::ToolSpec::is_nested_shell)
                    && eff
                        .unread
                        .iter()
                        .any(|r| matches!(r, UnreadReason::UnsupportedNestedShellForm { .. }))
                {
                    eff = modeled_nested_shell_effects(u, eff.tool_name.unwrap_or("sh"));
                }

                // Wiring (CR logged in handoff): P0's `HomeReadOnly` leaves
                // `operand_access = ReadWrite` while P2 treats `ReadWrite` as
                // `writes = true`, which would falsely HardDeny `cat ~/.ssh/config`,
                // `ls ~/.ssh`, and `cat ~/.config/atrium/config.toml`. When a
                // `HomeReadOnly` tool has no write flags (i.e. writes is due at
                // most to a redirect, which has its own `AccessMode::Write`),
                // downgrade operand access to `Read`.
                let redirect_writes = u
                    .redirects
                    .iter()
                    .any(ModelRedirect::writes_outside_dev_null);
                let tool_only_reads = !eff.writes
                    || (redirect_writes
                        && eff
                            .tool_name
                            .is_some_and(|n| is_never_writing_home_ro(n, eff.subcommand)));
                if tool_only_reads
                    && eff
                        .tool_name
                        .is_some_and(|n| is_home_readonly_tool(n, eff.subcommand))
                {
                    for a in eff.operand_access.iter_mut().skip(1) {
                        *a = AccessMode::Read;
                    }
                }

                // Push unit-level Unread reasons from P3 (avoiding duplicate
                // `UnreadReason::Program` already pushed by P1 for `sh x.sh`).
                for reason in &eff.unread {
                    let dup_p1_program = matches!(reason, UnreadReason::Program { .. })
                        && p1_rejected_nested.contains(&u.index);
                    if !dup_p1_program {
                        record_part_preserving(
                            &mut outcome.parts,
                            PartResult {
                                part: PartId::Unit(u.index),
                                outcome: PartOutcome::Unread(reason.clone()),
                            },
                            budget,
                        );
                    }
                }

                // P3 Git name-selection & wildcard-pathspec checks (§6).
                if eff.tool_name == Some("git")
                    && !budget.is_exhausted()
                    && let Err(e) =
                        check_git_selection_into(u, &eff, &folds, &mut outcome.parts, budget)
                {
                    record_exhausted(&mut outcome.parts, PartId::Unit(u.index), e.reason);
                }

                effects.push(eff);
            }
            Err(e) => {
                record_exhausted(&mut outcome.parts, PartId::Unit(u.index), e.reason);
                break;
            }
        }
    }

    // 3. P2: resolve command targets, globs, recursive roots, filters, writers, cd.
    let mut targets: Vec<Target> = Vec::new();
    if !budget.is_exhausted() && !parsed.units.is_empty() {
        match resolve_command_targets(
            &parsed.units,
            &effects,
            &effective_start_cwd,
            protected,
            resolver,
            fs_caps,
            budget,
        ) {
            Ok(resolved) => {
                outcome.multiply_linked_file |= resolved.multiply_linked_file;
                outcome.parts.extend(resolved.outcomes);
                targets = resolved.targets;
            }
            Err(e) => {
                record_exhausted(&mut outcome.parts, PartId::WholeCall, e.reason);
            }
        }
    }

    // 4. §2 best-effort pass over raw text and tolerant-lexer units.
    if !budget.is_exhausted() {
        let opts = BestEffortOptions {
            quoted_literal_words: Some(&quoted_literal_words),
        };
        if let Err(e) = run_best_effort_into(
            &parsed.best_effort,
            protected,
            &effective_start_cwd,
            &opts,
            budget,
            &mut outcome.parts,
        ) {
            record_exhausted(&mut outcome.parts, PartId::WholeCall, e.reason);
        }
    }

    // 5. P3: evaluate rules.
    let rule_coverage = if budget.is_exhausted() {
        RuleCoverage::default()
    } else {
        let rule_ctx = RuleEvalContext {
            check: input.check,
            preset: input.preset,
            thread_id: input.thread,
            units: &parsed.units,
            effects: &effects,
            best_effort: &parsed.best_effort,
            targets: &targets,
            outcomes: &outcome.parts,
            folds: &folds,
        };
        match evaluate_rules(&rule_ctx, rules, budget) {
            Ok(cov) => cov,
            Err(e) => {
                record_exhausted(&mut outcome.parts, PartId::WholeCall, e.reason);
                RuleCoverage::default()
            }
        }
    };

    // 6. Compute Home read-only eligibility across uncovered units (including
    //    Home's newline guard and recursive-root strictly-inside-home guard).
    let home_readonly_all = compute_command_home_readonly(
        line,
        &parsed.units,
        &effects,
        &targets,
        &rule_coverage,
        protected,
        budget,
        &mut outcome.parts,
    );

    let verdict = decide(outcome, &rule_coverage, input.preset, home_readonly_all);
    let remember = if verdict == PolicyVerdict::Ask {
        let rule_ctx = RuleEvalContext {
            check: input.check,
            preset: input.preset,
            thread_id: input.thread,
            units: &parsed.units,
            effects: &effects,
            best_effort: &parsed.best_effort,
            targets: &targets,
            outcomes: &outcome.parts,
            folds: &folds,
        };
        remember_matcher(&rule_ctx, outcome)
    } else {
        None
    };

    CallEvaluation {
        verdict,
        outcome: std::mem::take(outcome),
        remember_matcher: remember,
    }
}

fn resolve_command_cwd_override(
    sub_cwd: &str,
    protected: &ProtectedSet,
    resolver: &dyn PathResolver,
    fs_caps: &mut FsCaps,
    budget: &mut ComputeBudget,
) -> Result<Result<Vec<u8>, UnreadReason>, Exhausted> {
    budget.charge(sub_cwd.len().max(1) as u64)?;
    // Expand leading `$HOME` / `${HOME}` / `$PWD` / `${PWD}` on cwd override
    // (e.g. `cwd: Some("$HOME/.ssh")` in `adversarial_gate.rs`).
    let home_s = String::from_utf8_lossy(&protected.home);
    let cwd_s = String::from_utf8_lossy(&protected.cwd);
    let expanded: Vec<u8> = if let Some(rest) = sub_cwd
        .strip_prefix("$HOME")
        .or_else(|| sub_cwd.strip_prefix("${HOME}"))
        && (rest.is_empty() || rest.starts_with('/'))
    {
        let mut v = Vec::with_capacity(home_s.len() + rest.len());
        budget.charge_alloc(home_s.len() + rest.len())?;
        v.extend_from_slice(&protected.home);
        v.extend_from_slice(rest.as_bytes());
        v
    } else if let Some(rest) = sub_cwd
        .strip_prefix("$PWD")
        .or_else(|| sub_cwd.strip_prefix("${PWD}"))
        && (rest.is_empty() || rest.starts_with('/'))
    {
        let mut v = Vec::with_capacity(cwd_s.len() + rest.len());
        budget.charge_alloc(cwd_s.len() + rest.len())?;
        v.extend_from_slice(&protected.cwd);
        v.extend_from_slice(rest.as_bytes());
        v
    } else {
        sub_cwd.as_bytes().to_vec()
    };

    let target = resolve_literal(
        &expanded,
        &protected.cwd,
        &protected.home,
        TargetOrigin::ImplicitCwd { unit: None },
        AccessMode::Read,
        resolver,
        fs_caps,
        budget,
    )?;
    match target.resolution {
        TargetResolution::Resolved { canonical, .. } => Ok(Ok(canonical)),
        TargetResolution::Unread(r) => Ok(Err(r)),
    }
}

fn modeled_nested_shell_effects(unit: &CommandUnit, shell_name: &'static str) -> ToolEffects {
    let argc = unit.argv.len();
    let mut roles = Vec::with_capacity(argc);
    let operand_access = vec![AccessMode::Read; argc];
    for idx in 0..argc {
        if idx == 0 {
            roles.push(OperandRole::Command);
        } else if idx + 1 == argc {
            roles.push(OperandRole::Code);
        } else {
            roles.push(OperandRole::Flag);
        }
    }
    let redirect_writes = unit
        .redirects
        .iter()
        .any(ModelRedirect::writes_outside_dev_null);
    ToolEffects {
        tool_name: Some(shell_name),
        subcommand: None,
        roles,
        operand_access,
        writes: redirect_writes,
        derived_destinations: DerivedDestinations::None,
        reads_recursively: false,
        links: LinkPolicy::default(),
        code: CodeExecution::None,
        filter_dialect: None,
        filters: Vec::new(),
        home_readonly_eligible: false,
        cd_target_operand: None,
        git_selects_by_name: false,
        unread: Vec::new(),
    }
}

/// `HomeReadOnly` tools whose normative row has empty `forbidden` and `only: None`
/// (i.e. they can never write through flags, only via an output redirect).
fn is_never_writing_home_ro(tool_name: &str, _subcommand: Option<&str>) -> bool {
    matches!(
        tool_name,
        "ls" | "pwd"
            | "cat"
            | "head"
            | "tail"
            | "wc"
            | "stat"
            | "du"
            | "df"
            | "which"
            | "uname"
            | "whoami"
    )
}

#[allow(clippy::too_many_arguments)]
fn compute_command_home_readonly(
    line: &str,
    units: &[CommandUnit],
    effects: &[ToolEffects],
    targets: &[Target],
    rule_coverage: &RuleCoverage,
    protected: &ProtectedSet,
    budget: &mut ComputeBudget,
    outcomes: &mut Vec<PartResult>,
) -> bool {
    if units.is_empty() || effects.len() != units.len() || line.contains('\n') {
        return false;
    }
    let mut shared_opt = None;
    for (idx, u) in units.iter().enumerate() {
        if rule_coverage
            .per_unit
            .get(idx)
            .is_some_and(|c| c.allow_rule.is_some())
        {
            continue;
        }
        let eff = &effects[idx];
        if !eff.home_readonly_eligible {
            return false;
        }
        if eff.reads_recursively {
            let shared = match &shared_opt {
                Some(s) => s,
                None => match SharedSensitive::for_protected_budgeted(protected, budget) {
                    Ok(s) => shared_opt.insert(s),
                    Err(e) => {
                        record_exhausted(outcomes, PartId::Unit(u.index), e.reason);
                        return false;
                    }
                },
            };
            // Recursive read in Home requires at least one explicit root, and
            // every root must be strictly inside `home` and contain no
            // sensitive/protected/store path (`dev` `Home::root_ok`).
            let mut explicit_roots = 0usize;
            for t in targets {
                let is_this_unit_root = match &t.origin {
                    TargetOrigin::Operand { unit, argv_index } if *unit == u.index => {
                        eff.roles.get(*argv_index).is_some_and(|r| {
                            matches!(
                                r,
                                OperandRole::Path
                                    | OperandRole::FlagValue(crate::policy::model::ValueRole::Path)
                            )
                        })
                    }
                    TargetOrigin::ImplicitCwd { unit: Some(unit) } if *unit == u.index => {
                        // Implicit cwd root on `rg`/`grep -r` asks in Home (`dev` `engine.rs:826`).
                        return false;
                    }
                    _ => false,
                };
                if !is_this_unit_root {
                    continue;
                }
                explicit_roots += 1;
                let TargetResolution::Resolved { canonical, .. } = &t.resolution else {
                    return false;
                };
                match home_recursive_root_ok(canonical, protected, shared, budget) {
                    Ok(true) => {}
                    Ok(false) => return false,
                    Err(e) => {
                        record_exhausted(outcomes, PartId::Unit(u.index), e.reason);
                        return false;
                    }
                }
            }
            if explicit_roots == 0 {
                return false;
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------
// 2. `ToolSubject::Paths`
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn evaluate_paths(
    input: &PolicyInput<'_>,
    paths: &[String],
    glob: Option<&str>,
    recursive: bool,
    rules: &[Rule],
    protected: &ProtectedSet,
    resolver: &dyn PathResolver,
    folds: FoldRules,
    fs_caps: &mut FsCaps,
    budget: &mut ComputeBudget,
    outcome: &mut CallOutcome,
) -> CallEvaluation {
    // Structural caps FIRST before any fallible charge (D-150).
    if paths.len() > limits::MAX_USES {
        outcome.parts.push(PartResult {
            part: PartId::WholeCall,
            outcome: PartOutcome::TooLarge(Cap::Uses),
        });
        return finalize_too_large(std::mem::take(outcome));
    }
    if let Some(g) = glob
        && g.len() > limits::MAX_BYTES
    {
        outcome.parts.push(PartResult {
            part: PartId::WholeCall,
            outcome: PartOutcome::TooLarge(Cap::Bytes),
        });
        return finalize_too_large(std::mem::take(outcome));
    }

    let access = match input.check.kind {
        ToolKind::Read | ToolKind::Search => AccessMode::Read,
        _ => AccessMode::Write,
    };

    let mut targets: Vec<Target> = Vec::new();
    if !budget.is_exhausted() {
        match resolve_subject_paths(
            paths, glob, recursive, access, protected, resolver, fs_caps, budget,
        ) {
            Ok(resolved) => {
                outcome.multiply_linked_file |= resolved.multiply_linked_file;
                outcome.parts.extend(resolved.outcomes);
                targets = resolved.targets;
            }
            Err(e) => {
                record_exhausted(&mut outcome.parts, PartId::WholeCall, e.reason);
            }
        }
    }

    // Best-effort search-glob brace/escape/probe pass (§2, D-100).
    if let Some(g) = glob
        && !budget.is_exhausted()
        && let Err(e) = check_search_glob_best_effort(
            paths,
            g,
            recursive,
            protected,
            budget,
            &mut outcome.parts,
        )
    {
        record_exhausted(&mut outcome.parts, PartId::Filter { unit: None }, e.reason);
    }

    let empty_be = BestEffortRecord::default();
    let rule_coverage = if budget.is_exhausted() {
        RuleCoverage::default()
    } else {
        let rule_ctx = RuleEvalContext {
            check: input.check,
            preset: input.preset,
            thread_id: input.thread,
            units: &[],
            effects: &[],
            best_effort: &empty_be,
            targets: &targets,
            outcomes: &outcome.parts,
            folds: &folds,
        };
        match evaluate_rules(&rule_ctx, rules, budget) {
            Ok(cov) => cov,
            Err(e) => {
                record_exhausted(&mut outcome.parts, PartId::WholeCall, e.reason);
                RuleCoverage::default()
            }
        }
    };

    let home_readonly_all = match input.check.kind {
        ToolKind::Read => true,
        ToolKind::Search => {
            if !recursive {
                true
            } else {
                check_subject_recursive_roots_home_ok(
                    &targets,
                    protected,
                    budget,
                    &mut outcome.parts,
                )
            }
        }
        _ => false,
    };

    let verdict = decide(outcome, &rule_coverage, input.preset, home_readonly_all);
    let remember = if verdict == PolicyVerdict::Ask {
        let rule_ctx = RuleEvalContext {
            check: input.check,
            preset: input.preset,
            thread_id: input.thread,
            units: &[],
            effects: &[],
            best_effort: &empty_be,
            targets: &targets,
            outcomes: &outcome.parts,
            folds: &folds,
        };
        remember_matcher(&rule_ctx, outcome)
    } else {
        None
    };

    CallEvaluation {
        verdict,
        outcome: std::mem::take(outcome),
        remember_matcher: remember,
    }
}

fn check_subject_recursive_roots_home_ok(
    targets: &[Target],
    protected: &ProtectedSet,
    budget: &mut ComputeBudget,
    outcomes: &mut Vec<PartResult>,
) -> bool {
    let shared = match SharedSensitive::for_protected_budgeted(protected, budget) {
        Ok(s) => s,
        Err(e) => {
            record_exhausted(outcomes, PartId::WholeCall, e.reason);
            return false;
        }
    };
    let mut root_count = 0usize;
    for t in targets {
        if !matches!(
            t.origin,
            TargetOrigin::SubjectPath { .. } | TargetOrigin::ImplicitCwd { unit: None }
        ) {
            continue;
        }
        root_count += 1;
        let TargetResolution::Resolved { canonical, .. } = &t.resolution else {
            return false;
        };
        match home_recursive_root_ok(canonical, protected, &shared, budget) {
            Ok(true) => {}
            Ok(false) => return false,
            Err(e) => {
                record_exhausted(outcomes, PartId::WholeCall, e.reason);
                return false;
            }
        }
    }
    root_count > 0
}

// ---------------------------------------------------------------------------
// 3. `ToolSubject::Fetch`
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn evaluate_fetch(
    input: &PolicyInput<'_>,
    url: &str,
    rules: &[Rule],
    protected: &ProtectedSet,
    resolver: &dyn PathResolver,
    folds: FoldRules,
    fs_caps: &mut FsCaps,
    budget: &mut ComputeBudget,
    outcome: &mut CallOutcome,
) -> CallEvaluation {
    if url.len() > limits::MAX_BYTES {
        outcome.parts.push(PartResult {
            part: PartId::WholeCall,
            outcome: PartOutcome::TooLarge(Cap::Bytes),
        });
        return finalize_too_large(std::mem::take(outcome));
    }
    let mut targets: Vec<Target> = Vec::new();
    if let Some(file_path) = strip_file_url(url)
        && !budget.is_exhausted()
    {
        match resolve_subject_paths(
            &[file_path],
            None,
            false,
            AccessMode::Read,
            protected,
            resolver,
            fs_caps,
            budget,
        ) {
            Ok(resolved) => {
                outcome.multiply_linked_file |= resolved.multiply_linked_file;
                outcome.parts.extend(resolved.outcomes);
                targets = resolved.targets;
            }
            Err(e) => record_exhausted(&mut outcome.parts, PartId::WholeCall, e.reason),
        }
    }

    let empty_be = BestEffortRecord::default();
    let rule_ctx = RuleEvalContext {
        check: input.check,
        preset: input.preset,
        thread_id: input.thread,
        units: &[],
        effects: &[],
        best_effort: &empty_be,
        targets: &targets,
        outcomes: &outcome.parts,
        folds: &folds,
    };
    let rule_coverage = if budget.is_exhausted() {
        RuleCoverage::default()
    } else {
        match evaluate_rules(&rule_ctx, rules, budget) {
            Ok(cov) => cov,
            Err(e) => {
                record_exhausted(&mut outcome.parts, PartId::WholeCall, e.reason);
                RuleCoverage::default()
            }
        }
    };

    let verdict = decide(outcome, &rule_coverage, input.preset, false);
    CallEvaluation {
        verdict,
        outcome: std::mem::take(outcome),
        remember_matcher: None,
    }
}

fn strip_file_url(url: &str) -> Option<String> {
    let rest = if url.len() >= 7 && url[..7].eq_ignore_ascii_case("file://") {
        &url[7..]
    } else {
        return None;
    };
    let path = if rest.len() >= 9
        && rest[..9].eq_ignore_ascii_case("localhost")
        && (rest.len() == 9 || rest.as_bytes()[9] == b'/')
    {
        &rest[9..]
    } else {
        rest
    };
    Some(path.to_owned())
}

// ---------------------------------------------------------------------------
// 4. `ToolSubject::Mcp`
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn evaluate_mcp(
    input: &PolicyInput<'_>,
    rules: &[Rule],
    protected: &ProtectedSet,
    resolver: &dyn PathResolver,
    folds: FoldRules,
    fs_caps: &mut FsCaps,
    budget: &mut ComputeBudget,
    outcome: &mut CallOutcome,
) -> CallEvaluation {
    // Structural caps FIRST (D-150).
    let strings = match collect_json_strings_capped(&input.check.input) {
        Ok(s) => s,
        Err(cap) => {
            outcome.parts.push(PartResult {
                part: PartId::WholeCall,
                outcome: PartOutcome::TooLarge(cap),
            });
            return finalize_too_large(std::mem::take(outcome));
        }
    };

    let mut targets: Vec<Target> = Vec::new();
    if !budget.is_exhausted() {
        match SharedSensitive::for_protected_budgeted(protected, budget) {
            Ok(shared) => {
                for &s in &strings {
                    if let Err(e) = raw_text_checks(s, &shared, folds, budget, &mut outcome.parts) {
                        record_exhausted(&mut outcome.parts, PartId::WholeCall, e.reason);
                        break;
                    }
                }
            }
            Err(e) => record_exhausted(&mut outcome.parts, PartId::WholeCall, e.reason),
        }
    }

    if !budget.is_exhausted() {
        let path_candidates: Vec<String> = strings
            .iter()
            .copied()
            .filter(|s| !s.is_empty() && !s.contains(char::is_whitespace))
            .map(str::to_owned)
            .collect();
        if !path_candidates.is_empty() {
            match resolve_subject_paths(
                &path_candidates,
                None,
                false,
                AccessMode::Read,
                protected,
                resolver,
                fs_caps,
                budget,
            ) {
                Ok(resolved) => {
                    outcome.multiply_linked_file |= resolved.multiply_linked_file;
                    outcome.parts.extend(resolved.outcomes);
                    targets = resolved.targets;
                }
                Err(e) => record_exhausted(&mut outcome.parts, PartId::WholeCall, e.reason),
            }
        }
    }

    let empty_be = BestEffortRecord::default();
    let rule_ctx = RuleEvalContext {
        check: input.check,
        preset: input.preset,
        thread_id: input.thread,
        units: &[],
        effects: &[],
        best_effort: &empty_be,
        targets: &targets,
        outcomes: &outcome.parts,
        folds: &folds,
    };
    let rule_coverage = if budget.is_exhausted() {
        RuleCoverage::default()
    } else {
        match evaluate_rules(&rule_ctx, rules, budget) {
            Ok(cov) => cov,
            Err(e) => {
                record_exhausted(&mut outcome.parts, PartId::WholeCall, e.reason);
                RuleCoverage::default()
            }
        }
    };

    let verdict = decide(outcome, &rule_coverage, input.preset, false);
    CallEvaluation {
        verdict,
        outcome: std::mem::take(outcome),
        remember_matcher: None,
    }
}

// ---------------------------------------------------------------------------
// 5. `ToolSubject::None`
// ---------------------------------------------------------------------------

fn evaluate_none(
    input: &PolicyInput<'_>,
    rules: &[Rule],
    folds: FoldRules,
    budget: &mut ComputeBudget,
    mut outcome: CallOutcome,
) -> CallEvaluation {
    let empty_be = BestEffortRecord::default();
    let rule_ctx = RuleEvalContext {
        check: input.check,
        preset: input.preset,
        thread_id: input.thread,
        units: &[],
        effects: &[],
        best_effort: &empty_be,
        targets: &[],
        outcomes: &outcome.parts,
        folds: &folds,
    };
    let rule_coverage = if budget.is_exhausted() {
        RuleCoverage::default()
    } else {
        match evaluate_rules(&rule_ctx, rules, budget) {
            Ok(cov) => cov,
            Err(e) => {
                record_exhausted(&mut outcome.parts, PartId::WholeCall, e.reason);
                RuleCoverage::default()
            }
        }
    };

    // `ToolSubject::None` is allowed in every preset (including Careful),
    // unless a Deny rule or budget exhaustion applies (`dev` `engine.rs:840, 968`).
    let verdict = if let Some((reason, rule_id)) = &rule_coverage.deny {
        PolicyVerdict::Deny {
            reason: reason.clone(),
            rule: Some(rule_id.clone()),
        }
    } else if outcome
        .parts
        .iter()
        .any(|p| matches!(p.outcome, PartOutcome::Exhausted(_)))
    {
        PolicyVerdict::Ask
    } else {
        let rule = rule_coverage.unscoped_any_rule.clone().or_else(|| {
            rule_coverage
                .per_unit
                .first()
                .and_then(|u| u.allow_rule.clone())
        });
        PolicyVerdict::Allow { rule }
    };

    CallEvaluation {
        verdict,
        outcome,
        remember_matcher: None,
    }
}

// ---------------------------------------------------------------------------
// 6. `ToolSubject::Unknown` (§1: Exhausted → Ask everywhere, after HardDeny/Deny)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn evaluate_unknown(
    input: &PolicyInput<'_>,
    rules: &[Rule],
    protected: &ProtectedSet,
    resolver: &dyn PathResolver,
    folds: FoldRules,
    fs_caps: &mut FsCaps,
    budget: &mut ComputeBudget,
    outcome: &mut CallOutcome,
) -> CallEvaluation {
    // Structural caps FIRST (D-150).
    let strings = match collect_json_strings_capped(&input.check.input) {
        Ok(s) => s,
        Err(cap) => {
            outcome.parts.push(PartResult {
                part: PartId::WholeCall,
                outcome: PartOutcome::TooLarge(cap),
            });
            return finalize_too_large(std::mem::take(outcome));
        }
    };

    // Best-effort lex + best-effort hard-deny pass on extracted strings.
    let mut best_effort = BestEffortRecord::default();
    if !budget.is_exhausted() && !strings.is_empty() {
        match lex_best_effort(&strings, budget) {
            Ok(be) => {
                best_effort = be;
                if let Err(e) = run_best_effort_into(
                    &best_effort,
                    protected,
                    &protected.cwd,
                    &BestEffortOptions::default(),
                    budget,
                    &mut outcome.parts,
                ) {
                    record_exhausted(&mut outcome.parts, PartId::WholeCall, e.reason);
                }
            }
            Err(PartOutcome::TooLarge(cap)) => {
                outcome.parts.push(PartResult {
                    part: PartId::WholeCall,
                    outcome: PartOutcome::TooLarge(cap),
                });
                return finalize_too_large(std::mem::take(outcome));
            }
            Err(other) => {
                record_part_preserving(
                    &mut outcome.parts,
                    PartResult {
                        part: PartId::WholeCall,
                        outcome: other,
                    },
                    budget,
                );
            }
        }
    }

    // Also resolve single-token strings and best-effort operands via `resolver`
    // so symlinked credential paths inside Unknown payloads are caught too.
    let mut targets: Vec<Target> = Vec::new();
    if !budget.is_exhausted() {
        let mut candidates: Vec<String> = Vec::new();
        for &s in &strings {
            if !s.is_empty()
                && !s.contains(char::is_whitespace)
                && candidates.len() < limits::MAX_USES
            {
                candidates.push(s.to_owned());
            }
        }
        for u in &best_effort.units {
            for w in u.effective_argv.iter().skip(1) {
                if !w.is_empty() && !candidates.contains(w) && candidates.len() < limits::MAX_USES {
                    candidates.push(w.clone());
                }
            }
        }
        if !candidates.is_empty() {
            match resolve_subject_paths(
                &candidates,
                None,
                false,
                AccessMode::ReadWrite,
                protected,
                resolver,
                fs_caps,
                budget,
            ) {
                Ok(resolved) => {
                    outcome.multiply_linked_file |= resolved.multiply_linked_file;
                    outcome.parts.extend(resolved.outcomes);
                    targets = resolved.targets;
                }
                Err(e) => record_exhausted(&mut outcome.parts, PartId::WholeCall, e.reason),
            }
        }
    }

    // §1: `ToolSubject::Unknown` is ALWAYS `Exhausted(UnknownSubject)`.
    outcome.parts.push(PartResult {
        part: PartId::WholeCall,
        outcome: PartOutcome::Exhausted(ExhaustReason::UnknownSubject),
    });

    let rule_ctx = RuleEvalContext {
        check: input.check,
        preset: input.preset,
        thread_id: input.thread,
        units: &[],
        effects: &[],
        best_effort: &best_effort,
        targets: &targets,
        outcomes: &outcome.parts,
        folds: &folds,
    };
    let rule_coverage = if budget.is_exhausted() {
        RuleCoverage::default()
    } else {
        match evaluate_rules(&rule_ctx, rules, budget) {
            Ok(cov) => cov,
            Err(_) => RuleCoverage::default(),
        }
    };

    let verdict = decide(outcome, &rule_coverage, input.preset, false);
    CallEvaluation {
        verdict,
        outcome: std::mem::take(outcome),
        remember_matcher: None,
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// `dev` `Home::root_ok`: recursive search roots in Home must lie strictly
/// inside `home` (not `home` itself or an ancestor/sibling outside `home`)
/// and must not equal or contain any protected/store path.
fn home_recursive_root_ok(
    canonical: &[u8],
    protected: &ProtectedSet,
    shared: &SharedSensitive,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    let folded = fold_path_listed(canonical, protected.folds, budget)?;
    let home = shared.home_folded();
    let Some(rest) = under_bytes(&folded, home) else {
        return Ok(false);
    };
    if rest.is_empty() {
        return Ok(false);
    }

    // Must not equal or contain any Atrium data/protected path.
    for d in shared.data_folded() {
        if under_bytes(d, &folded).is_some() {
            return Ok(false);
        }
    }
    for d in &protected.atrium_protected_paths {
        let df = fold_path_listed(d, protected.folds, budget)?;
        if under_bytes(&df, &folded).is_some() {
            return Ok(false);
        }
    }

    // Must not equal or contain any credential file/tree, `.ssh`, browser root, or store.
    budget.charge(
        (CRED_FILES.len()
            + CRED_TREES.len()
            + BROWSER_ROOTS.len()
            + ASK_FILES.len()
            + ASK_TREES.len()
            + ASK_NAME_PREFIXES.len()
            + 2) as u64,
    )?;
    let ssh = join_folded(home, b".ssh");
    if under_bytes(&ssh, &folded).is_some() {
        return Ok(false);
    }
    for &rel in CRED_FILES
        .iter()
        .chain(CRED_TREES.iter())
        .chain(BROWSER_ROOTS.iter())
    {
        let target = join_folded(home, rel.as_bytes());
        if under_bytes(&target, &folded).is_some() {
            return Ok(false);
        }
    }
    for &(rel, _) in ASK_FILES.iter().chain(ASK_TREES.iter()) {
        let target = join_folded(home, rel.as_bytes());
        if under_bytes(&target, &folded).is_some() {
            return Ok(false);
        }
    }
    for &(dir, pre, _) in ASK_NAME_PREFIXES {
        let dir_path = join_folded(home, dir.as_bytes());
        let prefix_path = join_folded(&dir_path, pre.as_bytes());
        if under_bytes(&prefix_path, &folded).is_some() || under_bytes(&dir_path, &folded).is_some()
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Extracts strings from a JSON payload (`Mcp`/`Unknown`) while enforcing
/// structural caps (`MAX_USES` string count, `MAX_BYTES` total string bytes)
/// BEFORE any compute charge (D-150).
fn collect_json_strings_capped(v: &Value) -> Result<Vec<&str>, Cap> {
    let mut out: Vec<&str> = Vec::new();
    let mut total_bytes: usize = 0;
    let mut stack: Vec<&Value> = vec![v];
    while let Some(cur) = stack.pop() {
        match cur {
            Value::String(s) => {
                if out.len() >= limits::MAX_USES {
                    return Err(Cap::Uses);
                }
                total_bytes = total_bytes.saturating_add(s.len());
                if total_bytes > limits::MAX_BYTES {
                    return Err(Cap::Bytes);
                }
                out.push(s.as_str());
            }
            Value::Array(arr) => {
                if arr.len() > limits::MAX_USES.saturating_mul(2) {
                    return Err(Cap::Uses);
                }
                for item in arr.iter().rev() {
                    stack.push(item);
                }
            }
            Value::Object(map) => {
                if map.len() > limits::MAX_USES.saturating_mul(2) {
                    return Err(Cap::Uses);
                }
                for val in map.values() {
                    stack.push(val);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }
    Ok(out)
}

fn record_part_preserving(
    parts: &mut Vec<PartResult>,
    item: PartResult,
    budget: &mut ComputeBudget,
) {
    if let Err(e) = push_charged(parts, item, budget) {
        record_exhausted(parts, PartId::WholeCall, e.reason);
    }
}

fn record_exhausted(parts: &mut Vec<PartResult>, part: PartId, reason: ExhaustReason) {
    if !parts
        .iter()
        .any(|r| matches!(r.outcome, PartOutcome::Exhausted(_)))
    {
        parts.push(PartResult {
            part,
            outcome: PartOutcome::Exhausted(reason),
        });
    }
}

fn finalize_too_large(outcome: CallOutcome) -> CallEvaluation {
    let rule_coverage = RuleCoverage::default();
    let verdict = decide(&outcome, &rule_coverage, Preset::Home, false);
    CallEvaluation {
        verdict,
        outcome,
        remember_matcher: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn finalize_exhausted_or_ask(
    input: &PolicyInput<'_>,
    rules: &[Rule],
    folds: FoldRules,
    units: &[CommandUnit],
    effects: &[ToolEffects],
    targets: &[Target],
    best_effort: &BestEffortRecord,
    budget: &mut ComputeBudget,
    outcome: CallOutcome,
    home_readonly_all: bool,
) -> CallEvaluation {
    let rule_ctx = RuleEvalContext {
        check: input.check,
        preset: input.preset,
        thread_id: input.thread,
        units,
        effects,
        best_effort,
        targets,
        outcomes: &outcome.parts,
        folds: &folds,
    };
    let rule_coverage = if budget.is_exhausted() {
        RuleCoverage::default()
    } else {
        evaluate_rules(&rule_ctx, rules, budget).unwrap_or_default()
    };
    let verdict = decide(&outcome, &rule_coverage, input.preset, home_readonly_all);
    CallEvaluation {
        verdict,
        outcome,
        remember_matcher: None,
    }
}
