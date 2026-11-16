//! A tolerant shell lexer: one pass that yields simple commands (argv,
//! assignments, redirects) and notes everything a rule must not trust
//! (substitution, globs, expansion, backgrounding, here-docs, subshells).
//! The hard-deny list reads the same output but looks inside substitutions
//! and payloads (`sh -c`, `eval`, `find -exec`) too.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

use super::{Redirect, RedirectOp, Segment};

const MAX_DEPTH: u8 = 6;

/// How a command is joined to the one after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Join {
    /// `;`, a newline, `&`, `|` or the end of the line.
    #[default]
    Seq,
    /// `&&`: the next command runs only if this one succeeded.
    And,
    /// `||`: the next command runs only if this one failed.
    Or,
}

/// One simple command, plus what the lexer saw inside it.
#[derive(Debug, Clone)]
pub struct Unit {
    pub seg: Segment,
    /// argv after reserved words and wrappers (`env`, `command`, `exec`, …).
    pub eff: Vec<String>,
    /// Command and process substitution bodies found in this command.
    pub subs: Vec<String>,
    /// One of `subs` is a process substitution (`<(…)` / `>(…)`), whose
    /// output or input is another process rather than text.
    pub proc_sub: bool,
    /// Inside parentheses, a pipeline, or otherwise not in plain sequence:
    /// a `cd` here makes the working directory unknown.
    pub grouped: bool,
    /// For `find … -exec CMD {} …`: the directories `find` walks.
    pub exec_roots: Option<Vec<String>>,
    /// Produced by a payload or substitution, not written at top level.
    pub derived: bool,
    /// Non-zero: the units sharing this number are one pipeline.
    pub pipe: u32,
    /// The operator after this command, as written (top level of its text).
    pub join: Join,
    /// Which text this command was written in: every `lex` call (the line,
    /// one payload, one substitution, one here-doc) has its own number, so
    /// the directory walk can keep one `&&` chain per text.
    pub ctx: u32,
    /// How deeply that text is nested (0: the line itself).
    pub level: u8,
    /// The text runs in the shell of the text around it (`eval`, `source`,
    /// `.`), so a `cd` in it moves that shell too; otherwise it runs in a
    /// child (`sh -c`, a substitution, `find -exec`, a pipe).
    pub same_shell: bool,
    /// Followed by `&`: it runs in the background, in a child.
    pub background: bool,
    /// The compound commands of its text it is inside, outermost first, each
    /// with a number unique to that compound (round 6 item 4): the directory
    /// walk keeps one `&&` chain per compound and joins it to the chain
    /// around it when the compound ends.
    pub compound: Vec<(Compound, u32)>,
    /// The join written right after a `( … )` group this command ends
    /// (`(true) && x`): the group's own place in the chain around it.
    pub close_join: Option<Join>,
    /// With `same_shell`: the owner (`eval`, `source`) may run in a child
    /// instead (in a pipeline, a group or the background), so the shell
    /// after it may be where it was or where the text ended (round 6 item 7).
    pub shell_union: bool,
    /// The other readings of its wrappers (`eff` is the first): every check
    /// of what the command runs looks at all of them (M1-18, K1).
    pub alt: Vec<Vec<String>>,
}

impl Unit {
    /// `eff` and every other reading of the command's wrappers.
    pub fn readings(&self) -> impl Iterator<Item = &[String]> {
        std::iter::once(self.eff.as_slice()).chain(self.alt.iter().map(Vec::as_slice))
    }
}

/// What kind of compound command a command is inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compound {
    /// `( … )`: a subshell.
    Paren,
    /// `{ …; }`: runs once, in this shell.
    Brace,
    /// `if`, `case`, `coproc`: the body may not run (or runs in a child).
    Cond,
    /// `while`, `until`, `for`, `select`: the body runs any number of times.
    Loop,
    /// A function body: runs only when called, any number of times.
    Func,
}

/// An unknown wrapper invocation whose readings exceeded the bounded search
/// limit, together with whether the command sits downstream of a pipe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnboundedUnknown {
    pub words: Vec<String>,
    pub piped: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Lexed {
    pub units: Vec<Unit>,
    /// Why rules and Home's read-only list must not allow the line.
    pub reasons: Vec<String>,
    /// More commands than the checker takes on (`MAX_UNITS`).
    pub overflow: bool,
    /// Words of an unknown wrapper invocation whose readings could not be
    /// bounded (M1-18, K1 fail-closed).
    pub unbounded_unknown_words: Vec<UnboundedUnknown>,
    /// Every here-doc and here-string body at this level.
    pub stdin_texts: Vec<String>,
    /// The number of this text (`Unit::ctx`).
    pub ctx: u32,
}

pub const MAX_UNITS: usize = 512;

struct Builder {
    assignments: Vec<(String, String)>,
    argv: Vec<String>,
    redirects: Vec<Redirect>,
    subs: Vec<String>,
    proc_sub: bool,
    piped: bool,
    grouped: bool,
    pipe: u32,
    join: Join,
    background: bool,
    compound: Vec<(Compound, u32)>,
    close_join: Option<Join>,
}

static PIPE_IDS: AtomicU32 = AtomicU32::new(1);
static CTX_IDS: AtomicU32 = AtomicU32::new(1);
static GROUP_IDS: AtomicU32 = AtomicU32::new(1);

impl Builder {
    fn new(piped: bool, grouped: bool) -> Self {
        Self {
            pipe: 0,
            assignments: vec![],
            argv: vec![],
            redirects: vec![],
            subs: vec![],
            proc_sub: false,
            piped,
            grouped,
            join: Join::Seq,
            background: false,
            compound: vec![],
            close_join: None,
        }
    }
    fn is_empty(&self) -> bool {
        self.assignments.is_empty() && self.argv.is_empty() && self.redirects.is_empty()
    }
}

enum Pending {
    Redirect(u32, RedirectOp),
    HereString,
}

fn is_name(s: &str) -> bool {
    let mut it = s.chars();
    match it.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    it.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// How deeply the scanners below may nest (`$(` in `"` in `${` …) before
/// they give up and report the construct as unclosed (fail closed).
const MAX_SCAN_NEST: u16 = 128;

/// `start` is just after the opening delimiter; returns the body and the
/// index just past the matching `)`.
///
/// Round 6 item 3: every scanner skips every quoting construct the shell
/// reads inside it with the same scanners, at any depth: in a `$(…)`,
/// quotes, `${…}`, `` `…` `` and nested `$(…)`/`$((…))`; in a `"…"`,
/// `$(…)`, `` `…` `` and `${…}`; in a `${…}`, all of them (a `'` only
/// outside double quotes).
fn scan_paren(chars: &[char], start: usize) -> (String, usize, bool) {
    scan_paren_n(chars, start, 0)
}

fn scan_paren_n(chars: &[char], start: usize, n: u16) -> (String, usize, bool) {
    if n > MAX_SCAN_NEST {
        return (
            chars[start.min(chars.len())..].iter().collect(),
            chars.len(),
            false,
        );
    }
    let mut depth = 1usize;
    let mut i = start;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1,
            '\'' => {
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    i += 1;
                }
            }
            '"' => {
                let (end, _) = skip_dq(chars, i + 1, n + 1);
                i = end;
                continue;
            }
            '`' => {
                let (_, end, _) = scan_backtick(chars, i + 1);
                i = end;
                continue;
            }
            '$' if chars.get(i + 1) == Some(&'{') => {
                let (_, j, closed) = scan_dollar_brace_n(chars, i + 2, false, n + 1);
                if !closed {
                    return (chars[start..].iter().collect(), chars.len(), false);
                }
                i = j + 1;
                continue;
            }
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return (chars[start..i].iter().collect(), i + 1, true);
                }
            }
            _ => {}
        }
        i += 1;
    }
    (
        chars[start.min(chars.len())..].iter().collect(),
        chars.len(),
        false,
    )
}

/// From just after an opening `"`: the index just past the closing `"`, and
/// whether there was one. `$(…)`, `` `…` `` and `${…}` inside are skipped
/// whole (their quotes are their own).
fn skip_dq(chars: &[char], start: usize, n: u16) -> (usize, bool) {
    if n > MAX_SCAN_NEST {
        return (chars.len(), false);
    }
    let mut i = start;
    while i < chars.len() {
        match chars[i] {
            '"' => return (i + 1, true),
            '\\' => i += 2,
            '$' if chars.get(i + 1) == Some(&'(') => {
                let (_, end, ok) = scan_paren_n(chars, i + 2, n + 1);
                if !ok {
                    return (chars.len(), false);
                }
                i = end;
            }
            '$' if chars.get(i + 1) == Some(&'{') => {
                let (_, j, closed) = scan_dollar_brace_n(chars, i + 2, true, n + 1);
                if !closed {
                    return (chars.len(), false);
                }
                i = j + 1;
            }
            '`' => {
                let (_, end, ok) = scan_backtick(chars, i + 1);
                if !ok {
                    return (chars.len(), false);
                }
                i = end;
            }
            _ => i += 1,
        }
    }
    (chars.len(), false)
}

fn scan_backtick(chars: &[char], start: usize) -> (String, usize, bool) {
    let mut i = start;
    let mut body = String::new();
    while i < chars.len() {
        match chars[i] {
            '\\' if i + 1 < chars.len() => {
                body.push(chars[i + 1]);
                i += 2;
                continue;
            }
            '`' => return (body, i + 1, true),
            c => body.push(c),
        }
        i += 1;
    }
    (body, chars.len(), false)
}

