//! Exact and best-effort shell lexers (owned by P1; contract §2, §3, §4, §8).
//!
//! # Budget discipline (§8)
//!
//! Every function in P1 follows three rules, so no work or allocation happens
//! before, or without, a charge to the shared [`ComputeBudget`]:
//!
//! 1. **Charge, then work.** Each scan or copy of input-derived bytes charges
//!    one compute unit per byte before it runs; each lookup in P0's tables
//!    charges its row and byte comparisons before it runs ([`lookup_tool`],
//!    [`lookup_outside`], [`flag_charged`]); each comparison of two
//!    input-derived strings charges the bytes it may compare ([`bytes_eq`]).
//!    Comparisons against compile-time constants are bounded by the constant's
//!    length and ride on the per-step charge.
//! 2. **Charge, then allocate.** Every `Vec`/`String` growth charges the new
//!    capacity (and the copy of the old elements) before reserving it
//!    ([`charge_vec_growth`], [`charge_string_growth`]).
//! 3. **Reserve, then discover.** Storage for a value (a word's argv slot, a
//!    redirect's slot, a reason's outcome slot, the pending unit's output slot
//!    and the final `Exhausted` outcome's slot) is reserved and charged before
//!    the work that produces the value starts. Storing a finished value is
//!    then infallible ([`push_reserved`]), so exhaustion never drops anything
//!    found earlier, and recovery after exhaustion allocates nothing.
//!
//! A word interrupted by exhaustion is dropped whole, never kept truncated: a
//! truncated `~/.ssh/id_rsa.pub` would read `~/.ssh/id_rsa`, and exhaustion
//! must never create a hit (§8).

use crate::policy::limits;
use crate::policy::model::{
    BestEffortRecord, ByteProvenance, Cap, ComputeBudget, Exhausted, FoldRules, LexExactOutput,
    ModelRedirect, PartId, PartOutcome, PartResult, PipelineId, RawCommandUnit, Separator, StdFd,
    UnitIndex, UnreadReason, Word,
};
use crate::policy::tools::{self, FlagSpec, OutsideModelEntry, ToolSpec};

mod best_effort;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub(crate) use best_effort::{brace_expand_budgeted, lex_best_effort_into};

// ---------------------------------------------------------------------------
// Character classification helpers (§3)
// ---------------------------------------------------------------------------

/// Bytes allowed unquoted anywhere in a word (contract §3):
/// `A-Z a-z 0-9 _ . / , : @ % + -`.
///
/// `=` is also allowed unquoted except as a word's first byte (zsh `=cmd`).
#[inline]
pub(super) const fn is_allowed_unquoted_base(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'_' | b'.' | b'/' | b',' | b':' | b'@' | b'%' | b'+' | b'-'
        )
}

/// Bytes allowed after an unquoted backslash outside quotes (contract §3):
/// allowed unquoted characters (including `=`) or glob characters (`*`, `?`,
/// `[`, `]`).
#[inline]
const fn is_escapable_outside_quotes(b: u8) -> bool {
    is_allowed_unquoted_base(b) || matches!(b, b'=' | b'*' | b'?' | b'[' | b']')
}

/// Shell token delimiters that end an unquoted word or tilde prefix.
#[inline]
const fn is_word_boundary(b: u8) -> bool {
    matches!(
        b,
        b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'<' | b'>' | b'(' | b')'
    )
}

/// Returns `true` iff `name` is a valid POSIX shell variable name
/// (`[A-Za-z_][A-Za-z0-9_]*`). Callers charge `name.len()` before calling.
pub(super) fn is_posix_name(name: &[u8]) -> bool {
    match name.first() {
        Some(&b) if b.is_ascii_alphabetic() || b == b'_' => name[1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'_'),
        _ => false,
    }
}

/// Incremental state machine tracking whether the unquoted prefix seen so far
/// before the first unquoted `=` in a word is a valid POSIX shell assignment
/// name (`[A-Za-z_][A-Za-z0-9_]*`, optionally followed by one `+` for `+=`).
///
/// Updates in O(1) per byte with zero prefix rescanning (contract §3, §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AssignPrefix {
    /// No bytes consumed yet in this word.
    Empty,
    /// Non-empty valid POSIX variable name `[A-Za-z_][A-Za-z0-9_]*`.
    ValidName,
    /// Valid POSIX variable name followed by one unquoted `+` (`NAME+=`).
    ValidNamePlus,
    /// Disqualified (invalid char, quote/escape/sub, or already saw `=`).
    Invalid,
}

impl AssignPrefix {
    #[inline]
    pub(super) fn advance_unquoted_byte(&mut self, b: u8) {
        *self = match *self {
            Self::Empty => {
                if b.is_ascii_alphabetic() || b == b'_' {
                    Self::ValidName
                } else {
                    Self::Invalid
                }
            }
            Self::ValidName => {
                if b.is_ascii_alphanumeric() || b == b'_' {
                    Self::ValidName
                } else if b == b'+' {
                    Self::ValidNamePlus
                } else {
                    Self::Invalid
                }
            }
            Self::ValidNamePlus | Self::Invalid => Self::Invalid,
        };
    }

    #[inline]
    pub(super) fn advance_unquoted_char(&mut self, ch: char) {
        if ch.is_ascii() {
            self.advance_unquoted_byte(ch as u8);
        } else {
            *self = Self::Invalid;
        }
    }

    #[inline]
    pub(super) const fn is_valid_before_eq(self) -> bool {
        matches!(self, Self::ValidName | Self::ValidNamePlus)
    }
}

/// Private per-unit token-level metadata the exact lexer hands to
/// `v4::units` for `HOME` tracking without changing P0's frozen
/// `LexExactOutput` / `RawCommandUnit` types (contract §3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct RawUnitLexMeta {
    /// Count of leading words in `RawCommandUnit::argv` classified by the
    /// exact lexer as unquoted shell assignments (`NAME=value` / `NAME+=value`
    /// with zero quoting/escaping before the first unquoted `=`).
    pub(super) leading_assignment_count: usize,
    /// `true` iff any of those leading assignment words targets `HOME`.
    pub(super) sets_home: bool,
}

// ---------------------------------------------------------------------------
// Charged primitives (§8): table lookups, comparisons, word scans
// ---------------------------------------------------------------------------

/// Longest name length tracked by the per-length row counts below. Every name
/// in P0's tables is shorter (checked at compile time).
const NAME_LEN_CAP: usize = 64;

const fn tool_name_len_counts() -> [u16; NAME_LEN_CAP + 1] {
    let t = tools::all_tools();
    let mut counts = [0u16; NAME_LEN_CAP + 1];
    let mut i = 0;
    while i < t.len() {
        let len = t[i].name.len();
        assert!(len <= NAME_LEN_CAP, "tool name longer than NAME_LEN_CAP");
        counts[len] += 1;
        i += 1;
    }
    counts
}

const fn outside_name_len_counts() -> [u16; NAME_LEN_CAP + 1] {
    let t = tools::outside_model();
    let mut counts = [0u16; NAME_LEN_CAP + 1];
    let mut i = 0;
    while i < t.len() {
        let len = t[i].name.len();
        assert!(
            len <= NAME_LEN_CAP,
            "outside-model name longer than NAME_LEN_CAP"
        );
        counts[len] += 1;
        i += 1;
    }
    counts
}

const fn outside_name_max_len() -> usize {
    let t = tools::outside_model();
    let mut max = 0;
    let mut i = 0;
    while i < t.len() {
        if t[i].name.len() > max {
            max = t[i].name.len();
        }
        i += 1;
    }
    max
}

const fn long_flag_max_len() -> usize {
    let t = tools::all_tools();
    let mut max = 0;
    let mut i = 0;
    while i < t.len() {
        let flags = t[i].flags();
        let mut j = 0;
        while j < flags.len() {
            if let Some(long) = flags[j].long
                && long.len() > max
            {
                max = long.len();
            }
            j += 1;
        }
        i += 1;
    }
    max
}

const TOOL_NAME_COUNTS: [u16; NAME_LEN_CAP + 1] = tool_name_len_counts();
const OUTSIDE_NAME_COUNTS: [u16; NAME_LEN_CAP + 1] = outside_name_len_counts();

/// Longest `OUTSIDE_MODEL` name: no longer word can be a §3 reserved word.
pub(super) const OUTSIDE_NAME_MAX: usize = outside_name_max_len();

/// Longest long flag in P0's flag tables: a long-flag comparison inspects at
/// most this many bytes.
const LONG_FLAG_MAX: usize = long_flag_max_len();

/// Work of `table.iter().find(|e| e.name == name)`: one length check per row,
/// plus a byte comparison of `len` bytes for each row of the same length.
#[inline]
fn table_scan_cost(rows: usize, counts: &[u16; NAME_LEN_CAP + 1], len: usize) -> u64 {
    let same_len = if len <= NAME_LEN_CAP {
        counts[len] as usize
    } else {
        0
    };
    rows.saturating_add(same_len.saturating_mul(len)).max(1) as u64
}

/// [`tools::lookup`], charged for its row scan before it runs.
pub(super) fn lookup_tool(
    name: &str,
    budget: &mut ComputeBudget,
) -> Result<Option<&'static ToolSpec>, Exhausted> {
    budget.charge(table_scan_cost(
        tools::all_tools().len(),
        &TOOL_NAME_COUNTS,
        name.len(),
    ))?;
    Ok(tools::lookup(name))
}

/// [`tools::lookup_outside_model`], charged for its row scan before it runs.
pub(super) fn lookup_outside(
    name: &str,
    budget: &mut ComputeBudget,
) -> Result<Option<&'static OutsideModelEntry>, Exhausted> {
    budget.charge(table_scan_cost(
        tools::outside_model().len(),
        &OUTSIDE_NAME_COUNTS,
        name.len(),
    ))?;
    Ok(tools::lookup_outside_model(name))
}

