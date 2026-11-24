//! CR-12 Git name-selection and wildcard-pathspec checks (contract §6, §8,
//! M1-29 round 4 finding 1, D-153).

use crate::policy::model::{
    CommandUnit, ComputeBudget, Exhausted, FoldRules, Hit, HitMatch, HitSource, OperandRole,
    PartId, PartOutcome, PartResult, ToolEffects, UnreadReason,
};
use crate::policy::tools::{self, GIT_CRED_DIRS, GIT_CRED_NAMES, GIT_CRED_SUFFIXES};

use super::{
    fold_command_bytes_budgeted, reserve_vec_slots_budgeted, vec_push_budgeted,
    word_as_str_budgeted,
};

/// CR-17 credential store suffixes added beyond `dev`'s `GIT_CRED_SUFFIXES`
/// (D-111, contract §6, §11).
const CR17_GIT_CRED_SUFFIXES: &[&str] = &[
    ".gem/credentials",
    ".terraform.d/credentials.tfrc.json",
    ".config/op/config",
];

/// CR-17 credential store file names added beyond `dev`'s `GIT_CRED_NAMES`
/// (D-111, contract §6, §11).
const CR17_GIT_CRED_NAMES: &[&str] = &[".yarnrc.yml", "credentials.tfrc.json"];

/// Store family label emitted on CR-12/CR-17 Git selection `StoreHit`s.
const GIT_SELECTION_FAMILY: &str = "git-credential-selection";

#[derive(Debug, Clone, Copy, Default)]
struct PathspecMagicFlags {
    has_literal: bool,
    has_glob: bool,
}

