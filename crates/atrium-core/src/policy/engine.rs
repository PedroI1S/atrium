//! `DefaultPolicy`: hard deny, then rules (thread, Home, global), then the
//! preset. Pure: everything comes in `PolicyInput`.

use std::path::{Path, PathBuf};

use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::{
    HOME_READONLY_COMMANDS, Preset, ReadOnlyCommand, Recursion, Rule, RuleDecision, RuleId,
    RuleMatcher, RuleScope, ToolSubject,
};
use globset::GlobBuilder;

use super::DefaultPolicy;
use super::deny::{self, Analysis, Sensitive};
use super::rule_check;
use super::shell::Unit;
use crate::{PolicyEngine, PolicyInput, PolicyVerdict};

fn effective_kind(input: &PolicyInput<'_>) -> ToolKind {
    match input.check.subject {
        ToolSubject::Command { .. } => ToolKind::Execute,
        _ => input.check.kind,
    }
}

fn glob_matches(glob: &str, p: &Path) -> bool {
    GlobBuilder::new(glob)
        .literal_separator(true)
        .build()
        .map(|g| g.compile_matcher().is_match(p))
        .unwrap_or(false)
}

fn looks_like_path(w: &str) -> bool {
    if w.is_empty() || w.contains(char::is_whitespace) || w.contains('\0') {
        return false;
    }
    w.starts_with(['/', '~'])
        || w == "."
        || w == ".."
        || w.starts_with("./")
        || w.starts_with("../")
        || w.starts_with("$HOME")
        || w.starts_with("${HOME}")
        || w.starts_with("$PWD")
        || w.starts_with("${PWD}")
        || (w.contains('/') && !w.contains("://"))
        || (w.starts_with('.') && w.len() > 1 && !w.starts_with(".."))
}

fn is_command_path(a: &Analysis, u: &super::words::Use) -> bool {
    if u.unit == usize::MAX {
        return true;
    }
    if a.units.get(u.unit).is_some_and(|unit| {
        unit.seg
            .redirects
            .iter()
            .any(|r| r.op != super::RedirectOp::Dup && r.target == u.word)
    }) {
        return true;
    }
    looks_like_path(&u.word)
}

/// Every path the call names, ours and the gate's (without the working folder
/// or command words that aren't paths).
fn all_paths(input: &PolicyInput<'_>, a: &Analysis) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = vec![];
    for (idx, u) in a.uses.iter().enumerate() {
        if is_command_path(a, u) {
            v.extend(a.all_paths(idx).into_iter().cloned());
        }
    }
    if a.aligned.is_none() && a.uses.is_empty() {
        v.extend(input.resolved_paths.iter().cloned());
    }
    v
}

fn name_matches(rule: &Rule, input: &PolicyInput<'_>) -> bool {
    rule.tool_kind == effective_kind(input)
        && rule
            .tool_name
            .as_ref()
            .is_none_or(|n| *n == input.check.name)
}

fn scope_rank(r: &Rule, input: &PolicyInput<'_>) -> Option<u8> {
    match &r.scope {
        RuleScope::Thread { thread_id } => (thread_id == input.thread).then_some(0),
        RuleScope::Home => (input.preset == Preset::Home).then_some(1),
        RuleScope::Global => Some(2),
    }
}

fn path_scope_ok(rule: &Rule, paths: &[PathBuf]) -> bool {
    match &rule.path_scope {
        None => true,
        Some(g) => match rule.decision {
            RuleDecision::Allow => !paths.is_empty() && paths.iter().all(|p| glob_matches(g, p)),
            RuleDecision::Deny => paths.iter().any(|p| glob_matches(g, p)),
        },
    }
}

fn prefix_of(argv: &[String], prefix: &[String]) -> bool {
    !prefix.is_empty() && argv.len() >= prefix.len() && argv.iter().zip(prefix).all(|(a, b)| a == b)
}

fn harmless(unit: &Unit) -> bool {
    unit.seg.redirects.iter().all(super::Redirect::harmless)
}

/// Top-level simple commands of a parseable line.
fn parts(a: &Analysis) -> Vec<usize> {
    a.units
        .iter()
        .enumerate()
        .filter(|(_, u)| !u.derived)
        .map(|(i, _)| i)
        .collect()
}

