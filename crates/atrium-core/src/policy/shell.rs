//! A tolerant shell lexer: one pass that yields simple commands (argv,
//! assignments, redirects) and notes everything a rule must not trust
//! (substitution, globs, expansion, backgrounding, here-docs, subshells).
//! The hard-deny list reads the same output but looks inside substitutions
//! and payloads (`sh -c`, `eval`, `find -exec`) too.

use std::sync::atomic::{AtomicU32, Ordering};

use super::{Redirect, RedirectOp, Segment};

const MAX_DEPTH: u8 = 6;

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
}

#[derive(Debug, Clone, Default)]
pub struct Lexed {
    pub units: Vec<Unit>,
    /// Why rules and Home's read-only list must not allow the line.
    pub reasons: Vec<String>,
    /// More commands than the checker takes on (`MAX_UNITS`).
    pub overflow: bool,
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
}

static PIPE_IDS: AtomicU32 = AtomicU32::new(1);

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

/// `start` is just after the opening delimiter; returns the body and the
/// index just past the matching `)`.
fn scan_paren(chars: &[char], start: usize) -> (String, usize, bool) {
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
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
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
    (chars[start..].iter().collect(), chars.len(), false)
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

pub fn lex(line: &str) -> Lexed {
    lex_depth(line, 0, false)
}

fn lex_depth(line: &str, depth: u8, derived: bool) -> Lexed {
    let mut out = Lexed::default();
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

    macro_rules! flush {
        () => {{
            let b = std::mem::replace(&mut cur, Builder::new(false, paren_depth > 0));
            if !b.is_empty() {
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
                if c == '(' {
                    paren_depth += 1;
                } else {
                    paren_depth = paren_depth.saturating_sub(1);
                }
                out.reasons.push("subshell".into());
                i += 1;
                flush!();
                cur.grouped = paren_depth > 0;
                continue;
            }
            '&' if chars.get(i + 1) == Some(&'&') => {
                i += 2;
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
                // Both ends of a pipeline are not in plain sequence.
                cur.grouped |= is_pipe;
                if is_pipe && cur.pipe == 0 {
                    cur.pipe = PIPE_IDS.fetch_add(1, Ordering::Relaxed);
                }
                let id = cur.pipe;
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
                        let mut j = i + 2;
                        let mut body = String::new();
                        while j < chars.len() && chars[j] != '}' {
                            body.push(chars[j]);
                            j += 1;
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
        } = b;
        let seg = Segment {
            assignments,
            argv,
            redirects,
            piped,
        };
        let eff = unwrap(&seg.argv);
        if let Some(first) = seg.argv.first()
            && RESERVED.contains(&first.as_str())
        {
            out.reasons.push("compound command".into());
        }
        let mut extra: Vec<Lexed> = vec![];
        for body in &subs {
            let l = lex_depth(body, depth + 1, true);
            out.overflow |= l.overflow;
            extra.push(l);
        }
        let mut payload_units: Vec<Unit> = vec![];
        if let Some(p) = payload_of(&eff) {
            let l = lex_depth(&p, depth + 1, true);
            out.overflow |= l.overflow;
            payload_units.extend(l.units);
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
        };
        out.units.push(unit);
        if eff.first().map(|s| base(s)) == Some("find") {
            find_exec_units(&eff, &mut out.units);
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
    let is_stdin_code = |u: &Unit| {
        u.eff
            .first()
            .is_some_and(|f| is_shell(base(f)) || matches!(base(f), "eval" | "source" | "."))
            || reads_stdin_as_script(&u.eff)
    };
    if out.units.len() > MAX_UNITS {
        out.overflow = true;
        return out;
    }
    let mut texts: Vec<String> = vec![];
    // A body is shell-fed (lexed for the hard-deny list) unless it is clearly
    // text: its owner is a simple command that isn't a shell, `eval`,
    // `source` or an interpreter, and whose output isn't piped or
    // process-substituted. Groups, loops, subshells and anything piped count.
    let reads_code = |u: &Unit| {
        is_stdin_code(u)
            || u.eff.first().is_some_and(|f| {
                let b = base(f);
                b.starts_with("python")
                    || matches!(b, "perl" | "ruby" | "node" | "deno" | "bun" | "php" | "lua")
            })
    };
    for (owner, text) in &bodies {
        let clearly_text = raw_pos
            .get(*owner)
            .and_then(|p| out.units.get(*p))
            .is_some_and(|u| {
                !u.seg.argv.first().is_some_and(|w| {
                    (RESERVED.contains(&w.as_str()) && w != "!") || w == "}" || w == ")"
                }) && !reads_code(u)
                    && u.pipe == 0
                    && !u.proc_sub
                    && !u.grouped
            });
        if !clearly_text {
            texts.push(text.clone());
        }
    }
    if out.units.iter().any(is_stdin_code) {
        let mut seen_pipes: Vec<u32> = vec![];
        for u in out
            .units
            .iter()
            .filter(|u| u.pipe != 0 && u.seg.piped && is_stdin_code(u))
        {
            if seen_pipes.contains(&u.pipe) {
                continue;
            }
            seen_pipes.push(u.pipe);
            for prev in out.units.iter().filter(|p| p.pipe == u.pipe) {
                if prev
                    .eff
                    .first()
                    .is_some_and(|f| matches!(base(f), "echo" | "printf"))
                {
                    texts.push(
                        prev.eff[1..]
                            .iter()
                            .filter(|a| !a.starts_with('-'))
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(" "),
                    );
                }
            }
        }
    }
    let mut extra_units: Vec<Unit> = vec![];
    for t in &texts {
        let l = lex_depth(t, depth + 1, true);
        out.overflow |= l.overflow;
        for mut u in l.units {
            u.derived = true;
            extra_units.push(u);
        }
        if out.units.len() + extra_units.len() > MAX_UNITS {
            out.overflow = true;
            return out;
        }
    }
    out.units.extend(extra_units);
    out.reasons.sort();
    out.reasons.dedup();
    out
}

/// Words that start compound commands; a segment starting with one can't be
/// matched by a rule.
const RESERVED: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "function", "select", "{", "}", "!", "[[", "coproc",
];

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

/// Strips reserved words and transparent wrappers off the front of `argv`.
pub fn unwrap(argv: &[String]) -> Vec<String> {
    let mut a: Vec<String> = argv.to_vec();
    for _ in 0..8 {
        let Some(first) = a.first() else { break };
        let b = base(first).to_string();
        let skip = |a: &[String], flags_with_value: &[&str], bare: &[&str]| -> usize {
            let mut j = 1;
            while j < a.len() {
                let w = a[j].as_str();
                if w == "--" {
                    j += 1;
                    break;
                }
                if flags_with_value.contains(&w) {
                    j += 2;
                } else if bare.contains(&w) || (w.starts_with('-') && w.len() > 1 && w != "-") {
                    j += 1;
                } else {
                    break;
                }
            }
            j.min(a.len())
        };
        let n = match b.as_str() {
            "{" | "}" | "!" | "do" | "then" | "else" | "elif" | "if" | "while" | "until"
            | "time" | "builtin" | "nohup" | "coproc" => {
                if b == "time" || b == "nohup" || b == "builtin" {
                    skip(&a, &[], &[])
                } else {
                    1
                }
            }
            "command" | "setsid" | "unbuffer" => skip(&a, &[], &[]),
            "caffeinate" => skip(&a, &["-t", "-w"], &[]),
            "arch" => skip(&a, &["-e", "-d"], &[]),
            "sandbox-exec" => skip(&a, &["-f", "-n", "-p", "-D"], &[]),
            "chroot" => {
                // `chroot [opts] NEWROOT [COMMAND…]`; without a command it
                // starts a shell (see `reads_stdin_as_script`).
                let j = skip(&a, &["--userspec", "--groups"], &[]);
                if j + 1 < a.len() { j + 1 } else { 0 }
            }
            "flock" => {
                // `flock [opts] FILE COMMAND…`, or `flock FILE -c 'string'`
                // (the string runs through a shell).
                if let Some(c) = a
                    .iter()
                    .position(|w| matches!(w.as_str(), "-c" | "--command"))
                    && let Some(cmd) = a.get(c + 1).cloned()
                {
                    a = vec!["sh".into(), "-c".into(), cmd];
                    continue;
                }
                let j = skip(
                    &a,
                    &["-w", "-E", "--timeout", "--wait", "--conflict-exit-code"],
                    &[],
                );
                if j + 1 < a.len() { j + 1 } else { 0 }
            }
            "script" => {
                // macOS `script [opts] FILE [COMMAND…]`; Linux `script -c CMD`
                if let Some(c) = a
                    .iter()
                    .position(|w| matches!(w.as_str(), "-c" | "--command"))
                    && let Some(cmd) = a.get(c + 1).cloned()
                {
                    a = vec!["sh".into(), "-c".into(), cmd];
                    continue;
                }
                let j = skip(&a, &["-F", "-t", "-T", "-I", "-O", "-B", "-E", "-m"], &[]);
                if j + 1 < a.len() { j + 1 } else { 0 }
            }
            "busybox" | "toybox" => 1,
            "exec" => skip(&a, &["-a"], &[]),
            "nice" => skip(&a, &["-n"], &[]),
            "ionice" => skip(&a, &["-c", "-n", "-p"], &[]),
            "stdbuf" => skip(&a, &["-i", "-o", "-e"], &[]),
            "timeout" => {
                let j = skip(&a, &["-s", "-k"], &[]);
                (j + 1).min(a.len())
            }
            "xargs" => skip(
                &a,
                &["-n", "-P", "-I", "-L", "-s", "-d", "-a", "-E", "-J"],
                &[],
            ),
            "env" => {
                let mut j = 1;
                while j < a.len() {
                    let w = a[j].as_str();
                    if w == "--" {
                        j += 1;
                        break;
                    }
                    if matches!(w, "-u" | "-C" | "-P" | "--unset" | "--chdir") {
                        j += 2;
                    } else if w == "-S"
                        || w == "--split-string"
                        || w.starts_with("--split-string=")
                        || (w.starts_with("-S") && !w.starts_with("--"))
                    {
                        // a command line: splice its words in, split like a shell
                        let (text, width) = if w == "-S" || w == "--split-string" {
                            (a.get(j + 1).cloned(), 2)
                        } else if let Some(v) = w.strip_prefix("--split-string=") {
                            (Some(v.to_string()), 1)
                        } else {
                            (Some(w[2..].to_string()), 1)
                        };
                        if let Some(text) = text {
                            let words = split_words(&text);
                            let end = (j + width).min(a.len());
                            a.splice(j..end, words);
                        } else {
                            j += 1;
                        }
                    } else if (w.starts_with('-') && w.len() > 1) || is_assignment_word(w) {
                        j += 1;
                    } else {
                        break;
                    }
                }
                j.min(a.len())
            }
            _ => break,
        };
        if n == 0 {
            break;
        }
        if n >= a.len() {
            a.clear();
            break;
        }
        a.drain(..n);
    }
    a
}

/// Splits a string into words the way a shell would (quotes, escapes);
/// assignments stay words. Used for `env -S`.
fn split_words(text: &str) -> Vec<String> {
    let l = lex(text);
    match l.units.into_iter().find(|u| !u.derived) {
        Some(u) => {
            let mut w: Vec<String> = u
                .seg
                .assignments
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            w.extend(u.seg.argv);
            w
        }
        None => text.split_whitespace().map(str::to_string).collect(),
    }
}

fn is_assignment_word(w: &str) -> bool {
    w.split_once('=').is_some_and(|(n, _)| is_name(n))
}

const SHELLS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "ksh", "fish", "ash", "csh", "tcsh", "mksh", "yash", "rbash",
    "posh", "elvish", "xonsh",
];

/// A shell by name, or a command word that starts with `$` or a backtick
/// (`$SHELL`, `${SHELL}`, `` `which bash` ``): its value is only known at run
/// time, so it is assumed to be a shell.
pub fn is_shell(cmd: &str) -> bool {
    let b = base(cmd);
    SHELLS.contains(&b) || cmd.starts_with('$') || cmd.starts_with('`')
}

/// Commands that run their standard input as a script of commands without
/// being a shell: `at`, `batch`, `crontab -`, `make -f -`, `script`.
pub fn reads_stdin_as_script(eff: &[String]) -> bool {
    let Some(first) = eff.first() else {
        return false;
    };
    let args = &eff[1..];
    match base(first) {
        "at" | "batch" => !args.iter().any(|a| a == "-f" || a.starts_with("-f")),
        "script" => true,
        "chroot" => {
            // without a command, `chroot NEWROOT` starts a shell
            args.iter().filter(|a| !a.starts_with('-')).count() <= 1
        }
        "crontab" => args.iter().any(|a| a == "-"),
        "make" | "gmake" => args.iter().enumerate().any(|(i, a)| {
            a == "-f-"
                || a == "--file=-"
                || a == "--makefile=-"
                || (matches!(a.as_str(), "-f" | "--file" | "--makefile")
                    && args.get(i + 1).is_some_and(|n| n == "-"))
        }),
        _ => false,
    }
}

/// The string a shell or `eval` will run, if the command has one.
fn payload_of(eff: &[String]) -> Option<String> {
    let first = base(eff.first()?);
    if is_shell(eff.first()?) {
        let mut j = 1;
        while j < eff.len() {
            let w = &eff[j];
            if w == "--" {
                break;
            }
            if w.starts_with('-') && !w.starts_with("--") && w[1..].contains('c') {
                return eff.get(j + 1).cloned();
            }
            if matches!(w.as_str(), "-o" | "+o" | "-O" | "+O") {
                j += 2;
                continue;
            }
            if w.starts_with('-') || w.starts_with('+') {
                j += 1;
                continue;
            }
            break;
        }
        None
    } else if first == "eval" {
        if eff.len() > 1 {
            Some(eff[1..].join(" "))
        } else {
            None
        }
    } else {
        None
    }
}

/// The directories `find` walks and the index where its expression starts.
/// Skips the leading options (`-H -L -P -E -X -O<n> -D <x>`).
pub fn find_roots(eff: &[String]) -> (Vec<String>, usize) {
    let mut j = 1;
    while j < eff.len() {
        let w = eff[j].as_str();
        if matches!(w, "-H" | "-L" | "-P" | "-E" | "-X") || (w.starts_with("-O") && w.len() > 2) {
            j += 1;
        } else if w == "-D" {
            j += 2;
        } else {
            break;
        }
    }
    let mut roots = vec![];
    while j < eff.len() {
        let w = eff[j].as_str();
        if (w.starts_with('-') && w != "-") || w == "!" || w == "(" {
            break;
        }
        roots.push(eff[j].clone());
        j += 1;
    }
    if roots.is_empty() {
        roots.push(".".into());
    }
    (roots, j.min(eff.len()))
}

/// `find … -exec CMD … ;` becomes its own unit so hard-deny sees `CMD`.
fn find_exec_units(eff: &[String], units: &mut Vec<Unit>) {
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
            if !argv.is_empty() {
                let e = unwrap(&argv);
                units.push(Unit {
                    seg: Segment {
                        assignments: vec![],
                        argv,
                        redirects: vec![],
                        piped: false,
                    },
                    eff: e,
                    subs: vec![],
                    proc_sub: false,
                    grouped: true,
                    exec_roots: Some(roots.clone()),
                    derived: true,
                    pipe: 0,
                });
            }
            k = m;
        }
        k += 1;
    }
}
