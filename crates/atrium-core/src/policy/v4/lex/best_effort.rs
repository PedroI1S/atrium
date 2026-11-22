//! Tolerant best-effort shell lexer and bounded brace expander (owned by P1;
//! contract §2, §8, §12).
//!
//! Produces [`BestEffortUnit`]s — including units derived from `-c` scripts,
//! `eval`, `env -S`, `script -c`, `flock -c`, `find -exec`, substitutions,
//! here-docs/here-strings, and piped `echo`/`printf` — under the shared
//! [`ComputeBudget`], strictly linear in the line length, with CR-19 command
//! name folding. Kept strictly separate from the exact model.

use std::collections::HashMap;

use crate::policy::limits;
use crate::policy::model::{
    BestEffortRecord, BestEffortRedirectKind, BestEffortUnit, Cap, ComputeBudget, Exhausted,
    FoldRules, PartOutcome, PipelineId,
};

use super::{fold_command_bytes, is_posix_name};

/// Maximum nesting depth for scanning nested quoting/substitution delimiters
/// (`$(` inside `"` inside `${` …) before giving up linearly.
const MAX_SCAN_NEST: u16 = 64;

/// Maximum consecutive transparent wrappers unwrapped in the best-effort pass
/// (covers §10 row 5 `command` ×13 `sudo id` while remaining strictly bounded).
const MAX_BEST_EFFORT_WRAP: usize = 32;

/// Maximum alternative readings kept per unit when a wrapper has unknown flags.
const MAX_ALT_READINGS: usize = 8;

/// Maximum entries in the per-call recursion cache inside one best-effort pass.
const MAX_CALL_CACHE: usize = 256;

/// Maximum brace-expanded words produced from one token (N11, best-effort only).
const MAX_BRACE_RESULTS: usize = 256;

/// Maximum sequence items in `{x..y}` (N11, best-effort only).
const MAX_SEQUENCE: usize = 64;

/// Maximum total characters scanned inside `brace_expand_budgeted` (N11).
const MAX_BRACE_WORK: usize = 2_000_000;

/// Entry point shared by `lex_best_effort` and `build_units`. Populates
/// `record` incrementally so anything discovered before `Exhausted` is kept.
pub(crate) fn lex_best_effort_into(
    raw_texts: &[&str],
    record: &mut BestEffortRecord,
    budget: &mut ComputeBudget,
) -> Result<(), PartOutcome> {
    let total_bytes: usize = raw_texts
        .iter()
        .fold(0usize, |acc, s| acc.saturating_add(s.len()));
    if total_bytes > limits::MAX_BYTES {
        return Err(PartOutcome::TooLarge(Cap::Bytes));
    }
    if let Err(e) = budget.charge(1) {
        return Err(PartOutcome::Exhausted(e.reason));
    }

    let mut state = BeState {
        next_pipe_id: 1,
        cache: HashMap::new(),
    };

    for &text in raw_texts {
        if let Err(e) = budget
            .charge(text.len() as u64)
            .and_then(|()| budget.charge_alloc(text.len() + std::mem::size_of::<String>()))
        {
            return Err(PartOutcome::Exhausted(e.reason));
        }
        record.raw_texts.push(text.to_string());
        if let Err(outcome) = lex_one_text(text, 0, false, record, &mut state, budget) {
            return Err(outcome);
        }
    }

    Ok(())
}

struct BeState {
    next_pipe_id: u32,
    cache: HashMap<(String, u8), Vec<BestEffortUnit>>,
}

struct BeBuilder {
    assignments: Vec<(String, String)>,
    raw_argv: Vec<String>,
    redirects: Vec<(BestEffortRedirectKind, String)>,
    subs: Vec<String>,
    pipeline_id: Option<PipelineId>,
    piped: bool,
}

impl BeBuilder {
    fn new(pipeline_id: Option<PipelineId>, piped: bool) -> Self {
        Self {
            assignments: Vec::new(),
            raw_argv: Vec::new(),
            redirects: Vec::new(),
            subs: Vec::new(),
            pipeline_id,
            piped,
        }
    }

    fn is_empty(&self) -> bool {
        self.assignments.is_empty() && self.raw_argv.is_empty() && self.redirects.is_empty()
    }
}

enum BePending {
    Redirect(BestEffortRedirectKind),
    HereString,
}

/// Helper charging 1 compute unit + 1 alloc byte before pushing a char.
#[inline]
fn push_char_budgeted(
    s: &mut String,
    c: char,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    let n = c.len_utf8();
    budget.charge(n as u64)?;
    budget.charge_alloc(n)?;
    s.push(c);
    Ok(())
}

/// Helper charging copy + allocation before pushing a `&str`.
#[inline]
fn push_str_budgeted(
    s: &mut String,
    part: &str,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    let n = part.len();
    if n > 0 {
        budget.charge(n as u64)?;
        budget.charge_alloc(n)?;
        s.push_str(part);
    }
    Ok(())
}

#[inline]
fn clone_string_budgeted(s: &str, budget: &mut ComputeBudget) -> Result<String, Exhausted> {
    budget.charge(s.len().max(1) as u64)?;
    budget.charge_alloc(s.len() + std::mem::size_of::<String>())?;
    Ok(s.to_string())
}

#[inline]
fn clone_vec_string_budgeted(
    v: &[String],
    budget: &mut ComputeBudget,
) -> Result<Vec<String>, Exhausted> {
    let bytes: usize = v
        .iter()
        .map(|s| s.len() + std::mem::size_of::<String>())
        .sum();
    budget.charge(bytes.max(1) as u64)?;
    budget.charge_alloc(bytes)?;
    Ok(v.to_vec())
}

