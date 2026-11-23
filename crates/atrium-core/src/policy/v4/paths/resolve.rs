//! Literal path resolution (contract §3, §5, §8; M1-32 obligations).
//!
//! Resolves paths component-by-component in filesystem order so:
//! - symlinks are followed before `..` (even on dangling tails);
//! - process-dependent paths (`/proc/self`, `/proc/thread-self`, `/proc/<pid>`,
//!   `/dev/fd/`, `/dev/stdin`, `/dev/stdout`, `/dev/stderr`) are caught before
//!   the OS canonicalizer can hide them behind the gate's own process;
//! - `(dev, ino)` identities of the resolved path and all of its existing
//!   ancestors are collected accurately (popped on `..`);
//! - the deepest existing ancestor is canonicalized for volume spelling;
//! - dangling symlinks are tracked and count under their resolved target name
//!   (while also retaining the symlink's own identity when dangling);
//! - unreadable directories, symlink loops / `MAX_HOPS`, and non-absence I/O
//!   errors produce `Unread(UnresolvablePath)`.

use std::collections::VecDeque;

use crate::policy::limits;
use crate::policy::model::{
    AccessMode, ComputeBudget, Exhausted, FileIdentity, FileType, FsCaps, FsIoError, PathMetadata,
    ResolveError, Target, TargetOrigin, TargetResolution, UnreadReason, Word,
};
use crate::policy::v4::paths::PathResolver;
use crate::policy::v4::paths::sensitive::strip_firmlink;

/// Returns `true` iff `path` is a process-dependent path under §3 / §11 item 14:
/// `/proc/self`, `/proc/thread-self`, `/proc/<pid>`, `/dev/fd/`, `/dev/stdin`,
/// `/dev/stdout`, `/dev/stderr` (exact or prefix followed by `/`).
pub(crate) fn is_process_dependent_path(path: &[u8]) -> bool {
    let path = strip_firmlink(path);
    if !path.starts_with(b"/") {
        return false;
    }
    let mut comps = path.split(|&b| b == b'/').filter(|s| !s.is_empty());
    let Some(first) = comps.next() else {
        return false;
    };
    let Some(second) = comps.next() else {
        return false;
    };
    if first.eq_ignore_ascii_case(b"proc") {
        return second.eq_ignore_ascii_case(b"self")
            || second.eq_ignore_ascii_case(b"thread-self")
            || (!second.is_empty() && second.iter().all(u8::is_ascii_digit));
    }
    if first.eq_ignore_ascii_case(b"dev") {
        return second.eq_ignore_ascii_case(b"fd")
            || second.eq_ignore_ascii_case(b"stdin")
            || second.eq_ignore_ascii_case(b"stdout")
            || second.eq_ignore_ascii_case(b"stderr");
    }
    false
}

/// Returns `true` iff `path` is macOS `/.vol` or `/.vol/...` (which bypasses
/// path-based policy checks; `gate.rs` line 161).
fn is_volfs_path(path: &[u8]) -> bool {
    if !path.starts_with(b"/") {
        return false;
    }
    let mut comps = path.split(|&b| b == b'/').filter(|s| !s.is_empty());
    comps
        .next()
        .is_some_and(|first| first.eq_ignore_ascii_case(b".vol"))
}

/// Joins an absolute base directory with a relative byte suffix.
pub(crate) fn join_bytes(base: &[u8], rel: &[u8]) -> Vec<u8> {
    if rel.is_empty() {
        return if base.is_empty() {
            b"/".to_vec()
        } else {
            base.to_vec()
        };
    }
    let trimmed_base = strip_trailing_slashes(base);
    let trimmed_rel = rel.strip_prefix(b"/").unwrap_or(rel);
    if trimmed_base == b"/" {
        let mut out = Vec::with_capacity(1 + trimmed_rel.len());
        out.push(b'/');
        out.extend_from_slice(trimmed_rel);
        out
    } else {
        let mut out = Vec::with_capacity(trimmed_base.len() + 1 + trimmed_rel.len());
        out.extend_from_slice(trimmed_base);
        out.push(b'/');
        out.extend_from_slice(trimmed_rel);
        out
    }
}

fn strip_trailing_slashes(path: &[u8]) -> &[u8] {
    let mut end = path.len();
    while end > 1 && path[end - 1] == b'/' {
        end -= 1;
    }
    &path[..end]
}