/// [`tools::flag`], charged before it runs: a prefix strip (O(1)), then one
/// comparison per flag row of at most `min(len, LONG_FLAG_MAX)` bytes.
pub(super) fn flag_charged(
    flags: &'static [FlagSpec],
    tok: &str,
    budget: &mut ComputeBudget,
) -> Result<Option<&'static FlagSpec>, Exhausted> {
    let per_row = 1 + tok.len().min(LONG_FLAG_MAX);
    budget.charge(flags.len().saturating_mul(per_row).max(1) as u64)?;
    Ok(tools::flag(flags, tok))
}

/// [`tools::flag_short`], charged one unit per flag row before it runs.
pub(super) fn flag_short_charged(
    flags: &'static [FlagSpec],
    short: char,
    budget: &mut ComputeBudget,
) -> Result<Option<&'static FlagSpec>, Exhausted> {
    budget.charge(flags.len().max(1) as u64)?;
    Ok(tools::flag_short(flags, short))
}

/// `a == b` on input-derived bytes, charged before it runs: one length check,
/// plus the byte comparison when the lengths are equal.
#[inline]
pub(super) fn bytes_eq(a: &[u8], b: &[u8], budget: &mut ComputeBudget) -> Result<bool, Exhausted> {
    let cmp = if a.len() == b.len() { a.len() } else { 0 };
    budget.charge(1 + cmp as u64)?;
    Ok(a == b)
}

/// [`Word::has_unquoted_glob`], charged one unit per byte before the scan.
#[inline]
pub(super) fn word_has_unquoted_glob(
    w: &Word,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    budget.charge(w.len().max(1) as u64)?;
    Ok(w.has_unquoted_glob())
}

/// [`Word::as_str`] (a UTF-8 validation scan), charged one unit per byte
/// before the scan.
#[inline]
pub(super) fn word_as_str<'w>(
    w: &'w Word,
    budget: &mut ComputeBudget,
) -> Result<Option<&'w str>, Exhausted> {
    budget.charge(w.len().max(1) as u64)?;
    Ok(w.as_str())
}

/// Returns `true` iff `s` is a shell reserved word outside §3's simple-command
/// model, charged for the table scan before it runs.
#[inline]
fn is_reserved_word(s: &str, budget: &mut ComputeBudget) -> Result<bool, Exhausted> {
    Ok(lookup_outside(s, budget)?.is_some_and(|e| e.contract_section == "§3"))
}

// ---------------------------------------------------------------------------
// Shared budgeted collection growth helpers (§8)
// ---------------------------------------------------------------------------

/// Charges capacity growth (both allocation bytes for the new capacity delta
/// and compute units for copying existing elements on reallocation) before
/// reserving space in `v` using geometric growth. Afterwards
/// `v.len() + additional <= v.capacity()`.
#[inline]
pub(super) fn charge_vec_growth<T>(
    v: &mut Vec<T>,
    additional: usize,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    if budget.is_exhausted() {
        return budget.charge(0);
    }
    let needed = v.len().saturating_add(additional);
    if needed > v.capacity() {
        let old_cap = v.capacity();
        let elem_size = std::mem::size_of::<T>();
        if elem_size == 0 {
            return Ok(());
        }
        let min_cap = if elem_size == 1 { 8 } else { 4 };
        let target_cap = old_cap.saturating_mul(2).max(needed).max(min_cap);
        let delta_elems = target_cap.saturating_sub(old_cap);
        let delta_bytes = delta_elems.saturating_mul(elem_size);
        if old_cap > 0 {
            budget.charge(old_cap as u64)?;
        }
        budget.charge_alloc(delta_bytes)?;
        v.reserve_exact(target_cap - v.len());
    }
    Ok(())
}

/// Pushes `val` into capacity reserved (and charged) earlier with
/// [`charge_vec_growth`]. Never allocates when the reservation rule is kept;
/// debug builds (the test suite) assert it.
#[inline]
pub(super) fn push_reserved<T>(v: &mut Vec<T>, val: T) {
    debug_assert!(
        v.len() < v.capacity(),
        "slot must be reserved and charged before the value is produced (§8)"
    );
    v.push(val);
}

/// Pushes `val` into `v`, charging any capacity growth before allocating.
///
/// Only for values whose loss on exhaustion loses nothing found earlier
/// (work items, or values produced in the same step). Finished results use a
/// reservation made before their discovery and [`push_reserved`].
#[inline]
pub(super) fn vec_push_budgeted<T>(
    v: &mut Vec<T>,
    val: T,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    charge_vec_growth(v, 1, budget)?;
    v.push(val);
    Ok(())
}

/// Extends `v` with items from `items`, charging capacity growth before
/// allocating.
#[inline]
pub(super) fn vec_extend_budgeted<T>(
    v: &mut Vec<T>,
    items: Vec<T>,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    if !items.is_empty() {
        charge_vec_growth(v, items.len(), budget)?;
        budget.charge(items.len() as u64)?;
        v.extend(items);
    }
    Ok(())
}

/// Allocates a `Vec<T>` with exact capacity `cap`, charging its backing buffer
/// before allocating.
#[inline]
pub(super) fn vec_with_capacity_budgeted<T>(
    cap: usize,
    budget: &mut ComputeBudget,
) -> Result<Vec<T>, Exhausted> {
    if budget.is_exhausted() {
        budget.charge(0)?;
    }
    if cap == 0 {
        return Ok(Vec::new());
    }
    let bytes = cap.saturating_mul(std::mem::size_of::<T>());
    budget.charge_alloc(bytes)?;
    Ok(Vec::with_capacity(cap))
}

/// Charges `String` capacity growth (allocation delta + copy work on realloc)
/// before reserving space using geometric growth.
#[inline]
pub(super) fn charge_string_growth(
    s: &mut String,
    additional: usize,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    if budget.is_exhausted() {
        return budget.charge(0);
    }
    let needed = s.len().saturating_add(additional);
    if needed > s.capacity() {
        let old_cap = s.capacity();
        let target_cap = old_cap.saturating_mul(2).max(needed).max(8);
        let delta_bytes = target_cap.saturating_sub(old_cap);
        if old_cap > 0 {
            budget.charge(old_cap as u64)?;
        }
        budget.charge_alloc(delta_bytes)?;
        s.reserve_exact(target_cap - s.len());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Exact-lexer output sinks (§3, §8)
// ---------------------------------------------------------------------------

/// Destination of the exact lexer's units and outcomes.
///
/// The lexer reserves before it discovers (module docs, rule 3): `reserve_*`
/// charge and make room, `push_*` store into that room and never allocate.
/// `lex_exact` stores into a [`LexExactOutput`]; `v4::units` stores straight
/// into its `ParsedCommandUnits`, so nothing is copied between stages.
pub(super) trait ExactSink {
    /// Units this lexing call has stored so far (the pending unit's local
    /// index).
    fn local_units(&self) -> usize;
    /// `true` when no further unit fits under `MAX_UNITS`.
    fn at_unit_cap(&self) -> bool;
    /// Ensures one free unit slot (and its metadata slot).
    fn reserve_unit(&mut self, budget: &mut ComputeBudget) -> Result<(), Exhausted>;
    /// Ensures at least `free` free outcome slots.
    fn reserve_outcomes(
        &mut self,
        free: usize,
        budget: &mut ComputeBudget,
    ) -> Result<(), Exhausted>;
    /// Stores a unit into a reserved slot.
    fn push_unit(&mut self, unit: RawCommandUnit, meta: RawUnitLexMeta);
    /// Stores an outcome of local unit `local` into a reserved slot.
    fn push_unit_outcome(&mut self, local: usize, outcome: PartOutcome);
    /// Stores an outcome of the lexed text as a whole (the call, or the
    /// parent `-c` unit) into a reserved slot.
    fn push_text_outcome(&mut self, outcome: PartOutcome);
}

/// [`ExactSink`] behind P0's `lex_exact`.
struct OutputSink {
    out: LexExactOutput,
    whole: PartId,
}

impl ExactSink for OutputSink {
    fn local_units(&self) -> usize {
        self.out.units.len()
    }

    fn at_unit_cap(&self) -> bool {
        self.out.units.len() >= limits::MAX_UNITS
    }

    fn reserve_unit(&mut self, budget: &mut ComputeBudget) -> Result<(), Exhausted> {
        charge_vec_growth(&mut self.out.units, 1, budget)
    }

    fn reserve_outcomes(
        &mut self,
        free: usize,
        budget: &mut ComputeBudget,
    ) -> Result<(), Exhausted> {
        charge_vec_growth(&mut self.out.outcomes, free, budget)
    }

    fn push_unit(&mut self, unit: RawCommandUnit, _meta: RawUnitLexMeta) {
        push_reserved(&mut self.out.units, unit);
    }

    fn push_unit_outcome(&mut self, local: usize, outcome: PartOutcome) {
        push_reserved(
            &mut self.out.outcomes,
            PartResult {
                part: PartId::Unit(UnitIndex(local)),
                outcome,
            },
        );
    }

    fn push_text_outcome(&mut self, outcome: PartOutcome) {
        push_reserved(
            &mut self.out.outcomes,
            PartResult {
                part: self.whole.clone(),
                outcome,
            },
        );
    }
}

/// Unread reasons of the pending unit, plus the sink they are flushed into.
///
/// Invariant: the sink always has `pending.len() + 1` free outcome slots, so
/// moving the pending reasons and recording the final `Exhausted` never
/// allocate.
pub(super) struct ReasonSink<'s> {
    pending: Vec<UnreadReason>,
    out: &'s mut dyn ExactSink,
}

impl ReasonSink<'_> {
    /// Reserves room for `n` more pending reasons (and their outcome slots).
    fn reserve(&mut self, n: usize, budget: &mut ComputeBudget) -> Result<(), Exhausted> {
        charge_vec_growth(&mut self.pending, n, budget)?;
        let free = self.pending.len().saturating_add(n).saturating_add(1);
        self.out.reserve_outcomes(free, budget)
    }

    /// Stores a reason into room reserved with [`Self::reserve`].
    fn push_reserved(&mut self, reason: UnreadReason) {
        push_reserved(&mut self.pending, reason);
    }
}

