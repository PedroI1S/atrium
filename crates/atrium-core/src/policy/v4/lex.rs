//! Exact and best-effort shell lexers (owned by P1; contract §2, §3, §4, §8).

use crate::policy::limits;
use crate::policy::model::{
    BestEffortRecord, ByteProvenance, Cap, ComputeBudget, Exhausted, FoldRules, LexExactOutput,
    ModelRedirect, PartId, PartOutcome, PartResult, PipelineId, RawCommandUnit, Separator, StdFd,
    UnitIndex, UnreadReason, Word,
};

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
/// (`[A-Za-z_][A-Za-z0-9_]*`).
pub(super) fn is_posix_name(name: &[u8]) -> bool {
    match name.first() {
        Some(&b) if b.is_ascii_alphabetic() || b == b'_' => name[1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'_'),
        _ => false,
    }
}

/// Returns `Some(name_bytes)` if `word` is a leading shell assignment
/// (`NAME=value` or bash/zsh `NAME+=value`) whose variable name and `=`/`+=`
/// operator are unquoted (contract §3).
pub(super) fn leading_assignment_name(word: &Word) -> Option<&[u8]> {
    let bytes = word.literal_bytes();
    let prov = word.provenance();
    let eq = bytes
        .iter()
        .zip(prov)
        .position(|(&b, &p)| b == b'=' && p == ByteProvenance::Unquoted)?;
    if eq == 0 {
        return None;
    }
    // Check `NAME+=value` (bash/zsh append assignment; differs from POSIX sh).
    let name_end = if eq >= 2 && bytes[eq - 1] == b'+' && prov[eq - 1] == ByteProvenance::Unquoted {
        eq - 1
    } else {
        eq
    };
    if prov[..name_end]
        .iter()
        .all(|&p| p == ByteProvenance::Unquoted)
        && is_posix_name(&bytes[..name_end])
    {
        Some(&bytes[..name_end])
    } else {
        None
    }
}

/// Returns `true` iff `s` is a shell reserved word outside §3's simple-command
/// model (reusing `policy::shell::RESERVED`).
#[inline]
fn is_reserved_word(s: &str) -> bool {
    crate::policy::tools::lookup_outside_model(s).is_some_and(|e| e.contract_section == "§3")
}

// ---------------------------------------------------------------------------
// Shared budgeted collection growth helpers (§8)
// ---------------------------------------------------------------------------

/// Charges capacity growth (both allocation bytes for the new capacity delta
/// and compute units for copying existing elements on reallocation) before
/// reserving space in `v` using geometric growth.
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