/// Whether a deny rule matches anywhere in the call (loosely: through
/// assignments, wrappers, substitutions).
fn deny_matches(rule: &Rule, input: &PolicyInput<'_>, a: &Analysis, paths: &[PathBuf]) -> bool {
    if !name_matches(rule, input) {
        return false;
    }
    match &rule.matcher {
        RuleMatcher::Any => path_scope_ok(rule, paths),
        RuleMatcher::CommandPrefix { argv } => {
            a.units
                .iter()
                .any(|u| prefix_of(&u.eff, argv) || prefix_of(&u.seg.argv, argv))
                && path_scope_ok(rule, paths)
        }
        RuleMatcher::CommandExact { argv } => {
            a.units
                .iter()
                .any(|u| u.eff == *argv || u.seg.argv == *argv)
                && path_scope_ok(rule, paths)
        }
        RuleMatcher::PathGlob { glob } => {
            paths.iter().any(|p| glob_matches(glob, p)) && path_scope_ok(rule, paths)
        }
    }
}

fn rule_covers_argv(
    rule: &Rule,
    argv: &[String],
    has_derived: bool,
    for_wrapper_eff: bool,
) -> bool {
    if argv.is_empty() {
        return false;
    }
    if for_wrapper_eff
        && (rule_check::is_wrapper(&argv[0]) || rule_check::is_shell_or_runs_here(&argv[0]))
    {
        return false;
    }
    match &rule.matcher {
        RuleMatcher::CommandPrefix { argv: prefix } => {
            if !prefix_of(argv, prefix) || rule_check::command_prefix_blocked(argv, prefix) {
                return false;
            }
            if rule_check::is_find(&argv[0])
                && !has_derived
                && argv[1..].iter().any(|w| {
                    matches!(
                        w.to_lowercase().as_str(),
                        "-exec" | "-execdir" | "-ok" | "-okdir"
                    )
                })
            {
                return false;
            }
            true
        }
        RuleMatcher::CommandExact { argv: exact } => {
            !exact.is_empty()
                && argv == exact.as_slice()
                && !rule_check::command_exact_blocked(argv)
        }
        RuleMatcher::Any | RuleMatcher::PathGlob { .. } => false,
    }
}

/// Which slots in `unit_ids` (or the whole call when `unit_ids` is empty) an
/// allow rule covers. When `unit_ids` contains command units, `as_eff` selects
/// `u.eff` (for wrapper targets) instead of `u.seg.argv`.
fn allow_covers(
    rule: &Rule,
    input: &PolicyInput<'_>,
    a: &Analysis,
    paths: &[PathBuf],
    unit_ids: &[usize],
    glob_leaves: bool,
    has_derived: bool,
    as_eff: bool,
) -> Vec<bool> {
    let n = unit_ids.len().max(1);
    let none = vec![false; n];
    if !name_matches(rule, input) || matches!(input.check.subject, ToolSubject::Unknown) {
        return none;
    }
    // D-100: an escaping search glob is covered only by an unscoped `Any` rule.
    if glob_leaves && !(matches!(rule.matcher, RuleMatcher::Any) && rule.path_scope.is_none()) {
        return none;
    }
    let command = matches!(input.check.subject, ToolSubject::Command { .. });
    // an uncertain cwd: only `Any` covers it (contract §9: the line asks)
    let simple = command && a.reasons.is_empty() && !parts(a).is_empty() && !a.cwd_uncertain;
    if rule.path_scope.is_some() && command && !simple {
        return none;
    }
    if !path_scope_ok(rule, paths) {
        return none;
    }
    match &rule.matcher {
        RuleMatcher::Any => vec![true; n],
        RuleMatcher::CommandPrefix { .. } | RuleMatcher::CommandExact { .. } => {
            if !simple || unit_ids.is_empty() {
                return none;
            }
            unit_ids
                .iter()
                .map(|&i| {
                    let u = &a.units[i];
                    let argv = if as_eff { &u.eff } else { &u.seg.argv };
                    u.seg.assignments.is_empty()
                        && harmless(u)
                        && rule_covers_argv(rule, argv, has_derived, as_eff)
                })
                .collect()
        }
        RuleMatcher::PathGlob { glob } => {
            let ok = !paths.is_empty()
                && (!command || simple)
                && paths.iter().all(|p| glob_matches(glob, p));
            vec![ok; n]
        }
    }
}

