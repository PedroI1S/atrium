//! Rule validation, rule coverage, and "always…" matcher generation
//! (owned by P3; contract §7, §8, §11 items 4–5).

use std::path::Path;

use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::{
    Preset, Rule, RuleDecision, RuleId, RuleMatcher, RuleScope, ToolSubject,
};
use globset::{GlobBuilder, GlobMatcher};

use crate::policy::model::{
    CallOutcome, CodeExecution, CommandUnit, ComputeBudget, Exhausted, FoldRules, PartId,
    PartOutcome, RuleCoverage, RuleEvalContext, RuleValidationError, Target, TargetOrigin,
    TargetResolution, UnitIndex, UnitRuleCoverage, UnreadReason,
};
use crate::policy::tools::{
    self, CodeRunnerRow, ConditionalRunnerRow, ProgramPin, RunnerTrigger, SubcommandKind,
    ToolRowKind,
};

use super::tools::{
    SedParseOutcome, charge_str_clone, fold_command_str_pure, inspect_sed_script,
    vec_push_budgeted, vec_with_exact_budgeted, word_has_unquoted_glob_budgeted,
};

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// Public P3 entry point 1: `validate_rule_matcher` (§7, §11 items 4–5)
// ---------------------------------------------------------------------------

/// Validates a rule matcher against §7's code-runner pinning rules (rejects
/// Allow prefixes ending at a code runner with no pinned program, e.g.
/// `["awk"]`, `["python"]`, `["pipx", "run"]`, `["git"]`).
///
/// Used both at rule creation (§11 item 4) and when evaluating stored rules so
/// newly rejected stored rules stop covering anything (§11 item 5).
///
/// **Owned by:** P3 · **Contract:** §7.
pub(crate) fn validate_rule_matcher(
    matcher: &RuleMatcher,
    decision: RuleDecision,
    folds: &FoldRules,
) -> Result<(), RuleValidationError> {
    match matcher {
        RuleMatcher::Any => Ok(()),
        RuleMatcher::PathGlob { glob } => validate_glob_syntax(glob),
        RuleMatcher::CommandExact { argv } => {
            if argv.is_empty() || argv[0].is_empty() {
                return Err(RuleValidationError::EmptyArgv);
            }
            Ok(())
        }
        RuleMatcher::CommandPrefix { argv } => {
            if argv.is_empty() || argv[0].is_empty() {
                return Err(RuleValidationError::EmptyArgv);
            }
            if decision == RuleDecision::Deny {
                return Ok(());
            }
            validate_allow_command_prefix(argv, folds)
        }
    }
}

/// Formats a [`RuleValidationError`] into a plain user-facing explanation
/// suitable for the daemon (`ErrorCode::Invalid`) and the rules editor.
pub(crate) fn format_validation_error(err: &RuleValidationError) -> String {
    match err {
        RuleValidationError::EmptyArgv => "The rule's command is empty.".to_owned(),
        RuleValidationError::UnpinnedCodeRunner { tool, required_pin } => {
            format!("An Allow prefix rule can't end at `{tool}` without {required_pin}.")
        }
        RuleValidationError::InvalidGlob { glob } => {
            format!("Invalid glob syntax: `{glob}`.")
        }
    }
}

/// Validates a rule matcher and optional `path_scope` under §7, returning a
/// plain displayable explanation on failure (used by `policy::validate_rule_matcher`,
/// D-153).
pub(crate) fn validate_rule_draft_reason(
    matcher: &RuleMatcher,
    decision: RuleDecision,
    path_scope: Option<&str>,
    folds: &FoldRules,
) -> Result<(), String> {
    validate_rule_matcher(matcher, decision, folds).map_err(|e| format_validation_error(&e))?;
    if let Some(scope) = path_scope {
        validate_glob_syntax(scope).map_err(|e| format_validation_error(&e))?;
    }
    Ok(())
}

fn validate_glob_syntax(glob: &str) -> Result<(), RuleValidationError> {
    if glob.is_empty() {
        return Err(RuleValidationError::InvalidGlob {
            glob: glob.to_owned(),
        });
    }
    GlobBuilder::new(glob)
        .literal_separator(true)
        .build()
        .map(|_| ())
        .map_err(|_| RuleValidationError::InvalidGlob {
            glob: glob.to_owned(),
        })
}

fn validate_allow_command_prefix(
    argv: &[String],
    folds: &FoldRules,
) -> Result<(), RuleValidationError> {
    let folded = fold_command_str_pure(&argv[0], folds);

    // 1. Explicit `OUTSIDE_MODEL` entries marked `code_runner_no_pin`
    //    (non-§4 shells `zsh`..`xonsh` and non-§4 wrappers `nohup`..`arch`).
    if let Some(entry) = tools::lookup_outside_model(&folded) {
        if entry.code_runner_no_pin {
            return Err(RuleValidationError::UnpinnedCodeRunner {
                tool: entry.name.to_owned(),
                required_pin: "a modeled command (shells and wrappers outside the model cannot be allowed by prefix)",
            });
        }
        return Ok(());
    }

    // 2. Normative `TOOLS` table.
    let Some(spec) = tools::lookup(&folded) else {
        return Ok(());
    };

    match spec.kind {
        ToolRowKind::NestedShell(row) => {
            if !row.code_runner_no_pin {
                return Ok(());
            }
            // Requires `[shell, <script>]` with no flags before the script.
            if argv.len() >= 2 && !argv[1].is_empty() && !argv[1].starts_with('-') {
                Ok(())
            } else {
                Err(RuleValidationError::UnpinnedCodeRunner {
                    tool: spec.name.to_owned(),
                    required_pin: "pinning a script file right after it (e.g. `sh script.sh`)",
                })
            }
        }
        ToolRowKind::CodeRunner(row) => validate_code_runner_prefix(spec, row, argv),
        ToolRowKind::ConditionalRunner(row) => validate_conditional_runner_prefix(spec, row, argv),
        ToolRowKind::HomeReadOnly(_)
        | ToolRowKind::Search(_)
        | ToolRowKind::Wrapper(_)
        | ToolRowKind::DerivedDestinationWriter(_) => Ok(()),
    }
}