/// Substitution bodies in text that is expanded but not word-split (here-doc
/// bodies, `$((…))`).
fn find_substitutions(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && chars.get(i + 1) == Some(&'(') {
            let (body, end, _) = scan_paren(&chars, i + 2);
            out.push(body);
            i = end;
        } else if chars[i] == '`' {
            let (body, end, _) = scan_backtick(&chars, i + 1);
            out.push(body);
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// The body of a `${…}` starting at `start` (just after the `{`) and the
/// index of its closing `}`: quotes, escapes, `$(…)`, `` `…` `` and nested
/// `${…}` inside don't end it (`${x:-"}"}`, `"${x:-"'"}"`,
/// `"${x:-$(echo '"')}"`). Inside double quotes (`in_dq`) a single quote is
/// an ordinary character. The flag is false when no closing `}` was found.
fn scan_dollar_brace(chars: &[char], start: usize, in_dq: bool) -> (String, usize, bool) {
    scan_dollar_brace_n(chars, start, in_dq, 0)
}

fn scan_dollar_brace_n(chars: &[char], start: usize, in_dq: bool, n: u16) -> (String, usize, bool) {
    let start = start.min(chars.len());
    if n > MAX_SCAN_NEST {
        return (chars[start..].iter().collect(), chars.len(), false);
    }
    let mut j = start;
    let unclosed = || {
        (
            chars[start..].iter().collect::<String>(),
            chars.len(),
            false,
        )
    };
    while j < chars.len() {
        match chars[j] {
            '}' => return (chars[start..j].iter().collect(), j, true),
            '\\' => j += 2,
            '$' if chars.get(j + 1) == Some(&'(') => {
                let (_, end, ok) = scan_paren_n(chars, j + 2, n + 1);
                if !ok {
                    return unclosed();
                }
                j = end;
            }
            '$' if chars.get(j + 1) == Some(&'{') => {
                let (_, k, closed) = scan_dollar_brace_n(chars, j + 2, in_dq, n + 1);
                if !closed {
                    return unclosed();
                }
                j = k + 1;
            }
            '`' => {
                let (_, end, ok) = scan_backtick(chars, j + 1);
                if !ok {
                    return unclosed();
                }
                j = end;
            }
            '"' => {
                let (end, ok) = skip_dq(chars, j + 1, n + 1);
                if !ok {
                    return unclosed();
                }
                j = end;
            }
            '\'' if !in_dq => {
                j += 1;
                while j < chars.len() && chars[j] != '\'' {
                    j += 1;
                }
                if j >= chars.len() {
                    return unclosed();
                }
                j += 1;
            }
            _ => j += 1,
        }
    }
    unclosed()
}

/// A `${…}` with no closing `}` where quotes are read the shell's way: a
/// shell that reads them otherwise ends it at the first `}`, and what
/// follows is a command line too, checked as one (round 5 blocker 2).
fn ambiguous_tail(chars: &[char], start: usize, tails: &mut Vec<String>) {
    if let Some(k) = chars[start.min(chars.len())..]
        .iter()
        .position(|c| *c == '}')
    {
        let rest: String = chars[start + k + 1..].iter().collect();
        if !rest.trim().is_empty() {
            tails.push(rest);
        }
    }
}

type LexCache = HashMap<(String, u8), Rc<Lexed>>;

fn refresh_ids(l: &mut Lexed) {
    let mut ctx_map: HashMap<u32, u32> = HashMap::new();
    let mut pipe_map: HashMap<u32, u32> = HashMap::new();
    let mut group_map: HashMap<u32, u32> = HashMap::new();
    l.ctx = *ctx_map
        .entry(l.ctx)
        .or_insert_with(|| CTX_IDS.fetch_add(1, Ordering::Relaxed));
    for u in &mut l.units {
        u.ctx = *ctx_map
            .entry(u.ctx)
            .or_insert_with(|| CTX_IDS.fetch_add(1, Ordering::Relaxed));
        if u.pipe != 0 {
            u.pipe = *pipe_map
                .entry(u.pipe)
                .or_insert_with(|| PIPE_IDS.fetch_add(1, Ordering::Relaxed));
        }
        for (_, gid) in &mut u.compound {
            *gid = *group_map
                .entry(*gid)
                .or_insert_with(|| GROUP_IDS.fetch_add(1, Ordering::Relaxed));
        }
    }
}

pub fn lex(line: &str) -> Lexed {
    let mut cache = LexCache::default();
    lex_depth_uncached(line, 0, false, &mut cache)
}

fn lex_depth_cached(line: &str, depth: u8, cache: &mut LexCache) -> Rc<Lexed> {
    if depth > MAX_DEPTH {
        return Rc::new(Lexed {
            ctx: CTX_IDS.fetch_add(1, Ordering::Relaxed),
            reasons: vec!["nested too deeply".into()],
            ..Lexed::default()
        });
    }
    let key = (line.to_string(), depth);
    if let Some(hit) = cache.get(&key) {
        return Rc::clone(hit);
    }
    let res = Rc::new(lex_depth_uncached(line, depth, true, cache));
    if cache.len() < 4096 {
        cache.insert(key, Rc::clone(&res));
    }
    res
}

fn lex_depth_fresh(line: &str, depth: u8, cache: &mut LexCache) -> Lexed {
    let mut l = (*lex_depth_cached(line, depth, cache)).clone();
    refresh_ids(&mut l);
    l
}

fn lex_depth_uncached(line: &str, depth: u8, derived: bool, cache: &mut LexCache) -> Lexed {
    let mut out = Lexed {
        ctx: CTX_IDS.fetch_add(1, Ordering::Relaxed),
        ..Lexed::default()
    };
    if depth > MAX_DEPTH {
        out.reasons.push("nested too deeply".into());
        return out;
    }
    let chars: Vec<char> = line.chars().collect();
    // Shells treat `\r` as a word character; any other control character is
    // something this lexer can't vouch for.
    if chars.iter().any(|c| {
        (c.is_control() && !matches!(c, '\n' | '\t' | '\r')) || matches!(c, '\u{2028}' | '\u{2029}')
    }) {
        out.reasons.push("control character".into());
    }
    let mut bodies: Vec<(usize, String)> = vec![];
    let mut i = 0usize;
    let mut cur = Builder::new(false, false);
    let mut paren_depth = 0usize;
    let mut pending: Option<Pending> = None;
    let mut heredocs: Vec<(String, bool, bool)> = vec![];
    // the command each here-doc belongs to (the one holding the `<<`)
    let mut heredoc_owners: Vec<usize> = vec![];
    let mut raw_units: Vec<Builder> = vec![];
    // Open `( … )` groups (`true`) and `{ …; }`, `if`, `while`, `until`,
    // `for`, `case`, `select` bodies (`false`); `Some(pipe)` for one a pipe
    // feeds (`… | (true; sh)`, `… | { :; sh; }`, `… | if …; then sh; fi`):
    // every command in it may read the pipe (re-review blocker 5).
    // (kind, the pipe feeding it, its number)
    let mut groups: Vec<(Compound, Option<u32>, u32)> = vec![];
    // `f()` was just read: the next compound is a function body
    let mut fn_def = false;
    // a `( … )` group just closed: redirections that follow are its own
    // (`(true) > /dev/null && x`), and so is the join after them
    let mut after_paren = false;
    // text after an unclosed `${…}` (see `ambiguous_tail`)
    let mut tails: Vec<String> = vec![];

    macro_rules! fresh {
        () => {{
            let mut next = Builder::new(false, paren_depth > 0);
            if let Some(id) = groups.iter().rev().find_map(|g| g.1) {
                next.piped = true;
                next.grouped = true;
                next.pipe = id;
            }
            next
        }};
    }
    macro_rules! flush {
        () => {{
            #[allow(unused_assignments)]
            {
                after_paren = false;
            }
            let next = fresh!();
            let mut b = std::mem::replace(&mut cur, next);
            if !b.is_empty() {
                b.compound = groups.iter().map(|g| (g.0, g.2)).collect();
                raw_units.push(b);
            }
        }};
    }

    while i < chars.len() {
        let c = chars[i];
        // Separators and whitespace.
        match c {
            ' ' | '\t' => {
                i += 1;
                continue;
            }
            '\\' if chars.get(i + 1) == Some(&'\n') => {
                i += 2;
                continue;
            }
            '\n' => {
                i += 1;
                flush!();
                // Here-doc bodies start on the next line.
                let owners = std::mem::take(&mut heredoc_owners);
                for (hd_idx, (delim, strip, quoted)) in
                    std::mem::take(&mut heredocs).into_iter().enumerate()
                {
                    let owner = owners.get(hd_idx).copied().unwrap_or(usize::MAX);
                    let mut body = String::new();
                    loop {
                        if i >= chars.len() {
                            break;
                        }
                        let mut j = i;
                        while j < chars.len() && chars[j] != '\n' {
                            j += 1;
                        }
                        let l: String = chars[i..j].iter().collect();
                        i = (j + 1).min(chars.len());
                        let t = if strip {
                            l.trim_start_matches('\t')
                        } else {
                            l.as_str()
                        };
                        if t == delim {
                            break;
                        }
                        body.push_str(&l);
                        body.push('\n');
                    }
                    bodies.push((owner, body.clone()));
                    if !quoted {
                        for sub in find_substitutions(&body) {
                            if let Some(last) = raw_units.last_mut() {
                                last.subs.push(sub);
                            }
                        }
                    }
                }
                continue;
            }
            ';' => {
                i += if chars.get(i + 1) == Some(&';') { 2 } else { 1 };
                flush!();
                continue;
            }
            '(' | ')' => {
                out.reasons.push("subshell".into());
                i += 1;
                if c == '(' {
                    // `f()` / `f ()`: a function definition, not a subshell
                    let mut k = i;
                    while matches!(chars.get(k), Some(' ' | '\t')) {
                        k += 1;
                    }
                    // at any command position: after `{`, `!`, `time -p`,
                    // `coproc`, `then`, … too (round 7 item 4)
                    let n = cur.argv.len();
                    let def_position = n >= 1
                        && (opens_body(&cur.argv[..n - 1])
                            || (n >= 2
                                && cur.argv[n - 2] == "function"
                                && opens_body(&cur.argv[..n - 2])));
                    if chars.get(k) == Some(&')') && cur.assignments.is_empty() && def_position {
                        out.reasons.push("function definition".into());
                        i = k + 1;
                        // the name and the body are one command: its place
                        // in a chain is the join after the body
                        cur.join = Join::And;
                        flush!();
                        fn_def = true;
                        continue;
                    }
                    // `… | (sh)`: the group's commands read the pipe
                    let g = (cur.is_empty() && cur.piped).then_some(cur.pipe);
                    let kind = if std::mem::take(&mut fn_def) {
                        Compound::Func
                    } else {
                        Compound::Paren
                    };
                    flush!();
                    paren_depth += 1;
                    groups.push((kind, g, GROUP_IDS.fetch_add(1, Ordering::Relaxed)));
                    cur = fresh!();
                    cur.grouped = true;
                } else {
                    // the commands inside belong to the group
                    flush!();
                    paren_depth = paren_depth.saturating_sub(1);
                    if groups
                        .last()
                        .is_some_and(|g| matches!(g.0, Compound::Paren | Compound::Func))
                    {
                        groups.pop();
                    } else {
                        // `case x in a)`: a pattern, not a group
                    }
                    cur = fresh!();
                    cur.grouped = cur.grouped || paren_depth > 0;
                    after_paren = true;
                }
                continue;
            }
            '&' if chars.get(i + 1) == Some(&'&') => {
                i += 2;
                if group_tail(&cur, after_paren)
                    && let Some(last) = raw_units.last_mut()
                {
                    // `( … ) && x`
                    last.close_join = Some(Join::And);
                }
                cur.join = Join::And;
                flush!();
                continue;
            }
            '&' if chars.get(i + 1) == Some(&'>') => {
                // `&>` / `&>>`
                let (op, n) = if chars.get(i + 2) == Some(&'>') {
                    (RedirectOp::Append, 3)
                } else {
                    (RedirectOp::Write, 2)
                };
                i += n;
                pending = Some(Pending::Redirect(1, op));
                continue;
            }
            '&' => {
                out.reasons.push("backgrounding".into());
                i += 1;
                cur.background = true;
                flush!();
                continue;
            }
            '|' => {
                let is_pipe = chars.get(i + 1) != Some(&'|');
                i += if is_pipe && chars.get(i + 1) == Some(&'&') {
                    2
                } else if is_pipe {
                    1
                } else {
                    2
                };
                // `( … ) | x`: the group's place in the chain is the
                // pipeline's (round 7 major)
                if is_pipe
                    && after_paren
                    && group_tail(&cur, after_paren)
                    && let Some(last) = raw_units.last_mut()
                {
                    last.close_join = Some(Join::And);
                }
                // Both ends of a pipeline are not in plain sequence.
                cur.grouped |= is_pipe;
                if is_pipe && cur.pipe == 0 {
                    cur.pipe = PIPE_IDS.fetch_add(1, Ordering::Relaxed);
                }
                let id = cur.pipe;
                if !is_pipe {
                    if group_tail(&cur, after_paren)
                        && let Some(last) = raw_units.last_mut()
                    {
                        // `( … ) || x`
                        last.close_join = Some(Join::Or);
                    }
                    cur.join = Join::Or;
                }
                flush!();
                if is_pipe {
                    cur.piped = true;
                    cur.grouped = true;
                    cur.pipe = id;
                }
                continue;
            }
            '#' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            _ => {}
        }

        // Redirection operators (with an optional leading fd number).
        if c == '<' || c == '>' {
            if chars.get(i + 1) == Some(&'(') {
                // process substitution: handled as part of a word below
            } else {
                let fd_default = if c == '<' { 0 } else { 1 };
                let before = heredocs.len();
                i = lex_redirect(&chars, i, fd_default, &mut pending, &mut heredocs, &mut out);
                if heredocs.len() > before {
                    heredoc_owners.push(raw_units.len());
                }
                continue;
            }
        }

        // A word.
        let mut word = String::new();
        let mut unq_glob = false;
        let mut brace = false;
        let mut expansion = false;
        let mut quoted_prefix_empty = true; // no quote seen before first char
        let mut assign_name_end: Option<usize> = None;
        let start_i = i;
        while i < chars.len() {
            let c = chars[i];
            match c {
                ' ' | '\t' | '\n' | ';' | '&' | '|' | '(' | ')' => break,
                '<' | '>' if chars.get(i + 1) != Some(&'(') => {
                    // `2>file`: digits directly before a redirect are the fd.
                    break;
                }
                '\\' => {
                    if let Some(n) = chars.get(i + 1) {
                        if *n != '\n' {
                            word.push(*n);
                        }
                        i += 2;
                    } else {
                        i += 1;
                    }
                    quoted_prefix_empty = false;
                }
                '\'' => {
                    i += 1;
                    quoted_prefix_empty = false;
                    let mut closed = false;
                    while i < chars.len() {
                        if chars[i] == '\'' {
                            closed = true;
                            i += 1;
                            break;
                        }
                        word.push(chars[i]);
                        i += 1;
                    }
                    if !closed {
                        out.reasons.push("unterminated quote".into());
                    }
                }
                '"' => {
                    i += 1;
                    quoted_prefix_empty = false;
                    let mut closed = false;
                    while i < chars.len() {
                        match chars[i] {
                            '"' => {
                                closed = true;
                                i += 1;
                                break;
                            }
                            '\\' if matches!(
                                chars.get(i + 1),
                                Some('$' | '`' | '"' | '\\' | '\n')
                            ) =>
                            {
                                if chars[i + 1] != '\n' {
                                    word.push(chars[i + 1]);
                                }
                                i += 2;
                            }
                            '$' if chars.get(i + 1) == Some(&'(') => {
                                let (body, end, ok) = scan_paren(&chars, i + 2);
                                if !ok {
                                    out.reasons.push("unterminated substitution".into());
                                }
                                word.push_str("$(");
                                word.push_str(&body);
                                word.push(')');
                                cur.subs.push(body);
                                expansion = true;
                                i = end;
                            }
                            '`' => {
                                let (body, end, ok) = scan_backtick(&chars, i + 1);
                                if !ok {
                                    out.reasons.push("unterminated substitution".into());
                                }
                                word.push('`');
                                word.push_str(&body);
                                word.push('`');
                                cur.subs.push(body);
                                expansion = true;
                                i = end;
                            }
                            '$' if chars.get(i + 1) == Some(&'{') => {
                                // `"${x:-"'"}"`: the inner quotes belong to
                                // the expansion (round 5 blocker 2)
                                let (body, j, closed) = scan_dollar_brace(&chars, i + 2, true);
                                if !closed {
                                    ambiguous_tail(&chars, i + 2, &mut tails);
                                }
                                word.push_str("${");
                                word.push_str(&body);
                                word.push('}');
                                for sub in find_substitutions(&body) {
                                    cur.subs.push(sub);
                                }
                                expansion = true;
                                i = (j + 1).min(chars.len());
                            }
                            '$' => {
                                expansion = true;
                                word.push('$');
                                i += 1;
                            }
                            ch => {
                                word.push(ch);
                                i += 1;
                            }
                        }
                    }
                    if !closed {
                        out.reasons.push("unterminated quote".into());
                    }
                }
                '$' => {
                    expansion = true;
                    if chars.get(i + 1) == Some(&'(') {
                        let dbl = chars.get(i + 2) == Some(&'(');
                        let (body, end, ok) = scan_paren(&chars, i + 2);
                        if !ok {
                            out.reasons.push("unterminated substitution".into());
                        }
                        word.push_str("$(");
                        word.push_str(&body);
                        word.push(')');
                        if dbl {
                            for s in find_substitutions(&body) {
                                cur.subs.push(s);
                            }
                        }
                        cur.subs.push(body);
                        i = end;
                    } else if chars.get(i + 1) == Some(&'\'') {
                        // ANSI-C quoting: contents are escape sequences.
                        word.push('$');
                        i += 1;
                    } else if chars.get(i + 1) == Some(&'{') {
                        let (body, j, closed) = scan_dollar_brace(&chars, i + 2, false);
                        if !closed {
                            ambiguous_tail(&chars, i + 2, &mut tails);
                        }
                        word.push_str("${");
                        word.push_str(&body);
                        word.push('}');
                        i = (j + 1).min(chars.len());
                        for s in find_substitutions(&body) {
                            cur.subs.push(s);
                        }
                    } else {
                        word.push('$');
                        i += 1;
                    }
                }
                '`' => {
                    let (body, end, ok) = scan_backtick(&chars, i + 1);
                    if !ok {
                        out.reasons.push("unterminated substitution".into());
                    }
                    word.push('`');
                    word.push_str(&body);
                    word.push('`');
                    cur.subs.push(body);
                    expansion = true;
                    i = end;
                }
                '<' | '>' => {
                    // process substitution `<(…)` / `>(…)`
                    let (body, end, ok) = scan_paren(&chars, i + 2);
                    if !ok {
                        out.reasons.push("unterminated substitution".into());
                    }
                    word.push(c);
                    word.push('(');
                    word.push_str(&body);
                    word.push(')');
                    cur.subs.push(body);
                    cur.proc_sub = true;
                    expansion = true;
                    i = end;
                }
                '*' | '?' | '[' => {
                    unq_glob = true;
                    word.push(c);
                    i += 1;
                }
                '{' => {
                    // brace expansion needs a `,` or `..` at depth 1
                    let mut j = i + 1;
                    let mut d = 1;
                    let mut has = false;
                    while j < chars.len() && d > 0 {
                        if j - i > 256 {
                            // too far to tell: count it as expanding (asks)
                            has = true;
                            d = 0;
                            break;
                        }
                        match chars[j] {
                            '{' => d += 1,
                            '}' => d -= 1,
                            ',' if d == 1 => has = true,
                            '.' if d == 1 && chars.get(j + 1) == Some(&'.') => has = true,
                            ' ' | '\t' | '\n' | ';' | '&' | '|' => break,
                            _ => {}
                        }
                        j += 1;
                    }
                    if d == 0 && has {
                        brace = true;
                    }
                    word.push(c);
                    i += 1;
                }
                '~' if word.is_empty() && quoted_prefix_empty => {
                    // `~` and `~/x` are the user's home; `~name` is someone else's.
                    match chars.get(i + 1) {
                        None | Some('/') | Some(' ') | Some('\t') | Some('\n') | Some(';')
                        | Some('&') | Some('|') | Some(')') => {}
                        Some(_) => out.reasons.push("tilde expansion".into()),
                    }
                    word.push('~');
                    i += 1;
                }
                '=' if assign_name_end.is_none() && is_name(&word) && quoted_prefix_empty => {
                    assign_name_end = Some(word.len());
                    word.push('=');
                    i += 1;
                }
                ch => {
                    word.push(ch);
                    i += 1;
                    if ch != '=' {
                        // keep `quoted_prefix_empty` semantics: only quotes clear it
                    }
                }
            }
        }
        if i == start_i {
            // A lone character the loop didn't consume (shouldn't happen).
            i += 1;
            continue;
        }
        if word.starts_with('-') && !word.starts_with("--") && word.chars().count() > 64 {
            out.reasons.push("flag cluster too long".into());
        }
        if unq_glob {
            out.reasons.push("glob".into());
        }
        if brace {
            out.reasons.push("brace expansion".into());
        }
        if expansion {
            out.reasons.push("expansion".into());
        }
        // `2>file` / `3<file`: a number glued to a redirect is its fd.
        if let Some(op_c) = chars.get(i)
            && (*op_c == '<' || *op_c == '>')
            && chars.get(i + 1) != Some(&'(')
            && !word.is_empty()
            && word.chars().all(|c| c.is_ascii_digit())
            && pending.is_none()
        {
            let fd: u32 = word.parse().unwrap_or(1);
            let fd_default = fd;
            let mut dummy = None;
            let before = heredocs.len();
            let new_i = lex_redirect(&chars, i, fd_default, &mut dummy, &mut heredocs, &mut out);
            if heredocs.len() > before {
                heredoc_owners.push(raw_units.len());
            }
            pending = dummy;
            i = new_i;
            continue;
        }
        match pending.take() {
            Some(Pending::Redirect(fd, op)) => {
                cur.redirects.push(Redirect {
                    fd,
                    op: fix_dup(op, &word),
                    target: word,
                });
            }
            Some(Pending::HereString) => {
                out.reasons.push("here-string".into());
                bodies.push((raw_units.len(), word));
            }
            None => {
                // `{ …; }` groups, for a pipe into one
                // command position: nothing yet, or only words that start
                // a body (`then {`, `do if …`, `else (…)`) — round 5
                // blocker 3
                let command_position = cur.assignments.is_empty() && opens_body(&cur.argv);
                if command_position && quoted_prefix_empty {
                    let kind = match word.as_str() {
                        "{" if std::mem::take(&mut fn_def)
                            || cur.argv.iter().any(|w| w == "function") =>
                        {
                            Some(Compound::Func)
                        }
                        "{" if cur.argv.iter().any(|w| w == "coproc") => Some(Compound::Cond),
                        "{" => Some(Compound::Brace),
                        "if" | "case" => Some(Compound::Cond),
                        "while" | "until" | "for" | "select" => Some(Compound::Loop),
                        _ => None,
                    };
                    if let Some(kind) = kind {
                        let kind = if std::mem::take(&mut fn_def) {
                            Compound::Func
                        } else {
                            kind
                        };
                        groups.push((
                            kind,
                            cur.piped.then_some(cur.pipe),
                            GROUP_IDS.fetch_add(1, Ordering::Relaxed),
                        ));
                    } else if matches!(word.as_str(), "}" | "fi" | "done" | "esac") {
                        if groups.last().is_some_and(|g| g.0 != Compound::Paren) {
                            groups.pop();
                        } else {
                            out.reasons.push("unbalanced compound".into());
                        }
                    } else {
                        fn_def = false;
                    }
                }
                after_paren = false;
                if cur.argv.is_empty() && assign_name_end.is_some() {
                    let n = assign_name_end.unwrap_or(0);
                    cur.assignments
                        .push((word[..n].to_string(), word[n + 1..].to_string()));
                } else {
                    cur.argv.push(word);
                }
            }
        }
    }
    flush!();
    if !groups.is_empty() {
        out.reasons.push("unbalanced compound".into());
    }
    if pending.is_some() {
        out.reasons.push("redirect without target".into());
    }
    if !heredocs.is_empty() {
        // here-doc body never started (no newline after the operator)
    }

    // Reserved words and wrappers; payloads become extra units.
    let mut raw_pos: Vec<usize> = vec![];
    for b in raw_units {
        raw_pos.push(out.units.len());
        if out.units.len() > MAX_UNITS {
            out.overflow = true;
            return out;
        }
        let Builder {
            assignments,
            argv,
            redirects,
            subs,
            proc_sub,
            piped,
            grouped,
            pipe,
            join,
            background,
            compound,
            close_join,
        } = b;
        let seg = Segment {
            assignments,
            argv,
            redirects,
            piped,
        };
        let (eff, unknown_option) = unwrap_checked_at(&seg.argv, depth, cache);
        if unknown_option {
            out.reasons.push("unknown wrapper option".into());
        }
        // the other ways its wrappers can be read (round 7 item 6): each is
        // checked as a command of its own
        let mut other_readings: Vec<Vec<String>> = if unknown_option {
            let mut readings = Vec::new();
            for e in unwrap_readings_bounded(
                &seg.argv,
                depth,
                cache,
                piped,
                &mut out.unbounded_unknown_words,
            )
            .into_iter()
            .skip(1)
            {
                if !e.is_empty() && !readings.contains(&e) {
                    readings.push(e);
                }
            }
            readings
        } else {
            vec![]
        };
        // a command word built by a substitution that names a program
        // plainly (`$(which sudo) id`, `$(which env) rm -rf ~`) is also read
        // as that program, behind its wrappers (round 7, beside item 5)
        if let Some(f) = eff.first()
            && has_command_substitution(f)
            && let Some(name) = substituted_program_at(f, depth, cache)
        {
            let mut e = eff.clone();
            e[0] = name;
            for r in unwrap_readings(&e, depth, cache) {
                if !r.is_empty() && r != eff && !other_readings.contains(&r) {
                    other_readings.push(r);
                }
            }
        }
        if let Some(first) = seg.argv.first()
            && RESERVED.contains(&first.as_str())
        {
            out.reasons.push("compound command".into());
        }
        let mut extra: Vec<Lexed> = vec![];
        for body in &subs {
            let l = lex_depth_fresh(body, depth + 1, cache);
            out.overflow |= l.overflow;
            out.unbounded_unknown_words
                .extend(l.unbounded_unknown_words.iter().cloned());
            extra.push(l);
        }
        let mut payload_units: Vec<Unit> = vec![];
        // `eval` and `source` run their text in this shell
        // (also when a substitution names them: `$(which eval)`, `$(which
        // env) eval`: round 8 blocker 1)
        // Every reading of the wrappers counts (M1-18, K1): `command -Z X
        // eval …` runs `eval` in the reading where `-Z` takes `X`.
        let mut effs: Vec<Vec<String>> = vec![eff.clone()];
        effs.extend(other_readings.iter().cloned());
        let here: Vec<bool> = effs.iter().map(|e| runs_here(e, depth, cache)).collect();
        let same_shell_owner = here.iter().any(|h| *h);
        // in a pipeline, a group or the background it may run in a child;
        // when only some readings run the text here, the shell after it may
        // be where it was or where the text ended
        let owner_maybe_child =
            pipe != 0 || grouped || background || piped || !here.iter().all(|h| *h);
        let targets: Vec<String> = seg.redirects.iter().map(|r| r.target.clone()).collect();
        let payloads = code_texts_bounded(
            &seg.argv,
            &effs,
            &targets,
            depth,
            cache,
            piped,
            &mut out.unbounded_unknown_words,
        );
        for p in &payloads {
            let l = lex_depth_fresh(p, depth + 1, cache);
            out.overflow |= l.overflow;
            out.unbounded_unknown_words
                .extend(l.unbounded_unknown_words.iter().cloned());
            payload_units.extend(l.units.into_iter().map(|mut u| {
                if u.ctx == l.ctx {
                    u.same_shell = same_shell_owner;
                    u.shell_union = same_shell_owner && owner_maybe_child;
                }
                u
            }));
            // reasons of a payload don't matter: the call is `sh -c …`, which
            // rules never match as a prefix of `sh`, and the hard-deny list
            // reads the units.
        }
        let exec_roots = None;
        let unit = Unit {
            seg,
            eff: eff.clone(),
            subs,
            proc_sub,
            grouped,
            exec_roots,
            derived,
            pipe,
            join,
            ctx: out.ctx,
            level: depth,
            same_shell: false,
            background,
            compound,
            close_join,
            shell_union: false,
            alt: other_readings.clone(),
        };
        let readings: Vec<Unit> = other_readings
            .into_iter()
            .map(|e| Unit {
                eff: e,
                subs: vec![],
                grouped: true,
                derived: true,
                pipe: 0,
                join: Join::Seq,
                ctx: CTX_IDS.fetch_add(1, Ordering::Relaxed),
                level: depth + 1,
                compound: vec![],
                close_join: None,
                alt: vec![],
                ..unit.clone()
            })
            .collect();
        out.units.push(unit);
        if eff.first().is_some_and(|s| cmd_name(s) == "find") {
            find_exec_units(&eff, &mut out, depth + 1, cache);
        }
        for u in readings {
            if u.eff.first().is_some_and(|s| cmd_name(s) == "find") {
                let e = u.eff.clone();
                out.units.push(u);
                find_exec_units(&e, &mut out, depth + 1, cache);
            } else {
                out.units.push(u);
            }
        }
        for l in extra {
            for mut u in l.units {
                u.grouped = true;
                u.derived = true;
                out.units.push(u);
            }
        }
        for mut u in payload_units {
            u.derived = true;
            out.units.push(u);
        }
    }
    // Text fed to a shell on stdin (here-docs, here-strings, `echo … | sh`) is
    // a command line too. Any shell, `eval` or `source` marks it; each text is
    // lexed once.
    // Every reading of a command's wrappers counts (M1-18, K1).
    let is_stdin_code = |u: &Unit, cache: &mut LexCache| {
        u.readings().any(|e| stdin_code_eff(e, depth, cache)) || runs_here_any(u, depth, cache)
    };
    // A pipe into a command named at run time (`| $PAGER`) is read only when
    // it asks for a script on stdin (`-s`, `-`).
    let is_pipe_code =
        |u: &Unit, cache: &mut LexCache| u.readings().any(|e| pipe_code_eff(e, depth, cache));
    if out.units.len() > MAX_UNITS {
        out.overflow = true;
        return out;
    }
    // (text, where its owner is when the text runs in this shell)
    let mut texts: Vec<(String, Option<usize>, bool)> = vec![];
    // A body is shell-fed (lexed for the hard-deny list) unless it is clearly
    // text: its owner is a simple command that isn't a shell, `eval`,
    // `source` or an interpreter, and whose output isn't piped or
    // process-substituted. Groups, loops, subshells and anything piped count.
    let reads_code = |u: &Unit, cache: &mut LexCache| {
        is_stdin_code(u, cache)
            || u.readings().any(|e| {
                e.first().is_some_and(|f| {
                    let b = cmd_name(f);
                    b.starts_with("python")
                        || matches!(
                            b.as_ref(),
                            "perl" | "ruby" | "node" | "deno" | "bun" | "php" | "lua"
                        )
                })
            })
    };
    out.stdin_texts = bodies.iter().map(|(_, t)| t.clone()).collect();
    let is_crontab = |u: &Unit| u.eff.first().is_some_and(|f| cmd_name(f) == "crontab");
    for (owner, text) in &bodies {
        let owner_unit = raw_pos.get(*owner).and_then(|p| out.units.get(*p));
        if owner_unit.is_some_and(is_crontab) {
            texts.push((crontab_commands(text), None, false));
        }
        let same_shell = raw_pos.get(*owner).copied().filter(|p| {
            out.units
                .get(*p)
                .is_some_and(|u| runs_here_any(u, depth, cache))
        });
        // in a child, or only in some readings of its wrappers: both
        let union = same_shell.and_then(|p| out.units.get(p)).is_some_and(|u| {
            u.pipe != 0
                || u.grouped
                || u.background
                || u.seg.piped
                || !u.readings().all(|e| runs_here(e, depth, cache))
        });
        let clearly_text = raw_pos
            .get(*owner)
            .and_then(|p| out.units.get(*p))
            .is_some_and(|u| {
                !u.seg.argv.first().is_some_and(|w| {
                    (RESERVED.contains(&w.as_str()) && w != "!") || w == "}" || w == ")"
                }) && !reads_code(u, cache)
                    // `ssh host $(echo bash) <<EOF`: for a command that runs
                    // another one, a word built by a substitution may name
                    // the program that reads the body
                    && !(u
                        .eff
                        .first()
                        .is_some_and(|f| COMMAND_RUNNERS.contains(&cmd_name(f).as_ref()))
                        && u.seg.argv.iter().any(|w| has_command_substitution(w)))
                    && u.pipe == 0
                    && !u.proc_sub
                    && !u.grouped
            });
        if !clearly_text {
            texts.push((text.clone(), same_shell, union));
        }
    }
    let mut seen_pipes: Vec<u32> = vec![];
    for k in 0..out.units.len() {
        let pipe = out.units[k].pipe;
        if pipe == 0 || !out.units[k].seg.piped || seen_pipes.contains(&pipe) {
            continue;
        }
        if !is_pipe_code(&out.units[k], cache) {
            continue;
        }
        seen_pipes.push(pipe);
        let mut piped: Vec<String> = vec![];
        for prev in out.units.iter().filter(|p| p.pipe == pipe) {
            echo_printf_texts(&prev.eff, &mut piped);
        }
        if is_crontab(&out.units[k]) {
            let jobs: Vec<String> = piped.iter().map(|t| crontab_commands(t)).collect();
            piped.extend(jobs);
        }
        // a pipeline's commands run in children, except (zsh) its last
        // one: `… | source /dev/stdin` may run in this shell
        let same_shell = runs_here_any(&out.units[k], depth, cache).then_some(k);
        // ... in a child for other shells: both (round 6 item 7)
        texts.extend(piped.into_iter().map(|t| (t, same_shell, true)));
    }
    texts.extend(tails.into_iter().map(|t| (t, None, false)));
    let mut extra_units: Vec<Unit> = vec![];
    // a text run in this shell goes right after its owner (and the owner's
    // own payloads), so the directory walk sees its `cd`s in order
    let mut placed: Vec<(usize, Vec<Unit>)> = vec![];
    for (t, owner, union) in &texts {
        let l = lex_depth_fresh(t, depth + 1, cache);
        out.overflow |= l.overflow;
        out.unbounded_unknown_words
            .extend(l.unbounded_unknown_words.iter().cloned());
        let mut units = vec![];
        for mut u in l.units {
            u.derived = true;
            if u.ctx == l.ctx {
                u.same_shell = owner.is_some();
                u.shell_union = owner.is_some() && *union;
            }
            units.push(u);
        }
        let added = extra_units.len() + placed.iter().map(|p| p.1.len()).sum::<usize>();
        if out.units.len() + added + units.len() > MAX_UNITS {
            out.overflow = true;
            return out;
        }
        match owner {
            Some(k) => {
                let at = out.units[k + 1..]
                    .iter()
                    .position(|n| n.ctx == out.ctx)
                    .map_or(out.units.len(), |p| k + 1 + p);
                placed.push((at, units));
            }
            None => extra_units.extend(units),
        }
    }
    out.units.extend(extra_units);
    placed.sort_by_key(|p| std::cmp::Reverse(p.0));
    for (at, units) in placed {
        out.units.splice(at..at, units);
    }
    out.reasons.sort();
    out.reasons.dedup();
    out
}

/// Whether the words read so far leave the next word in command position,
/// where a reserved word opens or closes a compound command: nothing yet, or
/// only words that can precede a command (`then`, `do`, `else`, `elif`,
/// `!`, `{`, `if`, `while`, `until`, `time [-p]`, `coproc [NAME]`,
/// `function NAME`) — round 6 item 4.
fn opens_body(argv: &[String]) -> bool {
    let mut k = 0;
    while k < argv.len() {
        match argv[k].as_str() {
            "then" | "do" | "else" | "elif" | "!" | "{" | "if" | "while" | "until" => k += 1,
            "time" => {
                k += 1;
                if argv.get(k).is_some_and(|w| w == "-p") {
                    k += 1;
                }
            }
            "function" => {
                // `function NAME` (its body comes next)
                if k + 1 >= argv.len() {
                    return false;
                }
                k += 2;
            }
            "coproc" => {
                k += 1;
                // `coproc NAME {`
                if k < argv.len() && is_name(&argv[k]) && argv.get(k + 1).is_none_or(|w| w == "{") {
                    k += 1;
                }
            }
            _ => return false,
        }
    }
    true
}

/// Words that start compound commands; a segment starting with one can't be
/// matched by a rule.
const RESERVED: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "function", "select", "{", "}", "!", "[[", "coproc",
];

/// Whether the command being read is only what follows a `( … )` group
/// (nothing, or its redirections): a join here is the group's.
fn group_tail(cur: &Builder, after_paren: bool) -> bool {
    cur.is_empty() || (after_paren && cur.argv.is_empty() && cur.assignments.is_empty())
}

fn fix_dup(op: RedirectOp, word: &str) -> RedirectOp {
    // `>&file` writes both stdout and stderr to the file; `<&file` is an error.
    if op == RedirectOp::Dup && !(word == "-" || word.chars().all(|c| c.is_ascii_digit())) {
        return RedirectOp::Write;
    }
    op
}

/// Lexes a redirection operator at `i`; returns the new index.
fn lex_redirect(
    chars: &[char],
    mut i: usize,
    fd_default: u32,
    pending: &mut Option<Pending>,
    heredocs: &mut Vec<(String, bool, bool)>,
    out: &mut Lexed,
) -> usize {
    let c = chars[i];
    let n1 = chars.get(i + 1).copied();
    let n2 = chars.get(i + 2).copied();
    if c == '<' && n1 == Some('<') && n2 == Some('<') {
        i += 3;
        *pending = Some(Pending::HereString);
        return i;
    }
    if c == '<' && n1 == Some('<') {
        i += 2;
        let strip = chars.get(i) == Some(&'-');
        if strip {
            i += 1;
        }
        while matches!(chars.get(i), Some(' ' | '\t')) {
            i += 1;
        }
        let mut delim = String::new();
        let mut quoted = false;
        while i < chars.len()
            && !matches!(
                chars[i],
                ' ' | '\t' | '\n' | ';' | '&' | '|' | '(' | ')' | '<' | '>'
            )
        {
            match chars[i] {
                '\'' | '"' | '\\' => quoted = true,
                ch => delim.push(ch),
            }
            i += 1;
        }
        out.reasons.push("here-doc".into());
        heredocs.push((delim, strip, quoted));
        return i;
    }
    let (op, n) = match (c, n1) {
        ('>', Some('>')) => (RedirectOp::Append, 2),
        ('>', Some('&')) => (RedirectOp::Dup, 2),
        ('>', Some('|')) => (RedirectOp::Write, 2),
        ('>', _) => (RedirectOp::Write, 1),
        ('<', Some('>')) => (RedirectOp::ReadWrite, 2),
        ('<', Some('&')) => (RedirectOp::Dup, 2),
        _ => (RedirectOp::Read, 1),
    };
    i += n;
    while matches!(chars.get(i), Some(' ' | '\t')) {
        i += 1;
    }
    *pending = Some(Pending::Redirect(fd_default, op));
    i
}

pub fn base(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

/// Command basename folded the way macOS/APFS folds names (lowercase, which
/// maps K U+212A to k, plus ſ U+017F to s). Borrows when already folded.
pub(crate) fn cmd_name(s: &str) -> std::borrow::Cow<'_, str> {
    let b = base(s);
    if b.bytes().all(|c| c.is_ascii() && !c.is_ascii_uppercase()) {
        std::borrow::Cow::Borrowed(b)
    } else {
        std::borrow::Cow::Owned(b.to_lowercase().replace('ſ', "s"))
    }
}

/// The options a wrapper takes (round 6 item 5): short letters that are
/// flags, that take a value (the rest of the word, or the next word), or
/// that take an optional value only when attached (`xargs -i{}`); long
/// options that are flags or take a value (`--opt v`, `--opt=v`); and
/// whether a bare number is an option (`nice -10`).
struct Opts {
    flags: &'static str,
    vals: &'static str,
    /// Short options whose value is optional (`xargs -i[R]`): only an
    /// attached value is theirs.
    opt_vals: &'static str,
    /// Short options whose value is optional in one implementation and
    /// needed in another (util-linux `script -t[file]`, BSD `script -t
    /// time`): a separate word is read both ways (round 7 item 6).
    either_vals: &'static str,
    long_flags: &'static [&'static str],
    long_vals: &'static [&'static str],
    /// Long options whose value is optional (`xargs --replace[=R]`): only a
    /// value after `=` is theirs.
    long_opt_vals: &'static [&'static str],
    /// Long spellings of `either_vals` (`script --timing[=file]`): a
    /// separate word is read both ways.
    long_either_vals: &'static [&'static str],
    numeric: bool,
}

