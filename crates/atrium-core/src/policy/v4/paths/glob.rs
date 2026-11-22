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
use crate::policy::v4::paths::resolve::{join_bytes, resolve_literal_impl, resolve_word_literal};

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

    let mut atoms: Vec<CompAtom> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
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
                if let Some((class_res, next_i)) = parse_bracket_class(bytes, prov, i, budget)? {
                    match class_res {
                        Ok(table) => {
                            atoms.push(CompAtom::Class(table));
                            i = next_i;
                        }
                        Err(unread) => return Ok(Err(unread)),
                    }
                } else {
                    // Unclosed `[` is treated as a literal `[`.
                    atoms.push(CompAtom::Literal(b'['));
                    i += 1;
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

/// Parses an unquoted `[...]` class starting at `start`. Returns `Ok(None)` if
/// no matching unquoted `]` closes the class.
fn parse_bracket_class(
    bytes: &[u8],
    prov: &[ByteProvenance],
    start: usize,
    budget: &mut ComputeBudget,
) -> Result<Option<(Result<[u64; 4], UnreadReason>, usize)>, Exhausted> {
    let mut j = start + 1;
    if j >= bytes.len() {
        return Ok(None);
    }
    if prov[j] == ByteProvenance::Unquoted && matches!(bytes[j], b'!' | b'^') {
        // Check if this class actually closes; if so it's a negated class (§3).
        let mut scan = j + 1;
        if scan < bytes.len() && bytes[scan] == b']' {
            scan += 1;
        }
        while scan < bytes.len() {
            budget.charge(1)?;
            if bytes[scan] == b']' && prov[scan] == ByteProvenance::Unquoted {
                return Ok(Some((Err(UnreadReason::NegatedGlobClass), scan + 1)));
            }
            scan += 1;
        }
        return Ok(None);
    }

    // Find closing unquoted `]` (a leading `]` right after `[` is literal).
    let body_start = j;
    if j < bytes.len() && bytes[j] == b']' {
        j += 1;
    }
    let mut close = None;
    while j < bytes.len() {
        budget.charge(1)?;
        if bytes[j] == b']' && prov[j] == ByteProvenance::Unquoted {
            close = Some(j);
            break;
        }
        j += 1;
    }
    let Some(end) = close else {
        return Ok(None);
    };

    let mut table = [0u64; 4];
    let mut k = body_start;
    while k < end {
        budget.charge(1)?;
        let c = bytes[k];
        if k + 2 < end && bytes[k + 1] == b'-' && prov[k + 1] == ByteProvenance::Unquoted {
            let hi = bytes[k + 2];
            if c <= hi {
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
    Ok(Some((Ok(table), end + 1)))
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
    let raw = pattern.literal_bytes();
    let prov = pattern.provenance();
    budget.charge((raw.len() + cwd.len() + home.len() + 1) as u64)?;

    let mut expansion = GlobExpansion::default();

    if raw.is_empty() || raw.contains(&0) {
        expansion
            .unread
            .push(UnreadReason::UnresolvablePath { path: raw.to_vec() });
        return Ok(expansion);
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
        expansion.exact_matches.push(t);
        return Ok(expansion);
    }

    // Build an absolute `(bytes, provenance)` stream where expanded `home` and
    // `cwd` bytes are marked `Derived` so their characters never act as globs.
    let mut abs_bytes: Vec<u8> = Vec::with_capacity(cwd.len() + raw.len() + 2);
    let mut abs_prov: Vec<ByteProvenance> = Vec::with_capacity(cwd.len() + raw.len() + 2);

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
            expansion.unread.push(UnreadReason::UnsupportedTildeForm);
            return Ok(expansion);
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
                Ok(comp) => comps.push(comp),
                Err(unread) => {
                    expansion.unread.push(unread);
                    return Ok(expansion);
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
    let mut entries_seen: usize = 0;

    while let Some((dir, idx, depth, variant_only)) = stack.pop() {
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
                        );
                        continue;
                    }
                    Err(ResolveError::CapReached(c)) => {
                        push_unique_unread(&mut expansion.unread, UnreadReason::from(c));
                        break;
                    }
                    Err(ResolveError::Exhausted(e)) => return Err(e),
                }
            }
            if variant_only {
                if variant_seen.insert(dir.clone()) {
                    variant_paths.push(dir);
                }
            } else if exact_seen.insert(dir.clone()) {
                exact_paths.push(dir);
            }
            continue;
        }

        match &comps[idx] {
            PatComp::Literal(seg) => {
                if seg == b"." {
                    stack.push((dir, idx + 1, depth, variant_only));
                    continue;
                }
                if seg == b".." {
                    let next = join_bytes(&dir, b"..");
                    stack.push((next, idx + 1, depth, variant_only));
                    continue;
                }
                let next = join_bytes(&dir, seg);
                match resolver.lstat(&next, caps, budget) {
                    Ok(_) => {
                        stack.push((next, idx + 1, depth, variant_only));
                    }
                    Err(ResolveError::NotFound | ResolveError::Io(FsIoError::NotADirectory)) => {}
                    Err(ResolveError::Io(_)) => {
                        push_unique_unread(
                            &mut expansion.unread,
                            UnreadReason::UnresolvablePath { path: next },
                        );
                    }
                    Err(ResolveError::CapReached(c)) => {
                        push_unique_unread(&mut expansion.unread, UnreadReason::from(c));
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
                    );
                    continue;
                }
                if *is_globstar {
                    // 0-level match (`a/**/b` -> `a/b`) is variant-only (zsh/bash globstar).
                    stack.push((dir.clone(), idx + 1, depth, true));
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
                        );
                        continue;
                    }
                    Err(ResolveError::CapReached(c)) => {
                        push_unique_unread(&mut expansion.unread, UnreadReason::from(c));
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
                        );
                        hit_cap = true;
                        break;
                    }
                    let is_dot = entry.name.starts_with(b".");
                    if *is_globstar {
                        // 1-level match agrees across sh, bash, zsh defaults when not a dotfile.
                        let next = join_bytes(&dir, &entry.name);
                        let one_level_variant = variant_only || is_dot;
                        stack.push((next.clone(), idx + 1, depth + 1, one_level_variant));
                        // 2+-level descent is variant-only.
                        if entry.file_type == FileType::Dir {
                            if depth + 1 >= limits::GLOB_MAX_DEPTH {
                                push_unique_unread(
                                    &mut expansion.unread,
                                    UnreadReason::WalkCapReached(FsCap::WalkDepth),
                                );
                            } else {
                                stack.push((next, idx, depth + 1, true));
                            }
                        }
                    } else if match_component(atoms, &entry.name, budget)? {
                        let next = join_bytes(&dir, &entry.name);
                        let entry_variant = variant_only || (is_dot && !explicit_leading_dot);
                        stack.push((next, idx + 1, depth + 1, entry_variant));
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

    // No-match fallback (§5): a glob that matches nothing is checked as its
    // literal word.
    if exact_paths.is_empty() && variant_paths.is_empty() && expansion.unread.is_empty() {
        let fallback = resolve_word_literal(
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
        expansion.exact_matches.push(fallback);
        return Ok(expansion);
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
        expansion.exact_matches.push(target);
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
        expansion.variant_only_matches.push(target);
    }

    Ok(expansion)
}

fn push_unique_unread(out: &mut Vec<UnreadReason>, reason: UnreadReason) {
    if !out.contains(&reason) {
        out.push(reason);
    }
}
