//! The words of a command that name paths, in a fixed order, and the working
//! directory each is resolved against. The gate resolves them against the
//! filesystem; the engine redoes it lexically. Both run [`walk`], so the
//! gate's `resolved_paths` line up with the engine's words one to one.

use std::path::{Component, Path, PathBuf};

use super::RedirectOp;
use super::shell::{Compound, Join, Unit, base};

pub(super) struct AnsiCDecode {
    pub decoded: String,
    pub next_index: usize,
    pub closed: bool,
    pub control_char: bool,
    pub bad_escape: bool,
}

fn is_refused_control(c: char) -> bool {
    (c.is_control() && !matches!(c, '\n' | '\t' | '\r')) || matches!(c, '\u{2028}' | '\u{2029}')
}

/// Decodes a bash ANSI-C quoted span `$'…'` starting at `quote_idx` (the `'`).
/// Supports all bash `$'…'` escapes (`\a \b \e \E \f \n \r \t \v \\ \' \" \?`,
/// `\nnn` octal, `\xHH`, `\uHHHH`, `\UHHHHHHHH`, `\cX`). Sets flags so the
/// lexer fails closed on unterminated quotes, NUL, refused control characters,
/// or unknown/malformed escapes.
pub(super) fn decode_ansi_c(chars: &[char], quote_idx: usize) -> AnsiCDecode {
    let mut i = quote_idx + 1;
    let mut out = String::new();
    let mut pending_bytes: Vec<u8> = Vec::new();
    let mut closed = false;
    let mut saw_nul = false;
    let mut control_char = false;
    let mut bad_escape = false;

    let push_char = |c: char, out: &mut String, saw_nul: &mut bool, control_char: &mut bool| {
        if c == '\0' {
            *control_char = true;
            *saw_nul = true;
            return;
        }
        if is_refused_control(c) {
            *control_char = true;
        }
        if !*saw_nul {
            out.push(c);
        }
    };

    let flush_bytes = |pending: &mut Vec<u8>,
                       out: &mut String,
                       saw_nul: &mut bool,
                       control_char: &mut bool,
                       bad_escape: &mut bool| {
        while !pending.is_empty() {
            match std::str::from_utf8(pending) {
                Ok(s) => {
                    for ch in s.chars() {
                        push_char(ch, out, saw_nul, control_char);
                    }
                    pending.clear();
                }
                Err(err) => {
                    let valid_up_to = err.valid_up_to();
                    if valid_up_to > 0
                        && let Ok(s) = std::str::from_utf8(&pending[..valid_up_to])
                    {
                        for ch in s.chars() {
                            push_char(ch, out, saw_nul, control_char);
                        }
                    }
                    *bad_escape = true;
                    // Preserve word identity: never discard invalid bytes so
                    // `$'\xff.aws/credentials'` cannot collapse to `.aws/credentials`.
                    push_char('\u{FFFD}', out, saw_nul, control_char);
                    let skip = err.error_len().unwrap_or(pending.len() - valid_up_to);
                    pending.drain(..valid_up_to + skip);
                }
            }
        }
    };

    while i < chars.len() {
        match chars[i] {
            '\'' => {
                closed = true;
                i += 1;
                break;
            }
            '\\' => {
                i += 1;
                let Some(&esc) = chars.get(i) else {
                    flush_bytes(
                        &mut pending_bytes,
                        &mut out,
                        &mut saw_nul,
                        &mut control_char,
                        &mut bad_escape,
                    );
                    bad_escape = true;
                    push_char('\\', &mut out, &mut saw_nul, &mut control_char);
                    break;
                };
                match esc {
                    'a' | 'b' | 'e' | 'E' | 'f' | 'n' | 'r' | 't' | 'v' | '\\' | '\'' | '"'
                    | '?' => {
                        let ch = match esc {
                            'a' => '\x07',
                            'b' => '\x08',
                            'e' | 'E' => '\x1b',
                            'f' => '\x0c',
                            'n' => '\n',
                            'r' => '\r',
                            't' => '\t',
                            'v' => '\x0b',
                            '\\' => '\\',
                            '\'' => '\'',
                            '"' => '"',
                            _ => '?',
                        };
                        flush_bytes(
                            &mut pending_bytes,
                            &mut out,
                            &mut saw_nul,
                            &mut control_char,
                            &mut bad_escape,
                        );
                        push_char(ch, &mut out, &mut saw_nul, &mut control_char);
                        i += 1;
                    }
                    '0'..='7' => {
                        let mut val: u32 = 0;
                        let mut count = 0;
                        while count < 3
                            && let Some(&d) = chars.get(i)
                            && ('0'..='7').contains(&d)
                        {
                            val = val * 8 + (d as u32 - '0' as u32);
                            i += 1;
                            count += 1;
                        }
                        if val > 0xff {
                            bad_escape = true;
                        }
                        let byte = (val & 0xff) as u8;
                        if byte == 0 {
                            flush_bytes(
                                &mut pending_bytes,
                                &mut out,
                                &mut saw_nul,
                                &mut control_char,
                                &mut bad_escape,
                            );
                            control_char = true;
                            saw_nul = true;
                        } else if !saw_nul {
                            pending_bytes.push(byte);
                        }
                    }
                    'x' => {
                        i += 1;
                        let mut val: u32 = 0;
                        let mut count = 0;
                        while count < 2
                            && let Some(&d) = chars.get(i)
                            && let Some(digit) = d.to_digit(16)
                        {
                            val = val * 16 + digit;
                            i += 1;
                            count += 1;
                        }
                        if count == 0 {
                            bad_escape = true;
                            flush_bytes(
                                &mut pending_bytes,
                                &mut out,
                                &mut saw_nul,
                                &mut control_char,
                                &mut bad_escape,
                            );
                            if !saw_nul {
                                out.push_str("\\x");
                            }
                        } else {
                            let byte = val as u8;
                            if byte == 0 {
                                flush_bytes(
                                    &mut pending_bytes,
                                    &mut out,
                                    &mut saw_nul,
                                    &mut control_char,
                                    &mut bad_escape,
                                );
                                control_char = true;
                                saw_nul = true;
                            } else if !saw_nul {
                                pending_bytes.push(byte);
                            }
                        }
                    }
                    'u' | 'U' => {
                        let max_digits = if esc == 'u' { 4 } else { 8 };
                        i += 1;
                        flush_bytes(
                            &mut pending_bytes,
                            &mut out,
                            &mut saw_nul,
                            &mut control_char,
                            &mut bad_escape,
                        );
                        let mut val: u32 = 0;
                        let mut count = 0;
                        while count < max_digits
                            && let Some(&d) = chars.get(i)
                            && let Some(digit) = d.to_digit(16)
                        {
                            val = val * 16 + digit;
                            i += 1;
                            count += 1;
                        }
                        if count == 0 {
                            bad_escape = true;
                            if !saw_nul {
                                out.push('\\');
                                out.push(esc);
                            }
                        } else if let Some(ch) = char::from_u32(val) {
                            push_char(ch, &mut out, &mut saw_nul, &mut control_char);
                        } else {
                            bad_escape = true;
                            push_char('\u{FFFD}', &mut out, &mut saw_nul, &mut control_char);
                        }
                    }
                    'c' => {
                        i += 1;
                        flush_bytes(
                            &mut pending_bytes,
                            &mut out,
                            &mut saw_nul,
                            &mut control_char,
                            &mut bad_escape,
                        );
                        match chars.get(i).copied() {
                            None => {
                                bad_escape = true;
                                push_char('\\', &mut out, &mut saw_nul, &mut control_char);
                                push_char('c', &mut out, &mut saw_nul, &mut control_char);
                            }
                            Some('\'') => {
                                bad_escape = true;
                                push_char('\u{FFFD}', &mut out, &mut saw_nul, &mut control_char);
                                closed = true;
                                i += 1;
                                break;
                            }
                            Some('\\') => {
                                i += 1;
                                if chars.get(i) == Some(&'\\') {
                                    i += 1;
                                    push_char('\x1c', &mut out, &mut saw_nul, &mut control_char);
                                } else if chars.get(i) == Some(&'\'') {
                                    i += 1;
                                    push_char('\x1c', &mut out, &mut saw_nul, &mut control_char);
                                    push_char('\'', &mut out, &mut saw_nul, &mut control_char);
                                } else {
                                    push_char('\x1c', &mut out, &mut saw_nul, &mut control_char);
                                }
                            }
                            Some('?') => {
                                i += 1;
                                push_char('\x7f', &mut out, &mut saw_nul, &mut control_char);
                            }
                            Some(x) if x.is_ascii() => {
                                i += 1;
                                let ctrl = ((x as u8).to_ascii_uppercase() & 0x1f) as char;
                                push_char(ctrl, &mut out, &mut saw_nul, &mut control_char);
                            }
                            Some(_) => {
                                i += 1;
                                bad_escape = true;
                                push_char('\u{FFFD}', &mut out, &mut saw_nul, &mut control_char);
                            }
                        }
                    }
                    other => {
                        flush_bytes(
                            &mut pending_bytes,
                            &mut out,
                            &mut saw_nul,
                            &mut control_char,
                            &mut bad_escape,
                        );
                        bad_escape = true;
                        if !saw_nul {
                            out.push('\\');
                            out.push(other);
                        }
                        i += 1;
                    }
                }
            }
            ch => {
                flush_bytes(
                    &mut pending_bytes,
                    &mut out,
                    &mut saw_nul,
                    &mut control_char,
                    &mut bad_escape,
                );
                push_char(ch, &mut out, &mut saw_nul, &mut control_char);
                i += 1;
            }
        }
    }

    flush_bytes(
        &mut pending_bytes,
        &mut out,
        &mut saw_nul,
        &mut control_char,
        &mut bad_escape,
    );
    AnsiCDecode {
        decoded: out,
        next_index: i,
        closed,
        control_char,
        bad_escape,
    }
}

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
    /// A globbed `cd` target exceeded its matching/candidate budget or could
    /// not be bounded (fail-closed: Ask everywhere, never Allow).
    pub cd_glob_unbounded: bool,
}