const NO_OPTS: Opts = Opts {
    flags: "",
    vals: "",
    opt_vals: "",
    either_vals: "",
    long_flags: &["--help", "--version"],
    long_vals: &[],
    long_opt_vals: &[],
    long_either_vals: &[],
    numeric: false,
};

/// The option table of a wrapper `unwrap` strips, by name.
fn wrapper_opts(name: &str) -> Option<Opts> {
    let o = |flags, vals, long_flags, long_vals| Opts {
        flags,
        vals,
        opt_vals: "",
        either_vals: "",
        long_flags,
        long_vals,
        long_opt_vals: &[],
        long_either_vals: &[],
        numeric: false,
    };
    Some(match name {
        "nohup" | "builtin" | "busybox" | "toybox" => NO_OPTS,
        // bash `time -p`; GNU `time -f FMT -o FILE -a -q -v`; BSD `-l -h`
        "time" => o(
            "apqvVlh",
            "fo",
            &[
                "--append",
                "--portability",
                "--quiet",
                "--verbose",
                "--help",
                "--version",
            ],
            &["--format", "--output"],
        ),
        "command" => o("pvV", "", &[], &[]),
        "setsid" => o(
            "cfwhV",
            "",
            &["--ctty", "--fork", "--wait", "--help", "--version"],
            &[],
        ),
        "unbuffer" => o("p", "", &[], &[]),
        "caffeinate" => o("dimsu", "tw", &[], &[]),
        "sandbox-exec" => o("", "fnpD", &[], &[]),
        "chroot" => o(
            "n",
            "Ggu",
            &["--skip-chdir", "--help", "--version"],
            &["--userspec", "--groups"],
        ),
        "exec" => o("cl", "a", &[], &[]),
        "nice" => Opts {
            numeric: true,
            ..o("", "n", &["--help", "--version"], &["--adjustment"])
        },
        "ionice" => o(
            "thV",
            "cnpPu",
            &["--ignore", "--help", "--version"],
            &["--class", "--classdata", "--pid", "--pgid", "--uid"],
        ),
        "stdbuf" => o(
            "",
            "ioe",
            &["--help", "--version"],
            &["--input", "--output", "--error"],
        ),
        "timeout" => o(
            "fpv",
            "sk",
            &[
                "--foreground",
                "--preserve-status",
                "--verbose",
                "--help",
                "--version",
            ],
            &["--signal", "--kill-after"],
        ),
        // GNU `-i[R]`, `-e[E]`, `-l[N]` (optional), BSD `-I R`, `-E E`, `-L
        // N`: a separate word is read both ways (round 8 blocker 5)
        "xargs" => Opts {
            either_vals: "iel",
            long_opt_vals: &["--replace", "--eof", "--max-lines"],
            ..o(
                "0oprtx",
                "nPILsdaEJRS",
                &[
                    "--null",
                    "--interactive",
                    "--no-run-if-empty",
                    "--verbose",
                    "--exit",
                    "--open-tty",
                    "--show-limits",
                    "--help",
                    "--version",
                ],
                &[
                    "--max-args",
                    "--max-procs",
                    "--max-chars",
                    "--delimiter",
                    "--arg-file",
                    "--process-slot-var",
                ],
            )
        },
        "chrt" => o(
            "abdefimorRvhV",
            "TPD",
            &[
                "--all-tasks",
                "--batch",
                "--deadline",
                "--fifo",
                "--idle",
                "--other",
                "--rr",
                "--reset-on-fork",
                "--ext",
                "--max",
                "--verbose",
                "--help",
                "--version",
            ],
            &["--sched-runtime", "--sched-period", "--sched-deadline"],
        ),
        "taskset" => o(
            "achV",
            "",
            &["--all-tasks", "--cpu-list", "--help", "--version"],
            &[],
        ),
        "flock" => o(
            "sexnuoFhV",
            "wE",
            &[
                "--shared",
                "--exclusive",
                "--nonblock",
                "--nb",
                "--unlock",
                "--close",
                "--no-fork",
                "--verbose",
                "--help",
                "--version",
            ],
            &["--timeout", "--wait", "--conflict-exit-code"],
        ),
        // util-linux and BSD `script` (`c` is read by the caller):
        // util-linux `-t[file]`/`--timing[=file]`, BSD `-t time`
        "script" => Opts {
            either_vals: "t",
            long_either_vals: &["--timing"],
            ..o(
                "adeFfkpqrwhV",
                "EITOBmo",
                &[
                    "--append",
                    "--return",
                    "--flush",
                    "--force",
                    "--quiet",
                    "--help",
                    "--version",
                ],
                &[
                    "--log-io",
                    "--log-in",
                    "--log-out",
                    "--log-timing",
                    "--echo",
                    "--logging-format",
                    "--output-limit",
                ],
            )
        },
        _ => return None,
    })
}