struct RuleOutcome {
    deny: Option<(Option<String>, Option<RuleId>)>,
    covered: Vec<bool>,
    allow_rule: Option<RuleId>,
    any_allow: bool,
}

fn apply_rules(
    input: &PolicyInput<'_>,
    a: &Analysis,
    rules: &[Rule],
    glob_leaves: bool,
) -> RuleOutcome {
    let paths = all_paths(input, a);
    let command = matches!(input.check.subject, ToolSubject::Command { .. });
    let part_ids = if command { parts(a) } else { vec![] };
    let derived_ids: Vec<usize> = if command {
        a.units
            .iter()
            .enumerate()
            .filter(|(_, u)| u.derived)
            .map(|(i, _)| i)
            .collect()
    } else {
        vec![]
    };
    let wrapper_eff_ids: Vec<usize> = if command {
        a.units
            .iter()
            .enumerate()
            .filter(|(_, u)| {
                u.eff != u.seg.argv
                    || u.seg
                        .argv
                        .first()
                        .is_some_and(|f| rule_check::is_wrapper(f))
            })
            .map(|(i, _)| i)
            .collect()
    } else {
        vec![]
    };
    let has_derived = !derived_ids.is_empty();
    let n = part_ids.len().max(1);
    let mut covered = vec![false; n];
    let mut top_non_cmd = vec![false; n];
    let mut derived_covered = vec![false; derived_ids.len()];
    let mut eff_covered = vec![false; wrapper_eff_ids.len()];
    let mut allow_rule: Option<RuleId> = None;
    let mut any_allow = false;
    for rank in 0..3u8 {
        let ranked: Vec<&Rule> = rules
            .iter()
            .filter(|r| scope_rank(r, input) == Some(rank))
            .collect();
        let all_done = covered.iter().all(|c| *c)
            && derived_covered.iter().all(|c| *c)
            && eff_covered.iter().all(|c| *c);
        if !all_done
            && let Some(r) = ranked
                .iter()
                .find(|r| r.decision == RuleDecision::Deny && deny_matches(r, input, a, &paths))
        {
            return RuleOutcome {
                deny: Some((
                    Some(format!("denied by a rule ({:?})", r.matcher)),
                    Some(r.id.clone()),
                )),
                covered,
                allow_rule,
                any_allow,
            };
        }
        for r in ranked.iter().filter(|r| r.decision == RuleDecision::Allow) {
            let c = allow_covers(
                r,
                input,
                a,
                &paths,
                &part_ids,
                glob_leaves,
                has_derived,
                false,
            );
            let is_any = matches!(r.matcher, RuleMatcher::Any)
                && r.path_scope.is_none()
                && !c.is_empty()
                && c.iter().all(|hit| *hit);
            let is_non_cmd = matches!(r.matcher, RuleMatcher::Any | RuleMatcher::PathGlob { .. });
            if is_any {
                any_allow = true;
            }
            for (k, (slot, hit)) in covered.iter_mut().zip(&c).enumerate() {
                if *hit && !*slot {
                    *slot = true;
                    if is_non_cmd {
                        top_non_cmd[k] = true;
                    }
                    allow_rule.get_or_insert_with(|| r.id.clone());
                }
            }
            if !derived_ids.is_empty() {
                let dc = allow_covers(
                    r,
                    input,
                    a,
                    &paths,
                    &derived_ids,
                    glob_leaves,
                    has_derived,
                    false,
                );
                for (slot, hit) in derived_covered.iter_mut().zip(dc) {
                    if hit && !*slot {
                        *slot = true;
                    }
                }
            }
            if !wrapper_eff_ids.is_empty() {
                let ec = allow_covers(
                    r,
                    input,
                    a,
                    &paths,
                    &wrapper_eff_ids,
                    glob_leaves,
                    has_derived,
                    true,
                );
                for (slot, hit) in eff_covered.iter_mut().zip(ec) {
                    if hit && !*slot {
                        *slot = true;
                    }
                }
            }
        }
    }
    if !derived_covered.iter().all(|c| *c) || !eff_covered.iter().all(|c| *c) {
        for (slot, &non_cmd) in covered.iter_mut().zip(&top_non_cmd) {
            if !non_cmd {
                *slot = false;
            }
        }
    }
    RuleOutcome {
        deny: None,
        covered,
        allow_rule,
        any_allow,
    }
}

