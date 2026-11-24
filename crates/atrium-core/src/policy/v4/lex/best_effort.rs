//! Tolerant best-effort shell lexer and bounded brace expander (owned by P1;
//! contract §2, §8, §12).
//!
//! Produces [`BestEffortUnit`]s — including units derived from `-c` scripts,
//! `eval`, `env -S`, `script -c`, `flock -c`, `find -exec`, substitutions,
//! here-docs/here-strings, and piped `echo`/`printf` — under the shared
//! [`ComputeBudget`], strictly linear in the line length, with CR-19 command
//! name folding. Kept strictly separate from the exact model.
//!
//! Follows the budget discipline in `v4::lex`'s module docs. In particular:
//! - a builder's word, redirect target or here-string has its slot reserved
//!   before the word is scanned, and the copy of the word that the unit's
//!   effective reading (or recovery) makes later is charged when the word is
//!   stored, so a finished word is never lost and recovery allocates nothing;
//! - `raw_units` keeps a free slot for the pending builder and
//!   `record.units` keeps free slots for every builder not yet converted;
//! - payload deduplication uses [`PayloadSet`], a hash index charged per
//!   hashed byte, per probe and per compared byte (review round 3, finding 3).

use std::cell::RefCell;
use std::rc::Rc;

use crate::policy::limits;
use crate::policy::model::{
    BestEffortRecord, BestEffortRedirectKind, BestEffortUnit, Cap, ComputeBudget, Exhausted,
    FoldRules, PartOutcome, PipelineId,
};
use crate::policy::tools::{ToolRowKind, ToolSpec};

use super::{
    AssignPrefix, bytes_eq, charge_string_growth, charge_vec_growth, fold_command_bytes,
    is_posix_name, lookup_outside, lookup_tool, push_reserved, vec_extend_budgeted,
    vec_push_budgeted, vec_with_capacity_budgeted,
};

/// Maximum entries in the thread-local best-effort lexer memo (carried from
/// M1-31 N11, adapted to preserve exact deterministic `ComputeBudget` charges).
const LEX_MEMO: usize = 4;

struct LexMemoEntry {
    line: String,
    units: Rc<Vec<BestEffortUnit>>,
    /// [`estimate_units_bytes`] of `units`, stored so a hit need not walk
    /// the units again to size its copy.
    units_bytes: usize,
    pipe_count: u32,
    compute_units: u64,
    alloc_bytes: usize,
}

thread_local! {
    static BE_LEX_MEMO: RefCell<Vec<LexMemoEntry>> = const { RefCell::new(Vec::new()) };
}

/// Maximum nesting depth for scanning nested quoting/substitution delimiters
/// (`$(` inside `"` inside `${` …) before giving up linearly.
const MAX_SCAN_NEST: u16 = 64;

/// Maximum consecutive transparent wrappers unwrapped in the best-effort pass
/// (covers §10 row 5 `command` ×13 `sudo id` while remaining strictly bounded).
const MAX_BEST_EFFORT_WRAP: usize = 32;

/// Maximum alternative readings kept per unit when a wrapper has unknown flags.
const MAX_ALT_READINGS: usize = 8;

/// Maximum brace-expanded words produced from one token (N11, best-effort only).
const MAX_BRACE_RESULTS: usize = 256;

/// Maximum sequence items in `{x..y}` (N11, best-effort only).
const MAX_SEQUENCE: usize = 64;

/// Maximum total characters scanned inside `brace_expand_budgeted` (N11).
const MAX_BRACE_WORK: usize = 2_000_000;

/// Computes `(copy_units, delta_bytes)` that `charge_vec_growth` would charge
/// for adding `additional` elements to `v`, without mutating `v` or `budget`.
#[inline]
fn preview_vec_growth<T>(v: &Vec<T>, additional: usize) -> (u64, usize) {
    let needed = v.len().saturating_add(additional);
    if needed <= v.capacity() {
        return (0, 0);
    }
    let elem_size = std::mem::size_of::<T>();
    if elem_size == 0 {
        return (0, 0);
    }
    let old_cap = v.capacity();
    let min_cap = if elem_size == 1 { 8 } else { 4 };
    let target_cap = old_cap.saturating_mul(2).max(needed).max(min_cap);
    let delta_bytes = target_cap.saturating_sub(old_cap).saturating_mul(elem_size);
    (old_cap as u64, delta_bytes)
}

/// Entry point shared by `lex_best_effort` and `build_units`. Populates
/// `record` incrementally so anything discovered before `Exhausted` is kept.
pub(crate) fn lex_best_effort_into(
    raw_texts: &[&str],
    record: &mut BestEffortRecord,
    budget: &mut ComputeBudget,
) -> Result<(), PartOutcome> {
    budget
        .charge(raw_texts.len() as u64 + 1)
        .map_err(|e| PartOutcome::Exhausted(e.reason))?;
    let total_bytes: usize = raw_texts
        .iter()
        .fold(0usize, |acc, s| acc.saturating_add(s.len()));
    if total_bytes > limits::MAX_BYTES {
        return Err(PartOutcome::TooLarge(Cap::Bytes));
    }

    let mut state = BeState { next_pipe_id: 1 };

    for &text in raw_texts {
        if let Err(e) = budget
            .charge(text.len() as u64)
            .and_then(|()| budget.charge_alloc(text.len()))
            .and_then(|()| charge_vec_growth(&mut record.raw_texts, 1, budget))
        {
            return Err(PartOutcome::Exhausted(e.reason));
        }
        record.raw_texts.push(text.to_string());

        // Check the N11 thread-local memo (M1-31): hit only when the remaining
        // budget can afford the exact recorded compute + allocation cost (and
        // any vector growth + unit copy cost), so budget accounting is 100%
        // deterministic and tight budgets still run incrementally and preserve
        // partial units found before exhaustion (§8). The lookup's line
        // comparisons are charged before they run.
        let memo_hit = memo_lookup(text, budget).map_err(|e| PartOutcome::Exhausted(e.reason))?;
        if let Some((cached_units, units_bytes, pipe_count, cost_units, cost_alloc)) = memo_hit {
            let (vec_copy_units, vec_growth_bytes) =
                preview_vec_growth(&record.units, cached_units.len());
            let need_units =
                cost_units.max((units_bytes.max(1) as u64).saturating_add(vec_copy_units));
            let need_alloc = cost_alloc.max(units_bytes.saturating_add(vec_growth_bytes));
            if !budget.is_exhausted()
                && budget.remaining_units() >= need_units
                && budget.remaining_alloc_bytes() >= need_alloc
            {
                if record.units.len() + cached_units.len() > limits::MAX_UNITS {
                    return Err(PartOutcome::TooLarge(Cap::Units));
                }
                // Charge full hit cost (covering `record.units` growth + clones)
                // before reserving or cloning.
                if let Err(e) = budget
                    .charge(need_units)
                    .and_then(|()| budget.charge_alloc(need_alloc))
                {
                    return Err(PartOutcome::Exhausted(e.reason));
                }
                if record.units.len().saturating_add(cached_units.len()) > record.units.capacity() {
                    let old_cap = record.units.capacity();
                    let target_cap = old_cap
                        .saturating_mul(2)
                        .max(record.units.len().saturating_add(cached_units.len()))
                        .max(4);
                    record.units.reserve_exact(target_cap - record.units.len());
                }
                let base_pipe = state.next_pipe_id;
                state.next_pipe_id = state.next_pipe_id.saturating_add(pipe_count);
                for u in cached_units.iter() {
                    let mut cloned = u.clone();
                    cloned.index = record.units.len();
                    cloned.pipeline_id = u
                        .pipeline_id
                        .map(|PipelineId(rel)| PipelineId(base_pipe.saturating_add(rel)));
                    push_reserved(&mut record.units, cloned);
                }
                continue;
            }
        }

        let start_units_rem = budget.remaining_units();
        let start_alloc_rem = budget.remaining_alloc_bytes();
        let start_rec_len = record.units.len();
        let start_pipe_id = state.next_pipe_id;
        lex_one_text(text, 0, false, record, &mut state, budget)?;
        let pipe_count = state.next_pipe_id.saturating_sub(start_pipe_id);

        // Optional memo insertion: charge sizing, the complete clone, the key,
        // the `Rc` and the memo's own growth BEFORE performing them; skip
        // insertion (without exhausting) when unaffordable (§8).
        let produced_slice = &record.units[start_rec_len..];
        let Some(units_bytes) = estimate_units_bytes_if_affordable(produced_slice, budget) else {
            continue;
        };
        let memo_growth = BE_LEX_MEMO.with(|m| {
            let m = m.borrow();
            if m.len() >= LEX_MEMO {
                0
            } else {
                preview_vec_growth(&m, 1).1
            }
        });
        let memo_alloc = units_bytes
            .saturating_add(text.len())
            .saturating_add(memo_growth)
            .saturating_add(std::mem::size_of::<Vec<BestEffortUnit>>())
            .saturating_add(16);
        let memo_units = memo_alloc.saturating_add(LEX_MEMO).max(1) as u64;
        if !budget.is_exhausted()
            && budget.remaining_units() >= memo_units
            && budget.remaining_alloc_bytes() >= memo_alloc
        {
            let _ = budget.charge(memo_units);
            let _ = budget.charge_alloc(memo_alloc);
            let spent_units = start_units_rem.saturating_sub(budget.remaining_units());
            let spent_alloc = start_alloc_rem.saturating_sub(budget.remaining_alloc_bytes());
            let produced: Vec<BestEffortUnit> = produced_slice
                .iter()
                .enumerate()
                .map(|(k, u)| {
                    let mut c = u.clone();
                    c.index = k;
                    c.pipeline_id = u
                        .pipeline_id
                        .map(|PipelineId(id)| PipelineId(id.saturating_sub(start_pipe_id)));
                    c
                })
                .collect();
            BE_LEX_MEMO.with(|m| {
                let mut m = m.borrow_mut();
                if m.len() >= LEX_MEMO {
                    m.remove(0);
                }
                m.push(LexMemoEntry {
                    line: text.to_string(),
                    units: Rc::new(produced),
                    units_bytes,
                    pipe_count,
                    compute_units: spent_units,
                    alloc_bytes: spent_alloc,
                });
            });
        }
    }

    Ok(())
}