/// One option word read with a table: how many words it uses (1 or 2), or
/// `None` when the table doesn't know it. `shell_c`: the letter whose value
/// is a shell string (`flock -c`, `script -c`), returned when met.
enum Opt {
    Words(usize),
    Shell(Option<String>),
    Unknown,
    /// A value that is optional in one implementation and needed in
    /// another, given as the next word: 1 or 2 words (round 7 item 6).
    Either,
    /// A long flag written with `=value` (`taskset --cpu-list=0-3`), or
    /// short flags with something after them that isn't a flag (`taskset
    /// -c0-3`): `getopt` rejects it, so what runs isn't known; read as the
    /// flag alone, and as the flag followed by the rest as its own word (the
    /// rest starts at this byte of the word).
    Split(usize),
}

fn read_opt(a: &[String], j: usize, o: &Opts, shell_c: bool) -> Opt {
    let w = a[j].as_str();
    if let Some(long) = w.strip_prefix("--") {
        let (name, has_val) = match long.split_once('=') {
            Some((n, _)) => (n, true),
            None => (long, false),
        };
        // GNU `getopt_long` takes any unambiguous prefix (`--sig`,
        // `--adj`, `--comm`: round 8 blocker 5)
        let name = long_name(&format!("--{name}"), o, shell_c);
        if shell_c && name == "--command" {
            return Opt::Shell(match w.split_once('=') {
                Some((_, v)) => Some(v.to_string()),
                None => a.get(j + 1).cloned(),
            });
        }
        if o.long_vals.contains(&name.as_str()) {
            return Opt::Words(if has_val { 1 } else { 2 });
        }
        if o.long_opt_vals.contains(&name.as_str()) {
            return Opt::Words(1);
        }
        if o.long_either_vals.contains(&name.as_str()) {
            return if has_val { Opt::Words(1) } else { Opt::Either };
        }
        if o.long_flags.contains(&name.as_str()) {
            return match w.find('=') {
                Some(at) => Opt::Split(at),
                None => Opt::Words(1),
            };
        }
        return Opt::Unknown;
    }
    let cs: Vec<char> = w[1..].chars().collect();
    if o.numeric && cs.iter().all(|c| c.is_ascii_digit()) {
        return Opt::Words(1);
    }
    for (k, c) in cs.iter().enumerate() {
        let rest: String = cs[k + 1..].iter().collect();
        if shell_c && *c == 'c' {
            return Opt::Shell(if rest.is_empty() {
                a.get(j + 1).cloned()
            } else {
                Some(rest)
            });
        }
        if o.vals.contains(*c) {
            return Opt::Words(if rest.is_empty() { 2 } else { 1 });
        }
        if o.either_vals.contains(*c) {
            return if rest.is_empty() {
                Opt::Either
            } else {
                Opt::Words(1)
            };
        }
        if o.opt_vals.contains(*c) {
            return Opt::Words(1);
        }
        if !o.flags.contains(*c) {
            return if k == 0 {
                Opt::Unknown
            } else {
                Opt::Split(1 + cs[..k].iter().map(|c| c.len_utf8()).sum::<usize>())
            };
        }
    }
    Opt::Words(1)
}

/// A long option's full name in a table, from the name as written or an
/// unambiguous prefix of one; as written otherwise.
fn long_name(written: &str, o: &Opts, shell_c: bool) -> String {
    let all = o
        .long_flags
        .iter()
        .chain(o.long_vals)
        .chain(o.long_opt_vals)
        .chain(o.long_either_vals)
        .chain(shell_c.then_some(&"--command"));
    let mut found: Option<&str> = None;
    for n in all {
        if *n == written {
            return written.to_string();
        }
        if written.len() > 3 && n.starts_with(written) {
            if found.is_some_and(|f| f != *n) {
                return written.to_string();
            }
            found = Some(n);
        }
    }
    found.map_or_else(|| written.to_string(), str::to_string)
}

fn is_option(w: &str) -> bool {
    w.starts_with('-') && w.len() > 1
}

/// Strips reserved words and transparent wrappers off the front of `argv`.
pub fn unwrap(argv: &[String]) -> Vec<String> {
    unwrap_checked(argv).0
}

/// [`unwrap`], and whether a wrapper had an option its table doesn't know
/// (round 6 item 5) or one that can be read more than one way (round 7 item
/// 6): the command it runs is then not known for sure, and the line must
/// not be allowed by a rule (it is read as if the option were a flag, as
/// before).
pub fn unwrap_checked(argv: &[String]) -> (Vec<String>, bool) {
    let mut cache = LexCache::default();
    unwrap_checked_at(argv, 0, &mut cache)
}

fn unwrap_checked_at(argv: &[String], depth: u8, cache: &mut LexCache) -> (Vec<String>, bool) {
    let (eff, unknown, choices) = unwrap_reading(argv, 0, Some(depth), Some(cache));
    (eff, unknown || choices > 0)
}

/// Whether the command runs text in this shell (`eval`, `source`, `.`), in
/// any reading of its wrappers, also when a substitution names it.
fn runs_here(eff: &[String], depth: u8, cache: &mut LexCache) -> bool {
    let here = |e: &[String]| {
        e.first()
            .is_some_and(|f| matches!(cmd_name(f).as_ref(), "eval" | "source" | "."))
    };
    here(eff)
        || (eff.iter().any(|w| has_command_substitution(w))
            && unwrap_readings(eff, depth, cache).iter().any(|e| here(e)))
}

/// [`runs_here`] for any reading of a unit's wrappers.
fn runs_here_any(u: &Unit, depth: u8, cache: &mut LexCache) -> bool {
    u.readings().any(|e| runs_here(e, depth, cache))
}

/// Whether one reading of a command reads a script on stdin: a shell (or a
/// word that may name one), `eval`/`source`/`.`, or a stdin script reader.
fn stdin_code_eff(e: &[String], depth: u8, cache: &mut LexCache) -> bool {
    e.first().is_some_and(|f| maybe_shell(f))
        || runs_here(e, depth, cache)
        || reads_stdin_as_script(e)
}

/// Whether one reading of a command runs what is piped into it as code.
fn pipe_code_eff(e: &[String], depth: u8, cache: &mut LexCache) -> bool {
    e.first().is_some_and(|f| {
        is_shell(f)
            || matches!(cmd_name(f).as_ref(), "eval" | "source" | ".")
            // `| s$(echo h)`, `| $(echo sh)`: a word built by running
            // something may name a shell (a variable alone, `| $PAGER`,
            // is read only with `-s` or `-`)
            || (has_command_substitution(f)
                && substituted_program_at(f, depth, cache).is_none_or(|n| {
                    let mut e = e.to_vec();
                    e[0] = n;
                    // the wrappers in front of it too (round 7 item 5)
                    let (e, unknown) = unwrap_checked(&e);
                    unknown
                        || e.first().is_some_and(|f| {
                            maybe_shell(f) || matches!(cmd_name(f).as_ref(), "eval" | "source" | ".")
                        })
                        || reads_stdin_as_script(&e)
                }))
            || (maybe_shell(f) && e[1..].iter().any(|a| a == "-s" || a == "-"))
    }) || reads_stdin_as_script(e)
        || runs_here(e, depth, cache)
        // `| sh -c bash`, `| script -c bash f`, `| $(which script) -c
        // bash f` (round 8 blocker 3)
        || unwrap_readings(e, depth, cache).iter().any(|r| {
            is_shell(r.first().map_or("", |f| f.as_str()))
                && !shell_c_codes(r).is_empty()
                && super::deny::reads_stdin_as_code(r)
        })
}