pub const MAX_USES: usize = 4096;
pub const MAX_PATHS: usize = 16384;
/// Directories a known cwd may be (each `cd` that may fail doubles them).
const MAX_CANDIDATES: usize = 32;
/// Directories an unknown cwd is checked against, besides home and the start.
const MAX_SEEN: usize = 64;
const MAX_BRACE_RESULTS: usize = 256;
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
    let mut work = vec![w.to_string()];
    let mut out = vec![];
    while let Some(x) = work.pop() {
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
    /// these are the targets it can name, plus whether they came from a glob.
    Lost(Vec<PathBuf>, bool),
}

const MAX_CD_GLOB_VARIANTS: usize = 16;
const MAX_CD_GLOB_WORK: usize = 16_384;
const MAX_GLOBSTAR_DEPTH: usize = 256;
const MAX_NEEDED_DEPTHS: usize = 16;

/// One budget per `walk` call for globbed `cd` candidate matching. Once spent
/// (or when a representation limit is exceeded), `unbounded` flips true so the
/// engine fails closed to `Ask` everywhere instead of silently truncating.
struct CdGlobBudget {
    steps: usize,
    unbounded: bool,
}

impl CdGlobBudget {
    fn new() -> Self {
        Self {
            steps: 0,
            unbounded: false,
        }
    }