/// Why the exact lexer core stopped without finishing normally.
pub(super) enum CoreStop {
    /// `MAX_BYTES` or `MAX_UNITS` (§8: `TooLarge`).
    TooLarge(Cap),
    /// The budget was exhausted on entry, before anything was lexed or any
    /// slot could be reserved to record it.
    Entry(Exhausted),
}

// ---------------------------------------------------------------------------
// Budgeted exact scanner (§3, §8)
// ---------------------------------------------------------------------------

/// Builds a [`Word`]'s byte and provenance vectors using budgeted geometric
/// capacity growth, charging both reallocation copy work and allocated capacity
/// deltas before allocating, then constructs `Word` via P0's `Word::from_parts`
/// (contract §3, §8). Tracks the word's glob and quoting flags while it is
/// built, so nothing rescans a finished word.
struct WordBuilder {
    bytes: Vec<u8>,
    provenance: Vec<ByteProvenance>,
    /// Same as [`Word::has_unquoted_glob`] on the finished word.
    unquoted_glob: bool,
    /// Same as [`Word::is_all_unquoted`] on the finished word.
    all_unquoted: bool,
}

impl WordBuilder {
    const fn new() -> Self {
        Self {
            bytes: Vec::new(),
            provenance: Vec::new(),
            unquoted_glob: false,
            all_unquoted: true,
        }
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    #[inline]
    fn note(&mut self, byte: u8, prov: ByteProvenance) {
        if prov == ByteProvenance::Unquoted {
            if matches!(byte, b'*' | b'?' | b'[') {
                self.unquoted_glob = true;
            }
        } else {
            self.all_unquoted = false;
        }
    }

    /// Charges capacity growth (both copy units on reallocation and newly
    /// allocated capacity bytes across both backing buffers) BEFORE reserving.
    #[inline]
    fn reserve_growth(
        &mut self,
        additional: usize,
        budget: &mut ComputeBudget,
    ) -> Result<(), Exhausted> {
        if budget.is_exhausted() {
            return budget.charge(0);
        }
        let needed = self.bytes.len().saturating_add(additional);
        let cap_b = self.bytes.capacity();
        let cap_p = self.provenance.capacity();
        if needed > cap_b || needed > cap_p {
            let min_cap = 8usize;
            let target_b = if needed > cap_b {
                cap_b.saturating_mul(2).max(needed).max(min_cap)
            } else {
                cap_b
            };
            let target_p = if needed > cap_p {
                cap_p.saturating_mul(2).max(needed).max(min_cap)
            } else {
                cap_p
            };
            let copy_units = (if target_b > cap_b { cap_b } else { 0 })
                .saturating_add(if target_p > cap_p { cap_p } else { 0 });
            let delta_bytes = target_b
                .saturating_sub(cap_b)
                .saturating_add(target_p.saturating_sub(cap_p));
            if copy_units > 0 {
                budget.charge(copy_units as u64)?;
            }
            if delta_bytes > 0 {
                budget.charge_alloc(delta_bytes)?;
            }
            if target_b > cap_b {
                self.bytes.reserve_exact(target_b - self.bytes.len());
            }
            if target_p > cap_p {
                self.provenance
                    .reserve_exact(target_p - self.provenance.len());
            }
        }
        Ok(())
    }

    fn finish(self) -> Word {
        Word::from_parts(self.bytes, self.provenance)
            .expect("WordBuilder keeps bytes and provenance lengths equal")
    }
}

struct ExactScanner<'a, 'b> {
    bytes: &'a [u8],
    pos: usize,
    budget: &'b mut ComputeBudget,
}

impl<'a, 'b> ExactScanner<'a, 'b> {
    fn new(line: &'a str, budget: &'b mut ComputeBudget) -> Self {
        Self {
            bytes: line.as_bytes(),
            pos: 0,
            budget,
        }
    }

    #[inline]
    fn is_eof(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    #[inline]
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    #[inline]
    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.pos
            .checked_add(offset)
            .and_then(|i| self.bytes.get(i).copied())
    }

    /// Charges 1 compute unit before consuming and returning the next byte.
    #[inline]
    fn next_byte(&mut self) -> Result<Option<u8>, Exhausted> {
        if self.pos >= self.bytes.len() {
            return Ok(None);
        }
        self.budget.charge(1)?;
        let b = self.bytes[self.pos];
        self.pos += 1;
        Ok(Some(b))
    }

    /// Charges `n` compute units before advancing `n` bytes (clamped to EOF).
    #[inline]
    fn advance_n(&mut self, n: usize) -> Result<(), Exhausted> {
        let take = n.min(self.bytes.len() - self.pos);
        if take > 0 {
            self.budget.charge(take as u64)?;
            self.pos += take;
        }
        Ok(())
    }

    /// Charges copy work + any budgeted capacity growth across both `Word`
    /// buffers before pushing one byte and its provenance.
    #[inline]
    fn push_word_byte(
        &mut self,
        word: &mut WordBuilder,
        byte: u8,
        prov: ByteProvenance,
    ) -> Result<(), Exhausted> {
        self.budget.charge(1)?;
        word.reserve_growth(1, self.budget)?;
        word.note(byte, prov);
        word.bytes.push(byte);
        word.provenance.push(prov);
        Ok(())
    }

    #[inline]
    fn charge_word_meta(&mut self) -> Result<(), Exhausted> {
        self.budget.charge(1)
    }

    #[inline]
    fn charge_unit_meta(&mut self) -> Result<(), Exhausted> {
        self.budget.charge(1)
    }

    #[inline]
    fn charge_unread_meta(&mut self, extra_bytes: usize) -> Result<(), Exhausted> {
        self.budget.charge(1)?;
        if extra_bytes > 0 {
            self.budget.charge_alloc(extra_bytes)?;
        }
        Ok(())
    }

    /// Consumes a single UTF-8 scalar value starting at `self.pos` (already
    /// known to be non-EOF) and appends its bytes to `word` with `prov`.
    /// Charges 1 compute unit per byte scanned + copy/growth before pushing.
    fn consume_utf8_char_into(
        &mut self,
        word: &mut WordBuilder,
        prov: ByteProvenance,
    ) -> Result<u8, Exhausted> {
        let first = self.bytes[self.pos];
        let char_len = if first < 0x80 {
            1
        } else if first & 0xE0 == 0xC0 {
            2
        } else if first & 0xF0 == 0xE0 {
            3
        } else if first & 0xF8 == 0xF0 {
            4
        } else {
            1
        };
        let end = (self.pos + char_len).min(self.bytes.len());
        let len = end - self.pos;
        self.budget.charge(len as u64)?;
        self.budget.charge(len as u64)?;
        word.reserve_growth(len, self.budget)?;
        for &b in &self.bytes[self.pos..end] {
            word.note(b, prov);
            word.bytes.push(b);
            word.provenance.push(prov);
        }
        self.pos = end;
        Ok(first)
    }

    fn skip_horiz_ws(&mut self) -> Result<(), Exhausted> {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.next_byte()?;
        }
        Ok(())
    }
}

struct PendingUnit {
    argv: Vec<Word>,
    redirects: Vec<ModelRedirect>,
    separator_before: Separator,
    pipeline_id: Option<PipelineId>,
    seen_command_word: bool,
    leading_assignment_count: usize,
    leading_sets_home: bool,
}

impl PendingUnit {
    const fn new(separator_before: Separator, pipeline_id: Option<PipelineId>) -> Self {
        Self {
            argv: Vec::new(),
            redirects: Vec::new(),
            separator_before,
            pipeline_id,
            seen_command_word: false,
            leading_assignment_count: 0,
            leading_sets_home: false,
        }
    }

    fn is_empty(&self) -> bool {
        self.argv.is_empty() && self.redirects.is_empty()
    }
}

struct ReadWordOutcome {
    word: Word,
    /// True iff the token consisted purely of unquoted ASCII digits (no
    /// quotes, no escapes, non-empty).
    all_unquoted_digits: bool,
    /// Saturating value of those digits (meaningful iff `all_unquoted_digits`),
    /// computed while scanning so a digit word is never rescanned.
    digits_value: u32,
    /// True iff any quote (`'`, `"`) or escape (`\`) occurred anywhere in this
    /// token, even if the quoted span emitted zero bytes (e.g. `''if`, `if""`).
    contained_quoting: bool,
    /// Same as [`Word::has_unquoted_glob`] on `word`, tracked while scanning.
    has_unquoted_glob: bool,
    /// Same as [`Word::is_all_unquoted`] on `word`, tracked while scanning.
    all_unquoted: bool,
    /// `Some(name_end)` iff the token is a leading shell assignment
    /// (`NAME=value` or `NAME+=value`) where the entire prefix through the
    /// first unquoted `=` contained zero quoting or escaping (so `''FOO=1` and
    /// `FOO''=1` yield `None`, while `FOO=''` and `FOO="bar"` yield `Some(3)`).
    /// `word.literal_bytes()[..name_end]` is the assignment variable name.
    assignment_name_end: Option<usize>,
    /// True iff the token was a single standalone unquoted `{`, `}`, `!`, or
    /// `[[` (reserved words that contain bytes outside the allowed set). The
    /// reason's string was charged before the token was consumed.
    standalone_reserved: Option<&'static str>,
}

impl ReadWordOutcome {
    fn from_builder(word: WordBuilder) -> Self {
        Self {
            has_unquoted_glob: word.unquoted_glob,
            all_unquoted: word.all_unquoted,
            word: word.finish(),
            all_unquoted_digits: false,
            digits_value: 0,
            contained_quoting: false,
            assignment_name_end: None,
            standalone_reserved: None,
        }
    }
}

fn push_unread(
    scanner: &mut ExactScanner<'_, '_>,
    sink: &mut ReasonSink<'_>,
    reason: UnreadReason,
) -> Result<(), Exhausted> {
    let extra = match &reason {
        UnreadReason::ReservedWordOrConstruct { construct } => construct.len(),
        UnreadReason::UnsupportedRedirect { op } => op.len(),
        _ => 0,
    };
    scanner.charge_unread_meta(extra)?;
    sink.reserve(1, scanner.budget)?;
    sink.push_reserved(reason);
    Ok(())
}