/// A memo hit: the cached units, their copy size, pipeline count, and the
/// recorded compute and allocation cost.
type MemoHit = (Rc<Vec<BestEffortUnit>>, usize, u32, u64, usize);

/// Looks `text` up in the memo, charging the comparisons before they run.
///
/// The charge is the worst case for a full memo (`LEX_MEMO` entries, each a
/// length check plus a full comparison), whatever the memo holds, so a cold
/// run and a later hit charge the same totals (N11 determinism).
fn memo_lookup(text: &str, budget: &mut ComputeBudget) -> Result<Option<MemoHit>, Exhausted> {
    budget.charge((LEX_MEMO as u64).saturating_mul(1 + text.len() as u64))?;
    BE_LEX_MEMO.with(|m| {
        Ok(m.borrow().iter().find(|e| e.line == text).map(|e| {
            (
                Rc::clone(&e.units),
                e.units_bytes,
                e.pipe_count,
                e.compute_units,
                e.alloc_bytes,
            )
        }))
    })
}

struct BeState {
    next_pipe_id: u32,
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
    const fn new(pipeline_id: Option<PipelineId>, piped: bool) -> Self {
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

/// A best-effort word under construction.
///
/// When `prepay` is set (the word may become an argv word), each byte also
/// pre-pays the copy that the unit's effective reading, or recovery after
/// exhaustion, makes of the word later (one unit and one allocation byte per
/// byte), so that copy never has to be charged after the word is found. An
/// assignment word uses the same pre-payment for its name/value split.
struct WordBuf {
    s: String,
    prepay: bool,
    /// Every character so far is an ASCII digit (for `>&N` targets).
    all_digits: bool,
}

impl WordBuf {
    fn new(prepay: bool) -> Self {
        Self {
            s: String::new(),
            prepay,
            all_digits: true,
        }
    }

    fn push_char(&mut self, c: char, budget: &mut ComputeBudget) -> Result<(), Exhausted> {
        if self.prepay {
            budget.charge(c.len_utf8() as u64)?;
            budget.charge_alloc(c.len_utf8())?;
        }
        self.all_digits &= c.is_ascii_digit();
        push_char_budgeted(&mut self.s, c, budget)
    }

    fn push_str(&mut self, part: &str, budget: &mut ComputeBudget) -> Result<(), Exhausted> {
        if self.prepay {
            budget.charge(part.len() as u64)?;
            budget.charge_alloc(part.len())?;
        }
        if self.all_digits {
            budget.charge(part.len() as u64)?;
            self.all_digits = part.bytes().all(|b| b.is_ascii_digit());
        }
        push_str_budgeted(&mut self.s, part, budget)
    }

    fn len(&self) -> usize {
        self.s.len()
    }

    fn is_empty(&self) -> bool {
        self.s.is_empty()
    }
}

/// Pre-payment for one argv word's `String` header in a later copy of the
/// argv (see [`WordBuf`]), charged before the word is scanned.
const ARGV_COPY_HEADER: usize = std::mem::size_of::<String>();

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
    charge_string_growth(s, n, budget)?;
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
        charge_string_growth(s, n, budget)?;
        s.push_str(part);
    }
    Ok(())
}

#[inline]
fn clone_string_budgeted(s: &str, budget: &mut ComputeBudget) -> Result<String, Exhausted> {
    budget.charge(s.len().max(1) as u64)?;
    budget.charge_alloc(s.len())?;
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
            let s = clone_string_budgeted(body, budget)?;
            vec_push_budgeted(&mut out, s, budget)?;
            i = end;
        } else if bytes[i] == b'`' {
            let (body, end) = scan_backtick_bytes(text, i + 1, budget)?;
            vec_push_budgeted(&mut out, body, budget)?;
            i = end;
        } else {
            i += 1;
        }
    }
    Ok(out)
}

/// Keeps the slots recovery needs: one free `raw_units` slot for the pending
/// builder, and a free `record.units` slot for every builder in `raw_units`
/// plus the pending one.
fn reserve_recovery_slots(
    raw_units: &mut Vec<BeBuilder>,
    record: &mut BestEffortRecord,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    charge_vec_growth(raw_units, 1, budget)?;
    charge_vec_growth(&mut record.units, raw_units.len() + 1, budget)
}

/// Converts `b` into a record unit with the readings computed so far, into a
/// slot reserved by [`reserve_recovery_slots`].
fn push_converted(
    record: &mut BestEffortRecord,
    b: BeBuilder,
    effective_argv: Vec<String>,
    alt_argv: Vec<Vec<String>>,
    depth: u8,
    derived: bool,
) {
    if record.units.len() >= limits::MAX_UNITS {
        return;
    }
    let index = record.units.len();
    push_reserved(
        &mut record.units,
        BestEffortUnit {
            index,
            raw_argv: b.raw_argv,
            effective_argv,
            alt_argv,
            assignments: b.assignments,
            redirects: b.redirects,
            pipeline_id: b.pipeline_id,
            piped: b.piped,
            depth,
            derived,
        },
    );
}