/// The text a command runs as code, from every reading of its wrappers
/// (`effs`) and every step of unwrapping them (M1-18, K1 and K4): a
/// shell's `-c` string, `eval`'s words, `env -S`/`env -C` (also when a
/// substitution names `env`: `$(which env) -C ..`), the output of a `$(…)`
/// run as code or as the command word, and a process substitution a shell
/// or `source` runs (`bash <(…)`, `bash < <(…)`).
fn code_texts_bounded(
    argv: &[String],
    effs: &[Vec<String>],
    targets: &[String],
    depth: u8,
    cache: &mut LexCache,
    piped: bool,
    unbounded_words: &mut Vec<UnboundedUnknown>,
) -> Vec<String> {
    let mut payloads: Vec<String> = vec![];
    let add = |v: Vec<String>, out: &mut Vec<String>| {
        for p in v {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    };
    for e in effs {
        let (ps, unresolvable) = payloads_of_checked(e);
        add(ps, &mut payloads);
        if unresolvable {
            let entry = UnboundedUnknown {
                words: e.clone(),
                piped,
            };
            if !unbounded_words.contains(&entry) {
                unbounded_words.push(entry);
            }
        }
    }
    add(
        env_payloads_bounded(argv, depth, cache, piped, unbounded_words),
        &mut payloads,
    );
    for step in unwrap_steps(argv, depth, cache) {
        if step.first().is_some_and(|f| cmd_name(f) == "env") {
            add(
                env_payloads_bounded(&step, depth, cache, piped, unbounded_words),
                &mut payloads,
            );
        }
    }
    // the output of a `$(…)` that is run as code: `sh -c "$(echo …)"`,
    // `eval $(…)`, `$(…)` as the command word
    let mut code_subs: Vec<String> = payloads
        .iter()
        .flat_map(|p| find_substitutions(p))
        .collect();
    // the command word, also behind a wrapper (`env $(…)`, `nohup $(…)`:
    // round 5 blocker 7), in every reading
    for w0 in argv
        .first()
        .into_iter()
        .chain(effs.iter().filter_map(|e| e.first()))
    {
        if has_command_substitution(w0) {
            code_subs.extend(find_substitutions(w0));
        }
    }
    // `bash < <(echo …)`, `source <(…)`, `bash <(…)`: a process
    // substitution a shell runs (part 1's finding 9), in any reading
    if effs.iter().any(|e| {
        e.first()
            .is_some_and(|f| maybe_shell(f) || matches!(cmd_name(f).as_ref(), "source" | "."))
    }) {
        for w in argv.iter().skip(1).chain(targets) {
            if let Some(b) = w.strip_prefix("<(").and_then(|r| r.strip_suffix(')')) {
                code_subs.push(b.to_string());
            }
        }
    }
    let mut seen: Vec<&String> = vec![];
    for s in &code_subs {
        if seen.contains(&s) {
            continue;
        }
        seen.push(s);
        add(output_texts(s, depth, cache), &mut payloads);
    }
    payloads
}

/// Where a shell's options end, read two ways (M1-18, K3): as before (every
/// option one word) and the way the shell reads them, where each `o` or `O`
/// in a cluster (`-o NAME`, `+O NAME`, `-eo pipefail`) and `--rcfile`,
/// `--init-file` and `--emulate` take the next word. For each reading:
/// whether `-c` (or `--command`) was given, and the index of the first
/// operand.
fn shell_operand_starts(eff: &[String]) -> Vec<(bool, usize)> {
    let mut out = vec![];
    for valued in [false, true] {
        let mut seen_c = false;
        let mut j = 1;
        while j < eff.len() {
            let a = eff[j].as_str();
            if a == "--" {
                j += 1;
                break;
            }
            if a == "--command" {
                seen_c = true;
                j += 1;
                continue;
            }
            if a.starts_with("--") {
                j += if valued && matches!(a, "--rcfile" | "--init-file" | "--emulate") {
                    2
                } else {
                    1
                };
                continue;
            }
            if (a.starts_with('-') || a.starts_with('+')) && a.len() > 1 {
                let letters = &a[1..];
                if a.starts_with('-') {
                    seen_c |= letters.contains('c');
                }
                j += 1;
                if valued {
                    j += letters.chars().filter(|c| matches!(c, 'o' | 'O')).count();
                }
                continue;
            }
            break;
        }
        if !out.contains(&(seen_c, j)) {
            out.push((seen_c, j));
        }
    }
    out
}

/// The code a shell is given with `-c` (`sh -c CODE`, `bash -ec CODE`,
/// `bash -o errexit -c CODE`), under every reading of its options.
#[derive(Clone, Copy)]
enum PosRef {
    Index(usize),
    All,
}

enum PosResolve {
    Plain,
    Resolved(String),
    Unresolved(Option<String>),
}

/// Parses a `sh -c` script that consists solely of positional-parameter
/// expansions (`$0`, `"$1"`, `${0}`, `$@`, `"$*"`, etc.). Returns `None` for
/// ordinary scripts or named variables, `Some(Err(()))` for unresolvable
/// positional/special-parameter forms (`${1:-…}`, `$10`, `$#`, `$0foo`).
fn parse_pos_script(code: &str) -> Option<Result<Vec<(PosRef, bool)>, ()>> {
    let s = code.trim();
    if s.is_empty() || !s.contains('$') {
        return None;
    }
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    let mut items: Vec<(PosRef, bool)> = vec![];

    let parse_dollar = |pos: &mut usize, has_items: bool| -> Result<PosRef, Option<()>> {
        *pos += 1;
        if *pos >= n {
            return Err(if has_items { Some(()) } else { None });
        }
        if bytes[*pos] == b'{' {
            let start = *pos + 1;
            let Some(rel_end) = s[start..].find('}') else {
                return Err(if has_items { Some(()) } else { None });
            };
            let end = start + rel_end;
            let inner = &s[start..end];
            *pos = end + 1;
            if inner.is_empty() {
                return Err(if has_items { Some(()) } else { None });
            }
            if inner.bytes().all(|b| b.is_ascii_digit()) {
                return inner
                    .parse::<usize>()
                    .map(PosRef::Index)
                    .map_err(|_| Some(()));
            }
            if inner == "@" || inner == "*" {
                return Ok(PosRef::All);
            }
            if inner.as_bytes().first().is_some_and(|b| {
                b.is_ascii_digit() || matches!(b, b'@' | b'*' | b'#' | b'?' | b'-' | b'!' | b'$')
            }) {
                return Err(Some(()));
            }
            return Err(if has_items { Some(()) } else { None });
        }
        if bytes[*pos].is_ascii_digit() {
            if *pos + 1 < n && bytes[*pos + 1].is_ascii_digit() {
                return Err(Some(()));
            }
            let idx = (bytes[*pos] - b'0') as usize;
            *pos += 1;
            return Ok(PosRef::Index(idx));
        }
        if matches!(bytes[*pos], b'@' | b'*') {
            *pos += 1;
            return Ok(PosRef::All);
        }
        if matches!(bytes[*pos], b'#' | b'?' | b'-' | b'!' | b'$') {
            return Err(Some(()));
        }
        Err(if has_items { Some(()) } else { None })
    };

    while i < n {
        while i < n && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= n {
            break;
        }
        if bytes[i] == b'"' {
            i += 1;
            let mut in_quote_items = false;
            loop {
                while i < n && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                if i >= n {
                    return if !items.is_empty() {
                        Some(Err(()))
                    } else {
                        None
                    };
                }
                if bytes[i] == b'"' {
                    if !in_quote_items {
                        return None;
                    }
                    i += 1;
                    break;
                }
                if bytes[i] != b'$' {
                    return None;
                }
                match parse_dollar(&mut i, !items.is_empty()) {
                    Ok(r) => {
                        items.push((r, true));
                        in_quote_items = true;
                    }
                    Err(Some(())) => return Some(Err(())),
                    Err(None) => return None,
                }
                if i < n && bytes[i] != b'"' && !bytes[i].is_ascii_whitespace() {
                    return Some(Err(()));
                }
            }
            if i < n && !bytes[i].is_ascii_whitespace() {
                return if matches!(bytes[i], b';' | b'|' | b'&' | b'<' | b'>') {
                    None
                } else {
                    Some(Err(()))
                };
            }
        } else if bytes[i] == b'$' {
            match parse_dollar(&mut i, !items.is_empty()) {
                Ok(r) => items.push((r, false)),
                Err(Some(())) => return Some(Err(())),
                Err(None) => return None,
            }
            if i < n && !bytes[i].is_ascii_whitespace() {
                return if matches!(bytes[i], b';' | b'|' | b'&' | b'<' | b'>') {
                    None
                } else {
                    Some(Err(()))
                };
            }
        } else {
            return None;
        }
    }

    if items.is_empty() {
        None
    } else {
        Some(Ok(items))
    }
}

fn resolve_positional_script(code: &str, pos: &[String]) -> PosResolve {
    let items = match parse_pos_script(code) {
        None => return PosResolve::Plain,
        Some(Err(())) => return PosResolve::Unresolved(None),
        Some(Ok(v)) => v,
    };

    if let [(r, quoted)] = items.as_slice() {
        return match r {
            PosRef::Index(idx) => {
                let Some(val) = pos.get(*idx) else {
                    return PosResolve::Unresolved(None);
                };
                if val.contains(['$', '`']) {
                    PosResolve::Unresolved(Some(val.clone()))
                } else {
                    PosResolve::Resolved(val.clone())
                }
            }
            PosRef::All => {
                if pos.len() <= 1 {
                    return PosResolve::Unresolved(None);
                }
                let args = &pos[1..];
                let has_var = args.iter().any(|w| w.contains(['$', '`']));
                let resolved = if args.len() == 1 {
                    args[0].clone()
                } else if *quoted {
                    args.iter().map(|w| quote(w)).collect::<Vec<_>>().join(" ")
                } else {
                    let parts: Vec<String> = args
                        .iter()
                        .flat_map(|w| w.split_ascii_whitespace())
                        .map(quote)
                        .collect();
                    if parts.is_empty() {
                        return PosResolve::Unresolved(None);
                    }
                    parts.join(" ")
                };
                if has_var {
                    PosResolve::Unresolved(Some(resolved))
                } else {
                    PosResolve::Resolved(resolved)
                }
            }
        };
    }

    let mut words: Vec<String> = vec![];
    let mut has_var = false;
    for (r, quoted) in items {
        match r {
            PosRef::Index(idx) => {
                let Some(val) = pos.get(idx) else {
                    return PosResolve::Unresolved(None);
                };
                has_var |= val.contains(['$', '`']);
                if quoted {
                    words.push(quote(val));
                } else {
                    words.extend(val.split_ascii_whitespace().map(quote));
                }
            }
            PosRef::All => {
                if pos.len() > 1 {
                    for val in &pos[1..] {
                        has_var |= val.contains(['$', '`']);
                        if quoted {
                            words.push(quote(val));
                        } else {
                            words.extend(val.split_ascii_whitespace().map(quote));
                        }
                    }
                }
            }
        }
    }
    if words.is_empty() {
        return PosResolve::Unresolved(None);
    }
    let resolved = words.join(" ");
    if has_var {
        PosResolve::Unresolved(Some(resolved))
    } else {
        PosResolve::Resolved(resolved)
    }
}

fn shell_c_codes_checked(eff: &[String]) -> (Vec<String>, bool) {
    let mut out = vec![];
    let mut unresolvable = false;
    for (seen_c, j) in shell_operand_starts(eff) {
        if seen_c && let Some(code) = eff.get(j) {
            let pos = eff.get(j + 1..).unwrap_or(&[]);
            let resolved = match resolve_positional_script(code, pos) {
                PosResolve::Plain => Some(code.clone()),
                PosResolve::Resolved(s) => Some(s),
                PosResolve::Unresolved(opt) => {
                    unresolvable = true;
                    opt.or_else(|| Some(code.clone()))
                }
            };
            if let Some(c) = resolved
                && !out.contains(&c)
            {
                out.push(c);
            }
        }
    }
    (out, unresolvable)
}

pub fn shell_c_codes(eff: &[String]) -> Vec<String> {
    shell_c_codes_checked(eff).0
}

/// Every way the wrappers in front of `argv` can be read (round 7 item 6):
/// the first is [`unwrap_checked`]'s; the others differ in an option whose
/// value may or may not be the next word. At most 16.
///
/// A command word built by a substitution that names a program plainly is
/// read as that program, at every step (`$(which env) $(which sudo) id`:
/// round 8 blocker 4); `depth` bounds the substitutions read.
const MAX_UNKNOWN_STARTS: usize = 24;
const MAX_UNKNOWN_EMIT: usize = 32;

fn unwrap_readings(argv: &[String], depth: u8, cache: &mut LexCache) -> Vec<Vec<String>> {
    let mut unused = vec![];
    unwrap_readings_bounded(argv, depth, cache, false, &mut unused)
}

fn unwrap_readings_bounded(
    argv: &[String],
    depth: u8,
    cache: &mut LexCache,
    piped: bool,
    unbounded_words: &mut Vec<UnboundedUnknown>,
) -> Vec<Vec<String>> {
    let mut invs: Vec<(Vec<String>, bool)> = vec![];
    let mut unbounded = false;
    let (first, _, mut choices) = unwrap_reading_steps(
        argv,
        0,
        Some(depth),
        None,
        Some(cache),
        Some(&mut invs),
        &mut unbounded,
    );
    let mut out = vec![first];
    let mut mask = 1u32;
    while mask < (1u32 << choices.min(4)) {
        let (e, _, cur_choices) = unwrap_reading_steps(
            argv,
            mask,
            Some(depth),
            None,
            Some(cache),
            Some(&mut invs),
            &mut unbounded,
        );
        choices = choices.max(cur_choices);
        if !out.contains(&e) {
            out.push(e);
        }
        mask += 1;
    }
    if choices > 4 {
        let (e, _, _) = unwrap_reading_steps(
            argv,
            u32::MAX,
            Some(depth),
            None,
            Some(cache),
            Some(&mut invs),
            &mut unbounded,
        );
        if !out.contains(&e) {
            out.push(e);
        }
        if invs.is_empty() {
            unbounded = true;
        }
    }
    let mut any_pushed = false;
    for (inv, is_env) in invs {
        let mut inv_unbounded = false;
        expand_unknown_env_cmd(
            &inv,
            is_env,
            depth,
            cache,
            Some(&mut out),
            None,
            &mut inv_unbounded,
            0,
        );
        if inv_unbounded {
            any_pushed = true;
            let entry = UnboundedUnknown { words: inv, piped };
            if !unbounded_words.contains(&entry) {
                unbounded_words.push(entry);
            }
        }
    }
    if unbounded && !any_pushed {
        let entry = UnboundedUnknown {
            words: argv.to_vec(),
            piped,
        };
        if !unbounded_words.contains(&entry) {
            unbounded_words.push(entry);
        }
    }
    out
}

/// One option word read for one reading: how many words it uses, or the
/// shell string of `-c`. `mask` picks, bit by bit, the reading of each
/// option that can be read two ways (`choices` counts them).
fn take_opt(
    a: &mut Vec<String>,
    j: usize,
    o: &Opts,
    shell_c: bool,
    mask: u32,
    choices: &mut u32,
    unknown: &mut bool,
) -> Opt {
    let mut pick = || {
        let bit = *choices < 32 && (mask >> *choices) & 1 == 1;
        *choices += 1;
        bit
    };
    // A one-letter long prefix (`script --c`, `flock --c`): GNU
    // `getopt_long` takes it for `--command` when it is unique, and refuses
    // it when it isn't. Read both ways: as `--command`, and as an option the
    // table doesn't know (M1-18, K3).
    if shell_c
        && let Some(long) = a[j].strip_prefix("--")
        && let name = long.split_once('=').map_or(long, |(n, _)| n)
        && name.len() == 1
        && "command".starts_with(name)
    {
        *unknown = true;
        if pick() {
            return Opt::Shell(match long.split_once('=') {
                Some((_, v)) => Some(v.to_string()),
                None => a.get(j + 1).cloned(),
            });
        }
    }
    match read_opt(a, j, o, shell_c) {
        // an option the table doesn't know may take the next word as its
        // value or not: both (round 8 blocker 5)
        Opt::Unknown => {
            *unknown = true;
            let separate = !a[j].contains('=') && j + 1 < a.len();
            Opt::Words(if separate && pick() { 2 } else { 1 })
        }
        Opt::Either => Opt::Words(if pick() { 2 } else { 1 }),
        Opt::Split(at) => {
            *unknown = true;
            if pick() {
                let w = a[j].clone();
                let (head, rest) = w.split_at(at);
                let rest = rest.strip_prefix('=').unwrap_or(rest);
                a.splice(j..=j, [head.to_string(), rest.to_string()]);
            }
            Opt::Words(1)
        }
        r => r,
    }
}

/// Every intermediate argv of every reading of the wrappers in front of
/// `argv` (substituted command words resolved): `env -i $(which env) -C ..
/// cat x` passes through `env -C .. cat x` (M1-18, K1).
fn unwrap_steps(argv: &[String], depth: u8, cache: &mut LexCache) -> Vec<Vec<String>> {
    let mut steps: Vec<Vec<String>> = vec![];
    let mut invs: Vec<(Vec<String>, bool)> = vec![];
    let mut unbounded = false;
    let mut choices = 0u32;
    let mut mask = 0u32;
    loop {
        let (_, _, cur_choices) = unwrap_reading_steps(
            argv,
            mask,
            Some(depth),
            Some(&mut steps),
            Some(cache),
            Some(&mut invs),
            &mut unbounded,
        );
        choices = choices.max(cur_choices);
        mask += 1;
        if mask >= (1u32 << choices.min(4)) {
            break;
        }
    }
    if choices > 4 {
        unwrap_reading_steps(
            argv,
            u32::MAX,
            Some(depth),
            Some(&mut steps),
            Some(cache),
            Some(&mut invs),
            &mut unbounded,
        );
    }
    let mut extra_readings: Vec<Vec<String>> = vec![];
    for (inv, is_env) in invs {
        expand_unknown_env_cmd(
            &inv,
            is_env,
            depth,
            cache,
            Some(&mut extra_readings),
            None,
            &mut unbounded,
            0,
        );
    }
    for r in extra_readings {
        if steps.len() < 64 && !steps.contains(&r) {
            steps.push(r);
        }
    }
    steps
}

fn unwrap_reading(
    argv: &[String],
    mask: u32,
    resolve: Option<u8>,
    cache: Option<&mut LexCache>,
) -> (Vec<String>, bool, u32) {
    let mut unbounded = false;
    unwrap_reading_steps(argv, mask, resolve, None, cache, None, &mut unbounded)
}

fn unwrap_reading_steps(
    argv: &[String],
    mask: u32,
    resolve: Option<u8>,
    mut steps: Option<&mut Vec<Vec<String>>>,
    mut cache: Option<&mut LexCache>,
    mut unknown_invs: Option<&mut Vec<(Vec<String>, bool)>>,
    unbounded: &mut bool,
) -> (Vec<String>, bool, u32) {
    let mut local_cache = LexCache::default();
    let depth = resolve.unwrap_or(0);
    let mut a: Vec<String> = argv.to_vec();
    let mut unknown = false;
    let mut choices = 0u32;
    for _ in 0..12 {
        let Some(first) = a.first() else { break };
        if let Some(d) = resolve
            && has_command_substitution(first)
            && let Some(name) = match cache.as_deref_mut() {
                Some(c) => substituted_program_at(first, d, c),
                None => substituted_program_at(first, d, &mut local_cache),
            }
        {
            a[0] = name;
            continue;
        }
        if let Some(st) = steps.as_deref_mut()
            && st.len() < 64
            && !st.contains(&a)
        {
            st.push(a.clone());
        }
        let is_reserved = matches!(
            base(first),
            "{" | "}" | "!" | "do" | "then" | "else" | "elif" | "if" | "while" | "until" | "coproc"
        );
        let b = cmd_name(first).into_owned();
        // options with a table, then `operands` words before the command
        let table = |a: &mut Vec<String>,
                     o: &Opts,
                     operands: usize,
                     choices: &mut u32,
                     unk: &mut bool|
         -> usize {
            let mut j = 1;
            while j < a.len() {
                let w = a[j].as_str();
                if w == "--" {
                    j += 1;
                    break;
                }
                if !is_option(w) {
                    break;
                }
                match take_opt(a, j, o, false, mask, choices, unk) {
                    Opt::Words(n) => j += n,
                    _ => j += 1,
                }
            }
            (j + operands).min(a.len())
        };
        let mut step_unknown = false;
        let orig_a = (!is_reserved
            && unknown_invs.is_some()
            && (b == "env" || b == "arch" || wrapper_opts(&b).is_some())
            && a[1..].iter().any(|w| is_option(w)))
        .then(|| a.clone());
        let n = if is_reserved {
            1
        } else {
            match b.as_str() {
                "time" | "nohup" | "builtin" | "command" | "setsid" | "unbuffer" | "caffeinate"
                | "sandbox-exec" | "exec" | "nice" | "ionice" | "stdbuf" | "xargs" | "busybox"
                | "toybox" => {
                    let o = wrapper_opts(&b).unwrap_or(NO_OPTS);
                    if matches!(b.as_str(), "busybox" | "toybox") {
                        1
                    } else {
                        table(&mut a, &o, 0, &mut choices, &mut step_unknown)
                    }
                }
                "timeout" | "chrt" | "taskset" => {
                    let o = wrapper_opts(&b).unwrap_or(NO_OPTS);
                    // `chrt -p` / `taskset -p` act on a process: no command
                    if b != "timeout"
                        && a[1..]
                            .iter()
                            .take_while(|w| is_option(w))
                            .any(|w| !w.starts_with("--") && w.contains('p') || w == "--pid")
                    {
                        0
                    } else {
                        table(&mut a, &o, 1, &mut choices, &mut step_unknown)
                    }
                }
                "arch" => {
                    // `arch [-32|-64|-x86_64|-arm64|…] [-c] [-e VAR=v] [-d VAR] CMD`
                    let mut j = 1;
                    while j < a.len() && is_option(&a[j]) {
                        match a[j].as_str() {
                            "-arch" | "-e" | "-d" => j += 2,
                            "-c" | "-h" | "-32" | "-64" | "-x86_64" | "-arm64" | "-arm64e"
                            | "-i386" | "-x86_64h" => j += 1,
                            _ => {
                                step_unknown = true;
                                j += 1;
                            }
                        }
                    }
                    j.min(a.len())
                }
                "chroot" => {
                    // `chroot [opts] NEWROOT [COMMAND…]`; without a command it
                    // starts a shell (see `reads_stdin_as_script`).
                    let o = wrapper_opts(&b).unwrap_or(NO_OPTS);
                    let j = table(&mut a, &o, 0, &mut choices, &mut step_unknown);
                    if j + 1 < a.len() { j + 1 } else { 0 }
                }
                "flock" | "script" => {
                    // `flock [opts] FILE COMMAND…` / `flock [opts] FILE -c 'string'`;
                    // macOS `script [opts] FILE [COMMAND…]`, Linux `script -c CMD
                    // [FILE]`. The wrapper's own options may come after FILE too
                    // (read as options: the stricter reading). Only the
                    // wrapper's own `-c` is a shell string; a `-c` after the
                    // command starts belongs to the command
                    // (`flock l git -c x=y push`, `script f sudo sh -c id`).
                    let o = wrapper_opts(&b).unwrap_or(NO_OPTS);
                    let mut j = 1;
                    let mut file_seen = false;
                    let mut shell_string: Option<String> = None;
                    while j < a.len() {
                        let w = a[j].as_str();
                        if w == "--" {
                            j += 1;
                            if !file_seen && j < a.len() {
                                file_seen = true;
                                j += 1;
                            }
                            break;
                        }
                        if is_option(w) {
                            match take_opt(
                                &mut a,
                                j,
                                &o,
                                true,
                                mask,
                                &mut choices,
                                &mut step_unknown,
                            ) {
                                Opt::Words(n) => j += n,
                                Opt::Shell(s) => {
                                    shell_string = s;
                                    break;
                                }
                                _ => j += 1,
                            }
                        } else if !file_seen {
                            file_seen = true;
                            // `script FILE CMD…`: read also as if the command
                            // started at FILE (round 7 item 6: whatever a
                            // `-t`/`--timing` before it took, the command is
                            // checked from the first word that may start it)
                            let bit = b == "script" && j + 1 < a.len() && {
                                let bit = choices < 32 && (mask >> choices) & 1 == 1;
                                choices += 1;
                                bit
                            };
                            if bit {
                                break;
                            }
                            j += 1;
                        } else {
                            break;
                        }
                    }
                    if let Some(cmd) = shell_string {
                        if step_unknown
                            && let Some(orig) = orig_a
                            && let Some(invs) = unknown_invs.as_deref_mut()
                        {
                            let entry = (orig, false);
                            if !invs.contains(&entry) {
                                invs.push(entry);
                            }
                        }
                        a = vec!["sh".into(), "-c".into(), cmd];
                        unknown |= step_unknown;
                        continue;
                    }
                    if file_seen && j < a.len() { j } else { 0 }
                }
                "env" => {
                    let c = cache.as_deref_mut().unwrap_or(&mut local_cache);
                    let scan = scan_env(&mut a, 1, mask, &mut choices, &mut step_unknown, depth, c);
                    *unbounded |= scan.unbounded;
                    scan.cmd_index.min(a.len())
                }
                _ => break,
            }
        };
        unknown |= step_unknown;
        if step_unknown
            && let Some(orig) = orig_a
            && let Some(invs) = unknown_invs.as_deref_mut()
        {
            let entry = (orig, b == "env");
            if !invs.contains(&entry) {
                invs.push(entry);
            }
        }
        if n == 0 {
            break;
        }
        if n >= a.len() {
            a.clear();
            break;
        }
        a.drain(..n);
    }
    (a, unknown, choices)
}

/// The flag letters of an `env` cluster before its first value letter are
/// all known (`-i`, `-0`, `-v`).
fn env_flags_ok(w: &str) -> bool {
    w[1..]
        .chars()
        .take_while(|c| !matches!(c, 'u' | 'C' | 'P' | 'a' | 'S'))
        .all(|c| matches!(c, 'i' | '0' | 'v'))
}

/// For a short-option cluster of `env` (`-iu`, `-iCproj`, `-S'…'`): the
/// first letter that takes a value (`u`, `C`, `P`, `a`, `S`) and the rest of
/// the word after it (its value, or empty when the value is the next word).
/// `None` when every letter is a flag (or the word isn't a cluster).
fn env_cluster(w: &str) -> Option<(char, String)> {
    let flags = w.strip_prefix('-')?;
    if flags.is_empty() || flags.starts_with('-') {
        return None;
    }
    let cs: Vec<char> = flags.chars().collect();
    let k = cs
        .iter()
        .position(|c| matches!(c, 'u' | 'C' | 'P' | 'a' | 'S'))?;
    Some((cs[k], cs[k + 1..].iter().collect()))
}

/// Splits a string into words the way a shell would (quotes, escapes);
/// assignments stay words. Used for `env -S`.
fn split_words(text: &str, depth: u8, cache: &mut LexCache) -> (Vec<String>, bool) {
    let l = lex_depth_cached(text, depth, cache);
    if depth > MAX_DEPTH {
        return (text.split_whitespace().map(str::to_string).collect(), true);
    }
    let words = match l.units.iter().find(|u| !u.derived) {
        Some(u) => {
            let mut w: Vec<String> = u
                .seg
                .assignments
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            w.extend(u.seg.argv.iter().cloned());
            w
        }
        None => text.split_whitespace().map(str::to_string).collect(),
    };
    (words, false)
}

fn is_assignment_word(w: &str) -> bool {
    w.split_once('=').is_some_and(|(n, _)| is_name(n))
}

const SHELLS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "ksh", "fish", "ash", "csh", "tcsh", "mksh", "yash", "rbash",
    "posh", "elvish", "xonsh",
];