// ---------------------------------------------------------------------------
// Home's read-only commands
// ---------------------------------------------------------------------------

fn forbidden_flag(entry: &ReadOnlyCommand, args: &[String]) -> bool {
    let shorts: Vec<char> = entry
        .forbidden
        .iter()
        .filter(|f| f.len() == 2 && !f.starts_with("--"))
        .filter_map(|f| f.chars().nth(1))
        .collect();
    let longs: Vec<&str> = entry
        .forbidden
        .iter()
        .copied()
        .filter(|f| f.starts_with("--"))
        .collect();
    for a in args {
        if a == "--" {
            break;
        }
        if let Some(rest) = a.strip_prefix("--") {
            let name = format!("--{}", rest.split('=').next().unwrap_or(rest));
            if longs
                .iter()
                .any(|l| *l == name || (name.len() >= 3 && l.starts_with(&name)))
            {
                return true;
            }
        } else if a.starts_with('-') && a.len() > 1 && a[1..].chars().any(|c| shorts.contains(&c)) {
            return true;
        }
    }
    false
}

pub(crate) const RG_BOOL_S: &str = "iSsnNHIlcovwxuaUFPq0pb";
pub(crate) const RG_VAL_S: &str = "egtTmABCdjMEr";
pub(crate) const RG_BOOL_L: &[&str] = &[
    "hidden",
    "no-ignore",
    "no-ignore-vcs",
    "no-ignore-dot",
    "no-ignore-parent",
    "no-ignore-global",
    "no-ignore-files",
    "no-messages",
    "no-heading",
    "heading",
    "line-number",
    "no-line-number",
    "with-filename",
    "no-filename",
    "ignore-case",
    "smart-case",
    "case-sensitive",
    "count",
    "count-matches",
    "files-with-matches",
    "files-without-match",
    "only-matching",
    "invert-match",
    "word-regexp",
    "line-regexp",
    "fixed-strings",
    "multiline",
    "multiline-dotall",
    "text",
    "binary",
    "unrestricted",
    "files",
    "type-list",
    "json",
    "column",
    "no-column",
    "vimgrep",
    "trim",
    "pretty",
    "quiet",
    "stats",
    "null",
    "no-config",
    "version",
    "help",
    "pcre2",
    "no-pcre2",
    "crlf",
    "include-zero",
    "byte-offset",
    "glob-case-insensitive",
    "no-glob-case-insensitive",
];
pub(crate) const RG_VAL_L: &[&str] = &[
    "regexp",
    "glob",
    "iglob",
    "type",
    "type-not",
    "max-count",
    "after-context",
    "before-context",
    "context",
    "max-depth",
    "maxdepth",
    "threads",
    "max-columns",
    "encoding",
    "replace",
    "colors",
    "color",
    "sort",
    "sortr",
    "max-filesize",
    "path-separator",
    "engine",
    "context-separator",
    "type-add",
    "type-clear",
    "field-context-separator",
    "field-match-separator",
    "dfa-size-limit",
    "regex-size-limit",
];
pub(crate) const GREP_BOOL_S: &str = "ivnclLoqswxhHEFGPaIrRzZbTUy0123456789";
pub(crate) const GREP_VAL_S: &str = "emABCdD";
pub(crate) const GREP_BOOL_L: &[&str] = &[
    "ignore-case",
    "no-ignore-case",
    "invert-match",
    "line-number",
    "count",
    "files-with-matches",
    "files-without-match",
    "only-matching",
    "quiet",
    "silent",
    "no-messages",
    "word-regexp",
    "line-regexp",
    "no-filename",
    "with-filename",
    "extended-regexp",
    "fixed-strings",
    "basic-regexp",
    "perl-regexp",
    "text",
    "null-data",
    "null",
    "byte-offset",
    "initial-tab",
    "recursive",
    "dereference-recursive",
    "no-group-separator",
];
pub(crate) const GREP_VAL_L: &[&str] = &[
    "regexp",
    "max-count",
    "after-context",
    "before-context",
    "context",
    "directories",
    "devices",
    "include",
    "exclude",
    "exclude-dir",
    "label",
    "color",
    "colour",
    "binary-files",
    "group-separator",
];