#[allow(clippy::too_many_lines)]
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

    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut cur = BeBuilder::new(None, false);
    let mut raw_units: Vec<BeBuilder> = Vec::new();
    let mut pending: Option<BePending> = None;
    let mut heredocs: Vec<(String, bool, bool)> = Vec::new();
    let mut heredoc_owners: Vec<usize> = Vec::new();
    let mut bodies: Vec<(usize, String)> = Vec::new();

    reserve_recovery_slots(&mut raw_units, record, budget)
        .map_err(|e| PartOutcome::Exhausted(e.reason))?;

    // Moves a non-empty pending builder into its reserved `raw_units` slot,
    // then reserves the slots the next pending builder needs.
    let flush_builder = |cur: &mut BeBuilder,
                         next_pipe: Option<PipelineId>,
                         next_piped: bool,
                         raw_units: &mut Vec<BeBuilder>,
                         record: &mut BestEffortRecord,
                         budget: &mut ComputeBudget|
     -> Result<(), PartOutcome> {
        if cur.is_empty() {
            *cur = BeBuilder::new(next_pipe, next_piped);
            return Ok(());
        }
        if record.units.len() + raw_units.len() >= limits::MAX_UNITS {
            return Err(PartOutcome::TooLarge(Cap::Units));
        }
        budget
            .charge(1)
            .map_err(|e| PartOutcome::Exhausted(e.reason))?;
        let b = std::mem::replace(cur, BeBuilder::new(next_pipe, next_piped));
        push_reserved(raw_units, b);
        reserve_recovery_slots(raw_units, record, budget)
            .map_err(|e| PartOutcome::Exhausted(e.reason))
    };

    let scan_res: Result<(), PartOutcome> = (|| {
        let ex = |e: Exhausted| PartOutcome::Exhausted(e.reason);

        while i < bytes.len() {
            budget.charge(1).map_err(ex)?;
            match bytes[i] {
                b' ' | b'\t' => {
                    i += 1;
                }
                b'\\' if bytes.get(i + 1) == Some(&b'\n') => {
                    budget.charge(1).map_err(ex)?;
                    i += 2;
                }
                b'\n' => {
                    i += 1;
                    flush_builder(&mut cur, None, false, &mut raw_units, record, budget)?;
                    let owners = std::mem::take(&mut heredoc_owners);
                    for (hd_idx, (delim, strip_tabs, quoted)) in
                        std::mem::take(&mut heredocs).into_iter().enumerate()
                    {
                        let owner = owners.get(hd_idx).copied().unwrap_or(usize::MAX);
                        let mut body = String::new();
                        while i < bytes.len() {
                            let line_start = i;
                            while i < bytes.len() {
                                budget.charge(1).map_err(ex)?;
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
                                budget.charge(line_str.len() as u64 + 1).map_err(ex)?;
                                line_str.trim_start_matches('\t')
                            } else {
                                line_str
                            };
                            if bytes_eq(trimmed.as_bytes(), delim.as_bytes(), budget).map_err(ex)? {
                                break;
                            }
                            push_str_budgeted(&mut body, line_str, budget)
                                .and_then(|()| push_char_budgeted(&mut body, '\n', budget))
                                .map_err(ex)?;
                        }
                        if !quoted {
                            let subs = find_substitutions_budgeted(&body, budget).map_err(ex)?;
                            if let Some(last) = raw_units.last_mut() {
                                vec_extend_budgeted(&mut last.subs, subs, budget).map_err(ex)?;
                            }
                        }
                        vec_push_budgeted(&mut bodies, (owner, body), budget).map_err(ex)?;
                    }
                }
                b';' => {
                    if bytes.get(i + 1) == Some(&b';') {
                        budget.charge(1).map_err(ex)?;
                        i += 2;
                    } else {
                        i += 1;
                    }
                    flush_builder(&mut cur, None, false, &mut raw_units, record, budget)?;
                }
                b'(' | b')' => {
                    let c = bytes[i];
                    i += 1;
                    if c == b'(' {
                        // `f()` function definition: skip empty parens cleanly.
                        let mut k = i;
                        while matches!(bytes.get(k), Some(b' ' | b'\t')) {
                            budget.charge(1).map_err(ex)?;
                            k += 1;
                        }
                        if bytes.get(k) == Some(&b')') && !cur.raw_argv.is_empty() {
                            budget.charge(1).map_err(ex)?;
                            i = k + 1;
                        }
                    }
                    flush_builder(&mut cur, None, false, &mut raw_units, record, budget)?;
                }
                b'&' if bytes.get(i + 1) == Some(&b'&') => {
                    budget.charge(1).map_err(ex)?;
                    i += 2;
                    flush_builder(&mut cur, None, false, &mut raw_units, record, budget)?;
                }
                b'&' if bytes.get(i + 1) == Some(&b'>') => {
                    let step = if bytes.get(i + 2) == Some(&b'>') {
                        3
                    } else {
                        2
                    };
                    budget.charge((step - 1) as u64).map_err(ex)?;
                    i += step;
                    pending = Some(BePending::Redirect(BestEffortRedirectKind::Write));
                }
                b'&' => {
                    i += 1;
                    flush_builder(&mut cur, None, false, &mut raw_units, record, budget)?;
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
                    if step > 1 {
                        budget.charge((step - 1) as u64).map_err(ex)?;
                    }
                    i += step;
                    if is_pipe {
                        let pid = cur.pipeline_id.unwrap_or_else(|| {
                            let id = PipelineId(state.next_pipe_id);
                            state.next_pipe_id += 1;
                            id
                        });
                        cur.pipeline_id = Some(pid);
                        flush_builder(&mut cur, Some(pid), true, &mut raw_units, record, budget)?;
                    } else {
                        flush_builder(&mut cur, None, false, &mut raw_units, record, budget)?;
                    }
                }
                b'#' => {
                    i += 1;
                    while i < bytes.len() && bytes[i] != b'\n' {
                        budget.charge(1).map_err(ex)?;
                        i += 1;
                    }
                }
                b'<' | b'>' if bytes.get(i + 1) != Some(&b'(') => {
                    let before_hd = heredocs.len();
                    i = lex_be_redirect(text, i, &mut pending, &mut heredocs, budget)
                        .map_err(ex)?;
                    if heredocs.len() > before_hd {
                        vec_push_budgeted(&mut heredoc_owners, raw_units.len(), budget)
                            .map_err(ex)?;
                    }
                }
                _ => {
                    // Reserve, before scanning, the slot the finished word
                    // goes into, and pre-pay its later copy (review round 3,
                    // finding 1: a finished word is never lost).
                    match pending {
                        None => {
                            charge_vec_growth(&mut cur.raw_argv, 1, budget)
                                .and_then(|()| budget.charge(ARGV_COPY_HEADER as u64))
                                .and_then(|()| budget.charge_alloc(ARGV_COPY_HEADER))
                                .map_err(ex)?;
                        }
                        Some(BePending::Redirect(_)) => {
                            charge_vec_growth(&mut cur.redirects, 1, budget).map_err(ex)?;
                        }
                        Some(BePending::HereString) => {
                            charge_vec_growth(&mut bodies, 1, budget).map_err(ex)?;
                        }
                    }
                    let assignment_candidate = pending.is_none() && cur.raw_argv.is_empty();

                    // Read one word linearly.
                    let mut word = WordBuf::new(pending.is_none());
                    let mut quoted_prefix_empty = true;
                    let mut all_unquoted_digits = true;
                    let mut assign_prefix = AssignPrefix::Empty;
                    let mut seen_unquoted_eq = false;
                    let mut assign_eq: Option<usize> = None;
                    let start_i = i;

                    while i < bytes.len() {
                        if i > start_i {
                            budget.charge(1).map_err(ex)?;
                        }
                        let b = bytes[i];
                        match b {
                            b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'(' | b')' => break,
                            b'<' | b'>' if bytes.get(i + 1) != Some(&b'(') => break,
                            b'\\' => {
                                quoted_prefix_empty = false;
                                all_unquoted_digits = false;
                                assign_prefix = AssignPrefix::Invalid;
                                if i + 1 < bytes.len() {
                                    budget.charge(1).map_err(ex)?;
                                    let next_ch = text[i + 1..].chars().next().unwrap_or(' ');
                                    if next_ch != '\n' {
                                        word.push_char(next_ch, budget).map_err(ex)?;
                                    }
                                    i += 1 + next_ch.len_utf8();
                                } else {
                                    i += 1;
                                }
                            }
                            b'\'' => {
                                quoted_prefix_empty = false;
                                all_unquoted_digits = false;
                                assign_prefix = AssignPrefix::Invalid;
                                i += 1;
                                while i < bytes.len() {
                                    budget.charge(1).map_err(ex)?;
                                    if bytes[i] == b'\'' {
                                        i += 1;
                                        break;
                                    }
                                    let ch = text[i..].chars().next().unwrap_or(' ');
                                    word.push_char(ch, budget).map_err(ex)?;
                                    i += ch.len_utf8();
                                }
                            }
                            b'"' => {
                                quoted_prefix_empty = false;
                                all_unquoted_digits = false;
                                assign_prefix = AssignPrefix::Invalid;
                                i += 1;
                                while i < bytes.len() {
                                    budget.charge(1).map_err(ex)?;
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
                                            budget.charge(1).map_err(ex)?;
                                            let nb = bytes[i + 1];
                                            if nb != b'\n' {
                                                word.push_char(nb as char, budget).map_err(ex)?;
                                            }
                                            i += 2;
                                        }
                                        b'$' if bytes.get(i + 1) == Some(&b'(') => {
                                            budget.charge(1).map_err(ex)?;
                                            let (body, end) =
                                                scan_paren_bytes(text, i + 2, 0, budget)
                                                    .map_err(ex)?;
                                            word.push_str("$(", budget)
                                                .and_then(|()| word.push_str(body, budget))
                                                .and_then(|()| word.push_char(')', budget))
                                                .and_then(|()| {
                                                    let s = clone_string_budgeted(body, budget)?;
                                                    vec_push_budgeted(&mut cur.subs, s, budget)
                                                })
                                                .map_err(ex)?;
                                            i = end;
                                        }
                                        b'`' => {
                                            let (body, end) =
                                                scan_backtick_bytes(text, i + 1, budget)
                                                    .map_err(ex)?;
                                            word.push_char('`', budget)
                                                .and_then(|()| word.push_str(&body, budget))
                                                .and_then(|()| word.push_char('`', budget))
                                                .and_then(|()| {
                                                    vec_push_budgeted(&mut cur.subs, body, budget)
                                                })
                                                .map_err(ex)?;
                                            i = end;
                                        }
                                        b'$' if bytes.get(i + 1) == Some(&b'{') => {
                                            budget.charge(1).map_err(ex)?;
                                            let (body, end) = scan_dollar_brace_bytes(
                                                text,
                                                i + 2,
                                                true,
                                                0,
                                                budget,
                                            )
                                            .map_err(ex)?;
                                            word.push_str("${", budget)
                                                .and_then(|()| word.push_str(body, budget))
                                                .and_then(|()| word.push_char('}', budget))
                                                .map_err(ex)?;
                                            let subs = find_substitutions_budgeted(body, budget)
                                                .map_err(ex)?;
                                            vec_extend_budgeted(&mut cur.subs, subs, budget)
                                                .map_err(ex)?;
                                            i = end;
                                        }
                                        _ => {
                                            let ch = text[i..].chars().next().unwrap_or(' ');
                                            word.push_char(ch, budget).map_err(ex)?;
                                            i += ch.len_utf8();
                                        }
                                    }
                                }
                            }
                            b'$' if bytes.get(i + 1) == Some(&b'\'') => {
                                // Keep quote balance across `$'...'` (escaping `\'` and `\\`)
                                // without decoding ANSI-C escapes (dropped per §12).
                                quoted_prefix_empty = false;
                                all_unquoted_digits = false;
                                assign_prefix = AssignPrefix::Invalid;
                                budget.charge(1).map_err(ex)?;
                                word.push_str("$'", budget).map_err(ex)?;
                                i += 2;
                                while i < bytes.len() {
                                    budget.charge(1).map_err(ex)?;
                                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                                        budget.charge(1).map_err(ex)?;
                                        let next_ch = text[i + 1..].chars().next().unwrap_or(' ');
                                        word.push_char('\\', budget)
                                            .and_then(|()| word.push_char(next_ch, budget))
                                            .map_err(ex)?;
                                        i += 1 + next_ch.len_utf8();
                                    } else if bytes[i] == b'\'' {
                                        word.push_char('\'', budget).map_err(ex)?;
                                        i += 1;
                                        break;
                                    } else {
                                        let ch = text[i..].chars().next().unwrap_or(' ');
                                        word.push_char(ch, budget).map_err(ex)?;
                                        i += ch.len_utf8();
                                    }
                                }
                            }
                            b'$' if bytes.get(i + 1) == Some(&b'(') => {
                                all_unquoted_digits = false;
                                assign_prefix = AssignPrefix::Invalid;
                                let dbl = bytes.get(i + 2) == Some(&b'(');
                                budget.charge(1).map_err(ex)?;
                                let (body, end) =
                                    scan_paren_bytes(text, i + 2, 0, budget).map_err(ex)?;
                                word.push_str("$(", budget)
                                    .and_then(|()| word.push_str(body, budget))
                                    .and_then(|()| word.push_char(')', budget))
                                    .map_err(ex)?;
                                if dbl {
                                    let subs =
                                        find_substitutions_budgeted(body, budget).map_err(ex)?;
                                    vec_extend_budgeted(&mut cur.subs, subs, budget).map_err(ex)?;
                                }
                                clone_string_budgeted(body, budget)
                                    .and_then(|s| vec_push_budgeted(&mut cur.subs, s, budget))
                                    .map_err(ex)?;
                                i = end;
                            }
                            b'$' if bytes.get(i + 1) == Some(&b'{') => {
                                all_unquoted_digits = false;
                                assign_prefix = AssignPrefix::Invalid;
                                budget.charge(1).map_err(ex)?;
                                let (body, end) =
                                    scan_dollar_brace_bytes(text, i + 2, false, 0, budget)
                                        .map_err(ex)?;
                                word.push_str("${", budget)
                                    .and_then(|()| word.push_str(body, budget))
                                    .and_then(|()| word.push_char('}', budget))
                                    .map_err(ex)?;
                                let subs = find_substitutions_budgeted(body, budget).map_err(ex)?;
                                vec_extend_budgeted(&mut cur.subs, subs, budget).map_err(ex)?;
                                i = end;
                            }
                            b'`' => {
                                all_unquoted_digits = false;
                                assign_prefix = AssignPrefix::Invalid;
                                let (body, end) =
                                    scan_backtick_bytes(text, i + 1, budget).map_err(ex)?;
                                word.push_char('`', budget)
                                    .and_then(|()| word.push_str(&body, budget))
                                    .and_then(|()| word.push_char('`', budget))
                                    .and_then(|()| vec_push_budgeted(&mut cur.subs, body, budget))
                                    .map_err(ex)?;
                                i = end;
                            }
                            b'<' | b'>' => {
                                // Process substitution `<(...)` / `>(...)`.
                                all_unquoted_digits = false;
                                assign_prefix = AssignPrefix::Invalid;
                                let dir = b as char;
                                budget.charge(1).map_err(ex)?;
                                let (body, end) =
                                    scan_paren_bytes(text, i + 2, 0, budget).map_err(ex)?;
                                word.push_char(dir, budget)
                                    .and_then(|()| word.push_char('(', budget))
                                    .and_then(|()| word.push_str(body, budget))
                                    .and_then(|()| word.push_char(')', budget))
                                    .and_then(|()| {
                                        let s = clone_string_budgeted(body, budget)?;
                                        vec_push_budgeted(&mut cur.subs, s, budget)
                                    })
                                    .map_err(ex)?;
                                i = end;
                            }
                            b'=' => {
                                if !seen_unquoted_eq {
                                    seen_unquoted_eq = true;
                                    if quoted_prefix_empty && assign_prefix.is_valid_before_eq() {
                                        assign_eq = Some(word.len());
                                        if assignment_candidate {
                                            // Reserve the assignment's slot while
                                            // the word is still being scanned.
                                            charge_vec_growth(&mut cur.assignments, 1, budget)
                                                .map_err(ex)?;
                                        }
                                    }
                                    assign_prefix = AssignPrefix::Invalid;
                                }
                                all_unquoted_digits = false;
                                word.push_char('=', budget).map_err(ex)?;
                                i += 1;
                            }
                            _ => {
                                let ch = text[i..].chars().next().unwrap_or(' ');
                                assign_prefix.advance_unquoted_char(ch);
                                if !ch.is_ascii_digit() {
                                    all_unquoted_digits = false;
                                }
                                word.push_char(ch, budget).map_err(ex)?;
                                i += ch.len_utf8();
                            }
                        }
                    }

                    if i == start_i {
                        i += 1;
                        continue;
                    }

                    // Unquoted digits directly before `<` or `>` are the redirect fd.
                    if pending.is_none()
                        && quoted_prefix_empty
                        && all_unquoted_digits
                        && !word.is_empty()
                        && matches!(bytes.get(i), Some(b'<' | b'>'))
                        && bytes.get(i + 1) != Some(&b'(')
                    {
                        let before_hd = heredocs.len();
                        i = lex_be_redirect(text, i, &mut pending, &mut heredocs, budget)
                            .map_err(ex)?;
                        if heredocs.len() > before_hd {
                            vec_push_budgeted(&mut heredoc_owners, raw_units.len(), budget)
                                .map_err(ex)?;
                        }
                        continue;
                    }

                    // From here on the finished word goes into the slot
                    // reserved before it was scanned, with no further charge.
                    match pending.take() {
                        Some(BePending::Redirect(kind)) => {
                            let fixed = if kind == BestEffortRedirectKind::Dup
                                && !(word.s == "-" || word.all_digits)
                            {
                                BestEffortRedirectKind::Write
                            } else {
                                kind
                            };
                            push_reserved(&mut cur.redirects, (fixed, word.s));
                        }
                        Some(BePending::HereString) => {
                            push_reserved(&mut bodies, (raw_units.len(), word.s));
                        }
                        None => {
                            if assignment_candidate && let Some(eq) = assign_eq {
                                // The split's copy of the value was pre-paid
                                // byte by byte while the word was scanned.
                                let mut name = word.s;
                                let value = name.split_off(eq + 1);
                                name.truncate(eq);
                                if name.ends_with('+') {
                                    name.pop();
                                }
                                push_reserved(&mut cur.assignments, (name, value));
                            } else {
                                push_reserved(&mut cur.raw_argv, word.s);
                            }
                        }
                    }
                }
            }
        }
        flush_builder(&mut cur, None, false, &mut raw_units, record, budget)?;
        Ok(())
    })();

    if let Err(err) = scan_res {
        // Preserve every unit scanned before exhaustion (§8), into slots
        // reserved earlier; the argv copies were pre-paid.
        if matches!(err, PartOutcome::Exhausted(_)) {
            if !cur.is_empty() && record.units.len() + raw_units.len() < limits::MAX_UNITS {
                push_reserved(&mut raw_units, cur);
            }
            flush_builders_raw(raw_units.into_iter(), depth, derived, record);
        }
        return Err(err);
    }

    // Convert raw builders into `BestEffortUnit`s + run bounded code scans.
    let mut raw_to_record_idx: Vec<usize> =
        match vec_with_capacity_budgeted(raw_units.len(), budget) {
            Ok(v) => v,
            Err(e) => {
                flush_builders_raw(raw_units.into_iter(), depth, derived, record);
                return Err(PartOutcome::Exhausted(e.reason));
            }
        };
    let mut deferred_payloads: Vec<(u8, String)> = Vec::new();
    let mut pipeline_units: Vec<(PipelineId, usize)> = Vec::new();
    let mut raw_iter = raw_units.into_iter();

    while let Some(b) = raw_iter.next() {
        if record.units.len() >= limits::MAX_UNITS {
            return Err(PartOutcome::TooLarge(Cap::Units));
        }
        // The argv copy for the effective reading was pre-paid word by word.
        let init = b.raw_argv.clone();
        let BestEffortUnwrap {
            effective: eff,
            alt: mut all_alt,
            wrapper_payloads: mut extra_wrapper_payloads,
        } = match unwrap_best_effort(init, budget) {
            Ok(v) => v,
            Err((partial, e)) => {
                push_converted(record, b, partial, Vec::new(), depth, derived);
                flush_builders_raw(raw_iter, depth, derived, record);
                return Err(PartOutcome::Exhausted(e.reason));
            }
        };

        // Collect everything derived from this unit's readings.
        let derived_work = (|| -> Result<(PayloadSet, Vec<Vec<String>>), Exhausted> {
            // `$(which CMD)` / `$(command -v CMD)` / `$(type -p CMD)` /
            // `$(echo CMD)` command-word substitutions (CR-19 folded).
            resolve_cmd_sub_readings(&eff, &mut all_alt, &mut extra_wrapper_payloads, budget)?;

            // Bounded code payloads from wrapper steps, `eff` and `all_alt`,
            // deduplicated in linear charged work (review round 3, finding 3).
            let mut payloads = PayloadSet::default();
            for p in extra_wrapper_payloads.drain(..) {
                payloads.insert(p, budget)?;
            }
            for reading in std::iter::once(&eff).chain(all_alt.iter()) {
                for p in extract_code_payloads(reading, budget)? {
                    payloads.insert(p, budget)?;
                }
            }

            // `find -exec` / `-execdir` / `-ok` / `-okdir` sub-commands.
            let mut find_execs = Vec::new();
            if depth < limits::MAX_DEPTH {
                for reading in std::iter::once(&eff).chain(all_alt.iter()) {
                    if let Some(f) = reading.first()
                        && fold_str_cr19(f, budget)? == "find"
                    {
                        let cmds = extract_find_exec_cmds(reading, budget)?;
                        vec_extend_budgeted(&mut find_execs, cmds, budget)?;
                    }
                }
            }

            // Process substitutions `<(...)` passed to shells/source/`.`.
            collect_shell_proc_sub_payloads(
                &eff,
                &all_alt,
                &b.raw_argv,
                &b.redirects,
                &mut payloads,
                budget,
            )?;
            budget.charge(1)?;
            Ok((payloads, find_execs))
        })();
        let (payloads, find_execs) = match derived_work {
            Ok(v) => v,
            Err(e) => {
                push_converted(record, b, eff, all_alt, depth, derived);
                flush_builders_raw(raw_iter, depth, derived, record);
                return Err(PartOutcome::Exhausted(e.reason));
            }
        };

        let unit_idx = record.units.len();
        let BeBuilder {
            assignments,
            raw_argv,
            redirects,
            subs,
            pipeline_id,
            piped,
        } = b;
        push_reserved(&mut raw_to_record_idx, unit_idx);
        push_reserved(
            &mut record.units,
            BestEffortUnit {
                index: unit_idx,
                raw_argv,
                effective_argv: eff,
                alt_argv: all_alt,
                assignments,
                redirects,
                pipeline_id,
                piped,
                depth,
                derived,
            },
        );
        if let Some(pid) = pipeline_id
            && let Err(e) = vec_push_budgeted(&mut pipeline_units, (pid, unit_idx), budget)
        {
            flush_builders_raw(raw_iter, depth, derived, record);
            return Err(PartOutcome::Exhausted(e.reason));
        }

        // Push `find -exec` derived units and run their readings through the
        // same bounded payload extraction and recursion as ordinary units (§2, §12),
        // keeping a record slot for every builder still to convert.
        if !find_execs.is_empty()
            && depth < limits::MAX_DEPTH
            && let Err(err) = process_find_exec_cmds(
                find_execs,
                depth + 1,
                raw_iter.len(),
                record,
                &mut deferred_payloads,
                budget,
            )
        {
            if matches!(err, PartOutcome::Exhausted(_)) {
                flush_builders_raw(raw_iter, depth, derived, record);
            }
            return Err(err);
        }

        // Recurse on substitutions and extracted code payloads within `MAX_DEPTH`.
        if depth < limits::MAX_DEPTH {
            for p in subs.into_iter().chain(payloads.into_vec()) {
                if let Err(e) = vec_push_budgeted(&mut deferred_payloads, (depth + 1, p), budget) {
                    flush_builders_raw(raw_iter, depth, derived, record);
                    return Err(PartOutcome::Exhausted(e.reason));
                }
            }
        }
    }

    let ex = |e: Exhausted| PartOutcome::Exhausted(e.reason);

    // Scan 7: Here-doc / here-string bodies fed to shells, `eval`, `source`,
    // `.`, interpreters, `crontab`, or piped commands.
    for (owner, body) in bodies {
        budget.charge(1).map_err(ex)?;
        let owner_unit = raw_to_record_idx
            .get(owner)
            .and_then(|&idx| record.units.get(idx));
        let owner_folded = match owner_unit.and_then(|u| u.effective_argv.first()) {
            Some(f) => Some((f.as_str(), fold_str_cr19(f, budget).map_err(ex)?)),
            None => None,
        };
        let is_crontab = owner_folded.as_ref().is_some_and(|(_, s)| s == "crontab");
        if is_crontab && depth < limits::MAX_DEPTH {
            let cmds = crontab_commands_budgeted(&body, budget).map_err(ex)?;
            if !cmds.is_empty() {
                vec_push_budgeted(&mut deferred_payloads, (depth + 1, cmds), budget).map_err(ex)?;
            }
        }
        let feeds_code = match owner_unit {
            None => true,
            Some(u) => {
                if u.pipeline_id.is_some() {
                    true
                } else if let Some((raw_first, folded)) = owner_folded.as_ref() {
                    is_be_shell(raw_first, budget).map_err(ex)?
                        || is_be_interpreter(folded)
                        || matches!(folded.as_str(), "eval" | "source" | "." | "crontab")
                        || is_be_code_runner(folded, budget).map_err(ex)?
                        || is_be_reserved(raw_first, budget).map_err(ex)?
                } else {
                    false
                }
            }
        };
        if feeds_code && depth < limits::MAX_DEPTH {
            vec_push_budgeted(&mut deferred_payloads, (depth + 1, body), budget).map_err(ex)?;
        }
    }

    // Scan 8: `echo` / `printf` piped into a shell / `eval` / `source` /
    // `crontab`. The stages of one pipeline are consecutive in
    // `pipeline_units`, so each pipeline is one run, walked linearly.
    let mut g = 0usize;
    while g < pipeline_units.len() {
        let pid = pipeline_units[g].0;
        let mut end = g;
        while end < pipeline_units.len() && pipeline_units[end].0 == pid {
            budget.charge(1).map_err(ex)?;
            end += 1;
        }
        // The first piped stage that receives code.
        let mut receiver: Option<(usize, bool)> = None;
        for &(_, idx) in &pipeline_units[g..end] {
            budget.charge(1).map_err(ex)?;
            let u = &record.units[idx];
            if !u.piped {
                continue;
            }
            let Some(first) = u.effective_argv.first() else {
                continue;
            };
            let is_sh = is_be_shell(first, budget).map_err(ex)?;
            let folded = fold_str_cr19(first, budget).map_err(ex)?;
            if is_sh || matches!(folded.as_str(), "eval" | "source" | "." | "crontab") {
                receiver = Some((idx, folded == "crontab"));
                break;
            }
        }
        if let Some((idx, is_crontab)) = receiver {
            for &(_, other_idx) in &pipeline_units[g..end] {
                budget.charge(1).map_err(ex)?;
                if other_idx == idx {
                    continue;
                }
                let texts =
                    extract_echo_printf_texts(&record.units[other_idx].effective_argv, budget)
                        .map_err(ex)?;
                for t in texts {
                    if is_crontab && depth < limits::MAX_DEPTH {
                        let cmds = crontab_commands_budgeted(&t, budget).map_err(ex)?;
                        if !cmds.is_empty() {
                            vec_push_budgeted(&mut deferred_payloads, (depth + 1, cmds), budget)
                                .map_err(ex)?;
                        }
                    }
                    if depth < limits::MAX_DEPTH {
                        vec_push_budgeted(&mut deferred_payloads, (depth + 1, t), budget)
                            .map_err(ex)?;
                    }
                }
            }
        }
        g = end;
    }

    // Recursively lex deferred payloads within `MAX_DEPTH`.
    for (payload_depth, payload) in deferred_payloads {
        if payload_depth <= limits::MAX_DEPTH {
            lex_one_text(&payload, payload_depth, true, record, state, budget)?;
        }
    }

    Ok(())
}

