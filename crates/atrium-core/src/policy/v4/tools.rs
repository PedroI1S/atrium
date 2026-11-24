//! Tool-table application and Git name-selection checks (owned by P3;
//! contract §6, §8, D-141).

use std::mem::size_of;

use crate::policy::model::{
    AccessMode, CodeExecution, CommandUnit, ComputeBudget, DerivedDestinations, Exhausted,
    FoldRules, LinkPolicy, OperandRole, PartResult, SearchFilter, ToolEffects, UnreadReason,
    ValueRole, Word,
};
use crate::policy::tools::{
    self, CodeRunnerRow, ConditionalRunnerRow, DerivedDestinationTrigger,
    DerivedDestinationWriterRow, FilterCase, FlagKind, HomeReadOnlyRow, InlineCodeSpec,
    NestedShellRow, ProgramPin, RunnerTrigger, SearchRecursion, SearchRow, SubcommandKind,
    ToolRowKind, ToolSpec,
};

mod git;
mod sed;
#[cfg(test)]
mod tests;

pub(super) use sed::{SedParseOutcome, inspect_sed_script};

// ---------------------------------------------------------------------------
// Shared §8 budget helpers (charge BEFORE scanning, copying, or allocating)
// ---------------------------------------------------------------------------

/// Allocates a `Vec<T>` with exact capacity `len` after charging its heap
/// footprint against `budget`.
pub(super) fn vec_with_exact_budgeted<T>(
    len: usize,
    budget: &mut ComputeBudget,
) -> Result<Vec<T>, Exhausted> {
    if len > 0 {
        let bytes = len.saturating_mul(size_of::<T>());
        budget.charge_alloc(bytes)?;
    }
    Ok(Vec::with_capacity(len))
}

/// Result of CR-12 Git name-selection and wildcard-pathspec checks
/// (contract §6, §8, D-153).
///
/// Retains every `StoreHit` and `Unread(GitWildcardPathspec)` established
/// before completion or exhaustion so no earlier hit is lost if a later
/// operand exhausts [`ComputeBudget`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GitSelectionOutcome {
    pub(crate) results: Vec<PartResult>,
    pub(crate) exhausted: Option<Exhausted>,
}

/// Ensures `vec` has spare capacity for at least `additional` items without
/// reallocating later, charging element-copy compute units and growth
/// allocation bytes **before** reserving (§8).
pub(super) fn reserve_vec_slots_budgeted<T>(
    vec: &mut Vec<T>,
    additional: usize,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    let spare = vec.capacity().saturating_sub(vec.len());
    if spare >= additional {
        return Ok(());
    }
    let old_cap = vec.capacity();
    let needed_cap = vec.len().saturating_add(additional);
    let doubled = if old_cap == 0 {
        4
    } else {
        old_cap.saturating_mul(2)
    };
    let new_cap = doubled.max(needed_cap);
    let add_elems = new_cap - old_cap;
    budget.charge(vec.len().max(1) as u64)?;
    budget.charge_alloc(add_elems.saturating_mul(size_of::<T>()))?;
    vec.reserve_exact(new_cap - vec.len());
    Ok(())
}

/// Pushes `item` into `vec`, charging any capacity growth (allocation bytes +
/// element-copy compute units) **before** reallocating.
pub(super) fn vec_push_budgeted<T>(
    vec: &mut Vec<T>,
    item: T,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    if vec.len() == vec.capacity() {
        let old_cap = vec.capacity();
        let new_cap = if old_cap == 0 {
            4
        } else {
            old_cap.saturating_mul(2)
        };
        let add_elems = new_cap - old_cap;
        budget.charge(vec.len() as u64)?;
        budget.charge_alloc(add_elems.saturating_mul(size_of::<T>()))?;
        vec.reserve_exact(new_cap - vec.len());
    }
    vec.push(item);
    Ok(())
}

/// Clones `s` into an owned `String` with exact capacity after charging its
/// scan/copy compute units and allocation bytes.
pub(super) fn charge_str_clone(s: &str, budget: &mut ComputeBudget) -> Result<String, Exhausted> {
    let len = s.len();
    budget.charge(len.max(1) as u64)?;
    if len > 0 {
        budget.charge_alloc(len)?;
    }
    let mut out = String::with_capacity(len);
    out.push_str(s);
    Ok(out)
}

/// Joins `a` + `sep` + `b` into an owned `String` with exact capacity,
/// precharging both compute and allocation bytes.
pub(super) fn charge_str_join2(
    a: &str,
    sep: &str,
    b: &str,
    budget: &mut ComputeBudget,
) -> Result<String, Exhausted> {
    let len = a.len().saturating_add(sep.len()).saturating_add(b.len());
    budget.charge(len.max(1) as u64)?;
    if len > 0 {
        budget.charge_alloc(len)?;
    }
    let mut out = String::with_capacity(len);
    out.push_str(a);
    out.push_str(sep);
    out.push_str(b);
    Ok(out)
}

/// Clones a `Word` (or a byte subslice of it) preserving per-byte provenance,
/// charging both compute units and heap allocation bytes before allocating.
pub(super) fn slice_word_budgeted(
    word: &Word,
    start: usize,
    budget: &mut ComputeBudget,
) -> Result<Word, Exhausted> {
    let bytes = &word.literal_bytes()[start..];
    let prov = &word.provenance()[start..];
    let len = bytes.len();
    budget.charge((len.max(1) as u64).saturating_mul(2))?;
    // Both Vec<u8> and Vec<ByteProvenance> (1 byte each)
    budget.charge_alloc(len.saturating_mul(2))?;
    let mut b = Vec::with_capacity(len);
    b.extend_from_slice(bytes);
    let mut p = Vec::with_capacity(len);
    p.extend_from_slice(prov);
    Ok(Word::from_parts(b, p).unwrap_or_default())
}

/// Views `word` as UTF-8 after charging its byte length for the UTF-8 check.
pub(super) fn word_as_str_budgeted<'a>(
    word: &'a Word,
    budget: &mut ComputeBudget,
) -> Result<Option<&'a str>, Exhausted> {
    budget.charge(word.len().max(1) as u64)?;
    Ok(word.as_str())
}

/// Checks whether `word` has an unquoted glob character after charging its
/// byte length.
pub(super) fn word_has_unquoted_glob_budgeted(
    word: &Word,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    budget.charge(word.len().max(1) as u64)?;
    Ok(word.has_unquoted_glob())
}

/// Checks `bytes == other` after charging `min(a.len(), b.len()) + 1`.
pub(super) fn bytes_eq_budgeted(
    a: &[u8],
    b: &[u8],
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    budget.charge((a.len().min(b.len()) as u64).saturating_add(1))?;
    Ok(a == b)
}

/// CR-19 folds a raw command word (basename after last `/`, Unicode
/// `str::to_lowercase()`, optional `ſ` → `s` and `K` → `k`, with NO Unicode
/// normalization), charging compute and allocation **before** work.
///
/// Returns `Ok(None)` if the basename is not valid UTF-8.
pub(super) fn fold_command_bytes_budgeted(
    bytes: &[u8],
    folds: &FoldRules,
    budget: &mut ComputeBudget,
) -> Result<Option<String>, Exhausted> {
    // 1. Basename scan (right-to-left for '/').
    budget.charge(bytes.len().max(1) as u64)?;
    let base = match bytes.iter().rposition(|&b| b == b'/') {
        Some(pos) => &bytes[pos + 1..],
        None => bytes,
    };
    // 2. UTF-8 check.
    budget.charge(base.len().max(1) as u64)?;
    let Ok(s) = std::str::from_utf8(base) else {
        return Ok(None);
    };
    // 3. Fast ASCII check & lowercase path.
    budget.charge(s.len().max(1) as u64)?;
    if s.is_ascii() {
        budget.charge(s.len().max(1) as u64)?;
        if !s.is_empty() {
            budget.charge_alloc(s.len())?;
        }
        let mut out = String::with_capacity(s.len());
        for b in s.bytes() {
            out.push(b.to_ascii_lowercase() as char);
        }
        return Ok(Some(out));
    }
    // 4. Unicode path: string-level `str::to_lowercase()` preserves contextual
    //    rules (e.g. Greek final sigma), with no Unicode normalization.
    //    Worst-case UTF-8 lowercase expansion is 3x; charge 4x upfront.
    let bound = s.len().saturating_mul(4);
    budget.charge(bound.max(1) as u64)?;
    budget.charge_alloc(bound)?;
    let lowered = if folds.fold_kelvin {
        s.to_lowercase()
    } else {
        // Preserve U+212A (Kelvin sign) literally when `fold_kelvin == false`.
        let mut buf = String::with_capacity(s.len());
        let mut seg_start = 0;
        for (idx, ch) in s.char_indices() {
            if ch == '\u{212A}' {
                if seg_start < idx {
                    buf.push_str(&s[seg_start..idx].to_lowercase());
                }
                buf.push('\u{212A}');
                seg_start = idx + ch.len_utf8();
            }
        }
        if seg_start < s.len() {
            buf.push_str(&s[seg_start..].to_lowercase());
        }
        buf
    };
    if folds.fold_long_s && lowered.contains('\u{017F}') {
        let len = lowered.len();
        budget.charge(len.max(1) as u64)?;
        budget.charge_alloc(len)?;
        let mut out = String::with_capacity(len);
        for ch in lowered.chars() {
            if ch == '\u{017F}' {
                out.push('s');
            } else {
                out.push(ch);
            }
        }
        Ok(Some(out))
    } else {
        Ok(Some(lowered))
    }
}

/// Pure (unbudgeted) CR-19 command-word folding for `validate_rule_matcher`,
/// which has no `ComputeBudget` parameter.
pub(super) fn fold_command_str_pure(word: &str, folds: &FoldRules) -> String {
    let base = match word.rfind('/') {
        Some(pos) => &word[pos + 1..],
        None => word,
    };
    if base.is_ascii() {
        return base.to_ascii_lowercase();
    }
    let lowered = if folds.fold_kelvin {
        base.to_lowercase()
    } else {
        let mut buf = String::with_capacity(base.len());
        let mut seg_start = 0;
        for (idx, ch) in base.char_indices() {
            if ch == '\u{212A}' {
                if seg_start < idx {
                    buf.push_str(&base[seg_start..idx].to_lowercase());
                }
                buf.push('\u{212A}');
                seg_start = idx + ch.len_utf8();
            }
        }
        if seg_start < base.len() {
            buf.push_str(&base[seg_start..].to_lowercase());
        }
        buf
    };
    if folds.fold_long_s {
        lowered.replace('\u{017F}', "s")
    } else {
        lowered
    }
}