pub(super) fn check_git_selection_into_impl(
    unit: &CommandUnit,
    effects: &ToolEffects,
    folds: &FoldRules,
    out: &mut Vec<PartResult>,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    if effects.tool_name != Some("git") || unit.argv.is_empty() {
        return Ok(());
    }

    let sub = effects.subcommand;
    let check_cred_names = effects.git_selects_by_name;
    let check_wildcards = check_cred_names || matches!(sub, Some("status" | "ls-files"));
    if !check_cred_names && !check_wildcards {
        return Ok(());
    }

    // Find subcommand position (marked `OperandRole::Command` at index >= 1,
    // or fallback to index 1). Charge before scanning `effects.roles`.
    budget.charge(effects.roles.len().max(1) as u64)?;
    let subcmd_idx = effects
        .roles
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(idx, role)| (*role == OperandRole::Command).then_some(idx))
        .unwrap_or(1);
    if subcmd_idx >= unit.argv.len() {
        return Ok(());
    }

    // Track pathspec magic (`:(...)`, `:!`, `:`) separately from default
    // wildcard interpretation (`*`, `?`, `[`):
    // - `--literal-pathspecs` disables both pathspec magic and wildcards;
    // - `--noglob-pathspecs` makes wildcards literal by default while keeping
    //   explicit pathspec magic (including `:(glob)`) active;
    // - `--glob-pathspecs` leaves wildcards active (fail-closed if combined).
    let mut pathspec_magic_enabled = true;
    let mut default_glob_wildcards = true;
    for w in &unit.argv[1..subcmd_idx] {
        if let Some(tok) = word_as_str_budgeted(w, budget)? {
            match tok {
                "--literal-pathspecs" => {
                    pathspec_magic_enabled = false;
                    default_glob_wildcards = false;
                }
                "--noglob-pathspecs" => {
                    default_glob_wildcards = false;
                }
                "--glob-pathspecs" => {
                    pathspec_magic_enabled = true;
                    default_glob_wildcards = true;
                }
                _ => {}
            }
        }
    }

    let mut after_dd = false;
    let mut i = subcmd_idx + 1;

    while i < unit.argv.len() {
        // Reserve 2 result slots BEFORE inspecting operand `i` so committing
        // any established `StoreHit` / `GitWildcardPathspec` on this operand
        // cannot fail with allocation exhaustion (§8).
        reserve_vec_slots_budgeted(out, 2, budget)?;

        let argv_index = i;
        let part = PartId::Operand {
            unit: unit.index,
            argv_index,
        };
        let Some(arg) = word_as_str_budgeted(&unit.argv[i], budget)? else {
            // Non-UTF-8 pathspec: check raw bytes for active wildcards or `:` magic.
            let raw = unit.argv[i].literal_bytes();
            budget.charge(raw.len().max(1) as u64)?;
            if check_cred_names && pathspec_magic_enabled && raw.starts_with(b":") {
                push_store_hit_reserved(out, part);
            } else if check_wildcards && default_glob_wildcards {
                const NON_UTF8_LABEL: &str = "<non-utf8>";
                budget.charge(
                    (raw.len().max(1) as u64).saturating_add(NON_UTF8_LABEL.len() as u64),
                )?;
                budget.charge_alloc(NON_UTF8_LABEL.len())?;
                if raw_has_active_wildcard(raw) {
                    let mut pathspec = String::with_capacity(NON_UTF8_LABEL.len());
                    pathspec.push_str(NON_UTF8_LABEL);
                    push_wildcard_reserved(out, part, pathspec);
                }
            }
            i += 1;
            continue;
        };

        if !after_dd && arg == "--" {
            after_dd = true;
            i += 1;
            continue;
        }

        if !after_dd {
            // Handle `-L` / `--line-range` on `git log`: `start,end:file` or `:func:file`.
            if check_cred_names
                && (arg == "-L"
                    || arg == "--line-range"
                    || arg.starts_with("-L")
                    || arg.starts_with("--line-range="))
            {
                let (spec_opt, spec_part) = if arg == "-L" || arg == "--line-range" {
                    i += 1;
                    if i < unit.argv.len() {
                        let next_part = PartId::Operand {
                            unit: unit.index,
                            argv_index: i,
                        };
                        (word_as_str_budgeted(&unit.argv[i], budget)?, next_part)
                    } else {
                        (None, part)
                    }
                } else if let Some(rest) = arg.strip_prefix("-L") {
                    (Some(rest), part)
                } else {
                    (arg.strip_prefix("--line-range="), part)
                };
                if let Some(spec) = spec_opt {
                    budget.charge(spec.len().max(1) as u64)?;
                }
                match spec_opt.and_then(parse_git_line_range) {
                    Some(file) => {
                        if git_path_is_credential_or_ancestor(file, false, folds, budget)? {
                            push_store_hit_reserved(out, spec_part);
                        }
                    }
                    None => {
                        // Malformed or missing `-L` spec: fail closed as StoreHit.
                        push_store_hit_reserved(out, spec_part);
                    }
                }
                i += 1;
                continue;
            }

            // `--pathspec-from-file` reads pathspecs from a file we can't inspect.
            if arg == "--pathspec-from-file" || arg.starts_with("--pathspec-from-file=") {
                if check_cred_names {
                    push_store_hit_reserved(out, part);
                }
                i += 1;
                continue;
            }

            // Skip other flags before `--`.
            if arg.starts_with('-') {
                i += 1;
                continue;
            }
        }

        // Inspect operand / rev:path / pathspec.
        inspect_git_operand(
            arg,
            after_dd,
            check_cred_names,
            check_wildcards,
            pathspec_magic_enabled,
            default_glob_wildcards,
            part,
            folds,
            out,
            budget,
        )?;
        i += 1;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn inspect_git_operand(
    arg: &str,
    after_dd: bool,
    check_cred_names: bool,
    check_wildcards: bool,
    pathspec_magic_enabled: bool,
    default_glob_wildcards: bool,
    part: PartId,
    folds: &FoldRules,
    out: &mut Vec<PartResult>,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    budget.charge(arg.len().max(1) as u64)?;

    // 1. Leading `:` forms: `:N:PATH` (index stage), lone `:` pathspec,
    //    `:(magic)PATH`, `:!PATH`, `:^PATH`, `:/PATH`, `:PATH`.
    if let Some(after) = arg.strip_prefix(':') {
        // Index stage object spec `:N:PATH` (only before `--`, revision-object context).
        if !after_dd
            && let Some((n, path)) = after.split_once(':')
            && !n.is_empty()
            && n.bytes().all(|b| b.is_ascii_digit())
        {
            if check_cred_names && git_path_is_credential_or_ancestor(path, false, folds, budget)? {
                push_store_hit_reserved(out, part);
            }
            return Ok(());
        }

        if pathspec_magic_enabled {
            // Lone `:` is a special pathspec selecting the root/top of the tree
            // (ancestor of every credential-shaped name; §6).
            if after.is_empty() {
                if check_cred_names {
                    push_store_hit_reserved(out, part);
                }
                return Ok(());
            }

            // Pathspec magic or exclusion: `:(...)`, `:!`, `:^`, `:/`
            if after.starts_with(['(', '!', '^', '/']) {
                budget.charge(after.len().max(1) as u64)?;
                let (body, magic) = strip_pathspec_magic(after);
                // Commit the StoreHit FIRST (already reserved, infallible) so
                // it cannot be lost if wildcard string allocation exhausts later.
                if check_cred_names {
                    push_store_hit_reserved(out, part.clone());
                }
                if check_wildcards {
                    let wildcards_active = match body {
                        None => true,
                        Some(_) => !magic.has_literal && (magic.has_glob || default_glob_wildcards),
                    };
                    if wildcards_active {
                        let scan_len = body.map_or(1, |b| b.len().max(1));
                        budget.charge((arg.len().max(1) as u64).saturating_add(scan_len as u64))?;
                        if !arg.is_empty() {
                            budget.charge_alloc(arg.len())?;
                        }
                        let has_wc = match body {
                            Some(body_str) => str_has_active_wildcard(body_str),
                            None => true,
                        };
                        if has_wc {
                            let mut pathspec = String::with_capacity(arg.len());
                            pathspec.push_str(arg);
                            push_wildcard_reserved(out, part, pathspec);
                        }
                    }
                }
                return Ok(());
            }

            // Unrecognized `:` magic or extra `:` (fail closed on CR-12 subcommands).
            if after.starts_with(['~', '@', '#', '+', '=', '%', '&', '$', '\\', ':'])
                || after.contains(':')
            {
                if check_cred_names {
                    push_store_hit_reserved(out, part);
                }
                return Ok(());
            }

            // Plain `:PATH`: after `--` this is pathspec context (empty magic + PATH,
            // where `:.` / `:./` is a root ancestor); before `--` it is a stage-0
            // object spec (revision-object context).
            if after_dd {
                if check_wildcards && default_glob_wildcards {
                    budget.charge(
                        (arg.len().max(1) as u64).saturating_add(after.len().max(1) as u64),
                    )?;
                    if !arg.is_empty() {
                        budget.charge_alloc(arg.len())?;
                    }
                    if str_has_active_wildcard(after) {
                        let mut pathspec = String::with_capacity(arg.len());
                        pathspec.push_str(arg);
                        push_wildcard_reserved(out, part, pathspec);
                        return Ok(());
                    }
                }
                if check_cred_names
                    && git_path_is_credential_or_ancestor(after, true, folds, budget)?
                {
                    push_store_hit_reserved(out, part);
                }
            } else if check_cred_names
                && git_path_is_credential_or_ancestor(after, false, folds, budget)?
            {
                push_store_hit_reserved(out, part);
            }
            return Ok(());
        } else if !after_dd {
            // Under `--literal-pathspecs`, leading `:` before `--` is still a
            // stage-0 object spec (not a pathspec).
            if check_cred_names && git_path_is_credential_or_ancestor(after, false, folds, budget)?
            {
                push_store_hit_reserved(out, part);
            }
            return Ok(());
        }
        // Under `--literal-pathspecs` after `--`, `:` is literal: fall through to
        // plain pathspec handling below.
    }

    // 2. `REV:PATH` revision-object spec (before `--`, e.g. `HEAD:.env`,
    //    `HEAD:.gem/credentials`, `HEAD:README.md`). Kept separate from
    //    pathspec root-ancestor matching.
    if !after_dd && let Some((rev, path)) = arg.split_once(':') {
        let _ = rev;
        if check_cred_names && git_path_is_credential_or_ancestor(path, false, folds, budget)? {
            push_store_hit_reserved(out, part);
        }
        return Ok(());
    }

    // 3. Plain pathspec (either after `--` or a positional pathspec before `--`).
    if check_wildcards && default_glob_wildcards {
        budget.charge((arg.len().max(1) as u64).saturating_mul(2))?;
        if !arg.is_empty() {
            budget.charge_alloc(arg.len())?;
        }
        if str_has_active_wildcard(arg) {
            let mut pathspec = String::with_capacity(arg.len());
            pathspec.push_str(arg);
            push_wildcard_reserved(out, part, pathspec);
            return Ok(());
        }
    }

    if check_cred_names && git_path_is_credential_or_ancestor(arg, true, folds, budget)? {
        push_store_hit_reserved(out, part);
    }

    Ok(())
}

/// Strips leading `:(magic)` / `:!` / `:^` / `:/` prefixes from the substring
/// after the leading `:`. Returns `(Some(remaining_body), flags)`, or
/// `(None, flags)` if `:(...` is unclosed.
fn strip_pathspec_magic(after_colon: &str) -> (Option<&str>, PathspecMagicFlags) {
    let mut s = after_colon;
    let mut flags = PathspecMagicFlags::default();
    loop {
        if let Some(rest) = s.strip_prefix('(') {
            let mut depth = 1usize;
            let mut end = None;
            for (idx, ch) in rest.char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(idx);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(end_idx) = end else {
                return (None, flags);
            };
            let inside = &rest[..end_idx];
            for kw in inside.split(',') {
                if kw == "literal" {
                    flags.has_literal = true;
                } else if kw == "glob" {
                    flags.has_glob = true;
                }
            }
            s = &rest[end_idx + 1..];
        } else if let Some(rest) = s.strip_prefix(['/', '!', '^']) {
            s = rest;
        } else {
            break;
        }
    }
    (Some(s), flags)
}

/// Returns `true` if `s` contains an unescaped Git pathspec wildcard (`*`, `?`,
/// or `[`). A backslash escapes the immediately following byte.
fn str_has_active_wildcard(s: &str) -> bool {
    raw_has_active_wildcard(s.as_bytes())
}

fn raw_has_active_wildcard(bytes: &[u8]) -> bool {
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'*' | b'?' | b'[' => return true,
            _ => i += 1,
        }
    }
    false
}