fn validate_code_runner_prefix(
    spec: &'static tools::ToolSpec,
    row: CodeRunnerRow,
    argv: &[String],
) -> Result<(), RuleValidationError> {
    // Multi-subcommand code runners (`cargo`, `pipx`).
    if !row.subcommands.is_empty() {
        if argv.len() < 2 || argv[1].is_empty() || argv[1].starts_with('-') {
            return Err(RuleValidationError::UnpinnedCodeRunner {
                tool: spec.name.to_owned(),
                required_pin: "pinning a subcommand (e.g. `cargo test` or `pipx run <package>`)",
            });
        }
        if let Some(sub) = tools::subcommand(spec, &argv[1]) {
            if let SubcommandKind::CodeRunner(sr) = sub.kind {
                if !sr.code_runner_no_pin {
                    return Ok(());
                }
                if let Some(pin) = sr.program_pin
                    && pin.pin == ProgramPin::FirstOperand
                    && argv.len() >= 3
                    && !argv[2].is_empty()
                    && !argv[2].starts_with('-')
                {
                    return Ok(());
                }
                return Err(RuleValidationError::UnpinnedCodeRunner {
                    tool: format!("{} {}", spec.name, sub.name),
                    required_pin: "pinning the program or package name right after it (e.g. `pipx run ruff`)",
                });
            }
            return Ok(());
        }
        // Unlisted subcommand on a multi-subcommand code runner:
        // `cargo` pins via CommandAndSubcommand; `pipx` has no pin outside `pipx run <pkg>`.
        if row
            .program_pin
            .is_some_and(|p| p.pin == ProgramPin::CommandAndSubcommand)
        {
            return Ok(());
        }
        return Err(RuleValidationError::UnpinnedCodeRunner {
            tool: spec.name.to_owned(),
            required_pin: "pinning a supported subcommand and program (e.g. `pipx run <package>`)",
        });
    }

    if !row.code_runner_no_pin {
        return Ok(());
    }

    let Some(pin) = row.program_pin else {
        return Err(RuleValidationError::UnpinnedCodeRunner {
            tool: spec.name.to_owned(),
            required_pin: "a pinned program (this code runner has no pinnable prefix form; use an exact command rule instead)",
        });
    };

    match pin.pin {
        ProgramPin::FirstOperand => {
            if argv.len() >= 2 && !argv[1].is_empty() && !argv[1].starts_with('-') {
                Ok(())
            } else {
                Err(RuleValidationError::UnpinnedCodeRunner {
                    tool: spec.name.to_owned(),
                    required_pin: "pinning a script or package right after it (e.g. `python build.py`)",
                })
            }
        }
        ProgramPin::FlagValue => {
            if argv.len() >= 3
                && pin.flags.contains(&argv[1].as_str())
                && !argv[2].is_empty()
                && argv[2] != "-"
                && !has_competing_program_flags_pure(&argv[3..], pin.flags)
            {
                Ok(())
            } else {
                Err(RuleValidationError::UnpinnedCodeRunner {
                    tool: spec.name.to_owned(),
                    required_pin: "pinning a program file with `-f <file>` or `--file <file>`",
                })
            }
        }
        ProgramPin::CommandAndSubcommand => {
            if argv.len() >= 2 && !argv[1].is_empty() && !argv[1].starts_with('-') {
                Ok(())
            } else {
                Err(RuleValidationError::UnpinnedCodeRunner {
                    tool: spec.name.to_owned(),
                    required_pin: "pinning a subcommand",
                })
            }
        }
    }
}

fn has_competing_program_flags_pure(rest: &[String], pin_flags: &[&str]) -> bool {
    for tok in rest {
        if tok == "--" {
            break;
        }
        if !tok.starts_with('-') || tok == "-" {
            continue;
        }
        if tools::is_forbidden_flag(pin_flags, tok)
            || tools::is_forbidden_flag(&["-e", "--source", "--eval", "--expression"], tok)
        {
            return true;
        }
    }
    false
}