/// `a == b` on two readings, charged before it runs: one length check, one
/// per pair of strings, and the bytes of every same-length pair.
fn readings_eq(a: &[String], b: &[String], budget: &mut ComputeBudget) -> Result<bool, Exhausted> {
    budget.charge(1)?;
    if a.len() != b.len() {
        return Ok(false);
    }
    budget.charge(a.len() as u64)?;
    let bytes: usize = a
        .iter()
        .zip(b)
        .map(|(x, y)| if x.len() == y.len() { x.len() } else { 0 })
        .sum();
    budget.charge(bytes as u64)?;
    Ok(a == b)
}

/// Whether `readings` (at most `MAX_ALT_READINGS` entries) contains `r`,
/// each comparison charged by [`readings_eq`].
fn has_reading(
    readings: &[Vec<String>],
    r: &[String],
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    for x in readings {
        if readings_eq(x, r, budget)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn resolve_cmd_sub_readings(
    eff: &[String],
    all_alt: &mut Vec<Vec<String>>,
    extra_wrapper_payloads: &mut Vec<String>,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    let Some(first) = eff.first() else {
        return Ok(());
    };
    // `has_cmd_sub` makes two substring scans of the word.
    budget.charge((first.len() as u64).saturating_mul(2).max(1))?;
    if !has_cmd_sub(first) {
        return Ok(());
    }
    if let Some(sub_name) = substituted_program_budgeted(first, budget)? {
        // Reserve every alternative-reading slot before deriving readings, so
        // a derived reading is never lost (alternatives are capped at
        // `MAX_ALT_READINGS`).
        charge_vec_growth(
            all_alt,
            MAX_ALT_READINGS.saturating_sub(all_alt.len()),
            budget,
        )?;
        let mut replaced = clone_vec_string_budgeted(eff, budget)?;
        replaced[0] = sub_name;
        let BestEffortUnwrap {
            effective: sub_eff,
            alt: sub_alt,
            wrapper_payloads: sub_wp,
        } = unwrap_best_effort(replaced, budget).map_err(|(_, e)| e)?;
        vec_extend_budgeted(extra_wrapper_payloads, sub_wp, budget)?;
        if !sub_eff.is_empty()
            && all_alt.len() < MAX_ALT_READINGS
            && !readings_eq(&sub_eff, eff, budget)?
            && !has_reading(all_alt, &sub_eff, budget)?
        {
            push_reserved(all_alt, sub_eff);
        }
        for a in sub_alt {
            if !a.is_empty()
                && all_alt.len() < MAX_ALT_READINGS
                && !readings_eq(&a, eff, budget)?
                && !has_reading(all_alt, &a, budget)?
            {
                push_reserved(all_alt, a);
            }
        }
    }
    Ok(())
}

/// Distinct non-empty payloads in insertion order, deduplicated through an
/// open-addressing index of FNV-1a hashes (review round 3, finding 3).
///
/// Hashing charges one unit per byte, each probe one unit, and each
/// comparison of a matching hash the bytes it compares, all before the work;
/// the index table is charged by capacity and kept at most half full.
/// Inserting `n` payloads of total length `L` is O(n + L) charged work in
/// expectation; inputs whose hashes collide only cost more charged probes.
#[derive(Default)]
pub(super) struct PayloadSet {
    items: Vec<String>,
    hashes: Vec<u64>,
    /// `0` is empty; otherwise the index into `items`, plus one.
    slots: Vec<u32>,
}

impl PayloadSet {
    fn fnv1a(bytes: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }

    fn grow(&mut self, budget: &mut ComputeBudget) -> Result<(), Exhausted> {
        let new_len = self.slots.len().saturating_mul(2).max(16);
        // Zero-filling the new table and re-placing every stored hash.
        budget.charge(new_len as u64 + self.hashes.len() as u64)?;
        let mut slots: Vec<u32> = vec_with_capacity_budgeted(new_len, budget)?;
        slots.resize(new_len, 0);
        let mask = new_len - 1;
        for (idx, &h) in self.hashes.iter().enumerate() {
            let mut at = (h as usize) & mask;
            while slots[at] != 0 {
                budget.charge(1)?;
                at = (at + 1) & mask;
            }
            slots[at] = u32::try_from(idx + 1).unwrap_or(u32::MAX);
        }
        self.slots = slots;
        Ok(())
    }

    /// Adds `s` unless it is empty or already present.
    pub(super) fn insert(
        &mut self,
        s: String,
        budget: &mut ComputeBudget,
    ) -> Result<(), Exhausted> {
        if s.is_empty() {
            return Ok(());
        }
        budget.charge(s.len() as u64)?;
        let h = Self::fnv1a(s.as_bytes());
        if (self.items.len() + 1) * 2 > self.slots.len() {
            self.grow(budget)?;
        }
        let mask = self.slots.len() - 1;
        let mut at = (h as usize) & mask;
        loop {
            budget.charge(1)?;
            let slot = self.slots[at];
            if slot == 0 {
                break;
            }
            let idx = (slot - 1) as usize;
            if self.hashes[idx] == h && bytes_eq(self.items[idx].as_bytes(), s.as_bytes(), budget)?
            {
                return Ok(());
            }
            at = (at + 1) & mask;
        }
        charge_vec_growth(&mut self.items, 1, budget)?;
        charge_vec_growth(&mut self.hashes, 1, budget)?;
        self.slots[at] = u32::try_from(self.items.len() + 1).unwrap_or(u32::MAX);
        push_reserved(&mut self.items, s);
        push_reserved(&mut self.hashes, h);
        Ok(())
    }

    pub(super) fn into_vec(self) -> Vec<String> {
        self.items
    }
}

fn collect_shell_proc_sub_payloads(
    eff: &[String],
    all_alt: &[Vec<String>],
    raw_argv: &[String],
    redirects: &[(BestEffortRedirectKind, String)],
    payloads: &mut PayloadSet,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    let mut runs_proc_sub = false;
    for r in std::iter::once(eff).chain(all_alt.iter().map(Vec::as_slice)) {
        if let Some(f) = r.first() {
            if is_be_shell(f, budget)? {
                runs_proc_sub = true;
                break;
            }
            let folded = fold_str_cr19(f, budget)?;
            if matches!(folded.as_str(), "source" | ".") {
                runs_proc_sub = true;
                break;
            }
        }
    }
    if runs_proc_sub {
        for w in raw_argv
            .iter()
            .skip(1)
            .chain(redirects.iter().map(|(_, t)| t))
        {
            budget.charge(1)?;
            if let Some(inner) = w.strip_prefix("<(").and_then(|r| r.strip_suffix(')')) {
                let s = clone_string_budgeted(inner, budget)?;
                payloads.insert(s, budget)?;
            }
        }
    }
    Ok(())
}

/// Derives a unit for each `find -exec` command. `keep` record slots stay
/// free for the builders of the current text not yet converted, so their
/// recovery never allocates.
fn process_find_exec_cmds(
    find_execs: Vec<Vec<String>>,
    exec_depth: u8,
    keep: usize,
    record: &mut BestEffortRecord,
    deferred_payloads: &mut Vec<(u8, String)>,
    budget: &mut ComputeBudget,
) -> Result<(), PartOutcome> {
    let ex = |e: Exhausted| PartOutcome::Exhausted(e.reason);
    if exec_depth > limits::MAX_DEPTH {
        return Ok(());
    }
    for cmd_argv in find_execs {
        if record.units.len() >= limits::MAX_UNITS {
            return Err(PartOutcome::TooLarge(Cap::Units));
        }
        // Reserve the derived unit's record slot before deriving it.
        budget
            .charge(1)
            .and_then(|()| charge_vec_growth(&mut record.units, 1 + keep, budget))
            .map_err(ex)?;
        let init = clone_vec_string_budgeted(&cmd_argv, budget).map_err(ex)?;
        let BestEffortUnwrap {
            effective: f_eff,
            alt: mut f_alt,
            wrapper_payloads: mut f_wp,
        } = unwrap_best_effort(init, budget).map_err(|(_, e)| ex(e))?;

        resolve_cmd_sub_readings(&f_eff, &mut f_alt, &mut f_wp, budget).map_err(ex)?;

        let mut exec_payloads = PayloadSet::default();
        for p in f_wp {
            exec_payloads.insert(p, budget).map_err(ex)?;
        }
        for reading in std::iter::once(&f_eff).chain(f_alt.iter()) {
            for p in extract_code_payloads(reading, budget).map_err(ex)? {
                exec_payloads.insert(p, budget).map_err(ex)?;
            }
        }
        collect_shell_proc_sub_payloads(&f_eff, &f_alt, &cmd_argv, &[], &mut exec_payloads, budget)
            .map_err(ex)?;

        let mut nested_find_execs: Vec<Vec<String>> = Vec::new();
        if exec_depth < limits::MAX_DEPTH {
            for reading in std::iter::once(&f_eff).chain(f_alt.iter()) {
                if let Some(f) = reading.first()
                    && fold_str_cr19(f, budget).map_err(ex)? == "find"
                {
                    let cmds = extract_find_exec_cmds(reading, budget).map_err(ex)?;
                    vec_extend_budgeted(&mut nested_find_execs, cmds, budget).map_err(ex)?;
                }
            }
        }

        let idx = record.units.len();
        push_reserved(
            &mut record.units,
            BestEffortUnit {
                index: idx,
                raw_argv: cmd_argv,
                effective_argv: f_eff,
                alt_argv: f_alt,
                assignments: Vec::new(),
                redirects: Vec::new(),
                pipeline_id: None,
                piped: false,
                depth: exec_depth,
                derived: true,
            },
        );

        if exec_depth < limits::MAX_DEPTH {
            for p in exec_payloads.into_vec() {
                vec_push_budgeted(deferred_payloads, (exec_depth + 1, p), budget).map_err(ex)?;
            }
            if !nested_find_execs.is_empty() {
                process_find_exec_cmds(
                    nested_find_execs,
                    exec_depth + 1,
                    keep,
                    record,
                    deferred_payloads,
                    budget,
                )?;
            }
        }
    }
    Ok(())
}

/// Stores builders not yet converted, after exhaustion, into record slots
/// reserved by [`reserve_recovery_slots`]. Their effective reading is a copy
/// of the argv, pre-paid word by word when the words were stored.
fn flush_builders_raw(
    builders: impl Iterator<Item = BeBuilder>,
    depth: u8,
    derived: bool,
    record: &mut BestEffortRecord,
) {
    for b in builders {
        if record.units.len() >= limits::MAX_UNITS {
            break;
        }
        let eff = b.raw_argv.clone();
        push_converted(record, b, eff, Vec::new(), depth, derived);
    }
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
        vec_push_budgeted(heredocs, (delim, strip, quoted), budget)?;
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
        n += std::mem::size_of::<Vec<String>>();
        for s in v {
            n += s.len() + std::mem::size_of::<String>();
        }
    }
    for (k, v) in &u.assignments {
        n += k.len() + v.len() + std::mem::size_of::<(String, String)>();
    }
    for (_, t) in &u.redirects {
        n += t.len() + std::mem::size_of::<(BestEffortRedirectKind, String)>();
    }
    n
}

/// Sizes a copy of `units` for the optional memo, charging the sizing before
/// it runs. Returns `None` without exhausting the budget when the sizing is
/// unaffordable.
fn estimate_units_bytes_if_affordable(
    units: &[BestEffortUnit],
    budget: &mut ComputeBudget,
) -> Option<usize> {
    // Counting the strings visits each unit and each of its alternative
    // readings (at most `MAX_ALT_READINGS`).
    let count_cost = (units.len() as u64)
        .saturating_mul(2 + MAX_ALT_READINGS as u64)
        .saturating_add(1);
    if budget.is_exhausted() || budget.remaining_units() < count_cost {
        return None;
    }
    budget.charge(count_cost).ok()?;
    let strings: usize = units
        .iter()
        .map(|u| {
            u.raw_argv.len()
                + u.effective_argv.len()
                + u.alt_argv.len()
                + u.alt_argv.iter().map(Vec::len).sum::<usize>()
                + 2 * u.assignments.len()
                + u.redirects.len()
        })
        .sum();
    let walk = strings as u64 + 1;
    if budget.remaining_units() < walk {
        return None;
    }
    budget.charge(walk).ok()?;
    Some(units.iter().map(estimate_unit_bytes).sum())
}

// ---------------------------------------------------------------------------
// CR-19 helpers & tolerant wrapper unwrapping for best-effort pass (§2, §12)
// ---------------------------------------------------------------------------

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
    if lookup_tool(&folded, budget)?.is_some_and(ToolSpec::is_nested_shell) {
        return Ok(true);
    }
    Ok(lookup_outside(&folded, budget)?
        .is_some_and(|e| e.contract_section == "§4" && !e.reason.starts_with("Wrapper")))
}

