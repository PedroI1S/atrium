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
    Write,
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
}

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

/// Whether this command writes to (some of) the paths in its arguments.
pub fn is_writer(eff: &[String]) -> bool {
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
    if word == "~" {
        return h.into_owned();
    }
    if let Some(rest) = word.strip_prefix("~/") {
        return format!("{h}/{rest}");
    }
    for (var, val) in [("$HOME", &*h), ("${HOME}", &*h)] {
        if let Some(rest) = word.strip_prefix(var)
            && (rest.is_empty() || rest.starts_with('/'))
        {
            return format!("{val}{rest}");
        }
    }
    let c = cwd.to_string_lossy();
    for var in ["$PWD", "${PWD}"] {
        if let Some(rest) = word.strip_prefix(var)
            && (rest.is_empty() || rest.starts_with('/'))
        {
            return format!("{c}{rest}");
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

/// `a{b,c}d` → `abd`, `acd` (comma lists only; capped).
fn brace_expand(w: &str) -> Vec<String> {
    fn go(w: &str, out: &mut Vec<String>) {
        let cs: Vec<char> = w.chars().collect();
        let mut i = 0;
        while i < cs.len() {
            if cs[i] == '{' && (i == 0 || cs[i - 1] != '$') {
                let (mut d, mut j, mut commas) = (1usize, i + 1, vec![]);
                while j < cs.len() && d > 0 {
                    match cs[j] {
                        '{' => d += 1,
                        '}' => d -= 1,
                        ',' if d == 1 => commas.push(j),
                        _ => {}
                    }
                    j += 1;
                }
                if d == 0 && !commas.is_empty() {
                    let pre: String = cs[..i].iter().collect();
                    let post: String = cs[j..].iter().collect();
                    let mut bounds = vec![i];
                    bounds.extend(commas);
                    bounds.push(j - 1);
                    for k in 0..bounds.len() - 1 {
                        let alt: String = cs[bounds[k] + 1..bounds[k + 1]].iter().collect();
                        if out.len() > 256 {
                            return;
                        }
                        go(&format!("{pre}{alt}{post}"), out);
                    }
                    return;
                }
            }
            i += 1;
        }
        out.push(w.to_string());
    }
    let mut out = vec![];
    go(w, &mut out);
    if out.len() > 256 {
        return vec![w.to_string()];
    }
    out
}

/// Words that name paths, in the order the gate and the engine both use.
fn words_of(unit: &Unit) -> Vec<(String, Role)> {
    let mut out = vec![];
    for (w, role) in raw_words_of(unit) {
        for e in brace_expand(&w) {
            out.push((e, role));
        }
    }
    out
}

fn raw_words_of(unit: &Unit) -> Vec<(String, Role)> {
    let mut out = vec![];
    for (_, v) in &unit.seg.assignments {
        out.push((v.clone(), Role::Read));
    }
    let writer = is_writer(&unit.eff);
    let role = if writer { Role::Write } else { Role::Read };
    for (idx, w) in unit.seg.argv.iter().enumerate() {
        if idx == 0 && !w.contains('/') {
            continue;
        }
        if w.starts_with("--") {
            if let Some((_, v)) = w.split_once('=') {
                out.push((v.to_string(), role));
            }
        } else if w.starts_with('-') && w.len() > 1 {
            let rest = &w[2..];
            let rest = rest.strip_prefix('=').unwrap_or(rest);
            if !rest.is_empty() {
                out.push((rest.to_string(), role));
            }
        } else {
            out.push((w.clone(), role));
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
            seen.push(t.clone());
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
        for (word, role) in words_of(unit) {
            let paths: Vec<PathBuf> = state
                .candidates()
                .into_iter()
                .map(|c| resolve(&word, c, home))
                .collect();
            w.total += paths.len();
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
        );
    }
    w
}