fn push_unread_construct(
    scanner: &mut ExactScanner<'_, '_>,
    sink: &mut ReasonSink<'_>,
    construct: &str,
) -> Result<(), Exhausted> {
    scanner.charge_unread_meta(construct.len())?;
    sink.reserve(1, scanner.budget)?;
    sink.push_reserved(UnreadReason::ReservedWordOrConstruct {
        construct: construct.to_string(),
    });
    Ok(())
}

fn push_unread_redirect_op(
    scanner: &mut ExactScanner<'_, '_>,
    sink: &mut ReasonSink<'_>,
    op: &str,
) -> Result<(), Exhausted> {
    scanner.charge_unread_meta(op.len())?;
    sink.reserve(1, scanner.budget)?;
    sink.push_reserved(UnreadReason::UnsupportedRedirect { op: op.to_string() });
    Ok(())
}

/// Scans `'...'` (with opening `'` at `scanner.peek()`), preserving
/// `ByteProvenance::SingleQuoted`. When `class_len` is `Some`, also enforces
/// §3's no-`!`/no-`^` rule inside glob classes and tracks class byte count.
fn scan_single_quoted(
    scanner: &mut ExactScanner<'_, '_>,
    word: &mut WordBuilder,
    sink: &mut ReasonSink<'_>,
    mut class_len: Option<&mut usize>,
) -> Result<(), Exhausted> {
    scanner.next_byte()?;
    let mut closed = false;
    while let Some(c) = scanner.next_byte()? {
        if c == b'\'' {
            closed = true;
            break;
        }
        if let Some(ref mut clen) = class_len {
            if matches!(c, b'!' | b'^') {
                push_unread(scanner, sink, UnreadReason::NegatedGlobClass)?;
            }
            **clen += 1;
        }
        scanner.push_word_byte(word, c, ByteProvenance::SingleQuoted)?;
    }
    if !closed {
        push_unread(scanner, sink, UnreadReason::UnsupportedQuoteOrEscape)?;
    }
    Ok(())
}

/// Scans `"..."` (with opening `"` at `scanner.peek()`), preserving
/// `ByteProvenance::DoubleQuoted` / `ByteProvenance::Escaped`. When `class_len`
/// is `Some`, also enforces §3's no-`!`/no-`^` rule inside glob classes and
/// tracks class byte count.
fn scan_double_quoted(
    scanner: &mut ExactScanner<'_, '_>,
    word: &mut WordBuilder,
    sink: &mut ReasonSink<'_>,
    mut class_len: Option<&mut usize>,
) -> Result<(), Exhausted> {
    scanner.next_byte()?;
    let mut closed = false;
    while let Some(c) = scanner.next_byte()? {
        match c {
            b'"' => {
                closed = true;
                break;
            }
            b'$' | b'`' | b'!' => {
                push_unread(scanner, sink, UnreadReason::UnsupportedQuoteOrEscape)?;
                if let Some(ref mut clen) = class_len {
                    if c == b'!' {
                        push_unread(scanner, sink, UnreadReason::NegatedGlobClass)?;
                    }
                    **clen += 1;
                }
                scanner.push_word_byte(word, c, ByteProvenance::DoubleQuoted)?;
            }
            b'\\' => match scanner.next_byte()? {
                Some(esc @ (b'"' | b'\\')) => {
                    if let Some(ref mut clen) = class_len {
                        **clen += 1;
                    }
                    scanner.push_word_byte(word, esc, ByteProvenance::Escaped)?;
                }
                Some(other) => {
                    push_unread(scanner, sink, UnreadReason::UnsupportedQuoteOrEscape)?;
                    if let Some(ref mut clen) = class_len {
                        if matches!(other, b'!' | b'^') {
                            push_unread(scanner, sink, UnreadReason::NegatedGlobClass)?;
                        }
                        **clen += 2;
                    }
                    scanner.push_word_byte(word, b'\\', ByteProvenance::DoubleQuoted)?;
                    scanner.push_word_byte(word, other, ByteProvenance::DoubleQuoted)?;
                }
                None => {
                    push_unread(scanner, sink, UnreadReason::UnsupportedQuoteOrEscape)?;
                    break;
                }
            },
            other => {
                if let Some(ref mut clen) = class_len {
                    if other == b'^' {
                        push_unread(scanner, sink, UnreadReason::NegatedGlobClass)?;
                    }
                    **clen += 1;
                }
                scanner.push_word_byte(word, other, ByteProvenance::DoubleQuoted)?;
            }
        }
    }
    if !closed {
        push_unread(scanner, sink, UnreadReason::UnsupportedQuoteOrEscape)?;
    }
    Ok(())
}

/// Scans an outside-quotes backslash escape (`\` at `scanner.peek()`),
/// preserving `ByteProvenance::Escaped`. When `class_len` is `Some`, also
/// tracks class byte count and `!`/`^` inside glob classes.
fn scan_unquoted_escape(
    scanner: &mut ExactScanner<'_, '_>,
    word: &mut WordBuilder,
    sink: &mut ReasonSink<'_>,
    class_len: Option<&mut usize>,
) -> Result<(), Exhausted> {
    scanner.next_byte()?;
    match scanner.peek() {
        Some(next) if is_escapable_outside_quotes(next) => {
            scanner.next_byte()?;
            if let Some(clen) = class_len {
                *clen += 1;
            }
            scanner.push_word_byte(word, next, ByteProvenance::Escaped)?;
        }
        Some(b'\n') => {
            scanner.next_byte()?;
            push_unread(scanner, sink, UnreadReason::UnsupportedQuoteOrEscape)?;
        }
        Some(next) if next >= 0x80 => {
            push_unread(scanner, sink, UnreadReason::UnsupportedQuoteOrEscape)?;
            if let Some(clen) = class_len {
                *clen += 1;
            }
            scanner.consume_utf8_char_into(word, ByteProvenance::Escaped)?;
        }
        Some(next) => {
            scanner.next_byte()?;
            push_unread(scanner, sink, UnreadReason::UnsupportedQuoteOrEscape)?;
            if let Some(clen) = class_len {
                if matches!(next, b'!' | b'^') {
                    push_unread(scanner, sink, UnreadReason::NegatedGlobClass)?;
                }
                *clen += 1;
            }
            scanner.push_word_byte(word, next, ByteProvenance::Escaped)?;
        }
        None => {
            push_unread(scanner, sink, UnreadReason::UnsupportedQuoteOrEscape)?;
        }
    }
    Ok(())
}

/// Reads one standalone reserved token (`{`, `}`, `!`, `[[`), charging the
/// reason's string before consuming the token so the caller can record it
/// into its reserved slot without a further charge.
fn read_standalone(
    scanner: &mut ExactScanner<'_, '_>,
    kw: &'static str,
) -> Result<ReadWordOutcome, Exhausted> {
    scanner.budget.charge_alloc(kw.len())?;
    scanner.advance_n(kw.len())?;
    let mut word = WordBuilder::new();
    for &b in kw.as_bytes() {
        scanner.push_word_byte(&mut word, b, ByteProvenance::Unquoted)?;
    }
    let mut rw = ReadWordOutcome::from_builder(word);
    rw.standalone_reserved = Some(kw);
    Ok(rw)
}

