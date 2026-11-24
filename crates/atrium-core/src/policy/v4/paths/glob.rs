//! Provenance-aware shell glob expansion against the filesystem (contract §3, §5, §8).
//!
//! - Only `ByteProvenance::Unquoted` glob metacharacters (`*`, `?`, `[...]`)
//!   expand; quoted, escaped, and derived bytes stay literal (M1-32 round 4
//!   finding 1; §10 row 9).
//! - Separates matches agreed upon by the intersection of `sh`, `bash`, and
//!   `zsh` defaults (no dotglob; `**` as single-component `*`) from matches
//!   found only under a dialect variant (dot-matching, 0- or 2+-level `**`).
//! - Variant-only matches are `Unread(VariantOnlyGlobMatch)`, never hits (§1, §5).
//! - A glob that matches nothing falls back to checking the literal word (§5).

use std::collections::HashSet;

use crate::policy::limits;
use crate::policy::model::{
    AccessMode, ByteProvenance, ComputeBudget, Exhausted, FileType, FsCap, FsCaps, FsIoError,
    GlobExpansion, PartId, ResolveError, TargetOrigin, UnreadReason, Word,
};
use crate::policy::v4::paths::PathResolver;
use crate::policy::v4::paths::resolve::{
    join_bytes, resolve_literal_impl, resolve_word_literal, word_entry_path, word_spelling,
};
use crate::policy::v4::paths::sensitive::{ChargedRecord, lexical_spelling, push_charged};

impl ChargedRecord for PatComp {}
impl ChargedRecord for (Vec<u8>, usize, usize, bool) {}

/// One byte atom inside a single glob path component.
#[derive(Debug, Clone)]
enum CompAtom {
    Literal(u8),
    AnyOne,
    AnySeq,
    /// Positive byte-level character class (`[a-z0-9_]`); negated classes are
    /// rejected as `UnreadReason::NegatedGlobClass` (§3).
    Class([u64; 4]),
}

impl CompAtom {
    fn class_set(table: &mut [u64; 4], byte: u8) {
        table[(byte / 64) as usize] |= 1u64 << (byte % 64);
    }

    fn class_contains(table: &[u64; 4], byte: u8) -> bool {
        (table[(byte / 64) as usize] & (1u64 << (byte % 64))) != 0
    }
}

#[derive(Debug, Clone)]
enum PatComp {
    /// Component with no unquoted glob metacharacters.
    Literal(Vec<u8>),
    /// Component with one or more unquoted glob metacharacters.
    Glob {
        atoms: Vec<CompAtom>,
        /// `true` iff this component is literally two unquoted `*` bytes (`**`).
        is_globstar: bool,
        /// `true` iff the pattern explicitly begins with a literal `.`.
        explicit_leading_dot: bool,
    },
}