    fn charge(&mut self, n: usize) -> bool {
        self.steps = self.steps.saturating_add(n);
        if self.steps > MAX_CD_GLOB_WORK {
            self.unbounded = true;
            return false;
        }
        true
    }
}

fn normalize_cd_glob(pat: &str, budget: &mut CdGlobBudget) -> Vec<Vec<String>> {
    let abs = pat.starts_with('/');
    let folded = pat.replace('ſ', "s");
    let mut states: Vec<Vec<String>> = vec![vec![]];
    for comp in folded.split('/') {
        match comp {
            "" | "." => {}
            "**" => {
                for st in &mut states {
                    if st.last().map(String::as_str) != Some("**") {
                        st.push("**".to_string());
                    }
                }
            }
            ".." => {
                let mut next = Vec::new();
                for mut st in states {
                    if !budget.charge(1) {
                        next.push(st);
                        break;
                    }
                    match st.last().map(String::as_str) {
                        None => {
                            if !abs {
                                st.push("..".to_string());
                            }
                            next.push(st);
                        }
                        Some("..") => {
                            st.push("..".to_string());
                            next.push(st);
                        }
                        Some("**") => {
                            next.push(st.clone());
                            st.pop();
                            next.push(st.clone());
                            if !st.is_empty() {
                                st.pop();
                                next.push(st);
                            }
                        }
                        Some(last) if !last.contains(['*', '?', '[']) => {
                            st.pop();
                            next.push(st);
                        }
                        Some(_) => {
                            let mut popped = st.clone();
                            popped.pop();
                            next.push(popped);
                            st.push("..".to_string());
                            next.push(st);
                        }
                    }
                    if next.len() > MAX_CD_GLOB_VARIANTS {
                        budget.unbounded = true;
                        break;
                    }
                }
                next.sort();
                next.dedup();
                if next.len() > MAX_CD_GLOB_VARIANTS {
                    budget.unbounded = true;
                    next.truncate(MAX_CD_GLOB_VARIANTS);
                }
                states = next;
            }
            other => {
                for st in &mut states {
                    st.push(other.to_string());
                }
            }
        }
    }
    states
}