/// Reads one word starting at `scanner.pos` (which must not be at EOF or a
/// word-boundary metacharacter). Records any §3 `UnreadReason` encountered into
/// `sink`.
fn read_word(
    scanner: &mut ExactScanner<'_, '_>,
    sink: &mut ReasonSink<'_>,
    in_command_position: bool,
) -> Result<ReadWordOutcome, Exhausted> {
    scanner.charge_word_meta()?;
    let start_pos = scanner.pos;

    // Fast-recognize standalone unquoted `{`, `}`, `!`, `[[` when bounded by
    // word boundaries so they report `ReservedWordOrConstruct` cleanly.
    if in_command_position && let Some(b) = scanner.peek() {
        let next = scanner.peek_at(1);
        let at_single_boundary = next.is_none_or(is_word_boundary);
        if matches!(b, b'{' | b'}') && at_single_boundary {
            return read_standalone(scanner, if b == b'{' { "{" } else { "}" });
        }
        if b == b'!' && at_single_boundary {
            return read_standalone(scanner, "!");
        }
        if b == b'[' && next == Some(b'[') && scanner.peek_at(2).is_none_or(is_word_boundary) {
            return read_standalone(scanner, "[[");
        }
    }

    let mut word = WordBuilder::new();
    let mut started = false;
    let mut all_unquoted_digits = true;
    let mut digits_value = 0u32;
    let mut contained_quoting = false;
    let mut assign_prefix = AssignPrefix::Empty;
    let mut seen_first_unquoted_eq = false;
    let mut assignment_name_end: Option<usize> = None;

    while let Some(b) = scanner.peek() {
        if is_word_boundary(b) {
            break;
        }
        match b {
            b'\'' => {
                // Single quotes: any bytes until matching `'`.
                started = true;
                all_unquoted_digits = false;
                contained_quoting = true;
                assign_prefix = AssignPrefix::Invalid;
                scan_single_quoted(scanner, &mut word, sink, None)?;
            }
            b'"' => {
                // Double quotes: any bytes except `$`, backtick, `!`;
                // `\` allowed ONLY in `\"` and `\\`.
                started = true;
                all_unquoted_digits = false;
                contained_quoting = true;
                assign_prefix = AssignPrefix::Invalid;
                scan_double_quoted(scanner, &mut word, sink, None)?;
            }
            b'\\' => {
                // Outside quotes: backslash allowed ONLY before allowed
                // characters or glob characters (`*`, `?`, `[`, `]`).
                // Backslash-newline is outside the model.
                started = true;
                all_unquoted_digits = false;
                contained_quoting = true;
                assign_prefix = AssignPrefix::Invalid;
                scan_unquoted_escape(scanner, &mut word, sink, None)?;
            }
            b'~' => {
                let at_word_start = !started;
                started = true;
                all_unquoted_digits = false;
                assign_prefix = AssignPrefix::Invalid;
                scanner.next_byte()?;
                if at_word_start {
                    match scanner.peek() {
                        None | Some(b'/') => {}
                        Some(next) if is_word_boundary(next) => {}
                        Some(_) => {
                            push_unread(scanner, sink, UnreadReason::UnsupportedTildeForm)?;
                        }
                    }
                } else {
                    push_unread(
                        scanner,
                        sink,
                        UnreadReason::UnquotedByteOutsideModel { byte: b'~' },
                    )?;
                }
                scanner.push_word_byte(&mut word, b'~', ByteProvenance::Unquoted)?;
            }
            b'=' => {
                let at_word_start = !started;
                started = true;
                all_unquoted_digits = false;
                scanner.next_byte()?;
                if at_word_start {
                    // zsh `=cmd`
                    seen_first_unquoted_eq = true;
                    assign_prefix = AssignPrefix::Invalid;
                    push_unread(
                        scanner,
                        sink,
                        UnreadReason::UnquotedByteOutsideModel { byte: b'=' },
                    )?;
                } else if !seen_first_unquoted_eq {
                    seen_first_unquoted_eq = true;
                    if assign_prefix.is_valid_before_eq() {
                        let name_end = if assign_prefix == AssignPrefix::ValidNamePlus {
                            word.bytes.len() - 1
                        } else {
                            word.bytes.len()
                        };
                        assignment_name_end = Some(name_end);
                    }
                    assign_prefix = AssignPrefix::Invalid;
                }
                scanner.push_word_byte(&mut word, b'=', ByteProvenance::Unquoted)?;
            }
            b'*' | b'?' => {
                started = true;
                all_unquoted_digits = false;
                assign_prefix = AssignPrefix::Invalid;
                scanner.next_byte()?;
                scanner.push_word_byte(&mut word, b, ByteProvenance::Unquoted)?;
            }
            b'[' => {
                // Glob character class `[...]`: no `!` or `^` inside, must be
                // non-empty and closed, permits §3 quotes and escapes while
                // retaining class state and byte provenance.
                started = true;
                all_unquoted_digits = false;
                assign_prefix = AssignPrefix::Invalid;
                scanner.next_byte()?;
                scanner.push_word_byte(&mut word, b'[', ByteProvenance::Unquoted)?;
                let mut found_close = false;
                let mut class_len = 0usize;
                while let Some(cb) = scanner.peek() {
                    if is_word_boundary(cb) {
                        break;
                    }
                    match cb {
                        b']' => {
                            scanner.next_byte()?;
                            scanner.push_word_byte(&mut word, b']', ByteProvenance::Unquoted)?;
                            if class_len == 0 {
                                push_unread(
                                    scanner,
                                    sink,
                                    UnreadReason::UnquotedByteOutsideModel { byte: b'[' },
                                )?;
                            }
                            found_close = true;
                            break;
                        }
                        b'!' | b'^' => {
                            scanner.next_byte()?;
                            push_unread(scanner, sink, UnreadReason::NegatedGlobClass)?;
                            scanner.push_word_byte(&mut word, cb, ByteProvenance::Unquoted)?;
                            class_len += 1;
                        }
                        b'\'' => {
                            contained_quoting = true;
                            scan_single_quoted(scanner, &mut word, sink, Some(&mut class_len))?;
                        }
                        b'"' => {
                            contained_quoting = true;
                            scan_double_quoted(scanner, &mut word, sink, Some(&mut class_len))?;
                        }
                        b'\\' => {
                            contained_quoting = true;
                            scan_unquoted_escape(scanner, &mut word, sink, Some(&mut class_len))?;
                        }
                        c if (is_allowed_unquoted_base(c) && c != b'/')
                            || matches!(c, b'=' | b'*' | b'?') =>
                        {
                            scanner.next_byte()?;
                            scanner.push_word_byte(&mut word, c, ByteProvenance::Unquoted)?;
                            class_len += 1;
                        }
                        c if c >= 0x80 => {
                            let lead = scanner
                                .consume_utf8_char_into(&mut word, ByteProvenance::Unquoted)?;
                            push_unread(
                                scanner,
                                sink,
                                UnreadReason::UnquotedByteOutsideModel { byte: lead },
                            )?;
                            class_len += 1;
                        }
                        c => {
                            scanner.next_byte()?;
                            push_unread(
                                scanner,
                                sink,
                                UnreadReason::UnquotedByteOutsideModel { byte: c },
                            )?;
                            scanner.push_word_byte(&mut word, c, ByteProvenance::Unquoted)?;
                            class_len += 1;
                        }
                    }
                }
                if !found_close {
                    push_unread(
                        scanner,
                        sink,
                        UnreadReason::UnquotedByteOutsideModel { byte: b'[' },
                    )?;
                }
            }
            c if is_allowed_unquoted_base(c) => {
                started = true;
                if c.is_ascii_digit() {
                    digits_value = digits_value
                        .saturating_mul(10)
                        .saturating_add(u32::from(c - b'0'));
                } else {
                    all_unquoted_digits = false;
                }
                assign_prefix.advance_unquoted_byte(c);
                scanner.next_byte()?;
                scanner.push_word_byte(&mut word, c, ByteProvenance::Unquoted)?;
            }
            c if c >= 0x80 => {
                started = true;
                all_unquoted_digits = false;
                assign_prefix = AssignPrefix::Invalid;
                let lead = scanner.consume_utf8_char_into(&mut word, ByteProvenance::Unquoted)?;
                push_unread(
                    scanner,
                    sink,
                    UnreadReason::UnquotedByteOutsideModel { byte: lead },
                )?;
            }
            c => {
                // Any other unquoted ASCII byte (`$`, `` ` ``, `{`, `}`, `#`,
                // `!`, `^`, `]`, `\r`, control bytes, etc.).
                started = true;
                all_unquoted_digits = false;
                assign_prefix = AssignPrefix::Invalid;
                scanner.next_byte()?;
                push_unread(
                    scanner,
                    sink,
                    UnreadReason::UnquotedByteOutsideModel { byte: c },
                )?;
                scanner.push_word_byte(&mut word, c, ByteProvenance::Unquoted)?;
            }
        }
    }

    if scanner.pos == start_pos {
        all_unquoted_digits = false;
    }

    let is_digits = started && all_unquoted_digits && !word.is_empty();
    let mut rw = ReadWordOutcome::from_builder(word);
    rw.all_unquoted_digits = is_digits;
    rw.digits_value = digits_value;
    rw.contained_quoting = contained_quoting;
    rw.assignment_name_end = assignment_name_end;
    Ok(rw)
}

/// Compute units charged before reading a redirect target for the work of
/// recording what its classification stores (the redirect and up to two
/// reasons).
const REDIRECT_RECORD_UNITS: u64 = 3;

/// Longest reason string a redirect's classification can allocate after its
/// target is read (`2>` + ` (glob target)`, or a 10-digit fd + operator),
/// charged before the target is read.
const REDIRECT_REASON_ALLOC_MAX: usize = 16;