/// GNU `grep`'s long option names (both tables), for reading a prefix.
pub(crate) fn grep_long_names() -> impl Iterator<Item = &'static str> {
    GREP_BOOL_L.iter().chain(GREP_VAL_L).copied()
}

pub(crate) struct Searched {
    pub(crate) roots: Vec<String>,
    pub(crate) recursive: bool,
}

/// Splits an `rg`/`grep` command into search roots; `None` for a flag it
/// doesn't know (the command then asks).
pub(crate) fn search_roots(is_rg: bool, args: &[String]) -> Option<Searched> {
    roots_of(is_rg, false, args)
}

/// [`search_roots`] for `rg` that also reads `-L` / `--follow` as plain
/// flags (round-8 major 5, D-098): the gate checks the links they follow
/// itself, and the flag/value pairing stays the parser's, so a `-L` given
/// as the value of `-e`, `-g`, `-r`, `--file`, … stays a value.
pub(crate) fn rg_roots_following(args: &[String]) -> Option<Searched> {
    roots_of(true, true, args)
}

fn roots_of(is_rg: bool, follow: bool, args: &[String]) -> Option<Searched> {
    let (bs, vs, bl, vl) = if is_rg {
        (RG_BOOL_S, RG_VAL_S, RG_BOOL_L, RG_VAL_L)
    } else {
        (GREP_BOOL_S, GREP_VAL_S, GREP_BOOL_L, GREP_VAL_L)
    };
    let mut positional: Vec<String> = vec![];
    let mut have_pattern_flag = false;
    let mut files_mode = false;
    let mut recursive = is_rg;
    let mut i = 0;
    let mut dd = false;
    while i < args.len() {
        let a = &args[i];
        i += 1;
        if dd || !a.starts_with('-') || a == "-" {
            if a == "-" {
                return None;
            }
            positional.push(a.clone());
            continue;
        }
        if a == "--" {
            dd = true;
        } else if let Some(rest) = a.strip_prefix("--") {
            let (name, has_val) = match rest.split_once('=') {
                Some((n, _)) => (n, true),
                None => (rest, false),
            };
            if follow && name == "follow" && !has_val {
                continue;
            }
            if name == "files" || name == "type-list" {
                files_mode = true;
            }
            if !is_rg && name.len() >= 3 && "dereference-recursive".starts_with(name) {
                return None; // follows every symlink below the root
            }
            if name == "regexp" {
                have_pattern_flag = true;
            }
            if matches!(name, "recursive" | "dereference-recursive" | "directories")
                || (!is_rg
                    && name.len() >= 3
                    && ("recursive".starts_with(name)
                        || "dereference-recursive".starts_with(name)
                        || "directories".starts_with(name))
                    && !bl.contains(&name)
                    && !vl.contains(&name))
            {
                recursive = true;
            }
            if vl.contains(&name) {
                if !has_val {
                    i += 1;
                }
            } else if !bl.contains(&name) {
                return None;
            }
        } else {
            let cs: Vec<char> = a[1..].chars().collect();
            for (k, c) in cs.iter().enumerate() {
                if !is_rg && *c == 'R' {
                    return None; // follows every symlink below the root
                }
                if !is_rg && matches!(c, 'r' | 'd') {
                    recursive = true;
                }
                if follow && *c == 'L' {
                    continue;
                }
                if *c == 'e' {
                    have_pattern_flag = true;
                }
                if vs.contains(*c) {
                    if k + 1 == cs.len() {
                        i += 1;
                    }
                    break;
                } else if !bs.contains(*c) {
                    return None;
                }
            }
        }
    }
    let skip = usize::from(!have_pattern_flag && !files_mode);
    let roots = positional.into_iter().skip(skip).collect();
    Some(Searched { roots, recursive })
}