/// Parses one path component's `(bytes, provenance)` into a [`PatComp`],
/// rejecting unquoted negated character classes (`[!...]` / `[^...]`).
///
/// Linear in the component's length (§8, round-1 finding 12, round-2 finding
/// 7): the bracket structure is computed once by [`BracketIndex::build`], so
/// an unclosed `[` is decided in constant time and never rescans the
/// remainder, nested or not (`[[:digit:]` repeated, `[` repeated), and each
/// closed class body is scanned once.
fn parse_component(
    bytes: &[u8],
    prov: &[ByteProvenance],
    budget: &mut ComputeBudget,
) -> Result<Result<PatComp, UnreadReason>, Exhausted> {
    budget.charge(bytes.len().max(1) as u64)?;
    let has_unquoted_glob = bytes
        .iter()
        .zip(prov)
        .any(|(&b, &p)| p == ByteProvenance::Unquoted && matches!(b, b'*' | b'?' | b'['));
    if !has_unquoted_glob {
        budget.charge_alloc(bytes.len())?;
        return Ok(Ok(PatComp::Literal(bytes.to_vec())));
    }

    let is_globstar = bytes == b"**"
        && prov.len() == 2
        && prov[0] == ByteProvenance::Unquoted
        && prov[1] == ByteProvenance::Unquoted;

    let has_unquoted_bracket = bytes
        .iter()
        .zip(prov)
        .any(|(&b, &p)| p == ByteProvenance::Unquoted && b == b'[');
    let index = if has_unquoted_bracket {
        Some(BracketIndex::build(bytes, prov, budget)?)
    } else {
        None
    };

    budget.charge_alloc(bytes.len() * std::mem::size_of::<CompAtom>())?;
    let mut atoms: Vec<CompAtom> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        budget.charge(1)?;
        let b = bytes[i];
        let p = prov[i];
        if p != ByteProvenance::Unquoted {
            atoms.push(CompAtom::Literal(b));
            i += 1;
            continue;
        }
        match b {
            b'*' => {
                if !matches!(atoms.last(), Some(CompAtom::AnySeq)) {
                    atoms.push(CompAtom::AnySeq);
                }
                i += 1;
            }
            b'?' => {
                atoms.push(CompAtom::AnyOne);
                i += 1;
            }
            b'[' => {
                let Some(index) = index.as_ref() else {
                    atoms.push(CompAtom::Literal(b'['));
                    i += 1;
                    continue;
                };
                match parse_bracket_class(bytes, prov, i, index, budget)? {
                    BracketParse::Class(table, next_i) => {
                        atoms.push(CompAtom::Class(table));
                        i = next_i;
                    }
                    BracketParse::Unread(unread) => return Ok(Err(unread)),
                    BracketParse::Unclosed => {
                        // An unclosed `[` is a literal `[`; the next `[` is
                        // decided from the index, without rescanning (§8).
                        atoms.push(CompAtom::Literal(b'['));
                        i += 1;
                    }
                }
            }
            other => {
                atoms.push(CompAtom::Literal(other));
                i += 1;
            }
        }
    }

    let explicit_leading_dot = matches!(atoms.first(), Some(CompAtom::Literal(b'.')));
    Ok(Ok(PatComp::Glob {
        atoms,
        is_globstar,
        explicit_leading_dot,
    }))
}

/// Adds the bytes of the POSIX class `[:name:]` to `table`. Returns `false`
/// for an unknown name.
fn add_posix_class(table: &mut [u64; 4], name: &[u8]) -> bool {
    let pred: fn(u8) -> bool = match name {
        b"alnum" => |b| b.is_ascii_alphanumeric(),
        b"alpha" => |b| b.is_ascii_alphabetic(),
        b"ascii" => |b| b.is_ascii(),
        b"blank" => |b| matches!(b, b' ' | b'\t'),
        b"cntrl" => |b| b.is_ascii_control(),
        b"digit" => |b| b.is_ascii_digit(),
        b"graph" => |b| b.is_ascii_graphic(),
        b"lower" => |b| b.is_ascii_lowercase(),
        b"print" => |b| b.is_ascii_graphic() || b == b' ',
        b"punct" => |b| b.is_ascii_punctuation(),
        b"space" => |b| b.is_ascii_whitespace(),
        b"upper" => |b| b.is_ascii_uppercase(),
        b"word" => |b| b.is_ascii_alphanumeric() || b == b'_',
        b"xdigit" => |b| b.is_ascii_hexdigit(),
        _ => return false,
    };
    for b in 0u8..=127u8 {
        if pred(b) {
            CompAtom::class_set(table, b);
        }
    }
    true
}

/// Sentinel for "no position" in [`BracketIndex`].
const NO_POS: u32 = u32::MAX;

/// The bracket structure of one component, computed in two right-to-left
/// passes (§8).
struct BracketIndex {
    /// For an unquoted `[` at `p` followed by an unquoted `:`, `=` or `.`
    /// (a bracket item such as `[:alpha:]`): the position just past its
    /// matching `<delim>]`, or `NO_POS`. An item never spans an unquoted `[`,
    /// so each item's search covers the bytes up to the next `[` only, and
    /// those spans are disjoint.
    item_end: Vec<u32>,
    /// For each position `j` (and `len`): where a class-body scan starting at
    /// `j` meets its closing unquoted `]`, skipping whole bracket items, or
    /// `NO_POS`. A scan's result depends only on its position, so every scan
    /// shares this table.
    close_from: Vec<u32>,
}