/// Extracts `<file>` from `git log -L` (`start,end:file` or `:funcname:file`).
fn parse_git_line_range(spec: &str) -> Option<&str> {
    if let Some(after) = spec.strip_prefix(':') {
        let (func, file) = after.rsplit_once(':')?;
        (!func.is_empty() && !file.is_empty()).then_some(file)
    } else {
        let (range, file) = spec.rsplit_once(':')?;
        (!range.is_empty() && !file.is_empty()).then_some(file)
    }
}

/// Pushes a Git credential selection `StoreHit` into an already-reserved slot
/// of `out` (infallible; caller reserved via `reserve_vec_slots_budgeted`).
fn push_store_hit_reserved(out: &mut Vec<PartResult>, part: PartId) {
    debug_assert!(out.len() < out.capacity());
    out.push(PartResult {
        part: part.clone(),
        outcome: PartOutcome::StoreHit(Hit {
            hard_deny_id: None,
            source: HitSource::Model,
            matched: HitMatch::StoreFamily {
                family: GIT_SELECTION_FAMILY,
            },
            part,
        }),
    });
}

/// Pushes a `GitWildcardPathspec` `Unread` into an already-reserved slot of
/// `out` (infallible; caller reserved via `reserve_vec_slots_budgeted`).
fn push_wildcard_reserved(out: &mut Vec<PartResult>, part: PartId, pathspec: String) {
    debug_assert!(out.len() < out.capacity());
    out.push(PartResult {
        part,
        outcome: PartOutcome::Unread(UnreadReason::GitWildcardPathspec { pathspec }),
    });
}