/// Credential names a committed file can have (CR-12: `git show
/// REV:PATH` of a path whose name matches a credential asks): the
/// hard-deny list's files and the stores that ask, by their path below home
/// or, for names that are distinctive, by file name alone; private keys
/// (`id_*` but `.pub`); anything below a credential folder.
const GIT_CRED_SUFFIXES: &[&str] = &[
    ".aws/credentials",
    ".docker/config.json",
    ".kube/config",
    ".config/gh/hosts.yml",
    ".claude/.credentials.json",
    ".codex/auth.json",
    ".cargo/credentials",
    ".cargo/credentials.toml",
    ".config/git/credentials",
    ".azure/accesstokens.json",
    ".gem/credentials",
    ".terraform.d/credentials.tfrc.json",
    ".config/op/config",
];
const GIT_CRED_NAMES: &[&str] = &[
    ".netrc",
    ".git-credentials",
    ".npmrc",
    ".claude.json",
    ".pypirc",
    ".pgpass",
    ".vault-token",
    ".env",
    ".yarnrc.yml",
    "credentials.tfrc.json",
    ".bash_history",
    ".zsh_history",
    ".zhistory",
    ".sh_history",
    ".ksh_history",
    ".history",
    "fish_history",
    "accesstokens.json",
];
const GIT_CRED_DIRS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".config/gcloud",
    ".gemini",
    ".aws/sso/cache",
    ".zsh_sessions",
    ".bash_sessions",
    "library/keychains",
];

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

/// Whether a `git show`/`git log`/`git diff` argument names a committed file
/// whose name matches a credential: `REV:PATH`, `:PATH`, `:N:PATH` or a
/// pathspec (including `:(magic)PATH` and short magic `:/`, `:!`, `:^`).
/// Extracts `<file>` from `git log -L` / `--line-range` (`start,end:file` or `:funcname:file`).
fn parse_git_line_range(spec: &str) -> Option<&str> {
    if let Some(after) = spec.strip_prefix(':') {
        let (func, file) = after.rsplit_once(':')?;
        (!func.is_empty() && !file.is_empty()).then_some(file)
    } else {
        let (range, file) = spec.rsplit_once(':')?;
        (!range.is_empty() && !file.is_empty()).then_some(file)
    }
}

/// Whether `git show`/`git log`/`git diff` arguments name a committed file
/// whose name matches a credential.
fn git_args_name_credential(args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            return args[i + 1..].iter().any(|a| git_names_credential(a));
        }
        let line_spec = if arg == "-L" || arg == "--line-range" {
            i += 1;
            match args.get(i) {
                Some(s) => Some(s.as_str()),
                None => return true,
            }
        } else if let Some(s) = arg.strip_prefix("-L") {
            Some(s)
        } else {
            arg.strip_prefix("--line-range=")
        };
        if let Some(spec) = line_spec {
            let Some(file) = parse_git_line_range(spec) else {
                return true;
            };
            if git_path_is_credential(file) {
                return true;
            }
        } else if git_names_credential(arg) {
            return true;
        }
        i += 1;
    }
    false
}

fn git_names_credential(arg: &str) -> bool {
    if arg.starts_with('-') {
        return false;
    }
    let path = if let Some(after) = arg.strip_prefix(':') {
        if let Some((n, p)) = after.split_once(':')
            && !n.is_empty()
            && n.chars().all(|c| c.is_ascii_digit())
        {
            p
        } else {
            let mut s = after;
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
                        // Unclosed `:(...` pathspec magic: fail closed.
                        return true;
                    };
                    s = &rest[end_idx + 1..];
                } else if let Some(rest) = s.strip_prefix(['/', '!', '^', ':']) {
                    s = rest;
                } else {
                    break;
                }
            }
            if s.starts_with(['~', '@', '#', '+', '=', '%', '&', '$', '\\']) || s.contains(':') {
                // Unrecognized `:` magic or unparsed prefix: fail closed.
                return true;
            }
            s
        }
    } else if let Some((_, rest)) = arg.split_once(':') {
        rest
    } else {
        arg
    };
    git_path_is_credential(path)
}

fn git_path_is_credential(path: &str) -> bool {
    let l = path.to_lowercase().replace('ſ', "s");
    let mut parts: Vec<&str> = vec![];
    for c in l.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(c),
        }
    }
    let Some(name) = parts.last() else {
        return false;
    };
    let joined = parts.join("/");
    if GIT_CRED_NAMES.contains(name)
        || name.starts_with("msal_token_cache.")
        || is_git_key_name(name)
    {
        return true;
    }
    if GIT_CRED_SUFFIXES
        .iter()
        .any(|s| joined == *s || joined.ends_with(&format!("/{s}")))
    {
        return true;
    }
    // below a credential folder (`.ssh/x`, `a/.gnupg/y`), but not public
    // ssh files
    GIT_CRED_DIRS.iter().any(|d| {
        let at = |i: usize| parts[i..].join("/");
        (0..parts.len()).any(|i| {
            let rest = at(i);
            rest.strip_prefix(d)
                .and_then(|r| r.strip_prefix('/'))
                .is_some_and(|r| {
                    !r.is_empty()
                        && !(*d == ".ssh"
                            && (name.ends_with(".pub")
                                || name.starts_with("known_hosts")
                                || matches!(*name, "config" | "authorized_keys")))
                })
        })
    })
}