/// Parses a redirect starting at `<` or `>` (with optional preceding explicit
/// fd digits, or `&>` / `&>>`).
fn lex_exact_redirect(
    scanner: &mut ExactScanner<'_, '_>,
    explicit_fd: Option<(u32, usize)>,
    cur: &mut PendingUnit,
    sink: &mut ReasonSink<'_>,
    heredocs: &mut Vec<(Vec<u8>, bool)>,
) -> Result<(), Exhausted> {
    let c0 = scanner.peek().unwrap_or(0);
    let c1 = scanner.peek_at(1);
    let c2 = scanner.peek_at(2);

    // Combined redirect `&>` / `&>>` (not shared by POSIX sh).
    if c0 == b'&' && c1 == Some(b'>') {
        let len = if c2 == Some(b'>') { 3 } else { 2 };
        scanner.advance_n(len)?;
        push_unread(scanner, sink, UnreadReason::CombinedRedirect)?;
        scanner.skip_horiz_ws()?;
        if scanner.peek().is_some_and(|b| !is_word_boundary(b)) {
            let _ = read_word(scanner, sink, false)?;
        } else {
            let op = if len == 3 { "&>>" } else { "&>" };
            push_unread_redirect_op(scanner, sink, op)?;
        }
        return Ok(());
    }

    // Here-string `<<<`.
    if c0 == b'<' && c1 == Some(b'<') && c2 == Some(b'<') {
        scanner.advance_n(3)?;
        push_unread_construct(scanner, sink, "<<<")?;
        if let Some((fd, _)) = explicit_fd
            && fd > 2
        {
            push_unread(scanner, sink, UnreadReason::DescriptorOutsideStd { fd })?;
        }
        scanner.skip_horiz_ws()?;
        if scanner.peek().is_some_and(|b| !is_word_boundary(b)) {
            let _ = read_word(scanner, sink, false)?;
        }
        return Ok(());
    }

    // Here-document `<<` / `<<-`.
    if c0 == b'<' && c1 == Some(b'<') {
        let strip_tabs = c2 == Some(b'-');
        scanner.advance_n(if strip_tabs { 3 } else { 2 })?;
        push_unread_construct(scanner, sink, if strip_tabs { "<<-" } else { "<<" })?;
        if let Some((fd, _)) = explicit_fd
            && fd > 2
        {
            push_unread(scanner, sink, UnreadReason::DescriptorOutsideStd { fd })?;
        }
        scanner.skip_horiz_ws()?;
        // Reserve the delimiter's slot before scanning it (§8).
        charge_vec_growth(heredocs, 1, scanner.budget)?;
        let mut delim = Vec::new();
        while let Some(b) = scanner.peek() {
            if is_word_boundary(b) {
                break;
            }
            match b {
                b'\'' => {
                    scanner.next_byte()?;
                    while let Some(c) = scanner.next_byte()? {
                        if c == b'\'' {
                            break;
                        }
                        scanner.budget.charge(1)?;
                        vec_push_budgeted(&mut delim, c, scanner.budget)?;
                    }
                }
                b'"' => {
                    scanner.next_byte()?;
                    while let Some(c) = scanner.next_byte()? {
                        if c == b'"' {
                            break;
                        }
                        if c == b'\\'
                            && let Some(esc) = scanner.next_byte()?
                        {
                            scanner.budget.charge(1)?;
                            vec_push_budgeted(&mut delim, esc, scanner.budget)?;
                        } else {
                            scanner.budget.charge(1)?;
                            vec_push_budgeted(&mut delim, c, scanner.budget)?;
                        }
                    }
                }
                b'\\' => {
                    scanner.next_byte()?;
                    if let Some(esc) = scanner.next_byte()? {
                        scanner.budget.charge(1)?;
                        vec_push_budgeted(&mut delim, esc, scanner.budget)?;
                    }
                }
                _ => {
                    scanner.next_byte()?;
                    scanner.budget.charge(1)?;
                    vec_push_budgeted(&mut delim, b, scanner.budget)?;
                }
            }
        }
        if !delim.is_empty() {
            push_reserved(heredocs, (delim, strip_tabs));
        }
        return Ok(());
    }

    // Process substitution `<(` / `>(`.
    if matches!(c0, b'<' | b'>') && c1 == Some(b'(') {
        scanner.advance_n(2)?;
        push_unread_construct(scanner, sink, if c0 == b'<' { "<(" } else { ">(" })?;
        return Ok(());
    }

    // Standard / unsupported redirect operators.
    #[derive(Clone, Copy)]
    enum Op {
        Read,
        Write,
        Append,
        DupOut,
        DupIn,
        ReadWrite,
        Clobber,
    }
    let (op, op_len, op_str) = match (c0, c1) {
        (b'>', Some(b'>')) => (Op::Append, 2, ">>"),
        (b'>', Some(b'&')) => (Op::DupOut, 2, ">&"),
        (b'>', Some(b'|')) => (Op::Clobber, 2, ">|"),
        (b'>', _) => (Op::Write, 1, ">"),
        (b'<', Some(b'&')) => (Op::DupIn, 2, "<&"),
        (b'<', Some(b'>')) => (Op::ReadWrite, 2, "<>"),
        _ => (Op::Read, 1, "<"),
    };
    scanner.advance_n(op_len)?;
    scanner.skip_horiz_ws()?;

    // Reserve, before reading the target, everything its classification may
    // store: the pending unit's output slot, the redirect's slot, two reason
    // slots, their work, and the longest reason string (§8). After the read,
    // the redirect and its reasons are recorded without a further charge, so
    // a finished target is never lost (review round 3, finding 1).
    sink.out.reserve_unit(scanner.budget)?;
    charge_vec_growth(&mut cur.redirects, 1, scanner.budget)?;
    scanner.budget.charge(REDIRECT_RECORD_UNITS)?;
    sink.reserve(2, scanner.budget)?;
    scanner.budget.charge_alloc(REDIRECT_REASON_ALLOC_MAX)?;

    let target_opt = if scanner.peek().is_some_and(|b| !is_word_boundary(b)) {
        Some(read_word(scanner, sink, false)?)
    } else {
        None
    };
    let target_fd_over_std = |t: &ReadWordOutcome| -> Option<u32> {
        (t.all_unquoted_digits && t.digits_value > 2).then_some(t.digits_value)
    };

    if let Some((fd, _)) = explicit_fd
        && fd > 2
    {
        sink.push_reserved(UnreadReason::DescriptorOutsideStd { fd });
        // Also check if DupOut/DupIn has a target descriptor > 2.
        if matches!(op, Op::DupOut | Op::DupIn)
            && let Some(m) = target_opt.as_ref().and_then(target_fd_over_std)
        {
            sink.push_reserved(UnreadReason::DescriptorOutsideStd { fd: m });
        }
        return Ok(());
    }

    let op_reason = |op: &str| UnreadReason::UnsupportedRedirect { op: op.to_string() };
    match op {
        Op::DupIn => {
            sink.push_reserved(op_reason("<&"));
            if let Some(m) = target_opt.as_ref().and_then(target_fd_over_std) {
                sink.push_reserved(UnreadReason::DescriptorOutsideStd { fd: m });
            }
        }
        Op::ReadWrite | Op::Clobber => {
            sink.push_reserved(op_reason(op_str));
        }
        Op::DupOut => {
            let Some(target) = target_opt else {
                sink.push_reserved(op_reason(">&"));
                return Ok(());
            };
            let Some((fd, digit_len)) = explicit_fd else {
                // `>&2` / `>&-` / `>&file`: explicit `n` in 0..=2 required by §3's closed list.
                sink.push_reserved(op_reason(">&"));
                if let Some(m) = target_fd_over_std(&target) {
                    sink.push_reserved(UnreadReason::DescriptorOutsideStd { fd: m });
                }
                return Ok(());
            };
            if digit_len != 1 {
                sink.push_reserved(op_reason(">&"));
                return Ok(());
            }
            let Some(from) = u8::try_from(fd).ok().and_then(StdFd::from_u8) else {
                sink.push_reserved(UnreadReason::DescriptorOutsideStd { fd });
                return Ok(());
            };
            let tb = target.word.literal_bytes();
            if tb == b"-" && !target.contained_quoting && target.all_unquoted {
                push_reserved(&mut cur.redirects, ModelRedirect::Close { fd: from });
            } else if target.all_unquoted_digits && tb.len() == 1 && matches!(tb[0], b'0'..=b'2') {
                let to = StdFd::from_u8(tb[0] - b'0').expect("0..=2");
                push_reserved(&mut cur.redirects, ModelRedirect::Dup { from, to });
            } else if let Some(m) = target_fd_over_std(&target) {
                sink.push_reserved(UnreadReason::DescriptorOutsideStd { fd: m });
            } else {
                sink.push_reserved(op_reason(">&"));
            }
        }
        Op::Read | Op::Write | Op::Append => {
            let Some(target) = target_opt else {
                sink.push_reserved(op_reason(op_str));
                return Ok(());
            };
            let glob_target = target.has_unquoted_glob;
            let model_redir = match (explicit_fd, op) {
                (None, Op::Read) => Some(ModelRedirect::Read {
                    target: target.word,
                }),
                (None, Op::Write) => Some(ModelRedirect::Write {
                    target: target.word,
                }),
                (None, Op::Append) => Some(ModelRedirect::Append {
                    target: target.word,
                }),
                (Some((2, 1)), Op::Write) => Some(ModelRedirect::ErrWrite {
                    target: target.word,
                }),
                (Some((2, 1)), Op::Append) => Some(ModelRedirect::ErrAppend {
                    target: target.word,
                }),
                _ => None,
            };
            match model_redir {
                Some(r) => {
                    if glob_target {
                        // zsh expands globs in redirect targets (MULTIOS); not a literal target.
                        sink.push_reserved(UnreadReason::UnsupportedRedirect {
                            op: format!("{op_str} (glob target)"),
                        });
                    }
                    push_reserved(&mut cur.redirects, r);
                }
                None => match explicit_fd {
                    Some((fd, _)) => sink.push_reserved(UnreadReason::UnsupportedRedirect {
                        op: format!("{fd}{op_str}"),
                    }),
                    None => sink.push_reserved(op_reason(op_str)),
                },
            }
        }
    }

    Ok(())
}

/// Skips pending here-doc bodies after a newline, charging 1 compute unit per
/// byte scanned (and the delimiter comparison) so body text is never
/// mis-parsed as modeled commands.
fn skip_heredocs(
    scanner: &mut ExactScanner<'_, '_>,
    heredocs: &mut Vec<(Vec<u8>, bool)>,
) -> Result<(), Exhausted> {
    for (delim, strip_tabs) in std::mem::take(heredocs) {
        loop {
            if scanner.is_eof() {
                break;
            }
            let line_start = scanner.pos;
            while let Some(b) = scanner.next_byte()? {
                if b == b'\n' {
                    break;
                }
            }
            let mut line_end = scanner.pos;
            if line_end > line_start && scanner.bytes[line_end - 1] == b'\n' {
                line_end -= 1;
            }
            let mut slice = &scanner.bytes[line_start..line_end];
            if strip_tabs {
                while slice.first() == Some(&b'\t') {
                    scanner.budget.charge(1)?;
                    slice = &slice[1..];
                }
            }
            if bytes_eq(slice, &delim, scanner.budget)? {
                break;
            }
        }
    }
    Ok(())
}

/// Lexes one command text into [`RawCommandUnit`]s with per-byte provenance
/// under the intersection of POSIX `sh`, `bash`, and `zsh`, recording every
/// §3 construct outside the model as an `UnreadReason`.
///
/// Returns `Err(PartOutcome::Exhausted)` only when the budget is already
/// spent on entry, before anything was lexed. Exhaustion later records
/// `Exhausted` among the outcomes and keeps every unit and reason found.
///
/// **Owned by:** P1 · **Contract:** §3, §8.
pub(crate) fn lex_exact(
    line: &str,
    depth: u8,
    parent_unit: Option<UnitIndex>,
    budget: &mut ComputeBudget,
) -> Result<LexExactOutput, PartOutcome> {
    let mut sink = OutputSink {
        out: LexExactOutput::default(),
        whole: parent_unit.map_or(PartId::WholeCall, PartId::Unit),
    };
    match lex_exact_core(line, depth, parent_unit, &mut sink, budget) {
        Ok(()) => Ok(sink.out),
        Err(CoreStop::TooLarge(cap)) => Err(PartOutcome::TooLarge(cap)),
        Err(CoreStop::Entry(e)) => Err(PartOutcome::Exhausted(e.reason)),
    }
}

/// `Err(Ok(_))` stops with `TooLarge`; `Err(Err(_))` stops on exhaustion.
type Stop = Result<PartOutcome, Exhausted>;

/// Reserves, before a word is scanned, everything the finished word needs
/// (review round 3, finding 1): the pending unit's output slot, the word's
/// argv slot and, in command position, one reason slot plus its unit of work
/// for the assignment or reserved-word reason the word may carry.
fn reserve_word_storage(
    scanner: &mut ExactScanner<'_, '_>,
    cur: &mut PendingUnit,
    sink: &mut ReasonSink<'_>,
    in_command_position: bool,
) -> Result<(), Exhausted> {
    sink.out.reserve_unit(scanner.budget)?;
    charge_vec_growth(&mut cur.argv, 1, scanner.budget)?;
    if in_command_position {
        scanner.budget.charge(1)?;
        sink.reserve(1, scanner.budget)?;
    }
    Ok(())
}