/// Checks whether a Git path or pathspec (after CR-19 folding and lexical `.`/
/// `..` normalization) matches:
/// 0. a root/ancestor pathspec (`.`, `./`, `..`, `a/..`, `/`, `""`) when
///    `allow_root_ancestor == true` (pathspec context, never revision-object);
/// 1. a credential filename (`GIT_CRED_NAMES` + CR-17, `msal_token_cache.*`,
///    or SSH private key name `id_*` excluding code/public extensions);
/// 2. a credential suffix (`GIT_CRED_SUFFIXES` + CR-17) or a proper ancestor
///    of one (e.g. `.gem`, `.terraform.d`, `.config/op`, `.config`, `.aws`);
/// 3. a credential directory (`GIT_CRED_DIRS`), a proper ancestor of one
///    (e.g. `.aws/sso`, `library`), or a file below one (keeping `.ssh`'s
///    `.pub` / `known_hosts*` / `config` / `authorized_keys` read exceptions).
fn git_path_is_credential_or_ancestor(
    path: &str,
    allow_root_ancestor: bool,
    folds: &FoldRules,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    // Normalize and CR-19-fold component-by-component without quadratic joins.
    budget.charge(path.len().max(1) as u64)?;
    let mut parts: Vec<String> = Vec::new();
    for comp in path.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => {
                // Fold component via `fold_command_bytes_budgeted` (no `/` in `other`,
                // so basename is `other` itself).
                if let Some(folded) = fold_command_bytes_budgeted(other.as_bytes(), folds, budget)?
                {
                    vec_push_budgeted(&mut parts, folded, budget)?;
                }
            }
        }
    }

    let Some(name) = parts.last().map(String::as_str) else {
        // Empty normalized path (e.g. `.`, `./`, `..`, `a/..`, `/`, `""`):
        // in pathspec context this selects the tree root / ancestor of all
        // credential-shaped names (§6); in revision-object context keep separate.
        return Ok(allow_root_ancestor);
    };

    // 1. Filename checks (`GIT_CRED_NAMES`, CR-17 names, `msal_token_cache.*`, SSH key names).
    let name_table_cost = (GIT_CRED_NAMES.len() + CR17_GIT_CRED_NAMES.len() + 5) as u64;
    budget.charge(name_table_cost.saturating_mul(name.len().clamp(1, 32) as u64))?;
    if GIT_CRED_NAMES.contains(&name)
        || CR17_GIT_CRED_NAMES.contains(&name)
        || name.starts_with("msal_token_cache.")
        || is_git_key_name(name)
    {
        return Ok(true);
    }

    // 2. Credential suffixes (exact match OR proper directory ancestor, M1-29 round 4 finding 1).
    let total_suffixes = (GIT_CRED_SUFFIXES.len() + CR17_GIT_CRED_SUFFIXES.len()) as u64;
    budget.charge(total_suffixes.saturating_mul(path.len().max(1) as u64))?;
    for &suffix in GIT_CRED_SUFFIXES.iter().chain(CR17_GIT_CRED_SUFFIXES) {
        if path_matches_or_is_ancestor_of_pattern(&parts, suffix) {
            return Ok(true);
        }
    }

    // 3. Credential directories:
    //    - exact match or proper ancestor of dir (e.g. `.ssh`, `.aws`, `.aws/sso`, `.config`);
    //    - descendant below dir (keeping `.ssh` public/readable exceptions).
    budget.charge((GIT_CRED_DIRS.len() as u64).saturating_mul(path.len().max(1) as u64))?;
    for &dir in GIT_CRED_DIRS {
        if path_matches_or_is_ancestor_of_pattern(&parts, dir) {
            return Ok(true);
        }
        if path_is_below_dir(&parts, dir) {
            let ssh_public_exception = dir == ".ssh"
                && (name.ends_with(".pub")
                    || name.starts_with("known_hosts")
                    || matches!(name, "config" | "authorized_keys"));
            if !ssh_public_exception {
                return Ok(true);
            }
        }
    }

    // Ensure `tools` import is visibly tied to P0's table constants.
    let _ = tools::GIT_CRED_DIRS;
    Ok(false)
}