/// Pushes `val` into `v`, charging any capacity growth before allocating.
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
// Budgeted exact scanner (§3, §8)
// ---------------------------------------------------------------------------

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

    /// Charges copy + allocation (1 byte + 1 provenance byte) before pushing.
    #[inline]
    fn push_word_byte(
        &mut self,
        word: &mut Word,
        byte: u8,
        prov: ByteProvenance,
    ) -> Result<(), Exhausted> {
        self.budget.charge(1)?;
        self.budget.charge_alloc(2)?;
        word.push(byte, prov);
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
    fn charge_redirect_meta(&mut self) -> Result<(), Exhausted> {
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
    /// Charges 1 compute unit per byte scanned + copy/alloc per byte.
    fn consume_utf8_char_into(
        &mut self,
        word: &mut Word,
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
        self.budget.charge_alloc(len * 2)?;
        for &b in &self.bytes[self.pos..end] {
            word.push(b, prov);
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
    unreads: Vec<UnreadReason>,
    seen_command_word: bool,
}

impl PendingUnit {
    fn new(separator_before: Separator, pipeline_id: Option<PipelineId>) -> Self {
        Self {
            argv: Vec::new(),
            redirects: Vec::new(),
            separator_before,
            pipeline_id,
            unreads: Vec::new(),
            seen_command_word: false,
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
    /// True iff any quote (`'`, `"`) or escape (`\`) occurred anywhere in this
    /// token, even if the quoted span emitted zero bytes (e.g. `''if`, `if""`).
    contained_quoting: bool,
    /// True iff the token is a leading shell assignment (`NAME=value` or
    /// `NAME+=value`) where the entire prefix through the first unquoted `=`
    /// contained zero quoting or escaping (so `''FOO=1` and `FOO''=1` are not
    /// assignments, while `FOO=''` and `FOO="bar"` are).
    is_unquoted_assignment: bool,
    /// True iff the token was a single standalone unquoted `{`, `}`, `!`, or
    /// `[[` (reserved words that contain bytes outside the allowed set).
    standalone_reserved: Option<&'static str>,
}

fn push_unread(
    scanner: &mut ExactScanner<'_, '_>,
    unreads: &mut Vec<UnreadReason>,
    reason: UnreadReason,
) -> Result<(), Exhausted> {
    let extra = match &reason {
        UnreadReason::ReservedWordOrConstruct { construct } => construct.len(),
        UnreadReason::UnsupportedRedirect { op } => op.len(),
        _ => 0,
    };
    scanner.charge_unread_meta(extra)?;
    vec_push_budgeted(unreads, reason, scanner.budget)
}

fn push_unread_construct(
    scanner: &mut ExactScanner<'_, '_>,
    unreads: &mut Vec<UnreadReason>,
    construct: &str,
) -> Result<(), Exhausted> {
    scanner.charge_unread_meta(construct.len())?;
    charge_vec_growth(unreads, 1, scanner.budget)?;
    unreads.push(UnreadReason::ReservedWordOrConstruct {
        construct: construct.to_string(),
    });
    Ok(())
}

fn push_unread_redirect_op(
    scanner: &mut ExactScanner<'_, '_>,
    unreads: &mut Vec<UnreadReason>,
    op: &str,
) -> Result<(), Exhausted> {
    scanner.charge_unread_meta(op.len())?;
    charge_vec_growth(unreads, 1, scanner.budget)?;
    unreads.push(UnreadReason::UnsupportedRedirect { op: op.to_string() });
    Ok(())
}

/// Scans `'...'` (with opening `'` at `scanner.peek()`), preserving
/// `ByteProvenance::SingleQuoted`. When `class_len` is `Some`, also enforces
/// §3's no-`!`/no-`^` rule inside glob classes and tracks class byte count.
fn scan_single_quoted(
    scanner: &mut ExactScanner<'_, '_>,
    word: &mut Word,
    unreads: &mut Vec<UnreadReason>,
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
                push_unread(scanner, unreads, UnreadReason::NegatedGlobClass)?;
            }
            **clen += 1;
        }
        scanner.push_word_byte(word, c, ByteProvenance::SingleQuoted)?;
    }
    if !closed {
        push_unread(scanner, unreads, UnreadReason::UnsupportedQuoteOrEscape)?;
    }
    Ok(())
}

/// Scans `"..."` (with opening `"` at `scanner.peek()`), preserving
/// `ByteProvenance::DoubleQuoted` / `ByteProvenance::Escaped`. When `class_len`
/// is `Some`, also enforces §3's no-`!`/no-`^` rule inside glob classes and
/// tracks class byte count.
fn scan_double_quoted(
    scanner: &mut ExactScanner<'_, '_>,
    word: &mut Word,
    unreads: &mut Vec<UnreadReason>,
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
                push_unread(scanner, unreads, UnreadReason::UnsupportedQuoteOrEscape)?;
                if let Some(ref mut clen) = class_len {
                    if c == b'!' {
                        push_unread(scanner, unreads, UnreadReason::NegatedGlobClass)?;
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
                    push_unread(scanner, unreads, UnreadReason::UnsupportedQuoteOrEscape)?;
                    if let Some(ref mut clen) = class_len {
                        if matches!(other, b'!' | b'^') {
                            push_unread(scanner, unreads, UnreadReason::NegatedGlobClass)?;
                        }
                        **clen += 2;
                    }
                    scanner.push_word_byte(word, b'\\', ByteProvenance::DoubleQuoted)?;
                    scanner.push_word_byte(word, other, ByteProvenance::DoubleQuoted)?;
                }
                None => {
                    push_unread(scanner, unreads, UnreadReason::UnsupportedQuoteOrEscape)?;
                    break;
                }
            },
            other => {
                if let Some(ref mut clen) = class_len {
                    if other == b'^' {
                        push_unread(scanner, unreads, UnreadReason::NegatedGlobClass)?;
                    }
                    **clen += 1;
                }
                scanner.push_word_byte(word, other, ByteProvenance::DoubleQuoted)?;
            }
        }
    }
    if !closed {
        push_unread(scanner, unreads, UnreadReason::UnsupportedQuoteOrEscape)?;
    }
    Ok(())
}

/// Scans an outside-quotes backslash escape (`\` at `scanner.peek()`),
/// preserving `ByteProvenance::Escaped`. When `class_len` is `Some`, also
/// tracks class byte count and `!`/`^` inside glob classes.
fn scan_unquoted_escape(
    scanner: &mut ExactScanner<'_, '_>,
    word: &mut Word,
    unreads: &mut Vec<UnreadReason>,
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
            push_unread(scanner, unreads, UnreadReason::UnsupportedQuoteOrEscape)?;
        }
        Some(next) if next >= 0x80 => {
            push_unread(scanner, unreads, UnreadReason::UnsupportedQuoteOrEscape)?;
            if let Some(clen) = class_len {
                *clen += 1;
            }
            scanner.consume_utf8_char_into(word, ByteProvenance::Escaped)?;
        }
        Some(next) => {
            scanner.next_byte()?;
            push_unread(scanner, unreads, UnreadReason::UnsupportedQuoteOrEscape)?;
            if let Some(clen) = class_len {
                if matches!(next, b'!' | b'^') {
                    push_unread(scanner, unreads, UnreadReason::NegatedGlobClass)?;
                }
                *clen += 1;
            }
            scanner.push_word_byte(word, next, ByteProvenance::Escaped)?;
        }
        None => {
            push_unread(scanner, unreads, UnreadReason::UnsupportedQuoteOrEscape)?;
        }
    }
    Ok(())
}