enum BracketItem {
    Char(char),
    Range(char, char),
    Posix(String),
    Wildcard,
}

/// Parses a bash bracket expression `[...]` starting at `start` (`p[start] == '['`).
/// Supports `[!...]`/`[^...]`, leading `]`, ranges `a-z`, POSIX named classes
/// `[:lower:]` etc., and collating/equivalence classes `[.x.]`/`[=x=]`.
fn parse_bracket(p: &[char], start: usize) -> Option<(usize, bool, Vec<BracketItem>)> {
    let mut i = start + 1;
    if i >= p.len() {
        return None;
    }
    let negated = matches!(p.get(i), Some('!' | '^'));
    if negated {
        i += 1;
    }
    let mut items = Vec::new();
    if p.get(i) == Some(&']') {
        items.push(BracketItem::Char(']'));
        i += 1;
    }
    while i < p.len() {
        if p[i] == ']' {
            return Some((i + 1, negated, items));
        }
        if p[i] == '[' && matches!(p.get(i + 1), Some(':' | '.' | '=')) {
            let delim = p[i + 1];
            if let Some(rel_end) = p[i + 2..]
                .windows(2)
                .position(|w| w[0] == delim && w[1] == ']')
            {
                let end = i + 2 + rel_end;
                if delim == ':' {
                    let name: String = p[i + 2..end].iter().collect();
                    items.push(BracketItem::Posix(name));
                } else {
                    items.push(BracketItem::Wildcard);
                }
                i = end + 2;
                continue;
            }
        }
        if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
            items.push(BracketItem::Range(p[i], p[i + 2]));
            i += 3;
        } else {
            items.push(BracketItem::Char(p[i]));
            i += 1;
        }
    }
    None
}

fn posix_class_matches(name: &str, ch: char) -> Option<bool> {
    Some(match name {
        "alnum" => ch.is_ascii_alphanumeric(),
        // `cd` matching is case-insensitive, so `[:lower:]` and `[:upper:]` match either case.
        "alpha" | "lower" | "upper" => ch.is_ascii_alphabetic(),
        "ascii" => ch.is_ascii(),
        "blank" => matches!(ch, ' ' | '\t'),
        "cntrl" => ch.is_ascii_control(),
        "digit" => ch.is_ascii_digit(),
        "graph" => ch.is_ascii_graphic(),
        "print" => ch.is_ascii_graphic() || ch == ' ',
        "punct" => ch.is_ascii_punctuation(),
        "space" => ch.is_ascii_whitespace(),
        "word" => ch.is_ascii_alphanumeric() || ch == '_',
        "xdigit" => ch.is_ascii_hexdigit(),
        _ => return None,
    })
}

