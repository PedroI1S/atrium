//! The words of a command that name paths, in a fixed order, and the working
//! directory each is resolved against. The gate resolves them against the
//! filesystem; the engine redoes it lexically. Both run [`walk`], so the
//! gate's `resolved_paths` line up with the engine's words one to one.

use std::path::{Component, Path, PathBuf};

use super::RedirectOp;
use super::shell::{Unit, base};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Read,
    /// Known to write (redirects, `rm`, `cp`, `of=`, …).
    Write,
    /// A command that isn't a known reader: only Atrium's protected paths
    /// treat it as a write.
    MaybeWrite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CwdState {
    Known(PathBuf),
    /// After a `cd` the gate can't follow: every directory the command could
    /// be in (home, the start, every directory seen so far).
    Unknown(Vec<PathBuf>),
}

impl CwdState {
    pub fn candidates(&self) -> Vec<&PathBuf> {
        match self {
            CwdState::Known(p) => vec![p],
            CwdState::Unknown(v) => v.iter().collect(),
        }
    }
    pub fn is_unknown(&self) -> bool {
        matches!(self, CwdState::Unknown(_))
    }
}

#[derive(Debug, Clone)]
pub struct Use {
    pub word: String,
    pub role: Role,
    pub unit: usize,
    /// One resolution per cwd candidate.
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Default)]
pub struct Walk {
    pub uses: Vec<Use>,
    pub unit_cwd: Vec<CwdState>,
    pub unknown: bool,
    pub total: usize,
    /// The call is bigger than the checker will take on.
    pub overflow: bool,
}

pub const MAX_USES: usize = 4096;
pub const MAX_PATHS: usize = 16384;
const MAX_CANDIDATES: usize = 16;
const MAX_BRACE_RESULTS: usize = 256;

pub type Resolver<'a> = &'a dyn Fn(&str, &Path, &Path) -> PathBuf;

const WRITERS: &[&str] = &[
    "rm",
    "mv",
    "cp",
    "tee",
    "touch",
    "chmod",
    "chown",
    "chgrp",
    "ln",
    "truncate",
    "install",
    "mkdir",
    "rmdir",
    "rsync",
    "unlink",
    "shred",
    "patch",
    "ed",
    "ex",
    "wipefs",
    "blkdiscard",
];

/// Commands that only read what they are given. For Atrium's protected
/// paths, anything else counts as a writer.
const READERS: &[&str] = &[
    "cat",
    "head",
    "tail",
    "less",
    "more",
    "wc",
    "ls",
    "stat",
    "file",
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "ag",
    "ack",
    "diff",
    "cmp",
    "md5",
    "md5sum",
    "shasum",
    "sha1sum",
    "sha256sum",
    "du",
    "df",
    "tree",
    "echo",
    "printf",
    "which",
    "whereis",
    "type",
    "readlink",
    "realpath",
    "basename",
    "dirname",
    "test",
    "[",
    "pwd",
    "sort",
    "uniq",
    "cut",
    "tr",
    "nl",
    "od",
    "xxd",
    "hexdump",
    "strings",
    "bat",
    "jq",
    "column",
    "fold",
    "expand",
    "true",
    "false",
    "id",
    "whoami",
    "uname",
    "date",
    "cd",
    "pushd",
    "popd",
    "find",
    "open",
    "spctl",
    "mdls",
    "mdfind",
    "codesign",
    "awk",
    "gawk",
    "mawk",
    "sed",
    "perl",
];