fn stack_path_len(stack: &[(Vec<u8>, Option<FileIdentity>)], extra: Option<&[u8]>) -> usize {
    if stack.is_empty() && extra.is_none() {
        return 1;
    }
    let mut len = 0usize;
    for (seg, _) in stack {
        len += seg.len() + 1;
    }
    if let Some(e) = extra {
        len += e.len() + 1;
    }
    len.max(1)
}

fn with_tail_len(stack: &[(Vec<u8>, Option<FileIdentity>)], missing_tail: &[Vec<u8>]) -> usize {
    let base = stack_path_len(stack, None);
    let tail: usize = missing_tail.iter().map(|s| s.len() + 1).sum();
    base + tail
}

fn build_path_from_stack(
    stack: &[(Vec<u8>, Option<FileIdentity>)],
    extra: Option<&[u8]>,
) -> Vec<u8> {
    let len = stack_path_len(stack, extra);
    let mut out = Vec::with_capacity(len);
    if stack.is_empty() && extra.is_none() {
        out.push(b'/');
        return out;
    }
    for (seg, _) in stack {
        out.push(b'/');
        out.extend_from_slice(seg);
    }
    if let Some(e) = extra {
        out.push(b'/');
        out.extend_from_slice(e);
    }
    out
}

fn unread_target(origin: TargetOrigin, access: AccessMode, reason: UnreadReason) -> Target {
    Target {
        origin,
        access,
        resolution: TargetResolution::Unread(reason),
    }
}

/// Resolves a shell [`Word`] as a literal path, respecting `~` provenance:
/// a quoted/escaped leading `~` (e.g. `'~user'` or `\~/x`) is treated as a
/// literal filename in `cwd` rather than a home expansion (§3, §5).
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_word_literal(
    word: &Word,
    cwd: &[u8],
    home: &[u8],
    origin: TargetOrigin,
    access: AccessMode,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
) -> Result<Target, Exhausted> {
    let raw = word.literal_bytes();
    if raw.starts_with(b"~") && !word.starts_with_unquoted_tilde() {
        budget.charge((cwd.len() + raw.len() + 1) as u64)?;
        budget.charge_alloc(cwd.len() + raw.len() + 1)?;
        let joined = join_bytes(cwd, raw);
        return resolve_literal_impl(&joined, cwd, home, origin, access, resolver, caps, budget);
    }
    resolve_literal_impl(raw, cwd, home, origin, access, resolver, caps, budget)
}