fn bracket_matches_char(negated: bool, items: &[BracketItem], ch: char) -> bool {
    let ch_l = ch.to_ascii_lowercase();
    let ch_u = ch.to_ascii_uppercase();
    let mut matched = false;
    for item in items {
        let hit = match item {
            BracketItem::Char(c) => c.to_ascii_lowercase() == ch_l,
            BracketItem::Range(lo, hi) => {
                (*lo..=*hi).contains(&ch)
                    || (lo.to_ascii_lowercase()..=hi.to_ascii_lowercase()).contains(&ch_l)
                    || (lo.to_ascii_uppercase()..=hi.to_ascii_uppercase()).contains(&ch_u)
            }
            BracketItem::Posix(name) => match posix_class_matches(name, ch) {
                Some(b) => b,
                None => return true,
            },
            BracketItem::Wildcard => return true,
        };
        if hit {
            matched = true;
            break;
        }
    }
    if negated { !matched } else { matched }
}

fn leading_class_matches_dot(p: &str) -> bool {
    let cs: Vec<char> = p.chars().collect();
    if cs.first() != Some(&'[') {
        return false;
    }
    let Some((_, negated, items)) = parse_bracket(&cs, 0) else {
        return false;
    };
    if negated {
        return false;
    }
    bracket_matches_char(false, &items, '.')
}

fn cd_comp_matches(pat: &str, cand: &str, budget: &mut CdGlobBudget) -> bool {
    if cand.starts_with('.') && !(pat.starts_with('.') || leading_class_matches_dot(pat)) {
        return false;
    }
    if !pat.contains(['*', '?', '[']) {
        if !budget.charge(1) {
            return false;
        }
        return if pat.is_ascii() && cand.is_ascii() {
            pat.eq_ignore_ascii_case(cand)
        } else {
            pat.to_lowercase() == cand.to_lowercase()
        };
    }
    let p: Vec<char> = pat.chars().collect();
    let c: Vec<char> = cand.chars().collect();
    let cost = p
        .len()
        .saturating_add(1)
        .saturating_mul(c.len().saturating_add(1));
    if !budget.charge(cost) {
        return false;
    }
    let mut dp = vec![false; c.len() + 1];
    dp[0] = true;
    let mut pi = 0;
    while pi < p.len() {
        match p[pi] {
            '*' => {
                for ci in 1..=c.len() {
                    dp[ci] |= dp[ci - 1];
                }
                pi += 1;
            }
            '?' => {
                for ci in (1..=c.len()).rev() {
                    dp[ci] = dp[ci - 1];
                }
                dp[0] = false;
                pi += 1;
            }
            '[' => {
                if let Some((next_pi, negated, items)) = parse_bracket(&p, pi) {
                    for ci in (1..=c.len()).rev() {
                        dp[ci] = dp[ci - 1] && bracket_matches_char(negated, &items, c[ci - 1]);
                    }
                    dp[0] = false;
                    pi = next_pi;
                } else {
                    for ci in (1..=c.len()).rev() {
                        dp[ci] = dp[ci - 1] && c[ci - 1] == '[';
                    }
                    dp[0] = false;
                    pi += 1;
                }
            }
            ch => {
                let ch_l = ch.to_ascii_lowercase();
                for ci in (1..=c.len()).rev() {
                    dp[ci] = dp[ci - 1] && c[ci - 1].to_ascii_lowercase() == ch_l;
                }
                dp[0] = false;
                pi += 1;
            }
        }
    }
    dp[c.len()]
}

fn cd_comps_match(pat: &[String], cand: &[&str], budget: &mut CdGlobBudget) -> bool {
    if pat.len() != cand.len() {
        return false;
    }
    for (p, c) in pat.iter().zip(cand) {
        if !cd_comp_matches(p, c, budget) {
            return false;
        }
    }
    true
}

