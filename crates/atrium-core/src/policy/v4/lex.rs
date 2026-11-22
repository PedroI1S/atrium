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
    crate::policy::shell::RESERVED.contains(&s)
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
        self.budget.charge(1)?;
        self.budget.charge_alloc(std::mem::size_of::<Word>())
    }

    #[inline]
    fn charge_unit_meta(&mut self) -> Result<(), Exhausted> {
        self.budget.charge(1)?;
        self.budget
            .charge_alloc(std::mem::size_of::<RawCommandUnit>())
    }

    #[inline]
    fn charge_redirect_meta(&mut self) -> Result<(), Exhausted> {
        self.budget.charge(1)?;
        self.budget
            .charge_alloc(std::mem::size_of::<ModelRedirect>())
    }

    #[inline]
    fn charge_unread_meta(&mut self, extra_bytes: usize) -> Result<(), Exhausted> {
        self.budget.charge(1)?;
        self.budget
            .charge_alloc(std::mem::size_of::<PartResult>() + extra_bytes)
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
    unreads.push(reason);
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
        if matches!(b, b'{' | b'}') && at_single_boundary {
            scanner.next_byte()?;
            let mut word = Word::new();
            scanner.push_word_byte(&mut word, b, ByteProvenance::Unquoted)?;
            let kw = if b == b'{' { "{" } else { "}" };
            return Ok(ReadWordOutcome {
                word,
                all_unquoted_digits: false,
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
                standalone_reserved: Some("[["),
            });
        }
    }

    let mut word = Word::new();
    let mut started = false;
    let mut all_unquoted_digits = true;

    while let Some(b) = scanner.peek() {
        if is_word_boundary(b) {
            break;
        }
        match b {
            b'\'' => {
                // Single quotes: any bytes until matching `'`.
                started = true;
                all_unquoted_digits = false;
                scanner.next_byte()?;
                let mut closed = false;
                while let Some(c) = scanner.next_byte()? {
                    if c == b'\'' {
                        closed = true;
                        break;
                    }
                    scanner.push_word_byte(&mut word, c, ByteProvenance::SingleQuoted)?;
                }
                if !closed {
                    push_unread(scanner, unreads, UnreadReason::UnsupportedQuoteOrEscape)?;
                }
            }
            b'"' => {
                // Double quotes: any bytes except `$`, backtick, `!`;
                // `\` allowed ONLY in `\"` and `\\`.
                started = true;
                all_unquoted_digits = false;
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
                            scanner.push_word_byte(&mut word, c, ByteProvenance::DoubleQuoted)?;
                        }
                        b'\\' => match scanner.next_byte()? {
                            Some(esc @ (b'"' | b'\\')) => {
                                scanner.push_word_byte(&mut word, esc, ByteProvenance::Escaped)?;
                            }
                            Some(other) => {
                                push_unread(
                                    scanner,
                                    unreads,
                                    UnreadReason::UnsupportedQuoteOrEscape,
                                )?;
                                scanner.push_word_byte(
                                    &mut word,
                                    b'\\',
                                    ByteProvenance::DoubleQuoted,
                                )?;
                                scanner.push_word_byte(
                                    &mut word,
                                    other,
                                    ByteProvenance::DoubleQuoted,
                                )?;
                            }
                            None => {
                                push_unread(
                                    scanner,
                                    unreads,
                                    UnreadReason::UnsupportedQuoteOrEscape,
                                )?;
                                break;
                            }
                        },
                        other => {
                            scanner.push_word_byte(
                                &mut word,
                                other,
                                ByteProvenance::DoubleQuoted,
                            )?;
                        }
                    }
                }
                if !closed {
                    push_unread(scanner, unreads, UnreadReason::UnsupportedQuoteOrEscape)?;
                }
            }
            b'\\' => {
                // Outside quotes: backslash allowed ONLY before allowed
                // characters or glob characters (`*`, `?`, `[`, `]`).
                // Backslash-newline is outside the model.
                started = true;
                all_unquoted_digits = false;
                scanner.next_byte()?;
                match scanner.peek() {
                    Some(next) if is_escapable_outside_quotes(next) => {
                        scanner.next_byte()?;
                        scanner.push_word_byte(&mut word, next, ByteProvenance::Escaped)?;
                    }
                    Some(b'\n') => {
                        scanner.next_byte()?;
                        push_unread(scanner, unreads, UnreadReason::UnsupportedQuoteOrEscape)?;
                    }
                    Some(next) if next >= 0x80 => {
                        push_unread(scanner, unreads, UnreadReason::UnsupportedQuoteOrEscape)?;
                        scanner.consume_utf8_char_into(&mut word, ByteProvenance::Escaped)?;
                    }
                    Some(next) => {
                        scanner.next_byte()?;
                        push_unread(scanner, unreads, UnreadReason::UnsupportedQuoteOrEscape)?;
                        scanner.push_word_byte(&mut word, next, ByteProvenance::Escaped)?;
                    }
                    None => {
                        push_unread(scanner, unreads, UnreadReason::UnsupportedQuoteOrEscape)?;
                    }
                }
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
                // non-empty and closed, only allowed ASCII/glob chars inside.
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
                        b'\'' | b'"' | b'\\' => {
                            push_unread(scanner, unreads, UnreadReason::UnsupportedQuoteOrEscape)?;
                            // Hand back to outer word loop so quotes stay in sync.
                            found_close = true;
                            break;
                        }
                        c if is_allowed_unquoted_base(c) || matches!(c, b'=' | b'*' | b'?') => {
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

    Ok(ReadWordOutcome {
        word,
        all_unquoted_digits: started && all_unquoted_digits && !word.is_empty(),
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
            push_unread(
                scanner,
                &mut cur.unreads,
                UnreadReason::UnsupportedRedirect { op: op.to_string() },
            )?;
        }
        return Ok(());
    }

    // Here-string `<<<`.
    if c0 == b'<' && c1 == Some(b'<') && c2 == Some(b'<') {
        scanner.advance_n(3)?;
        push_unread(
            scanner,
            &mut cur.unreads,
            UnreadReason::ReservedWordOrConstruct {
                construct: "<<<".to_string(),
            },
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
        if scanner.peek().is_some_and(|b| !is_word_boundary(b)) {
            let _ = read_word(scanner, &mut cur.unreads, false)?;
        }
        return Ok(());
    }

    // Here-document `<<` / `<<-`.
    if c0 == b'<' && c1 == Some(b'<') {
        let strip_tabs = c2 == Some(b'-');
        scanner.advance_n(if strip_tabs { 3 } else { 2 })?;
        push_unread(
            scanner,
            &mut cur.unreads,
            UnreadReason::ReservedWordOrConstruct {
                construct: if strip_tabs {
                    "<<-".to_string()
                } else {
                    "<<".to_string()
                },
            },
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
            scanner.next_byte()?;
            if !matches!(b, b'\'' | b'"' | b'\\') {
                scanner.budget.charge(1)?;
                scanner.budget.charge_alloc(1)?;
                delim.push(b);
            }
        }
        scanner
            .budget
            .charge_alloc(std::mem::size_of::<(Vec<u8>, bool)>())?;
        heredocs.push((delim, strip_tabs));
        return Ok(());
    }

    // Process substitution `<(` / `>(`.
    if matches!(c0, b'<' | b'>') && c1 == Some(b'(') {
        scanner.advance_n(2)?;
        push_unread(
            scanner,
            &mut cur.unreads,
            UnreadReason::ReservedWordOrConstruct {
                construct: if c0 == b'<' {
                    "<(".to_string()
                } else {
                    ">(".to_string()
                },
            },
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
            push_unread(
                scanner,
                &mut cur.unreads,
                UnreadReason::UnsupportedRedirect {
                    op: "<&".to_string(),
                },
            )?;
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
            push_unread(
                scanner,
                &mut cur.unreads,
                UnreadReason::UnsupportedRedirect {
                    op: op_str.to_string(),
                },
            )?;
        }
        Op::DupOut => {
            let Some(target) = target_opt else {
                push_unread(
                    scanner,
                    &mut cur.unreads,
                    UnreadReason::UnsupportedRedirect {
                        op: ">&".to_string(),
                    },
                )?;
                return Ok(());
            };
            let Some((fd, digit_len)) = explicit_fd else {
                // `>&2` / `>&-` / `>&file`: explicit `n` in 0..=2 required by §3's closed list.
                push_unread(
                    scanner,
                    &mut cur.unreads,
                    UnreadReason::UnsupportedRedirect {
                        op: ">&".to_string(),
                    },
                )?;
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
                push_unread(
                    scanner,
                    &mut cur.unreads,
                    UnreadReason::UnsupportedRedirect {
                        op: ">&".to_string(),
                    },
                )?;
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
            if target.word.is_all_unquoted() && tb == b"-" {
                scanner.charge_redirect_meta()?;
                cur.redirects.push(ModelRedirect::Close { fd: from });
            } else if target.all_unquoted_digits && tb.len() == 1 && matches!(tb[0], b'0'..=b'2') {
                let to = StdFd::from_u8(tb[0] - b'0').expect("0..=2");
                scanner.charge_redirect_meta()?;
                cur.redirects.push(ModelRedirect::Dup { from, to });
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
                    push_unread(
                        scanner,
                        &mut cur.unreads,
                        UnreadReason::UnsupportedRedirect {
                            op: ">&".to_string(),
                        },
                    )?;
                }
            } else {
                push_unread(
                    scanner,
                    &mut cur.unreads,
                    UnreadReason::UnsupportedRedirect {
                        op: ">&".to_string(),
                    },
                )?;
            }
        }
        Op::Read | Op::Write | Op::Append => {
            let Some(target) = target_opt else {
                push_unread(
                    scanner,
                    &mut cur.unreads,
                    UnreadReason::UnsupportedRedirect {
                        op: op_str.to_string(),
                    },
                )?;
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
                        push_unread(
                            scanner,
                            &mut cur.unreads,
                            UnreadReason::UnsupportedRedirect {
                                op: format!("{op_str} (glob target)"),
                            },
                        )?;
                    }
                    scanner.charge_redirect_meta()?;
                    cur.redirects.push(r);
                }
                None => {
                    let op_label = match explicit_fd {
                        Some((fd, _)) => format!("{fd}{op_str}"),
                        None => op_str.to_string(),
                    };
                    push_unread(
                        scanner,
                        &mut cur.unreads,
                        UnreadReason::UnsupportedRedirect { op: op_label },
                    )?;
                }
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
        let finished = std::mem::replace(cur, PendingUnit::new(next_sep, next_pipe));
        if finished.is_empty() {
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
        scanner.charge_unit_meta().map_err(Err)?;
        let idx = UnitIndex(out.units.len());
        let redirect_only = finished.argv.is_empty() && !finished.redirects.is_empty();
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
            scanner
                .charge_unread_meta("redirect-only command".len())
                .map_err(Err)?;
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
                    push_unread(
                        &mut scanner,
                        &mut cur.unreads,
                        UnreadReason::ReservedWordOrConstruct {
                            construct: "#".to_string(),
                        },
                    )
                    .map_err(Err)?;
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
                        push_unread(
                            &mut scanner,
                            &mut cur.unreads,
                            UnreadReason::ReservedWordOrConstruct {
                                construct: ";;".to_string(),
                            },
                        )
                        .map_err(Err)?;
                    } else {
                        scanner.next_byte().map_err(Err)?;
                        if cur.is_empty() {
                            push_unread(
                                &mut scanner,
                                &mut cur.unreads,
                                UnreadReason::ReservedWordOrConstruct {
                                    construct: ";".to_string(),
                                },
                            )
                            .map_err(Err)?;
                        }
                    }
                    if let Some(op) = expecting_after_op.take()
                        && cur.is_empty()
                    {
                        push_unread(
                            &mut scanner,
                            &mut cur.unreads,
                            UnreadReason::ReservedWordOrConstruct {
                                construct: op.to_string(),
                            },
                        )
                        .map_err(Err)?;
                    }
                    active_pipe = None;
                    flush_unit(&mut cur, Separator::Semi, None, &mut out, &mut scanner)?;
                }
                b'&' => match scanner.peek_at(1) {
                    Some(b'&') => {
                        scanner.advance_n(2).map_err(Err)?;
                        if cur.is_empty() {
                            push_unread(
                                &mut scanner,
                                &mut cur.unreads,
                                UnreadReason::ReservedWordOrConstruct {
                                    construct: "&&".to_string(),
                                },
                            )
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
                        push_unread(
                            &mut scanner,
                            &mut cur.unreads,
                            UnreadReason::ReservedWordOrConstruct {
                                construct: "&".to_string(),
                            },
                        )
                        .map_err(Err)?;
                        active_pipe = None;
                        expecting_after_op = None;
                        flush_unit(&mut cur, Separator::Semi, None, &mut out, &mut scanner)?;
                    }
                },
                b'|' => match scanner.peek_at(1) {
                    Some(b'|') => {
                        scanner.advance_n(2).map_err(Err)?;
                        if cur.is_empty() {
                            push_unread(
                                &mut scanner,
                                &mut cur.unreads,
                                UnreadReason::ReservedWordOrConstruct {
                                    construct: "||".to_string(),
                                },
                            )
                            .map_err(Err)?;
                        }
                        active_pipe = None;
                        expecting_after_op = Some("||");
                        flush_unit(&mut cur, Separator::Or, None, &mut out, &mut scanner)?;
                    }
                    Some(b'&') => {
                        scanner.advance_n(2).map_err(Err)?;
                        push_unread(
                            &mut scanner,
                            &mut cur.unreads,
                            UnreadReason::ReservedWordOrConstruct {
                                construct: "|&".to_string(),
                            },
                        )
                        .map_err(Err)?;
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
                            push_unread(
                                &mut scanner,
                                &mut cur.unreads,
                                UnreadReason::ReservedWordOrConstruct {
                                    construct: "|".to_string(),
                                },
                            )
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
                    push_unread(
                        &mut scanner,
                        &mut cur.unreads,
                        UnreadReason::ReservedWordOrConstruct {
                            construct: construct.to_string(),
                        },
                    )
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
                        push_unread(
                            &mut scanner,
                            &mut cur.unreads,
                            UnreadReason::ReservedWordOrConstruct {
                                construct: kw.to_string(),
                            },
                        )
                        .map_err(Err)?;
                        cur.seen_command_word = true;
                        cur.argv.push(rw.word);
                        continue;
                    }

                    if !cur.seen_command_word {
                        if leading_assignment_name(&rw.word).is_some() {
                            push_unread(&mut scanner, &mut cur.unreads, UnreadReason::Assignment)
                                .map_err(Err)?;
                            cur.argv.push(rw.word);
                            continue;
                        }
                        if rw.word.is_all_unquoted()
                            && let Some(s) = rw.word.as_str()
                            && is_reserved_word(s)
                        {
                            push_unread(
                                &mut scanner,
                                &mut cur.unreads,
                                UnreadReason::ReservedWordOrConstruct {
                                    construct: s.to_string(),
                                },
                            )
                            .map_err(Err)?;
                        }
                        cur.seen_command_word = true;
                    }
                    cur.argv.push(rw.word);
                }
            }
        }

        if let Some(op) = expecting_after_op.take()
            && cur.is_empty()
        {
            push_unread(
                &mut scanner,
                &mut cur.unreads,
                UnreadReason::ReservedWordOrConstruct {
                    construct: op.to_string(),
                },
            )
            .map_err(Err)?;
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
                for r in cur.unreads {
                    out.outcomes.push(PartResult {
                        part: PartId::Unit(idx),
                        outcome: PartOutcome::Unread(r),
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

    // Unicode path: `char::to_lowercase()` + optional `ſ` (U+017F) -> `s` and
    // `K` (U+212A) -> `k`, with NO Unicode normalization (NFC/NFD).
    let mut out_len = 0usize;
    for chunk in base.utf8_chunks() {
        for c in chunk.valid().chars() {
            out_len += folded_char_len(c, folds);
        }
        if !chunk.invalid().is_empty() {
            out_len += chunk.invalid().len() * char::REPLACEMENT_CHARACTER.len_utf8();
        }
    }
    budget.charge(out_len as u64)?;
    budget.charge_alloc(out_len)?;

    let mut out = String::with_capacity(out_len);
    for chunk in base.utf8_chunks() {
        for c in chunk.valid().chars() {
            push_folded_char(&mut out, c, folds);
        }
        for _ in chunk.invalid() {
            out.push(char::REPLACEMENT_CHARACTER);
        }
    }
    Ok(out)
}

#[inline]
fn folded_char_len(c: char, folds: &FoldRules) -> usize {
    if c == '\u{017f}' {
        if folds.fold_long_s { 1 } else { c.len_utf8() }
    } else if c == '\u{212a}' {
        if folds.fold_kelvin { 1 } else { c.len_utf8() }
    } else {
        c.to_lowercase().map(char::len_utf8).sum()
    }
}

#[inline]
fn push_folded_char(out: &mut String, c: char, folds: &FoldRules) {
    if c == '\u{017f}' {
        if folds.fold_long_s {
            out.push('s');
        } else {
            out.push(c);
        }
    } else if c == '\u{212a}' {
        if folds.fold_kelvin {
            out.push('k');
        } else {
            out.push(c);
        }
    } else {
        for lc in c.to_lowercase() {
            out.push(lc);
        }
    }
}