fn validate_conditional_runner_prefix(
    spec: &'static tools::ToolSpec,
    row: ConditionalRunnerRow,
    argv: &[String],
) -> Result<(), RuleValidationError> {
    if !row.code_runner_no_pin {
        return Ok(());
    }

    // 1. Subcommand-bearing conditional runner (`git`).
    if !row.subcommands.is_empty() {
        if argv.len() < 2 || argv[1].is_empty() || argv[1].starts_with('-') {
            return Err(RuleValidationError::UnpinnedCodeRunner {
                tool: spec.name.to_owned(),
                required_pin: "pinning a non-code-running subcommand (e.g. `git status`)",
            });
        }
        let sub = argv[1].as_str();
        let rest = &argv[2..];
        for &trigger in row.triggers {
            match trigger {
                RunnerTrigger::Subcommand(s) if sub == s => {
                    return Err(RuleValidationError::UnpinnedCodeRunner {
                        tool: format!("{} {s}", spec.name),
                        required_pin: "a non-code-running subcommand (this subcommand runs external programs)",
                    });
                }
                RunnerTrigger::SubcommandWithSubarg { subcommand, subarg } if sub == subcommand => {
                    let first_non_flag = rest
                        .iter()
                        .take_while(|a| a.as_str() != "--")
                        .find(|a| !a.starts_with('-'))
                        .map(String::as_str);
                    if first_non_flag.is_none() || first_non_flag == Some(subarg) {
                        return Err(RuleValidationError::UnpinnedCodeRunner {
                            tool: format!("{} {subcommand}", spec.name),
                            required_pin: "pinning a non-code-running action (this prefix would cover code execution)",
                        });
                    }
                }
                RunnerTrigger::SubcommandWithFlag { subcommand, flag } if sub == subcommand => {
                    for arg in rest {
                        if arg == "--" {
                            break;
                        }
                        if matches_git_flag_pure(arg, flag) {
                            return Err(RuleValidationError::UnpinnedCodeRunner {
                                tool: format!("{} {subcommand}", spec.name),
                                required_pin: "a form without code-running flags",
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        return Ok(());
    }

    // 2. Script-command conditional runner (`sed`).
    let script_cmds = row.triggers.iter().find_map(|t| match t {
        RunnerTrigger::ScriptCommands(cs) => Some(*cs),
        _ => None,
    });
    if let Some(cmds) = script_cmds {
        if argv.len() < 2 {
            return Err(RuleValidationError::UnpinnedCodeRunner {
                tool: spec.name.to_owned(),
                required_pin: "pinning a program file (`sed -f script.sed`) or a safe script",
            });
        }
        let pin_flags: &[&str] = &["-f", "--file"];
        if pin_flags.contains(&argv[1].as_str()) {
            if argv.len() >= 3
                && !argv[2].is_empty()
                && argv[2] != "-"
                && !has_competing_program_flags_pure(&argv[3..], pin_flags)
            {
                return Ok(());
            }
            return Err(RuleValidationError::UnpinnedCodeRunner {
                tool: spec.name.to_owned(),
                required_pin: "pinning a program file (`sed -f script.sed`)",
            });
        }
        // Check inline script(s) for trigger commands (`e`, `w`, `r`, `W`, `R`).
        let mut default_budget = ComputeBudget::with_default_limits();
        if matches!(
            validate_sed_inline_prefix_budgeted(argv, cmds, &mut default_budget),
            Ok(true)
        ) {
            return Ok(());
        }
        return Err(RuleValidationError::UnpinnedCodeRunner {
            tool: spec.name.to_owned(),
            required_pin: "pinning a program file (`sed -f script.sed`) or a safe inline script",
        });
    }

    // 3. Flag-trigger conditional runner with no subcommands or program pin (`find`).
    Err(RuleValidationError::UnpinnedCodeRunner {
        tool: spec.name.to_owned(),
        required_pin: "using an exact command rule (a prefix rule on `find` would allow appending `-exec`)",
    })
}

fn matches_git_flag_pure(tok: &str, trigger: &str) -> bool {
    if let Some(long) = trigger.strip_prefix("--") {
        let Some(rest) = tok.strip_prefix("--") else {
            return false;
        };
        let name = rest.split('=').next().unwrap_or(rest);
        return name == long;
    }
    if trigger.len() == 2
        && trigger.starts_with('-')
        && let Some(ch) = trigger.chars().nth(1)
        && let Some(cluster) = tok.strip_prefix('-')
        && !cluster.is_empty()
        && !tok.starts_with("--")
    {
        return cluster.contains(ch);
    }
    tok == trigger
}

fn validate_sed_inline_prefix_budgeted(
    argv: &[String],
    trigger_cmds: &[char],
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    let mut scripts: Vec<&str> = Vec::new();
    let mut first_pos: Option<&str> = None;
    let mut i = 1usize;
    let mut dd = false;
    while i < argv.len() {
        let tok = argv[i].as_str();
        budget.charge(tok.len().max(1) as u64)?;
        if dd || !tok.starts_with('-') || tok == "-" {
            if first_pos.is_none() {
                first_pos = Some(tok);
            }
            i += 1;
            continue;
        }
        if tok == "--" {
            dd = true;
            i += 1;
            continue;
        }
        if let Some(rest) = tok.strip_prefix("--") {
            let (name, val) = match rest.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (rest, None),
            };
            match name {
                "expression" => {
                    if let Some(v) = val {
                        vec_push_budgeted(&mut scripts, v, budget)?;
                        i += 1;
                    } else if i + 1 < argv.len() {
                        vec_push_budgeted(&mut scripts, argv[i + 1].as_str(), budget)?;
                        i += 2;
                    } else {
                        return Ok(false);
                    }
                }
                "quiet" | "silent" | "regexp-extended" | "separate" | "unbuffered"
                | "null-data" | "posix" | "sandbox" => {
                    if val.is_some() {
                        return Ok(false);
                    }
                    i += 1;
                }
                _ => return Ok(false),
            }
        } else {
            let mut consumed_next = false;
            for (byte_off, ch) in tok[1..].char_indices() {
                let rest_start = 1 + byte_off + ch.len_utf8();
                match ch {
                    'e' => {
                        if rest_start < tok.len() {
                            vec_push_budgeted(&mut scripts, &tok[rest_start..], budget)?;
                        } else if i + 1 < argv.len() {
                            vec_push_budgeted(&mut scripts, argv[i + 1].as_str(), budget)?;
                            consumed_next = true;
                        } else {
                            return Ok(false);
                        }
                        break;
                    }
                    'n' | 'E' | 'r' | 's' | 'u' | 'z' => {}
                    _ => return Ok(false),
                }
            }
            i += if consumed_next { 2 } else { 1 };
        }
    }

    if scripts.is_empty() {
        let Some(s) = first_pos else {
            return Ok(false);
        };
        vec_push_budgeted(&mut scripts, s, budget)?;
    }

    for s in scripts {
        match inspect_sed_script(s, trigger_cmds, budget)? {
            SedParseOutcome::Safe => {}
            SedParseOutcome::Triggered | SedParseOutcome::Uncertain => return Ok(false),
        }
    }
    Ok(true)
}

// ---------------------------------------------------------------------------
// Public P3 entry point 2: `evaluate_rules` (§1 step 3, §2, §7, §8)
// ---------------------------------------------------------------------------

struct UnitEvalInfo<'a> {
    simple_ok: bool,
    has_plain_unread: bool,
    has_exhausted: bool,
    pinned_program_words: Option<&'a [String]>,
    wrapper_words: usize,
}

fn wrapper_word_count(unit: &CommandUnit, budget: &mut ComputeBudget) -> Result<usize, Exhausted> {
    budget.charge(unit.removed_wrappers.len().max(1) as u64)?;
    let mut count = 0usize;
    for w in &unit.removed_wrappers {
        count = count.saturating_add(w.words.len());
    }
    Ok(count)
}

struct CallOutcomeFlags {
    whole_call_plain_unread: bool,
    whole_call_exhausted: bool,
    non_cmd_plain_unread: bool,
    non_cmd_exhausted: bool,
    any_exhausted: bool,
}

fn part_unit_index(part: &PartId) -> Option<UnitIndex> {
    match part {
        PartId::Unit(u) | PartId::Operand { unit: u, .. } | PartId::Redirect { unit: u, .. } => {
            Some(*u)
        }
        PartId::Filter { unit } => *unit,
        PartId::WholeCall | PartId::SubjectPath { .. } | PartId::BestEffortUnit(_) => None,
    }
}

fn find_unit_slot(
    units: &[CommandUnit],
    uid: UnitIndex,
    budget: &mut ComputeBudget,
) -> Result<Option<usize>, Exhausted> {
    budget.charge(1)?;
    if let Some(u) = units.get(uid.0)
        && u.index == uid
    {
        return Ok(Some(uid.0));
    }
    budget.charge(units.len().max(1) as u64)?;
    Ok(units.iter().position(|u| u.index == uid))
}

/// Evaluates `rules` against a call under `dev`'s precedence (thread > Home >
/// global, Deny before Allow within each rank): Deny rules match exact and
/// best-effort units; Allow rules cover `Clear`, `StoreHit`, or pinned
/// `Unread (program)` units meeting simple-command and canonical path-scope
/// conditions.
///
/// **Owned by:** P3 · **Contract:** §1 step 3, §7, §8, D-153.
pub(crate) fn evaluate_rules(
    call: &RuleEvalContext<'_>,
    rules: &[Rule],
    budget: &mut ComputeBudget,
) -> Result<RuleCoverage, Exhausted> {
    let is_command = matches!(call.check.subject, ToolSubject::Command { .. });
    let is_unknown = matches!(call.check.subject, ToolSubject::Unknown);
    let eff_kind = if is_command {
        ToolKind::Execute
    } else {
        call.check.kind
    };

    let n = if is_command {
        call.units.len().max(1)
    } else {
        1
    };
    budget.charge(n as u64)?;
    let mut per_unit = vec_with_exact_budgeted::<UnitRuleCoverage>(n, budget)?;
    for _ in 0..n {
        per_unit.push(UnitRuleCoverage::default());
    }

    // Pre-compute per-unit simple-command, P1/P2/P3 outcome, target, and
    // program-pin facts once (all scans charged before running; §7, §8, D-153).
    let mut unit_info = vec_with_exact_budgeted::<UnitEvalInfo<'_>>(call.units.len(), budget)?;
    for (idx, u) in call.units.iter().enumerate() {
        let simple_ok = is_unit_simple_for_allow(u, budget)?;
        let wrapper_words = wrapper_word_count(u, budget)?;
        let eff = call.effects.get(idx);
        let mut has_plain_unread = false;
        let mut pinned_program_words: Option<&[String]> = None;
        if let Some(e) = eff {
            budget.charge(e.unread.len().max(1) as u64)?;
            for reason in &e.unread {
                if !matches!(reason, UnreadReason::Program { .. }) {
                    has_plain_unread = true;
                    break;
                }
            }
            if let CodeExecution::Program { program_words, .. } = &e.code {
                pinned_program_words = Some(program_words.as_slice());
            }
        }
        unit_info.push(UnitEvalInfo {
            simple_ok,
            has_plain_unread,
            has_exhausted: false,
            pinned_program_words,
            wrapper_words,
        });
    }

    let mut whole_call_plain_unread = false;
    let mut whole_call_exhausted = false;
    let mut non_cmd_plain_unread = false;
    let mut non_cmd_exhausted = false;

    // Incorporate prior P1/P2/P3 outcomes (`call.outcomes`, D-153).
    budget.charge(call.outcomes.len().max(1) as u64)?;
    for pr in call.outcomes {
        match &pr.outcome {
            PartOutcome::Exhausted(_) => {
                if is_command && let Some(uid) = part_unit_index(&pr.part) {
                    if let Some(slot) = find_unit_slot(call.units, uid, budget)? {
                        unit_info[slot].has_exhausted = true;
                    } else {
                        whole_call_exhausted = true;
                    }
                } else if !is_command
                    && matches!(
                        pr.part,
                        PartId::SubjectPath { .. } | PartId::Filter { unit: None }
                    )
                {
                    non_cmd_exhausted = true;
                } else {
                    whole_call_exhausted = true;
                }
            }
            PartOutcome::Unread(UnreadReason::Program { program_words }) => {
                if is_command
                    && let Some(uid) = part_unit_index(&pr.part)
                    && let Some(slot) = find_unit_slot(call.units, uid, budget)?
                    && unit_info[slot].pinned_program_words.is_none()
                {
                    unit_info[slot].pinned_program_words = Some(program_words.as_slice());
                } else if !is_command {
                    non_cmd_plain_unread = true;
                } else if part_unit_index(&pr.part).is_none() {
                    whole_call_plain_unread = true;
                }
            }
            PartOutcome::Unread(_) => {
                if is_command && let Some(uid) = part_unit_index(&pr.part) {
                    if let Some(slot) = find_unit_slot(call.units, uid, budget)? {
                        unit_info[slot].has_plain_unread = true;
                    } else {
                        whole_call_plain_unread = true;
                    }
                } else if !is_command
                    && matches!(
                        pr.part,
                        PartId::SubjectPath { .. } | PartId::Filter { unit: None }
                    )
                {
                    non_cmd_plain_unread = true;
                } else {
                    whole_call_plain_unread = true;
                }
            }
            PartOutcome::HardDenyHit(_)
            | PartOutcome::StoreHit(_)
            | PartOutcome::Clear
            | PartOutcome::TooLarge(_) => {}
        }
    }

    // Incorporate target-level `Unread` and `variant_only` outcomes from P2
    // (`call.targets`, §7, D-153).
    budget.charge(call.targets.len().max(1) as u64)?;
    for t in call.targets {
        if matches!(t.resolution, TargetResolution::Unread(_)) || target_is_variant_only(&t.origin)
        {
            if is_command && let Some(uid) = target_unit_index(&t.origin) {
                if let Some(slot) = find_unit_slot(call.units, uid, budget)? {
                    unit_info[slot].has_plain_unread = true;
                } else {
                    whole_call_plain_unread = true;
                }
            } else if !is_command {
                non_cmd_plain_unread = true;
            } else {
                whole_call_plain_unread = true;
            }
        }
    }

    // Clear pinned_program_words whenever the same unit also has a plain Unread
    // or Exhausted outcome (§1 step 6: a pinned rule authorizes ONLY the
    // Unread (program) reason, never another reason on the same unit).
    budget.charge(unit_info.len().max(1) as u64)?;
    let mut any_unit_exhausted = false;
    for info in &mut unit_info {
        if info.has_exhausted {
            any_unit_exhausted = true;
        }
        if info.has_plain_unread || info.has_exhausted {
            info.pinned_program_words = None;
        }
    }

    let flags = CallOutcomeFlags {
        whole_call_plain_unread,
        whole_call_exhausted,
        non_cmd_plain_unread,
        non_cmd_exhausted,
        any_exhausted: whole_call_exhausted || non_cmd_exhausted || any_unit_exhausted,
    };

    let mut unscoped_any_rule: Option<RuleId> = None;

    // Precedence ranks: 0 = Thread, 1 = Home, 2 = Global (`dev`'s `apply_rules`).
    for rank in 0..3u8 {
        budget.charge(rules.len().max(1) as u64)?;

        let all_covered = all_units_and_be_covered(
            call,
            is_command,
            &flags,
            &per_unit,
            &unscoped_any_rule,
            budget,
        )?;
        if all_covered {
            break;
        }

        // 1. Deny rules at this rank (checked BEFORE Allow at the same rank;
        //    because the whole call is still uncovered, match against EVERY
        //    exact and best-effort unit, preserving `dev`'s call-level
        //    suppression semantics).
        for rule in rules {
            if rule.decision != RuleDecision::Deny {
                continue;
            }
            if scope_rank_budgeted(rule, call, budget)? != Some(rank) {
                continue;
            }
            if !rule_name_matches_budgeted(rule, eff_kind, &call.check.name, budget)? {
                continue;
            }
            if !rule_is_valid_budgeted(rule, call.folds, budget)? {
                continue;
            }
            // Reserve & charge storage for the Deny result BEFORE matching so
            // committing an established Deny cannot fail with Exhausted (§1, §8).
            let deny_label = deny_reason_label(&rule.matcher);
            let reason_len = deny_label.len();
            let id_len = rule.id.0.len();
            budget.charge((reason_len.max(1) as u64).saturating_add(id_len.max(1) as u64))?;
            let alloc_bytes = reason_len.saturating_add(id_len);
            if alloc_bytes > 0 {
                budget.charge_alloc(alloc_bytes)?;
            }
            if deny_rule_matches(rule, call, is_command, budget)? {
                let mut reason = String::with_capacity(reason_len);
                reason.push_str(deny_label);
                let mut id = String::with_capacity(id_len);
                id.push_str(&rule.id.0);
                return Ok(RuleCoverage {
                    deny: Some((reason, RuleId(id))),
                    per_unit,
                    unscoped_any_rule,
                });
            }
        }

        // 2. Allow rules at this rank (never cover `ToolSubject::Unknown`).
        if is_unknown {
            continue;
        }
        for rule in rules {
            if rule.decision != RuleDecision::Allow {
                continue;
            }
            if scope_rank_budgeted(rule, call, budget)? != Some(rank) {
                continue;
            }
            if !rule_name_matches_budgeted(rule, eff_kind, &call.check.name, budget)? {
                continue;
            }
            if !rule_is_valid_budgeted(rule, call.folds, budget)? {
                // §11 item 5: stored rules rejected by rev 4 validation stop covering anything.
                continue;
            }
            apply_allow_rule(
                rule,
                call,
                is_command,
                &flags,
                &unit_info,
                &mut per_unit,
                &mut unscoped_any_rule,
                budget,
            )?;
        }
    }

    Ok(RuleCoverage {
        deny: None,
        per_unit,
        unscoped_any_rule,
    })
}

fn is_unit_simple_for_allow(
    unit: &CommandUnit,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    if unit.argv.is_empty() {
        return Ok(false);
    }
    for w in unit.original_argv() {
        if word_has_unquoted_glob_budgeted(w, budget)? {
            return Ok(false);
        }
    }
    budget.charge(unit.redirects.len().max(1) as u64)?;
    for r in &unit.redirects {
        if let Some(t) = r.target() {
            budget.charge(t.len().max(1) as u64)?;
        }
        if !r.is_harmless_for_rules() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn scope_rank_budgeted(
    rule: &Rule,
    call: &RuleEvalContext<'_>,
    budget: &mut ComputeBudget,
) -> Result<Option<u8>, Exhausted> {
    match &rule.scope {
        RuleScope::Thread { thread_id } => {
            budget.charge(thread_id.0.len().max(1) as u64)?;
            Ok((thread_id == call.thread_id).then_some(0))
        }
        RuleScope::Home => {
            budget.charge(1)?;
            Ok((call.preset == Preset::Home).then_some(1))
        }
        RuleScope::Global => {
            budget.charge(1)?;
            Ok(Some(2))
        }
    }
}

fn rule_name_matches_budgeted(
    rule: &Rule,
    eff_kind: ToolKind,
    check_name: &str,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    budget.charge(1)?;
    if rule.tool_kind != eff_kind {
        return Ok(false);
    }
    match &rule.tool_name {
        None => Ok(true),
        Some(n) => {
            budget.charge(n.len().min(check_name.len()).max(1) as u64)?;
            Ok(n == check_name)
        }
    }
}

/// Validates a stored rule under §7 (charging budget before validation and
/// glob syntax checking) so stored invalid rules stop covering anything
/// (§11 item 5).
fn rule_is_valid_budgeted(
    rule: &Rule,
    folds: &FoldRules,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    match &rule.matcher {
        RuleMatcher::Any => budget.charge(1)?,
        RuleMatcher::CommandPrefix { argv } | RuleMatcher::CommandExact { argv } => {
            let mut cost = argv.len().max(1) as u64;
            let mut alloc = 0usize;
            for w in argv {
                cost = cost.saturating_add((w.len() as u64).saturating_mul(4));
                alloc = alloc.saturating_add(w.len().saturating_mul(4));
            }
            // Charge for table scan on command word + any sed script vector.
            cost = cost
                .saturating_add((tools::all_tools().len() + tools::outside_model().len()) as u64);
            alloc = alloc.saturating_add(argv.len().saturating_mul(std::mem::size_of::<&str>()));
            budget.charge(cost)?;
            if alloc > 0 {
                budget.charge_alloc(alloc)?;
            }
        }
        RuleMatcher::PathGlob { glob } => {
            let len = glob.len().max(1);
            budget.charge((len as u64).saturating_mul(8))?;
            budget.charge_alloc(len.saturating_mul(16))?;
        }
    }
    if validate_rule_matcher(&rule.matcher, rule.decision, folds).is_err() {
        return Ok(false);
    }
    if let Some(scope) = &rule.path_scope {
        let len = scope.len().max(1);
        budget.charge((len as u64).saturating_mul(8))?;
        budget.charge_alloc(len.saturating_mul(16))?;
        if validate_glob_syntax(scope).is_err() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn compile_glob_budgeted(
    glob: &str,
    budget: &mut ComputeBudget,
) -> Result<Option<GlobMatcher>, Exhausted> {
    let len = glob.len().max(1);
    budget.charge((len as u64).saturating_mul(8))?;
    budget.charge_alloc(len.saturating_mul(32))?;
    Ok(GlobBuilder::new(glob)
        .literal_separator(true)
        .build()
        .ok()
        .map(|g| g.compile_matcher()))
}

fn match_canonical_bytes(
    matcher: &GlobMatcher,
    canonical: &[u8],
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    budget.charge(canonical.len().max(1) as u64)?;
    #[cfg(unix)]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        Ok(matcher.is_match(Path::new(OsStr::from_bytes(canonical))))
    }
    #[cfg(not(unix))]
    {
        Ok(std::str::from_utf8(canonical).is_ok_and(|s| matcher.is_match(Path::new(s))))
    }
}

fn target_unit_index(origin: &TargetOrigin) -> Option<UnitIndex> {
    match origin {
        TargetOrigin::Operand { unit, .. } | TargetOrigin::Redirect { unit, .. } => Some(*unit),
        TargetOrigin::ImplicitCwd { unit } => *unit,
        TargetOrigin::SubjectPath { .. } => None,
        TargetOrigin::ExpandedFrom { source, .. } => match source {
            PartId::Unit(u)
            | PartId::Operand { unit: u, .. }
            | PartId::Redirect { unit: u, .. } => Some(*u),
            PartId::Filter { unit } => *unit,
            PartId::WholeCall | PartId::SubjectPath { .. } | PartId::BestEffortUnit(_) => None,
        },
    }
}

fn target_is_variant_only(origin: &TargetOrigin) -> bool {
    matches!(
        origin,
        TargetOrigin::ExpandedFrom {
            variant_only: true,
            ..
        }
    )
}

/// Checks canonical path targets for an **Allow** `path_scope` or `PathGlob`:
/// requires at least one target, ZERO `Unread` or `variant_only` targets, and
/// EVERY canonical target matching `matcher` (§7, M1-29 round 4 findings 2–3).
fn all_targets_match_glob(
    targets: &[Target],
    unit_filter: Option<UnitIndex>,
    matcher: &GlobMatcher,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    budget.charge(targets.len().max(1) as u64)?;
    let mut count = 0usize;
    for t in targets {
        if let Some(u) = unit_filter
            && target_unit_index(&t.origin) != Some(u)
        {
            continue;
        }
        count += 1;
        if target_is_variant_only(&t.origin) {
            return Ok(false);
        }
        match &t.resolution {
            TargetResolution::Resolved { canonical, .. } => {
                if !match_canonical_bytes(matcher, canonical, budget)? {
                    return Ok(false);
                }
            }
            TargetResolution::Unread(_) => return Ok(false),
        }
    }
    Ok(count > 0)
}

/// Checks canonical path targets for a **Deny** `PathGlob` matcher:
/// fail-closed if ANY resolved canonical target matches `matcher`.
fn deny_targets_match_glob(
    targets: &[Target],
    matcher: &GlobMatcher,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    budget.charge(targets.len().max(1) as u64)?;
    for t in targets {
        if target_is_variant_only(&t.origin) {
            continue;
        }
        if let TargetResolution::Resolved { canonical, .. } = &t.resolution
            && match_canonical_bytes(matcher, canonical, budget)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Checks `path_scope` on a Deny rule: requires EVERY canonical target to be
/// resolved (zero `Unread` or `variant_only` targets) and inside `path_scope`
/// (`dev`'s `path_scope_ok` semantics and §7's Unread-target condition).
fn deny_path_scope_ok(
    path_scope: Option<&str>,
    targets: &[Target],
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    let Some(scope) = path_scope else {
        return Ok(true);
    };
    let Some(matcher) = compile_glob_budgeted(scope, budget)? else {
        return Ok(false);
    };
    all_targets_match_glob(targets, None, &matcher, budget)
}

fn deny_rule_matches(
    rule: &Rule,
    call: &RuleEvalContext<'_>,
    is_command: bool,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    match &rule.matcher {
        RuleMatcher::Any => deny_path_scope_ok(rule.path_scope.as_deref(), call.targets, budget),
        RuleMatcher::CommandPrefix { argv } => {
            if !is_command {
                return Ok(false);
            }
            if !deny_command_matches(argv, true, call, budget)? {
                return Ok(false);
            }
            deny_path_scope_ok(rule.path_scope.as_deref(), call.targets, budget)
        }
        RuleMatcher::CommandExact { argv } => {
            if !is_command {
                return Ok(false);
            }
            if !deny_command_matches(argv, false, call, budget)? {
                return Ok(false);
            }
            deny_path_scope_ok(rule.path_scope.as_deref(), call.targets, budget)
        }
        RuleMatcher::PathGlob { glob } => {
            let Some(matcher) = compile_glob_budgeted(glob, budget)? else {
                return Ok(false);
            };
            if !deny_targets_match_glob(call.targets, &matcher, budget)? {
                return Ok(false);
            }
            deny_path_scope_ok(rule.path_scope.as_deref(), call.targets, budget)
        }
    }
}

/// Checks whether a Deny `CommandPrefix` (`is_prefix == true`) or
/// `CommandExact` (`is_prefix == false`) matches ANY exact unit OR ANY
/// best-effort unit while the call remains uncovered (§7: "a Deny rule matches
/// exact **and** best-effort units").
fn deny_command_matches(
    rule_argv: &[String],
    is_prefix: bool,
    call: &RuleEvalContext<'_>,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    if rule_argv.is_empty() {
        return Ok(false);
    }

    // 1. Every exact modeled unit (call-level suppression handles the
    //    whole-call covered case before Deny rules run).
    budget.charge(call.units.len().max(1) as u64)?;
    for u in call.units {
        if words_match_argv(u.argv.iter(), rule_argv, is_prefix, budget)?
            || words_match_argv(u.original_argv(), rule_argv, is_prefix, budget)?
        {
            return Ok(true);
        }
    }

    // 2. Every best-effort unit.
    budget.charge(call.best_effort.units.len().max(1) as u64)?;
    for be in &call.best_effort.units {
        if strings_match_argv(&be.effective_argv, rule_argv, is_prefix, budget)?
            || strings_match_argv(&be.raw_argv, rule_argv, is_prefix, budget)?
        {
            return Ok(true);
        }
        budget.charge(be.alt_argv.len().max(1) as u64)?;
        for alt in &be.alt_argv {
            if strings_match_argv(alt, rule_argv, is_prefix, budget)? {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

/// Returns `true` iff `be` has no alternate interpretations (`alt_argv`) and
/// matches an exact modeled unit whose `per_unit` slot is already covered by a
/// higher-ranked Allow rule.
fn be_unit_covered_by_exact(
    be: &crate::policy::model::BestEffortUnit,
    units: &[CommandUnit],
    per_unit: &[UnitRuleCoverage],
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    if !be.alt_argv.is_empty() {
        return Ok(false);
    }
    budget.charge(units.len().max(1) as u64)?;
    for (idx, u) in units.iter().enumerate() {
        if per_unit.get(idx).is_none_or(|c| c.allow_rule.is_none()) {
            continue;
        }
        if words_match_argv(u.original_argv(), &be.raw_argv, false, budget)?
            && words_match_argv(u.argv.iter(), &be.effective_argv, false, budget)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Returns `true` when higher-ranked Allow rules have already covered the
/// ENTIRE call (every exact unit and every best-effort unit, with no
/// uncovered `Unread` or `Exhausted` part), so no lower-ranked rule (Deny or
/// Allow) can affect the result.
fn all_units_and_be_covered(
    call: &RuleEvalContext<'_>,
    is_command: bool,
    flags: &CallOutcomeFlags,
    per_unit: &[UnitRuleCoverage],
    unscoped_any_rule: &Option<RuleId>,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    // An Exhausted part can never be covered or suppress a lower-ranked Deny.
    if flags.any_exhausted {
        return Ok(false);
    }
    if unscoped_any_rule.is_some() {
        return Ok(true);
    }
    if flags.whole_call_plain_unread {
        return Ok(false);
    }
    if is_command && call.units.is_empty() {
        return Ok(false);
    }
    budget.charge(per_unit.len().max(1) as u64)?;
    if !per_unit.iter().all(|c| c.allow_rule.is_some()) {
        return Ok(false);
    }
    if is_command {
        budget.charge(call.best_effort.units.len().max(1) as u64)?;
        for be in &call.best_effort.units {
            if !be_unit_covered_by_exact(be, call.units, per_unit, budget)? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn words_match_argv<'a>(
    words: impl Iterator<Item = &'a crate::policy::model::Word>,
    rule_argv: &[String],
    is_prefix: bool,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    let mut iter = words;
    for expected in rule_argv {
        let Some(actual) = iter.next() else {
            budget.charge(1)?;
            return Ok(false);
        };
        let b = actual.literal_bytes();
        budget.charge(b.len().min(expected.len()).max(1) as u64)?;
        if b != expected.as_bytes() {
            return Ok(false);
        }
    }
    if is_prefix {
        Ok(true)
    } else {
        budget.charge(1)?;
        Ok(iter.next().is_none())
    }
}

fn strings_match_argv(
    actual: &[String],
    rule_argv: &[String],
    is_prefix: bool,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    budget.charge(1)?;
    if is_prefix {
        if actual.len() < rule_argv.len() {
            return Ok(false);
        }
    } else if actual.len() != rule_argv.len() {
        return Ok(false);
    }
    for (a, b) in actual.iter().zip(rule_argv.iter()) {
        budget.charge(a.len().min(b.len()).max(1) as u64)?;
        if a != b {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Checks whether `rule_argv` (which already matched `unit.original_argv()`)
/// names the full pinned program `prog_words` starting immediately after the
/// unit's `wrapper_offset` removed wrapper words (§7, §11 item 8).
fn rule_argv_names_pinned_program(
    rule_argv: &[String],
    wrapper_offset: usize,
    prog_words: &[String],
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    let Some(after_wrappers) = rule_argv.get(wrapper_offset..) else {
        budget.charge(1)?;
        return Ok(false);
    };
    if after_wrappers.len() < prog_words.len() {
        budget.charge(1)?;
        return Ok(false);
    }
    strings_match_argv(after_wrappers, prog_words, true, budget)
}

#[allow(clippy::too_many_arguments)]
fn apply_allow_rule(
    rule: &Rule,
    call: &RuleEvalContext<'_>,
    is_command: bool,
    flags: &CallOutcomeFlags,
    unit_info: &[UnitEvalInfo<'_>],
    per_unit: &mut [UnitRuleCoverage],
    unscoped_any_rule: &mut Option<RuleId>,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    let scope_matcher = match &rule.path_scope {
        Some(scope) => {
            let Some(m) = compile_glob_budgeted(scope, budget)? else {
                return Ok(());
            };
            Some(m)
        }
        None => None,
    };

    match &rule.matcher {
        RuleMatcher::Any => {
            if let Some(ref scope_m) = scope_matcher {
                // Scoped Any rule requires established canonical targets (§2, §7).
                if flags.whole_call_plain_unread || flags.whole_call_exhausted {
                    return Ok(());
                }
                if is_command {
                    for (idx, u) in call.units.iter().enumerate() {
                        if per_unit[idx].allow_rule.is_some() {
                            continue;
                        }
                        if !unit_info[idx].simple_ok
                            || unit_info[idx].has_plain_unread
                            || unit_info[idx].has_exhausted
                            || unit_info[idx].pinned_program_words.is_some()
                        {
                            continue;
                        }
                        if all_targets_match_glob(call.targets, Some(u.index), scope_m, budget)? {
                            per_unit[idx].allow_rule = Some(clone_rule_id(&rule.id, budget)?);
                        }
                    }
                } else if !flags.non_cmd_plain_unread
                    && !flags.non_cmd_exhausted
                    && per_unit[0].allow_rule.is_none()
                    && all_targets_match_glob(call.targets, None, scope_m, budget)?
                {
                    per_unit[0].allow_rule = Some(clone_rule_id(&rule.id, budget)?);
                }
            } else {
                // Unscoped Any rule covers Clear, StoreHit, and plain Unread,
                // but NEVER Exhausted (§2, D-153).
                if !flags.any_exhausted && unscoped_any_rule.is_none() {
                    *unscoped_any_rule = Some(clone_rule_id(&rule.id, budget)?);
                }
                for (idx, slot) in per_unit.iter_mut().enumerate() {
                    let slot_exhausted = flags.whole_call_exhausted
                        || if is_command {
                            unit_info.get(idx).is_some_and(|u| u.has_exhausted)
                        } else {
                            flags.non_cmd_exhausted
                        };
                    if slot_exhausted {
                        continue;
                    }
                    slot.unscoped_any_allow = true;
                    if slot.allow_rule.is_none() {
                        slot.allow_rule = Some(clone_rule_id(&rule.id, budget)?);
                    }
                }
            }
        }
        RuleMatcher::CommandPrefix { argv: rule_argv } => {
            if !is_command
                || call.units.is_empty()
                || flags.whole_call_plain_unread
                || flags.whole_call_exhausted
            {
                return Ok(());
            }
            for (idx, u) in call.units.iter().enumerate() {
                if per_unit[idx].allow_rule.is_some() {
                    continue;
                }
                if !unit_info[idx].simple_ok
                    || unit_info[idx].has_plain_unread
                    || unit_info[idx].has_exhausted
                {
                    continue;
                }
                if let Some(ref scope_m) = scope_matcher
                    && !all_targets_match_glob(call.targets, Some(u.index), scope_m, budget)?
                {
                    continue;
                }
                if !words_match_argv(u.original_argv(), rule_argv, true, budget)? {
                    continue;
                }
                if let Some(prog_words) = unit_info[idx].pinned_program_words {
                    // Rule must name the full pinned program past any removed
                    // wrapper words (§7, §11 item 8).
                    if !rule_argv_names_pinned_program(
                        rule_argv,
                        unit_info[idx].wrapper_words,
                        prog_words,
                        budget,
                    )? {
                        continue;
                    }
                    per_unit[idx].covers_pinned_program = true;
                }
                per_unit[idx].allow_rule = Some(clone_rule_id(&rule.id, budget)?);
            }
        }
        RuleMatcher::CommandExact { argv: rule_argv } => {
            if !is_command
                || call.units.is_empty()
                || flags.whole_call_plain_unread
                || flags.whole_call_exhausted
            {
                return Ok(());
            }
            for (idx, u) in call.units.iter().enumerate() {
                if per_unit[idx].allow_rule.is_some() {
                    continue;
                }
                if !unit_info[idx].simple_ok
                    || unit_info[idx].has_plain_unread
                    || unit_info[idx].has_exhausted
                {
                    continue;
                }
                if let Some(ref scope_m) = scope_matcher
                    && !all_targets_match_glob(call.targets, Some(u.index), scope_m, budget)?
                {
                    continue;
                }
                if !words_match_argv(u.original_argv(), rule_argv, false, budget)? {
                    continue;
                }
                if let Some(prog_words) = unit_info[idx].pinned_program_words {
                    // Rule must name the full pinned program past any removed
                    // wrapper words (§7, §11 item 8).
                    if !rule_argv_names_pinned_program(
                        rule_argv,
                        unit_info[idx].wrapper_words,
                        prog_words,
                        budget,
                    )? {
                        continue;
                    }
                    per_unit[idx].covers_pinned_program = true;
                }
                per_unit[idx].allow_rule = Some(clone_rule_id(&rule.id, budget)?);
            }
        }
        RuleMatcher::PathGlob { glob } => {
            if flags.whole_call_plain_unread || flags.whole_call_exhausted {
                return Ok(());
            }
            let Some(glob_m) = compile_glob_budgeted(glob, budget)? else {
                return Ok(());
            };
            if is_command {
                for (idx, u) in call.units.iter().enumerate() {
                    if per_unit[idx].allow_rule.is_some() {
                        continue;
                    }
                    if !unit_info[idx].simple_ok
                        || unit_info[idx].has_plain_unread
                        || unit_info[idx].has_exhausted
                        || unit_info[idx].pinned_program_words.is_some()
                    {
                        continue;
                    }
                    if !all_targets_match_glob(call.targets, Some(u.index), &glob_m, budget)? {
                        continue;
                    }
                    if let Some(ref scope_m) = scope_matcher
                        && !all_targets_match_glob(call.targets, Some(u.index), scope_m, budget)?
                    {
                        continue;
                    }
                    per_unit[idx].allow_rule = Some(clone_rule_id(&rule.id, budget)?);
                }
            } else if !flags.non_cmd_plain_unread
                && !flags.non_cmd_exhausted
                && per_unit[0].allow_rule.is_none()
                && all_targets_match_glob(call.targets, None, &glob_m, budget)?
                && match &scope_matcher {
                    Some(scope_m) => all_targets_match_glob(call.targets, None, scope_m, budget)?,
                    None => true,
                }
            {
                per_unit[0].allow_rule = Some(clone_rule_id(&rule.id, budget)?);
            }
        }
    }

    Ok(())
}

fn clone_rule_id(id: &RuleId, budget: &mut ComputeBudget) -> Result<RuleId, Exhausted> {
    Ok(RuleId(charge_str_clone(&id.0, budget)?))
}

fn deny_reason_label(matcher: &RuleMatcher) -> &'static str {
    match matcher {
        RuleMatcher::Any => "denied by a rule (Any)",
        RuleMatcher::CommandPrefix { .. } => "denied by a rule (CommandPrefix)",
        RuleMatcher::CommandExact { .. } => "denied by a rule (CommandExact)",
        RuleMatcher::PathGlob { .. } => "denied by a rule (PathGlob)",
    }
}

// ---------------------------------------------------------------------------
// Public P3 entry point 3: `remember_matcher` (§7, §9)
// ---------------------------------------------------------------------------

/// Computes the "always…" matcher (`CommandExact` or `PathGlob`) for an Ask
/// call, returning `None` whenever any part of `outcome` is `Unread` or
/// `Exhausted`.
///
/// **Owned by:** P3 · **Contract:** §7, §9.
pub(crate) fn remember_matcher(
    call: &RuleEvalContext<'_>,
    outcome: &CallOutcome,
) -> Option<RuleMatcher> {
    // §7, §9: never offer "always…" when any part of the outcome is Unread or
    // Exhausted (nor for TooLarge / HardDenyHit).
    if outcome.parts.iter().any(|p| {
        matches!(
            p.outcome,
            PartOutcome::Unread(_)
                | PartOutcome::Exhausted(_)
                | PartOutcome::TooLarge(_)
                | PartOutcome::HardDenyHit(_)
        )
    }) {
        return None;
    }

    match &call.check.subject {
        ToolSubject::Command { .. } => {
            if call.units.len() != 1 {
                return None;
            }
            let u = &call.units[0];
            if u.argv.is_empty() {
                return None;
            }
            if u.original_argv()
                .any(crate::policy::model::Word::has_unquoted_glob)
            {
                return None;
            }
            if !u
                .redirects
                .iter()
                .all(crate::policy::model::ModelRedirect::is_harmless_for_rules)
            {
                return None;
            }
            let argv: Option<Vec<String>> = u
                .original_argv()
                .map(|w| w.as_str().map(str::to_owned))
                .collect();
            let argv = argv?;
            if argv.is_empty() {
                return None;
            }
            Some(RuleMatcher::CommandExact { argv })
        }
        ToolSubject::Paths { paths, .. } => {
            let file_tool = matches!(
                call.check.kind,
                ToolKind::Read
                    | ToolKind::Edit
                    | ToolKind::Delete
                    | ToolKind::Move
                    | ToolKind::Search
            );
            if !file_tool || paths.len() != 1 || call.targets.len() != 1 {
                return None;
            }
            let target = &call.targets[0];
            if target_is_variant_only(&target.origin) {
                return None;
            }
            let TargetResolution::Resolved { canonical, .. } = &target.resolution else {
                return None;
            };
            let canonical_str = std::str::from_utf8(canonical).ok()?;
            Some(RuleMatcher::PathGlob {
                glob: escape_glob_str(canonical_str),
            })
        }
        ToolSubject::Fetch { .. }
        | ToolSubject::Mcp { .. }
        | ToolSubject::None
        | ToolSubject::Unknown => None,
    }
}

/// Escapes glob metacharacters so a remembered `PathGlob` matches only `path`.
fn escape_glob_str(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if matches!(c, '*' | '?' | '[' | ']' | '{' | '}' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}