fn cd_comps_match_globstar(pat: &[String], cand: &[&str], budget: &mut CdGlobBudget) -> bool {
    if !budget.charge(
        pat.len()
            .saturating_mul(cand.len().saturating_add(1))
            .max(1),
    ) {
        return false;
    }
    let mut dp = vec![false; cand.len() + 1];
    dp[0] = true;
    for p in pat {
        if p == "**" {
            for j in 1..=cand.len() {
                if !cand[j - 1].starts_with('.') {
                    dp[j] |= dp[j - 1];
                }
            }
        } else {
            for j in (1..=cand.len()).rev() {
                dp[j] = dp[j - 1] && cd_comp_matches(p, cand[j - 1], budget);
            }
            dp[0] = false;
        }
        if budget.unbounded {
            return false;
        }
    }
    dp[cand.len()]
}

fn upward_depth(word: &str) -> usize {
    let mut depth: isize = 0;
    let mut min_depth: isize = 0;
    for comp in word.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < min_depth {
                    min_depth = depth;
                }
            }
            _ => depth += 1,
        }
    }
    (-min_depth) as usize
}

fn downward_depth(word: &str) -> usize {
    let mut depth: isize = 0;
    for comp in word.split('/') {
        match comp {
            "" | "." => {}
            ".." => depth -= 1,
            _ => depth += 1,
        }
    }
    depth.max(0) as usize
}

fn needed_upward_depths(units: &[Unit], after_idx: usize, budget: &mut CdGlobBudget) -> Vec<usize> {
    let mut depths: Vec<usize> = vec![0];
    let mut record = |d: usize, budget: &mut CdGlobBudget| {
        if d > MAX_GLOBSTAR_DEPTH {
            budget.unbounded = true;
            return;
        }
        if !depths.contains(&d) {
            if depths.len() >= MAX_NEEDED_DEPTHS {
                budget.unbounded = true;
                return;
            }
            depths.push(d);
        }
    };
    let mut cum_cd_up: usize = 0;
    let mut cum_cd_down: usize = 0;
    for u in &units[after_idx + 1..] {
        if !budget.charge(1) {
            break;
        }
        let is_cd = u
            .eff
            .first()
            .is_some_and(|f| matches!(base(f), "cd" | "pushd" | "chdir"));
        let in_repeat = u
            .compound
            .iter()
            .any(|(kind, _)| matches!(kind, Compound::Loop | Compound::Func));
        let mut cd_step_up = 0usize;
        let mut cd_step_down = 0usize;
        for (wi, w) in u.eff.iter().enumerate() {
            let up = upward_depth(w);
            if up > 0 {
                record(up, budget);
                record(cum_cd_up.saturating_add(up), budget);
                if cum_cd_down > 0 {
                    record(up.saturating_sub(cum_cd_down), budget);
                }
                if is_cd && wi > 0 {
                    cd_step_up = cd_step_up.max(up);
                    if in_repeat {
                        budget.unbounded = true;
                    }
                }
            }
            if is_cd
                && wi > 0
                && !w.starts_with('-')
                && !Path::new(w).is_absolute()
                && !w.starts_with('~')
            {
                cd_step_down = cd_step_down.max(downward_depth(w));
            }
            if let Some((_, val)) = w.split_once('=') {
                let vup = upward_depth(val);
                if vup > 0 {
                    record(vup, budget);
                    record(cum_cd_up.saturating_add(vup), budget);
                    if cum_cd_down > 0 {
                        record(vup.saturating_sub(cum_cd_down), budget);
                    }
                }
            }
        }
        for r in &u.seg.redirects {
            let up = upward_depth(&r.target);
            if up > 0 {
                record(up, budget);
                record(cum_cd_up.saturating_add(up), budget);
                if cum_cd_down > 0 {
                    record(up.saturating_sub(cum_cd_down), budget);
                }
            }
        }
        if is_cd {
            if cd_step_up > 0 {
                cum_cd_up = cum_cd_up.saturating_add(cd_step_up);
                record(cum_cd_up, budget);
            }
            if cd_step_down > 0 {
                cum_cd_down = cum_cd_down.saturating_add(cd_step_down);
            }
        }
    }
    depths
}