fn lookup_outside_budgeted(
    folded: &str,
    budget: &mut ComputeBudget,
) -> Result<Option<&'static tools::OutsideModelEntry>, Exhausted> {
    budget.charge(
        (tools::outside_model().len() as u64).saturating_mul(folded.len().clamp(1, 16) as u64),
    )?;
    Ok(tools::lookup_outside_model(folded))
}

fn lookup_tool_budgeted(
    folded: &str,
    budget: &mut ComputeBudget,
) -> Result<Option<&'static ToolSpec>, Exhausted> {
    budget.charge(
        (tools::all_tools().len() as u64).saturating_mul(folded.len().clamp(1, 16) as u64),
    )?;
    Ok(tools::lookup(folded))
}

fn lookup_subcommand_budgeted(
    tool: &'static ToolSpec,
    name: &str,
    budget: &mut ComputeBudget,
) -> Result<Option<&'static tools::SubcommandSpec>, Exhausted> {
    budget.charge(
        (tool.subcommands().len().max(1) as u64).saturating_mul(name.len().clamp(1, 16) as u64),
    )?;
    Ok(tools::subcommand(tool, name))
}

fn flag_short_budgeted(
    flags: &'static [tools::FlagSpec],
    short: char,
    budget: &mut ComputeBudget,
) -> Result<Option<&'static tools::FlagSpec>, Exhausted> {
    budget.charge(flags.len().max(1) as u64)?;
    Ok(tools::flag_short(flags, short))
}

fn flag_long_budgeted(
    flags: &'static [tools::FlagSpec],
    long: &str,
    budget: &mut ComputeBudget,
) -> Result<Option<&'static tools::FlagSpec>, Exhausted> {
    budget.charge((flags.len().max(1) as u64).saturating_mul(long.len().clamp(1, 32) as u64))?;
    Ok(tools::flag_long(flags, long))
}

fn is_forbidden_flag_budgeted(
    forbidden: &[&str],
    token: &str,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    if forbidden.is_empty() {
        return Ok(false);
    }
    budget.charge((forbidden.len() as u64).saturating_mul(token.len().max(1) as u64))?;
    Ok(tools::is_forbidden_flag(forbidden, token))
}

// ---------------------------------------------------------------------------
// Public P3 entry point 1: `classify_unit` (§6, §8)
// ---------------------------------------------------------------------------

/// Applies `policy::tools`'s normative table to one modeled [`CommandUnit`],
/// assigning per-position [`OperandRole`]s, detecting unknown flags,
/// computing write/recursive/link/code effects, extracting search filters, and
/// checking Home read-only eligibility on the command word as written.
///
/// **Owned by:** P3 · **Contract:** §6, §8.
pub(crate) fn classify_unit(
    unit: &CommandUnit,
    folds: &FoldRules,
    budget: &mut ComputeBudget,
) -> Result<ToolEffects, Exhausted> {
    // 1. Check redirects (charged before work).
    let mut redirect_writes = false;
    let mut all_redirects_harmless = true;
    budget.charge(unit.redirects.len().max(1) as u64)?;
    for redir in &unit.redirects {
        if let Some(target) = redir.target() {
            budget.charge(target.len().max(1) as u64)?;
        }
        if redir.writes_outside_dev_null() {
            redirect_writes = true;
        }
        if !redir.is_harmless_for_rules() {
            all_redirects_harmless = false;
        }
    }

    if unit.argv.is_empty() {
        return Ok(ToolEffects {
            tool_name: None,
            subcommand: None,
            roles: Vec::new(),
            operand_access: Vec::new(),
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
        });
    }

    // 2. Check simple-command Home guards (no wrappers, no globs, harmless redirects).
    let mut has_unquoted_glob = false;
    for w in &unit.argv {
        if word_has_unquoted_glob_budgeted(w, budget)? {
            has_unquoted_glob = true;
        }
    }
    let home_guards_ok =
        unit.removed_wrappers.is_empty() && !has_unquoted_glob && all_redirects_harmless;

    // 3. Pre-allocate per-argv roles & access modes with conservative unknown-tool
    //    defaults: argv[0] = Command/Read, every other word = Path/ReadWrite.
    let argc = unit.argv.len();
    budget.charge(argc as u64)?;
    let mut roles = vec_with_exact_budgeted::<OperandRole>(argc, budget)?;
    let mut operand_access = vec_with_exact_budgeted::<AccessMode>(argc, budget)?;
    roles.push(OperandRole::Command);
    operand_access.push(AccessMode::Read);
    for _ in 1..argc {
        roles.push(OperandRole::Path);
        operand_access.push(AccessMode::ReadWrite);
    }

    let mut effects = ToolEffects {
        tool_name: None,
        subcommand: None,
        roles,
        operand_access,
        writes: true,
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
    };

    // 4. Fold argv[0] using CR-19 / FoldRules.
    let Some(folded) = fold_command_bytes_budgeted(unit.argv[0].literal_bytes(), folds, budget)?
    else {
        // Non-UTF-8 command word: unknown-tool treatment.
        return Ok(effects);
    };

    // 5. Explicit OUTSIDE_MODEL entries are always Unread (§6).
    if let Some(entry) = lookup_outside_budgeted(&folded, budget)? {
        let tool = charge_str_clone(entry.name, budget)?;
        vec_push_budgeted(
            &mut effects.unread,
            UnreadReason::OutsideToolModel { tool },
            budget,
        )?;
        return Ok(effects);
    }

    // 6. Lookup in normative TOOLS table.
    let Some(spec) = lookup_tool_budgeted(&folded, budget)? else {
        // Unknown tool (§4, including UNKNOWN_BY_DESIGN).
        // `cd` is handled directly by P2's §5 cwd tracker via `cd_target_operand`.
        if folded == "cd" {
            classify_cd(unit, redirect_writes, &mut effects, budget)?;
        }
        return Ok(effects);
    };

    effects.tool_name = Some(spec.name);

    match spec.kind {
        ToolRowKind::HomeReadOnly(row) => {
            classify_home_readonly(
                unit,
                spec.name,
                row,
                1,
                home_guards_ok,
                redirect_writes,
                &mut effects,
                budget,
            )?;
        }
        ToolRowKind::Search(row) => {
            classify_search(
                unit,
                spec.name,
                row,
                home_guards_ok,
                redirect_writes,
                &mut effects,
                budget,
            )?;
        }
        ToolRowKind::Wrapper(_) => {
            // §4 wrappers are removed by P1; a unit still starting with one is Unread.
            let wrapper = charge_str_clone(spec.name, budget)?;
            vec_push_budgeted(
                &mut effects.unread,
                UnreadReason::UnsupportedWrapperForm { wrapper },
                budget,
            )?;
        }
        ToolRowKind::NestedShell(row) => {
            classify_nested_shell(unit, spec.name, row, &mut effects, budget)?;
        }
        ToolRowKind::CodeRunner(row) => {
            classify_code_runner(unit, spec, row, &mut effects, budget)?;
        }
        ToolRowKind::ConditionalRunner(row) => {
            classify_conditional_runner(
                unit,
                spec,
                row,
                home_guards_ok,
                redirect_writes,
                &mut effects,
                budget,
            )?;
        }
        ToolRowKind::DerivedDestinationWriter(row) => {
            classify_derived_destination_writer(unit, spec.name, row, 1, &mut effects, budget)?;
        }
    }

    Ok(effects)
}

// ---------------------------------------------------------------------------
// Unknown-by-design `cd` helper (§5 cwd tracking handoff to P2)
// ---------------------------------------------------------------------------