/// Flushes the pending unit into the sink. Every charge happens before `cur`
/// is moved, so exhaustion leaves the pending unit intact for recovery.
fn flush_unit(
    cur: &mut PendingUnit,
    next_sep: Separator,
    next_pipe: Option<PipelineId>,
    sink: &mut ReasonSink<'_>,
    scanner: &mut ExactScanner<'_, '_>,
    depth: u8,
    parent_unit: Option<UnitIndex>,
) -> Result<(), Stop> {
    const REDIRECT_ONLY: &str = "redirect-only command";
    if cur.is_empty() {
        // The pending reasons belong to the text; their slots are reserved.
        for r in sink.pending.drain(..) {
            sink.out.push_text_outcome(PartOutcome::Unread(r));
        }
        *cur = PendingUnit::new(next_sep, next_pipe);
        return Ok(());
    }
    if sink.out.at_unit_cap() {
        return Err(Ok(PartOutcome::TooLarge(Cap::Units)));
    }
    let redirect_only = cur.argv.is_empty() && !cur.redirects.is_empty();
    scanner.charge_unit_meta().map_err(Err)?;
    sink.out.reserve_unit(scanner.budget).map_err(Err)?;
    if redirect_only {
        scanner
            .charge_unread_meta(REDIRECT_ONLY.len())
            .map_err(Err)?;
        sink.reserve(1, scanner.budget).map_err(Err)?;
    }
    let finished = std::mem::replace(cur, PendingUnit::new(next_sep, next_pipe));
    let idx = sink.out.local_units();
    for r in sink.pending.drain(..) {
        sink.out.push_unit_outcome(idx, PartOutcome::Unread(r));
    }
    if redirect_only {
        // Redirect-only command (`> foo`, `< foo`) diverges in zsh
        // (NULLCMD / READNULLCMD). Keep the unit so its redirects are still
        // checked as literal paths, while marking it Unread.
        sink.out.push_unit_outcome(
            idx,
            PartOutcome::Unread(UnreadReason::ReservedWordOrConstruct {
                construct: REDIRECT_ONLY.to_string(),
            }),
        );
    }
    sink.out.push_unit(
        RawCommandUnit {
            index: UnitIndex(idx),
            argv: finished.argv,
            redirects: finished.redirects,
            separator_before: finished.separator_before,
            pipeline_id: finished.pipeline_id,
            depth,
            parent_unit,
        },
        RawUnitLexMeta {
            leading_assignment_count: finished.leading_assignment_count,
            sets_home: finished.leading_sets_home,
        },
    );
    Ok(())
}

/// The exact lexer, writing into any [`ExactSink`] (contract §3, §8).
#[allow(clippy::too_many_lines)]
pub(super) fn lex_exact_core(
    line: &str,
    depth: u8,
    parent_unit: Option<UnitIndex>,
    out: &mut dyn ExactSink,
    budget: &mut ComputeBudget,
) -> Result<(), CoreStop> {
    if line.len() > limits::MAX_BYTES {
        return Err(CoreStop::TooLarge(Cap::Bytes));
    }

    // Charge entry work and reserve the slot that records a later
    // `Exhausted`. If even that fails, nothing has been lexed yet.
    budget
        .charge(1)
        .and_then(|()| out.reserve_outcomes(1, budget))
        .map_err(CoreStop::Entry)?;

    let mut scanner = ExactScanner::new(line, budget);
    let mut sink = ReasonSink {
        pending: Vec::new(),
        out,
    };
    let mut cur = PendingUnit::new(Separator::Start, None);
    let mut next_pipe_id = 0u32;
    let mut expecting_after_op: Option<&'static str> = None;
    let mut heredocs: Vec<(Vec<u8>, bool)> = Vec::new();

    let run = (|| -> Result<(), Stop> {
        while let Some(b) = scanner.peek() {
            match b {
                b' ' | b'\t' => {
                    scanner.next_byte().map_err(Err)?;
                }
                b'#' => {
                    // Comment at token boundary: outside the model (§3).
                    push_unread_construct(&mut scanner, &mut sink, "#").map_err(Err)?;
                    while let Some(ch) = scanner.peek() {
                        if ch == b'\n' {
                            break;
                        }
                        scanner.next_byte().map_err(Err)?;
                    }
                }
                b'\n' => {
                    scanner.next_byte().map_err(Err)?;
                    if !heredocs.is_empty() {
                        skip_heredocs(&mut scanner, &mut heredocs).map_err(Err)?;
                    }
                    if expecting_after_op.is_some() && cur.is_empty() {
                        // Line continuation after `&&`, `||`, or `|`: keep
                        // waiting for the right-hand unit.
                        continue;
                    }
                    expecting_after_op = None;
                    flush_unit(
                        &mut cur,
                        Separator::Newline,
                        None,
                        &mut sink,
                        &mut scanner,
                        depth,
                        parent_unit,
                    )?;
                }
                b';' => {
                    if scanner.peek_at(1) == Some(b';') {
                        scanner.advance_n(2).map_err(Err)?;
                        push_unread_construct(&mut scanner, &mut sink, ";;").map_err(Err)?;
                    } else {
                        scanner.next_byte().map_err(Err)?;
                        if cur.is_empty() {
                            push_unread_construct(&mut scanner, &mut sink, ";").map_err(Err)?;
                        }
                    }
                    if let Some(op) = expecting_after_op.take()
                        && cur.is_empty()
                    {
                        push_unread_construct(&mut scanner, &mut sink, op).map_err(Err)?;
                    }
                    flush_unit(
                        &mut cur,
                        Separator::Semi,
                        None,
                        &mut sink,
                        &mut scanner,
                        depth,
                        parent_unit,
                    )?;
                }
                b'&' => match scanner.peek_at(1) {
                    Some(b'&') => {
                        scanner.advance_n(2).map_err(Err)?;
                        if cur.is_empty() {
                            push_unread_construct(&mut scanner, &mut sink, "&&").map_err(Err)?;
                        }
                        expecting_after_op = Some("&&");
                        flush_unit(
                            &mut cur,
                            Separator::And,
                            None,
                            &mut sink,
                            &mut scanner,
                            depth,
                            parent_unit,
                        )?;
                    }
                    Some(b'>') => {
                        expecting_after_op = None;
                        lex_exact_redirect(&mut scanner, None, &mut cur, &mut sink, &mut heredocs)
                            .map_err(Err)?;
                    }
                    _ => {
                        scanner.next_byte().map_err(Err)?;
                        push_unread_construct(&mut scanner, &mut sink, "&").map_err(Err)?;
                        expecting_after_op = None;
                        flush_unit(
                            &mut cur,
                            Separator::Semi,
                            None,
                            &mut sink,
                            &mut scanner,
                            depth,
                            parent_unit,
                        )?;
                    }
                },
                b'|' => match scanner.peek_at(1) {
                    Some(b'|') => {
                        scanner.advance_n(2).map_err(Err)?;
                        if cur.is_empty() {
                            push_unread_construct(&mut scanner, &mut sink, "||").map_err(Err)?;
                        }
                        expecting_after_op = Some("||");
                        flush_unit(
                            &mut cur,
                            Separator::Or,
                            None,
                            &mut sink,
                            &mut scanner,
                            depth,
                            parent_unit,
                        )?;
                    }
                    Some(b'&') => {
                        scanner.advance_n(2).map_err(Err)?;
                        push_unread_construct(&mut scanner, &mut sink, "|&").map_err(Err)?;
                        let pipe = if cur.is_empty() {
                            None
                        } else {
                            let id = cur.pipeline_id.unwrap_or_else(|| {
                                next_pipe_id += 1;
                                PipelineId(next_pipe_id)
                            });
                            cur.pipeline_id = Some(id);
                            Some(id)
                        };
                        expecting_after_op = Some("|&");
                        flush_unit(
                            &mut cur,
                            Separator::Pipe,
                            pipe,
                            &mut sink,
                            &mut scanner,
                            depth,
                            parent_unit,
                        )?;
                    }
                    _ => {
                        scanner.next_byte().map_err(Err)?;
                        let pipe = if cur.is_empty() {
                            push_unread_construct(&mut scanner, &mut sink, "|").map_err(Err)?;
                            None
                        } else {
                            let id = cur.pipeline_id.unwrap_or_else(|| {
                                next_pipe_id += 1;
                                PipelineId(next_pipe_id)
                            });
                            cur.pipeline_id = Some(id);
                            Some(id)
                        };
                        expecting_after_op = Some("|");
                        flush_unit(
                            &mut cur,
                            Separator::Pipe,
                            pipe,
                            &mut sink,
                            &mut scanner,
                            depth,
                            parent_unit,
                        )?;
                    }
                },
                b'(' | b')' => {
                    let construct = if b == b'(' && scanner.peek_at(1) == Some(b'(') {
                        scanner.advance_n(2).map_err(Err)?;
                        "(("
                    } else if b == b')' && scanner.peek_at(1) == Some(b')') {
                        scanner.advance_n(2).map_err(Err)?;
                        "))"
                    } else if b == b'(' && scanner.peek_at(1) == Some(b')') {
                        scanner.advance_n(2).map_err(Err)?;
                        "()"
                    } else {
                        scanner.next_byte().map_err(Err)?;
                        if b == b'(' { "(" } else { ")" }
                    };
                    push_unread_construct(&mut scanner, &mut sink, construct).map_err(Err)?;
                }
                b'<' | b'>' => {
                    expecting_after_op = None;
                    lex_exact_redirect(&mut scanner, None, &mut cur, &mut sink, &mut heredocs)
                        .map_err(Err)?;
                }
                _ => {
                    expecting_after_op = None;
                    let in_cmd_pos = !cur.seen_command_word;
                    reserve_word_storage(&mut scanner, &mut cur, &mut sink, in_cmd_pos)
                        .map_err(Err)?;
                    let rw = read_word(&mut scanner, &mut sink, in_cmd_pos).map_err(Err)?;

                    // Unquoted digits directly followed by `<` or `>` (not
                    // process substitution `<(` / `>(`) form an explicit fd.
                    if rw.all_unquoted_digits
                        && matches!(scanner.peek(), Some(b'<' | b'>'))
                        && scanner.peek_at(1) != Some(b'(')
                    {
                        let digit_len = rw.word.len();
                        lex_exact_redirect(
                            &mut scanner,
                            Some((rw.digits_value, digit_len)),
                            &mut cur,
                            &mut sink,
                            &mut heredocs,
                        )
                        .map_err(Err)?;
                        continue;
                    }

                    // From here on the finished word goes into its reserved
                    // argv slot before any further charge, so it is never lost.
                    if let Some(kw) = rw.standalone_reserved {
                        cur.seen_command_word = true;
                        push_reserved(&mut cur.argv, rw.word);
                        sink.push_reserved(UnreadReason::ReservedWordOrConstruct {
                            construct: kw.to_string(),
                        });
                        continue;
                    }

                    if !cur.seen_command_word {
                        if let Some(name_end) = rw.assignment_name_end {
                            cur.leading_assignment_count += 1;
                            if &rw.word.literal_bytes()[..name_end] == b"HOME" {
                                cur.leading_sets_home = true;
                            }
                            push_reserved(&mut cur.argv, rw.word);
                            sink.push_reserved(UnreadReason::Assignment);
                            continue;
                        }
                        cur.seen_command_word = true;
                        let candidate = !rw.contained_quoting
                            && rw.all_unquoted
                            && rw.word.len() <= OUTSIDE_NAME_MAX;
                        push_reserved(&mut cur.argv, rw.word);
                        if candidate {
                            // Charge the reason's string before the keyword
                            // check, so a found keyword is always recorded.
                            let w = cur.argv.last().expect("word was just stored");
                            scanner.budget.charge_alloc(w.len()).map_err(Err)?;
                            if let Some(s) = word_as_str(w, scanner.budget).map_err(Err)?
                                && is_reserved_word(s, scanner.budget).map_err(Err)?
                            {
                                sink.push_reserved(UnreadReason::ReservedWordOrConstruct {
                                    construct: s.to_string(),
                                });
                            }
                        }
                        continue;
                    }
                    push_reserved(&mut cur.argv, rw.word);
                }
            }
        }

        if let Some(op) = expecting_after_op.take()
            && cur.is_empty()
        {
            push_unread_construct(&mut scanner, &mut sink, op).map_err(Err)?;
        }
        flush_unit(
            &mut cur,
            Separator::Start,
            None,
            &mut sink,
            &mut scanner,
            depth,
            parent_unit,
        )?;
        Ok(())
    })();

    match run {
        Ok(()) => Ok(()),
        Err(Ok(PartOutcome::TooLarge(cap))) => Err(CoreStop::TooLarge(cap)),
        Err(Ok(other)) => unreachable!("lexer stops only with TooLarge: {other:?}"),
        Err(Err(exhausted)) => {
            // Preserve every unit and reason found before exhaustion (§8).
            // Every slot used here was reserved before its value was found,
            // so recovery allocates nothing. Whether a redirect-only pending
            // unit would have stayed redirect-only is unknown here, so that
            // reason is not guessed.
            if !cur.is_empty() && !sink.out.at_unit_cap() {
                let idx = sink.out.local_units();
                for r in sink.pending.drain(..) {
                    sink.out.push_unit_outcome(idx, PartOutcome::Unread(r));
                }
                sink.out.push_unit(
                    RawCommandUnit {
                        index: UnitIndex(idx),
                        argv: cur.argv,
                        redirects: cur.redirects,
                        separator_before: cur.separator_before,
                        pipeline_id: cur.pipeline_id,
                        depth,
                        parent_unit,
                    },
                    RawUnitLexMeta {
                        leading_assignment_count: cur.leading_assignment_count,
                        sets_home: cur.leading_sets_home,
                    },
                );
            } else {
                for r in sink.pending.drain(..) {
                    sink.out.push_text_outcome(PartOutcome::Unread(r));
                }
            }
            sink.out
                .push_text_outcome(PartOutcome::Exhausted(exhausted.reason));
            Ok(())
        }
    }
}