fn push_tree_tail(
    base: &str,
    tail: &[String],
    needed_ups: &[usize],
    budget: &mut CdGlobBudget,
    push_target: &mut impl FnMut(String),
) {
    if tail.is_empty() {
        push_target(base.to_string());
        return;
    }
    let concretize_comp = |c: &str| -> String {
        if c.contains(['*', '?', '[']) {
            "x".to_string()
        } else {
            c.to_string()
        }
    };
    if let Some(first_gs) = tail.iter().position(|c| c == "**") {
        let pre: Vec<String> = tail[..first_gs]
            .iter()
            .map(|c| concretize_comp(c))
            .collect();
        let post: Vec<String> = tail[first_gs + 1..]
            .iter()
            .filter(|c| c.as_str() != "**")
            .map(|c| concretize_comp(c))
            .collect();
        let net_comps = |comps: &[String]| -> isize {
            let mut d: isize = 0;
            for c in comps {
                if c == ".." {
                    d -= 1;
                } else if !c.is_empty() && c != "." {
                    d += 1;
                }
            }
            d
        };
        let fixed_net = net_comps(&pre) + net_comps(&post);
        let mut emit_depths: Vec<usize> = vec![0];
        for &up in needed_ups {
            let target_d = (up as isize) - fixed_net;
            if target_d >= 0 {
                let d = target_d as usize;
                if d > MAX_GLOBSTAR_DEPTH {
                    budget.unbounded = true;
                } else if !emit_depths.contains(&d) {
                    emit_depths.push(d);
                }
            }
        }
        if pre.is_empty() && post.is_empty() && !emit_depths.contains(&1) {
            emit_depths.push(1);
        }
        for depth in emit_depths {
            if !budget.charge(depth + 1) {
                break;
            }
            let mut comps = pre.clone();
            comps.extend(std::iter::repeat_n("x".to_string(), depth));
            comps.extend(post.iter().cloned());
            if comps.is_empty() {
                push_target(base.to_string());
            } else {
                push_target(format!("{base}/{}", comps.join("/")));
            }
        }
    } else {
        let comps: Vec<String> = tail.iter().map(|c| concretize_comp(c)).collect();
        push_target(format!("{base}/{}", comps.join("/")));
    }
}

#[allow(clippy::too_many_arguments)]
fn cd_glob_matches(
    expanded: &str,
    cands: &[PathBuf],
    start: &Path,
    home: &Path,
    protected_roots: &[PathBuf],
    units: &[Unit],
    unit_idx: usize,
    budget: &mut CdGlobBudget,
    resolve: Resolver<'_>,
) -> Vec<PathBuf> {
    let mut raw_patterns: Vec<String> = Vec::new();
    if Path::new(expanded).is_absolute() {
        raw_patterns.push(expanded.to_string());
    } else {
        for c in cands {
            let cs = c.to_string_lossy();
            let base_c = cs.trim_end_matches('/');
            let pat = format!("{base_c}/{expanded}");
            if !raw_patterns.contains(&pat) {
                raw_patterns.push(pat);
            }
        }
    }

    let mut variants: Vec<Vec<String>> = Vec::new();
    for raw in &raw_patterns {
        for v in normalize_cd_glob(raw, budget) {
            if !variants.contains(&v) {
                if variants.len() >= MAX_CD_GLOB_VARIANTS {
                    budget.unbounded = true;
                    break;
                }
                variants.push(v);
            }
        }
        if budget.unbounded {
            break;
        }
    }

    let has_globstar = variants.iter().any(|v| v.iter().any(|c| c == "**"));
    let needed_ups = if has_globstar {
        needed_upward_depths(units, unit_idx, budget)
    } else {
        Vec::new()
    };

    // Deduplicate on raw path before resolving so the gate (`fs_resolve`) and
    // engine (`lexical_resolve`) always build candidate lists of the exact same shape.
    let mut seen_raw: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut matched: Vec<PathBuf> = Vec::new();
    let mut push_target = |raw_path: String| {
        if seen_raw.insert(raw_path.clone()) {
            matched.push(resolve(&raw_path, start, home));
        }
    };

    for root in protected_roots {
        if budget.unbounded {
            break;
        }
        let root_str = root.to_string_lossy();
        let root_base = root_str.trim_end_matches('/');
        let target_owned: Vec<String> = root_base
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| s.replace('ſ', "s"))
            .collect();
        if target_owned.is_empty() {
            continue;
        }
        let target: Vec<&str> = target_owned.iter().map(String::as_str).collect();
        for v in &variants {
            if !budget.charge(1) {
                break;
            }
            let var_has_gs = v.iter().any(|c| c == "**");
            let exact = if var_has_gs {
                cd_comps_match_globstar(v, &target, budget)
            } else {
                cd_comps_match(v, &target, budget)
            };
            if exact {
                push_target(root_base.to_string());
            }
            if var_has_gs {
                for k in 1..=v.len() {
                    if cd_comps_match_globstar(&v[..k], &target, budget) {
                        // When `**` matched `target`, the same `**` can also
                        // continue into descendants of `target`.
                        let tail = if v[k - 1] == "**" {
                            &v[k - 1..]
                        } else {
                            &v[k..]
                        };
                        push_tree_tail(root_base, tail, &needed_ups, budget, &mut push_target);
                    }
                    if budget.unbounded {
                        break;
                    }
                }
            } else if v.len() > target.len() && cd_comps_match(&v[..target.len()], &target, budget)
            {
                push_tree_tail(root_base, &v[target.len()..], &[], budget, &mut push_target);
            }
            if budget.unbounded {
                break;
            }
        }
    }

    matched
}