impl BracketIndex {
    fn build(
        bytes: &[u8],
        prov: &[ByteProvenance],
        budget: &mut ComputeBudget,
    ) -> Result<Self, Exhausted> {
        let n = bytes.len();
        // Two passes over the component, plus each item's disjoint span.
        budget.charge((n as u64).saturating_mul(3).saturating_add(1))?;
        budget.charge_alloc((2 * n + 1).saturating_mul(std::mem::size_of::<u32>()))?;
        let unquoted = |k: usize, b: u8| bytes[k] == b && prov[k] == ByteProvenance::Unquoted;
        let to_pos = |k: usize| u32::try_from(k).unwrap_or(NO_POS);

        let mut item_end = vec![NO_POS; n];
        let mut next_bracket = n; // the nearest unquoted `[` after the current position
        for p in (0..n).rev() {
            if unquoted(p, b'[')
                && p + 3 < n
                && prov[p + 1] == ByteProvenance::Unquoted
                && matches!(bytes[p + 1], b':' | b'=' | b'.')
            {
                let delim = bytes[p + 1];
                let mut s = p + 2;
                while s + 1 < n && s < next_bracket {
                    if unquoted(s, delim) && unquoted(s + 1, b']') {
                        item_end[p] = to_pos(s + 2);
                        break;
                    }
                    s += 1;
                }
            }
            if unquoted(p, b'[') {
                next_bracket = p;
            }
        }

        let mut close_from = vec![NO_POS; n + 1];
        for j in (0..n).rev() {
            close_from[j] = if unquoted(j, b']') {
                to_pos(j)
            } else if unquoted(j, b'[') && item_end[j] != NO_POS {
                close_from[item_end[j] as usize]
            } else {
                close_from[j + 1]
            };
        }
        Ok(Self {
            item_end,
            close_from,
        })
    }

    fn item_end(&self, k: usize) -> Option<usize> {
        self.item_end
            .get(k)
            .filter(|&&e| e != NO_POS)
            .map(|&e| e as usize)
    }
}

enum BracketParse {
    Class([u64; 4], usize),
    Unread(UnreadReason),
    Unclosed,
}

/// Parses the unquoted `[...]` class starting at `start` using `index`, in
/// time linear in the class body (which the caller then skips).
fn parse_bracket_class(
    bytes: &[u8],
    prov: &[ByteProvenance],
    start: usize,
    index: &BracketIndex,
    budget: &mut ComputeBudget,
) -> Result<BracketParse, Exhausted> {
    let mut j = start + 1;
    if j >= bytes.len() {
        return Ok(BracketParse::Unclosed);
    }
    let body_start = j;
    let mut has_bang_or_caret = false;
    if prov[j] == ByteProvenance::Unquoted && matches!(bytes[j], b'!' | b'^') {
        has_bang_or_caret = true;
        j += 1;
    }
    if j < bytes.len() && bytes[j] == b']' {
        j += 1;
    }
    let close = index.close_from[j];
    if close == NO_POS {
        return Ok(BracketParse::Unclosed);
    }
    let end = close as usize;

    // One pass over the body (bodies of closed classes are disjoint).
    let mut k = j;
    while k < end {
        budget.charge(1)?;
        if bytes[k] == b'['
            && prov[k] == ByteProvenance::Unquoted
            && let Some(item_end) = index.item_end(k)
            && item_end <= end
        {
            k = item_end;
            continue;
        }
        if prov[k] == ByteProvenance::Unquoted && matches!(bytes[k], b'!' | b'^') {
            has_bang_or_caret = true;
        }
        k += 1;
    }
    if has_bang_or_caret {
        return Ok(BracketParse::Unread(UnreadReason::NegatedGlobClass));
    }

    let mut table = [0u64; 4];
    let mut k = body_start;
    while k < end {
        budget.charge(1)?;
        if bytes[k] == b'['
            && prov[k] == ByteProvenance::Unquoted
            && let Some(item_end) = index.item_end(k)
            && item_end <= end
        {
            let delim = bytes[k + 1];
            let inner = &bytes[k + 2..item_end - 2];
            if delim == b':' && add_posix_class(&mut table, inner) {
                k = item_end;
                continue;
            }
            budget.charge_alloc(end + 1 - start)?;
            return Ok(BracketParse::Unread(UnreadReason::VariantOnlyGlobMatch {
                pattern: bytes[start..=end].to_vec(),
            }));
        }
        let c = bytes[k];
        if k + 2 < end && bytes[k + 1] == b'-' && prov[k + 1] == ByteProvenance::Unquoted {
            let hi = bytes[k + 2];
            if c <= hi {
                budget.charge(u64::from(hi - c) + 1)?;
                for ch in c..=hi {
                    CompAtom::class_set(&mut table, ch);
                }
            }
            k += 3;
        } else {
            CompAtom::class_set(&mut table, c);
            k += 1;
        }
    }
    Ok(BracketParse::Class(table, end + 1))
}