struct Home<'a> {
    input: &'a PolicyInput<'a>,
    a: &'a Analysis,
    s: Sensitive,
}

impl Home<'_> {
    /// Strictly inside home, and not a place a search could reach credentials
    /// from.
    fn root_ok(&self, p: &Path) -> bool {
        let h = self.input.home;
        p.starts_with(h) && p != h && !self.s.is_or_contains(p)
    }

    fn word_paths(&self, unit: usize, word: &str) -> Vec<PathBuf> {
        let mut out = vec![];
        if let Some(idxs) = self.a.use_index.get(&(unit, word.to_string())) {
            for &idx in idxs {
                out.extend(self.a.all_paths(idx).into_iter().cloned());
            }
        }
        out
    }

    /// Whether a path the call names is a credential store that asks
    /// (CR-12): ours, the gate's (symlinks resolved, hard links mapped onto
    /// the store), and, when the gate's list didn't line up, its flat list.
    fn names_ask_store(&self, unit: Option<usize>) -> bool {
        let hit = self.a.uses.iter().enumerate().any(|(idx, u)| {
            unit.is_none_or(|i| u.unit == i)
                && self.a.all_paths(idx).iter().any(|p| self.s.ask_hit(p))
        });
        hit || (self.a.aligned.is_none()
            && self.input.resolved_paths.iter().any(|p| self.s.ask_hit(p)))
    }

    fn readonly(&self, i: usize) -> bool {
        let u = &self.a.units[i];
        if !u.seg.assignments.is_empty() || !harmless(u) || u.seg.argv.is_empty() {
            return false;
        }
        if self.names_ask_store(Some(i)) {
            return false;
        }
        let argv = &u.seg.argv;
        let Some(entry) = HOME_READONLY_COMMANDS.iter().find(|e| {
            argv.len() >= e.prefix.len() && e.prefix.iter().zip(argv).all(|(p, a)| p == a)
        }) else {
            return false;
        };
        let args = &argv[entry.prefix.len()..];
        if forbidden_flag(entry, args) {
            return false;
        }
        if let Some(only) = entry.only
            && !args.iter().all(|a| only.contains(&a.as_str()))
        {
            return false;
        }
        if args
            .iter()
            .any(|a| a.starts_with("--exec-path") || a.starts_with("--config-env"))
        {
            return false;
        }
        // `git show HEAD:.npmrc`: a committed file named like a credential
        // (CR-12)
        if entry.prefix.first() == Some(&"git") && git_args_name_credential(args) {
            return false;
        }
        let is_rg = entry.prefix == ["rg"];
        match entry.recursion {
            Recursion::Never => true,
            Recursion::Always | Recursion::WithFlags(_) => {
                let Some(s) = search_roots(is_rg, args) else {
                    return false;
                };
                if !s.recursive {
                    return true;
                }
                if s.roots.is_empty() {
                    return false;
                }
                s.roots.iter().all(|r| {
                    let ps = self.word_paths(i, r);
                    !ps.is_empty() && ps.iter().all(|p| self.root_ok(p))
                })
            }
        }
    }

    fn evaluate(&self, covered: &[bool], allow_rule: Option<RuleId>) -> PolicyVerdict {
        let input = self.input;
        match &input.check.subject {
            ToolSubject::None => PolicyVerdict::Allow { rule: None },
            ToolSubject::Paths {
                paths,
                recursive,
                glob,
            } => match input.check.kind {
                // a credential store that asks (CR-12)
                ToolKind::Read | ToolKind::Search if self.names_ask_store(None) => {
                    PolicyVerdict::Ask
                }
                ToolKind::Search
                    if glob.as_ref().is_some_and(|g| {
                        let roots: Vec<PathBuf> = if paths.is_empty() {
                            vec![input.cwd.to_path_buf()]
                        } else {
                            (0..self.a.uses.len())
                                .flat_map(|i| self.a.all_paths(i).into_iter().cloned())
                                .collect()
                        };
                        self.s
                            .ask_search_glob_hit(&roots, g, self.a.glob_search.as_ref())
                    }) =>
                {
                    PolicyVerdict::Ask
                }
                ToolKind::Read => PolicyVerdict::Allow { rule: None },
                ToolKind::Search => {
                    if *recursive {
                        let roots: Vec<PathBuf> = if paths.is_empty() {
                            vec![input.cwd.to_path_buf()]
                        } else {
                            (0..self.a.uses.len())
                                .flat_map(|i| self.a.all_paths(i).into_iter().cloned())
                                .collect()
                        };
                        if !roots.iter().all(|r| self.root_ok(r)) {
                            return PolicyVerdict::Ask;
                        }
                    }
                    PolicyVerdict::Allow { rule: None }
                }
                _ => PolicyVerdict::Ask,
            },
            ToolSubject::Command { line, .. } => {
                let tops = parts(self.a);
                if tops.is_empty()
                    || !self.a.reasons.is_empty()
                    || self.a.cwd_uncertain
                    || line.contains('\n')
                {
                    return PolicyVerdict::Ask;
                }
                let mut used_rule = None;
                for (k, &i) in tops.iter().enumerate() {
                    if covered.get(k).copied().unwrap_or(false) {
                        used_rule = used_rule.or_else(|| allow_rule.clone());
                    } else if !self.readonly(i) {
                        return PolicyVerdict::Ask;
                    }
                }
                PolicyVerdict::Allow { rule: used_rule }
            }
            _ => PolicyVerdict::Ask,
        }
    }
}