/// Reads one word starting at `scanner.pos` (which must not be at EOF or a
/// word-boundary metacharacter). Records any §3 `UnreadReason` encountered into
/// `unreads`.
fn read_word(
    scanner: &mut ExactScanner<'_, '_>,
    unreads: &mut Vec<UnreadReason>,
    in_command_position: bool,
) -> Result<ReadWordOutcome, Exhausted> {
    scanner.charge_word_meta()?;
    let start_pos = scanner.pos;

    // Fast-recognize standalone unquoted `{`, `}`, `!`, `[[` when bounded by
    // word boundaries so they report `ReservedWordOrConstruct` cleanly.
    if let Some(b) = scanner.peek() {
        let next = scanner.peek_at(1);
        let at_single_boundary = next.is_none_or(is_word_boundary);
        if matches!(b, b'{' | b'}') && at_single_boundary && in_command_position {
            scanner.next_byte()?;
            let mut word = Word::new();
            scanner.push_word_byte(&mut word, b, ByteProvenance::Unquoted)?;
            let kw = if b == b'{' { "{" } else { "}" };
            return Ok(ReadWordOutcome {
                word,
                all_unquoted_digits: false,
                contained_quoting: false,
                is_unquoted_assignment: false,
                standalone_reserved: Some(kw),
            });
        }
        if b == b'!' && at_single_boundary && in_command_position {
            scanner.next_byte()?;
            let mut word = Word::new();
            scanner.push_word_byte(&mut word, b'!', ByteProvenance::Unquoted)?;
            return Ok(ReadWordOutcome {
                word,
                all_unquoted_digits: false,
                contained_quoting: false,
                is_unquoted_assignment: false,
                standalone_reserved: Some("!"),
            });
        }
        if b == b'['
            && next == Some(b'[')
            && scanner.peek_at(2).is_none_or(is_word_boundary)
            && in_command_position
        {
            scanner.advance_n(2)?;
            let mut word = Word::new();
            scanner.push_word_byte(&mut word, b'[', ByteProvenance::Unquoted)?;
            scanner.push_word_byte(&mut word, b'[', ByteProvenance::Unquoted)?;
            return Ok(ReadWordOutcome {
                word,
                all_unquoted_digits: false,
                contained_quoting: false,
                is_unquoted_assignment: false,
                standalone_reserved: Some("[["),
            });
        }
    }

    let mut word = Word::new();
    let mut started = false;
    let mut all_unquoted_digits = true;
    let mut contained_quoting = false;
    let mut seen_first_unquoted_eq = false;
    let mut is_unquoted_assignment = false;

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
                scan_single_quoted(scanner, &mut word, unreads, None)?;
            }
            b'"' => {
                // Double quotes: any bytes except `$`, backtick, `!`;
                // `\` allowed ONLY in `\"` and `\\`.
                started = true;
                all_unquoted_digits = false;
                contained_quoting = true;
                scan_double_quoted(scanner, &mut word, unreads, None)?;
            }
            b'\\' => {
                // Outside quotes: backslash allowed ONLY before allowed
                // characters or glob characters (`*`, `?`, `[`, `]`).
                // Backslash-newline is outside the model.
                started = true;
                all_unquoted_digits = false;
                contained_quoting = true;
                scan_unquoted_escape(scanner, &mut word, unreads, None)?;
            }
            b'~' => {
                let at_word_start = !started;
                started = true;
                all_unquoted_digits = false;
                scanner.next_byte()?;
                if at_word_start {
                    match scanner.peek() {
                        None | Some(b'/') => {}
                        Some(next) if is_word_boundary(next) => {}
                        Some(_) => {
                            push_unread(scanner, unreads, UnreadReason::UnsupportedTildeForm)?;
                        }
                    }
                } else {
                    push_unread(
                        scanner,
                        unreads,
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
                    push_unread(
                        scanner,
                        unreads,
                        UnreadReason::UnquotedByteOutsideModel { byte: b'=' },
                    )?;
                } else if !seen_first_unquoted_eq {
                    seen_first_unquoted_eq = true;
                    if !contained_quoting {
                        let prefix = word.literal_bytes();
                        let name = prefix.strip_suffix(b"+").unwrap_or(prefix);
                        if !prefix.is_empty() && is_posix_name(name) {
                            is_unquoted_assignment = true;
                        }
                    }
                }
                scanner.push_word_byte(&mut word, b'=', ByteProvenance::Unquoted)?;
            }
            b'*' | b'?' => {
                started = true;
                all_unquoted_digits = false;
                scanner.next_byte()?;
                scanner.push_word_byte(&mut word, b, ByteProvenance::Unquoted)?;
            }
            b'[' => {
                // Glob character class `[...]`: no `!` or `^` inside, must be
                // non-empty and closed, permits §3 quotes and escapes while
                // retaining class state and byte provenance.
                started = true;
                all_unquoted_digits = false;
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
                                    unreads,
                                    UnreadReason::UnquotedByteOutsideModel { byte: b'[' },
                                )?;
                            }
                            found_close = true;
                            break;
                        }
                        b'!' | b'^' => {
                            scanner.next_byte()?;
                            push_unread(scanner, unreads, UnreadReason::NegatedGlobClass)?;
                            scanner.push_word_byte(&mut word, cb, ByteProvenance::Unquoted)?;
                            class_len += 1;
                        }
                        b'\'' => {
                            contained_quoting = true;
                            scan_single_quoted(scanner, &mut word, unreads, Some(&mut class_len))?;
                        }
                        b'"' => {
                            contained_quoting = true;
                            scan_double_quoted(scanner, &mut word, unreads, Some(&mut class_len))?;
                        }
                        b'\\' => {
                            contained_quoting = true;
                            scan_unquoted_escape(
                                scanner,
                                &mut word,
                                unreads,
                                Some(&mut class_len),
                            )?;
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
                                unreads,
                                UnreadReason::UnquotedByteOutsideModel { byte: lead },
                            )?;
                            class_len += 1;
                        }
                        c => {
                            scanner.next_byte()?;
                            push_unread(
                                scanner,
                                unreads,
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
                        unreads,
                        UnreadReason::UnquotedByteOutsideModel { byte: b'[' },
                    )?;
                }
            }
            c if is_allowed_unquoted_base(c) => {
                started = true;
                if !c.is_ascii_digit() {
                    all_unquoted_digits = false;
                }
                scanner.next_byte()?;
                scanner.push_word_byte(&mut word, c, ByteProvenance::Unquoted)?;
            }
            c if c >= 0x80 => {
                started = true;
                all_unquoted_digits = false;
                let lead = scanner.consume_utf8_char_into(&mut word, ByteProvenance::Unquoted)?;
                push_unread(
                    scanner,
                    unreads,
                    UnreadReason::UnquotedByteOutsideModel { byte: lead },
                )?;
            }
            c => {
                // Any other unquoted ASCII byte (`$`, `` ` ``, `{`, `}`, `#`,
                // `!`, `^`, `]`, `\r`, control bytes, etc.).
                started = true;
                all_unquoted_digits = false;
                scanner.next_byte()?;
                push_unread(
                    scanner,
                    unreads,
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
    Ok(ReadWordOutcome {
        word,
        all_unquoted_digits: is_digits,
        contained_quoting,
        is_unquoted_assignment,
        standalone_reserved: None,
    })
}

/// Parses a redirect starting at `<` or `>` (with optional preceding explicit
/// fd digits, or `&>` / `&>>`).
fn lex_exact_redirect(
    scanner: &mut ExactScanner<'_, '_>,
    explicit_fd: Option<(u32, usize)>,
    cur: &mut PendingUnit,
    heredocs: &mut Vec<(Vec<u8>, bool)>,
) -> Result<(), Exhausted> {
    let c0 = scanner.peek().unwrap_or(0);
    let c1 = scanner.peek_at(1);
    let c2 = scanner.peek_at(2);

    // Combined redirect `&>` / `&>>` (not shared by POSIX sh).
    if c0 == b'&' && c1 == Some(b'>') {
        let len = if c2 == Some(b'>') { 3 } else { 2 };
        scanner.advance_n(len)?;
        push_unread(scanner, &mut cur.unreads, UnreadReason::CombinedRedirect)?;
        scanner.skip_horiz_ws()?;
        if scanner.peek().is_some_and(|b| !is_word_boundary(b)) {
            let _ = read_word(scanner, &mut cur.unreads, false)?;
        } else {
            let op = if len == 3 { "&>>" } else { "&>" };
            push_unread_redirect_op(scanner, &mut cur.unreads, op)?;
        }
        return Ok(());
    }

    // Here-string `<<<`.
    if c0 == b'<' && c1 == Some(b'<') && c2 == Some(b'<') {
        scanner.advance_n(3)?;
        push_unread_construct(scanner, &mut cur.unreads, "<<<")?;
        if let Some((fd, _)) = explicit_fd
            && fd > 2
        {
            push_unread(
                scanner,
                &mut cur.unreads,
                UnreadReason::DescriptorOutsideStd { fd },
            )?;
        }
        scanner.skip_horiz_ws()?;
        if scanner.peek().is_some_and(|b| !is_word_boundary(b)) {
            let _ = read_word(scanner, &mut cur.unreads, false)?;
        }
        return Ok(());
    }

    // Here-document `<<` / `<<-`.
    if c0 == b'<' && c1 == Some(b'<') {
        let strip_tabs = c2 == Some(b'-');
        scanner.advance_n(if strip_tabs { 3 } else { 2 })?;
        push_unread_construct(
            scanner,
            &mut cur.unreads,
            if strip_tabs { "<<-" } else { "<<" },
        )?;
        if let Some((fd, _)) = explicit_fd
            && fd > 2
        {
            push_unread(
                scanner,
                &mut cur.unreads,
                UnreadReason::DescriptorOutsideStd { fd },
            )?;
        }
        scanner.skip_horiz_ws()?;
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
            vec_push_budgeted(heredocs, (delim, strip_tabs), scanner.budget)?;
        }
        return Ok(());
    }

    // Process substitution `<(` / `>(`.
    if matches!(c0, b'<' | b'>') && c1 == Some(b'(') {
        scanner.advance_n(2)?;
        push_unread_construct(
            scanner,
            &mut cur.unreads,
            if c0 == b'<' { "<(" } else { ">(" },
        )?;
        return Ok(());
    }

    // Standard / unsupported redirect operators.
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

    let target_opt = if scanner.peek().is_some_and(|b| !is_word_boundary(b)) {
        Some(read_word(scanner, &mut cur.unreads, false)?)
    } else {
        None
    };

    if let Some((fd, _)) = explicit_fd
        && fd > 2
    {
        push_unread(
            scanner,
            &mut cur.unreads,
            UnreadReason::DescriptorOutsideStd { fd },
        )?;
        // Also check if DupOut/DupIn has a target descriptor > 2.
        if matches!(op, Op::DupOut | Op::DupIn)
            && let Some(ref t) = target_opt
            && t.all_unquoted_digits
            && let Some(m) = parse_u32_digits(t.word.literal_bytes())
            && m > 2
        {
            push_unread(
                scanner,
                &mut cur.unreads,
                UnreadReason::DescriptorOutsideStd { fd: m },
            )?;
        }
        return Ok(());
    }

    match op {
        Op::DupIn => {
            push_unread_redirect_op(scanner, &mut cur.unreads, "<&")?;
            if let Some(ref t) = target_opt
                && t.all_unquoted_digits
                && let Some(m) = parse_u32_digits(t.word.literal_bytes())
                && m > 2
            {
                push_unread(
                    scanner,
                    &mut cur.unreads,
                    UnreadReason::DescriptorOutsideStd { fd: m },
                )?;
            }
        }
        Op::ReadWrite | Op::Clobber => {
            push_unread_redirect_op(scanner, &mut cur.unreads, op_str)?;
        }
        Op::DupOut => {
            let Some(target) = target_opt else {
                push_unread_redirect_op(scanner, &mut cur.unreads, ">&")?;
                return Ok(());
            };
            let Some((fd, digit_len)) = explicit_fd else {
                // `>&2` / `>&-` / `>&file`: explicit `n` in 0..=2 required by §3's closed list.
                push_unread_redirect_op(scanner, &mut cur.unreads, ">&")?;
                if target.all_unquoted_digits
                    && let Some(m) = parse_u32_digits(target.word.literal_bytes())
                    && m > 2
                {
                    push_unread(
                        scanner,
                        &mut cur.unreads,
                        UnreadReason::DescriptorOutsideStd { fd: m },
                    )?;
                }
                return Ok(());
            };
            if digit_len != 1 {
                push_unread_redirect_op(scanner, &mut cur.unreads, ">&")?;
                return Ok(());
            }
            let Some(from) = StdFd::from_u8(fd as u8) else {
                push_unread(
                    scanner,
                    &mut cur.unreads,
                    UnreadReason::DescriptorOutsideStd { fd },
                )?;
                return Ok(());
            };
            let tb = target.word.literal_bytes();
            if !target.contained_quoting && target.word.is_all_unquoted() && tb == b"-" {
                scanner.charge_redirect_meta()?;
                vec_push_budgeted(
                    &mut cur.redirects,
                    ModelRedirect::Close { fd: from },
                    scanner.budget,
                )?;
            } else if target.all_unquoted_digits && tb.len() == 1 && matches!(tb[0], b'0'..=b'2') {
                let to = StdFd::from_u8(tb[0] - b'0').expect("0..=2");
                scanner.charge_redirect_meta()?;
                vec_push_budgeted(
                    &mut cur.redirects,
                    ModelRedirect::Dup { from, to },
                    scanner.budget,
                )?;
            } else if target.all_unquoted_digits {
                if let Some(m) = parse_u32_digits(tb)
                    && m > 2
                {
                    push_unread(
                        scanner,
                        &mut cur.unreads,
                        UnreadReason::DescriptorOutsideStd { fd: m },
                    )?;
                } else {
                    push_unread_redirect_op(scanner, &mut cur.unreads, ">&")?;
                }
            } else {
                push_unread_redirect_op(scanner, &mut cur.unreads, ">&")?;
            }
        }
        Op::Read | Op::Write | Op::Append => {
            let Some(target) = target_opt else {
                push_unread_redirect_op(scanner, &mut cur.unreads, op_str)?;
                return Ok(());
            };
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
                    if r.target().is_some_and(Word::has_unquoted_glob) {
                        // zsh expands globs in redirect targets (MULTIOS); not a literal target.
                        let suffix = " (glob target)";
                        scanner.charge_unread_meta(op_str.len() + suffix.len())?;
                        charge_vec_growth(&mut cur.unreads, 1, scanner.budget)?;
                        cur.unreads.push(UnreadReason::UnsupportedRedirect {
                            op: format!("{op_str}{suffix}"),
                        });
                    }
                    scanner.charge_redirect_meta()?;
                    vec_push_budgeted(&mut cur.redirects, r, scanner.budget)?;
                }
                None => match explicit_fd {
                    Some((fd, _)) => {
                        let extra = 10 + op_str.len();
                        scanner.charge_unread_meta(extra)?;
                        charge_vec_growth(&mut cur.unreads, 1, scanner.budget)?;
                        cur.unreads.push(UnreadReason::UnsupportedRedirect {
                            op: format!("{fd}{op_str}"),
                        });
                    }
                    None => push_unread_redirect_op(scanner, &mut cur.unreads, op_str)?,
                },
            }
        }
    }

    Ok(())
}