/// Implementation of `v4::paths::resolve_literal` (contract §3, §5, §8).
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_literal_impl(
    raw: &[u8],
    cwd: &[u8],
    home: &[u8],
    origin: TargetOrigin,
    access: AccessMode,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
) -> Result<Target, Exhausted> {
    budget.charge((raw.len() + cwd.len() + home.len() + 1) as u64)?;

    if raw.is_empty() || raw.contains(&0) {
        return Ok(unread_target(
            origin,
            access,
            UnreadReason::UnresolvablePath { path: raw.to_vec() },
        ));
    }

    // Modeled `~` expansion (§3): only bare `~` or `~/...`.
    let expanded: Vec<u8> = if raw == b"~" {
        budget.charge_alloc(home.len())?;
        home.to_vec()
    } else if let Some(rest) = raw.strip_prefix(b"~/") {
        budget.charge_alloc(home.len() + rest.len() + 1)?;
        join_bytes(home, rest)
    } else if raw.starts_with(b"~") {
        return Ok(unread_target(
            origin,
            access,
            UnreadReason::UnsupportedTildeForm,
        ));
    } else if raw.starts_with(b"/") {
        budget.charge_alloc(raw.len())?;
        raw.to_vec()
    } else {
        budget.charge_alloc(cwd.len() + raw.len() + 1)?;
        join_bytes(cwd, raw)
    };

    if is_process_dependent_path(&expanded) {
        return Ok(unread_target(
            origin,
            access,
            UnreadReason::ProcessDependentPath { path: expanded },
        ));
    }
    if is_volfs_path(&expanded) {
        return Ok(unread_target(
            origin,
            access,
            UnreadReason::UnresolvablePath { path: expanded },
        ));
    }

    budget.charge_alloc(expanded.len())?;
    let mut queue: VecDeque<Vec<u8>> = expanded
        .split(|&b| b == b'/')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_vec())
        .collect();

    let mut existing_stack: Vec<(Vec<u8>, Option<FileIdentity>)> = Vec::new();
    let mut missing_tail: Vec<Vec<u8>> = Vec::new();
    let mut hops: u8 = 0;
    // Number of front segments in `queue` originating from symlink expansions:
    // a symlink is dangling only when resolution enters `missing_tail` while
    // resolving segments produced by a symlink target (not an absent child of
    // an existing directory reached through a valid directory symlink).
    let mut symlink_target_segs: usize = 0;
    let mut dangling_symlink = false;
    let mut final_meta: Option<PathMetadata> = None;
    let mut symlink_identities: Vec<FileIdentity> = Vec::new();

    while let Some(seg) = queue.pop_front() {
        let from_symlink = symlink_target_segs > 0;
        if from_symlink {
            symlink_target_segs -= 1;
        }
        budget.charge(seg.len().max(1) as u64)?;
        if seg == b"." {
            continue;
        }
        if seg == b".." {
            if !missing_tail.is_empty() {
                missing_tail.pop();
            } else if !existing_stack.is_empty() {
                existing_stack.pop();
                final_meta = None;
            }
            let cur_len = with_tail_len(&existing_stack, &missing_tail);
            budget.charge_alloc(cur_len)?;
            let cur = build_with_tail(&existing_stack, &missing_tail);
            if is_process_dependent_path(&cur) {
                return Ok(unread_target(
                    origin,
                    access,
                    UnreadReason::ProcessDependentPath { path: cur },
                ));
            }
            continue;
        }

        if !missing_tail.is_empty() {
            missing_tail.push(seg);
            let cur_len = with_tail_len(&existing_stack, &missing_tail);
            budget.charge_alloc(cur_len)?;
            let cur = build_with_tail(&existing_stack, &missing_tail);
            if is_process_dependent_path(&cur) {
                return Ok(unread_target(
                    origin,
                    access,
                    UnreadReason::ProcessDependentPath { path: cur },
                ));
            }
            continue;
        }

        let cand_len = stack_path_len(&existing_stack, Some(&seg));
        budget.charge_alloc(cand_len)?;
        let cand = build_path_from_stack(&existing_stack, Some(&seg));
        if is_process_dependent_path(&cand) {
            return Ok(unread_target(
                origin,
                access,
                UnreadReason::ProcessDependentPath { path: cand },
            ));
        }
        if is_volfs_path(&cand) {
            return Ok(unread_target(
                origin,
                access,
                UnreadReason::UnresolvablePath { path: cand },
            ));
        }

        match resolver.lstat(&cand, caps, budget) {
            Ok(meta) => {
                if meta.file_type == FileType::Symlink {
                    if hops >= limits::MAX_HOPS {
                        return Ok(unread_target(
                            origin,
                            access,
                            UnreadReason::UnresolvablePath { path: cand },
                        ));
                    }
                    match resolver.read_link(&cand, caps, budget) {
                        Ok(target) => {
                            if target.is_empty() || target.contains(&0) {
                                return Ok(unread_target(
                                    origin,
                                    access,
                                    UnreadReason::UnresolvablePath { path: cand },
                                ));
                            }
                            hops += 1;
                            budget
                                .charge((existing_stack.len() + symlink_identities.len()) as u64)?;
                            budget.charge_alloc(
                                (existing_stack.len() + 1) * std::mem::size_of::<FileIdentity>(),
                            )?;
                            if let Some(id) = meta.identity
                                && !symlink_identities.contains(&id)
                            {
                                symlink_identities.push(id);
                            }
                            // Keep the identity of every directory holding the
                            // followed link: the path as spelled names an entry
                            // inside them, so a protected folder stays a hit by
                            // identity after the link leads out of it (§5: the
                            // protected spellings count in addition to identity;
                            // `~/.gnupg/sub/link -> /tmp/x` names `~/.gnupg`).
                            for (_, dir_id) in &existing_stack {
                                if let Some(dir_id) = dir_id
                                    && !symlink_identities.contains(dir_id)
                                {
                                    symlink_identities.push(*dir_id);
                                }
                            }
                            if target.starts_with(b"/") {
                                if is_process_dependent_path(&target) {
                                    return Ok(unread_target(
                                        origin,
                                        access,
                                        UnreadReason::ProcessDependentPath { path: target },
                                    ));
                                }
                                if is_volfs_path(&target) {
                                    return Ok(unread_target(
                                        origin,
                                        access,
                                        UnreadReason::UnresolvablePath { path: target },
                                    ));
                                }
                                existing_stack.clear();
                                final_meta = None;
                            }
                            budget.charge_alloc(target.len())?;
                            let target_segs: Vec<Vec<u8>> = target
                                .split(|&b| b == b'/')
                                .filter(|s| !s.is_empty())
                                .map(|s| s.to_vec())
                                .collect();
                            symlink_target_segs =
                                symlink_target_segs.saturating_add(target_segs.len());
                            for s in target_segs.into_iter().rev() {
                                queue.push_front(s);
                            }
                        }
                        Err(ResolveError::Exhausted(e)) => return Err(e),
                        Err(ResolveError::CapReached(c)) => {
                            return Ok(unread_target(origin, access, UnreadReason::from(c)));
                        }
                        Err(ResolveError::NotFound | ResolveError::Io(_)) => {
                            return Ok(unread_target(
                                origin,
                                access,
                                UnreadReason::UnresolvablePath { path: cand },
                            ));
                        }
                    }
                } else {
                    existing_stack.push((seg, meta.identity));
                    final_meta = Some(meta);
                }
            }
            Err(ResolveError::NotFound | ResolveError::Io(FsIoError::NotADirectory)) => {
                if from_symlink {
                    dangling_symlink = true;
                }
                missing_tail.push(seg);
                final_meta = None;
            }
            Err(ResolveError::Io(_)) => {
                return Ok(unread_target(
                    origin,
                    access,
                    UnreadReason::UnresolvablePath { path: cand },
                ));
            }
            Err(ResolveError::CapReached(c)) => {
                return Ok(unread_target(origin, access, UnreadReason::from(c)));
            }
            Err(ResolveError::Exhausted(e)) => return Err(e),
        }
    }

    // Canonicalize the deepest existing ancestor for volume canonical spelling
    // (handles case-insensitive volumes and platform firmlinks).
    let canon_base = if existing_stack.is_empty() {
        budget.charge_alloc(1)?;
        b"/".to_vec()
    } else {
        let existing_len = stack_path_len(&existing_stack, None);
        budget.charge_alloc(existing_len)?;
        let existing_path = build_path_from_stack(&existing_stack, None);
        match resolver.canonicalize(&existing_path, caps, budget) {
            Ok(c) => c,
            Err(ResolveError::Exhausted(e)) => return Err(e),
            Err(ResolveError::CapReached(c)) => {
                return Ok(unread_target(origin, access, UnreadReason::from(c)));
            }
            Err(ResolveError::Io(_)) => {
                return Ok(unread_target(
                    origin,
                    access,
                    UnreadReason::UnresolvablePath {
                        path: existing_path,
                    },
                ));
            }
            Err(ResolveError::NotFound) => existing_path,
        }
    };

    let tail_bytes: usize = missing_tail.iter().map(|s| s.len() + 1).sum();
    if tail_bytes > 0 {
        budget.charge(tail_bytes as u64)?;
        budget.charge_alloc(tail_bytes)?;
    }
    let mut canonical = canon_base;
    for seg in &missing_tail {
        if canonical.last() != Some(&b'/') {
            canonical.push(b'/');
        }
        canonical.extend_from_slice(seg);
    }

    if is_process_dependent_path(&canonical) {
        return Ok(unread_target(
            origin,
            access,
            UnreadReason::ProcessDependentPath { path: canonical },
        ));
    }
    if is_volfs_path(&canonical) {
        return Ok(unread_target(
            origin,
            access,
            UnreadReason::UnresolvablePath { path: canonical },
        ));
    }

    let dangling_symlink = dangling_symlink && !missing_tail.is_empty();
    let multiply_linked = missing_tail.is_empty()
        && final_meta.is_some_and(|m| m.file_type == FileType::File && m.nlink > 1);

    let max_ids = existing_stack.len() + symlink_identities.len();
    budget.charge_alloc(max_ids * std::mem::size_of::<FileIdentity>())?;
    let mut identities: Vec<FileIdentity> = Vec::with_capacity(max_ids);
    for (_, id_opt) in &existing_stack {
        if let Some(id) = id_opt
            && !identities.contains(id)
        {
            identities.push(*id);
        }
    }
    // Always preserve traversed symlink identities (both dangling links and
    // symlinks placed at protected spellings that canonicalize to ordinary
    // targets) and the identities of the directories that held them (§5).
    for id in symlink_identities {
        if !identities.contains(&id) {
            identities.push(id);
        }
    }

    Ok(Target {
        origin,
        access,
        resolution: TargetResolution::Resolved {
            canonical,
            identities,
            dangling_symlink,
            multiply_linked,
        },
    })
}

fn build_with_tail(
    existing_stack: &[(Vec<u8>, Option<FileIdentity>)],
    missing_tail: &[Vec<u8>],
) -> Vec<u8> {
    let mut out = build_path_from_stack(existing_stack, None);
    for seg in missing_tail {
        if out.last() != Some(&b'/') {
            out.push(b'/');
        }
        out.extend_from_slice(seg);
    }
    out
}