fn classify_cd(
    unit: &CommandUnit,
    redirect_writes: bool,
    effects: &mut ToolEffects,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    // `cd` changes the shell's working directory; it does not write files
    // unless accompanied by a non-/dev/null output redirect.
    effects.writes = redirect_writes;
    let mut idx = 1usize;
    let mut bad_flag = false;
    while idx < unit.argv.len() {
        let Some(tok) = word_as_str_budgeted(&unit.argv[idx], budget)? else {
            break;
        };
        if tok == "--" {
            effects.roles[idx] = OperandRole::Flag;
            effects.operand_access[idx] = AccessMode::Read;
            idx += 1;
            break;
        }
        if tok.len() > 1
            && tok.starts_with('-')
            && !tok[1..].starts_with(|c: char| c.is_ascii_digit())
        {
            budget.charge(tok.len() as u64)?;
            if tok[1..].chars().all(|c| matches!(c, 'L' | 'P' | 'e' | '@')) {
                effects.roles[idx] = OperandRole::Flag;
                effects.operand_access[idx] = AccessMode::Read;
            } else {
                bad_flag = true;
                let tool = charge_str_clone("cd", budget)?;
                let flag = charge_str_clone(tok, budget)?;
                vec_push_budgeted(
                    &mut effects.unread,
                    UnreadReason::UnknownFlag { tool, flag },
                    budget,
                )?;
            }
            idx += 1;
        } else {
            break;
        }
    }
    let remaining = unit.argv.len().saturating_sub(idx);
    if !bad_flag && remaining == 1 {
        let target = &unit.argv[idx];
        let b = target.literal_bytes();
        budget.charge(b.len().max(1) as u64)?;
        let is_dash = b == b"-";
        let is_stack =
            b.len() >= 2 && matches!(b[0], b'-' | b'+') && b[1..].iter().all(u8::is_ascii_digit);
        if !is_dash && !is_stack && !word_has_unquoted_glob_budgeted(target, budget)? {
            effects.roles[idx] = OperandRole::Path;
            effects.operand_access[idx] = AccessMode::Read;
            effects.cd_target_operand = Some(idx);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Kind 1: HomeReadOnly (§6, D-141)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn classify_home_readonly(
    unit: &CommandUnit,
    expected_name: &'static str,
    row: HomeReadOnlyRow,
    args_start: usize,
    home_guards_ok: bool,
    redirect_writes: bool,
    effects: &mut ToolEffects,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    let spec = row.home_readonly;
    let as_written_ok = if spec.prefix.len() == 1 {
        bytes_eq_budgeted(
            unit.argv[0].literal_bytes(),
            spec.prefix[0].as_bytes(),
            budget,
        )?
    } else if spec.prefix.len() == 2 && args_start == 2 && unit.argv.len() >= 2 {
        bytes_eq_budgeted(
            unit.argv[0].literal_bytes(),
            spec.prefix[0].as_bytes(),
            budget,
        )? && bytes_eq_budgeted(
            unit.argv[1].literal_bytes(),
            spec.prefix[1].as_bytes(),
            budget,
        )?
    } else {
        false
    };

    // Check forbidden flags before `--` (mirrors dev's `engine::forbidden_flag`).
    let mut has_forbidden = false;
    for arg in &unit.argv[args_start..] {
        if bytes_eq_budgeted(arg.literal_bytes(), b"--", budget)? {
            break;
        }
        match word_as_str_budgeted(arg, budget)? {
            Some(tok) => {
                if is_forbidden_flag_budgeted(spec.forbidden, tok, budget)? {
                    has_forbidden = true;
                }
            }
            None => {
                if !spec.forbidden.is_empty() && arg.literal_bytes().starts_with(b"-") {
                    has_forbidden = true;
                }
            }
        }
    }

    // Check `only` constraint (e.g. `git branch` listing flags).
    let mut only_ok = true;
    if let Some(only) = spec.only {
        for arg in &unit.argv[args_start..] {
            match word_as_str_budgeted(arg, budget)? {
                Some(tok) => {
                    budget.charge(
                        (only.len() as u64).saturating_mul(tok.len().clamp(1, 16) as u64),
                    )?;
                    if !only.contains(&tok) {
                        only_ok = false;
                    }
                }
                None => {
                    only_ok = false;
                }
            }
        }
    }

    // `HomeReadOnly` rows keep every word as `Path` (`ReadWrite`), and a
    // forbidden flag ONLY removes Home eligibility (never emits `Unread`).
    // If a forbidden flag is present or `only` is violated (e.g. `git branch
    // new-name` or `tree -o out`), mark as possible writer; otherwise only
    // output redirects outside `/dev/null` make it a writer.
    let _ = expected_name;
    effects.writes = redirect_writes || has_forbidden || !only_ok;
    effects.home_readonly_eligible = home_guards_ok && as_written_ok && !has_forbidden && only_ok;
    effects.git_selects_by_name = row.git_selects_by_name;
    Ok(())
}

// ---------------------------------------------------------------------------
// Kind 2: Search (`rg` and `grep` family; §5, §6, D-141)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn classify_search(
    unit: &CommandUnit,
    tool_name: &'static str,
    row: SearchRow,
    home_guards_ok: bool,
    redirect_writes: bool,
    effects: &mut ToolEffects,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    effects.filter_dialect = Some(row.filter_dialect);
    effects.writes = redirect_writes;

    let mut recursive = matches!(row.recursive, SearchRecursion::Always);
    let mut links = row.links;
    let mut has_pattern_flag = false;
    let mut filter_ci_global = false;
    let mut has_forbidden_home_flag = false;
    let mut stdin_dash = false;
    let mut saw_unknown_flag = false;

    let mut positional: Vec<usize> = Vec::new();
    let mut raw_filters: Vec<(tools::FilterSpec, Word, Option<usize>)> = Vec::new();

    let mut dd = false;
    let mut i = 1usize;
    while i < unit.argv.len() {
        let tok_word = &unit.argv[i];
        let tok_bytes = tok_word.literal_bytes();
        budget.charge(tok_bytes.len().max(1) as u64)?;

        if dd || !tok_bytes.starts_with(b"-") || tok_bytes == b"-" {
            if !dd && tok_bytes == b"-" {
                stdin_dash = true;
            }
            vec_push_budgeted(&mut positional, i, budget)?;
            i += 1;
            continue;
        }

        if tok_bytes == b"--" {
            effects.roles[i] = OperandRole::Flag;
            effects.operand_access[i] = AccessMode::Read;
            dd = true;
            i += 1;
            continue;
        }

        let Some(tok) = word_as_str_budgeted(tok_word, budget)? else {
            saw_unknown_flag = true;
            has_forbidden_home_flag = true;
            let tool = charge_str_clone(tool_name, budget)?;
            let flag = charge_str_clone("<non-utf8>", budget)?;
            vec_push_budgeted(
                &mut effects.unread,
                UnreadReason::UnknownFlag { tool, flag },
                budget,
            )?;
            i += 1;
            continue;
        };

        if let Some(home) = row.home_readonly
            && is_forbidden_flag_budgeted(home.forbidden, tok, budget)?
        {
            has_forbidden_home_flag = true;
        }

        if let Some(rest) = tok.strip_prefix("--") {
            budget.charge(rest.len().max(1) as u64)?;
            let (name, eq_pos) = match rest.find('=') {
                Some(pos) => (&rest[..pos], Some(2 + pos)),
                None => (rest, None),
            };
            let Some(fspec) = flag_long_budgeted(row.flags, name, budget)? else {
                saw_unknown_flag = true;
                // Keep conservative Path/ReadWrite on unknown flag token.
                let tool = charge_str_clone(tool_name, budget)?;
                let flag = charge_str_join2("--", "", name, budget)?;
                vec_push_budgeted(
                    &mut effects.unread,
                    UnreadReason::UnknownFlag { tool, flag },
                    budget,
                )?;
                i += 1;
                continue;
            };

            let eff = fspec.effects();
            recursive = eff.recursive.apply(recursive);
            links.at_root = eff.follows_links_at_root.apply(links.at_root);
            links.below_root = eff.follows_links_below_root.apply(links.below_root);
            has_pattern_flag |= eff.provides_pattern;
            filter_ci_global = eff.filter_case_insensitive.apply(filter_ci_global);
            if let Some(unread_kind) = eff.unread {
                let reason = match unread_kind {
                    tools::FlagUnread::FilterForm => UnreadReason::UnsupportedFilterForm,
                    tools::FlagUnread::FlagEffect => UnreadReason::UnsupportedFlagEffect {
                        tool: charge_str_clone(tool_name, budget)?,
                        flag: charge_str_join2("--", "", name, budget)?,
                    },
                };
                vec_push_budgeted(&mut effects.unread, reason, budget)?;
            }

            match fspec.kind {
                FlagKind::Bool => {
                    if eq_pos.is_some() {
                        saw_unknown_flag = true;
                        let tool = charge_str_clone(tool_name, budget)?;
                        let flag = charge_str_join2("--", "", name, budget)?;
                        vec_push_budgeted(
                            &mut effects.unread,
                            UnreadReason::UnknownFlag { tool, flag },
                            budget,
                        )?;
                    } else {
                        effects.roles[i] = OperandRole::Flag;
                        effects.operand_access[i] = AccessMode::Read;
                    }
                    i += 1;
                }
                FlagKind::Value(val_role) => {
                    if let Some(eq) = eq_pos {
                        if val_role == ValueRole::Path {
                            // P2 resolves whole argv words and cannot strip `--file=`,
                            // so attached path-valued flags fail closed to Unread.
                            effects.roles[i] = OperandRole::Path;
                            effects.operand_access[i] = AccessMode::Read;
                            let tool = charge_str_clone(tool_name, budget)?;
                            let flag = charge_str_join2("--", "", name, budget)?;
                            vec_push_budgeted(
                                &mut effects.unread,
                                UnreadReason::UnsupportedFlagEffect { tool, flag },
                                budget,
                            )?;
                        } else {
                            effects.roles[i] = OperandRole::Flag;
                            effects.operand_access[i] = AccessMode::Read;
                            if let Some(fspec_filter) = eff.filter {
                                let pat_word = slice_word_budgeted(tok_word, eq + 1, budget)?;
                                vec_push_budgeted(
                                    &mut raw_filters,
                                    (fspec_filter, pat_word, Some(i)),
                                    budget,
                                )?;
                            }
                        }
                        i += 1;
                    } else {
                        effects.roles[i] = OperandRole::Flag;
                        effects.operand_access[i] = AccessMode::Read;
                        if i + 1 < unit.argv.len() {
                            i += 1;
                            effects.roles[i] = OperandRole::FlagValue(val_role);
                            effects.operand_access[i] = AccessMode::Read;
                            if let Some(fspec_filter) = eff.filter {
                                let pat_word = slice_word_budgeted(&unit.argv[i], 0, budget)?;
                                vec_push_budgeted(
                                    &mut raw_filters,
                                    (fspec_filter, pat_word, Some(i)),
                                    budget,
                                )?;
                            }
                            i += 1;
                        } else {
                            saw_unknown_flag = true;
                            let tool = charge_str_clone(tool_name, budget)?;
                            let flag = charge_str_join2("--", "", name, budget)?;
                            vec_push_budgeted(
                                &mut effects.unread,
                                UnreadReason::UnknownFlag { tool, flag },
                                budget,
                            )?;
                            i += 1;
                        }
                    }
                }
            }
        } else {
            // Short option cluster `-...`
            budget.charge(tok.len().max(1) as u64)?;
            let mut cluster_ok = true;
            let mut consumed_next = false;
            let mut attached_path = false;
            for (byte_off, ch) in tok[1..].char_indices() {
                let Some(fspec) = flag_short_budgeted(row.flags, ch, budget)? else {
                    saw_unknown_flag = true;
                    cluster_ok = false;
                    let tool = charge_str_clone(tool_name, budget)?;
                    let mut flag_buf = [0u8; 5];
                    flag_buf[0] = b'-';
                    let ch_len = ch.encode_utf8(&mut flag_buf[1..]).len();
                    let flag_s = std::str::from_utf8(&flag_buf[..1 + ch_len]).unwrap_or("-?");
                    let flag = charge_str_clone(flag_s, budget)?;
                    vec_push_budgeted(
                        &mut effects.unread,
                        UnreadReason::UnknownFlag { tool, flag },
                        budget,
                    )?;
                    break;
                };

                let eff = fspec.effects();
                recursive = eff.recursive.apply(recursive);
                links.at_root = eff.follows_links_at_root.apply(links.at_root);
                links.below_root = eff.follows_links_below_root.apply(links.below_root);
                has_pattern_flag |= eff.provides_pattern;
                filter_ci_global = eff.filter_case_insensitive.apply(filter_ci_global);
                if let Some(unread_kind) = eff.unread {
                    let mut flag_buf = [0u8; 5];
                    flag_buf[0] = b'-';
                    let ch_len = ch.encode_utf8(&mut flag_buf[1..]).len();
                    let flag_s = std::str::from_utf8(&flag_buf[..1 + ch_len]).unwrap_or("-?");
                    let reason = match unread_kind {
                        tools::FlagUnread::FilterForm => UnreadReason::UnsupportedFilterForm,
                        tools::FlagUnread::FlagEffect => UnreadReason::UnsupportedFlagEffect {
                            tool: charge_str_clone(tool_name, budget)?,
                            flag: charge_str_clone(flag_s, budget)?,
                        },
                    };
                    vec_push_budgeted(&mut effects.unread, reason, budget)?;
                }

                if let FlagKind::Value(val_role) = fspec.kind {
                    let rest_start = 1 + byte_off + ch.len_utf8();
                    if rest_start < tok.len() {
                        // Attached short-flag value (e.g. `-g*.rs`, `-ePAT`, `-fFILE`)
                        if val_role == ValueRole::Path {
                            attached_path = true;
                            let mut flag_buf = [0u8; 5];
                            flag_buf[0] = b'-';
                            let ch_len = ch.encode_utf8(&mut flag_buf[1..]).len();
                            let flag_s =
                                std::str::from_utf8(&flag_buf[..1 + ch_len]).unwrap_or("-?");
                            let tool = charge_str_clone(tool_name, budget)?;
                            let flag = charge_str_clone(flag_s, budget)?;
                            vec_push_budgeted(
                                &mut effects.unread,
                                UnreadReason::UnsupportedFlagEffect { tool, flag },
                                budget,
                            )?;
                        } else if let Some(fspec_filter) = eff.filter {
                            let pat_word = slice_word_budgeted(tok_word, rest_start, budget)?;
                            vec_push_budgeted(
                                &mut raw_filters,
                                (fspec_filter, pat_word, Some(i)),
                                budget,
                            )?;
                        }
                    } else if i + 1 < unit.argv.len() {
                        consumed_next = true;
                        effects.roles[i + 1] = OperandRole::FlagValue(val_role);
                        effects.operand_access[i + 1] = AccessMode::Read;
                        if let Some(fspec_filter) = eff.filter {
                            let pat_word = slice_word_budgeted(&unit.argv[i + 1], 0, budget)?;
                            vec_push_budgeted(
                                &mut raw_filters,
                                (fspec_filter, pat_word, Some(i + 1)),
                                budget,
                            )?;
                        }
                    } else {
                        saw_unknown_flag = true;
                        cluster_ok = false;
                        let mut flag_buf = [0u8; 5];
                        flag_buf[0] = b'-';
                        let ch_len = ch.encode_utf8(&mut flag_buf[1..]).len();
                        let flag_s = std::str::from_utf8(&flag_buf[..1 + ch_len]).unwrap_or("-?");
                        let tool = charge_str_clone(tool_name, budget)?;
                        let flag = charge_str_clone(flag_s, budget)?;
                        vec_push_budgeted(
                            &mut effects.unread,
                            UnreadReason::UnknownFlag { tool, flag },
                            budget,
                        )?;
                    }
                    break;
                }
            }

            if cluster_ok && !attached_path {
                effects.roles[i] = OperandRole::Flag;
                effects.operand_access[i] = AccessMode::Read;
            } else if attached_path {
                effects.roles[i] = OperandRole::Path;
                effects.operand_access[i] = AccessMode::Read;
            }
            i += if consumed_next { 2 } else { 1 };
        }
    }

    // Assign positional roles: when an unknown flag appeared, keep conservative
    // Path/ReadWrite on all positional words so no path check is relaxed.
    if !saw_unknown_flag {
        for (pos_k, &argv_idx) in positional.iter().enumerate() {
            if pos_k == 0 && !has_pattern_flag {
                effects.roles[argv_idx] = OperandRole::Pattern;
                effects.operand_access[argv_idx] = AccessMode::Read;
            } else {
                effects.roles[argv_idx] = OperandRole::Path;
                effects.operand_access[argv_idx] = AccessMode::Read;
            }
        }
    }

    // Finalize filters with the unit's final global case-insensitivity toggle.
    let mut unsupported_filter = raw_filters.len() > 1;
    for (fspec_filter, pattern, argv_index) in raw_filters {
        budget.charge(1)?;
        if pattern.literal_bytes().first() == Some(&b'!') {
            unsupported_filter = true;
        }
        let case_insensitive = match fspec_filter.case {
            FilterCase::AlwaysSensitive => false,
            FilterCase::AlwaysInsensitive => true,
            FilterCase::FollowGlobalToggle => filter_ci_global,
        };
        vec_push_budgeted(
            &mut effects.filters,
            SearchFilter {
                dialect: fspec_filter.dialect,
                pattern,
                case_insensitive,
                argv_index,
            },
            budget,
        )?;
    }
    if unsupported_filter {
        vec_push_budgeted(
            &mut effects.unread,
            UnreadReason::UnsupportedFilterForm,
            budget,
        )?;
    }

    effects.reads_recursively = recursive;
    effects.links = links;

    if let Some(home) = row.home_readonly {
        let as_written_ok = bytes_eq_budgeted(
            unit.argv[0].literal_bytes(),
            home.prefix[0].as_bytes(),
            budget,
        )?;
        let followed_ok = !(links.below_root && !home.allow_followed_search);
        effects.home_readonly_eligible = home_guards_ok
            && as_written_ok
            && !has_forbidden_home_flag
            && !stdin_dash
            && effects.unread.is_empty()
            && followed_ok;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Kind 3b: NestedShell (`sh`, `bash`, `dash`; §4, §6)
// ---------------------------------------------------------------------------

fn classify_nested_shell(
    unit: &CommandUnit,
    shell_name: &'static str,
    row: NestedShellRow,
    effects: &mut ToolEffects,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    if row.script_pin == ProgramPin::FirstOperand
        && unit.argv.len() >= 2
        && let Some(arg0) = word_as_str_budgeted(&unit.argv[0], budget)?
        && let Some(arg1) = word_as_str_budgeted(&unit.argv[1], budget)?
        && !arg1.is_empty()
        && !arg1.starts_with('-')
    {
        effects.roles[1] = OperandRole::ProgramFile;
        effects.operand_access[1] = AccessMode::Read;
        let mut program_words = vec_with_exact_budgeted::<String>(2, budget)?;
        program_words.push(charge_str_clone(arg0, budget)?);
        program_words.push(charge_str_clone(arg1, budget)?);
        let mut unread_words = vec_with_exact_budgeted::<String>(2, budget)?;
        unread_words.push(charge_str_clone(arg0, budget)?);
        unread_words.push(charge_str_clone(arg1, budget)?);
        effects.code = CodeExecution::Program {
            program_operand: Some(1),
            program_words,
        };
        vec_push_budgeted(
            &mut effects.unread,
            UnreadReason::Program {
                program_words: unread_words,
            },
            budget,
        )?;
        return Ok(());
    }

    let shell = charge_str_clone(shell_name, budget)?;
    vec_push_budgeted(
        &mut effects.unread,
        UnreadReason::UnsupportedNestedShellForm { shell },
        budget,
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Kind 4: CodeRunner (§4, §6, §11 item 8, D-141)
// ---------------------------------------------------------------------------

fn classify_code_runner(
    unit: &CommandUnit,
    spec: &'static ToolSpec,
    row: CodeRunnerRow,
    effects: &mut ToolEffects,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    // Multi-subcommand code runners (`cargo`, `pipx`).
    if !row.subcommands.is_empty() {
        if unit.argv.len() >= 2
            && let Some(sub_str) = word_as_str_budgeted(&unit.argv[1], budget)?
            && let Some(sub) = lookup_subcommand_budgeted(spec, sub_str, budget)?
            && let SubcommandKind::CodeRunner(sub_row) = sub.kind
        {
            effects.subcommand = Some(sub.name);
            effects.roles[1] = OperandRole::Command;
            effects.operand_access[1] = AccessMode::Read;
            return classify_code_runner_leaf(
                unit,
                spec.name,
                Some(sub.name),
                sub_row,
                2,
                effects,
                budget,
            );
        }
        // Bare `cargo` / `pipx` or unlisted subcommand: plain Unread.
        let tool = charge_str_clone(spec.name, budget)?;
        effects.code = CodeExecution::Inline {
            code_operands: Vec::new(),
        };
        vec_push_budgeted(
            &mut effects.unread,
            UnreadReason::CodeRunnerInlineCode { tool },
            budget,
        )?;
        return Ok(());
    }

    classify_code_runner_leaf(unit, spec.name, None, row, 1, effects, budget)
}

fn classify_code_runner_leaf(
    unit: &CommandUnit,
    tool_name: &'static str,
    sub_name: Option<&'static str>,
    row: CodeRunnerRow,
    args_start: usize,
    effects: &mut ToolEffects,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    if let Some(pin_spec) = row.program_pin {
        match pin_spec.pin {
            ProgramPin::CommandAndSubcommand => {
                if let Some(sub) = sub_name
                    && unit.argv.len() >= 2
                    && let Some(w0) = word_as_str_budgeted(&unit.argv[0], budget)?
                    && let Some(w1) = word_as_str_budgeted(&unit.argv[1], budget)?
                {
                    let _ = sub;
                    let mut p1 = vec_with_exact_budgeted::<String>(2, budget)?;
                    p1.push(charge_str_clone(w0, budget)?);
                    p1.push(charge_str_clone(w1, budget)?);
                    let mut p2 = vec_with_exact_budgeted::<String>(2, budget)?;
                    p2.push(charge_str_clone(w0, budget)?);
                    p2.push(charge_str_clone(w1, budget)?);
                    effects.code = CodeExecution::Program {
                        program_operand: None,
                        program_words: p1,
                    };
                    vec_push_budgeted(
                        &mut effects.unread,
                        UnreadReason::Program { program_words: p2 },
                        budget,
                    )?;
                    return Ok(());
                }
            }
            ProgramPin::FirstOperand => {
                if unit.argv.len() > args_start
                    && let Some(first_op) = word_as_str_budgeted(&unit.argv[args_start], budget)?
                    && !first_op.is_empty()
                    && !first_op.starts_with('-')
                {
                    // Build exact program_words = argv[0..=args_start], retaining
                    // validated `&str` views so no word is rescanned without charge (§8).
                    let count = args_start + 1;
                    let mut views = vec_with_exact_budgeted::<&str>(count, budget)?;
                    let mut valid_utf8 = true;
                    for (k, w) in unit.argv[..count].iter().enumerate() {
                        let s = if k == args_start {
                            Some(first_op)
                        } else {
                            word_as_str_budgeted(w, budget)?
                        };
                        if let Some(s) = s {
                            views.push(s);
                        } else {
                            valid_utf8 = false;
                            break;
                        }
                    }
                    if valid_utf8 {
                        effects.roles[args_start] = OperandRole::ProgramFile;
                        effects.operand_access[args_start] = AccessMode::Read;
                        let mut p1 = vec_with_exact_budgeted::<String>(count, budget)?;
                        let mut p2 = vec_with_exact_budgeted::<String>(count, budget)?;
                        for &s in &views {
                            p1.push(charge_str_clone(s, budget)?);
                            p2.push(charge_str_clone(s, budget)?);
                        }
                        effects.code = CodeExecution::Program {
                            program_operand: Some(args_start),
                            program_words: p1,
                        };
                        vec_push_budgeted(
                            &mut effects.unread,
                            UnreadReason::Program { program_words: p2 },
                            budget,
                        )?;
                        return Ok(());
                    }
                }
            }
            ProgramPin::FlagValue => {
                // `awk`/`gawk`/`mawk` `-f FILE` / `--file FILE`:
                // Pin ONLY when `argv[args_start]` is an exact listed pin flag,
                // `argv[args_start + 1]` is a non-empty, non-`-` program file,
                // and no second pin flag or inline-code flag (`-e`, `--source`,
                // `--eval`) appears before `--`.
                if unit.argv.len() > args_start + 1
                    && let Some(flag_tok) = word_as_str_budgeted(&unit.argv[args_start], budget)?
                    && pin_spec.flags.contains(&flag_tok)
                    && let Some(prog_tok) =
                        word_as_str_budgeted(&unit.argv[args_start + 1], budget)?
                    && !prog_tok.is_empty()
                    && prog_tok != "-"
                    && !has_competing_awk_code_flags(
                        &unit.argv[args_start + 2..],
                        pin_spec.flags,
                        budget,
                    )?
                    && let Some(w0) = word_as_str_budgeted(&unit.argv[0], budget)?
                {
                    effects.roles[args_start] = OperandRole::Flag;
                    effects.operand_access[args_start] = AccessMode::Read;
                    effects.roles[args_start + 1] = OperandRole::ProgramFile;
                    effects.operand_access[args_start + 1] = AccessMode::Read;
                    let mut p1 = vec_with_exact_budgeted::<String>(3, budget)?;
                    p1.push(charge_str_clone(w0, budget)?);
                    p1.push(charge_str_clone(flag_tok, budget)?);
                    p1.push(charge_str_clone(prog_tok, budget)?);
                    let mut p2 = vec_with_exact_budgeted::<String>(3, budget)?;
                    p2.push(charge_str_clone(w0, budget)?);
                    p2.push(charge_str_clone(flag_tok, budget)?);
                    p2.push(charge_str_clone(prog_tok, budget)?);
                    effects.code = CodeExecution::Program {
                        program_operand: Some(args_start + 1),
                        program_words: p1,
                    };
                    vec_push_budgeted(
                        &mut effects.unread,
                        UnreadReason::Program { program_words: p2 },
                        budget,
                    )?;
                    return Ok(());
                }
            }
        }
    }

    // Inline code (not pinned): record inline code operand indices if present.
    let mut code_operands = Vec::new();
    match row.inline_code {
        InlineCodeSpec::None => {}
        InlineCodeSpec::Flags(inline_flags) => {
            let mut i = args_start;
            while i < unit.argv.len() {
                let Some(tok) = word_as_str_budgeted(&unit.argv[i], budget)? else {
                    i += 1;
                    continue;
                };
                if tok == "--" {
                    break;
                }
                if !tok.starts_with('-') || tok == "-" {
                    break;
                }
                if let Some(attached) = match_inline_flag(tok, inline_flags, budget)? {
                    if attached {
                        vec_push_budgeted(&mut code_operands, i, budget)?;
                    } else if i + 1 < unit.argv.len() {
                        vec_push_budgeted(&mut code_operands, i + 1, budget)?;
                        i += 1;
                    }
                }
                i += 1;
            }
        }
        InlineCodeSpec::Operand(pos_idx) => {
            let mut pos = 0usize;
            let mut dd = false;
            for idx in args_start..unit.argv.len() {
                let b = unit.argv[idx].literal_bytes();
                budget.charge(b.len().max(1) as u64)?;
                if !dd && b == b"--" {
                    dd = true;
                    continue;
                }
                if !dd && b.starts_with(b"-") && b != b"-" {
                    continue;
                }
                if pos == pos_idx {
                    vec_push_budgeted(&mut code_operands, idx, budget)?;
                    break;
                }
                pos += 1;
            }
        }
        InlineCodeSpec::OperandsFrom(start_op) => {
            let first = args_start.saturating_add(start_op);
            if first < unit.argv.len() {
                let count = unit.argv.len() - first;
                code_operands = vec_with_exact_budgeted::<usize>(count, budget)?;
                budget.charge(count as u64)?;
                for idx in first..unit.argv.len() {
                    code_operands.push(idx);
                }
            }
        }
    }

    effects.code = CodeExecution::Inline { code_operands };
    let tool = match sub_name {
        Some(sub) => charge_str_join2(tool_name, " ", sub, budget)?,
        None => charge_str_clone(tool_name, budget)?,
    };
    vec_push_budgeted(
        &mut effects.unread,
        UnreadReason::CodeRunnerInlineCode { tool },
        budget,
    )?;
    Ok(())
}

/// Returns `Ok(Some(attached))` if `tok` matches one of `inline_flags`
/// (`-c`, `--eval`, `--eval=X`, `-Ic`, `-cX`), where `attached == true` means
/// the code is inside `tok` itself rather than in the following argv word.
fn match_inline_flag(
    tok: &str,
    inline_flags: &[&str],
    budget: &mut ComputeBudget,
) -> Result<Option<bool>, Exhausted> {
    budget.charge((inline_flags.len().max(1) as u64).saturating_mul(tok.len().max(1) as u64))?;
    if let Some(rest) = tok.strip_prefix("--") {
        let (name, has_eq) = match rest.split_once('=') {
            Some((n, _)) => (n, true),
            None => (rest, false),
        };
        for &f in inline_flags {
            if let Some(fl) = f.strip_prefix("--")
                && name == fl
            {
                return Ok(Some(has_eq));
            }
        }
        return Ok(None);
    }
    if let Some(cluster) = tok.strip_prefix('-')
        && !cluster.is_empty()
    {
        for (byte_off, ch) in cluster.char_indices() {
            for &f in inline_flags {
                if f.len() == 2 && !f.starts_with("--") && f.chars().nth(1) == Some(ch) {
                    let attached = byte_off + ch.len_utf8() < cluster.len();
                    return Ok(Some(attached));
                }
            }
        }
    }
    Ok(None)
}

/// Checks whether trailing args after `awk -f PROG` contain another `-f`/`--file`
/// or inline-code flag (`-e`, `--source`, `--eval`) before `--`.
fn has_competing_awk_code_flags(
    rest: &[Word],
    pin_flags: &[&str],
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    for w in rest {
        let Some(tok) = word_as_str_budgeted(w, budget)? else {
            continue;
        };
        if tok == "--" {
            break;
        }
        if !tok.starts_with('-') || tok == "-" {
            continue;
        }
        if is_forbidden_flag_budgeted(pin_flags, tok, budget)? {
            return Ok(true);
        }
        if is_forbidden_flag_budgeted(&["-e", "--source", "--eval"], tok, budget)? {
            return Ok(true);
        }
    }
    Ok(false)
}

// ---------------------------------------------------------------------------
// Kind 5: ConditionalRunner (`find`, `git`, `sed`; §6, D-141)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn classify_conditional_runner(
    unit: &CommandUnit,
    spec: &'static ToolSpec,
    row: ConditionalRunnerRow,
    home_guards_ok: bool,
    redirect_writes: bool,
    effects: &mut ToolEffects,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    // Subcommand-bearing conditional runner (`git`).
    if !row.subcommands.is_empty() {
        return classify_git(
            unit,
            spec,
            row,
            home_guards_ok,
            redirect_writes,
            effects,
            budget,
        );
    }

    // Script-command / program-file conditional runner (`sed`).
    let has_script_trigger = row
        .triggers
        .iter()
        .any(|t| matches!(t, RunnerTrigger::ScriptCommands(_)));
    if has_script_trigger {
        return classify_sed(unit, spec.name, row, effects, budget);
    }

    // Flag-trigger conditional runner (`find`).
    for idx in 1..unit.argv.len() {
        let Some(tok) = word_as_str_budgeted(&unit.argv[idx], budget)? else {
            continue;
        };
        for &trigger in row.triggers {
            if let RunnerTrigger::Flag(f) = trigger {
                budget.charge(tok.len().max(1) as u64)?;
                if tok == f {
                    let tool = charge_str_clone(spec.name, budget)?;
                    effects.code = CodeExecution::Inline {
                        code_operands: Vec::new(),
                    };
                    vec_push_budgeted(
                        &mut effects.unread,
                        UnreadReason::CodeRunnerInlineCode { tool },
                        budget,
                    )?;
                    return Ok(());
                }
            }
        }
    }

    // No trigger fired: unknown-tool treatment (possible writer, every word
    // Path/ReadWrite, never Home-eligible).
    Ok(())
}

/// Locates `git`'s subcommand past leading global options and applies
/// `GIT_TRIGGERS` + `GIT_SUBCOMMANDS` + `cat-file` name selection.
#[allow(clippy::too_many_arguments)]
fn classify_git(
    unit: &CommandUnit,
    spec: &'static ToolSpec,
    row: ConditionalRunnerRow,
    home_guards_ok: bool,
    redirect_writes: bool,
    effects: &mut ToolEffects,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    // 1. Walk global options before the subcommand, respecting short/long
    //    option values so attached values (`git -Csrc status`) and separate
    //    values (`git -C -c status`) are never scanned as option letters.
    let mut i = 1usize;
    let mut triggered_tool: Option<String> = None;
    let mut subcmd_idx: Option<(usize, &str)> = None;

    while i < unit.argv.len() {
        let Some(tok) = word_as_str_budgeted(&unit.argv[i], budget)? else {
            break;
        };
        if tok == "--" || !tok.starts_with('-') || tok == "-" {
            if tok != "--" && !tok.starts_with('-') {
                subcmd_idx = Some((i, tok));
            }
            break;
        }
        let opt = parse_git_global_opt(tok, budget)?;
        budget.charge(row.triggers.len().max(1) as u64)?;
        if let Some(matched_flag) = opt.trigger
            && triggered_tool.is_none()
            && row
                .triggers
                .iter()
                .any(|t| matches!(t, RunnerTrigger::Flag(f) if *f == matched_flag))
        {
            triggered_tool = Some(charge_str_clone(spec.name, budget)?);
        }
        if opt.consumes_next && i + 1 < unit.argv.len() {
            i += 2;
        } else {
            i += 1;
        }
    }

    // 2. Check subcommand triggers (`Subcommand`, `SubcommandWithSubarg`,
    //    `SubcommandWithFlag`).
    if let Some((sidx, sname)) = subcmd_idx {
        let sub_args = &unit.argv[sidx + 1..];
        for &trigger in row.triggers {
            match trigger {
                RunnerTrigger::Subcommand(target_sub) => {
                    budget.charge(sname.len().max(1) as u64)?;
                    if sname == target_sub && triggered_tool.is_none() {
                        triggered_tool =
                            Some(charge_str_join2(spec.name, " ", target_sub, budget)?);
                    }
                }
                RunnerTrigger::SubcommandWithSubarg { subcommand, subarg } => {
                    budget.charge(sname.len().max(1) as u64)?;
                    if sname == subcommand && triggered_tool.is_none() {
                        for arg in sub_args {
                            let Some(a) = word_as_str_budgeted(arg, budget)? else {
                                continue;
                            };
                            if a == "--" {
                                break;
                            }
                            if !a.starts_with('-') {
                                budget.charge(a.len().max(1) as u64)?;
                                if a == subarg {
                                    triggered_tool =
                                        Some(charge_str_join2(spec.name, " ", subcommand, budget)?);
                                }
                                break;
                            }
                        }
                    }
                }
                RunnerTrigger::SubcommandWithFlag { subcommand, flag } => {
                    budget.charge(sname.len().max(1) as u64)?;
                    if sname == subcommand
                        && triggered_tool.is_none()
                        && git_subcommand_has_flag_trigger_budgeted(
                            subcommand, sub_args, flag, budget,
                        )?
                    {
                        triggered_tool =
                            Some(charge_str_join2(spec.name, " ", subcommand, budget)?);
                    }
                }
                RunnerTrigger::Flag(_)
                | RunnerTrigger::ScriptCommands(_)
                | RunnerTrigger::ProgramFileFlag { .. } => {}
            }
        }

        // 3. Apply modeled subcommand row (`HomeReadOnly` / `DerivedDestinationWriter`)
        //    or `cat-file` CR-12 name selection.
        if let Some(sub) = lookup_subcommand_budgeted(spec, sname, budget)? {
            effects.subcommand = Some(sub.name);
            effects.roles[sidx] = OperandRole::Command;
            effects.operand_access[sidx] = AccessMode::Read;
            match sub.kind {
                SubcommandKind::HomeReadOnly(sub_row) => {
                    // Home eligibility requires no global opts before the subcommand (`sidx == 1`)
                    // and no code-runner trigger.
                    let sub_home_guards = home_guards_ok && sidx == 1 && triggered_tool.is_none();
                    classify_home_readonly(
                        unit,
                        sub.name,
                        sub_row,
                        sidx + 1,
                        sub_home_guards,
                        redirect_writes,
                        effects,
                        budget,
                    )?;
                }
                SubcommandKind::DerivedDestinationWriter(sub_row) => {
                    let tool_label = charge_str_join2(spec.name, " ", sub.name, budget)?;
                    classify_derived_destination_writer_label(
                        unit,
                        spec.name,
                        &tool_label,
                        sub_row,
                        sidx + 1,
                        effects,
                        budget,
                    )?;
                }
                SubcommandKind::CodeRunner(sub_row) => {
                    classify_code_runner_leaf(
                        unit,
                        spec.name,
                        Some(sub.name),
                        sub_row,
                        sidx + 1,
                        effects,
                        budget,
                    )?;
                }
            }
        } else if sname == "cat-file" {
            // `git cat-file` receives unknown-tool treatment under D-141, while
            // keeping CR-12 git name selection (§6).
            effects.subcommand = Some("cat-file");
            effects.roles[sidx] = OperandRole::Command;
            effects.operand_access[sidx] = AccessMode::Read;
            effects.git_selects_by_name = true;
        }
    }

    if let Some(tool) = triggered_tool {
        effects.writes = true;
        effects.home_readonly_eligible = false;
        effects.code = CodeExecution::Inline {
            code_operands: Vec::new(),
        };
        vec_push_budgeted(
            &mut effects.unread,
            UnreadReason::CodeRunnerInlineCode { tool },
            budget,
        )?;
    }

    Ok(())
}

/// Arity of a single short option letter inside a `-...` cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShortOptArity {
    /// Boolean flag: takes no value; following letters in the token are more flags.
    Bool,
    /// Required value: remaining bytes in the token are the attached value (never
    /// read as option letters); if at end of token, consumes the next argv word.
    Value,
    /// Optional attached value (e.g. `git rebase -S[<keyid>]`): remaining bytes in
    /// the token are the attached value (never read as option letters); if at end
    /// of token, does NOT consume the next argv word.
    OptionalAttachedValue,
    /// Unknown short option: stop scanning remaining bytes of the token so a
    /// potential attached value is never misread as option letters.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GitPathspecFlag {
    Literal,
    Noglob,
    Glob,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GitGlobalOpt {
    /// Matched top-level trigger flag (`"-c"`, `"--config-env"`, `"--exec-path"`).
    pub(super) trigger: Option<&'static str>,
    /// Whether this token consumes the immediately following argv word as its value.
    pub(super) consumes_next: bool,
    /// Pathspec global flag, if any.
    pub(super) pathspec_flag: Option<GitPathspecFlag>,
}

fn git_global_short_arity(ch: char) -> ShortOptArity {
    match ch {
        'v' | 'h' | 'p' | 'P' => ShortOptArity::Bool,
        'C' | 'c' => ShortOptArity::Value,
        _ => ShortOptArity::Unknown,
    }
}

/// Parses one top-level Git option token before the subcommand, distinguishing
/// short options from their attached or separate values so value bytes (e.g.
/// `src` in `git -Csrc status`) are never read as option letters.
pub(super) fn parse_git_global_opt(
    tok: &str,
    budget: &mut ComputeBudget,
) -> Result<GitGlobalOpt, Exhausted> {
    budget.charge(tok.len().max(1) as u64)?;
    if let Some(rest) = tok.strip_prefix("--") {
        let (name, has_eq) = match rest.split_once('=') {
            Some((n, _)) => (n, true),
            None => (rest, false),
        };
        let trigger = match name {
            "config-env" => Some("--config-env"),
            "exec-path" => Some("--exec-path"),
            _ => None,
        };
        let consumes_next = !has_eq
            && matches!(
                name,
                "git-dir"
                    | "work-tree"
                    | "namespace"
                    | "super-prefix"
                    | "config-env"
                    | "attr-source"
            );
        let pathspec_flag = if !has_eq {
            match name {
                "literal-pathspecs" => Some(GitPathspecFlag::Literal),
                "noglob-pathspecs" => Some(GitPathspecFlag::Noglob),
                "glob-pathspecs" => Some(GitPathspecFlag::Glob),
                _ => None,
            }
        } else {
            None
        };
        return Ok(GitGlobalOpt {
            trigger,
            consumes_next,
            pathspec_flag,
        });
    }

    if let Some(cluster) = tok.strip_prefix('-')
        && !cluster.is_empty()
    {
        for (byte_off, ch) in cluster.char_indices() {
            let at_end = byte_off + ch.len_utf8() >= cluster.len();
            if ch == 'c' {
                return Ok(GitGlobalOpt {
                    trigger: Some("-c"),
                    consumes_next: at_end,
                    pathspec_flag: None,
                });
            }
            match git_global_short_arity(ch) {
                ShortOptArity::Bool => {}
                ShortOptArity::Value => {
                    return Ok(GitGlobalOpt {
                        trigger: None,
                        consumes_next: at_end,
                        pathspec_flag: None,
                    });
                }
                ShortOptArity::OptionalAttachedValue | ShortOptArity::Unknown => {
                    break;
                }
            }
        }
    }

    Ok(GitGlobalOpt {
        trigger: None,
        consumes_next: false,
        pathspec_flag: None,
    })
}

fn git_subcommand_short_arity(subcommand: &str, ch: char) -> ShortOptArity {
    match subcommand {
        "rebase" => match ch {
            'm' | 'f' | 'i' | 'q' | 'v' | 'n' | 'k' | 'p' | 'h' => ShortOptArity::Bool,
            's' | 'X' | 'x' | 'C' => ShortOptArity::Value,
            'S' | 'r' => ShortOptArity::OptionalAttachedValue,
            _ => ShortOptArity::Unknown,
        },
        "difftool" => match ch {
            'd' | 'y' | 'g' | 'h' | 'p' | 'u' | 's' | 'w' | 'b' | 'R' | 'z' => ShortOptArity::Bool,
            't' | 'x' | 'S' | 'G' | 'O' | 'I' | 'l' => ShortOptArity::Value,
            'U' | 'C' | 'M' | 'B' | 'W' => ShortOptArity::OptionalAttachedValue,
            _ => ShortOptArity::Unknown,
        },
        "mergetool" => match ch {
            'y' | 'g' | 'h' => ShortOptArity::Bool,
            't' | 'x' => ShortOptArity::Value,
            _ => ShortOptArity::Unknown,
        },
        "diff" => match ch {
            'p' | 'u' | 's' | 'w' | 'b' | 'R' | 'z' | 'h' => ShortOptArity::Bool,
            'S' | 'G' | 'O' | 'I' | 'l' => ShortOptArity::Value,
            'U' | 'C' | 'M' | 'B' | 'W' => ShortOptArity::OptionalAttachedValue,
            _ => ShortOptArity::Unknown,
        },
        "push" => match ch {
            'v' | 'q' | 'f' | 'u' | 'n' | 'd' | 'h' => ShortOptArity::Bool,
            'o' => ShortOptArity::Value,
            _ => ShortOptArity::Unknown,
        },
        _ => ShortOptArity::Unknown,
    }
}

fn git_subcommand_long_takes_separate_value(subcommand: &str, name: &str) -> bool {
    match subcommand {
        "rebase" => matches!(
            name,
            "onto" | "strategy" | "strategy-option" | "exec" | "empty" | "whitespace"
        ),
        "difftool" => matches!(name, "tool" | "extcmd" | "tool-cmd" | "output"),
        "mergetool" => matches!(name, "tool" | "tool-cmd"),
        "diff" => matches!(
            name,
            "output"
                | "diff-filter"
                | "src-prefix"
                | "dst-prefix"
                | "line-prefix"
                | "inter-hunk-context"
                | "unified"
                | "word-diff-regex"
        ),
        "push" => matches!(name, "repo" | "push-option" | "receive-pack" | "exec"),
        _ => false,
    }
}

/// Evaluates one option token on a Git subcommand against `trigger`, returning
/// `(matched_trigger, consumes_next_word)` without scanning option-value bytes
/// as option letters.
pub(super) fn git_subcommand_opt_step(subcommand: &str, tok: &str, trigger: &str) -> (bool, bool) {
    if let Some(rest) = tok.strip_prefix("--") {
        let (name, has_eq) = match rest.split_once('=') {
            Some((n, _)) => (n, true),
            None => (rest, false),
        };
        let matched = trigger.strip_prefix("--").is_some_and(|l| name == l);
        let consumes_next = !has_eq && git_subcommand_long_takes_separate_value(subcommand, name);
        return (matched, consumes_next);
    }

    if let Some(cluster) = tok.strip_prefix('-')
        && !cluster.is_empty()
    {
        let trigger_short = (trigger.len() == 2 && !trigger.starts_with("--"))
            .then(|| trigger.chars().nth(1))
            .flatten();
        for (byte_off, ch) in cluster.char_indices() {
            let at_end = byte_off + ch.len_utf8() >= cluster.len();
            if trigger_short == Some(ch) {
                let consumes_next =
                    at_end && git_subcommand_short_arity(subcommand, ch) == ShortOptArity::Value;
                return (true, consumes_next);
            }
            match git_subcommand_short_arity(subcommand, ch) {
                ShortOptArity::Bool => {}
                ShortOptArity::Value => return (false, at_end),
                ShortOptArity::OptionalAttachedValue | ShortOptArity::Unknown => {
                    return (false, false);
                }
            }
        }
    }

    (tok == trigger, false)
}

/// Pure subcommand flag-trigger scan used by `validate_rule_matcher` (§7).
pub(super) fn git_subcommand_has_flag_trigger_pure(
    subcommand: &str,
    args: &[String],
    trigger: &str,
) -> bool {
    let mut i = 0usize;
    while i < args.len() {
        let tok = args[i].as_str();
        if tok == "--" {
            break;
        }
        if !tok.starts_with('-') || tok == "-" {
            i += 1;
            continue;
        }
        let (matched, consumes_next) = git_subcommand_opt_step(subcommand, tok, trigger);
        if matched {
            return true;
        }
        i += if consumes_next { 2 } else { 1 };
    }
    false
}

/// Budgeted subcommand flag-trigger scan used by `classify_git` (§6, §8).
fn git_subcommand_has_flag_trigger_budgeted(
    subcommand: &str,
    args: &[Word],
    trigger: &str,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    let mut i = 0usize;
    while i < args.len() {
        let Some(tok) = word_as_str_budgeted(&args[i], budget)? else {
            i += 1;
            continue;
        };
        if tok == "--" {
            break;
        }
        if !tok.starts_with('-') || tok == "-" {
            i += 1;
            continue;
        }
        budget.charge(tok.len().max(1) as u64)?;
        let (matched, consumes_next) = git_subcommand_opt_step(subcommand, tok, trigger);
        if matched {
            return Ok(true);
        }
        i += if consumes_next { 2 } else { 1 };
    }
    Ok(false)
}

/// Classifies `sed` against `SED_TRIGGERS` (`ScriptCommands(&['e', 'w', 'r'])`
/// and `ProgramFileFlag` `-f`/`--file`).
fn classify_sed(
    unit: &CommandUnit,
    tool_name: &'static str,
    row: ConditionalRunnerRow,
    effects: &mut ToolEffects,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    let trigger_cmds: &'static [char] = row
        .triggers
        .iter()
        .find_map(|t| match t {
            RunnerTrigger::ScriptCommands(cs) => Some(*cs),
            _ => None,
        })
        .unwrap_or(&['e', 'w', 'r']);

    // Check for `-f` / `--file` program file flags and collect `-e` scripts.
    let mut expr_scripts: Vec<&str> = Vec::new();
    let mut program_files: Vec<(usize, Option<usize>)> = Vec::new(); // (flag_idx, separate_val_idx)
    let mut positional: Vec<usize> = Vec::new();
    let mut uncertain = false;
    let mut saw_bare_in_place = false;

    let mut i = 1usize;
    let mut dd = false;
    while i < unit.argv.len() {
        let Some(tok) = word_as_str_budgeted(&unit.argv[i], budget)? else {
            uncertain = true;
            i += 1;
            continue;
        };
        if dd || !tok.starts_with('-') || tok == "-" {
            vec_push_budgeted(&mut positional, i, budget)?;
            i += 1;
            continue;
        }
        if tok == "--" {
            dd = true;
            i += 1;
            continue;
        }
        if let Some(rest) = tok.strip_prefix("--") {
            budget.charge(rest.len().max(1) as u64)?;
            let (name, val) = match rest.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (rest, None),
            };
            match name {
                "file" => {
                    if val.is_some() {
                        vec_push_budgeted(&mut program_files, (i, None), budget)?;
                        i += 1;
                    } else if i + 1 < unit.argv.len() {
                        vec_push_budgeted(&mut program_files, (i, Some(i + 1)), budget)?;
                        i += 2;
                    } else {
                        uncertain = true;
                        i += 1;
                    }
                }
                "expression" => {
                    if let Some(v) = val {
                        vec_push_budgeted(&mut expr_scripts, v, budget)?;
                        i += 1;
                    } else if i + 1 < unit.argv.len() {
                        if let Some(s) = word_as_str_budgeted(&unit.argv[i + 1], budget)? {
                            vec_push_budgeted(&mut expr_scripts, s, budget)?;
                        } else {
                            uncertain = true;
                        }
                        i += 2;
                    } else {
                        uncertain = true;
                        i += 1;
                    }
                }
                "quiet" | "silent" | "regexp-extended" | "separate" | "unbuffered"
                | "null-data" | "posix" | "sandbox" | "debug" | "follow-symlinks" => {
                    if val.is_some() {
                        uncertain = true;
                    }
                    i += 1;
                }
                "in-place" => {
                    if val.is_none() {
                        saw_bare_in_place = true;
                    }
                    i += 1;
                }
                "line-length" => {
                    i += if val.is_some() { 1 } else { 2 };
                }
                _ => {
                    uncertain = true;
                    i += 1;
                }
            }
        } else {
            // Short option cluster `-...`
            budget.charge(tok.len().max(1) as u64)?;
            let mut consumed_next = false;
            for (byte_off, ch) in tok[1..].char_indices() {
                let rest_start = 1 + byte_off + ch.len_utf8();
                match ch {
                    'f' => {
                        if rest_start < tok.len() {
                            vec_push_budgeted(&mut program_files, (i, None), budget)?;
                        } else if i + 1 < unit.argv.len() {
                            vec_push_budgeted(&mut program_files, (i, Some(i + 1)), budget)?;
                            consumed_next = true;
                        } else {
                            uncertain = true;
                        }
                        break;
                    }
                    'e' => {
                        if rest_start < tok.len() {
                            vec_push_budgeted(&mut expr_scripts, &tok[rest_start..], budget)?;
                        } else if i + 1 < unit.argv.len() {
                            if let Some(s) = word_as_str_budgeted(&unit.argv[i + 1], budget)? {
                                vec_push_budgeted(&mut expr_scripts, s, budget)?;
                            } else {
                                uncertain = true;
                            }
                            consumed_next = true;
                        } else {
                            uncertain = true;
                        }
                        break;
                    }
                    'i' => {
                        if rest_start >= tok.len() {
                            saw_bare_in_place = true;
                            // Bare `-i` is ambiguous across GNU (no arg) and
                            // BSD/macOS (consumes next word as backup suffix).
                            // Only `sed -i '' ...` (the macOS empty-suffix
                            // idiom) cannot hide a script behind a suffix;
                            // every other bare `-i` form (e.g.
                            // `sed -i p 'w out' input`) is classified Unread.
                            budget.charge(1)?;
                            if i + 1 < unit.argv.len()
                                && unit.argv[i + 1].literal_bytes().is_empty()
                            {
                                consumed_next = true;
                            } else {
                                uncertain = true;
                            }
                        }
                        break;
                    }
                    'l' => {
                        if rest_start >= tok.len() {
                            consumed_next = true;
                        }
                        break;
                    }
                    'n' | 'E' | 'r' | 's' | 'u' | 'z' => {}
                    _ => {
                        uncertain = true;
                        break;
                    }
                }
            }
            i += if consumed_next { 2 } else { 1 };
        }
    }

    // 1. Program file trigger (`sed -f PROG` / `sed --file PROG`):
    if !program_files.is_empty() {
        // Pin as `Unread (program)` ONLY when there is exactly one separate
        // `-f PROG` / `--file PROG` at `argv[1..=2]`, no `-e` scripts, no
        // uncertainty, and a non-empty non-`-` program path.
        if program_files.len() == 1
            && expr_scripts.is_empty()
            && !uncertain
            && !saw_bare_in_place
            && let (1, Some(2)) = program_files[0]
            && let Some(w0) = word_as_str_budgeted(&unit.argv[0], budget)?
            && let Some(w1) = word_as_str_budgeted(&unit.argv[1], budget)?
            && (w1 == "-f" || w1 == "--file")
            && let Some(w2) = word_as_str_budgeted(&unit.argv[2], budget)?
            && !w2.is_empty()
            && w2 != "-"
        {
            effects.roles[1] = OperandRole::Flag;
            effects.operand_access[1] = AccessMode::Read;
            effects.roles[2] = OperandRole::ProgramFile;
            effects.operand_access[2] = AccessMode::Read;
            let mut p1 = vec_with_exact_budgeted::<String>(3, budget)?;
            p1.push(charge_str_clone(w0, budget)?);
            p1.push(charge_str_clone(w1, budget)?);
            p1.push(charge_str_clone(w2, budget)?);
            let mut p2 = vec_with_exact_budgeted::<String>(3, budget)?;
            p2.push(charge_str_clone(w0, budget)?);
            p2.push(charge_str_clone(w1, budget)?);
            p2.push(charge_str_clone(w2, budget)?);
            effects.code = CodeExecution::Program {
                program_operand: Some(2),
                program_words: p1,
            };
            vec_push_budgeted(
                &mut effects.unread,
                UnreadReason::Program { program_words: p2 },
                budget,
            )?;
            return Ok(());
        }
        // Multiple `-f`, attached `-fFILE`, `-f -`, or `-f` combined with `-e`:
        // plain Unread.
        let tool = charge_str_clone(tool_name, budget)?;
        effects.code = CodeExecution::Inline {
            code_operands: Vec::new(),
        };
        vec_push_budgeted(
            &mut effects.unread,
            UnreadReason::CodeRunnerInlineCode { tool },
            budget,
        )?;
        return Ok(());
    }

    // 2. Inline scripts: either all `-e` scripts, or positional[0] (handling
    //    macOS `sed -i '' SCRIPT FILE` where positional[0] is empty backup ext).
    if uncertain {
        let tool = charge_str_clone(tool_name, budget)?;
        effects.code = CodeExecution::Inline {
            code_operands: Vec::new(),
        };
        vec_push_budgeted(
            &mut effects.unread,
            UnreadReason::CodeRunnerInlineCode { tool },
            budget,
        )?;
        return Ok(());
    }

    if expr_scripts.is_empty() {
        let Some(sidx) = positional.first().copied() else {
            // No script operand at all -> cannot tell reliably -> Unread.
            let tool = charge_str_clone(tool_name, budget)?;
            effects.code = CodeExecution::Inline {
                code_operands: Vec::new(),
            };
            vec_push_budgeted(
                &mut effects.unread,
                UnreadReason::CodeRunnerInlineCode { tool },
                budget,
            )?;
            return Ok(());
        };
        let Some(script) = word_as_str_budgeted(&unit.argv[sidx], budget)? else {
            let tool = charge_str_clone(tool_name, budget)?;
            effects.code = CodeExecution::Inline {
                code_operands: Vec::new(),
            };
            vec_push_budgeted(
                &mut effects.unread,
                UnreadReason::CodeRunnerInlineCode { tool },
                budget,
            )?;
            return Ok(());
        };
        vec_push_budgeted(&mut expr_scripts, script, budget)?;
    }

    for script in expr_scripts {
        match inspect_sed_script(script, trigger_cmds, budget)? {
            SedParseOutcome::Safe => {}
            SedParseOutcome::Triggered | SedParseOutcome::Uncertain => {
                let tool = charge_str_clone(tool_name, budget)?;
                effects.code = CodeExecution::Inline {
                    code_operands: Vec::new(),
                };
                vec_push_budgeted(
                    &mut effects.unread,
                    UnreadReason::CodeRunnerInlineCode { tool },
                    budget,
                )?;
                return Ok(());
            }
        }
    }

    // No trigger fired: unknown-tool treatment (possible writer, every word
    // Path/ReadWrite, never Home-eligible).
    Ok(())
}

// ---------------------------------------------------------------------------
// Kind 6: DerivedDestinationWriter (§5, §6, §11 item 10, D-141)
// ---------------------------------------------------------------------------

fn classify_derived_destination_writer(
    unit: &CommandUnit,
    tool_name: &'static str,
    row: DerivedDestinationWriterRow,
    args_start: usize,
    effects: &mut ToolEffects,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    classify_derived_destination_writer_label(
        unit, tool_name, tool_name, row, args_start, effects, budget,
    )
}

fn classify_derived_destination_writer_label(
    unit: &CommandUnit,
    tool_name: &'static str,
    unread_label: &str,
    row: DerivedDestinationWriterRow,
    args_start: usize,
    effects: &mut ToolEffects,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    match row.trigger {
        DerivedDestinationTrigger::Always => {
            effects.derived_destinations = DerivedDestinations::Always;
            let tool = charge_str_clone(unread_label, budget)?;
            vec_push_budgeted(
                &mut effects.unread,
                UnreadReason::DerivedDestinations { tool },
                budget,
            )?;
        }
        DerivedDestinationTrigger::WhenSourceIsDir => {
            // P2 resolves `mv`'s source operands against `PathResolver` and
            // emits `Unread(DerivedDestinations)` only when a source is a dir.
            effects.derived_destinations = DerivedDestinations::WhenSourceIsDir;
        }
        DerivedDestinationTrigger::Flags(trigger_flags) => {
            let mut triggered = false;
            // For `tar`, old-style bundled key letters without leading `-`
            // (e.g. `tar xf a.tar`, `tar xzf a.tar.gz`) sit at `argv[args_start]`.
            if tool_name == "tar"
                && args_start < unit.argv.len()
                && let Some(first_arg) = word_as_str_budgeted(&unit.argv[args_start], budget)?
                && !first_arg.is_empty()
                && !first_arg.starts_with('-')
            {
                budget.charge(
                    (trigger_flags.len().max(1) as u64)
                        .saturating_mul(first_arg.len().max(1) as u64),
                )?;
                for &f in trigger_flags {
                    if f.len() == 2
                        && !f.starts_with("--")
                        && let Some(ch) = f.chars().nth(1)
                        && first_arg.contains(ch)
                    {
                        triggered = true;
                        break;
                    }
                }
            }

            if !triggered {
                for arg in &unit.argv[args_start..] {
                    let Some(tok) = word_as_str_budgeted(arg, budget)? else {
                        continue;
                    };
                    if tok == "--" {
                        break;
                    }
                    if !tok.starts_with('-') || tok == "-" {
                        continue;
                    }
                    if matches_exact_or_short_cluster(tok, trigger_flags, budget)? {
                        triggered = true;
                        break;
                    }
                }
            }

            if triggered {
                effects.derived_destinations = DerivedDestinations::Always;
                let tool = charge_str_clone(unread_label, budget)?;
                vec_push_budgeted(
                    &mut effects.unread,
                    UnreadReason::DerivedDestinations { tool },
                    budget,
                )?;
            }
        }
    }
    Ok(())
}

/// Matches exact long options (`--recursive`, `--recursive=..`) and short-flag
/// letters inside clusters (`-R`, `-av`, `-xzf`).
fn matches_exact_or_short_cluster(
    tok: &str,
    flags: &[&str],
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    budget.charge((flags.len().max(1) as u64).saturating_mul(tok.len().max(1) as u64))?;
    if let Some(rest) = tok.strip_prefix("--") {
        let name = rest.split('=').next().unwrap_or(rest);
        return Ok(flags
            .iter()
            .any(|&f| f.strip_prefix("--").is_some_and(|l| l == name)));
    }
    if let Some(cluster) = tok.strip_prefix('-')
        && !cluster.is_empty()
    {
        return Ok(cluster.chars().any(|c| {
            flags
                .iter()
                .any(|&f| f.len() == 2 && !f.starts_with("--") && f.chars().nth(1) == Some(c))
        }));
    }
    Ok(false)
}

// ---------------------------------------------------------------------------
// Public P3 entry point 2: `check_git_selection` (§6, §8, CR-12)
// ---------------------------------------------------------------------------

/// Runs CR-12 Git selection checks on `git show`/`cat-file`/`log`/`diff`:
/// committed credential names, credential-ancestor pathspecs, pathspec
/// magic/exclusion (`StoreHit`), and active wildcard pathspecs
/// (`Unread(GitWildcardPathspec)`), plus wildcard pathspec checks on
/// `git status` and `git ls-files`.
///
/// **Owned by:** P3 · **Contract:** §6, §8.
pub(crate) fn check_git_selection(
    unit: &CommandUnit,
    effects: &ToolEffects,
    folds: &FoldRules,
    budget: &mut ComputeBudget,
) -> GitSelectionOutcome {
    let mut results = Vec::new();
    let res = git::check_git_selection_into_impl(unit, effects, folds, &mut results, budget);
    GitSelectionOutcome {
        results,
        exhausted: res.err(),
    }
}

/// Sink variant of [`check_git_selection`] that appends established results
/// directly into `out` (reserving slots before checking each operand) so every
/// earlier `StoreHit` / `Unread(GitWildcardPathspec)` remains in `out` if a
/// later operand returns `Err(Exhausted)` (§6, §8, D-153).
pub(crate) fn check_git_selection_into(
    unit: &CommandUnit,
    effects: &ToolEffects,
    folds: &FoldRules,
    out: &mut Vec<PartResult>,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    git::check_git_selection_into_impl(unit, effects, folds, out, budget)
}