/// Linear/budgeted glob matcher for one path component against one directory
/// entry name (DP with two rolling rows, O(|atoms| * |name|) worst case,
/// charged before execution).
fn match_component(
    atoms: &[CompAtom],
    name: &[u8],
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    let m = atoms.len();
    let n = name.len();
    let steps = ((m + 1) as u64).saturating_mul((n + 1) as u64).max(1);
    budget.charge(steps)?;

    // Fast path for single `*`.
    if m == 1 && matches!(atoms[0], CompAtom::AnySeq) {
        return Ok(true);
    }

    budget.charge_alloc((n + 1) * 2)?;
    let mut prev = vec![false; n + 1];
    let mut next = vec![false; n + 1];
    prev[0] = true;

    for atom in atoms {
        next.fill(false);
        match atom {
            CompAtom::AnySeq => {
                let mut seen = false;
                for j in 0..=n {
                    if prev[j] {
                        seen = true;
                    }
                    next[j] = seen;
                }
            }
            CompAtom::AnyOne => {
                for j in 0..n {
                    if prev[j] {
                        next[j + 1] = true;
                    }
                }
            }
            CompAtom::Literal(lit) => {
                for j in 0..n {
                    if prev[j] && name[j] == *lit {
                        next[j + 1] = true;
                    }
                }
            }
            CompAtom::Class(table) => {
                for j in 0..n {
                    if prev[j] && CompAtom::class_contains(table, name[j]) {
                        next[j + 1] = true;
                    }
                }
            }
        }
        std::mem::swap(&mut prev, &mut next);
        if !prev.iter().any(|&b| b) {
            return Ok(false);
        }
    }
    Ok(prev[n])
}

/// Implementation of `v4::paths::expand_shell_glob` (contract §3, §5, §8).
#[allow(clippy::too_many_arguments)]
pub(crate) fn expand_shell_glob_impl(
    pattern: &Word,
    cwd: &[u8],
    home: &[u8],
    source: PartId,
    access: AccessMode,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
) -> Result<GlobExpansion, Exhausted> {
    expand_shell_glob_written(
        pattern, cwd, cwd, home, source, access, resolver, caps, budget,
    )
    .map(|(expansion, _)| expansion)
}

/// A glob expansion with the entry path of each exact match.
pub(crate) type WrittenExpansion = (GlobExpansion, Vec<Option<Vec<u8>>>);

