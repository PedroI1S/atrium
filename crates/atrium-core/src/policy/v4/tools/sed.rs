//! Linear, budgeted `sed` script parser for §6's `ConditionalRunner` trigger
//! (`e`, `w`, `r`, plus GNU `W`, `R`, and `s/.../.../e|w` flags).
//!
//! Recognises script commands without misreading pattern or replacement text
//! (`s/error/warn/g` is `Safe`, not triggered by `e`/`w`/`r`). Any construct
//! that cannot be parsed reliably returns [`SedParseOutcome::Uncertain`],
//! which callers treat as `Unread` (§6).

use crate::policy::model::{ComputeBudget, Exhausted};

/// Outcome of inspecting one `sed` script string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SedParseOutcome {
    /// Script parsed completely and contains no trigger command or flag.
    Safe,
    /// Script contains a trigger command (`e`, `w`, `r`, `W`, `R`) or
    /// substitution flag (`s/.../.../e`, `s/.../.../w`).
    Triggered,
    /// Script contains syntax outside our reliable subset (unclosed delimiter,
    /// unrecognized command, unbalanced `{}`), so the unit must be `Unread`.
    Uncertain,
}

/// Inspects `script` in a single linear pass, charging `ComputeBudget` before
/// scanning bytes.
pub(crate) fn inspect_sed_script(
    script: &str,
    trigger_cmds: &[char],
    budget: &mut ComputeBudget,
) -> Result<SedParseOutcome, Exhausted> {
    let bytes = script.as_bytes();
    budget.charge((bytes.len().max(1) as u64).saturating_mul(2))?;

    let mut i = 0usize;
    let mut brace_depth = 0usize;

    while i < bytes.len() {
        // Skip whitespace and statement separators.
        while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n' | b';') {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }

        // Comment `#` runs to end of line.
        if bytes[i] == b'#' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // Closing block brace `}`.
        if bytes[i] == b'}' {
            if brace_depth == 0 {
                return Ok(SedParseOutcome::Uncertain);
            }
            brace_depth -= 1;
            i += 1;
            continue;
        }

        // Optional address (or address1,address2 range).
        match skip_address(bytes, i) {
            AddrRes::None => {}
            AddrRes::Matched(next) => {
                i = next;
                while i < bytes.len() && matches!(bytes[i], b' ' | b'\t') {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b',' {
                    i += 1;
                    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t') {
                        i += 1;
                    }
                    match skip_address(bytes, i) {
                        AddrRes::Matched(next2) => i = next2,
                        AddrRes::None | AddrRes::Uncertain => {
                            return Ok(SedParseOutcome::Uncertain);
                        }
                    }
                }
            }
            AddrRes::Uncertain => return Ok(SedParseOutcome::Uncertain),
        }

        // Skip whitespace and optional `!` negation modifier(s).
        while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'!') {
            i += 1;
        }
        if i >= bytes.len() {
            return Ok(SedParseOutcome::Uncertain);
        }

        let cmd = bytes[i] as char;
        i += 1;

        if cmd == '{' {
            brace_depth += 1;
            continue;
        }

        // Check listed trigger commands plus GNU file-read/write variants `W`/`R`.
        if trigger_cmds.contains(&cmd) || matches!(cmd, 'W' | 'R') {
            return Ok(SedParseOutcome::Triggered);
        }

        match cmd {
            's' => {
                // `s<delim><pattern><delim><replacement><delim><flags>`
                if i >= bytes.len() {
                    return Ok(SedParseOutcome::Uncertain);
                }
                let delim = bytes[i];
                if delim == b'\n' || delim == b'\\' {
                    return Ok(SedParseOutcome::Uncertain);
                }
                i += 1;
                // Pattern (respects `[...]` character classes).
                let Some(after_pat) = scan_regex_to_delim(bytes, i, delim) else {
                    return Ok(SedParseOutcome::Uncertain);
                };
                i = after_pat;
                // Replacement (escaped delim and `\<newline>` stay inside replacement).
                let Some(after_rep) = scan_replacement_to_delim(bytes, i, delim) else {
                    return Ok(SedParseOutcome::Uncertain);
                };
                i = after_rep;
                // Flags up to separator, `}`, `#`, or EOF.
                while i < bytes.len() {
                    let b = bytes[i];
                    if matches!(b, b';' | b'\n' | b'}' | b'#') {
                        break;
                    }
                    if matches!(b, b' ' | b'\t' | b'\r') {
                        i += 1;
                        continue;
                    }
                    let fc = b as char;
                    if fc == 'e' || fc == 'w' || fc == 'W' || trigger_cmds.contains(&fc) {
                        return Ok(SedParseOutcome::Triggered);
                    }
                    if matches!(b, b'g' | b'p' | b'i' | b'I' | b'm' | b'M' | b'0'..=b'9') {
                        i += 1;
                    } else {
                        return Ok(SedParseOutcome::Uncertain);
                    }
                }
            }
            'y' => {
                // `y<delim><src><delim><dst><delim>`
                if i >= bytes.len() {
                    return Ok(SedParseOutcome::Uncertain);
                }
                let delim = bytes[i];
                if delim == b'\n' || delim == b'\\' {
                    return Ok(SedParseOutcome::Uncertain);
                }
                i += 1;
                let Some(after_src) = scan_replacement_to_delim(bytes, i, delim) else {
                    return Ok(SedParseOutcome::Uncertain);
                };
                i = after_src;
                let Some(after_dst) = scan_replacement_to_delim(bytes, i, delim) else {
                    return Ok(SedParseOutcome::Uncertain);
                };
                i = after_dst;
            }
            // Zero-argument safe commands
            'd' | 'D' | 'g' | 'G' | 'h' | 'H' | 'n' | 'N' | 'p' | 'P' | 'x' | '=' | 'z' | 'F' => {
                while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\r') {
                    i += 1;
                }
                if i < bytes.len() && !matches!(bytes[i], b';' | b'\n' | b'}' | b'#') {
                    return Ok(SedParseOutcome::Uncertain);
                }
            }
            // Commands with optional numeric argument (`q [exit]`, `Q [exit]`, `l [width]`)
            'q' | 'Q' | 'l' => {
                while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\r') {
                    i += 1;
                }
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\r') {
                    i += 1;
                }
                if i < bytes.len() && !matches!(bytes[i], b';' | b'\n' | b'}' | b'#') {
                    return Ok(SedParseOutcome::Uncertain);
                }
            }
            // Branch / label commands: label name runs to separator / `}` / `#` / EOL
            ':' | 'b' | 't' | 'T' => {
                while i < bytes.len() && !matches!(bytes[i], b';' | b'\n' | b'}' | b'#') {
                    i += 1;
                }
            }
            // Text insertion commands (`a`, `i`, `c`): text runs to unescaped newline
            'a' | 'i' | 'c' => {
                while i < bytes.len() && matches!(bytes[i], b' ' | b'\t') {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b'\\' {
                    i += 1;
                    if i < bytes.len() && bytes[i] == b'\n' {
                        i += 1;
                    }
                }
                // Consume lines until an unescaped newline or EOF.
                while i < bytes.len() {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'\n' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            _ => return Ok(SedParseOutcome::Uncertain),
        }
    }

    if brace_depth != 0 {
        return Ok(SedParseOutcome::Uncertain);
    }

    Ok(SedParseOutcome::Safe)
}