/// A shell by name, or by the variables that hold one (`$SHELL`, `$BASH`).
pub fn is_shell(cmd: &str) -> bool {
    SHELLS.contains(&cmd_name(cmd).as_ref())
        || matches!(
            cmd,
            "$SHELL" | "${SHELL}" | "$BASH" | "${BASH}" | "$0" | "${0}"
        )
}

/// A shell by name, or a command word built at run time (`$SHELL`,
/// `s$(echo h)`, `` `which bash` ``, `s${E}h`), which may be one. Used where
/// text is handed to the command as a script (here-docs, `-c` payloads); a
/// pipe into `$PAGER` is not read as code unless it says `-s` or `-`.
pub fn maybe_shell(cmd: &str) -> bool {
    is_shell(cmd) || has_substitution_or_var(cmd)
}

fn has_substitution_or_var(w: &str) -> bool {
    w.contains('$') || w.contains('`')
}

/// Commands that run a command named in their arguments somewhere else (a
/// host, a container, a namespace) and hand it their stdin.
const COMMAND_RUNNERS: &[&str] = &[
    "ssh",
    "mosh",
    "autossh",
    "nsenter",
    "unshare",
    "docker",
    "podman",
    "nerdctl",
    "kubectl",
    "oc",
    "lxc",
    "incus",
    "machinectl",
    "systemd-run",
    "systemd-nspawn",
    "firejail",
    "bwrap",
    "flatpak-spawn",
    "toolbox",
    "distrobox",
    "distrobox-enter",
    "multipass",
    "vagrant",
    "limactl",
    "colima",
    "su",
    "runuser",
    "setpriv",
    "parallel",
];

/// A word holding a command substitution.
pub fn has_command_substitution(w: &str) -> bool {
    w.contains("$(") || w.contains('`')
}

/// The program a command word built by a command substitution names, when
/// it plainly names one (round 5, round 6 item 6): `$(brew --prefix)/bin/jq`
/// ends in a literal name, `$(which less)` (`command -v`, `type -p`,
/// `whence`) looks up a literal name, `$(echo node)` prints one word.
/// `None` when it may be anything (`s$(echo h)`, `$(x)/bin/$Y`, `$(x)/bin/s*`,
/// known output with a space that splits into more words): the caller then
/// reads it as a shell.
pub fn substituted_program(w: &str) -> Option<String> {
    let mut cache = LexCache::default();
    substituted_program_at(w, 0, &mut cache)
}

fn substituted_program_at(w: &str, depth: u8, cache: &mut LexCache) -> Option<String> {
    if !has_command_substitution(w) || depth >= MAX_DEPTH {
        return None;
    }
    let bodies = find_substitutions(w);
    let mut outputs: Vec<String> = vec![];
    for body in &bodies {
        for t in output_texts(body, depth, cache) {
            let t = t.trim().to_string();
            // output that splits into words picks the program itself
            if t.chars().any(char::is_whitespace) {
                return None;
            }
            outputs.push(t);
        }
    }
    let unquoted: String = w.chars().filter(|c| !matches!(c, '"' | '\'')).collect();
    let literal_name = |n: &str| {
        !n.is_empty()
            && n.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+' | '/'))
    };
    let last_of = |n: &str| n.rsplit('/').next().unwrap_or(n).to_string();
    // `$(brew --prefix)/bin/jq`: a literal program after the last `/`
    if let Some((_, last)) = unquoted.rsplit_once('/')
        && !last.contains('\\')
        && literal_name(last)
        && !unquoted.ends_with(')')
        && !unquoted.ends_with('`')
    {
        return Some(last.to_string());
    }
    let whole = unquoted
        .strip_prefix("$(")
        .and_then(|r| r.strip_suffix(')'))
        .or_else(|| unquoted.strip_prefix('`').and_then(|r| r.strip_suffix('`')));
    if let Some(body) = whole
        && bodies.len() == 1
    {
        // `$(which less)`, `$(command -v jq)`, `` `type -p jq` ``
        let words: Vec<&str> = body.split_whitespace().collect();
        let name = match words.as_slice() {
            [w0, n] if matches!(cmd_name(w0).as_ref(), "which" | "whence") => Some(*n),
            [w0, "-v" | "-V", n] if cmd_name(w0) == "command" => Some(*n),
            [w0, "-p" | "-P", n] if cmd_name(w0) == "type" => Some(*n),
            _ => None,
        };
        if let Some(n) = name {
            return literal_name(n).then(|| last_of(n));
        }
        // `$(echo node)`: one known word
        if let [one] = outputs.as_slice()
            && literal_name(one)
        {
            return Some(last_of(one));
        }
    }
    None
}

/// Commands that run their standard input as a script of commands without
/// being a shell: `at`, `batch`, `crontab -`, `make -f -`, `script`.
pub fn reads_stdin_as_script(eff: &[String]) -> bool {
    let Some(first) = eff.first() else {
        return false;
    };
    let args = &eff[1..];
    let stdin = |w: &str| matches!(w, "-" | "/dev/stdin" | "/dev/fd/0");
    match cmd_name(first).as_ref() {
        // with `-f FILE` the job comes from the file, but a body given on
        // stdin is still read as one (it costs nothing to check)
        "at" | "batch" => true,
        "script" => true,
        "chroot" => {
            // without a command, `chroot NEWROOT` starts a shell
            let mut operands = 0;
            let mut k = 0;
            while k < args.len() {
                let a = args[k].as_str();
                if matches!(a, "--userspec" | "--groups") {
                    k += 2;
                    continue;
                }
                if !a.starts_with('-') {
                    operands += 1;
                }
                k += 1;
            }
            operands <= 1
        }
        // `crontab [-u USER] [FILE]`: without a file (or with `-`) the
        // table comes from stdin; `-l`, `-r`, `-e` don't read one
        "crontab" => {
            let mut k = 0;
            let mut file = false;
            while k < args.len() {
                let a = args[k].as_str();
                if stdin(a) {
                    return true;
                }
                if let Some(flags) = a.strip_prefix('-') {
                    // letters up to `u`; after `u` the rest of the word (or
                    // the next word) is the user name (`-uroot`: round 5
                    // blocker 5)
                    let (before, user_next) = match flags.find('u') {
                        Some(p) => (&flags[..p], p + 1 == flags.len()),
                        None => (flags, false),
                    };
                    if before.contains(['l', 'r', 'e']) {
                        return false;
                    }
                    k += if user_next { 2 } else { 1 };
                    continue;
                }
                file = true;
                k += 1;
            }
            !file
        }
        "make" | "gmake" => args.iter().enumerate().any(|(i, a)| {
            let next_is_stdin = args.get(i + 1).is_some_and(|n| stdin(n));
            if let Some(v) = a
                .strip_prefix("--file=")
                .or_else(|| a.strip_prefix("--makefile="))
            {
                return stdin(v);
            }
            if matches!(a.as_str(), "--file" | "--makefile") {
                return next_is_stdin;
            }
            // `-f -`, `-f-`, `-sf -`, `-f/dev/stdin`
            if a.starts_with('-')
                && !a.starts_with("--")
                && let Some(k) = a.find('f')
            {
                let rest = &a[k + 1..];
                return if rest.is_empty() {
                    next_is_stdin
                } else {
                    stdin(rest)
                };
            }
            false
        }),
        _ => false,
    }
}

/// The commands of a crontab: each line without its five schedule fields
/// (or its `@reboot`-style word); variable lines and comments dropped.
fn crontab_commands(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let l = line.trim_start();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let skip = if l.starts_with('@') { 1 } else { 5 };
        let mut rest = l;
        for _ in 0..skip {
            rest = rest.trim_start();
            match rest.find(char::is_whitespace) {
                Some(k) => rest = &rest[k..],
                None => rest = "",
            }
        }
        let first = l.split_whitespace().next().unwrap_or("");
        if !l.starts_with('@') && is_assignment_word(first) {
            continue;
        }
        out.push_str(rest.trim_start());
        out.push('\n');
    }
    out
}