#[allow(clippy::too_many_arguments)]
fn cd_effect(
    unit: &Unit,
    unit_idx: usize,
    units: &[Unit],
    followed: bool,
    cands: &[PathBuf],
    start: &Path,
    home: &Path,
    protected_roots: &[PathBuf],
    budget: &mut CdGlobBudget,
    resolve: Resolver<'_>,
) -> Option<Cd> {
    let first = unit.eff.first()?;
    let name = base(first);
    if !matches!(name, "cd" | "pushd" | "popd" | "chdir") {
        return None;
    }
    if name == "popd" {
        return Some(Cd::Lost(vec![], false));
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
    let mut glob_cd = false;
    let mut resolve_all = |a: &str| -> Option<Vec<PathBuf>> {
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
        if has_unknown_var(&expanded) {
            None
        } else if expanded.contains(['*', '?', '[']) {
            glob_cd = true;
            Some(cd_glob_matches(
                &expanded,
                cands,
                start,
                home,
                protected_roots,
                units,
                unit_idx,
                budget,
                resolve,
            ))
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
        Some(t) if readable && !glob_cd && name != "pushd" && followed => Cd::Moved(t),
        Some(t) => Cd::Lost(t, glob_cd),
        None => Cd::Lost(vec![], glob_cd),
    })
}

/// Appends up to `MAX_SEEN` directories. No two candidates are compared by
/// value: the gate (filesystem) and the engine (lexical) must build lists of
/// the same shape from the same words.
fn push_seen(seen: &mut Vec<PathBuf>, more: &[PathBuf]) -> bool {
    let mut truncated = false;
    for p in more {
        if seen.len() >= MAX_SEEN {
            truncated = true;
            break;
        }
        seen.push(p.clone());
    }
    truncated
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
pub fn walk(
    units: &[Unit],
    start: &Path,
    home: &Path,
    protected_roots: &[PathBuf],
    resolve: Resolver<'_>,
) -> Walk {
    let mut w = Walk::default();
    let mut cd_budget = CdGlobBudget::new();
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
        let effect = match cd_effect(
            unit,
            i,
            units,
            followed,
            &lv.cands,
            start,
            home,
            protected_roots,
            &mut cd_budget,
            resolve,
        ) {
            Some(Cd::Moved(t)) if repeats => Some(Cd::Lost(t, false)),
            e => e,
        };
        w.cd_glob_unbounded |= cd_budget.unbounded;
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
            Some(Cd::Lost(targets, from_glob)) => {
                let truncated = push_seen(&mut seen, &targets);
                if from_glob && truncated {
                    w.cd_glob_unbounded = true;
                }
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