fn is_be_reserved(raw: &str, budget: &mut ComputeBudget) -> Result<bool, Exhausted> {
    Ok(lookup_outside(raw, budget)?.is_some_and(|e| e.contract_section == "§3"))
}

/// Whether `folded` is an unconditional code runner in P0's table
/// (`ToolRowKind::CodeRunner`) for Scan 7 (here-doc / here-string body
/// extraction). `ToolRowKind::ConditionalRunner` (`find`, `git`, `sed`) is
/// deliberately excluded: `git` and plain `sed` read data on stdin (e.g.
/// `git commit -F - <<EOF`, `sed 's/a/b/' <<EOF`), and `find`'s `-exec*` /
/// `-ok*` triggers are already extracted by Scan 5.
fn is_be_code_runner(folded: &str, budget: &mut ComputeBudget) -> Result<bool, Exhausted> {
    Ok(lookup_tool(folded, budget)?.is_some_and(|t| matches!(t.kind, ToolRowKind::CodeRunner(_))))
}

/// Compares against constants only (at most 9 bytes each).
fn is_be_interpreter(folded: &str) -> bool {
    folded.starts_with("python")
        || matches!(
            folded,
            "perl" | "ruby" | "node" | "deno" | "bun" | "php" | "lua" | "osascript" | "pwsh"
        )
}