/// Widths above this are not padded (a `%999999999s` must not allocate).
const MAX_PAD: usize = 256;

/// Decodes one backslash escape at `cs[i]` (just after the `\`), the way
/// `echo -e`, zsh's and dash's `echo`, `printf` formats and `%b` do.
/// `octal0`: octal is `\0NNN` (echo, `%b`) rather than `\NNN` (a format).
/// Returns the index after the escape, or `None` for `\c` (output stops).
fn unescape_one(cs: &[char], i: usize, octal0: bool, out: &mut String) -> Option<usize> {
    let Some(&c) = cs.get(i) else {
        out.push('\\');
        return Some(i);
    };
    let hex = |from: usize, max: usize| -> (u32, usize) {
        let mut v = 0u32;
        let mut n = 0;
        while n < max
            && let Some(d) = cs.get(from + n).and_then(|c| c.to_digit(16))
        {
            v = v.saturating_mul(16).saturating_add(d);
            n += 1;
        }
        (v, n)
    };
    let octal = |from: usize, max: usize| -> (u32, usize) {
        let mut v = 0u32;
        let mut n = 0;
        while n < max
            && let Some(d) = cs.get(from + n).and_then(|c| c.to_digit(8))
        {
            v = v * 8 + d;
            n += 1;
        }
        (v, n)
    };
    let mut push_code = |v: u32| {
        if let Some(ch) = char::from_u32(v & 0x10ffff) {
            out.push(ch);
        }
    };
    let simple = match c {
        'a' => Some('\x07'),
        'b' => Some('\x08'),
        'e' | 'E' => Some('\x1b'),
        'f' => Some('\x0c'),
        'n' => Some('\n'),
        'r' => Some('\r'),
        't' => Some('\t'),
        'v' => Some('\x0b'),
        '\\' | '\'' | '"' | '?' => Some(c),
        _ => None,
    };
    if let Some(ch) = simple {
        out.push(ch);
        return Some(i + 1);
    }
    match c {
        'c' => None,
        'x' | 'u' | 'U' => {
            let max = match c {
                'x' => 2,
                'u' => 4,
                _ => 8,
            };
            let (v, n) = hex(i + 1, max);
            if n == 0 {
                out.push('\\');
                out.push(c);
            } else {
                push_code(v);
            }
            Some(i + 1 + n)
        }
        '0' if octal0 => {
            let (v, n) = octal(i + 1, 3);
            push_code(v);
            Some(i + 1 + n)
        }
        '0'..='7' if !octal0 => {
            let (v, n) = octal(i, 3);
            push_code(v);
            Some(i + n)
        }
        _ => {
            out.push('\\');
            out.push(c);
            Some(i + 1)
        }
    }
}

/// `s` with every escape decoded; `false` when a `\c` stopped the output.
fn unescape_to(s: &str, octal0: bool, out: &mut String) -> bool {
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        if cs[i] == '\\' {
            match unescape_one(&cs, i + 1, octal0, out) {
                Some(n) => i = n,
                None => return false,
            }
        } else {
            out.push(cs[i]);
            i += 1;
        }
    }
    true
}

/// What `printf FORMAT ARGS…` prints: the format is reused while arguments
/// remain; a conversion with no argument left prints nothing (or `0`);
/// width, precision (`*` takes an argument), `%c` and `%b` are applied.
fn format_printf(fmt: &str, args: &[String], b_octal0: bool) -> String {
    let cs: Vec<char> = fmt.chars().collect();
    let mut out = String::new();
    let mut idx = 0usize;
    let mut used_positional = false;
    loop {
        let before = idx;
        let mut i = 0;
        while i < cs.len() {
            let c = cs[i];
            if c == '\\' {
                match unescape_one(&cs, i + 1, false, &mut out) {
                    Some(n) => i = n,
                    None => return out,
                }
                continue;
            }
            if c != '%' {
                out.push(c);
                i += 1;
                continue;
            }
            i += 1;
            if cs.get(i) == Some(&'%') {
                out.push('%');
                i += 1;
                continue;
            }
            // `%2$s`: an argument named by number (zsh, ksh, BSD); the
            // format is then not reused (re-review blocker 5)
            let mut digits = 0;
            while cs.get(i + digits).is_some_and(|c| c.is_ascii_digit()) {
                digits += 1;
            }
            let positional: Option<usize> = (digits > 0 && cs.get(i + digits) == Some(&'$'))
                .then(|| cs[i..i + digits].iter().collect::<String>().parse().ok())
                .flatten();
            if positional.is_some() {
                i += digits + 1;
                used_positional = true;
            }
            let mut next_arg = || {
                if let Some(n) = positional {
                    return n.checked_sub(1).and_then(|k| args.get(k)).cloned();
                }
                let a = args.get(idx).cloned();
                if a.is_some() {
                    idx += 1;
                }
                a
            };
            let mut left = false;
            while let Some(&f) = cs.get(i) {
                if matches!(f, '-' | '+' | ' ' | '#' | '0' | '\'') {
                    left |= f == '-';
                    i += 1;
                } else {
                    break;
                }
            }
            let number = |i: &mut usize, next_arg: &mut dyn FnMut() -> Option<String>| {
                if cs.get(*i) == Some(&'*') {
                    *i += 1;
                    let v: i64 = next_arg().and_then(|a| a.trim().parse().ok()).unwrap_or(0);
                    return Some(v);
                }
                let mut v: Option<i64> = None;
                while let Some(d) = cs.get(*i).and_then(|c| c.to_digit(10)) {
                    v = Some(
                        v.unwrap_or(0)
                            .saturating_mul(10)
                            .saturating_add(i64::from(d)),
                    );
                    *i += 1;
                }
                v
            };
            let width = number(&mut i, &mut next_arg);
            let precision = if cs.get(i) == Some(&'.') {
                i += 1;
                Some(number(&mut i, &mut next_arg).unwrap_or(0))
            } else {
                None
            };
            while matches!(cs.get(i), Some('h' | 'l' | 'L' | 'q' | 'j' | 'z' | 't')) {
                i += 1;
            }
            let conv = cs.get(i).copied();
            i += 1;
            let arg = next_arg();
            let mut piece = String::new();
            let mut stop = false;
            match conv {
                Some('s') => piece.push_str(arg.as_deref().unwrap_or("")),
                Some('b') => {
                    stop = !unescape_to(arg.as_deref().unwrap_or(""), b_octal0, &mut piece);
                }
                Some('c') => piece.extend(arg.as_deref().unwrap_or("").chars().next()),
                Some(_) => piece.push_str(arg.as_deref().unwrap_or("0")),
                None => {}
            }
            if let (Some(p), Some('s' | 'b')) = (precision, conv) {
                piece = piece.chars().take(p.max(0) as usize).collect();
            }
            if let Some(w) = width {
                let (w, left) = (w.unsigned_abs() as usize, left || w < 0);
                let n = piece.chars().count();
                if w > n && w <= MAX_PAD {
                    let pad = " ".repeat(w - n);
                    piece = if left { piece + &pad } else { pad + &piece };
                }
            }
            out.push_str(&piece);
            if stop {
                return out;
            }
        }
        if idx == before || idx >= args.len() || used_positional {
            break;
        }
    }
    out
}

/// The text `echo`, `printf` or `yes` writes, in every reading a shell might
/// give it (escapes decoded or not, either octal form). Nothing for other
/// commands.
fn echo_printf_texts(eff: &[String], texts: &mut Vec<String>) {
    let Some(first) = eff.first() else {
        return;
    };
    match cmd_name(first).as_ref() {
        "echo" => {
            let mut j = 1;
            while j < eff.len() {
                let w = eff[j].as_str();
                if w.starts_with('-')
                    && w.len() > 1
                    && w[1..].chars().all(|c| matches!(c, 'n' | 'e' | 'E'))
                {
                    j += 1;
                } else {
                    break;
                }
            }
            if j < eff.len() {
                let joined = eff[j..].join(" ");
                for octal0 in [true, false] {
                    let mut unesc = String::new();
                    unescape_to(&joined, octal0, &mut unesc);
                    if unesc != joined {
                        texts.push(unesc);
                    }
                }
                texts.push(joined);
            }
        }
        "printf" => {
            let mut j = 1;
            while j < eff.len() {
                let w = eff[j].as_str();
                if w == "--" {
                    j += 1;
                    break;
                } else if w == "-v" {
                    j += 2;
                } else {
                    break;
                }
            }
            if j < eff.len() {
                let rest = &eff[j..];
                for b_octal0 in [true, false] {
                    texts.push(format_printf(&rest[0], &rest[1..], b_octal0));
                }
                if rest.len() > 1 {
                    texts.push(rest[1..].join(" "));
                }
            }
        }
        "yes" => {
            let t = if eff.len() > 1 {
                eff[1..].join(" ")
            } else {
                "y".to_string()
            };
            texts.push(t);
        }
        _ => {}
    }
}

/// What the commands in `body` write to stdout, as far as the engine can read
/// it (`echo`, `printf`, `yes`, here-docs and here-strings), for a `$(…)`
/// whose output is run as code (`sh -c "$(…)"`, `eval $(…)`, `$(…)` as the
/// command word).
fn output_texts(body: &str, depth: u8, cache: &mut LexCache) -> Vec<String> {
    let l = lex_depth_cached(body, depth + 1, cache);
    let mut texts = l.stdin_texts.clone();
    for u in &l.units {
        echo_printf_texts(&u.eff, &mut texts);
    }
    texts
}

/// The strings a shell or `eval` will run: a shell's `-c` string, fish's
/// `-C`/`--init-command`, `eval`'s arguments. Options that take a value
/// (`--rcfile F`, `--emulate sh`, `-o pipefail`) are skipped.
fn payloads_of_checked(eff: &[String]) -> (Vec<String>, bool) {
    let mut out = vec![];
    let mut unresolvable = false;
    let Some(f) = eff.first() else {
        return (out, false);
    };
    let first = cmd_name(f);
    if maybe_shell(f) {
        let mut has_c = false;
        let mut inline_cmds: Vec<String> = vec![];
        let mut j = 1;
        while j < eff.len() {
            let w = &eff[j];
            if w == "--" {
                j += 1;
                break;
            }
            if (w.starts_with('-') && !w.starts_with("--") && w[1..].contains('c'))
                || w == "--command"
            {
                has_c = true;
                j += 1;
                continue;
            }
            if let Some(rest) = w.strip_prefix("--command=") {
                inline_cmds.push(rest.to_string());
                j += 1;
                continue;
            }
            if let Some(rest) = w.strip_prefix("--init-command=") {
                out.push(rest.to_string());
                j += 1;
                continue;
            }
            if matches!(w.as_str(), "-C" | "--init-command") {
                out.extend(eff.get(j + 1).cloned());
                j += 2;
                continue;
            }
            if matches!(
                w.as_str(),
                "-o" | "+o" | "-O" | "+O" | "--rcfile" | "--init-file" | "--emulate"
            ) {
                j += 2;
                continue;
            }
            if w.starts_with('-') || w.starts_with('+') {
                j += 1;
                continue;
            }
            break;
        }
        let mut push_c = |code: &str, pos: &[String]| match resolve_positional_script(code, pos) {
            PosResolve::Plain => out.push(code.to_string()),
            PosResolve::Resolved(s) => out.push(s),
            PosResolve::Unresolved(opt) => {
                unresolvable = true;
                if let Some(s) = opt {
                    out.push(s);
                }
            }
        };
        for code in inline_cmds {
            push_c(&code, eff.get(j..).unwrap_or(&[]));
        }
        if has_c && let Some(code) = eff.get(j) {
            push_c(code, eff.get(j + 1..).unwrap_or(&[]));
        }
        // and as the shell reads its options (`-eo pipefail -c CODE`: K3)
        let (c_codes, c_unres) = shell_c_codes_checked(eff);
        unresolvable |= c_unres;
        for code in c_codes {
            if !out.contains(&code) {
                out.push(code);
            }
        }
    } else if first == "eval" {
        let args = if eff.get(1).map(String::as_str) == Some("--") {
            &eff[2..]
        } else {
            &eff[1..]
        };
        if !args.is_empty() {
            out.push(args.join(" "));
        }
    }
    (out, unresolvable)
}

/// `env -S 'TEXT'` and `env -C DIR CMD…`: the command line `env` runs, as
/// text (`cd DIR && CMD…` for `-C`), so its words are checked as paths too.
/// GNU `env`'s long options, by name or unambiguous prefix (`--chd` is
/// `--chdir`), keeping a `=value`; `None` for one it doesn't know.
fn env_long(w: &str) -> Option<String> {
    const LONGS: &[&str] = &[
        "--ignore-environment",
        "--null",
        "--unset",
        "--chdir",
        "--argv0",
        "--split-string",
        "--debug",
        "--block-signal",
        "--default-signal",
        "--ignore-signal",
        "--list-signal-handling",
        "--help",
        "--version",
    ];
    let (name, val) = match w.split_once('=') {
        Some((n, v)) => (n, Some(v)),
        None => (w, None),
    };
    if name.len() <= 2 {
        return None;
    }
    let full = if LONGS.contains(&name) {
        name
    } else {
        let mut hits = LONGS.iter().filter(|n| n.starts_with(name));
        let first = hits.next()?;
        if hits.next().is_some() {
            return None;
        }
        first
    };
    Some(match val {
        Some(v) => format!("{full}={v}"),
        None => full.to_string(),
    })
}

struct EnvScan {
    chdir: Option<String>,
    split_texts: Vec<String>,
    cmd_index: usize,
    unbounded: bool,
}

/// Scans `env` options starting at `start`, reading unknown long and short
/// options both as flags and as taking a value (controlled by `mask`/`choices`)
/// and splicing `-S`/`--split-string` words into `a` (M1-18, K1).
fn scan_env(
    a: &mut Vec<String>,
    start: usize,
    mask: u32,
    choices: &mut u32,
    unknown: &mut bool,
    depth: u8,
    cache: &mut LexCache,
) -> EnvScan {
    let mut pick = || {
        let bit = *choices < 32 && (mask >> *choices) & 1 == 1;
        *choices += 1;
        bit
    };
    let mut chdir: Option<String> = None;
    let mut split_texts: Vec<String> = vec![];
    let mut splices = 0usize;
    let mut unbounded = false;
    let mut j = start;
    while j < a.len() {
        // `--chd`, `--spl`, `--arg`: GNU prefixes (round 8 blocker 1)
        if a[j].starts_with("--")
            && let Some(full) = env_long(&a[j])
        {
            a[j] = full;
        }
        let w = a[j].as_str();
        if w == "--" {
            j += 1;
            break;
        }
        let cluster = env_cluster(w);
        if matches!(w, "--unset" | "--chdir" | "--argv0") {
            if w == "--chdir" {
                chdir = a.get(j + 1).cloned();
            }
            j += 2;
        } else if let Some(d) = w.strip_prefix("--chdir=") {
            chdir = Some(d.to_string());
            j += 1;
        } else if w == "-" {
            // `env -`: the same as `-i`
            j += 1;
        } else if w == "--split-string"
            || w.starts_with("--split-string=")
            || matches!(cluster, Some(('S', _)))
        {
            if cluster.is_some() && !env_flags_ok(w) {
                *unknown = true;
                if pick() {
                    j += 1;
                    continue;
                }
            }
            // a command line: splice its words in, split like a shell
            // (`-S`, `-S'…'`, `-iS '…'`)
            let after_s = match cluster {
                Some((_, rest)) => rest,
                None => String::new(),
            };
            let (text, width) =
                if w == "--split-string" || (!w.starts_with("--") && after_s.is_empty()) {
                    (a.get(j + 1).cloned(), 2)
                } else if let Some(v) = w.strip_prefix("--split-string=") {
                    (Some(v.to_string()), 1)
                } else {
                    (Some(after_s), 1)
                };
            if let Some(text) = text {
                split_texts.push(text.clone());
                if splices < 16 {
                    splices += 1;
                    let (words, depth_exceeded) = split_words(&text, depth + 1, cache);
                    if depth_exceeded {
                        *unknown = true;
                        unbounded = true;
                    }
                    let end = (j + width).min(a.len());
                    a.splice(j..end, words);
                } else {
                    *unknown = true;
                    unbounded = true;
                    j += width;
                }
            } else {
                j += 1;
            }
        } else if let Some((letter, rest)) = cluster {
            // `-u NAME`, `-iu PATH`, `-iCproj` (round 5 blocker 4)
            let ok = env_flags_ok(w);
            if !ok {
                *unknown = true;
            }
            if !ok && !rest.is_empty() && pick() {
                j += 1;
                continue;
            }
            let take_two = if !ok && rest.is_empty() {
                !pick()
            } else {
                rest.is_empty()
            };
            if letter == 'C' {
                if rest.is_empty() {
                    if take_two {
                        chdir = a.get(j + 1).cloned();
                    }
                } else {
                    chdir = Some(rest);
                }
            }
            j += if take_two { 2 } else { 1 };
        } else if w.starts_with("--") {
            let name = w.split_once('=').map_or(w, |(n, _)| n);
            let known_no_val = matches!(
                name,
                "--ignore-environment"
                    | "--null"
                    | "--debug"
                    | "--list-signal-handling"
                    | "--help"
                    | "--version"
            );
            let known_opt_val = matches!(
                name,
                "--block-signal" | "--default-signal" | "--ignore-signal"
            );
            let known_val = matches!(name, "--unset" | "--chdir" | "--argv0");
            if (known_no_val && w.contains('=')) || (!known_no_val && !known_opt_val && !known_val)
            {
                *unknown = true;
                // its value may be the next word (round 8 blocker 5)
                if !w.contains('=') && pick() {
                    j += 1;
                }
            }
            j += 1;
        } else if is_option(w) {
            if !env_flags_ok(w) {
                *unknown = true;
                if pick() {
                    j += 1;
                }
            }
            j += 1;
        } else if is_assignment_word(w) {
            j += 1;
        } else {
            break;
        }
    }
    EnvScan {
        chdir,
        split_texts,
        cmd_index: j,
        unbounded,
    }
}