/// Returns `true` if any suffix `parts[start..]` is either equal to `pattern`'s
/// slash-separated components OR is a non-empty proper prefix (ancestor) of
/// `pattern`'s components.
///
/// Example: for `pattern = ".gem/credentials"`, matches `[".gem", "credentials"]`,
/// `["sub", ".gem", "credentials"]`, `[".gem"]`, and `["sub", ".gem"]`.
fn path_matches_or_is_ancestor_of_pattern(parts: &[String], pattern: &str) -> bool {
    let mut pat_len = 0usize;
    for _ in pattern.split('/') {
        pat_len += 1;
    }
    // Check each suffix `parts[start..]` whose length is `1..=pat_len`.
    let min_start = parts.len().saturating_sub(pat_len);
    for start in min_start..parts.len() {
        let tail = &parts[start..];
        let mut pat_iter = pattern.split('/');
        if tail.iter().all(|c| pat_iter.next() == Some(c.as_str())) {
            return true;
        }
    }
    false
}

/// Returns `true` if `parts` contains `dir`'s components consecutively with at
/// least one additional component following them.
fn path_is_below_dir(parts: &[String], dir: &str) -> bool {
    let mut dir_len = 0usize;
    for _ in dir.split('/') {
        dir_len += 1;
    }
    if parts.len() <= dir_len {
        return false;
    }
    for start in 0..(parts.len() - dir_len) {
        let mut dir_iter = dir.split('/');
        if parts[start..start + dir_len]
            .iter()
            .all(|c| dir_iter.next() == Some(c.as_str()))
        {
            return true;
        }
    }
    false
}

fn is_git_key_name(name: &str) -> bool {
    const CODE_EXTS: &[&str] = &[
        "pub", "rs", "ts", "tsx", "js", "jsx", "py", "go", "c", "cc", "cpp", "h", "hpp", "java",
        "rb", "md", "html", "css", "svg", "png",
    ];
    ["id_rsa", "id_dsa", "id_ecdsa", "id_ed25519"]
        .iter()
        .any(|k| {
            name == *k
                || name.strip_prefix(k).is_some_and(|r| {
                    r.starts_with(['.', '-', '_'])
                        && !r
                            .rsplit('.')
                            .next()
                            .is_some_and(|ext| CODE_EXTS.contains(&ext))
                })
        })
}