enum AddrRes {
    None,
    Matched(usize),
    Uncertain,
}

fn skip_address(bytes: &[u8], start: usize) -> AddrRes {
    if start >= bytes.len() {
        return AddrRes::None;
    }
    match bytes[start] {
        b'$' => AddrRes::Matched(start + 1),
        b'0'..=b'9' => {
            let mut i = start;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            // Optional GNU step `~N` or relative offset `+N`.
            if i < bytes.len() && matches!(bytes[i], b'~' | b'+') {
                let sep = i;
                i += 1;
                let digits_start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i == digits_start {
                    let _ = sep;
                    return AddrRes::Uncertain;
                }
            }
            AddrRes::Matched(i)
        }
        b'+' => {
            // Relative second address `+N` or `~N`
            let mut i = start + 1;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i == start + 1 {
                AddrRes::Uncertain
            } else {
                AddrRes::Matched(i)
            }
        }
        b'~' => {
            let mut i = start + 1;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i == start + 1 {
                AddrRes::Uncertain
            } else {
                AddrRes::Matched(i)
            }
        }
        b'/' => match scan_regex_to_delim(bytes, start + 1, b'/') {
            Some(mut next) => {
                // Optional GNU regex address flags `I` / `M`.
                while next < bytes.len() && matches!(bytes[next], b'I' | b'M') {
                    next += 1;
                }
                AddrRes::Matched(next)
            }
            None => AddrRes::Uncertain,
        },
        b'\\' => {
            if start + 1 >= bytes.len() {
                return AddrRes::Uncertain;
            }
            let delim = bytes[start + 1];
            if delim == b'\n' || delim == b'\\' {
                return AddrRes::Uncertain;
            }
            match scan_regex_to_delim(bytes, start + 2, delim) {
                Some(mut next) => {
                    while next < bytes.len() && matches!(bytes[next], b'I' | b'M') {
                        next += 1;
                    }
                    AddrRes::Matched(next)
                }
                None => AddrRes::Uncertain,
            }
        }
        _ => AddrRes::None,
    }
}

/// Scans a sed regex up to an unescaped `delim` outside `[...]`, returning the
/// index immediately after `delim`, or `None` if unclosed.
fn scan_regex_to_delim(bytes: &[u8], mut i: usize, delim: u8) -> Option<usize> {
    let mut in_class = false;
    let mut class_first = false;
    while i < bytes.len() {
        let b = bytes[i];
        if in_class {
            if b == b'[' && i + 1 < bytes.len() && matches!(bytes[i + 1], b':' | b'.' | b'=') {
                // POSIX bracket element `[:...:]`, `[....]`, `[=...=]`
                let marker = bytes[i + 1];
                i += 2;
                let mut closed = false;
                while i + 1 < bytes.len() {
                    if bytes[i] == marker && bytes[i + 1] == b']' {
                        i += 2;
                        closed = true;
                        break;
                    }
                    i += 1;
                }
                if !closed {
                    return None;
                }
                class_first = false;
                continue;
            }
            if b == b']' && !class_first {
                in_class = false;
                i += 1;
                continue;
            }
            if b == b'^' && class_first {
                // Negated class still treats immediately following `]` as literal.
                i += 1;
                continue;
            }
            class_first = false;
            i += 1;
            continue;
        }
        if b == b'\\' {
            if i + 1 >= bytes.len() {
                return None;
            }
            i += 2;
            continue;
        }
        if b == delim {
            return Some(i + 1);
        }
        if b == b'\n' {
            return None;
        }
        if b == b'[' {
            in_class = true;
            class_first = true;
            i += 1;
            continue;
        }
        i += 1;
    }
    None
}

/// Scans a sed replacement or `y` string up to an unescaped `delim`, returning
/// the index immediately after `delim`, or `None` if unclosed.
fn scan_replacement_to_delim(bytes: &[u8], mut i: usize, delim: u8) -> Option<usize> {
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' {
            if i + 1 >= bytes.len() {
                return None;
            }
            i += 2;
            continue;
        }
        if b == delim {
            return Some(i + 1);
        }
        if b == b'\n' {
            return None;
        }
        i += 1;
    }
    None
}
