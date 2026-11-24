//! CR-12 Git name-selection and wildcard-pathspec checks (contract §6, §8,
//! M1-29 round 4 finding 1).

use crate::policy::model::{
    CommandUnit, ComputeBudget, Exhausted, FoldRules, Hit, HitMatch, HitSource, OperandRole,
    PartId, PartOutcome, PartResult, ToolEffects, UnreadReason,
};
use crate::policy::tools::{self, GIT_CRED_DIRS, GIT_CRED_NAMES, GIT_CRED_SUFFIXES};

use super::{
    charge_str_clone, fold_command_bytes_budgeted, vec_push_budgeted, word_as_str_budgeted,
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

pub(super) fn check_git_selection_impl(
    unit: &CommandUnit,
    effects: &ToolEffects,
    folds: &FoldRules,
    budget: &mut ComputeBudget,
) -> Result<Vec<PartResult>, Exhausted> {
    if effects.tool_name != Some("git") || unit.argv.is_empty() {
        return Ok(Vec::new());
    }

    let sub = effects.subcommand;
    let check_cred_names = effects.git_selects_by_name;
    let check_wildcards = check_cred_names || matches!(sub, Some("status" | "ls-files"));
    if !check_cred_names && !check_wildcards {
        return Ok(Vec::new());
    }

    // Find subcommand position (marked `OperandRole::Command` at index >= 1,
    // or fallback to index 1).
    let subcmd_idx = effects
        .roles
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(idx, role)| (*role == OperandRole::Command).then_some(idx))
        .unwrap_or(1);
    if subcmd_idx >= unit.argv.len() {
        return Ok(Vec::new());
    }

    // Check leading global options for `--literal-pathspecs`.
    let mut literal_pathspecs = false;
    for w in &unit.argv[1..subcmd_idx] {
        if let Some(tok) = word_as_str_budgeted(w, budget)? {
            if tok == "--literal-pathspecs" {
                literal_pathspecs = true;
            } else if matches!(tok, "--glob-pathspecs" | "--noglob-pathspecs") {
                literal_pathspecs = false;
            }
        }
    }

    let mut out: Vec<PartResult> = Vec::new();
    let mut after_dd = false;
    let mut i = subcmd_idx + 1;

    while i < unit.argv.len() {
        let argv_index = i;
        let part = PartId::Operand {
            unit: unit.index,
            argv_index,
        };
        let Some(arg) = word_as_str_budgeted(&unit.argv[i], budget)? else {
            // Non-UTF-8 pathspec: check raw bytes for active wildcards or `:` magic.
            let raw = unit.argv[i].literal_bytes();
            budget.charge(raw.len().max(1) as u64)?;
            if check_cred_names && raw.starts_with(b":") {
                push_store_hit(&mut out, part, budget)?;
            } else if check_wildcards && !literal_pathspecs && raw_has_active_wildcard(raw) {
                let pathspec = charge_str_clone("<non-utf8>", budget)?;
                vec_push_budgeted(
                    &mut out,
                    PartResult {
                        part,
                        outcome: PartOutcome::Unread(UnreadReason::GitWildcardPathspec {
                            pathspec,
                        }),
                    },
                    budget,
                )?;
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
                match spec_opt.and_then(parse_git_line_range) {
                    Some(file) => {
                        if git_path_is_credential_or_ancestor(file, folds, budget)? {
                            push_store_hit(&mut out, spec_part, budget)?;
                        }
                    }
                    None => {
                        // Malformed or missing `-L` spec: fail closed as StoreHit.
                        push_store_hit(&mut out, spec_part, budget)?;
                    }
                }
                i += 1;
                continue;
            }

            // `--pathspec-from-file` reads pathspecs from a file we can't inspect.
            if arg == "--pathspec-from-file" || arg.starts_with("--pathspec-from-file=") {
                if check_cred_names {
                    push_store_hit(&mut out, part, budget)?;
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
            literal_pathspecs,
            part,
            folds,
            &mut out,
            budget,
        )?;
        i += 1;
    }

    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn inspect_git_operand(
    arg: &str,
    after_dd: bool,
    check_cred_names: bool,
    check_wildcards: bool,
    literal_pathspecs: bool,
    part: PartId,
    folds: &FoldRules,
    out: &mut Vec<PartResult>,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    budget.charge(arg.len().max(1) as u64)?;

    // 1. Leading `:` forms: `:N:PATH` (index stage), `:(magic)PATH`, `:!PATH`,
    //    `:^PATH`, `:/PATH`, `:PATH`.
    if let Some(after) = arg.strip_prefix(':') {
        // Index stage spec `:N:PATH` (only before `--`, when cred checks apply).
        if !after_dd
            && let Some((n, path)) = after.split_once(':')
            && !n.is_empty()
            && n.bytes().all(|b| b.is_ascii_digit())
        {
            if check_cred_names && git_path_is_credential_or_ancestor(path, folds, budget)? {
                push_store_hit(out, part, budget)?;
            }
            return Ok(());
        }

        // Pathspec magic or exclusion: `:(...)`, `:!`, `:^`, `:/`
        if after.starts_with(['(', '!', '^', '/']) {
            let (body, is_literal_magic) = strip_pathspec_magic(after);
            if check_cred_names {
                push_store_hit(out, part.clone(), budget)?;
            }
            if check_wildcards && !is_literal_magic && !literal_pathspecs {
                let wildcard_or_unclosed = match body {
                    Some(body_str) => str_has_active_wildcard(body_str),
                    None => true,
                };
                if wildcard_or_unclosed {
                    let pathspec = charge_str_clone(arg, budget)?;
                    vec_push_budgeted(
                        out,
                        PartResult {
                            part,
                            outcome: PartOutcome::Unread(UnreadReason::GitWildcardPathspec {
                                pathspec,
                            }),
                        },
                        budget,
                    )?;
                }
            }
            return Ok(());
        }

        // Unrecognized `:` magic or extra `:` (fail closed on CR-12 subcommands).
        if after.starts_with(['~', '@', '#', '+', '=', '%', '&', '$', '\\', ':'])
            || after.contains(':')
        {
            if check_cred_names {
                push_store_hit(out, part, budget)?;
            }
            return Ok(());
        }

        // Plain `:PATH` (stage 0 object spec or pathspec)
        if check_wildcards && !literal_pathspecs && after_dd && str_has_active_wildcard(after) {
            let pathspec = charge_str_clone(arg, budget)?;
            vec_push_budgeted(
                out,
                PartResult {
                    part,
                    outcome: PartOutcome::Unread(UnreadReason::GitWildcardPathspec { pathspec }),
                },
                budget,
            )?;
            return Ok(());
        }
        if check_cred_names && git_path_is_credential_or_ancestor(after, folds, budget)? {
            push_store_hit(out, part, budget)?;
        }
        return Ok(());
    }

    // 2. `REV:PATH` object spec (before `--`, e.g. `HEAD:.env`, `HEAD:.gem/credentials`).
    if !after_dd && let Some((rev, path)) = arg.split_once(':') {
        // Note: `rev` is a revision (not a pathspec); `path` is a tree path.
        let _ = rev;
        if check_cred_names && git_path_is_credential_or_ancestor(path, folds, budget)? {
            push_store_hit(out, part, budget)?;
        }
        return Ok(());
    }

    // 3. Plain pathspec (either after `--` or a positional pathspec before `--`).
    if check_wildcards && !literal_pathspecs && str_has_active_wildcard(arg) {
        let pathspec = charge_str_clone(arg, budget)?;
        vec_push_budgeted(
            out,
            PartResult {
                part,
                outcome: PartOutcome::Unread(UnreadReason::GitWildcardPathspec { pathspec }),
            },
            budget,
        )?;
        return Ok(());
    }

    if check_cred_names && git_path_is_credential_or_ancestor(arg, folds, budget)? {
        push_store_hit(out, part, budget)?;
    }

    Ok(())
}

/// Strips leading `:(magic)` / `:!` / `:^` / `:/` prefixes from the substring
/// after the leading `:`. Returns `(Some(remaining_body), has_literal_magic)`,
/// or `(None, false)` if `:(...` is unclosed.
fn strip_pathspec_magic(after_colon: &str) -> (Option<&str>, bool) {
    let mut s = after_colon;
    let mut has_literal = false;
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
                return (None, false);
            };
            let inside = &rest[..end_idx];
            if inside.split(',').any(|k| k == "literal") {
                has_literal = true;
            }
            s = &rest[end_idx + 1..];
        } else if let Some(rest) = s.strip_prefix(['/', '!', '^']) {
            s = rest;
        } else {
            break;
        }
    }
    (Some(s), has_literal)
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

fn push_store_hit(
    out: &mut Vec<PartResult>,
    part: PartId,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    vec_push_budgeted(
        out,
        PartResult {
            part: part.clone(),
            outcome: PartOutcome::StoreHit(Hit {
                hard_deny_id: None,
                source: HitSource::Model,
                matched: HitMatch::StoreFamily {
                    family: GIT_SELECTION_FAMILY,
                },
                part,
            }),
        },
        budget,
    )
}

/// Checks whether a Git path or pathspec (after CR-19 folding and lexical `.`/
/// `..` normalization) matches:
/// 1. a credential filename (`GIT_CRED_NAMES` + CR-17, `msal_token_cache.*`,
///    or SSH private key name `id_*` excluding code/public extensions);
/// 2. a credential suffix (`GIT_CRED_SUFFIXES` + CR-17) or a proper ancestor
///    of one (e.g. `.gem`, `.terraform.d`, `.config/op`, `.config`, `.aws`);
/// 3. a credential directory (`GIT_CRED_DIRS`), a proper ancestor of one
///    (e.g. `.aws/sso`, `library`), or a file below one (keeping `.ssh`'s
///    `.pub` / `known_hosts*` / `config` / `authorized_keys` read exceptions).
fn git_path_is_credential_or_ancestor(
    path: &str,
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
        return Ok(false);
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
    budget.charge(
        total_suffixes
            .saturating_mul(parts.len().max(1) as u64)
            .saturating_mul(4),
    )?;
    for &suffix in GIT_CRED_SUFFIXES.iter().chain(CR17_GIT_CRED_SUFFIXES) {
        if path_matches_or_is_ancestor_of_pattern(&parts, suffix) {
            return Ok(true);
        }
    }

    // 3. Credential directories:
    //    - exact match or proper ancestor of dir (e.g. `.ssh`, `.aws`, `.aws/sso`, `.config`);
    //    - descendant below dir (keeping `.ssh` public/readable exceptions).
    budget.charge(
        (GIT_CRED_DIRS.len() as u64)
            .saturating_mul(parts.len().max(1) as u64)
            .saturating_mul(4),
    )?;
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