/// Two substring scans; callers charge `2 * w.len()` before calling.
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
    // `trim` (both ends) and `split_whitespace` each scan at most the body.
    budget.charge((subs[0].len() as u64).saturating_mul(2).max(1))?;
    let body = subs[0].trim();
    let mut it = body.split_whitespace();
    let w0 = it.next();
    let w1 = it.next();
    let w2 = it.next();
    let w3 = it.next();
    let candidate = match (w0, w1, w2, w3) {
        (Some(w0), Some(name), None, None) => {
            let f = fold_str_cr19(w0, budget)?;
            if matches!(f.as_str(), "which" | "whence" | "echo") {
                Some(name)
            } else {
                None
            }
        }
        (Some(w0), Some("-v" | "-V"), Some(name), None) => {
            let f = fold_str_cr19(w0, budget)?;
            (f == "command").then_some(name)
        }
        (Some(w0), Some("-p" | "-P"), Some(name), None) => {
            let f = fold_str_cr19(w0, budget)?;
            (f == "type").then_some(name)
        }
        _ => None,
    };
    let Some(c) = candidate else {
        return Ok(None);
    };
    budget.charge(c.len().max(1) as u64)?;
    if c.is_empty() || c.contains(['$', '`', '*', '?', '[']) {
        return Ok(None);
    }
    Ok(Some(clone_string_budgeted(c, budget)?))
}