/// Whether this command might write to the paths it is given: everything
/// that isn't a known reader, and the write forms of the readers
/// (`sort -o`, `find -delete`, …).
pub fn is_writer(eff: &[String]) -> bool {
    let Some(first) = eff.first() else {
        return false;
    };
    let b = base(first);
    if is_known_writer(eff) {
        return true;
    }
    let args = &eff[1..];
    let flag = |a: &String, c: char| a.starts_with('-') && !a.starts_with("--") && a.contains(c);
    let operands = args
        .iter()
        .filter(|a| !a.starts_with('-') || *a == "-")
        .count();
    // `--out`, `--o`: GNU long options take unique prefixes
    let long_is = |a: &String, full: &str| {
        let n = a.split('=').next().unwrap_or(a);
        n.starts_with("--") && n.len() >= 3 && full.starts_with(n)
    };
    match b {
        "sort" => args.iter().any(|a| flag(a, 'o') || long_is(a, "--output")),
        "uniq" => operands >= 2,
        "find" => args.iter().any(|a| {
            a == "-delete"
                || a.starts_with("-exec")
                || a.starts_with("-ok")
                || a.starts_with("-fprint")
                || a == "-fls"
        }),
        "xxd" => operands >= 2,
        "tree" => args.iter().any(|a| flag(a, 'o') || long_is(a, "--output")),
        "sed" => args.iter().any(|a| {
            let script = if let Some(r) = a.strip_prefix("--expression=") {
                Some(r)
            } else if a.starts_with("-e") && a.len() > 2 {
                Some(&a[2..])
            } else if !a.starts_with('-') && (a.contains(char::is_whitespace) || !a.contains('/')) {
                Some(a.as_str())
            } else {
                None
            };
            script.is_some_and(|s| s.contains(['w', 'W']))
        }),
        "awk" | "gawk" | "mawk" => args.iter().any(|a| {
            // gawk's `-i inplace` / `--include=inplace` rewrites its file
            a.starts_with("-i")
                || a.starts_with("--include")
                || a.contains("inplace")
                || (!a.starts_with('-') && a.contains(['>', '|']))
                || a.contains("system(")
        }),
        "codesign" => {
            let signs = args.iter().any(|a| {
                matches!(a.as_str(), "--sign" | "--force" | "--remove-signature")
                    || flag(a, 's')
                    || flag(a, 'f')
            });
            signs
                || !args.iter().any(|a| {
                    a.starts_with("-d")
                        || a.starts_with("-v")
                        || a == "--display"
                        || a == "--verify"
                })
        }
        _ => !READERS.contains(&b),
    }
}

/// Commands known to write to a path or device they are given.
pub fn is_known_writer(eff: &[String]) -> bool {
    let Some(first) = eff.first() else {
        return false;
    };
    let b = base(first);
    if WRITERS.contains(&b) {
        return true;
    }
    match b {
        "sed" => eff.iter().skip(1).any(|a| {
            a == "-i" || a.starts_with("-i") && !a.starts_with("--") || a.starts_with("--in-place")
        }),
        "perl" => eff.iter().skip(1).any(|a| {
            a.starts_with("-i") || (a.starts_with('-') && !a.starts_with("--") && a.contains('i'))
        }),
        _ => false,
    }
}

/// `$HOME`, `${HOME}`, `$PWD` and a leading `~` expanded.
pub fn expand_home(word: &str, home: &Path, cwd: &Path) -> String {
    let h = home.to_string_lossy();
    let c = cwd.to_string_lossy();
    if let Some(after) = word.strip_prefix('~') {
        let (name, rest) = match after.find('/') {
            Some(i) => (&after[..i], &after[i..]),
            None => (after, ""),
        };
        let user = home.file_name().map(|n| n.to_string_lossy().into_owned());
        if name.is_empty() || user.as_deref() == Some(name) {
            return format!("{h}{rest}");
        }
        if name == "+" || name == "-" {
            return format!("{c}{rest}");
        }
        return word.to_string();
    }
    for (var, val) in [
        ("$HOME", &*h),
        ("${HOME}", &*h),
        ("$PWD", &*c),
        ("${PWD}", &*c),
    ] {
        if let Some(rest) = word.strip_prefix(var)
            && (rest.is_empty() || rest.starts_with('/'))
        {
            return format!("{val}{rest}");
        }
    }
    word.to_string()
}

pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                if !out.pop() && !out.has_root() {
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// Pure resolution: expands `~`/`$HOME`, joins onto `cwd`, removes `.`/`..`.
pub fn lexical_resolve(word: &str, cwd: &Path, home: &Path) -> PathBuf {
    let w = expand_home(word, home, cwd);
    let p = Path::new(&w);
    if p.is_absolute() {
        normalize(p)
    } else {
        normalize(&cwd.join(p))
    }
}

fn has_unknown_var(w: &str) -> bool {
    w.contains('$') || w.contains('`')
}

/// `a{b,c}d` → `abd`, `acd` (comma lists only). `None` when it would make
/// more than a few hundred words.
fn brace_expand(w: &str) -> Option<Vec<String>> {
    let mut work = vec![w.to_string()];
    let mut out = vec![];
    while let Some(x) = work.pop() {
        let cs: Vec<char> = x.chars().collect();
        // matching braces and their top-level commas, in one pass
        let mut stack: Vec<(usize, Vec<usize>)> = vec![];
        let mut best: Option<(usize, usize, Vec<usize>)> = None;
        for (i, &c) in cs.iter().enumerate() {
            match c {
                '{' => stack.push((i, vec![])),
                ',' => {
                    if let Some(top) = stack.last_mut() {
                        top.1.push(i);
                    }
                }
                '}' => {
                    if let Some((open, commas)) = stack.pop()
                        && !commas.is_empty()
                        && (open == 0 || cs[open - 1] != '$')
                        && best.as_ref().is_none_or(|b| open < b.0)
                    {
                        best = Some((open, i, commas));
                    }
                }
                _ => {}
            }
        }
        match best {
            None => out.push(x),
            Some((open, close, commas)) => {
                let pre: String = cs[..open].iter().collect();
                let post: String = cs[close + 1..].iter().collect();
                let mut bounds = vec![open];
                bounds.extend(commas);
                bounds.push(close);
                for k in 0..bounds.len() - 1 {
                    let alt: String = cs[bounds[k] + 1..bounds[k + 1]].iter().collect();
                    work.push(format!("{pre}{alt}{post}"));
                }
            }
        }
        if work.len() + out.len() > MAX_BRACE_RESULTS {
            return None;
        }
    }
    Some(out)
}

/// Words that name paths, in the order the gate and the engine both use.
fn words_of(unit: &Unit, overflow: &mut bool) -> Vec<(String, Role)> {
    let mut out = vec![];
    for (w, role) in raw_words_of(unit) {
        match brace_expand(&w) {
            Some(v) => out.extend(v.into_iter().map(|e| (e, role))),
            None => {
                *overflow = true;
                return out;
            }
        }
        if out.len() > MAX_USES {
            *overflow = true;
            return out;
        }
    }
    out
}

/// Words inside a `sed`/`awk` script (`w /path`, `print > \"/path\"`).
fn push_script_tokens(script: &str, role: Role, out: &mut Vec<(String, Role)>) {
    for tok in script.split_whitespace().take(64) {
        let t = tok.trim_matches(|c| "\"'{}();".contains(c));
        if !t.is_empty() {
            out.push((t.to_string(), role));
        }
    }
}

fn raw_words_of(unit: &Unit) -> Vec<(String, Role)> {
    let mut out = vec![];
    for (_, v) in &unit.seg.assignments {
        out.push((v.clone(), Role::Read));
    }
    let role = if is_known_writer(&unit.eff) {
        Role::Write
    } else if is_writer(&unit.eff) {
        Role::MaybeWrite
    } else {
        Role::Read
    };
    let script_cmd = unit
        .eff
        .first()
        .is_some_and(|f| matches!(base(f), "sed" | "awk" | "gawk" | "mawk"));
    for (idx, w) in unit.seg.argv.iter().enumerate() {
        if idx == 0 {
            // running a binary isn't writing it
            if w.contains('/') {
                out.push((w.clone(), Role::Read));
            }
            continue;
        }
        if w.starts_with("--") {
            if let Some((_, v)) = w.split_once('=') {
                out.push((v.to_string(), role));
                if script_cmd {
                    push_script_tokens(v, role, &mut out);
                }
            }
        } else if w.starts_with('-') && w.len() > 1 {
            // every suffix after a flag letter: `-uf.npmrc` is `-u -f .npmrc`
            let cs: Vec<char> = w.chars().skip(1).collect();
            for k in 1..=cs.len().min(64) {
                if k >= cs.len() {
                    break;
                }
                let rest: String = cs[k..].iter().collect();
                let rest = rest.strip_prefix('=').unwrap_or(&rest);
                if !rest.is_empty() {
                    out.push((rest.to_string(), role));
                    if script_cmd {
                        push_script_tokens(rest, role, &mut out);
                    }
                }
            }
        } else {
            out.push((w.clone(), role));
            if script_cmd {
                push_script_tokens(w, role, &mut out);
            }
            if idx > 0
                && let Some((k, v)) = w.split_once('=')
                && !k.is_empty()
            {
                let r = if k == "of" { Role::Write } else { role };
                out.push((v.to_string(), r));
            }
        }
    }
    for r in &unit.seg.redirects {
        if r.op == RedirectOp::Dup {
            continue;
        }
        let role = match r.op {
            RedirectOp::Read => Role::Read,
            _ => Role::Write,
        };
        out.push((r.target.clone(), role));
    }
    out
}

/// Applies a `cd`-like command to the tracked directory.
#[allow(clippy::too_many_arguments)]
fn apply_cd(
    unit: &Unit,
    state: &mut CwdState,
    prev: &mut Option<PathBuf>,
    seen: &mut Vec<PathBuf>,
    start: &Path,
    home: &Path,
    resolve: Resolver<'_>,
    unknown: &mut bool,
    overflow: &mut bool,
) {
    let Some(first) = unit.eff.first() else {
        return;
    };
    let name = base(first);
    if !matches!(name, "cd" | "pushd" | "popd" | "chdir") {
        return;
    }
    let make_unknown = |seen: &Vec<PathBuf>| {
        let mut c = vec![home.to_path_buf(), start.to_path_buf()];
        c.extend(seen.iter().cloned());
        CwdState::Unknown(c)
    };
    if unit.grouped || name == "popd" {
        *state = make_unknown(seen);
        *unknown = true;
        return;
    }
    let mut args: Vec<&String> = unit.eff.iter().skip(1).collect();
    while let Some(a) = args.first() {
        if matches!(a.as_str(), "-L" | "-P" | "-e" | "-@" | "--") {
            args.remove(0);
        } else {
            break;
        }
    }
    let current = match state {
        CwdState::Known(p) => Some(p.clone()),
        CwdState::Unknown(_) => None,
    };
    let target: Option<PathBuf> = match args.as_slice() {
        [] if name == "pushd" => None,
        [] => Some(home.to_path_buf()),
        [a] if a.as_str() == "-" => prev.clone(),
        [a] => {
            let expanded = expand_home(a, home, current.as_deref().unwrap_or(start));
            if has_unknown_var(&expanded) || expanded.contains(['*', '?', '[']) {
                None
            } else if Path::new(&expanded).is_absolute() {
                Some(resolve(&expanded, start, home))
            } else {
                current.as_ref().map(|c| resolve(&expanded, c, home))
            }
        }
        _ => None,
    };
    match target {
        Some(t) => {
            *prev = current.or_else(|| prev.take());
            if !seen.contains(&t) {
                seen.push(t.clone());
            }
            if seen.len() > MAX_CANDIDATES {
                *overflow = true;
            }
            *state = CwdState::Known(t);
        }
        None => {
            *state = make_unknown(seen);
            *unknown = true;
        }
    }
}

pub fn walk(units: &[Unit], start: &Path, home: &Path, resolve: Resolver<'_>) -> Walk {
    let mut w = Walk::default();
    let mut state = CwdState::Known(start.to_path_buf());
    let mut prev: Option<PathBuf> = None;
    let mut seen: Vec<PathBuf> = vec![start.to_path_buf()];
    for (i, unit) in units.iter().enumerate() {
        w.unit_cwd.push(state.clone());
        for (word, role) in words_of(unit, &mut w.overflow) {
            let paths: Vec<PathBuf> = state
                .candidates()
                .into_iter()
                .map(|c| resolve(&word, c, home))
                .collect();
            w.total += paths.len();
            if w.uses.len() >= MAX_USES || w.total > MAX_PATHS {
                w.overflow = true;
                return w;
            }
            w.uses.push(Use {
                word,
                role,
                unit: i,
                paths,
            });
        }
        apply_cd(
            unit,
            &mut state,
            &mut prev,
            &mut seen,
            start,
            home,
            resolve,
            &mut w.unknown,
            &mut w.overflow,
        );
        if w.overflow {
            return w;
        }
    }
    w
}