/// Linear byte-slice scanner for `$(...)` / `<(...)` / `>(...)` starting just
/// after `(`. Always advances past the scanned region so the outer lexer never
/// rescans the same bytes.
fn scan_paren_bytes<'a>(
    text: &'a str,
    start: usize,
    nest: u16,
    budget: &mut ComputeBudget,
) -> Result<(&'a str, usize), Exhausted> {
    let bytes = text.as_bytes();
    let start = start.min(bytes.len());
    if nest > MAX_SCAN_NEST {
        let rest = &text[start..];
        budget.charge(rest.len() as u64)?;
        return Ok((rest, bytes.len()));
    }
    let mut depth = 1usize;
    let mut i = start;
    while i < bytes.len() {
        budget.charge(1)?;
        match bytes[i] {
            b'\\' => {
                if i + 1 < bytes.len() {
                    budget.charge(1)?;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            b'\'' => {
                i += 1;
                while i < bytes.len() {
                    budget.charge(1)?;
                    let b = bytes[i];
                    i += 1;
                    if b == b'\'' {
                        break;
                    }
                }
            }
            b'"' => {
                i = skip_dq_bytes(text, i + 1, nest + 1, budget)?;
            }
            b'`' => {
                let (_, end) = scan_backtick_bytes(text, i + 1, budget)?;
                i = end;
            }
            b'$' if bytes.get(i + 1) == Some(&b'{') => {
                budget.charge(1)?;
                let (_, end) = scan_dollar_brace_bytes(text, i + 2, false, nest + 1, budget)?;
                i = end;
            }
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((&text[start..i], i + 1));
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    Ok((&text[start..], bytes.len()))
}

fn skip_dq_bytes(
    text: &str,
    start: usize,
    nest: u16,
    budget: &mut ComputeBudget,
) -> Result<usize, Exhausted> {
    let bytes = text.as_bytes();
    let start = start.min(bytes.len());
    if nest > MAX_SCAN_NEST {
        budget.charge((bytes.len() - start) as u64)?;
        return Ok(bytes.len());
    }
    let mut i = start;
    while i < bytes.len() {
        budget.charge(1)?;
        match bytes[i] {
            b'"' => return Ok(i + 1),
            b'\\' => {
                if i + 1 < bytes.len() {
                    budget.charge(1)?;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            b'$' if bytes.get(i + 1) == Some(&b'(') => {
                budget.charge(1)?;
                let (_, end) = scan_paren_bytes(text, i + 2, nest + 1, budget)?;
                i = end;
            }
            b'$' if bytes.get(i + 1) == Some(&b'{') => {
                budget.charge(1)?;
                let (_, end) = scan_dollar_brace_bytes(text, i + 2, true, nest + 1, budget)?;
                i = end;
            }
            b'`' => {
                let (_, end) = scan_backtick_bytes(text, i + 1, budget)?;
                i = end;
            }
            _ => i += 1,
        }
    }
    Ok(bytes.len())
}

fn scan_backtick_bytes(
    text: &str,
    start: usize,
    budget: &mut ComputeBudget,
) -> Result<(String, usize), Exhausted> {
    let bytes = text.as_bytes();
    let mut i = start.min(bytes.len());
    let mut body = String::new();
    while i < bytes.len() {
        budget.charge(1)?;
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            budget.charge(1)?;
            let ch = text[i + 1..].chars().next().unwrap_or('\\');
            push_char_budgeted(&mut body, ch, budget)?;
            i += 1 + ch.len_utf8();
        } else if bytes[i] == b'`' {
            return Ok((body, i + 1));
        } else {
            let ch = text[i..].chars().next().unwrap_or(' ');
            push_char_budgeted(&mut body, ch, budget)?;
            i += ch.len_utf8();
        }
    }
    Ok((body, bytes.len()))
}

fn scan_dollar_brace_bytes<'a>(
    text: &'a str,
    start: usize,
    in_dq: bool,
    nest: u16,
    budget: &mut ComputeBudget,
) -> Result<(&'a str, usize), Exhausted> {
    let bytes = text.as_bytes();
    let start = start.min(bytes.len());
    if nest > MAX_SCAN_NEST {
        let rest = &text[start..];
        budget.charge(rest.len() as u64)?;
        return Ok((rest, bytes.len()));
    }
    let mut i = start;
    while i < bytes.len() {
        budget.charge(1)?;
        match bytes[i] {
            b'}' => return Ok((&text[start..i], i + 1)),
            b'\\' => {
                if i + 1 < bytes.len() {
                    budget.charge(1)?;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            b'$' if bytes.get(i + 1) == Some(&b'(') => {
                budget.charge(1)?;
                let (_, end) = scan_paren_bytes(text, i + 2, nest + 1, budget)?;
                i = end;
            }
            b'$' if bytes.get(i + 1) == Some(&b'{') => {
                budget.charge(1)?;
                let (_, end) = scan_dollar_brace_bytes(text, i + 2, in_dq, nest + 1, budget)?;
                i = end;
            }
            b'`' => {
                let (_, end) = scan_backtick_bytes(text, i + 1, budget)?;
                i = end;
            }
            b'"' => {
                i = skip_dq_bytes(text, i + 1, nest + 1, budget)?;
            }
            b'\'' if !in_dq => {
                i += 1;
                while i < bytes.len() {
                    budget.charge(1)?;
                    let b = bytes[i];
                    i += 1;
                    if b == b'\'' {
                        break;
                    }
                }
            }
            _ => i += 1,
        }
    }
    Ok((&text[start..], bytes.len()))
}

/// Scans `$(...)` and `` `...` `` bodies in text that is expanded but not
/// word-split (here-doc bodies, `${...}` bodies, `$((...))`). Linear in
/// `text.len()`.
fn find_substitutions_budgeted(
    text: &str,
    budget: &mut ComputeBudget,
) -> Result<Vec<String>, Exhausted> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        budget.charge(1)?;
        if bytes[i] == b'$' && bytes.get(i + 1) == Some(&b'(') {
            budget.charge(1)?;
            let (body, end) = scan_paren_bytes(text, i + 2, 0, budget)?;
            out.push(clone_string_budgeted(body, budget)?);
            i = end;
        } else if bytes[i] == b'`' {
            let (body, end) = scan_backtick_bytes(text, i + 1, budget)?;
            out.push(body);
            i = end;
        } else {
            i += 1;
        }
    }
    Ok(out)
}

fn lex_one_text(
    text: &str,
    depth: u8,
    derived: bool,
    record: &mut BestEffortRecord,
    state: &mut BeState,
    budget: &mut ComputeBudget,
) -> Result<(), PartOutcome> {
    if depth > limits::MAX_DEPTH {
        return Ok(());
    }
    if derived {
        let key_len = text.len() + 1;
        if let Some(cached) = state.cache.get(&(text.to_string(), depth)).cloned() {
            for mut u in cached {
                if record.units.len() >= limits::MAX_UNITS {
                    return Err(PartOutcome::TooLarge(Cap::Units));
                }
                let est = estimate_unit_bytes(&u);
                if let Err(e) = budget
                    .charge(est.max(1) as u64)
                    .and_then(|()| budget.charge_alloc(est))
                {
                    return Err(PartOutcome::Exhausted(e.reason));
                }
                u.index = record.units.len();
                record.units.push(u);
            }
            return Ok(());
        }
        let _ = key_len;
    }

    let start_unit_len = record.units.len();
    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut cur = BeBuilder::new(None, false);
    let mut raw_units: Vec<BeBuilder> = Vec::new();
    let mut pending: Option<BePending> = None;
    let mut heredocs: Vec<(String, bool, bool)> = Vec::new();
    let mut heredoc_owners: Vec<usize> = Vec::new();
    let mut bodies: Vec<(usize, String)> = Vec::new();

    let flush_builder = |cur: &mut BeBuilder,
                         next_pipe: Option<PipelineId>,
                         next_piped: bool,
                         raw_units: &mut Vec<BeBuilder>,
                         budget: &mut ComputeBudget|
     -> Result<(), PartOutcome> {
        let b = std::mem::replace(cur, BeBuilder::new(next_pipe, next_piped));
        if !b.is_empty() {
            if let Err(e) = budget
                .charge(1)
                .and_then(|()| budget.charge_alloc(std::mem::size_of::<BeBuilder>()))
            {
                return Err(PartOutcome::Exhausted(e.reason));
            }
            raw_units.push(b);
            if raw_units.len() > limits::MAX_UNITS {
                return Err(PartOutcome::TooLarge(Cap::Units));
            }
        }
        Ok(())
    };

    while i < bytes.len() {
        if let Err(e) = budget.charge(1) {
            return Err(PartOutcome::Exhausted(e.reason));
        }
        match bytes[i] {
            b' ' | b'\t' => {
                i += 1;
            }
            b'\\' if bytes.get(i + 1) == Some(&b'\n') => {
                if let Err(e) = budget.charge(1) {
                    return Err(PartOutcome::Exhausted(e.reason));
                }
                i += 2;
            }
            b'\n' => {
                i += 1;
                flush_builder(&mut cur, None, false, &mut raw_units, budget)?;
                let owners = std::mem::take(&mut heredoc_owners);
                for (hd_idx, (delim, strip_tabs, quoted)) in
                    std::mem::take(&mut heredocs).into_iter().enumerate()
                {
                    let owner = owners.get(hd_idx).copied().unwrap_or(usize::MAX);
                    let mut body = String::new();
                    while i < bytes.len() {
                        let line_start = i;
                        while i < bytes.len() {
                            if let Err(e) = budget.charge(1) {
                                return Err(PartOutcome::Exhausted(e.reason));
                            }
                            if bytes[i] == b'\n' {
                                break;
                            }
                            i += 1;
                        }
                        let line_str = &text[line_start..i];
                        if i < bytes.len() && bytes[i] == b'\n' {
                            i += 1;
                        }
                        let trimmed = if strip_tabs {
                            line_str.trim_start_matches('\t')
                        } else {
                            line_str
                        };
                        if trimmed == delim {
                            break;
                        }
                        if let Err(e) = push_str_budgeted(&mut body, line_str, budget)
                            .and_then(|()| push_char_budgeted(&mut body, '\n', budget))
                        {
                            return Err(PartOutcome::Exhausted(e.reason));
                        }
                    }
                    if !quoted {
                        match find_substitutions_budgeted(&body, budget) {
                            Ok(subs) => {
                                if let Some(last) = raw_units.last_mut() {
                                    last.subs.extend(subs);
                                }
                            }
                            Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
                        }
                    }
                    bodies.push((owner, body));
                }
            }
            b';' => {
                if bytes.get(i + 1) == Some(&b';') {
                    if let Err(e) = budget.charge(1) {
                        return Err(PartOutcome::Exhausted(e.reason));
                    }
                    i += 2;
                } else {
                    i += 1;
                }
                flush_builder(&mut cur, None, false, &mut raw_units, budget)?;
            }
            b'(' | b')' => {
                let c = bytes[i];
                i += 1;
                if c == b'(' {
                    // `f()` function definition: skip empty parens cleanly.
                    let mut k = i;
                    while matches!(bytes.get(k), Some(b' ' | b'\t')) {
                        if let Err(e) = budget.charge(1) {
                            return Err(PartOutcome::Exhausted(e.reason));
                        }
                        k += 1;
                    }
                    if bytes.get(k) == Some(&b')') && !cur.raw_argv.is_empty() {
                        if let Err(e) = budget.charge(1) {
                            return Err(PartOutcome::Exhausted(e.reason));
                        }
                        i = k + 1;
                    }
                }
                flush_builder(&mut cur, None, false, &mut raw_units, budget)?;
            }
            b'&' if bytes.get(i + 1) == Some(&b'&') => {
                if let Err(e) = budget.charge(1) {
                    return Err(PartOutcome::Exhausted(e.reason));
                }
                i += 2;
                flush_builder(&mut cur, None, false, &mut raw_units, budget)?;
            }
            b'&' if bytes.get(i + 1) == Some(&b'>') => {
                let step = if bytes.get(i + 2) == Some(&b'>') {
                    3
                } else {
                    2
                };
                if let Err(e) = budget.charge((step - 1) as u64) {
                    return Err(PartOutcome::Exhausted(e.reason));
                }
                i += step;
                pending = Some(BePending::Redirect(BestEffortRedirectKind::Write));
            }
            b'&' => {
                i += 1;
                flush_builder(&mut cur, None, false, &mut raw_units, budget)?;
            }
            b'|' => {
                let is_pipe = bytes.get(i + 1) != Some(&b'|');
                let step = if is_pipe && bytes.get(i + 1) == Some(&b'&') {
                    2
                } else if is_pipe {
                    1
                } else {
                    2
                };
                if step > 1
                    && let Err(e) = budget.charge((step - 1) as u64)
                {
                    return Err(PartOutcome::Exhausted(e.reason));
                }
                i += step;
                if is_pipe {
                    let pid = cur.pipeline_id.unwrap_or_else(|| {
                        let id = PipelineId(state.next_pipe_id);
                        state.next_pipe_id += 1;
                        id
                    });
                    cur.pipeline_id = Some(pid);
                    flush_builder(&mut cur, Some(pid), true, &mut raw_units, budget)?;
                } else {
                    flush_builder(&mut cur, None, false, &mut raw_units, budget)?;
                }
            }
            b'#' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'\n' {
                    if let Err(e) = budget.charge(1) {
                        return Err(PartOutcome::Exhausted(e.reason));
                    }
                    i += 1;
                }
            }
            b'<' | b'>' if bytes.get(i + 1) != Some(&b'(') => {
                let before_hd = heredocs.len();
                match lex_be_redirect(text, i, &mut pending, &mut heredocs, budget) {
                    Ok(new_i) => i = new_i,
                    Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
                }
                if heredocs.len() > before_hd {
                    heredoc_owners.push(raw_units.len());
                }
            }
            _ => {
                // Read one word linearly.
                let mut word = String::new();
                let mut quoted_prefix_empty = true;
                let mut assign_eq: Option<usize> = None;
                let start_i = i;

                while i < bytes.len() {
                    if i > start_i
                        && let Err(e) = budget.charge(1)
                    {
                        return Err(PartOutcome::Exhausted(e.reason));
                    }
                    let b = bytes[i];
                    match b {
                        b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'(' | b')' => break,
                        b'<' | b'>' if bytes.get(i + 1) != Some(&b'(') => break,
                        b'\\' => {
                            quoted_prefix_empty = false;
                            if i + 1 < bytes.len() {
                                if let Err(e) = budget.charge(1) {
                                    return Err(PartOutcome::Exhausted(e.reason));
                                }
                                let next_ch = text[i + 1..].chars().next().unwrap_or(' ');
                                if next_ch != '\n'
                                    && let Err(e) = push_char_budgeted(&mut word, next_ch, budget)
                                {
                                    return Err(PartOutcome::Exhausted(e.reason));
                                }
                                i += 1 + next_ch.len_utf8();
                            } else {
                                i += 1;
                            }
                        }
                        b'\'' => {
                            quoted_prefix_empty = false;
                            i += 1;
                            while i < bytes.len() {
                                if let Err(e) = budget.charge(1) {
                                    return Err(PartOutcome::Exhausted(e.reason));
                                }
                                if bytes[i] == b'\'' {
                                    i += 1;
                                    break;
                                }
                                let ch = text[i..].chars().next().unwrap_or(' ');
                                if let Err(e) = push_char_budgeted(&mut word, ch, budget) {
                                    return Err(PartOutcome::Exhausted(e.reason));
                                }
                                i += ch.len_utf8();
                            }
                        }
                        b'"' => {
                            quoted_prefix_empty = false;
                            i += 1;
                            while i < bytes.len() {
                                if let Err(e) = budget.charge(1) {
                                    return Err(PartOutcome::Exhausted(e.reason));
                                }
                                match bytes[i] {
                                    b'"' => {
                                        i += 1;
                                        break;
                                    }
                                    b'\\'
                                        if matches!(
                                            bytes.get(i + 1),
                                            Some(b'$' | b'`' | b'"' | b'\\' | b'\n')
                                        ) =>
                                    {
                                        if let Err(e) = budget.charge(1) {
                                            return Err(PartOutcome::Exhausted(e.reason));
                                        }
                                        let nb = bytes[i + 1];
                                        if nb != b'\n'
                                            && let Err(e) =
                                                push_char_budgeted(&mut word, nb as char, budget)
                                        {
                                            return Err(PartOutcome::Exhausted(e.reason));
                                        }
                                        i += 2;
                                    }
                                    b'$' if bytes.get(i + 1) == Some(&b'(') => {
                                        if let Err(e) = budget.charge(1) {
                                            return Err(PartOutcome::Exhausted(e.reason));
                                        }
                                        match scan_paren_bytes(text, i + 2, 0, budget) {
                                            Ok((body, end)) => {
                                                if let Err(e) =
                                                    push_str_budgeted(&mut word, "$(", budget)
                                                        .and_then(|()| {
                                                            push_str_budgeted(
                                                                &mut word, body, budget,
                                                            )
                                                        })
                                                        .and_then(|()| {
                                                            push_char_budgeted(
                                                                &mut word, ')', budget,
                                                            )
                                                        })
                                                        .and_then(|()| {
                                                            clone_string_budgeted(body, budget)
                                                                .map(|s| cur.subs.push(s))
                                                        })
                                                {
                                                    return Err(PartOutcome::Exhausted(e.reason));
                                                }
                                                i = end;
                                            }
                                            Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
                                        }
                                    }
                                    b'`' => match scan_backtick_bytes(text, i + 1, budget) {
                                        Ok((body, end)) => {
                                            if let Err(e) =
                                                push_char_budgeted(&mut word, '`', budget)
                                                    .and_then(|()| {
                                                        push_str_budgeted(&mut word, &body, budget)
                                                    })
                                                    .and_then(|()| {
                                                        push_char_budgeted(&mut word, '`', budget)
                                                    })
                                            {
                                                return Err(PartOutcome::Exhausted(e.reason));
                                            }
                                            cur.subs.push(body);
                                            i = end;
                                        }
                                        Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
                                    },
                                    b'$' if bytes.get(i + 1) == Some(&b'{') => {
                                        if let Err(e) = budget.charge(1) {
                                            return Err(PartOutcome::Exhausted(e.reason));
                                        }
                                        match scan_dollar_brace_bytes(text, i + 2, true, 0, budget)
                                        {
                                            Ok((body, end)) => {
                                                if let Err(e) =
                                                    push_str_budgeted(&mut word, "${", budget)
                                                        .and_then(|()| {
                                                            push_str_budgeted(
                                                                &mut word, body, budget,
                                                            )
                                                        })
                                                        .and_then(|()| {
                                                            push_char_budgeted(
                                                                &mut word, '}', budget,
                                                            )
                                                        })
                                                {
                                                    return Err(PartOutcome::Exhausted(e.reason));
                                                }
                                                match find_substitutions_budgeted(body, budget) {
                                                    Ok(subs) => cur.subs.extend(subs),
                                                    Err(e) => {
                                                        return Err(PartOutcome::Exhausted(
                                                            e.reason,
                                                        ));
                                                    }
                                                }
                                                i = end;
                                            }
                                            Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
                                        }
                                    }
                                    _ => {
                                        let ch = text[i..].chars().next().unwrap_or(' ');
                                        if let Err(e) = push_char_budgeted(&mut word, ch, budget) {
                                            return Err(PartOutcome::Exhausted(e.reason));
                                        }
                                        i += ch.len_utf8();
                                    }
                                }
                            }
                        }
                        b'$' if bytes.get(i + 1) == Some(&b'(') => {
                            let dbl = bytes.get(i + 2) == Some(&b'(');
                            if let Err(e) = budget.charge(1) {
                                return Err(PartOutcome::Exhausted(e.reason));
                            }
                            match scan_paren_bytes(text, i + 2, 0, budget) {
                                Ok((body, end)) => {
                                    if let Err(e) = push_str_budgeted(&mut word, "$(", budget)
                                        .and_then(|()| push_str_budgeted(&mut word, body, budget))
                                        .and_then(|()| push_char_budgeted(&mut word, ')', budget))
                                    {
                                        return Err(PartOutcome::Exhausted(e.reason));
                                    }
                                    if dbl {
                                        match find_substitutions_budgeted(body, budget) {
                                            Ok(subs) => cur.subs.extend(subs),
                                            Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
                                        }
                                    }
                                    match clone_string_budgeted(body, budget) {
                                        Ok(s) => cur.subs.push(s),
                                        Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
                                    }
                                    i = end;
                                }
                                Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
                            }
                        }
                        b'$' if bytes.get(i + 1) == Some(&b'{') => {
                            if let Err(e) = budget.charge(1) {
                                return Err(PartOutcome::Exhausted(e.reason));
                            }
                            match scan_dollar_brace_bytes(text, i + 2, false, 0, budget) {
                                Ok((body, end)) => {
                                    if let Err(e) = push_str_budgeted(&mut word, "${", budget)
                                        .and_then(|()| push_str_budgeted(&mut word, body, budget))
                                        .and_then(|()| push_char_budgeted(&mut word, '}', budget))
                                    {
                                        return Err(PartOutcome::Exhausted(e.reason));
                                    }
                                    match find_substitutions_budgeted(body, budget) {
                                        Ok(subs) => cur.subs.extend(subs),
                                        Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
                                    }
                                    i = end;
                                }
                                Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
                            }
                        }
                        b'`' => match scan_backtick_bytes(text, i + 1, budget) {
                            Ok((body, end)) => {
                                if let Err(e) = push_char_budgeted(&mut word, '`', budget)
                                    .and_then(|()| push_str_budgeted(&mut word, &body, budget))
                                    .and_then(|()| push_char_budgeted(&mut word, '`', budget))
                                {
                                    return Err(PartOutcome::Exhausted(e.reason));
                                }
                                cur.subs.push(body);
                                i = end;
                            }
                            Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
                        },
                        b'<' | b'>' => {
                            // Process substitution `<(...)` / `>(...)`.
                            let dir = b as char;
                            if let Err(e) = budget.charge(1) {
                                return Err(PartOutcome::Exhausted(e.reason));
                            }
                            match scan_paren_bytes(text, i + 2, 0, budget) {
                                Ok((body, end)) => {
                                    if let Err(e) = push_char_budgeted(&mut word, dir, budget)
                                        .and_then(|()| push_char_budgeted(&mut word, '(', budget))
                                        .and_then(|()| push_str_budgeted(&mut word, body, budget))
                                        .and_then(|()| push_char_budgeted(&mut word, ')', budget))
                                        .and_then(|()| {
                                            clone_string_budgeted(body, budget)
                                                .map(|s| cur.subs.push(s))
                                        })
                                    {
                                        return Err(PartOutcome::Exhausted(e.reason));
                                    }
                                    i = end;
                                }
                                Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
                            }
                        }
                        b'=' if assign_eq.is_none()
                            && quoted_prefix_empty
                            && is_posix_name(word.as_bytes()) =>
                        {
                            assign_eq = Some(word.len());
                            if let Err(e) = push_char_budgeted(&mut word, '=', budget) {
                                return Err(PartOutcome::Exhausted(e.reason));
                            }
                            i += 1;
                        }
                        _ => {
                            let ch = text[i..].chars().next().unwrap_or(' ');
                            if let Err(e) = push_char_budgeted(&mut word, ch, budget) {
                                return Err(PartOutcome::Exhausted(e.reason));
                            }
                            i += ch.len_utf8();
                        }
                    }
                }

                if i == start_i {
                    i += 1;
                    continue;
                }

                // Digits directly before `<` or `>` are the redirect fd.
                if pending.is_none()
                    && !word.is_empty()
                    && word.bytes().all(|c| c.is_ascii_digit())
                    && matches!(bytes.get(i), Some(b'<' | b'>'))
                    && bytes.get(i + 1) != Some(&b'(')
                {
                    let before_hd = heredocs.len();
                    match lex_be_redirect(text, i, &mut pending, &mut heredocs, budget) {
                        Ok(new_i) => i = new_i,
                        Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
                    }
                    if heredocs.len() > before_hd {
                        heredoc_owners.push(raw_units.len());
                    }
                    continue;
                }

                match pending.take() {
                    Some(BePending::Redirect(kind)) => {
                        let fixed = if kind == BestEffortRedirectKind::Dup
                            && !(word == "-" || word.bytes().all(|c| c.is_ascii_digit()))
                        {
                            BestEffortRedirectKind::Write
                        } else {
                            kind
                        };
                        cur.redirects.push((fixed, word));
                    }
                    Some(BePending::HereString) => {
                        bodies.push((raw_units.len(), word));
                    }
                    None => {
                        if cur.raw_argv.is_empty()
                            && let Some(eq) = assign_eq
                        {
                            let k = match clone_string_budgeted(&word[..eq], budget) {
                                Ok(s) => s,
                                Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
                            };
                            let v = match clone_string_budgeted(&word[eq + 1..], budget) {
                                Ok(s) => s,
                                Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
                            };
                            cur.assignments.push((k, v));
                        } else {
                            cur.raw_argv.push(word);
                        }
                    }
                }
            }
        }
    }
    flush_builder(&mut cur, None, false, &mut raw_units, budget)?;

    // Convert raw builders into `BestEffortUnit`s + run bounded code scans.
    let mut raw_to_record_idx: Vec<usize> = Vec::with_capacity(raw_units.len());
    let mut deferred_payloads: Vec<String> = Vec::new();
    let mut pipeline_units: Vec<(PipelineId, usize)> = Vec::new();

    for b in raw_units {
        if record.units.len() >= limits::MAX_UNITS {
            return Err(PartOutcome::TooLarge(Cap::Units));
        }
        let (eff, alt) = match unwrap_best_effort(&b.raw_argv, budget) {
            Ok(v) => v,
            Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
        };

        // Check `$(which CMD)` / `$(command -v CMD)` / `$(type -p CMD)` / `$(echo CMD)`
        // command-word substitutions (CR-19 folded).
        let mut all_alt = alt;
        if let Some(first) = eff.first()
            && has_cmd_sub(first)
            && let Ok(Some(sub_name)) = substituted_program_budgeted(first, budget)
        {
            let mut replaced = match clone_vec_string_budgeted(&eff, budget) {
                Ok(v) => v,
                Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
            };
            replaced[0] = sub_name;
            if let Ok((sub_eff, sub_alt)) = unwrap_best_effort(&replaced, budget) {
                if !sub_eff.is_empty()
                    && sub_eff != eff
                    && !all_alt.contains(&sub_eff)
                    && all_alt.len() < MAX_ALT_READINGS
                {
                    all_alt.push(sub_eff);
                }
                for a in sub_alt {
                    if !a.is_empty()
                        && a != eff
                        && !all_alt.contains(&a)
                        && all_alt.len() < MAX_ALT_READINGS
                    {
                        all_alt.push(a);
                    }
                }
            }
        }

        // Collect bounded code payloads from `eff` + `all_alt` + `b.raw_argv` + `b.subs`.
        let mut payloads = Vec::new();
        let mut push_unique = |s: String| {
            if !s.is_empty() && !payloads.contains(&s) {
                payloads.push(s);
            }
        };
        for reading in std::iter::once(&eff).chain(all_alt.iter()) {
            match extract_code_payloads(reading, budget) {
                Ok(ps) => {
                    for p in ps {
                        push_unique(p);
                    }
                }
                Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
            }
        }
        // `env -S` / `env -C` inside raw_argv before unwrapping.
        match extract_env_payloads(&b.raw_argv, budget) {
            Ok(ps) => {
                for p in ps {
                    push_unique(p);
                }
            }
            Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
        }
        // `script -c` / `flock -c` inside raw_argv.
        match extract_script_flock_payloads(&b.raw_argv, budget) {
            Ok(ps) => {
                for p in ps {
                    push_unique(p);
                }
            }
            Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
        }

        // Extract `find -exec` / `-execdir` / `-ok` / `-okdir` sub-commands.
        let mut find_execs = Vec::new();
        for reading in std::iter::once(&eff).chain(all_alt.iter()) {
            if reading
                .first()
                .is_some_and(|f| fold_str_cr19(f, budget).ok().as_deref() == Some("find"))
            {
                match extract_find_exec_cmds(reading, budget) {
                    Ok(cmds) => find_execs.extend(cmds),
                    Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
                }
            }
        }

        // Process substitutions `<(...)` passed to shells/source/`.` via args or redirects.
        if std::iter::once(&eff).chain(all_alt.iter()).any(|r| {
            r.first().is_some_and(|f| {
                is_be_shell(f, budget).unwrap_or(false)
                    || matches!(
                        fold_str_cr19(f, budget).ok().as_deref(),
                        Some("source" | ".")
                    )
            })
        }) {
            for w in b
                .raw_argv
                .iter()
                .skip(1)
                .chain(b.redirects.iter().map(|(_, t)| t))
            {
                if let Some(inner) = w.strip_prefix("<(").and_then(|r| r.strip_suffix(')'))
                    && let Ok(s) = clone_string_budgeted(inner, budget)
                {
                    push_unique(s);
                }
            }
        }

        let unit_idx = record.units.len();
        raw_to_record_idx.push(unit_idx);
        if let Some(pid) = b.pipeline_id {
            pipeline_units.push((pid, unit_idx));
        }

        if let Err(e) = budget
            .charge(1)
            .and_then(|()| budget.charge_alloc(std::mem::size_of::<BestEffortUnit>()))
        {
            return Err(PartOutcome::Exhausted(e.reason));
        }
        record.units.push(BestEffortUnit {
            index: unit_idx,
            raw_argv: b.raw_argv,
            effective_argv: eff,
            alt_argv: all_alt,
            assignments: b.assignments,
            redirects: b.redirects,
            pipeline_id: b.pipeline_id,
            piped: b.piped,
            depth,
            derived,
        });

        // Push `find -exec` derived units.
        for cmd_argv in find_execs {
            if record.units.len() >= limits::MAX_UNITS {
                return Err(PartOutcome::TooLarge(Cap::Units));
            }
            let (f_eff, f_alt) = match unwrap_best_effort(&cmd_argv, budget) {
                Ok(v) => v,
                Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
            };
            if let Err(e) = budget
                .charge(1)
                .and_then(|()| budget.charge_alloc(std::mem::size_of::<BestEffortUnit>()))
            {
                return Err(PartOutcome::Exhausted(e.reason));
            }
            let idx = record.units.len();
            record.units.push(BestEffortUnit {
                index: idx,
                raw_argv: cmd_argv,
                effective_argv: f_eff,
                alt_argv: f_alt,
                assignments: Vec::new(),
                redirects: Vec::new(),
                pipeline_id: None,
                piped: false,
                depth: depth.saturating_add(1),
                derived: true,
            });
        }

        // Recurse on substitutions and extracted code payloads.
        for sub in b.subs {
            // Also check `$(echo ...)` / `$(printf ...)` outputs when used as code.
            deferred_payloads.push(sub);
        }
        for p in payloads {
            deferred_payloads.push(p);
        }
    }

    // Scan 7: Here-doc / here-string bodies fed to shells, `eval`, `source`,
    // `.`, interpreters, `crontab`, or piped commands.
    for (owner, body) in bodies {
        let owner_unit = raw_to_record_idx
            .get(owner)
            .and_then(|&idx| record.units.get(idx));
        let is_crontab = owner_unit.is_some_and(|u| {
            u.effective_argv
                .first()
                .is_some_and(|f| fold_str_cr19(f, budget).ok().as_deref() == Some("crontab"))
        });
        if is_crontab {
            match crontab_commands_budgeted(&body, budget) {
                Ok(cmds) if !cmds.is_empty() => deferred_payloads.push(cmds),
                Ok(_) => {}
                Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
            }
        }
        let feeds_code = match owner_unit {
            None => true,
            Some(u) => {
                u.pipeline_id.is_some()
                    || u.effective_argv.first().is_some_and(|f| {
                        let folded = fold_str_cr19(f, budget).ok();
                        let s = folded.as_deref().unwrap_or("");
                        is_be_shell(f, budget).unwrap_or(false)
                            || is_be_interpreter(s)
                            || matches!(s, "eval" | "source" | "." | "crontab")
                            || crate::policy::shell::COMMAND_RUNNERS.contains(&s)
                            || crate::policy::shell::RESERVED.contains(&f.as_str())
                    })
            }
        };
        if feeds_code {
            deferred_payloads.push(body);
        }
    }

    // Scan 8: `echo` / `printf` piped into a shell / `eval` / `source` / `crontab`.
    let mut checked_pipes: Vec<PipelineId> = Vec::new();
    for &(pid, idx) in &pipeline_units {
        if checked_pipes.contains(&pid) {
            continue;
        }
        let u = &record.units[idx];
        if !u.piped {
            continue;
        }
        let receives_code = u.effective_argv.first().is_some_and(|f| {
            let folded = fold_str_cr19(f, budget).ok();
            let s = folded.as_deref().unwrap_or("");
            is_be_shell(f, budget).unwrap_or(false)
                || matches!(s, "eval" | "source" | "." | "crontab")
        });
        if !receives_code {
            continue;
        }
        checked_pipes.push(pid);
        let is_crontab = u
            .effective_argv
            .first()
            .is_some_and(|f| fold_str_cr19(f, budget).ok().as_deref() == Some("crontab"));
        for &(other_pid, other_idx) in &pipeline_units {
            if other_pid != pid || other_idx == idx {
                continue;
            }
            let prev_eff = &record.units[other_idx].effective_argv;
            match extract_echo_printf_texts(prev_eff, budget) {
                Ok(texts) => {
                    for t in texts {
                        if is_crontab
                            && let Ok(cmds) = crontab_commands_budgeted(&t, budget)
                            && !cmds.is_empty()
                        {
                            deferred_payloads.push(cmds);
                        }
                        deferred_payloads.push(t);
                    }
                }
                Err(e) => return Err(PartOutcome::Exhausted(e.reason)),
            }
        }
    }

    // Recursively lex deferred payloads at `depth + 1`.
    if depth < limits::MAX_DEPTH {
        for payload in deferred_payloads {
            lex_one_text(&payload, depth + 1, true, record, state, budget)?;
        }
    }

    if derived && state.cache.len() < MAX_CALL_CACHE {
        let produced = record.units[start_unit_len..].to_vec();
        if let Ok(key_text) = clone_string_budgeted(text, budget) {
            state.cache.insert((key_text, depth), produced);
        }
    }

    Ok(())
}

fn lex_be_redirect(
    text: &str,
    mut i: usize,
    pending: &mut Option<BePending>,
    heredocs: &mut Vec<(String, bool, bool)>,
    budget: &mut ComputeBudget,
) -> Result<usize, Exhausted> {
    let bytes = text.as_bytes();
    let c0 = bytes[i];
    let c1 = bytes.get(i + 1).copied();
    let c2 = bytes.get(i + 2).copied();

    if c0 == b'<' && c1 == Some(b'<') && c2 == Some(b'<') {
        budget.charge(2)?;
        *pending = Some(BePending::HereString);
        return Ok(i + 3);
    }
    if c0 == b'<' && c1 == Some(b'<') {
        let strip = c2 == Some(b'-');
        budget.charge(if strip { 2 } else { 1 })?;
        i += if strip { 3 } else { 2 };
        while matches!(bytes.get(i), Some(b' ' | b'\t')) {
            budget.charge(1)?;
            i += 1;
        }
        let mut delim = String::new();
        let mut quoted = false;
        while i < bytes.len()
            && !matches!(
                bytes[i],
                b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'(' | b')' | b'<' | b'>'
            )
        {
            budget.charge(1)?;
            match bytes[i] {
                b'\'' | b'"' | b'\\' => {
                    quoted = true;
                    i += 1;
                }
                _ => {
                    let ch = text[i..].chars().next().unwrap_or(' ');
                    push_char_budgeted(&mut delim, ch, budget)?;
                    i += ch.len_utf8();
                }
            }
        }
        heredocs.push((delim, strip, quoted));
        return Ok(i);
    }

    let (kind, n) = match (c0, c1) {
        (b'>', Some(b'>')) => (BestEffortRedirectKind::Write, 2),
        (b'>', Some(b'&')) => (BestEffortRedirectKind::Dup, 2),
        (b'>', Some(b'|')) => (BestEffortRedirectKind::Write, 2),
        (b'>', _) => (BestEffortRedirectKind::Write, 1),
        (b'<', Some(b'>')) => (BestEffortRedirectKind::Write, 2),
        (b'<', Some(b'&')) => (BestEffortRedirectKind::Dup, 2),
        _ => (BestEffortRedirectKind::Read, 1),
    };
    if n > 1 {
        budget.charge((n - 1) as u64)?;
    }
    i += n;
    while matches!(bytes.get(i), Some(b' ' | b'\t')) {
        budget.charge(1)?;
        i += 1;
    }
    *pending = Some(BePending::Redirect(kind));
    Ok(i)
}

fn estimate_unit_bytes(u: &BestEffortUnit) -> usize {
    let mut n = std::mem::size_of::<BestEffortUnit>();
    for s in &u.raw_argv {
        n += s.len() + std::mem::size_of::<String>();
    }
    for s in &u.effective_argv {
        n += s.len() + std::mem::size_of::<String>();
    }
    for v in &u.alt_argv {
        for s in v {
            n += s.len() + std::mem::size_of::<String>();
        }
    }
    for (k, v) in &u.assignments {
        n += k.len() + v.len() + 2 * std::mem::size_of::<String>();
    }
    for (_, t) in &u.redirects {
        n += t.len() + std::mem::size_of::<String>();
    }
    n
}

// ---------------------------------------------------------------------------
// CR-19 helpers & tolerant wrapper unwrapping for best-effort pass (§2, §12)
// ---------------------------------------------------------------------------

#[inline]
fn fold_str_cr19(s: &str, budget: &mut ComputeBudget) -> Result<String, Exhausted> {
    fold_command_bytes(s.as_bytes(), &FoldRules::REV4, budget)
}

fn is_be_shell(cmd: &str, budget: &mut ComputeBudget) -> Result<bool, Exhausted> {
    if matches!(
        cmd,
        "$SHELL" | "${SHELL}" | "$BASH" | "${BASH}" | "$0" | "${0}"
    ) {
        return Ok(true);
    }
    let folded = fold_str_cr19(cmd, budget)?;
    Ok(crate::policy::shell::SHELLS.contains(&folded.as_str()))
}

fn is_be_interpreter(folded: &str) -> bool {
    folded.starts_with("python")
        || matches!(
            folded,
            "perl" | "ruby" | "node" | "deno" | "bun" | "php" | "lua" | "osascript" | "pwsh"
        )
}

#[inline]
fn has_cmd_sub(w: &str) -> bool {
    w.contains("$(") || w.contains('`')
}

/// Resolves simple `$(which CMD)` / `$(command -v CMD)` / `$(type -p CMD)` /
/// `$(echo CMD)` command-word substitutions with CR-19 folding.
fn substituted_program_budgeted(
    w: &str,
    budget: &mut ComputeBudget,
) -> Result<Option<String>, Exhausted> {
    let subs = find_substitutions_budgeted(w, budget)?;
    if subs.len() != 1 {
        return Ok(None);
    }
    let body = subs[0].trim();
    let parts: Vec<&str> = body.split_whitespace().collect();
    let candidate = match parts.as_slice() {
        [w0, name] => {
            let f = fold_str_cr19(w0, budget)?;
            if matches!(f.as_str(), "which" | "whence" | "echo") {
                Some(*name)
            } else {
                None
            }
        }
        [w0, "-v" | "-V", name] => {
            let f = fold_str_cr19(w0, budget)?;
            (f == "command").then_some(*name)
        }
        [w0, "-p" | "-P", name] => {
            let f = fold_str_cr19(w0, budget)?;
            (f == "type").then_some(*name)
        }
        _ => None,
    };
    match candidate {
        Some(c) if !c.is_empty() && !c.contains(['$', '`', '*', '?', '[']) => {
            Ok(Some(clone_string_budgeted(c, budget)?))
        }
        _ => Ok(None),
    }
}

/// Tolerantly strips reserved words and wrappers (CR-19 folded) up to
/// `MAX_BEST_EFFORT_WRAP` steps, returning `(effective_argv, alt_argv)`.
fn unwrap_best_effort(
    argv: &[String],
    budget: &mut ComputeBudget,
) -> Result<(Vec<String>, Vec<Vec<String>>), Exhausted> {
    let mut cur = clone_vec_string_budgeted(argv, budget)?;
    let mut alts: Vec<Vec<String>> = Vec::new();

    for _ in 0..MAX_BEST_EFFORT_WRAP {
        let Some(first) = cur.first() else {
            break;
        };
        budget.charge(1)?;
        // Reserved words match un-folded basename.
        let base_raw = first.rsplit('/').next().unwrap_or(first.as_str());
        if matches!(
            base_raw,
            "{" | "}" | "!" | "do" | "then" | "else" | "elif" | "if" | "while" | "until" | "coproc"
        ) {
            cur.remove(0);
            continue;
        }
        let folded = fold_str_cr19(first, budget)?;
        let Some((skip, maybe_alt_skip)) = wrapper_skip_be(&folded, &cur, budget)? else {
            break;
        };
        if let Some(alt_skip) = maybe_alt_skip
            && alt_skip != skip
            && alt_skip < cur.len()
            && alts.len() < MAX_ALT_READINGS
        {
            let alt_slice = clone_vec_string_budgeted(&cur[alt_skip..], budget)?;
            if !alt_slice.is_empty() && !alts.contains(&alt_slice) {
                alts.push(alt_slice);
            }
        }
        if skip == 0 || skip > cur.len() {
            break;
        }
        cur.drain(..skip);
    }

    Ok((cur, alts))
}

/// Returns `Ok(Some((primary_skip, optional_alt_skip)))` for a recognized
/// best-effort wrapper, or `Ok(None)` when `folded` is not a wrapper.
fn wrapper_skip_be(
    folded: &str,
    argv: &[String],
    budget: &mut ComputeBudget,
) -> Result<Option<(usize, Option<usize>)>, Exhausted> {
    let is_opt = |w: &str| w.starts_with('-') && w.len() > 1;
    let mut j = 1usize;
    let mut alt_j: Option<usize> = None;

    match folded {
        "nohup" | "builtin" | "busybox" | "toybox" => return Ok(Some((1, None))),
        "command" => {
            while j < argv.len() && is_opt(&argv[j]) {
                budget.charge(1)?;
                if argv[j] == "--" {
                    j += 1;
                    break;
                }
                if argv[j].strip_prefix('-').is_some_and(|s| {
                    !s.is_empty() && s.bytes().all(|b| matches!(b, b'p' | b'v' | b'V'))
                }) {
                    j += 1;
                } else {
                    if !argv[j].contains('=') && j + 1 < argv.len() && alt_j.is_none() {
                        alt_j = Some(j + 2);
                    }
                    j += 1;
                }
            }
            return Ok(Some((j, alt_j)));
        }
        "exec" => {
            while j < argv.len() && is_opt(&argv[j]) {
                budget.charge(1)?;
                if argv[j] == "--" {
                    j += 1;
                    break;
                }
                if argv[j] == "-a" {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            return Ok(Some((j.min(argv.len()), None)));
        }
        "nice" => {
            while j < argv.len() && is_opt(&argv[j]) {
                budget.charge(1)?;
                let w = argv[j].as_str();
                if w == "--" {
                    j += 1;
                    break;
                }
                if w == "-n" || w == "--adjustment" {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            return Ok(Some((j.min(argv.len()), None)));
        }
        "timeout" => {
            while j < argv.len() && is_opt(&argv[j]) {
                budget.charge(1)?;
                let w = argv[j].as_str();
                if w == "--" {
                    j += 1;
                    break;
                }
                if matches!(w, "-s" | "-k" | "--signal" | "--kill-after") {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            // Skip duration operand `D`.
            let skip = (j + 1).min(argv.len());
            return Ok(Some((skip, None)));
        }
        "time" => {
            while j < argv.len() && is_opt(&argv[j]) {
                budget.charge(1)?;
                let w = argv[j].as_str();
                if w == "--" {
                    j += 1;
                    break;
                }
                if matches!(w, "-f" | "-o" | "--format" | "--output") {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            return Ok(Some((j.min(argv.len()), None)));
        }
        "env" => {
            while j < argv.len() {
                budget.charge(1)?;
                let w = argv[j].as_str();
                if w == "--" {
                    j += 1;
                    break;
                }
                if w == "-" {
                    j += 1;
                    continue;
                }
                if is_opt(w) {
                    if matches!(
                        w,
                        "-u" | "-C"
                            | "-P"
                            | "-a"
                            | "-S"
                            | "--unset"
                            | "--chdir"
                            | "--split-string"
                            | "--argv0"
                    ) {
                        j += 2;
                    } else if w.starts_with("--")
                        || w.starts_with("-i")
                        || w.starts_with("-0")
                        || w.starts_with("-v")
                        || w.starts_with("-u")
                        || w.starts_with("-C")
                        || w.starts_with("-S")
                    {
                        j += 1;
                    } else {
                        if !w.contains('=') && j + 1 < argv.len() && alt_j.is_none() {
                            alt_j = Some(j + 2);
                        }
                        j += 1;
                    }
                    continue;
                }
                if let Some((name, _)) = w.split_once('=')
                    && is_posix_name(name.as_bytes())
                {
                    j += 1;
                    continue;
                }
                break;
            }
            return Ok(Some((j.min(argv.len()), alt_j)));
        }
        "chroot" => {
            while j < argv.len() && is_opt(&argv[j]) {
                budget.charge(1)?;
                let w = argv[j].as_str();
                if w == "--" {
                    j += 1;
                    break;
                }
                if matches!(w, "-G" | "-g" | "-u" | "--userspec" | "--groups") {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            // Skip NEWROOT operand.
            return Ok(Some(((j + 1).min(argv.len()), None)));
        }
        "arch" => {
            while j < argv.len() && is_opt(&argv[j]) {
                budget.charge(1)?;
                let w = argv[j].as_str();
                if matches!(w, "-arch" | "-e" | "-d") {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            return Ok(Some((j.min(argv.len()), None)));
        }
        "setsid" | "unbuffer" | "caffeinate" | "sandbox-exec" | "ionice" | "stdbuf" | "xargs"
        | "chrt" | "taskset" => {
            let takes_val = |w: &str| match folded {
                "caffeinate" => matches!(w, "-t" | "-w"),
                "sandbox-exec" => matches!(w, "-f" | "-n" | "-p" | "-D"),
                "ionice" => matches!(
                    w,
                    "-c" | "-n"
                        | "-p"
                        | "-P"
                        | "-u"
                        | "--class"
                        | "--classdata"
                        | "--pid"
                        | "--pgid"
                        | "--uid"
                ),
                "stdbuf" => matches!(w, "-i" | "-o" | "-e" | "--input" | "--output" | "--error"),
                "xargs" => matches!(
                    w,
                    "-n" | "-P"
                        | "-I"
                        | "-L"
                        | "-s"
                        | "-d"
                        | "-a"
                        | "-E"
                        | "-J"
                        | "-R"
                        | "-S"
                        | "--max-args"
                        | "--max-procs"
                        | "--max-chars"
                        | "--delimiter"
                        | "--arg-file"
                ),
                "chrt" => matches!(
                    w,
                    "-T" | "-P" | "-D" | "--sched-runtime" | "--sched-period" | "--sched-deadline"
                ),
                _ => false,
            };
            while j < argv.len() && is_opt(&argv[j]) {
                budget.charge(1)?;
                let w = argv[j].as_str();
                if w == "--" {
                    j += 1;
                    break;
                }
                if takes_val(w) {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            let extra = usize::from(matches!(folded, "chrt" | "taskset"));
            return Ok(Some(((j + extra).min(argv.len()), None)));
        }
        "flock" | "script" => {
            // `flock -c CMD` / `script -c CMD` payloads are extracted separately;
            // non-`-c` forms take 1 file operand before the command.
            while j < argv.len() && is_opt(&argv[j]) {
                budget.charge(1)?;
                let w = argv[j].as_str();
                if w == "--" {
                    j += 1;
                    break;
                }
                if matches!(w, "-c" | "--command") || w.starts_with("--command=") {
                    return Ok(None);
                }
                if matches!(
                    w,
                    "-w" | "-E"
                        | "--timeout"
                        | "--wait"
                        | "--conflict-exit-code"
                        | "-t"
                        | "-T"
                        | "-I"
                        | "-O"
                        | "-B"
                        | "-m"
                        | "-o"
                        | "--log-io"
                        | "--log-in"
                        | "--log-out"
                ) {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            return Ok(Some(((j + 1).min(argv.len()), None)));
        }
        _ => {}
    }

    Ok(None)
}

// ---------------------------------------------------------------------------
// Bounded code scans for best-effort pass (listed one by one in handoff)
// ---------------------------------------------------------------------------

/// Scan 1 (`sh`/`bash`/`zsh`/`ksh`/`fish`/… `-c` / `--command` / `-C`) and
/// Scan 2 (`eval` arguments). No K7 positional resolution (dropped per §4, §12).
fn extract_code_payloads(
    eff: &[String],
    budget: &mut ComputeBudget,
) -> Result<Vec<String>, Exhausted> {
    let mut out = Vec::new();
    let Some(first) = eff.first() else {
        return Ok(out);
    };
    let folded = fold_str_cr19(first, budget)?;

    if is_be_shell(first, budget)? {
        let mut seen_c = false;
        let mut j = 1usize;
        while j < eff.len() {
            budget.charge(1)?;
            let w = eff[j].as_str();
            if w == "--" {
                j += 1;
                break;
            }
            if let Some(rest) = w
                .strip_prefix("--command=")
                .or_else(|| w.strip_prefix("--init-command="))
            {
                out.push(clone_string_budgeted(rest, budget)?);
                j += 1;
                continue;
            }
            if w == "--command" {
                seen_c = true;
                j += 1;
                continue;
            }
            if matches!(w, "--init-command" | "-C") && folded == "fish" {
                if let Some(code) = eff.get(j + 1) {
                    out.push(clone_string_budgeted(code, budget)?);
                }
                j += 2;
                continue;
            }
            if matches!(w, "--rcfile" | "--init-file" | "--emulate") {
                j += 2;
                continue;
            }
            if w.starts_with("--") {
                j += 1;
                continue;
            }
            if (w.starts_with('-') || w.starts_with('+')) && w.len() > 1 {
                let letters = &w[1..];
                if w.starts_with('-') && letters.contains('c') {
                    seen_c = true;
                }
                let extra = letters
                    .bytes()
                    .filter(|&b| matches!(b, b'o' | b'O'))
                    .count();
                j += 1 + extra;
                continue;
            }
            break;
        }
        if seen_c && let Some(code) = eff.get(j) {
            out.push(clone_string_budgeted(code, budget)?);
        }
    } else if folded == "eval" {
        let args = match eff.get(1).map(String::as_str) {
            Some("--") => &eff[2..],
            _ => &eff[1..],
        };
        if !args.is_empty() {
            let total_len: usize = args.iter().map(|s| s.len() + 1).sum();
            budget.charge(total_len as u64)?;
            budget.charge_alloc(total_len)?;
            out.push(args.join(" "));
        }
    }

    Ok(out)
}

/// Scan 3: `env -S` / `--split-string` (and attached `-S...`) payloads.
fn extract_env_payloads(
    argv: &[String],
    budget: &mut ComputeBudget,
) -> Result<Vec<String>, Exhausted> {
    let mut out = Vec::new();
    let Some(first) = argv.first() else {
        return Ok(out);
    };
    if fold_str_cr19(first, budget)? != "env" {
        return Ok(out);
    }
    let mut j = 1usize;
    while j < argv.len() {
        budget.charge(1)?;
        let w = argv[j].as_str();
        if w == "--" {
            break;
        }
        if w == "-S" || w == "--split-string" {
            if let Some(val) = argv.get(j + 1) {
                let mut combined = clone_string_budgeted(val, budget)?;
                for rest in &argv[(j + 2).min(argv.len())..] {
                    push_char_budgeted(&mut combined, ' ', budget)?;
                    push_shell_single_quoted(&mut combined, rest, budget)?;
                }
                out.push(combined);
            }
            break;
        }
        if let Some(val) = w
            .strip_prefix("--split-string=")
            .or_else(|| w.strip_prefix("-S").filter(|r| !r.is_empty()))
        {
            let mut combined = clone_string_budgeted(val, budget)?;
            for rest in &argv[(j + 1).min(argv.len())..] {
                push_char_budgeted(&mut combined, ' ', budget)?;
                push_shell_single_quoted(&mut combined, rest, budget)?;
            }
            out.push(combined);
            break;
        }
        if matches!(
            w,
            "-u" | "-C" | "-P" | "-a" | "--unset" | "--chdir" | "--argv0"
        ) {
            j += 2;
        } else if w.starts_with('-') || w.contains('=') {
            j += 1;
        } else {
            break;
        }
    }
    Ok(out)
}

fn push_shell_single_quoted(
    out: &mut String,
    s: &str,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    push_char_budgeted(out, '\'', budget)?;
    for ch in s.chars() {
        if ch == '\'' {
            push_str_budgeted(out, "'\\''", budget)?;
        } else {
            push_char_budgeted(out, ch, budget)?;
        }
    }
    push_char_budgeted(out, '\'', budget)
}

/// Scan 4: `script -c` / `--command` and `flock -c` / `--command`.
fn extract_script_flock_payloads(
    argv: &[String],
    budget: &mut ComputeBudget,
) -> Result<Vec<String>, Exhausted> {
    let mut out = Vec::new();
    let Some(first) = argv.first() else {
        return Ok(out);
    };
    let folded = fold_str_cr19(first, budget)?;
    if !matches!(folded.as_str(), "script" | "flock") {
        return Ok(out);
    }
    let mut j = 1usize;
    while j < argv.len() {
        budget.charge(1)?;
        let w = argv[j].as_str();
        if w == "--" {
            break;
        }
        if w == "-c" || w == "--command" {
            if let Some(cmd) = argv.get(j + 1) {
                out.push(clone_string_budgeted(cmd, budget)?);
            }
            j += 2;
        } else if let Some(cmd) = w
            .strip_prefix("--command=")
            .or_else(|| w.strip_prefix("-c").filter(|r| !r.is_empty()))
        {
            out.push(clone_string_budgeted(cmd, budget)?);
            j += 1;
        } else {
            j += 1;
        }
    }
    Ok(out)
}

/// Scan 5: `find -exec` / `-execdir` / `-ok` / `-okdir` sub-commands.
fn extract_find_exec_cmds(
    eff: &[String],
    budget: &mut ComputeBudget,
) -> Result<Vec<Vec<String>>, Exhausted> {
    let mut out = Vec::new();
    let mut j = 1usize;
    while j < eff.len() {
        budget.charge(1)?;
        if matches!(eff[j].as_str(), "-exec" | "-execdir" | "-ok" | "-okdir") {
            j += 1;
            let start = j;
            while j < eff.len() && !matches!(eff[j].as_str(), ";" | "+") {
                budget.charge(1)?;
                j += 1;
            }
            if j > start {
                out.push(clone_vec_string_budgeted(&eff[start..j], budget)?);
            }
        }
        j += 1;
    }
    Ok(out)
}

/// Scan 7 helper: extract command column from crontab lines.
fn crontab_commands_budgeted(text: &str, budget: &mut ComputeBudget) -> Result<String, Exhausted> {
    let mut out = String::new();
    for line in text.lines() {
        budget.charge(line.len().max(1) as u64)?;
        let t = line.trim_start();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let cmd = if t.starts_with('@') {
            t.split_once(char::is_whitespace)
                .map(|(_, r)| r.trim_start())
        } else {
            let mut fields = 0usize;
            let mut rest = t;
            while fields < 5 {
                let Some((_, after)) = rest.split_once(char::is_whitespace) else {
                    rest = "";
                    break;
                };
                rest = after.trim_start();
                fields += 1;
            }
            (fields == 5 && !rest.is_empty()).then_some(rest)
        };
        if let Some(c) = cmd {
            push_str_budgeted(&mut out, c, budget)?;
            push_char_budgeted(&mut out, '\n', budget)?;
        }
    }
    Ok(out)
}

/// Scan 8 helper: extract text operands from `echo` / `printf` (CR-19 folded).
fn extract_echo_printf_texts(
    eff: &[String],
    budget: &mut ComputeBudget,
) -> Result<Vec<String>, Exhausted> {
    let mut out = Vec::new();
    let Some(first) = eff.first() else {
        return Ok(out);
    };
    let folded = fold_str_cr19(first, budget)?;
    match folded.as_str() {
        "echo" => {
            let mut j = 1usize;
            while j < eff.len() {
                budget.charge(1)?;
                let w = eff[j].as_str();
                if w.starts_with('-')
                    && w.len() > 1
                    && w[1..].bytes().all(|b| matches!(b, b'n' | b'e' | b'E'))
                {
                    j += 1;
                } else {
                    break;
                }
            }
            if j < eff.len() {
                let joined = eff[j..].join(" ");
                budget.charge(joined.len() as u64)?;
                budget.charge_alloc(joined.len())?;
                out.push(joined);
            }
        }
        "printf" => {
            let mut j = 1usize;
            if eff.get(j).map(String::as_str) == Some("--") {
                j += 1;
            }
            for w in &eff[j.min(eff.len())..] {
                out.push(clone_string_budgeted(w, budget)?);
            }
        }
        _ => {}
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Scan 9: N11 bounded brace expansion (M1-31, best-effort only, budgeted)
// ---------------------------------------------------------------------------

type BraceWords = Result<Vec<String>, Vec<usize>>;

/// Bounded brace expansion for the best-effort pass (carried from M1-31 N11,
/// adapted to [`ComputeBudget`]).
///
/// - Returns `Ok(Some(words))` within `MAX_BRACE_RESULTS` (256), `MAX_SEQUENCE`
///   (64), and `MAX_BRACE_WORK` (2_000_000).
/// - Returns `Ok(None)` when the expansion would exceed those structural bounds.
/// - Returns `Err(Exhausted)` if `budget` runs out.
/// - Linear on deeply nested braces (does not re-copy nested interiors).
/// - Skips the lower-bound shortcut when `$` is present so `{,$}`×9 is not
///   falsely rejected (M1-31 review 1, finding 10).
/// - Safe `i64` arithmetic so `{i64::MAX..i64::MIN}` never overflows (M1-31
///   review 1, finding 6).
pub(crate) fn brace_expand_budgeted(
    w: &str,
    budget: &mut ComputeBudget,
) -> Result<Option<Vec<String>>, Exhausted> {
    budget.charge(w.len().max(1) as u64)?;
    if !w.contains('{') {
        return Ok(Some(vec![clone_string_budgeted(w, budget)?]));
    }
    if !w.contains('$') && brace_results_lower_bound(w, budget)? > MAX_BRACE_RESULTS {
        return Ok(None);
    }

    let mut work = vec![clone_string_budgeted(w, budget)?];
    let mut out = Vec::new();
    let mut scanned = 0usize;

    while let Some(x) = work.pop() {
        scanned = scanned.saturating_add(x.len());
        if scanned > MAX_BRACE_WORK {
            return Ok(None);
        }
        budget.charge(x.len().max(1) as u64)?;
        let cs: Vec<char> = x.chars().collect();
        budget.charge_alloc(cs.len() * std::mem::size_of::<char>())?;

        // (open, commas, holds_nested_pair)
        let mut stack: Vec<(usize, Vec<usize>, bool)> = Vec::new();
        let mut best: Option<(usize, usize, BraceWords)> = None;

        for (i, &c) in cs.iter().enumerate() {
            match c {
                '{' => stack.push((i, Vec::new(), false)),
                ',' => {
                    if let Some(top) = stack.last_mut() {
                        top.1.push(i);
                    }
                }
                '}' => {
                    let Some((open, commas, nested)) = stack.pop() else {
                        continue;
                    };
                    if let Some(parent) = stack.last_mut() {
                        parent.2 = true;
                    }
                    if (open > 0 && cs[open - 1] == '$')
                        || best.as_ref().is_some_and(|b| open >= b.0)
                    {
                        continue;
                    }
                    let alts = if commas.is_empty() {
                        if nested {
                            continue;
                        }
                        let inner: String = cs[open + 1..i].iter().collect();
                        match brace_sequence(&inner, budget)? {
                            Some(v) => Ok(v),
                            None => continue,
                        }
                    } else {
                        Err(commas)
                    };
                    best = Some((open, i, alts));
                }
                _ => {}
            }
        }

        match best {
            None => out.push(x),
            Some((open, close, alts)) => {
                let alts: Vec<String> = match alts {
                    Ok(v) => v,
                    Err(commas) => {
                        let mut bounds = Vec::with_capacity(commas.len() + 2);
                        bounds.push(open);
                        bounds.extend(commas);
                        bounds.push(close);
                        let mut v = Vec::with_capacity(bounds.len() - 1);
                        for k in 0..bounds.len() - 1 {
                            let s: String = cs[bounds[k] + 1..bounds[k + 1]].iter().collect();
                            budget.charge(s.len().max(1) as u64)?;
                            budget.charge_alloc(s.len() + std::mem::size_of::<String>())?;
                            v.push(s);
                        }
                        v
                    }
                };
                if work.len() + out.len() + alts.len() > MAX_BRACE_RESULTS {
                    return Ok(None);
                }
                let pre: String = cs[..open].iter().collect();
                let post: String = cs[close + 1..].iter().collect();
                for alt in alts {
                    let len = pre.len() + alt.len() + post.len();
                    budget.charge(len.max(1) as u64)?;
                    budget.charge_alloc(len + std::mem::size_of::<String>())?;
                    let mut combined = String::with_capacity(len);
                    combined.push_str(&pre);
                    combined.push_str(&alt);
                    combined.push_str(&post);
                    work.push(combined);
                }
            }
        }

        if work.len() + out.len() > MAX_BRACE_RESULTS {
            return Ok(None);
        }
    }

    Ok(Some(out))
}

fn brace_sequence(
    inner: &str,
    budget: &mut ComputeBudget,
) -> Result<Option<Vec<String>>, Exhausted> {
    budget.charge(inner.len().max(1) as u64)?;
    let parts: Vec<&str> = inner.split("..").collect();
    if !(2..=3).contains(&parts.len()) {
        return Ok(None);
    }
    let step: i128 = match parts.get(2) {
        Some(s) => {
            let Ok(v) = s.parse::<i64>() else {
                return Ok(None);
            };
            i128::from(v).abs().max(1)
        }
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
    let mut out = Vec::new();
    if let (Some(x), Some(y)) = (letter(a), letter(b)) {
        let (x, y) = (i128::from(x as u8), i128::from(y as u8));
        let dir = if x <= y { 1i128 } else { -1i128 };
        let mut v = x;
        while (dir == 1 && v <= y) || (dir == -1 && v >= y) {
            if out.len() >= MAX_SEQUENCE {
                return Ok(None);
            }
            let Ok(byte) = u8::try_from(v) else {
                return Ok(None);
            };
            let s = char::from(byte).to_string();
            budget.charge(1)?;
            budget.charge_alloc(1 + std::mem::size_of::<String>())?;
            out.push(s);
            v += dir * step;
        }
        return Ok(Some(out));
    }

    let parse_int = |s: &str| -> Option<i64> {
        let d = s.strip_prefix(['-', '+']).unwrap_or(s);
        if d.is_empty() || !d.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        s.parse::<i64>().ok()
    };
    let (Some(x), Some(y)) = (parse_int(a), parse_int(b)) else {
        return Ok(None);
    };
    let padded = |s: &str| {
        let d = s.strip_prefix('-').unwrap_or(s);
        d.len() > 1 && d.starts_with('0')
    };
    let width = if padded(a) || padded(b) {
        a.len().max(b.len())
    } else {
        0
    };
    // Use `i128` arithmetic so `{i64::MAX..i64::MIN}` never overflows.
    let (x, y) = (i128::from(x), i128::from(y));
    let dir = if x <= y { 1i128 } else { -1i128 };
    let mut v = x;
    while (dir == 1 && v <= y) || (dir == -1 && v >= y) {
        if out.len() >= MAX_SEQUENCE {
            return Ok(None);
        }
        let s = if v < 0 {
            format!("-{:0>w$}", -v, w = width.saturating_sub(1))
        } else {
            format!("{v:0>width$}")
        };
        budget.charge(s.len().max(1) as u64)?;
        budget.charge_alloc(s.len() + std::mem::size_of::<String>())?;
        out.push(s);
        v += dir * step;
    }
    Ok(Some(out))
}

fn brace_results_lower_bound(w: &str, budget: &mut ComputeBudget) -> Result<usize, Exhausted> {
    budget.charge(w.len().max(1) as u64)?;
    let cs: Vec<char> = w.chars().collect();
    budget.charge_alloc(cs.len() * std::mem::size_of::<char>())?;
    let mut stack: Vec<(usize, usize, bool)> = Vec::new();
    let mut pairs: Vec<(usize, usize, usize, bool)> = Vec::new();
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
                    return Ok(bound);
                }
            }
        }
    }
    Ok(bound)
}