/// Tolerantly strips reserved words and wrappers (CR-19 folded) up to
/// `MAX_BEST_EFFORT_WRAP` steps, returning `(effective_argv, alt_argv)`.
struct BestEffortUnwrap {
    effective: Vec<String>,
    alt: Vec<Vec<String>>,
    wrapper_payloads: Vec<String>,
}

/// Unwraps `cur` (an owned copy of the argv, already charged by the caller).
///
/// The words stripped are dropped once, at the end, with one shift of the
/// argv charged by its length, instead of one shift per wrapper. On
/// exhaustion the argv is handed back unshifted, so the caller can still
/// record it as the unit's effective reading.
fn unwrap_best_effort(
    mut cur: Vec<String>,
    budget: &mut ComputeBudget,
) -> Result<BestEffortUnwrap, (Vec<String>, Exhausted)> {
    let mut alts: Vec<Vec<String>> = Vec::new();
    let mut wrapper_payloads: Vec<String> = Vec::new();
    let mut start = 0usize;

    let scan = (|| -> Result<(), Exhausted> {
        for _ in 0..MAX_BEST_EFFORT_WRAP {
            let rest = &cur[start..];
            let Some(first) = rest.first() else {
                break;
            };
            // Reserved words match the un-folded basename (a scan from the end).
            budget.charge(first.len().max(1) as u64)?;
            let base_raw = first.rsplit('/').next().unwrap_or(first.as_str());
            if matches!(
                base_raw,
                "{" | "}"
                    | "!"
                    | "do"
                    | "then"
                    | "else"
                    | "elif"
                    | "if"
                    | "while"
                    | "until"
                    | "coproc"
            ) {
                start += 1;
                continue;
            }
            let folded = fold_str_cr19(first, budget)?;
            if folded == "env" {
                let env_payloads = extract_env_payloads(rest, budget)?;
                vec_extend_budgeted(&mut wrapper_payloads, env_payloads, budget)?;
            } else if matches!(folded.as_str(), "script" | "flock") {
                let sf_payloads = extract_script_flock_payloads(rest, budget)?;
                vec_extend_budgeted(&mut wrapper_payloads, sf_payloads, budget)?;
            }
            let Some((skip, maybe_alt_skip)) = wrapper_skip_be(&folded, rest, budget)? else {
                break;
            };
            if let Some(alt_skip) = maybe_alt_skip
                && alt_skip != skip
                && alt_skip < rest.len()
                && alts.len() < MAX_ALT_READINGS
            {
                // Reserve the reading's slot before deriving it.
                charge_vec_growth(&mut alts, 1, budget)?;
                let alt_slice = clone_vec_string_budgeted(&rest[alt_skip..], budget)?;
                if !alt_slice.is_empty() && !has_reading(&alts, &alt_slice, budget)? {
                    push_reserved(&mut alts, alt_slice);
                }
            }
            if skip == 0 || skip > rest.len() {
                break;
            }
            start += skip;
        }
        if start > 0 {
            budget.charge(cur.len() as u64)?;
        }
        Ok(())
    })();
    if let Err(e) = scan {
        return Err((cur, e));
    }
    cur.drain(..start);

    Ok(BestEffortUnwrap {
        effective: cur,
        alt: alts,
        wrapper_payloads,
    })
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
                budget.charge(1 + 2 * argv[j].len() as u64)?;
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
                budget.charge(1 + argv[j].len() as u64)?;
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
                budget.charge(w.len().max(1) as u64)?;
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
            budget.charge(1 + 2 * eff[j].len() as u64)?;
            let w = eff[j].as_str();
            if w == "--" {
                j += 1;
                break;
            }
            if let Some(rest) = w
                .strip_prefix("--command=")
                .or_else(|| w.strip_prefix("--init-command="))
            {
                let s = clone_string_budgeted(rest, budget)?;
                vec_push_budgeted(&mut out, s, budget)?;
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
                    let s = clone_string_budgeted(code, budget)?;
                    vec_push_budgeted(&mut out, s, budget)?;
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
            let s = clone_string_budgeted(code, budget)?;
            vec_push_budgeted(&mut out, s, budget)?;
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
            vec_push_budgeted(&mut out, args.join(" "), budget)?;
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
        budget.charge(1 + argv[j].len() as u64)?;
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
                vec_push_budgeted(&mut out, combined, budget)?;
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
            vec_push_budgeted(&mut out, combined, budget)?;
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
                let s = clone_string_budgeted(cmd, budget)?;
                vec_push_budgeted(&mut out, s, budget)?;
            }
            j += 2;
        } else if let Some(cmd) = w
            .strip_prefix("--command=")
            .or_else(|| w.strip_prefix("-c").filter(|r| !r.is_empty()))
        {
            let s = clone_string_budgeted(cmd, budget)?;
            vec_push_budgeted(&mut out, s, budget)?;
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
                let cmd = clone_vec_string_budgeted(&eff[start..j], budget)?;
                vec_push_budgeted(&mut out, cmd, budget)?;
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
        budget.charge(3 * line.len() as u64 + 1)?;
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
                budget.charge(1 + eff[j].len() as u64)?;
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
                let total_len: usize = eff[j..].iter().map(|s| s.len() + 1).sum();
                budget.charge(total_len as u64)?;
                budget.charge_alloc(total_len)?;
                vec_push_budgeted(&mut out, eff[j..].join(" "), budget)?;
            }
        }
        "printf" => {
            let mut j = 1usize;
            if eff.get(j).map(String::as_str) == Some("--") {
                j += 1;
            }
            for w in &eff[j.min(eff.len())..] {
                let s = clone_string_budgeted(w, budget)?;
                vec_push_budgeted(&mut out, s, budget)?;
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
    // One charged pass finds both `{` and `$`.
    budget.charge(w.len().max(1) as u64)?;
    let (has_brace, has_dollar) = w
        .bytes()
        .fold((false, false), |(b, d), c| (b || c == b'{', d || c == b'$'));
    if !has_brace {
        let mut out = vec_with_capacity_budgeted(1, budget)?;
        out.push(clone_string_budgeted(w, budget)?);
        return Ok(Some(out));
    }
    if !has_dollar && brace_results_lower_bound(w, budget)? > MAX_BRACE_RESULTS {
        return Ok(None);
    }

    let mut work: Vec<String> = Vec::new();
    let initial = clone_string_budgeted(w, budget)?;
    vec_push_budgeted(&mut work, initial, budget)?;
    let mut out: Vec<String> = Vec::new();
    let mut scanned = 0usize;

    while let Some(x) = work.pop() {
        scanned = scanned.saturating_add(x.len());
        if scanned > MAX_BRACE_WORK {
            return Ok(None);
        }
        budget.charge(x.len().max(1) as u64)?;
        let mut cs = vec_with_capacity_budgeted(x.len(), budget)?;
        cs.extend(x.chars());
        // The brace-matching pass below walks `cs` once more.
        budget.charge(cs.len().max(1) as u64)?;

        // (open, commas, holds_nested_pair)
        let mut stack: Vec<(usize, Vec<usize>, bool)> = Vec::new();
        let mut best: Option<(usize, usize, BraceWords)> = None;

        for (i, &c) in cs.iter().enumerate() {
            match c {
                '{' => vec_push_budgeted(&mut stack, (i, Vec::new(), false), budget)?,
                ',' => {
                    if let Some(top) = stack.last_mut() {
                        vec_push_budgeted(&mut top.1, i, budget)?;
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
                        let inner_slice = &cs[open + 1..i];
                        let inner_bytes: usize = inner_slice.iter().map(|ch| ch.len_utf8()).sum();
                        budget.charge(inner_bytes.max(1) as u64)?;
                        budget.charge_alloc(inner_bytes)?;
                        let mut inner = String::with_capacity(inner_bytes);
                        for &ch in inner_slice {
                            inner.push(ch);
                        }
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
            None => vec_push_budgeted(&mut out, x, budget)?,
            Some((open, close, alts)) => {
                let alts: Vec<String> = match alts {
                    Ok(v) => v,
                    Err(commas) => {
                        let bounds_len = commas.len().saturating_add(2);
                        budget.charge(bounds_len as u64)?;
                        let mut bounds = vec_with_capacity_budgeted(bounds_len, budget)?;
                        bounds.push(open);
                        bounds.extend(commas);
                        bounds.push(close);
                        let mut v = vec_with_capacity_budgeted(bounds.len() - 1, budget)?;
                        for k in 0..bounds.len() - 1 {
                            let slice = &cs[bounds[k] + 1..bounds[k + 1]];
                            let byte_len: usize = slice.iter().map(|ch| ch.len_utf8()).sum();
                            budget.charge(byte_len.max(1) as u64)?;
                            budget.charge_alloc(byte_len)?;
                            let mut s = String::with_capacity(byte_len);
                            for &ch in slice {
                                s.push(ch);
                            }
                            v.push(s);
                        }
                        v
                    }
                };
                if work.len() + out.len() + alts.len() > MAX_BRACE_RESULTS {
                    return Ok(None);
                }
                let pre_slice = &cs[..open];
                let post_slice = &cs[close + 1..];
                let pre_bytes: usize = pre_slice.iter().map(|ch| ch.len_utf8()).sum();
                let post_bytes: usize = post_slice.iter().map(|ch| ch.len_utf8()).sum();
                budget.charge((pre_bytes + post_bytes).max(1) as u64)?;
                budget.charge_alloc(pre_bytes.saturating_add(post_bytes))?;
                let mut pre = String::with_capacity(pre_bytes);
                for &ch in pre_slice {
                    pre.push(ch);
                }
                let mut post = String::with_capacity(post_bytes);
                for &ch in post_slice {
                    post.push(ch);
                }
                for alt in alts {
                    let len = pre.len() + alt.len() + post.len();
                    budget.charge(len.max(1) as u64)?;
                    budget.charge_alloc(len)?;
                    let mut combined = String::with_capacity(len);
                    combined.push_str(&pre);
                    combined.push_str(&alt);
                    combined.push_str(&post);
                    vec_push_budgeted(&mut work, combined, budget)?;
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
    // `split("..")`, then the digit checks and parses of each part: three passes.
    budget.charge((inner.len() as u64).saturating_mul(3).max(1))?;
    let mut split = inner.split("..");
    let Some(a) = split.next() else {
        return Ok(None);
    };
    let Some(b) = split.next() else {
        return Ok(None);
    };
    let step_part = split.next();
    if split.next().is_some() {
        return Ok(None);
    }
    let step: i128 = match step_part {
        Some(s) => {
            let Ok(v) = s.parse::<i64>() else {
                return Ok(None);
            };
            i128::from(v).abs().max(1)
        }
        None => 1,
    };
    let letter = |s: &str| {
        let mut it = s.chars();
        match (it.next(), it.next()) {
            (Some(c), None) if c.is_ascii_alphabetic() => Some(c),
            _ => None,
        }
    };
    let mut out: Vec<String> = Vec::new();
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
            budget.charge(1)?;
            budget.charge_alloc(1)?;
            let s = char::from(byte).to_string();
            vec_push_budgeted(&mut out, s, budget)?;
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
        let s = padded_number(v, width, budget)?;
        vec_push_budgeted(&mut out, s, budget)?;
        v += dir * step;
    }
    Ok(Some(out))
}

/// `v` zero-padded to `width` like `{a..b}` with a padded endpoint (bash:
/// `-` then the magnitude padded to `width - 1` for negatives), built in an
/// exactly sized `String` charged before it is allocated.
fn padded_number(v: i128, width: usize, budget: &mut ComputeBudget) -> Result<String, Exhausted> {
    let neg = v < 0;
    let mut digits = [0u8; 40];
    let mut n = v.unsigned_abs();
    let mut k = 0usize;
    loop {
        digits[k] = b'0' + (n % 10) as u8;
        n /= 10;
        k += 1;
        if n == 0 {
            break;
        }
    }
    let body_width = if neg { width.saturating_sub(1) } else { width };
    let zeros = body_width.saturating_sub(k);
    let len = usize::from(neg) + zeros + k;
    budget.charge(len as u64)?;
    budget.charge_alloc(len)?;
    let mut s = String::with_capacity(len);
    if neg {
        s.push('-');
    }
    s.extend(std::iter::repeat_n('0', zeros));
    s.extend(digits[..k].iter().rev().map(|&d| d as char));
    Ok(s)
}

fn brace_results_lower_bound(w: &str, budget: &mut ComputeBudget) -> Result<usize, Exhausted> {
    budget.charge(w.len().max(1) as u64)?;
    let mut cs = vec_with_capacity_budgeted(w.len(), budget)?;
    cs.extend(w.chars());
    // The pairing pass below walks `cs` once more.
    budget.charge(cs.len().max(1) as u64)?;

    // Record pairs in `open` order directly so the pass is strictly O(n) with
    // no sorting, and charge both auxiliary vectors before every growth (§8).
    let mut stack: Vec<usize> = Vec::new();
    // (open, close_or_max, commas, holds_nested_pair)
    let mut pairs: Vec<(usize, usize, usize, bool)> = Vec::new();
    for (i, &c) in cs.iter().enumerate() {
        match c {
            '{' => {
                let pair_idx = pairs.len();
                vec_push_budgeted(&mut pairs, (i, usize::MAX, 0, false), budget)?;
                vec_push_budgeted(&mut stack, pair_idx, budget)?;
            }
            ',' => {
                if let Some(&top_idx) = stack.last() {
                    pairs[top_idx].2 += 1;
                }
            }
            '}' => {
                if let Some(pair_idx) = stack.pop() {
                    pairs[pair_idx].1 = i;
                    if let Some(&parent_idx) = stack.last() {
                        pairs[parent_idx].3 = true;
                    }
                }
            }
            _ => {}
        }
    }

    if !pairs.is_empty() {
        budget.charge(pairs.len() as u64)?;
    }
    let mut bound = 1usize;
    let mut reach: Option<usize> = None;
    for &(open, close, commas, inner) in &pairs {
        if close == usize::MAX {
            continue;
        }
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