/// [`expand_shell_glob_impl`] on a written working directory `written_cwd`:
/// each exact match's `Target::written` is the matched path as spelled from
/// the pattern (on `written_cwd` for a relative pattern), so its protected
/// names are checked as on `dev` (D-147, D-149); variant-only matches get no
/// written spelling, since they are never hits. Also returns, aligned with
/// `exact_matches`, each match's actual entry path (the expanded entry, not
/// followed), for checks of the entry's own type (review 5, finding 4).
#[allow(clippy::too_many_arguments)]
pub(crate) fn expand_shell_glob_written(
    pattern: &Word,
    cwd: &[u8],
    written_cwd: &[u8],
    home: &[u8],
    source: PartId,
    access: AccessMode,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
) -> Result<WrittenExpansion, Exhausted> {
    let raw = pattern.literal_bytes();
    let prov = pattern.provenance();
    budget.charge((raw.len() + cwd.len() + home.len() + 1) as u64)?;

    let mut expansion = GlobExpansion::default();
    let mut entries: Vec<Option<Vec<u8>>> = Vec::new();
    let relative = !raw.starts_with(b"/") && !pattern.starts_with_unquoted_tilde();
    let cwd_comps = cwd.split(|&b| b == b'/').filter(|c| !c.is_empty()).count();

    if raw.is_empty() || raw.contains(&0) {
        budget.charge_alloc(raw.len() + std::mem::size_of::<UnreadReason>())?;
        expansion
            .unread
            .push(UnreadReason::UnresolvablePath { path: raw.to_vec() });
        return Ok((expansion, entries));
    }

    // No unquoted glob characters at all: resolve as a literal path (§3, §10 row 9).
    if !pattern.has_unquoted_glob() {
        let t = resolve_word_literal(
            pattern,
            cwd,
            home,
            TargetOrigin::ExpandedFrom {
                source,
                variant_only: false,
            },
            access,
            resolver,
            caps,
            budget,
        )?;
        let mut t = t;
        t.written = word_spelling(pattern, written_cwd, home, budget)?;
        expansion.exact_matches.push(t);
        let e = word_entry_path(pattern, cwd, home, budget)?;
        push_charged(&mut entries, e, budget)?;
        return Ok((expansion, entries));
    }

    // Build an absolute `(bytes, provenance)` stream where expanded `home` and
    // `cwd` bytes are marked `Derived` so their characters never act as globs.
    let abs_cap = cwd.len().max(home.len()) + raw.len() + 2;
    budget.charge_alloc(abs_cap * 2)?;
    let mut abs_bytes: Vec<u8> = Vec::with_capacity(abs_cap);
    let mut abs_prov: Vec<ByteProvenance> = Vec::with_capacity(abs_cap);

    if pattern.starts_with_unquoted_tilde() {
        if raw == b"~" {
            abs_bytes.extend_from_slice(home);
            abs_prov.extend(std::iter::repeat_n(ByteProvenance::Derived, home.len()));
        } else if let Some(rest) = raw.strip_prefix(b"~/") {
            let rest_prov = &prov[2..];
            abs_bytes.extend_from_slice(home);
            abs_prov.extend(std::iter::repeat_n(ByteProvenance::Derived, home.len()));
            if abs_bytes.last() != Some(&b'/') {
                abs_bytes.push(b'/');
                abs_prov.push(ByteProvenance::Derived);
            }
            abs_bytes.extend_from_slice(rest);
            abs_prov.extend_from_slice(rest_prov);
        } else {
            push_charged(
                &mut expansion.unread,
                UnreadReason::UnsupportedTildeForm,
                budget,
            )?;
            return Ok((expansion, entries));
        }
    } else if raw.starts_with(b"/") {
        abs_bytes.extend_from_slice(raw);
        abs_prov.extend_from_slice(prov);
    } else {
        abs_bytes.extend_from_slice(cwd);
        abs_prov.extend(std::iter::repeat_n(ByteProvenance::Derived, cwd.len()));
        if abs_bytes.last() != Some(&b'/') {
            abs_bytes.push(b'/');
            abs_prov.push(ByteProvenance::Derived);
        }
        abs_bytes.extend_from_slice(raw);
        abs_prov.extend_from_slice(prov);
    }

    let trailing_slash = abs_bytes.len() > 1 && abs_bytes.last() == Some(&b'/');

    // Split into components separated by `/`.
    let mut comps: Vec<PatComp> = Vec::new();
    let mut start = 0usize;
    while start <= abs_bytes.len() {
        let end = abs_bytes[start..]
            .iter()
            .position(|&b| b == b'/')
            .map_or(abs_bytes.len(), |off| start + off);
        if end > start {
            match parse_component(&abs_bytes[start..end], &abs_prov[start..end], budget)? {
                Ok(comp) => push_charged(&mut comps, comp, budget)?,
                Err(unread) => {
                    push_charged(&mut expansion.unread, unread, budget)?;
                    return Ok((expansion, entries));
                }
            }
        }
        if end == abs_bytes.len() {
            break;
        }
        start = end + 1;
    }

    // Walk leading literal prefix without `read_dir` calls.
    let first_glob = comps
        .iter()
        .position(|c| matches!(c, PatComp::Glob { .. }))
        .unwrap_or(comps.len());

    budget.charge_alloc(abs_bytes.len().max(1))?;
    let mut prefix = Vec::new();
    if first_glob == 0 {
        prefix.push(b'/');
    } else {
        for c in &comps[..first_glob] {
            if let PatComp::Literal(seg) = c {
                prefix.push(b'/');
                prefix.extend_from_slice(seg);
            }
        }
    }

    let mut exact_paths: Vec<Vec<u8>> = Vec::new();
    let mut exact_seen: HashSet<Vec<u8>> = HashSet::new();
    let mut variant_paths: Vec<Vec<u8>> = Vec::new();
    let mut variant_seen: HashSet<Vec<u8>> = HashSet::new();

    // DFS stack: `(current_path, comp_idx, glob_depth, variant_only)`.
    let mut stack: Vec<(Vec<u8>, usize, usize, bool)> = vec![(prefix, first_glob, 0, false)];
    let mut visited: HashSet<(Vec<u8>, usize, bool)> = HashSet::new();
    let mut globstar_symlink_dirs: HashSet<Vec<u8>> = HashSet::new();
    let mut entries_seen: usize = 0;

    while let Some((dir, idx, depth, variant_only)) = stack.pop() {
        budget.charge(dir.len().max(1) as u64)?;
        budget.charge_alloc(dir.len())?;
        budget.charge_alloc(4 * std::mem::size_of::<(Vec<u8>, usize, bool)>())?;
        if !visited.insert((dir.clone(), idx, variant_only)) {
            continue;
        }
        if idx == comps.len() {
            if trailing_slash {
                match resolver.stat(&dir, caps, budget) {
                    Ok(meta) if meta.file_type == FileType::Dir => {}
                    Ok(_) => continue,
                    Err(ResolveError::NotFound | ResolveError::Io(FsIoError::NotADirectory)) => {
                        continue;
                    }
                    Err(ResolveError::Io(_)) => {
                        push_unique_unread(
                            &mut expansion.unread,
                            UnreadReason::UnresolvablePath { path: dir },
                            budget,
                        )?;
                        continue;
                    }
                    Err(ResolveError::CapReached(c)) => {
                        push_unique_unread(&mut expansion.unread, UnreadReason::from(c), budget)?;
                        break;
                    }
                    Err(ResolveError::Exhausted(e)) => return Err(e),
                }
            }
            if variant_only {
                if !variant_seen.contains(&dir) {
                    budget.charge_alloc(dir.len().saturating_mul(2))?;
                    variant_seen.insert(dir.clone());
                    push_charged(&mut variant_paths, dir, budget)?;
                }
            } else if !exact_seen.contains(&dir) {
                budget.charge_alloc(dir.len().saturating_mul(2))?;
                exact_seen.insert(dir.clone());
                push_charged(&mut exact_paths, dir, budget)?;
            }
            continue;
        }

        match &comps[idx] {
            PatComp::Literal(seg) => {
                if seg == b"." || seg == b".." {
                    // POSIX globbing requires `dir` to be an existing directory for `.` / `..`.
                    match resolver.stat(&dir, caps, budget) {
                        Ok(meta) if meta.file_type == FileType::Dir => {}
                        Ok(_)
                        | Err(
                            ResolveError::NotFound | ResolveError::Io(FsIoError::NotADirectory),
                        ) => {
                            continue;
                        }
                        Err(ResolveError::Io(_)) => {
                            push_unique_unread(
                                &mut expansion.unread,
                                UnreadReason::UnresolvablePath { path: dir },
                                budget,
                            )?;
                            continue;
                        }
                        Err(ResolveError::CapReached(c)) => {
                            push_unique_unread(
                                &mut expansion.unread,
                                UnreadReason::from(c),
                                budget,
                            )?;
                            break;
                        }
                        Err(ResolveError::Exhausted(e)) => return Err(e),
                    }
                    if seg == b"." {
                        push_charged(&mut stack, (dir, idx + 1, depth, variant_only), budget)?;
                    } else {
                        budget.charge_alloc(dir.len() + 3)?;
                        let next = join_bytes(&dir, b"..");
                        push_charged(&mut stack, (next, idx + 1, depth, variant_only), budget)?;
                    }
                    continue;
                }
                budget.charge_alloc(dir.len() + seg.len() + 1)?;
                let next = join_bytes(&dir, seg);
                match resolver.lstat(&next, caps, budget) {
                    Ok(_) => {
                        push_charged(&mut stack, (next, idx + 1, depth, variant_only), budget)?;
                    }
                    Err(ResolveError::NotFound | ResolveError::Io(FsIoError::NotADirectory)) => {}
                    Err(ResolveError::Io(_)) => {
                        push_unique_unread(
                            &mut expansion.unread,
                            UnreadReason::UnresolvablePath { path: next },
                            budget,
                        )?;
                    }
                    Err(ResolveError::CapReached(c)) => {
                        push_unique_unread(&mut expansion.unread, UnreadReason::from(c), budget)?;
                        break;
                    }
                    Err(ResolveError::Exhausted(e)) => return Err(e),
                }
            }
            PatComp::Glob {
                atoms,
                is_globstar,
                explicit_leading_dot,
            } => {
                if depth >= limits::GLOB_MAX_DEPTH {
                    push_unique_unread(
                        &mut expansion.unread,
                        UnreadReason::WalkCapReached(FsCap::WalkDepth),
                        budget,
                    )?;
                    continue;
                }
                if *is_globstar {
                    // 0-level match (`a/**/b` -> `a/b`) is variant-only (zsh/bash globstar).
                    budget.charge_alloc(dir.len())?;
                    push_charged(&mut stack, (dir.clone(), idx + 1, depth, true), budget)?;
                }

                let entries = match resolver.read_dir(&dir, caps, budget) {
                    Ok(e) => e,
                    Err(ResolveError::NotFound | ResolveError::Io(FsIoError::NotADirectory)) => {
                        continue;
                    }
                    Err(ResolveError::Io(_)) => {
                        push_unique_unread(
                            &mut expansion.unread,
                            UnreadReason::UnresolvablePath { path: dir },
                            budget,
                        )?;
                        continue;
                    }
                    Err(ResolveError::CapReached(c)) => {
                        push_unique_unread(&mut expansion.unread, UnreadReason::from(c), budget)?;
                        break;
                    }
                    Err(ResolveError::Exhausted(e)) => return Err(e),
                };

                let mut hit_cap = false;
                for entry in entries {
                    if entry.name == b"." || entry.name == b".." {
                        continue;
                    }
                    entries_seen += 1;
                    if entries_seen > limits::WALK_MAX_ENTRIES {
                        push_unique_unread(
                            &mut expansion.unread,
                            UnreadReason::WalkCapReached(FsCap::EntriesRead),
                            budget,
                        )?;
                        hit_cap = true;
                        break;
                    }
                    let is_dot = entry.name.starts_with(b".");
                    if *is_globstar {
                        // 1-level match agrees across sh, bash, zsh defaults when not a dotfile.
                        let next_len = dir.len() + entry.name.len() + 1;
                        budget.charge_alloc(next_len.saturating_mul(2))?;
                        let next = join_bytes(&dir, &entry.name);
                        let one_level_variant = variant_only || is_dot;
                        push_charged(
                            &mut stack,
                            (next.clone(), idx + 1, depth + 1, one_level_variant),
                            budget,
                        )?;
                        // 2+-level descent is variant-only (including symlinked dirs for bash 4 globstar).
                        let descend_dir = match entry.file_type {
                            FileType::Dir => true,
                            FileType::Symlink => match resolver.stat(&next, caps, budget) {
                                Ok(md) if md.file_type == FileType::Dir => {
                                    match resolver.canonicalize(&next, caps, budget) {
                                        Ok(canon) => globstar_symlink_dirs.insert(canon),
                                        Err(ResolveError::Exhausted(e)) => return Err(e),
                                        Err(ResolveError::CapReached(c)) => {
                                            push_unique_unread(
                                                &mut expansion.unread,
                                                UnreadReason::from(c),
                                                budget,
                                            )?;
                                            hit_cap = true;
                                            false
                                        }
                                        Err(ResolveError::NotFound) => false,
                                        Err(_) => {
                                            budget.charge_alloc(next.len())?;
                                            push_unique_unread(
                                                &mut expansion.unread,
                                                UnreadReason::UnresolvablePath {
                                                    path: next.clone(),
                                                },
                                                budget,
                                            )?;
                                            false
                                        }
                                    }
                                }
                                Ok(_) | Err(ResolveError::NotFound) => false,
                                Err(ResolveError::Io(_)) => {
                                    budget.charge_alloc(next.len())?;
                                    push_unique_unread(
                                        &mut expansion.unread,
                                        UnreadReason::UnresolvablePath { path: next.clone() },
                                        budget,
                                    )?;
                                    false
                                }
                                Err(ResolveError::CapReached(c)) => {
                                    push_unique_unread(
                                        &mut expansion.unread,
                                        UnreadReason::from(c),
                                        budget,
                                    )?;
                                    hit_cap = true;
                                    false
                                }
                                Err(ResolveError::Exhausted(e)) => return Err(e),
                            },
                            FileType::File | FileType::Other => false,
                        };
                        if hit_cap {
                            break;
                        }
                        if descend_dir {
                            if depth + 1 >= limits::GLOB_MAX_DEPTH {
                                push_unique_unread(
                                    &mut expansion.unread,
                                    UnreadReason::WalkCapReached(FsCap::WalkDepth),
                                    budget,
                                )?;
                            } else {
                                push_charged(&mut stack, (next, idx, depth + 1, true), budget)?;
                            }
                        }
                    } else if match_component(atoms, &entry.name, budget)? {
                        budget.charge_alloc(dir.len() + entry.name.len() + 1)?;
                        let next = join_bytes(&dir, &entry.name);
                        let entry_variant = variant_only || (is_dot && !explicit_leading_dot);
                        push_charged(
                            &mut stack,
                            (next, idx + 1, depth + 1, entry_variant),
                            budget,
                        )?;
                    }
                }
                if hit_cap {
                    break;
                }
            }
        }
    }

    // Deduplicate: anything matched in the intersection is NOT variant-only.
    variant_paths.retain(|p| !exact_seen.contains(p));
    exact_paths.sort();
    variant_paths.sort();

    // No-match fallback (§5): when the sh/bash/zsh intersection matched nothing
    // (and no cap/I/O unread prevented expansion), check the literal word too —
    // even if a dialect variant matched dotfiles or recursive `**`, because
    // default sh/bash passes the unexpanded word literally.
    if exact_paths.is_empty() && expansion.unread.is_empty() {
        let fallback = resolve_word_literal(
            pattern,
            cwd,
            home,
            TargetOrigin::ExpandedFrom {
                source: source.clone(),
                variant_only: false,
            },
            access,
            resolver,
            caps,
            budget,
        )?;
        let mut fallback = fallback;
        fallback.written = word_spelling(pattern, written_cwd, home, budget)?;
        expansion.exact_matches.push(fallback);
        let e = word_entry_path(pattern, cwd, home, budget)?;
        push_charged(&mut entries, e, budget)?;
    }

    for matched in exact_paths {
        let target = resolve_literal_impl(
            &matched,
            cwd,
            home,
            TargetOrigin::ExpandedFrom {
                source: source.clone(),
                variant_only: false,
            },
            access,
            resolver,
            caps,
            budget,
        )?;
        let mut target = target;
        target.written = if relative {
            let rest: Vec<&[u8]> = matched
                .split(|&b| b == b'/')
                .filter(|c| !c.is_empty())
                .skip(cwd_comps)
                .collect();
            budget.charge(matched.len() as u64 + 1)?;
            budget.charge_alloc(matched.len() + rest.len() * std::mem::size_of::<&[u8]>())?;
            let rel = rest.join(&b'/');
            lexical_spelling(&rel, written_cwd, home, budget)?
        } else {
            lexical_spelling(&matched, b"/", home, budget)?
        };
        expansion.exact_matches.push(target);
        budget.charge_alloc(matched.len())?;
        push_charged(&mut entries, Some(matched), budget)?;
    }

    for matched in variant_paths {
        let target = resolve_literal_impl(
            &matched,
            cwd,
            home,
            TargetOrigin::ExpandedFrom {
                source: source.clone(),
                variant_only: true,
            },
            access,
            resolver,
            caps,
            budget,
        )?;
        let mut target = target;
        target.written = None;
        expansion.variant_only_matches.push(target);
    }

    Ok((expansion, entries))
}

fn push_unique_unread(
    out: &mut Vec<UnreadReason>,
    reason: UnreadReason,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    budget.charge(out.len() as u64 + 1)?;
    if !out.contains(&reason) {
        push_charged(out, reason, budget)?;
    }
    Ok(())
}
