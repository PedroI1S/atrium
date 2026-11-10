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

/// Every path the call names, ours and the gate's, plus the cwd of a command.
fn all_paths(input: &PolicyInput<'_>, a: &Analysis) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = vec![];
    for idx in 0..a.uses.len() {
        v.extend(a.all_paths(idx).into_iter().cloned());
    }
    if a.aligned.is_none() {
        v.extend(input.resolved_paths.iter().cloned());
    }
    if matches!(input.check.subject, ToolSubject::Command { .. }) {
        v.push(a.start.clone());
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
        Some(g) => !paths.is_empty() && paths.iter().all(|p| glob_matches(g, p)),
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

/// Which parts (segments, or the whole call) an allow rule covers.
fn allow_covers(
    rule: &Rule,
    input: &PolicyInput<'_>,
    a: &Analysis,
    paths: &[PathBuf],
    part_ids: &[usize],
) -> Vec<bool> {
    let n = part_ids.len().max(1);
    let none = vec![false; n];
    if !name_matches(rule, input) || matches!(input.check.subject, ToolSubject::Unknown) {
        return none;
    }
    let command = matches!(input.check.subject, ToolSubject::Command { .. });
    let simple = command && a.reasons.is_empty() && !part_ids.is_empty();
    if rule.path_scope.is_some() && command && !simple {
        return none;
    }
    if !path_scope_ok(rule, paths) {
        return none;
    }
    match &rule.matcher {
        RuleMatcher::Any => vec![true; n],
        RuleMatcher::CommandPrefix { argv } => {
            if !simple {
                return none;
            }
            part_ids
                .iter()
                .map(|&i| {
                    let u = &a.units[i];
                    u.seg.assignments.is_empty() && harmless(u) && prefix_of(&u.seg.argv, argv)
                })
                .collect()
        }
        RuleMatcher::CommandExact { argv } => {
            if !simple || argv.is_empty() {
                return none;
            }
            part_ids
                .iter()
                .map(|&i| {
                    let u = &a.units[i];
                    u.seg.assignments.is_empty() && harmless(u) && u.seg.argv == *argv
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
}

fn apply_rules(input: &PolicyInput<'_>, a: &Analysis, rules: &[Rule]) -> RuleOutcome {
    let paths = all_paths(input, a);
    let part_ids = if matches!(input.check.subject, ToolSubject::Command { .. }) {
        parts(a)
    } else {
        vec![]
    };
    let n = part_ids.len().max(1);
    let mut covered = vec![false; n];
    let mut allow_rule: Option<RuleId> = None;
    for rank in 0..3u8 {
        let ranked: Vec<&Rule> = rules
            .iter()
            .filter(|r| scope_rank(r, input) == Some(rank))
            .collect();
        if !covered.iter().all(|c| *c)
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
            };
        }
        for r in ranked.iter().filter(|r| r.decision == RuleDecision::Allow) {
            let c = allow_covers(r, input, a, &paths, &part_ids);
            for (slot, hit) in covered.iter_mut().zip(c) {
                if hit && !*slot {
                    *slot = true;
                    allow_rule.get_or_insert_with(|| r.id.clone());
                }
            }
        }
    }
    RuleOutcome {
        deny: None,
        covered,
        allow_rule,
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

const RG_BOOL_S: &str = "iSsnNHIlcovwxuaUFPq0pb";
const RG_VAL_S: &str = "egtTmABCdjMEr";
const RG_BOOL_L: &[&str] = &[
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
const RG_VAL_L: &[&str] = &[
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
const GREP_BOOL_S: &str = "ivnclLoqswxhHEFGPaIrRzZbTUy0123456789";
const GREP_VAL_S: &str = "emABCdD";
const GREP_BOOL_L: &[&str] = &[
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
const GREP_VAL_L: &[&str] = &[
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

struct Searched {
    roots: Vec<String>,
    recursive: bool,
}

/// Splits an `rg`/`grep` command into search roots; `None` for a flag it
/// doesn't know (the command then asks).
fn search_roots(is_rg: bool, args: &[String]) -> Option<Searched> {
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

    fn readonly(&self, i: usize) -> bool {
        let u = &self.a.units[i];
        if !u.seg.assignments.is_empty() || !harmless(u) || u.seg.argv.is_empty() {
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
                paths, recursive, ..
            } => match input.check.kind {
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
                if tops.is_empty() || !self.a.reasons.is_empty() || line.contains('\n') {
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
        let out = apply_rules(input, &a, rules);
        if let Some((reason, rule)) = out.deny {
            return PolicyVerdict::Deny {
                reason: reason.unwrap_or_default(),
                rule,
            };
        }
        let unknown = matches!(input.check.subject, ToolSubject::Unknown);
        let command = matches!(input.check.subject, ToolSubject::Command { .. });
        if !unknown
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