fn push_bounded_str(out: &mut Vec<String>, val: String, unbounded: &mut bool) {
    if out.contains(&val) {
        return;
    }
    if out.len() < MAX_UNKNOWN_EMIT {
        out.push(val);
    } else {
        *unbounded = true;
    }
}

fn push_bounded_vec(out: &mut Vec<Vec<String>>, val: Vec<String>, unbounded: &mut bool) {
    if val.is_empty() || out.contains(&val) {
        return;
    }
    if out.len() < MAX_UNKNOWN_EMIT {
        out.push(val);
    } else {
        *unbounded = true;
    }
}

#[allow(clippy::too_many_arguments)]
fn expand_nested_reading(
    cand: &[String],
    depth: u8,
    cache: &mut LexCache,
    mut readings: Option<&mut Vec<Vec<String>>>,
    payloads: Option<&mut Vec<String>>,
    unbounded: &mut bool,
    rec_depth: u8,
) {
    let b0 = cmd_name(&cand[0]).into_owned();
    if b0 == "env" || b0 == "command" {
        expand_unknown_env_cmd(
            cand,
            b0 == "env",
            depth,
            cache,
            readings,
            payloads,
            unbounded,
            rec_depth + 1,
        );
    } else if wrapper_opts(&b0).is_some() || b0 == "arch" {
        let mut local_unbounded = vec![];
        for r_sub in unwrap_readings_bounded(cand, depth, cache, false, &mut local_unbounded) {
            if let Some(r) = readings.as_deref_mut() {
                push_bounded_vec(r, r_sub, unbounded);
            }
        }
        if !local_unbounded.is_empty() {
            *unbounded = true;
        }
    }
}

#[allow(clippy::too_many_arguments)]
/// Fail-closed expansion for an unknown `env` or `command` invocation:
/// treats every remaining non-option word as a possible start of the payload
/// and collects every `-C`/`--chdir` and `-S`/`--split-string` value.
fn expand_unknown_env_cmd(
    a: &[String],
    is_env: bool,
    depth: u8,
    cache: &mut LexCache,
    mut readings: Option<&mut Vec<Vec<String>>>,
    mut payloads: Option<&mut Vec<String>>,
    unbounded: &mut bool,
    rec_depth: u8,
) {
    if rec_depth > 4 || a.len() > MAX_UNKNOWN_STARTS {
        *unbounded = true;
        return;
    }
    let mut split_vals: Vec<(usize, String)> = vec![];
    let mut chdir_vals: Vec<(usize, String)> = vec![];
    if is_env {
        for m in 1..a.len() {
            let w = &a[m];
            let long = if w.starts_with("--") {
                env_long(w)
            } else {
                None
            };
            let lw = long.as_deref().unwrap_or(w.as_str());
            if lw == "--split-string" {
                if let Some(v) = a.get(m + 1) {
                    split_vals.push((m + 2, v.clone()));
                }
            } else if let Some(v) = lw.strip_prefix("--split-string=") {
                split_vals.push((m + 1, v.to_string()));
            } else if lw == "--chdir" {
                if let Some(v) = a.get(m + 1) {
                    chdir_vals.push((m, v.clone()));
                }
            } else if let Some(v) = lw.strip_prefix("--chdir=") {
                chdir_vals.push((m, v.to_string()));
            } else if w.starts_with('-') && !w.starts_with("--") {
                for (pos, _) in w[1..].match_indices('S') {
                    let after = &w[1 + pos + 1..];
                    if !after.is_empty() {
                        split_vals.push((m + 1, after.to_string()));
                    } else if let Some(v) = a.get(m + 1) {
                        split_vals.push((m + 2, v.clone()));
                    }
                }
                for (pos, _) in w[1..].match_indices('C') {
                    let after = &w[1 + pos + 1..];
                    if !after.is_empty() {
                        chdir_vals.push((m, after.to_string()));
                    } else if let Some(v) = a.get(m + 1) {
                        chdir_vals.push((m, v.clone()));
                    }
                }
            }
        }
    }

    let with_tail = |tail: usize, s: &str| -> String {
        if tail >= a.len() {
            s.to_string()
        } else {
            let tail_words: Vec<String> = a[tail..].iter().map(|w| quote(w)).collect();
            format!("{s} {}", tail_words.join(" "))
        }
    };

    for (tail, s) in &split_vals {
        if let Some(p) = payloads.as_deref_mut() {
            push_bounded_str(p, with_tail(*tail, s), unbounded);
        }
        let (sw, depth_exceeded) = split_words(s, depth + 1, cache);
        if depth_exceeded {
            *unbounded = true;
        }
        let mut sp = sw;
        sp.extend_from_slice(&a[*tail..]);
        if !sp.is_empty() {
            if sp.len() > MAX_UNKNOWN_STARTS {
                *unbounded = true;
            } else {
                if let Some(r) = readings.as_deref_mut() {
                    push_bounded_vec(r, sp.clone(), unbounded);
                }
                if sp[0].starts_with('-') || is_assignment_word(&sp[0]) {
                    let mut wrapped = vec!["env".to_string()];
                    wrapped.extend(sp);
                    expand_unknown_env_cmd(
                        &wrapped,
                        true,
                        depth,
                        cache,
                        readings.as_deref_mut(),
                        payloads.as_deref_mut(),
                        unbounded,
                        rec_depth + 1,
                    );
                } else {
                    expand_nested_reading(
                        &sp,
                        depth,
                        cache,
                        readings.as_deref_mut(),
                        payloads.as_deref_mut(),
                        unbounded,
                        rec_depth,
                    );
                }
            }
        }
    }

    let is_script_or_flock = a
        .first()
        .is_some_and(|s| matches!(cmd_name(s).as_ref(), "script" | "flock"));
    if is_script_or_flock && let Some(r) = readings.as_deref_mut() {
        for m in 1..a.len() {
            let w = a[m].as_str();
            let cmd = if w == "-c"
                || (w.starts_with("--")
                    && !w.contains('=')
                    && w.len() >= 3
                    && "--command".starts_with(w))
            {
                a.get(m + 1).cloned()
            } else if let Some((name, val)) = w.split_once('=') {
                (name.len() >= 3 && "--command".starts_with(name)).then(|| val.to_string())
            } else if w.starts_with('-') && !w.starts_with("--") && w.ends_with('c') {
                a.get(m + 1).cloned()
            } else {
                None
            };
            if let Some(c) = cmd {
                push_bounded_vec(r, vec!["sh".into(), "-c".into(), c], unbounded);
            }
        }
    }

    for i in 1..a.len() {
        let w = &a[i];
        if w == "--" || is_assignment_word(w) || is_option(w) {
            continue;
        }
        let cand = &a[i..];
        let mut resolved = cand.to_vec();
        if has_command_substitution(&resolved[0])
            && let Some(name) = substituted_program_at(&resolved[0], depth, cache)
        {
            resolved[0] = name;
        }
        if let Some(r) = readings.as_deref_mut() {
            push_bounded_vec(r, resolved.clone(), unbounded);
        }
        expand_nested_reading(
            &resolved,
            depth,
            cache,
            readings.as_deref_mut(),
            payloads.as_deref_mut(),
            unbounded,
            rec_depth,
        );
        if is_env && let Some(p) = payloads.as_deref_mut() {
            for (m, d) in &chdir_vals {
                if *m < i {
                    let rest: Vec<String> = cand.iter().map(|cw| quote(cw)).collect();
                    push_bounded_str(
                        p,
                        format!("cd {} && {}", quote(d), rest.join(" ")),
                        unbounded,
                    );
                }
            }
        }
    }

    if is_env && let Some(p) = payloads {
        for (_, d) in &chdir_vals {
            for (tail, s) in &split_vals {
                let cmd = with_tail(*tail, s);
                push_bounded_str(p, format!("cd {} && {cmd}", quote(d)), unbounded);
            }
        }
    }
}

fn env_payloads_bounded(
    argv: &[String],
    depth: u8,
    cache: &mut LexCache,
    piped: bool,
    unbounded_words: &mut Vec<UnboundedUnknown>,
) -> Vec<String> {
    let mut out = vec![];
    let Some(k) = argv.iter().position(|w| cmd_name(w) == "env") else {
        return out;
    };
    let mut push = |p: String| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    let mut choices = 0u32;
    let mut mask = 0u32;
    let mut any_unknown = false;
    let mut unbounded = false;
    let mut run_mask = |m: u32,
                        out_push: &mut dyn FnMut(String),
                        any_unknown: &mut bool,
                        unbounded: &mut bool|
     -> u32 {
        let mut a = argv[k..].to_vec();
        let mut cur_choices = 0u32;
        let mut unknown = false;
        let res = scan_env(&mut a, 1, m, &mut cur_choices, &mut unknown, depth, cache);
        *any_unknown |= unknown;
        *unbounded |= res.unbounded;
        for t in res.split_texts {
            out_push(t);
        }
        if let Some(d) = res.chdir {
            let rest: Vec<String> = a[res.cmd_index.min(a.len())..]
                .iter()
                .map(|w| quote(w))
                .collect();
            out_push(format!("cd {} && {}", quote(&d), rest.join(" ")));
        }
        cur_choices
    };
    loop {
        let cur_choices = run_mask(mask, &mut push, &mut any_unknown, &mut unbounded);
        choices = choices.max(cur_choices);
        mask += 1;
        if mask >= (1u32 << choices.min(4)) {
            break;
        }
    }
    if choices > 4 {
        run_mask(u32::MAX, &mut push, &mut any_unknown, &mut unbounded);
    }
    if any_unknown || unbounded {
        expand_unknown_env_cmd(
            &argv[k..],
            true,
            depth,
            cache,
            None,
            Some(&mut out),
            &mut unbounded,
            0,
        );
        if unbounded {
            let entry = UnboundedUnknown {
                words: argv[k..].to_vec(),
                piped,
            };
            if !unbounded_words.contains(&entry) {
                unbounded_words.push(entry);
            }
        }
    }
    out
}

/// Single-quotes a word for re-lexing.
fn quote(w: &str) -> String {
    format!("'{}'", w.replace('\'', "'\\''"))
}

/// The directories `find` walks and the index where its expression starts.
/// Skips the leading options with a table (round 6 item 2): GNU `-H -L -P`,
/// `-O<n>`, `-D <debug>`; BSD `-E -X -d -s -x` and `-f PATH` (a root).
/// Without a root `find` walks `.`.
pub fn find_roots(eff: &[String]) -> (Vec<String>, usize) {
    let (roots, j, _) = find_roots_explicit(eff);
    (roots, j)
}

/// [`find_roots`], and whether a root was written (`false`: it walks `.`,
/// the directory it runs in).
pub fn find_roots_explicit(eff: &[String]) -> (Vec<String>, usize, bool) {
    let mut roots = vec![];
    let mut j = 1;
    while j < eff.len() {
        let w = eff[j].as_str();
        if w == "--" {
            j += 1;
            break;
        }
        if w == "-D" {
            j += 2;
        } else if w == "-f" {
            if let Some(r) = eff.get(j + 1) {
                roots.push(r.clone());
            }
            j += 2;
        } else if (w.starts_with("-O") && w.len() > 2 && w[2..].chars().all(|c| c.is_ascii_digit()))
            || (w.len() > 1
                && w.starts_with('-')
                && !w.starts_with("--")
                && w[1..].chars().all(|c| "HLPEXdsx".contains(c)))
        {
            j += 1;
        } else {
            break;
        }
    }
    while j < eff.len() {
        let w = eff[j].as_str();
        if (w.starts_with('-') && w != "-") || w == "!" || w == "(" {
            break;
        }
        roots.push(eff[j].clone());
        j += 1;
    }
    let explicit = !roots.is_empty();
    if roots.is_empty() {
        roots.push(".".into());
    }
    (roots, j.min(eff.len()), explicit)
}

/// `find … -exec CMD … ;` becomes its own unit so hard-deny sees `CMD`, in
/// every reading of its wrappers; what `CMD` runs as code (`sh -c …`,
/// `env -C DIR …`, `eval`, a substitution) becomes units too, read the same
/// way as for a command written on the line (M1-18, K4), and a `find` it
/// runs is read again.
fn find_exec_units(eff: &[String], out: &mut Lexed, level: u8, cache: &mut LexCache) {
    let (roots, j) = find_roots(eff);
    let mut k = j;
    while k < eff.len() {
        if matches!(eff[k].as_str(), "-exec" | "-execdir" | "-ok" | "-okdir") {
            let mut argv = vec![];
            let mut m = k + 1;
            while m < eff.len() && eff[m] != ";" && eff[m] != "+" {
                argv.push(eff[m].clone());
                m += 1;
            }
            k = m + 1;
            if argv.is_empty() {
                continue;
            }
            // every reading of its wrappers and substituted words (`-exec
            // $(which sudo) id`: round 8 blocker 4)
            let mut effs = vec![unwrap_checked_at(&argv, level, cache).0];
            for r in unwrap_readings_bounded(
                &argv,
                level,
                cache,
                false,
                &mut out.unbounded_unknown_words,
            ) {
                if !r.is_empty() && !effs.contains(&r) {
                    effs.push(r);
                }
            }
            let payloads = code_texts_bounded(
                &argv,
                &effs,
                &[],
                level,
                cache,
                false,
                &mut out.unbounded_unknown_words,
            );
            for e in &effs {
                out.units.push(Unit {
                    seg: Segment {
                        assignments: vec![],
                        argv: argv.clone(),
                        redirects: vec![],
                        piped: false,
                    },
                    eff: e.clone(),
                    subs: vec![],
                    proc_sub: false,
                    grouped: true,
                    exec_roots: Some(roots.clone()),
                    derived: true,
                    pipe: 0,
                    join: Join::Seq,
                    ctx: CTX_IDS.fetch_add(1, Ordering::Relaxed),
                    level,
                    same_shell: false,
                    background: false,
                    compound: vec![],
                    close_join: None,
                    shell_union: false,
                    alt: vec![],
                });
                if level < MAX_DEPTH && e.first().is_some_and(|f| cmd_name(f) == "find") {
                    find_exec_units(e, out, level + 1, cache);
                }
            }
            // `find` runs it in a child: nothing it runs moves this shell
            for p in &payloads {
                let l = lex_depth_fresh(p, level + 1, cache);
                out.overflow |= l.overflow;
                out.unbounded_unknown_words
                    .extend(l.unbounded_unknown_words.iter().cloned());
                for mut u in l.units {
                    u.derived = true;
                    out.units.push(u);
                }
            }
            if out.units.len() > MAX_UNITS {
                out.overflow = true;
                return;
            }
            continue;
        }
        k += 1;
    }
}