impl PolicyEngine for DefaultPolicy {
    fn evaluate(&self, input: &PolicyInput<'_>, rules: &[Rule]) -> PolicyVerdict {
        let a = deny::analyze(input);
        if a.overflow {
            return PolicyVerdict::Deny {
                reason: "too large to check".into(),
                rule: None,
            };
        }
        if let Some(id) = deny::hard_deny(input, &a) {
            return PolicyVerdict::HardDeny { id };
        }
        let esc = a.glob_escape.as_ref();
        let glob_leaves = esc.is_some_and(deny::GlobEscape::leaves);
        let glob_unbounded =
            esc.is_some_and(|e| e.unbounded) || a.glob_search.as_ref().is_some_and(|(_, u)| *u);
        let out = apply_rules(input, &a, rules, glob_leaves);
        if let Some((reason, rule)) = out.deny {
            return PolicyVerdict::Deny {
                reason: reason.unwrap_or_default(),
                rule,
            };
        }
        // A search glob that can name files outside every path the subject
        // gives (M1-18, P1; D-100): hard-denied (above) if an expansion hits
        // the hard-deny list, otherwise allowed by FullAccess or an Any rule
        // and asked in Home/Normal/Trusted/Careful and under prefix/scoped rules.
        // When glob expansion overflows (brace or variant cap), Ask everywhere.
        let unknown = matches!(input.check.subject, ToolSubject::Unknown)
            || a.unbounded_unknown_dangerous
            || glob_unbounded;
        let command = matches!(input.check.subject, ToolSubject::Command { .. });
        if !unknown
            && (!glob_leaves || out.any_allow)
            && out.covered.iter().all(|c| *c)
            && (command && !parts(&a).is_empty() || !command)
        {
            return PolicyVerdict::Allow {
                rule: out.allow_rule,
            };
        }
        if unknown {
            return PolicyVerdict::Ask;
        }
        if glob_leaves {
            return if input.preset == Preset::FullAccess {
                PolicyVerdict::Allow { rule: None }
            } else {
                PolicyVerdict::Ask
            };
        }
        match input.preset {
            Preset::Home | Preset::Normal | Preset::Trusted => {
                let h = Home {
                    input,
                    a: &a,
                    s: deny::sensitive(input),
                };
                h.evaluate(&out.covered, out.allow_rule)
            }
            Preset::Careful => {
                if matches!(input.check.subject, ToolSubject::None) {
                    PolicyVerdict::Allow { rule: None }
                } else {
                    PolicyVerdict::Ask
                }
            }
            Preset::FullAccess => PolicyVerdict::Allow { rule: None },
        }
    }
}