/// Runs the tolerant best-effort lexer over `raw_texts`, producing
/// [`BestEffortRecord`] (including units derived from `-c` scripts and
/// substitutions) under the shared compute budget.
///
/// **Owned by:** P1 · **Contract:** §2, §8.
pub(crate) fn lex_best_effort(
    raw_texts: &[&str],
    budget: &mut ComputeBudget,
) -> Result<BestEffortRecord, PartOutcome> {
    let mut record = BestEffortRecord::default();
    lex_best_effort_into(raw_texts, &mut record, budget)?;
    Ok(record)
}

/// Folds a command word according to CR-19 / `FoldRules` (path basename +
/// listed case folds, no Unicode normalization).
///
/// **Owned by:** P1 · **Contract:** §4, §8.
pub(crate) fn fold_command_word(
    word: &Word,
    folds: &FoldRules,
    budget: &mut ComputeBudget,
) -> Result<String, Exhausted> {
    fold_command_bytes(word.literal_bytes(), folds, budget)
}

/// Shared CR-19 basename + listed case-fold implementation (used by both
/// `fold_command_word` and the best-effort pass). Every pass over the bytes is
/// charged before it runs (§8).
pub(super) fn fold_command_bytes(
    raw: &[u8],
    folds: &FoldRules,
    budget: &mut ComputeBudget,
) -> Result<String, Exhausted> {
    // Pass 1: find the basename (scan from the end for `/`).
    budget.charge(raw.len().max(1) as u64)?;
    let base = match raw.iter().rposition(|&b| b == b'/') {
        Some(pos) => &raw[pos + 1..],
        None => raw,
    };

    // Pass 2: ASCII check.
    budget.charge(base.len().max(1) as u64)?;
    if base.is_ascii() {
        // Pass 3: lowercase copy.
        budget.charge(base.len() as u64)?;
        budget.charge_alloc(base.len())?;
        let mut out = String::with_capacity(base.len());
        for &b in base {
            out.push(b.to_ascii_lowercase() as char);
        }
        return Ok(out);
    }

    // Unicode path: string-level `str::to_lowercase()` (preserving contextual
    // rules such as Greek final sigma `ΟΣ` -> `ος`) plus optional `FoldRules`
    // (`ſ` U+017F -> `s` and `K` U+212A -> `k`), with NO Unicode normalization
    // (NFC/NFD).
    //
    // Pass 3: sizing. Decoding each char and walking its lowercase mapping (at
    // most 3 chars) costs at most 4 units per byte.
    budget.charge((base.len() as u64).saturating_mul(4))?;
    let mut lower_total = 0usize;
    let mut out_total = 0usize;
    let mut needs_rewrite = false;
    let mut has_invalid = false;

    for chunk in base.utf8_chunks() {
        for c in chunk.valid().chars() {
            let lc_len: usize = c.to_lowercase().map(char::len_utf8).sum();
            lower_total = lower_total.saturating_add(lc_len);
            if c == '\u{017f}' {
                if folds.fold_long_s {
                    needs_rewrite = true;
                    out_total = out_total.saturating_add(1);
                } else {
                    out_total = out_total.saturating_add(c.len_utf8());
                }
            } else if c == '\u{212a}' {
                if folds.fold_kelvin {
                    // `str::to_lowercase()` already maps U+212A -> `k` (1 byte).
                    out_total = out_total.saturating_add(1);
                } else {
                    needs_rewrite = true;
                    out_total = out_total.saturating_add(c.len_utf8());
                }
            } else {
                out_total = out_total.saturating_add(lc_len);
            }
        }
        if !chunk.invalid().is_empty() {
            has_invalid = true;
            out_total = out_total.saturating_add(
                chunk
                    .invalid()
                    .len()
                    .saturating_mul(char::REPLACEMENT_CHARACTER.len_utf8()),
            );
        }
    }

    // `str::to_lowercase()` starts at `valid.len()` and may double capacity up
    // to `2 * lower_len` if characters expand (e.g. U+0130). Charge worst-case
    // temporary allocation before calling it (§8).
    let lower_alloc_est = lower_total.max(base.len()).saturating_mul(2);
    if !has_invalid && !needs_rewrite {
        // Pass 4: `to_lowercase` decodes the input and writes the output.
        budget.charge(
            (base.len().saturating_add(lower_total))
                .saturating_mul(4)
                .max(1) as u64,
        )?;
        budget.charge_alloc(lower_alloc_est)?;
        let valid = std::str::from_utf8(base).expect("no invalid UTF-8 chunks");
        return Ok(valid.to_lowercase());
    }

    // Pass 4: rewrite. Per chunk `to_lowercase` (decode + write), per char a
    // lowercase-length walk (at most 4 units per byte), and the output pushes.
    budget.charge(
        (base.len().saturating_add(lower_total))
            .saturating_mul(4)
            .saturating_add(base.len().saturating_mul(4))
            .saturating_add(out_total)
            .max(1) as u64,
    )?;
    budget.charge_alloc(lower_alloc_est.saturating_add(out_total))?;

    let mut out = String::with_capacity(out_total);
    for chunk in base.utf8_chunks() {
        let valid = chunk.valid();
        if !valid.is_empty() {
            let lowered = valid.to_lowercase();
            let mut lower_it = lowered.chars();
            for orig in valid.chars() {
                let count = orig.to_lowercase().count();
                if orig == '\u{017f}' && folds.fold_long_s {
                    for _ in 0..count {
                        let _ = lower_it.next();
                    }
                    out.push('s');
                } else if orig == '\u{212a}' && !folds.fold_kelvin {
                    for _ in 0..count {
                        let _ = lower_it.next();
                    }
                    out.push('\u{212a}');
                } else {
                    for _ in 0..count {
                        if let Some(lc) = lower_it.next() {
                            out.push(lc);
                        }
                    }
                }
            }
        }
        for _ in chunk.invalid() {
            out.push(char::REPLACEMENT_CHARACTER);
        }
    }
    Ok(out)
}