fn parse_u32_digits(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut val = 0u32;
    for &b in bytes {
        val = val.saturating_mul(10).saturating_add(u32::from(b - b'0'));
    }
    Some(val)
}

/// Skips pending here-doc bodies after a newline, charging 1 compute unit per
/// byte scanned so body text is never mis-parsed as modeled commands.
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
                    slice = &slice[1..];
                }
            }
            if slice == delim.as_slice() {
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
/// **Owned by:** P1 · **Contract:** §3, §8.
pub(crate) fn lex_exact(
    line: &str,
    depth: u8,
    parent_unit: Option<UnitIndex>,
    budget: &mut ComputeBudget,
) -> Result<LexExactOutput, PartOutcome> {
    if line.len() > limits::MAX_BYTES {
        return Err(PartOutcome::TooLarge(Cap::Bytes));
    }

    let whole_part = parent_unit.map_or(PartId::WholeCall, PartId::Unit);
    let mut out = LexExactOutput::default();

    // Charge at least 1 unit on entry so an already-exhausted budget fails
    // immediately even on empty input.
    if let Err(e) = budget.charge(1) {
        out.outcomes.push(PartResult {
            part: whole_part,
            outcome: PartOutcome::Exhausted(e.reason),
        });
        return Ok(out);
    }

    let mut scanner = ExactScanner::new(line, budget);
    let mut cur = PendingUnit::new(Separator::Start, None);
    let mut next_pipe_id = 0u32;
    let mut active_pipe: Option<PipelineId> = None;
    let mut expecting_after_op: Option<&'static str> = None;
    let mut heredocs: Vec<(Vec<u8>, bool)> = Vec::new();

    let flush_unit = |cur: &mut PendingUnit,
                      next_sep: Separator,
                      next_pipe: Option<PipelineId>,
                      out: &mut LexExactOutput,
                      scanner: &mut ExactScanner<'_, '_>|
     -> Result<(), Result<PartOutcome, Exhausted>> {
        if cur.is_empty() {
            if !cur.unreads.is_empty() {
                charge_vec_growth(&mut out.outcomes, cur.unreads.len(), scanner.budget)
                    .map_err(Err)?;
            }
            let finished = std::mem::replace(cur, PendingUnit::new(next_sep, next_pipe));
            for r in finished.unreads {
                out.outcomes.push(PartResult {
                    part: whole_part.clone(),
                    outcome: PartOutcome::Unread(r),
                });
            }
            return Ok(());
        }
        if out.units.len() >= limits::MAX_UNITS {
            return Err(Ok(PartOutcome::TooLarge(Cap::Units)));
        }
        // Charge unit metadata, output vector growth, and redirect-only reason
        // BEFORE moving `cur` so exhaustion preserves the pending unit's words
        // and reasons (§8).
        let redirect_only = cur.argv.is_empty() && !cur.redirects.is_empty();
        scanner.charge_unit_meta().map_err(Err)?;
        charge_vec_growth(&mut out.units, 1, scanner.budget).map_err(Err)?;
        let extra_outcomes = cur.unreads.len() + usize::from(redirect_only);
        if extra_outcomes > 0 {
            charge_vec_growth(&mut out.outcomes, extra_outcomes, scanner.budget).map_err(Err)?;
        }
        if redirect_only {
            scanner
                .charge_unread_meta("redirect-only command".len())
                .map_err(Err)?;
        }
        let finished = std::mem::replace(cur, PendingUnit::new(next_sep, next_pipe));
        let idx = UnitIndex(out.units.len());
        for r in finished.unreads {
            out.outcomes.push(PartResult {
                part: PartId::Unit(idx),
                outcome: PartOutcome::Unread(r),
            });
        }
        if redirect_only {
            // Redirect-only command (`> foo`, `< foo`) diverges in zsh
            // (NULLCMD / READNULLCMD). Keep the unit so its redirects are still
            // checked as literal paths, while marking it Unread.
            out.outcomes.push(PartResult {
                part: PartId::Unit(idx),
                outcome: PartOutcome::Unread(UnreadReason::ReservedWordOrConstruct {
                    construct: "redirect-only command".to_string(),
                }),
            });
        }
        out.units.push(RawCommandUnit {
            index: idx,
            argv: finished.argv,
            redirects: finished.redirects,
            separator_before: finished.separator_before,
            pipeline_id: finished.pipeline_id,
            depth,
            parent_unit,
        });
        Ok(())
    };

    let run = (|| -> Result<(), Result<PartOutcome, Exhausted>> {
        while let Some(b) = scanner.peek() {
            match b {
                b' ' | b'\t' => {
                    scanner.next_byte().map_err(Err)?;
                }
                b'#' => {
                    // Comment at token boundary: outside the model (§3).
                    push_unread_construct(&mut scanner, &mut cur.unreads, "#").map_err(Err)?;
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
                    active_pipe = None;
                    expecting_after_op = None;
                    flush_unit(&mut cur, Separator::Newline, None, &mut out, &mut scanner)?;
                }
                b';' => {
                    if scanner.peek_at(1) == Some(b';') {
                        scanner.advance_n(2).map_err(Err)?;
                        push_unread_construct(&mut scanner, &mut cur.unreads, ";;").map_err(Err)?;
                    } else {
                        scanner.next_byte().map_err(Err)?;
                        if cur.is_empty() {
                            push_unread_construct(&mut scanner, &mut cur.unreads, ";")
                                .map_err(Err)?;
                        }
                    }
                    if let Some(op) = expecting_after_op.take()
                        && cur.is_empty()
                    {
                        push_unread_construct(&mut scanner, &mut cur.unreads, op).map_err(Err)?;
                    }
                    active_pipe = None;
                    flush_unit(&mut cur, Separator::Semi, None, &mut out, &mut scanner)?;
                }
                b'&' => match scanner.peek_at(1) {
                    Some(b'&') => {
                        scanner.advance_n(2).map_err(Err)?;
                        if cur.is_empty() {
                            push_unread_construct(&mut scanner, &mut cur.unreads, "&&")
                                .map_err(Err)?;
                        }
                        active_pipe = None;
                        expecting_after_op = Some("&&");
                        flush_unit(&mut cur, Separator::And, None, &mut out, &mut scanner)?;
                    }
                    Some(b'>') => {
                        expecting_after_op = None;
                        lex_exact_redirect(&mut scanner, None, &mut cur, &mut heredocs)
                            .map_err(Err)?;
                    }
                    _ => {
                        scanner.next_byte().map_err(Err)?;
                        push_unread_construct(&mut scanner, &mut cur.unreads, "&").map_err(Err)?;
                        active_pipe = None;
                        expecting_after_op = None;
                        flush_unit(&mut cur, Separator::Semi, None, &mut out, &mut scanner)?;
                    }
                },
                b'|' => match scanner.peek_at(1) {
                    Some(b'|') => {
                        scanner.advance_n(2).map_err(Err)?;
                        if cur.is_empty() {
                            push_unread_construct(&mut scanner, &mut cur.unreads, "||")
                                .map_err(Err)?;
                        }
                        active_pipe = None;
                        expecting_after_op = Some("||");
                        flush_unit(&mut cur, Separator::Or, None, &mut out, &mut scanner)?;
                    }
                    Some(b'&') => {
                        scanner.advance_n(2).map_err(Err)?;
                        push_unread_construct(&mut scanner, &mut cur.unreads, "|&").map_err(Err)?;
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
                        active_pipe = pipe;
                        expecting_after_op = Some("|&");
                        flush_unit(&mut cur, Separator::Pipe, pipe, &mut out, &mut scanner)?;
                    }
                    _ => {
                        scanner.next_byte().map_err(Err)?;
                        let pipe = if cur.is_empty() {
                            push_unread_construct(&mut scanner, &mut cur.unreads, "|")
                                .map_err(Err)?;
                            None
                        } else {
                            let id = cur.pipeline_id.unwrap_or_else(|| {
                                next_pipe_id += 1;
                                PipelineId(next_pipe_id)
                            });
                            cur.pipeline_id = Some(id);
                            Some(id)
                        };
                        active_pipe = pipe;
                        expecting_after_op = Some("|");
                        flush_unit(&mut cur, Separator::Pipe, pipe, &mut out, &mut scanner)?;
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
                    push_unread_construct(&mut scanner, &mut cur.unreads, construct)
                        .map_err(Err)?;
                }
                b'<' | b'>' => {
                    expecting_after_op = None;
                    lex_exact_redirect(&mut scanner, None, &mut cur, &mut heredocs).map_err(Err)?;
                }
                _ => {
                    expecting_after_op = None;
                    let in_cmd_pos = !cur.seen_command_word;
                    let rw = read_word(&mut scanner, &mut cur.unreads, in_cmd_pos).map_err(Err)?;

                    // Unquoted digits directly followed by `<` or `>` (not
                    // process substitution `<(` / `>(`) form an explicit fd.
                    if rw.all_unquoted_digits
                        && matches!(scanner.peek(), Some(b'<' | b'>'))
                        && scanner.peek_at(1) != Some(b'(')
                    {
                        let digit_len = rw.word.len();
                        let fd = parse_u32_digits(rw.word.literal_bytes()).unwrap_or(u32::MAX);
                        lex_exact_redirect(
                            &mut scanner,
                            Some((fd, digit_len)),
                            &mut cur,
                            &mut heredocs,
                        )
                        .map_err(Err)?;
                        continue;
                    }

                    if let Some(kw) = rw.standalone_reserved {
                        push_unread_construct(&mut scanner, &mut cur.unreads, kw).map_err(Err)?;
                        cur.seen_command_word = true;
                        vec_push_budgeted(&mut cur.argv, rw.word, scanner.budget).map_err(Err)?;
                        continue;
                    }

                    if !cur.seen_command_word {
                        if rw.is_unquoted_assignment {
                            push_unread(&mut scanner, &mut cur.unreads, UnreadReason::Assignment)
                                .map_err(Err)?;
                            vec_push_budgeted(&mut cur.argv, rw.word, scanner.budget)
                                .map_err(Err)?;
                            continue;
                        }
                        if !rw.contained_quoting
                            && rw.word.is_all_unquoted()
                            && let Some(s) = rw.word.as_str()
                            && is_reserved_word(s)
                        {
                            push_unread_construct(&mut scanner, &mut cur.unreads, s)
                                .map_err(Err)?;
                        }
                        cur.seen_command_word = true;
                    }
                    vec_push_budgeted(&mut cur.argv, rw.word, scanner.budget).map_err(Err)?;
                }
            }
        }

        if let Some(op) = expecting_after_op.take()
            && cur.is_empty()
        {
            push_unread_construct(&mut scanner, &mut cur.unreads, op).map_err(Err)?;
        }
        let _ = active_pipe;
        flush_unit(&mut cur, Separator::Start, None, &mut out, &mut scanner)?;
        Ok(())
    })();

    match run {
        Ok(()) => Ok(out),
        Err(Ok(too_large)) => Err(too_large),
        Err(Err(exhausted)) => {
            // Preserve every unit and outcome found before budget exhaustion
            // (§8: nothing found before exhaustion is lost).
            if !cur.is_empty() && out.units.len() < limits::MAX_UNITS {
                let idx = UnitIndex(out.units.len());
                let redirect_only = cur.argv.is_empty() && !cur.redirects.is_empty();
                for r in cur.unreads {
                    out.outcomes.push(PartResult {
                        part: PartId::Unit(idx),
                        outcome: PartOutcome::Unread(r),
                    });
                }
                if redirect_only {
                    out.outcomes.push(PartResult {
                        part: PartId::Unit(idx),
                        outcome: PartOutcome::Unread(UnreadReason::ReservedWordOrConstruct {
                            construct: "redirect-only command".to_string(),
                        }),
                    });
                }
                out.units.push(RawCommandUnit {
                    index: idx,
                    argv: cur.argv,
                    redirects: cur.redirects,
                    separator_before: cur.separator_before,
                    pipeline_id: cur.pipeline_id,
                    depth,
                    parent_unit,
                });
            } else {
                for r in cur.unreads {
                    out.outcomes.push(PartResult {
                        part: whole_part.clone(),
                        outcome: PartOutcome::Unread(r),
                    });
                }
            }
            out.outcomes.push(PartResult {
                part: whole_part,
                outcome: PartOutcome::Exhausted(exhausted.reason),
            });
            Ok(out)
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
/// `fold_command_word` and the best-effort pass).
pub(super) fn fold_command_bytes(
    raw: &[u8],
    folds: &FoldRules,
    budget: &mut ComputeBudget,
) -> Result<String, Exhausted> {
    // Charge scanning of the full command word before inspecting it (§8).
    budget.charge(raw.len().max(1) as u64)?;
    let base = match raw.iter().rposition(|&b| b == b'/') {
        Some(pos) => &raw[pos + 1..],
        None => raw,
    };

    // Fast path for pure ASCII basenames.
    if base.is_ascii() {
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
    // (`ſ` U+017F -> `s` and `K` U+212A -> `k`), with NO Unicode normalization
    // (NFC/NFD).
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
        budget.charge(lower_total.max(1) as u64)?;
        budget.charge_alloc(lower_alloc_est)?;
        let valid = std::str::from_utf8(base).expect("no invalid UTF-8 chunks");
        return Ok(valid.to_lowercase());
    }

    budget.charge(lower_total.saturating_add(out_total).max(1) as u64)?;
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
