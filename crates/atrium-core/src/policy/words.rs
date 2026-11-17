//! The words of a command that name paths, in a fixed order, and the working
//! directory each is resolved against. The gate resolves them against the
//! filesystem; the engine redoes it lexically. Both run [`walk`], so the
//! gate's `resolved_paths` line up with the engine's words one to one.

use std::path::{Component, Path, PathBuf};

use super::RedirectOp;
use super::shell::{Compound, Join, Unit, base};

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
    Known(Vec<PathBuf>),
    /// After a `cd` the gate can't follow: every directory the command could
    /// be in (home, the start, every directory seen so far).
    Unknown(Vec<PathBuf>),
}

impl CwdState {
    pub fn candidates(&self) -> Vec<&PathBuf> {
        match self {
            CwdState::Known(v) | CwdState::Unknown(v) => v.iter().collect(),
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
/// Directories a known cwd may be (each `cd` that may fail doubles them).
const MAX_CANDIDATES: usize = 32;
/// Directories an unknown cwd is checked against, besides home and the start.
const MAX_SEEN: usize = 64;
const MAX_BRACE_RESULTS: usize = 256;
/// Characters `brace_expand` may scan in total before it gives up (N11):
/// past this the word counts as too large, as past `MAX_BRACE_RESULTS`.
const MAX_BRACE_WORK: usize = 2_000_000;
/// The longest `{x..y}` sequence expanded.
const MAX_SEQUENCE: usize = 64;

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

/// `a{b,c}d` → `abd`, `acd`; `{1..3}`, `{a..c}`, `{01..10..2}` as the
/// shell's sequence expressions (round 7 item 1). `None` when it would make
/// more than a few hundred words.
pub(crate) fn brace_expand(w: &str) -> Option<Vec<String>> {
    if !w.contains('{') {
        return Some(vec![w.to_string()]);
    }
    if brace_results_lower_bound(w) > MAX_BRACE_RESULTS {
        return None;
    }
    let mut work = vec![w.to_string()];
    let mut out = vec![];
    let mut scanned = 0usize;
    while let Some(x) = work.pop() {
        scanned = scanned.saturating_add(x.len());
        if scanned > MAX_BRACE_WORK {
            return None;
        }
        let cs: Vec<char> = x.chars().collect();
        // matching braces and their top-level commas, in one pass
        let mut stack: Vec<(usize, Vec<usize>)> = vec![];
        // (open, close, the words between them)
        let mut best: Option<(usize, usize, Vec<String>)> = None;
        for (i, &c) in cs.iter().enumerate() {
            match c {
                '{' => stack.push((i, vec![])),
                ',' => {
                    if let Some(top) = stack.last_mut() {
                        top.1.push(i);
                    }
                }
                '}' => {
                    let Some((open, commas)) = stack.pop() else {
                        continue;
                    };
                    if (open > 0 && cs[open - 1] == '$')
                        || best.as_ref().is_some_and(|b| open >= b.0)
                    {
                        continue;
                    }
                    let alts = if commas.is_empty() {
                        let inner: String = cs[open + 1..i].iter().collect();
                        match brace_sequence(&inner) {
                            Some(v) => v,
                            None => continue,
                        }
                    } else {
                        let mut bounds = vec![open];
                        bounds.extend(commas);
                        bounds.push(i);
                        (0..bounds.len() - 1)
                            .map(|k| cs[bounds[k] + 1..bounds[k + 1]].iter().collect())
                            .collect()
                    };
                    best = Some((open, i, alts));
                }
                _ => {}
            }
        }
        match best {
            None => out.push(x),
            Some((open, close, alts)) => {
                let pre: String = cs[..open].iter().collect();
                let post: String = cs[close + 1..].iter().collect();
                for alt in alts {
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

/// A sequence expression's words: `1..3`, `a..c`, `05..1..2`. `None`: not
/// a sequence, or a long one (the braces stay as written: a path check then
/// can't read the word and asks).
fn brace_sequence(inner: &str) -> Option<Vec<String>> {
    let parts: Vec<&str> = inner.split("..").collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let step: i64 = match parts.get(2) {
        Some(s) => s.parse::<i64>().ok()?.checked_abs()?.max(1),
        None => 1,
    };
    let (a, b) = (parts[0], parts[1]);
    let letter = |s: &str| {
        let mut it = s.chars();
        match (it.next(), it.next()) {
            (Some(c), None) if c.is_ascii_alphabetic() => Some(c),
            _ => None,
        }
    };
    let mut out = vec![];
    if let (Some(x), Some(y)) = (letter(a), letter(b)) {
        let (x, y) = (x as i64, y as i64);
        let dir = if x <= y { 1 } else { -1 };
        let mut v = x;
        while (dir == 1 && v <= y) || (dir == -1 && v >= y) {
            if out.len() >= MAX_SEQUENCE {
                return None;
            }
            out.push(char::from(u8::try_from(v).ok()?).to_string());
            v += dir * step;
        }
        return Some(out);
    }
    let int = |s: &str| {
        let d = s.strip_prefix(['-', '+']).unwrap_or(s);
        if d.is_empty() || !d.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        s.parse::<i64>().ok()
    };
    let (x, y) = (int(a)?, int(b)?);
    // a leading zero pads every word to the wider end
    let padded = |s: &str| {
        let d = s.strip_prefix('-').unwrap_or(s);
        d.len() > 1 && d.starts_with('0')
    };
    let width = if padded(a) || padded(b) {
        a.len().max(b.len())
    } else {
        0
    };
    let dir = if x <= y { 1 } else { -1 };
    let mut v = x;
    while (dir == 1 && v <= y) || (dir == -1 && v >= y) {
        if out.len() >= MAX_SEQUENCE {
            return None;
        }
        out.push(if v < 0 {
            format!("-{:0>w$}", -v, w = width.saturating_sub(1))
        } else {
            format!("{v:0>width$}")
        });
        v = v.checked_add(dir * step)?;
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

/// What a directory change does to the tracked directory.
enum Cd {
    /// The shell is in one of these if the command worked (one per
    /// candidate the command ran in, or one for an absolute target).
    Moved(Vec<PathBuf>),
    /// The engine can't say where the shell is (`popd`, `pushd`, `cd -`,
    /// `cd $X`, a `cd` in a subshell or pipeline, options it can't read);
    /// these are the targets it can name.
    Lost(Vec<PathBuf>),
}

/// The effect of `unit` if it changes directory, resolved against every
/// directory the shell may be in (`cands`). `followed`: where it goes can be
/// followed (not in a pipeline or the background).
fn cd_effect(
    unit: &Unit,
    followed: bool,
    cands: &[PathBuf],
    start: &Path,
    home: &Path,
    resolve: Resolver<'_>,
) -> Option<Cd> {
    let first = unit.eff.first()?;
    let name = base(first);
    if !matches!(name, "cd" | "pushd" | "popd" | "chdir") {
        return None;
    }
    if name == "popd" {
        return Some(Cd::Lost(vec![]));
    }
    // Options: `-L`, `-P`, `-e`, `-@` alone or in a cluster (`-LP`, `-Pe`),
    // then `--`. Any other option leaves the `cd` unread: its targets stay
    // candidates but the directory becomes unknown (re-review blocker 4).
    let mut args: Vec<&String> = unit.eff.iter().skip(1).collect();
    let mut readable = true;
    while let Some(a) = args.first() {
        let a = a.as_str();
        if a == "--" {
            args.remove(0);
            break;
        }
        if a.len() > 1 && a.starts_with('-') && !a[1..].starts_with(|c: char| c.is_ascii_digit()) {
            if !a[1..].chars().all(|c| matches!(c, 'L' | 'P' | 'e' | '@')) {
                readable = false;
            }
            args.remove(0);
        } else {
            break;
        }
    }
    let resolve_all = |a: &str| -> Option<Vec<PathBuf>> {
        // `~-` ($OLDPWD), `~+` ($PWD), `~+N`/`~-N`, `-N`, `+N` (the
        // directory stack): not a directory the line names (round 5
        // blocker 6)
        let stack_form = |w: &str| {
            w.strip_prefix(['-', '+'])
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        };
        if a.starts_with("~+") || a.starts_with("~-") || stack_form(a) {
            return None;
        }
        let expanded = expand_home(a, home, start);
        if has_unknown_var(&expanded) || expanded.contains(['*', '?', '[']) {
            None
        } else if Path::new(&expanded).is_absolute() {
            Some(vec![resolve(&expanded, start, home)])
        } else {
            Some(cands.iter().map(|c| resolve(&expanded, c, home)).collect())
        }
    };
    let targets: Option<Vec<PathBuf>> = match args.as_slice() {
        [] if name == "pushd" => None,
        [] => Some(vec![home.to_path_buf()]),
        // `$OLDPWD`: not something the line names
        [a] if a.as_str() == "-" => None,
        [a] => resolve_all(a),
        // `cd OLD NEW` (zsh) and other forms: every word may be the target
        more => {
            readable = false;
            Some(
                more.iter()
                    .filter(|a| a.as_str() != "-")
                    .filter_map(|a| resolve_all(a))
                    .flatten()
                    .collect(),
            )
        }
    };
    // `pushd` keeps a stack the line can pop later: never followed.
    Some(match targets {
        Some(t) if readable && name != "pushd" && followed => Cd::Moved(t),
        Some(t) => Cd::Lost(t),
        None => Cd::Lost(vec![]),
    })
}

/// Appends up to `MAX_SEEN` directories. No two candidates are compared by
/// value: the gate (filesystem) and the engine (lexical) must build lists of
/// the same shape from the same words.
fn push_seen(seen: &mut Vec<PathBuf>, more: &[PathBuf]) {
    for p in more {
        if seen.len() >= MAX_SEEN {
            return;
        }
        seen.push(p.clone());
    }
}

/// An unknown directory: home, the start and every directory the line named.
fn lost_cands(home: &Path, start: &Path, seen: &[PathBuf]) -> Vec<PathBuf> {
    let mut v = vec![home.to_path_buf(), start.to_path_buf()];
    v.extend(seen.iter().cloned());
    v
}

/// Whether the command after unit `i` in the same text is an `exit` that
/// certainly ends the shell the `cd` ran in: not in a pipeline, a group or
/// the background (there it leaves only a child: round 5 blocker 6), and not
/// `return` (which fails outside a function and lets the line go on:
/// re-review blocker 3).
fn exits_after(units: &[Unit], i: usize) -> bool {
    let u = &units[i];
    units[i + 1..]
        .iter()
        .find(|n| n.ctx == u.ctx)
        .is_some_and(|n| {
            n.pipe == 0
                && !n.background
                && !n.grouped
                && !n.seg.piped
                && n.seg.argv.first().is_some_and(|f| f == "exit")
                && n.eff.first().is_some_and(|f| base(f) == "exit")
        })
}

/// The directory state of one text (the line, a payload, a substitution):
/// each has its own `&&` chain.
struct Level {
    /// `Unit::ctx` of the text; `None` for the line itself.
    ctx: Option<u32>,
    depth: u8,
    cands: Vec<PathBuf>,
    /// where the shell is when an `&&` chain stopped early
    pool: Vec<PathBuf>,
    unknown: bool,
    /// The text runs in the shell of the one around it (`eval`, `source`).
    same_shell: bool,
    /// An `&&` chain ended at the last command: its stopping places join
    /// the candidates before the next command of this text, not before what
    /// the last command runs itself (payloads, substitutions, `-exec`),
    /// which runs only if the chain got that far (round 5 major).
    pending: bool,
    /// The join of the last command of this text seen so far.
    last_join: Join,
    /// `close_join` of the last command of this text seen so far.
    last_close: Option<Join>,
    /// The text ran where its owner may be in a child (round 6 item 7):
    /// past it the shell is where it was or where the text ended.
    union: bool,
    /// The last command of this text seen so far came after `||` and may
    /// not have run (round 7 item 2): a text it runs in this shell (`||
    /// eval …`, `|| source …`) then adds to where the shell may be.
    last_skippable: bool,
    /// The compound commands open at this point, outermost first.
    frames: Vec<Frame>,
}

/// One open compound command of a text (round 6 item 4): its commands keep
/// their own `&&` chain; the chain around it is put aside until it ends.
struct Frame {
    id: u32,
    kind: Compound,
    /// Where the shell was when the compound started.
    entry: Vec<PathBuf>,
    /// The chain around it, put aside.
    saved_pool: Vec<PathBuf>,
    saved_last_join: Join,
}

/// Ends the innermost compound: the shell carries on where its commands left
/// it (and, unless it is a `{ …; }` that always runs once, where it was
/// before: the body may not have run); where its chain may have stopped
/// joins the chain around it, which decides when that counts.
fn close_frame(lv: &mut Level) {
    let Some(f) = lv.frames.pop() else { return };
    let inner = std::mem::take(&mut lv.pool);
    lv.pool = f.saved_pool;
    lv.pool.extend(inner);
    if f.kind != Compound::Brace && !lv.unknown {
        lv.cands.extend(f.entry);
    }
    lv.last_join = f.saved_last_join;
    lv.pending = false;
}

/// Ends a stopped `&&` chain: where it may have stopped becomes a candidate.
fn flush_pool(lv: &mut Level, seen: &mut Vec<PathBuf>, home: &Path, start: &Path, w: &mut Walk) {
    lv.pending = false;
    if lv.pool.is_empty() {
        return;
    }
    if lv.unknown {
        push_seen(seen, &lv.pool);
        lv.pool.clear();
        lv.cands = lost_cands(home, start, seen);
    } else {
        lv.cands.append(&mut lv.pool);
    }
    too_many(lv, seen, home, start, w);
}

/// Too many candidates to follow: the directory becomes unknown (asks).
fn too_many(lv: &mut Level, seen: &mut Vec<PathBuf>, home: &Path, start: &Path, w: &mut Walk) {
    if !lv.unknown && lv.cands.len() > MAX_CANDIDATES {
        push_seen(seen, &lv.cands);
        push_seen(seen, &lv.pool);
        lv.pool.clear();
        lv.unknown = true;
        w.unknown = true;
        lv.cands = lost_cands(home, start, seen);
    }
}

/// Resolves every word against every directory the shell may be in.
///
/// A `cd` that is followed by `&&` moves the rest of that chain to its
/// target; after `;`, `||`, a newline or the end of an `&&` chain the shell
/// may be in the target or where it was before (the `cd` may have failed), so
/// both stay candidates. A command written as `cd DIR || exit` leaves only the
/// target. A change the engine can't follow makes the directory unknown:
/// every directory the line named, home and the start.
///
/// Each text (the line, a `sh -c` or `eval` payload, a substitution, a
/// here-doc a shell reads) keeps its own chain, starting where the text
/// around it is; when it ends, the text around it carries on where it was,
/// unless the text ran in the same shell (`eval`, `source`), which takes
/// over where it ended (re-review majors 1 and 2).
pub fn walk(units: &[Unit], start: &Path, home: &Path, resolve: Resolver<'_>) -> Walk {
    let mut w = Walk::default();
    let mut seen: Vec<PathBuf> = vec![];
    let mut stack: Vec<Level> = vec![Level {
        ctx: None,
        depth: 0,
        cands: vec![start.to_path_buf()],
        pool: vec![],
        unknown: false,
        same_shell: false,
        pending: false,
        last_join: Join::Seq,
        last_close: None,
        union: false,
        last_skippable: false,
        frames: vec![],
    }];
    for (i, unit) in units.iter().enumerate() {
        // leave the texts this unit isn't in
        while stack.len() > 1 {
            let top = &stack[stack.len() - 1];
            let other =
                top.depth > unit.level || (top.depth == unit.level && top.ctx != Some(unit.ctx));
            if !other {
                break;
            }
            let Some(mut done) = stack.pop() else { break };
            while !done.frames.is_empty() {
                close_frame(&mut done);
            }
            if done.same_shell
                && let Some(parent) = stack.last_mut()
            {
                // the text ran in this shell: past it the shell is where the
                // text ended; where its chains stopped joins this text's
                // pool (it counts once this text's own chain is over). When
                // it may have run in a child, the shell may also still be
                // where it was (round 6 item 7).
                if done.unknown && !parent.unknown {
                    push_seen(&mut seen, &parent.cands);
                    push_seen(&mut seen, &parent.pool);
                    parent.pool.clear();
                    parent.unknown = true;
                    w.unknown = true;
                    parent.cands = lost_cands(home, start, &seen);
                } else if done.union {
                    parent.cands.extend(done.cands);
                } else {
                    parent.cands = done.cands;
                }
                if !done.pool.is_empty() && !parent.unknown {
                    parent.pool.extend(done.pool);
                    parent.pending |= parent.last_join != Join::And;
                }
            }
        }
        // enter the text it is in
        let top_depth = stack[stack.len() - 1].depth;
        if unit.level > top_depth || (unit.level > 0 && stack.len() == 1) {
            let top = &stack[stack.len() - 1];
            let entered = Level {
                ctx: Some(unit.ctx),
                depth: unit.level,
                cands: top.cands.clone(),
                pool: vec![],
                unknown: top.unknown,
                same_shell: unit.same_shell,
                pending: false,
                last_join: Join::Seq,
                last_close: None,
                union: unit.shell_union || (unit.same_shell && top.last_skippable),
                last_skippable: false,
                frames: vec![],
            };
            stack.push(entered);
        }
        let Some(lv) = stack.last_mut() else {
            return w;
        };
        // leave the compounds this unit isn't in, enter the ones it is in
        let common = lv
            .frames
            .iter()
            .zip(&unit.compound)
            .take_while(|(f, c)| f.id == c.1)
            .count();
        if lv.frames.len() > common {
            while lv.frames.len() > common {
                close_frame(lv);
            }
            let closer = unit
                .seg
                .argv
                .first()
                .is_some_and(|f| matches!(f.as_str(), "}" | "fi" | "done" | "esac"));
            if !closer {
                // `( … ) && x`: the group's place in the chain is the join
                // written after it (none read: a `;`)
                let j = lv.last_close.unwrap_or(Join::Seq);
                if j == Join::And {
                    lv.pool.extend(lv.cands.iter().cloned());
                }
                lv.last_join = j;
                lv.pending = j != Join::And && !lv.pool.is_empty();
            }
        }
        for &(kind, id) in &unit.compound[common..] {
            if lv.pending {
                flush_pool(lv, &mut seen, home, start, &mut w);
            }
            // after `||` the compound may not run at all
            let kind = if kind == Compound::Brace && lv.last_join == Join::Or {
                Compound::Cond
            } else {
                kind
            };
            lv.frames.push(Frame {
                id,
                kind,
                entry: lv.cands.clone(),
                saved_pool: std::mem::take(&mut lv.pool),
                saved_last_join: lv.last_join,
            });
            lv.last_join = Join::Seq;
        }
        too_many(lv, &mut seen, home, start, &mut w);
        if lv.pending {
            flush_pool(lv, &mut seen, home, start, &mut w);
        }
        w.unit_cwd.push(if lv.unknown {
            CwdState::Unknown(lv.cands.clone())
        } else {
            CwdState::Known(lv.cands.clone())
        });
        for (word, role) in words_of(unit, &mut w.overflow) {
            let paths: Vec<PathBuf> = lv.cands.iter().map(|c| resolve(&word, c, home)).collect();
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
        if w.overflow {
            return w;
        }
        // a command piped into the next one doesn't end a chain: the
        // pipeline's status is its last command's
        let mid_pipe = unit.pipe != 0
            && units[i + 1..]
                .iter()
                .find(|n| n.ctx == unit.ctx && n.level == unit.level)
                .is_some_and(|n| n.pipe == unit.pipe);
        let join = if mid_pipe { Join::And } else { unit.join };
        // after `||` this command may not run (`a || cd x && y`: `y` may run
        // where the shell was): where it was stays a candidate
        let skipped_from = lv.last_join == Join::Or && !lv.unknown;
        let lv_skippable = lv.last_join == Join::Or;
        let entry = if skipped_from {
            lv.cands.clone()
        } else {
            vec![]
        };
        // a `cd` in a loop or a function body may run any number of times
        let repeats = lv
            .frames
            .iter()
            .any(|f| matches!(f.kind, Compound::Loop | Compound::Func));
        // a `cd` in a `( … )` group is followed inside it, and where it went
        // stays a candidate after it (the group's frame adds where the shell
        // was: round 7 major); in a pipeline or the background it is lost
        let in_paren = unit.compound.iter().any(|c| c.0 == Compound::Paren)
            && unit.pipe == 0
            && !unit.seg.piped
            && !unit.background;
        let followed = !unit.grouped || in_paren;
        let effect = match cd_effect(unit, followed, &lv.cands, start, home, resolve) {
            Some(Cd::Moved(t)) if repeats => Some(Cd::Lost(t)),
            e => e,
        };
        let moved = effect.is_some();
        match effect {
            None => {
                if join == Join::And {
                    lv.pool.extend(lv.cands.iter().cloned());
                }
            }
            Some(Cd::Moved(targets)) => {
                push_seen(&mut seen, &targets);
                if lv.unknown {
                    lv.cands = lost_cands(home, start, &seen);
                } else if join == Join::And {
                    lv.pool.append(&mut lv.cands);
                    lv.cands = targets;
                } else if join == Join::Or && exits_after(units, i) {
                    lv.cands = targets;
                } else {
                    let mut next = targets;
                    next.append(&mut lv.cands);
                    lv.cands = next;
                }
            }
            Some(Cd::Lost(targets)) => {
                push_seen(&mut seen, &targets);
                if !lv.unknown {
                    push_seen(&mut seen, &lv.cands);
                    push_seen(&mut seen, &lv.pool);
                }
                lv.pool.clear();
                lv.unknown = true;
                w.unknown = true;
                lv.cands = lost_cands(home, start, &seen);
            }
        }
        if skipped_from && moved && !lv.unknown {
            lv.cands.extend(entry);
        }
        lv.last_join = join;
        lv.last_close = unit.close_join;
        lv.last_skippable = lv_skippable;
        if join != Join::And && !lv.pool.is_empty() {
            lv.pending = true;
        }
        too_many(lv, &mut seen, home, start, &mut w);
    }
    w
}

/// A lower bound on the words `brace_expand` makes from `w` (N11): the
/// product of the comma counts of the top-level brace pairs with no pair
/// inside them and no `$` before them. Each such group picks one of its
/// words independently in every result. One pass, so a word with thousands
/// of groups is refused without being expanded.
fn brace_results_lower_bound(w: &str) -> usize {
    let cs: Vec<char> = w.chars().collect();
    // (open, close, top-level commas, has a pair inside)
    let mut stack: Vec<(usize, usize, bool)> = vec![];
    let mut pairs: Vec<(usize, usize, usize, bool)> = vec![];
    for (i, &c) in cs.iter().enumerate() {
        match c {
            '{' => stack.push((i, 0, false)),
            ',' => {
                if let Some(top) = stack.last_mut() {
                    top.1 += 1;
                }
            }
            '}' => {
                if let Some((open, commas, inner)) = stack.pop() {
                    pairs.push((open, i, commas, inner));
                    if let Some(parent) = stack.last_mut() {
                        parent.2 = true;
                    }
                }
            }
            _ => {}
        }
    }
    // pairs close in order; a pair is top-level when no later-closing pair
    // opened before it
    pairs.sort_unstable_by_key(|p| p.0);
    let mut bound = 1usize;
    let mut reach: Option<usize> = None;
    for &(open, close, commas, inner) in &pairs {
        let nested = reach.is_some_and(|r| close < r);
        if !nested {
            reach = Some(close);
            if !inner && commas > 0 && !(open > 0 && cs[open - 1] == '$') {
                bound = bound.saturating_mul(commas + 1);
                if bound > MAX_BRACE_RESULTS {
                    return bound;
                }
            }
        }
    }
    bound
}
