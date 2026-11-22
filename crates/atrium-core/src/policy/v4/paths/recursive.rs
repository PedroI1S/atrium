//! Recursive read roots, S8 protected-root policy, search filters, followed
//! link walks, and bounded hard-link walks (contract §1 step 4, §2, §5, §8,
//! §11 items 7, 11, 12, 15).

use std::collections::{HashMap, HashSet};

use globset::{GlobBuilder, GlobMatcher};

use crate::policy::limits;
use crate::policy::model::{
    AccessMode, ComputeBudget, DirEntry, Exhausted, FileIdentity, FileType, FilterDialect, FsCap,
    FsCaps, FsIoError, Hit, HitMatch, HitSource, LinkPolicy, PartId, PartOutcome, PartResult,
    ProtectedSet, RecursiveRootResult, ResolveError, SearchFilter, Target, TargetOrigin,
    TargetResolution, UnreadReason,
};
use crate::policy::v4::paths::PathResolver;
use crate::policy::v4::paths::resolve::{join_bytes, resolve_literal_impl};
use crate::policy::v4::paths::sensitive::{
    ASK_FILES, ASK_NAME_PREFIXES, ASK_TREES, BROWSER_FILES, BROWSER_ROOTS, CRED_FILES, CRED_TREES,
    SharedSensitive, check_target_impl, collect_write_only_identities, fold_listed_component,
    fold_path_listed, join_folded, origin_to_part, under_bytes, under_str,
};

fn finish_recursive_exhausted(
    mut result: RecursiveRootResult,
    part: PartId,
    e: Exhausted,
) -> Result<RecursiveRootResult, Exhausted> {
    if result.outcomes.is_empty() && !result.multiply_linked_file {
        return Err(e);
    }
    if !result
        .outcomes
        .iter()
        .any(|r| matches!(r.outcome, PartOutcome::Exhausted(_)))
    {
        result.outcomes.push(PartResult {
            part,
            outcome: PartOutcome::Exhausted(e.reason),
        });
    }
    Ok(result)
}

pub(crate) fn check_recursive_root_impl(
    root: &Target,
    links: LinkPolicy,
    filters: &[SearchFilter],
    protected: &ProtectedSet,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
) -> Result<RecursiveRootResult, Exhausted> {
    let root_part = origin_to_part(&root.origin);
    let mut result = RecursiveRootResult::default();

    let (root_canon, root_identities, root_multiply_linked) = match &root.resolution {
        TargetResolution::Unread(reason) => {
            budget.charge(1)?;
            result.outcomes.push(PartResult {
                part: root_part,
                outcome: PartOutcome::Unread(reason.clone()),
            });
            return Ok(result);
        }
        TargetResolution::Resolved {
            canonical,
            identities,
            multiply_linked,
            ..
        } => (canonical, identities, *multiply_linked),
    };

    if root_multiply_linked {
        result.multiply_linked_file = true;
    }

    // 1. Check the root target itself (identities, listed folds, unlisted equivalences).
    let root_check = match check_target_impl(root, protected, budget) {
        Ok(r) => r,
        Err(e) => return finish_recursive_exhausted(result, root_part, e),
    };
    let root_is_hard_deny = matches!(root_check.outcome, PartOutcome::HardDenyHit(_));
    if !matches!(root_check.outcome, PartOutcome::Clear) {
        result.outcomes.push(root_check);
    }

    // Access-aware write-only identity protection when `root` is written (§5).
    if root.access != AccessMode::Read && !root_is_hard_deny {
        match collect_write_only_identities(protected, resolver, caps, budget) {
            Ok(wo) => {
                let mut matched_wo = false;
                for id in root_identities {
                    if let Some((_, hard_id)) = wo.identities.iter().find(|(pid, _)| pid == id) {
                        result.outcomes.push(PartResult {
                            part: root_part.clone(),
                            outcome: PartOutcome::HardDenyHit(Hit {
                                hard_deny_id: Some(hard_id),
                                source: HitSource::Model,
                                matched: HitMatch::ProtectedPath {
                                    path: root_canon.clone(),
                                },
                                part: root_part.clone(),
                            }),
                        });
                        matched_wo = true;
                        break;
                    }
                }
                if !matched_wo {
                    for u in wo.unreads {
                        result.outcomes.push(PartResult {
                            part: root_part.clone(),
                            outcome: PartOutcome::Unread(u),
                        });
                    }
                }
            }
            Err(e) => return finish_recursive_exhausted(result, root_part, e),
        }
    }

    let shared = match SharedSensitive::for_protected_budgeted(protected, budget) {
        Ok(s) => s,
        Err(e) => return finish_recursive_exhausted(result, root_part, e),
    };
    if let Err(e) = budget
        .charge(root_canon.len().max(1) as u64)
        .and_then(|()| budget.charge_alloc(root_canon.len().max(1)))
    {
        return finish_recursive_exhausted(result, root_part, e);
    }
    let folded_root = fold_path_listed(root_canon, protected.folds);
    let exempt_root = is_s8_exempt(&folded_root, shared.home_folded());

    // 2. S8 protected-root policy (§5, §11 item 12): a recursive root that
    //    contains an existing hard-deny path is HardDeny whatever its filters.
    //    Exemptions: home itself, ancestors of home, and `~/.config` (D-133).
    //    Extends to ancestors of Atrium data paths outside home.
    let s8_hit = match check_s8_protected_root(
        root_canon,
        &folded_root,
        &root_part,
        protected,
        &shared,
        resolver,
        caps,
        budget,
        &mut result.outcomes,
    ) {
        Ok(hit) => hit,
        Err(e) => return finish_recursive_exhausted(result, root_part, e),
    };

    // 2b. S8-exempted roots (home, ancestors of home, `~/.config`) and roots
    //     containing credential stores (CR-12 + CR-17) keep `dev`'s
    //     `Ask / Allow (rule) / Allow (Any) / Allow (FullAccess)` verdict via
    //     `StoreHit` (§5, §10 row 16).
    if !s8_hit && let Some(family) = root_contains_store_or_exempt_sensitive(&folded_root, &shared)
    {
        result.outcomes.push(PartResult {
            part: root_part.clone(),
            outcome: PartOutcome::StoreHit(Hit {
                hard_deny_id: None,
                source: HitSource::Model,
                matched: HitMatch::StoreFamily { family },
                part: root_part.clone(),
            }),
        });
    }

    // 3. Search filters (§5): single supported filter -> enumerate selected
    //    entries + match probe names; multiple/negated/unsupported -> Unread.
    let filter_part = filter_part_id(&root.origin);
    if filters.len() > 1 {
        result.outcomes.push(PartResult {
            part: filter_part,
            outcome: PartOutcome::Unread(UnreadReason::UnsupportedFilterForm),
        });
    } else if let Some(filter) = filters.first()
        && let Err(e) = check_filter_on_root(
            root_canon,
            &folded_root,
            filter,
            true,
            &filter_part,
            protected,
            &shared,
            resolver,
            caps,
            budget,
            &mut result.outcomes,
            &mut result.multiply_linked_file,
        )
    {
        return finish_recursive_exhausted(result, filter_part, e);
    }

    // 4. Followed-link walk (§5, §10 row 15, §11 item 15, M1-32 `rg -L` verdict).
    if links.below_root
        && let Err(e) = walk_followed_links(
            root_canon,
            &root_part,
            !filters.is_empty(),
            protected,
            &shared,
            resolver,
            caps,
            budget,
            &mut result.outcomes,
            &mut result.multiply_linked_file,
        )
    {
        return finish_recursive_exhausted(result, root_part, e);
    }

    // 5. Bounded hard-link walk (§2, §5, §8, §11 item 7): 4,096 entries,
    //    depth 8, skipping `node_modules` and `.git`, clear when its local
    //    cutoff runs out, while recording global `FsCaps` failures as Unread
    //    and preserving S8 exemptions on exempt roots.
    if let Err(e) = walk_hard_links(
        root_canon,
        &root_part,
        !filters.is_empty(),
        exempt_root,
        protected,
        resolver,
        caps,
        budget,
        &mut result.outcomes,
        &mut result.multiply_linked_file,
    ) {
        return finish_recursive_exhausted(result, root_part, e);
    }

    if result.outcomes.is_empty() {
        result.outcomes.push(PartResult {
            part: root_part,
            outcome: PartOutcome::Clear,
        });
    }

    Ok(result)
}

fn filter_part_id(origin: &TargetOrigin) -> PartId {
    match origin {
        TargetOrigin::Operand { unit, .. } | TargetOrigin::ImplicitCwd { unit: Some(unit) } => {
            PartId::Filter { unit: Some(*unit) }
        }
        _ => PartId::Filter { unit: None },
    }
}

/// Checks whether `root_canon` is exempt from S8 (`home`, ancestor of `home`,
/// or `~/.config` per D-133).
fn is_s8_exempt(folded_root: &[u8], home_folded: &[u8]) -> bool {
    if under_bytes(home_folded, folded_root).is_some() {
        return true;
    }
    let dot_config = join_folded(home_folded, b".config");
    folded_root == dot_config.as_slice()
}

/// S8 protected-root check: returns `Ok(true)` if an existing hard-deny path
/// was found under `root_canon` (and pushes the `HardDenyHit` into `outcomes`).
type S8DirCache = HashMap<Vec<u8>, Option<Vec<DirEntry>>>;

#[allow(clippy::too_many_arguments)]
fn s8_read_dir_cached<'a>(
    dir: &[u8],
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
    part: &PartId,
    outcomes: &mut Vec<PartResult>,
    cache: &'a mut S8DirCache,
) -> Result<Option<&'a [DirEntry]>, Exhausted> {
    if !cache.contains_key(dir) {
        budget.charge_alloc(dir.len())?;
        let val = match resolver.read_dir(dir, caps, budget) {
            Ok(entries) => Some(entries),
            Err(ResolveError::NotFound | ResolveError::Io(FsIoError::NotADirectory)) => None,
            Err(ResolveError::Io(_)) => {
                outcomes.push(PartResult {
                    part: part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::UnresolvablePath {
                        path: dir.to_vec(),
                    }),
                });
                None
            }
            Err(ResolveError::CapReached(c)) => {
                outcomes.push(PartResult {
                    part: part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::from(c)),
                });
                None
            }
            Err(ResolveError::Exhausted(e)) => return Err(e),
        };
        cache.insert(dir.to_vec(), val);
    }
    Ok(cache.get(dir).and_then(|opt| opt.as_deref()))
}

/// Discovers existing listed-fold spellings of `suffix_folded` under `base`
/// within the filesystem caps and compute budget (fast-pathing exact `lstat`
/// first, then matching directory entries via `fold_listed_component`).
#[allow(clippy::too_many_arguments)]
fn find_s8_folded_matches(
    base: &[u8],
    suffix_folded: &str,
    protected: &ProtectedSet,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
    part: &PartId,
    outcomes: &mut Vec<PartResult>,
    cache: &mut S8DirCache,
) -> Result<Vec<Vec<u8>>, Exhausted> {
    let comps: Vec<&str> = suffix_folded.split('/').filter(|s| !s.is_empty()).collect();
    if comps.is_empty() {
        return match resolver.lstat(base, caps, budget) {
            Ok(_) => {
                budget.charge_alloc(base.len())?;
                Ok(vec![base.to_vec()])
            }
            Err(ResolveError::NotFound | ResolveError::Io(FsIoError::NotADirectory)) => {
                Ok(Vec::new())
            }
            Err(ResolveError::Io(_)) => {
                outcomes.push(PartResult {
                    part: part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::UnresolvablePath {
                        path: base.to_vec(),
                    }),
                });
                Ok(Vec::new())
            }
            Err(ResolveError::CapReached(c)) => {
                outcomes.push(PartResult {
                    part: part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::from(c)),
                });
                Ok(Vec::new())
            }
            Err(ResolveError::Exhausted(e)) => Err(e),
        };
    }

    // Fast path: check exact lowercase spelling via `lstat` first (also handles
    // non-listable `chmod 0111` directories).
    let exact_len = base.len() + 1 + suffix_folded.len();
    budget.charge_alloc(exact_len)?;
    let exact = join_bytes(base, suffix_folded.as_bytes());
    match resolver.lstat(&exact, caps, budget) {
        Ok(_) => return Ok(vec![exact]),
        Err(ResolveError::NotFound | ResolveError::Io(FsIoError::NotADirectory)) => {}
        Err(ResolveError::Io(_)) => {
            outcomes.push(PartResult {
                part: part.clone(),
                outcome: PartOutcome::Unread(UnreadReason::UnresolvablePath { path: exact }),
            });
        }
        Err(ResolveError::CapReached(c)) => {
            outcomes.push(PartResult {
                part: part.clone(),
                outcome: PartOutcome::Unread(UnreadReason::from(c)),
            });
            return Ok(Vec::new());
        }
        Err(ResolveError::Exhausted(e)) => return Err(e),
    }

    // Walk component-by-component via cached `read_dir` to establish existence
    // across all listed-fold spellings (e.g. `.aws/CREDENTIALS`, `.AWS/CREDENTIALS`,
    // `Library/Application Support/Google/Chrome`).
    budget.charge_alloc(base.len())?;
    let mut frontier: Vec<Vec<u8>> = vec![base.to_vec()];
    let mut entries_seen = 0usize;
    let folds = protected.folds;
    for comp in comps {
        let mut next: Vec<Vec<u8>> = Vec::new();
        for dir in &frontier {
            let Some(entries) =
                s8_read_dir_cached(dir, resolver, caps, budget, part, outcomes, cache)?
            else {
                continue;
            };
            for entry in entries {
                if entry.name == b"." || entry.name == b".." {
                    continue;
                }
                entries_seen += 1;
                if entries_seen > limits::WALK_MAX_ENTRIES {
                    outcomes.push(PartResult {
                        part: part.clone(),
                        outcome: PartOutcome::Unread(UnreadReason::WalkCapReached(
                            FsCap::EntriesRead,
                        )),
                    });
                    return Ok(Vec::new());
                }
                budget.charge(entry.name.len().max(1) as u64)?;
                budget.charge_alloc(entry.name.len().max(1))?;
                let folded =
                    fold_listed_component(&entry.name, folds.fold_long_s, folds.fold_kelvin);
                if folded == comp.as_bytes() {
                    budget.charge_alloc(dir.len() + 1 + entry.name.len())?;
                    let cand = join_bytes(dir, &entry.name);
                    if !next.contains(&cand) {
                        next.push(cand);
                    }
                }
            }
        }
        if next.is_empty() {
            return Ok(Vec::new());
        }
        frontier = next;
    }
    Ok(frontier)
}

#[allow(clippy::too_many_arguments)]
fn check_s8_protected_root(
    root_canon: &[u8],
    folded_root: &[u8],
    part: &PartId,
    protected: &ProtectedSet,
    shared: &SharedSensitive,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
    outcomes: &mut Vec<PartResult>,
) -> Result<bool, Exhausted> {
    let home_folded = shared.home_folded();
    if is_s8_exempt(folded_root, home_folded) {
        return Ok(false);
    }

    let push_hit = |outcomes: &mut Vec<PartResult>, id: &'static str, path: Vec<u8>| {
        outcomes.push(PartResult {
            part: part.clone(),
            outcome: PartOutcome::HardDenyHit(Hit {
                hard_deny_id: Some(id),
                source: HitSource::Model,
                matched: HitMatch::ProtectedPath { path },
                part: part.clone(),
            }),
        });
    };

    // Helper: check if `candidate` exists via `lstat`, recording I/O or cap Unreads.
    let check_exists = |cand: &[u8],
                        resolver: &dyn PathResolver,
                        caps: &mut FsCaps,
                        budget: &mut ComputeBudget,
                        outcomes: &mut Vec<PartResult>|
     -> Result<bool, Exhausted> {
        match resolver.lstat(cand, caps, budget) {
            Ok(_) => Ok(true),
            Err(ResolveError::NotFound | ResolveError::Io(FsIoError::NotADirectory)) => Ok(false),
            Err(ResolveError::Io(_)) => {
                outcomes.push(PartResult {
                    part: part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::UnresolvablePath {
                        path: cand.to_vec(),
                    }),
                });
                Ok(false)
            }
            Err(ResolveError::CapReached(c)) => {
                outcomes.push(PartResult {
                    part: part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::from(c)),
                });
                Ok(false)
            }
            Err(ResolveError::Exhausted(e)) => Err(e),
        }
    };

    let mut dir_cache: S8DirCache = HashMap::new();

    // 1. Atrium data paths (inside OR outside home; M1-31 round 2 finding 4).
    for (raw_d, folded_d) in protected
        .atrium_data_paths
        .iter()
        .zip(shared.data_folded().iter())
    {
        if let Some(suffix_bytes) = under_bytes(folded_d, folded_root) {
            if check_exists(raw_d, resolver, caps, budget, outcomes)? {
                push_hit(outcomes, "atrium-self", raw_d.clone());
                return Ok(true);
            }
            if let Ok(suffix_str) = std::str::from_utf8(suffix_bytes) {
                let matches = find_s8_folded_matches(
                    root_canon,
                    suffix_str,
                    protected,
                    resolver,
                    caps,
                    budget,
                    part,
                    outcomes,
                    &mut dir_cache,
                )?;
                if let Some(found) = matches.into_iter().next() {
                    push_hit(outcomes, "atrium-self", found);
                    return Ok(true);
                }
            }
        }
    }

    // 2. Directories linked into `~/.ssh` (inside OR outside home).
    for (raw_d, folded_d) in protected
        .ssh_linked_dirs
        .iter()
        .zip(shared.ssh_linked_folded().iter())
    {
        if let Some(suffix_bytes) = under_bytes(folded_d, folded_root) {
            if check_exists(raw_d, resolver, caps, budget, outcomes)? {
                push_hit(outcomes, "credential-read", raw_d.clone());
                return Ok(true);
            }
            if let Ok(suffix_str) = std::str::from_utf8(suffix_bytes) {
                let matches = find_s8_folded_matches(
                    root_canon,
                    suffix_str,
                    protected,
                    resolver,
                    caps,
                    budget,
                    part,
                    outcomes,
                    &mut dir_cache,
                )?;
                if let Some(found) = matches.into_iter().next() {
                    push_hit(outcomes, "credential-read", found);
                    return Ok(true);
                }
            }
        }
    }

    // 3. Home-relative credential files, trees, `.ssh`, and browser roots.
    let Some(root_rel_bytes) = under_bytes(folded_root, home_folded) else {
        return Ok(false);
    };
    let Ok(root_rel) = std::str::from_utf8(root_rel_bytes) else {
        return Ok(false);
    };

    for f in CRED_FILES {
        if let Some(suffix) = under_str(f, root_rel) {
            let matches = find_s8_folded_matches(
                root_canon,
                suffix,
                protected,
                resolver,
                caps,
                budget,
                part,
                outcomes,
                &mut dir_cache,
            )?;
            if let Some(found) = matches.into_iter().next() {
                push_hit(outcomes, "credential-read", found);
                return Ok(true);
            }
        }
    }

    for t in CRED_TREES {
        if let Some(suffix) = under_str(t, root_rel) {
            let matches = find_s8_folded_matches(
                root_canon,
                suffix,
                protected,
                resolver,
                caps,
                budget,
                part,
                outcomes,
                &mut dir_cache,
            )?;
            if let Some(found) = matches.into_iter().next() {
                push_hit(outcomes, "credential-read", found);
                return Ok(true);
            }
        }
    }

    // `.ssh`: check if any existing hard-deny entry lives inside `~/.ssh`
    // (including listed-fold spellings like `.SSH` / `.ſsh`).
    if under_str(root_rel, ".ssh").is_some() {
        if let Some(found) = dir_has_existing_hard_deny_ssh_entry(
            root_canon, protected, shared, resolver, caps, budget, part, outcomes,
        )? {
            push_hit(outcomes, "credential-read", found);
            return Ok(true);
        }
    } else if let Some(suffix) = under_str(".ssh", root_rel) {
        let ssh_roots = find_s8_folded_matches(
            root_canon,
            suffix,
            protected,
            resolver,
            caps,
            budget,
            part,
            outcomes,
            &mut dir_cache,
        )?;
        for ssh_root in ssh_roots {
            if let Some(found) = dir_has_existing_hard_deny_ssh_entry(
                &ssh_root, protected, shared, resolver, caps, budget, part, outcomes,
            )? {
                push_hit(outcomes, "credential-read", found);
                return Ok(true);
            }
        }
    }

    // Browser roots & profile folders (M1-31 review 1 finding 4: ordinary
    // files like `Default/Preferences` are NOT folders holding credentials).
    for b in BROWSER_ROOTS {
        if let Some(suffix) = under_str(b, root_rel) {
            let b_paths = find_s8_folded_matches(
                root_canon,
                suffix,
                protected,
                resolver,
                caps,
                budget,
                part,
                outcomes,
                &mut dir_cache,
            )?;
            for b_path in b_paths {
                if let Some(found) = browser_dir_has_existing_cred(
                    &b_path, protected, resolver, caps, budget, part, outcomes,
                )? {
                    push_hit(outcomes, "credential-read", found);
                    return Ok(true);
                }
            }
        } else if under_str(root_rel, b).is_some() {
            // Only a directory inside a browser root can contain browser credential files.
            match resolver.lstat(root_canon, caps, budget) {
                Ok(md) if md.file_type == FileType::Dir => {
                    if let Some(found) = browser_dir_has_existing_cred(
                        root_canon, protected, resolver, caps, budget, part, outcomes,
                    )? {
                        push_hit(outcomes, "credential-read", found);
                        return Ok(true);
                    }
                }
                Ok(_)
                | Err(ResolveError::NotFound | ResolveError::Io(FsIoError::NotADirectory)) => {}
                Err(ResolveError::Io(_)) => {
                    outcomes.push(PartResult {
                        part: part.clone(),
                        outcome: PartOutcome::Unread(UnreadReason::UnresolvablePath {
                            path: root_canon.to_vec(),
                        }),
                    });
                }
                Err(ResolveError::CapReached(c)) => {
                    outcomes.push(PartResult {
                        part: part.clone(),
                        outcome: PartOutcome::Unread(UnreadReason::from(c)),
                    });
                }
                Err(ResolveError::Exhausted(e)) => return Err(e),
            }
        }
    }

    Ok(false)
}

#[allow(clippy::too_many_arguments)]
fn dir_has_existing_hard_deny_ssh_entry(
    ssh_root: &[u8],
    protected: &ProtectedSet,
    shared: &SharedSensitive,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
    part: &PartId,
    outcomes: &mut Vec<PartResult>,
) -> Result<Option<Vec<u8>>, Exhausted> {
    budget.charge_alloc(ssh_root.len())?;
    let mut stack = vec![(ssh_root.to_vec(), 0u8)];
    let mut seen = 0usize;
    let mut emitted_depth_cap = false;
    while let Some((dir, depth)) = stack.pop() {
        let entries = match resolver.read_dir(&dir, caps, budget) {
            Ok(e) => e,
            Err(ResolveError::NotFound | ResolveError::Io(FsIoError::NotADirectory)) => continue,
            Err(ResolveError::Io(_)) => {
                outcomes.push(PartResult {
                    part: part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::UnresolvablePath { path: dir }),
                });
                continue;
            }
            Err(ResolveError::CapReached(c)) => {
                outcomes.push(PartResult {
                    part: part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::from(c)),
                });
                return Ok(None);
            }
            Err(ResolveError::Exhausted(e)) => return Err(e),
        };
        for entry in entries {
            if entry.name == b"." || entry.name == b".." {
                continue;
            }
            seen += 1;
            if seen > limits::WALK_MAX_ENTRIES {
                outcomes.push(PartResult {
                    part: part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::WalkCapReached(FsCap::EntriesRead)),
                });
                return Ok(None);
            }
            let child_len = dir.len() + 1 + entry.name.len();
            budget.charge(child_len as u64)?;
            budget.charge_alloc(child_len.saturating_mul(2))?;
            let child = join_bytes(&dir, &entry.name);
            let folded = fold_path_listed(&child, protected.folds);
            if entry.file_type != FileType::Dir
                && shared
                    .hard_deny_hit_folded(&folded, AccessMode::Read)
                    .is_some()
            {
                return Ok(Some(child));
            }
            if entry.file_type == FileType::Dir {
                if depth + 1 < limits::WALK_MAX_DEPTH {
                    stack.push((child, depth + 1));
                } else if !emitted_depth_cap {
                    emitted_depth_cap = true;
                    outcomes.push(PartResult {
                        part: part.clone(),
                        outcome: PartOutcome::Unread(UnreadReason::WalkCapReached(
                            FsCap::WalkDepth,
                        )),
                    });
                }
            }
        }
    }
    Ok(None)
}

/// Bounded check within the frozen walk limits (`WALK_MAX_DEPTH`,
/// `WALK_MAX_ENTRIES`) for any existing `BROWSER_FILES` inside a browser root
/// or profile directory, emitting `Unread(WalkCapReached)` when a limit
/// prevents completion (§5, §8).
fn browser_dir_has_existing_cred(
    root: &[u8],
    protected: &ProtectedSet,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
    part: &PartId,
    outcomes: &mut Vec<PartResult>,
) -> Result<Option<Vec<u8>>, Exhausted> {
    budget.charge_alloc(root.len())?;
    let mut stack = vec![(root.to_vec(), 0u8)];
    let mut seen = 0usize;
    let mut emitted_depth_cap = false;
    while let Some((dir, depth)) = stack.pop() {
        let entries = match resolver.read_dir(&dir, caps, budget) {
            Ok(e) => e,
            Err(ResolveError::NotFound | ResolveError::Io(FsIoError::NotADirectory)) => continue,
            Err(ResolveError::Io(_)) => {
                outcomes.push(PartResult {
                    part: part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::UnresolvablePath { path: dir }),
                });
                continue;
            }
            Err(ResolveError::CapReached(c)) => {
                outcomes.push(PartResult {
                    part: part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::from(c)),
                });
                return Ok(None);
            }
            Err(ResolveError::Exhausted(e)) => return Err(e),
        };
        for entry in entries {
            if entry.name == b"." || entry.name == b".." {
                continue;
            }
            seen += 1;
            if seen > limits::WALK_MAX_ENTRIES {
                outcomes.push(PartResult {
                    part: part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::WalkCapReached(FsCap::EntriesRead)),
                });
                return Ok(None);
            }
            budget.charge(entry.name.len().max(1) as u64)?;
            budget.charge_alloc(entry.name.len().max(1))?;
            let name_folded = fold_listed_component(
                &entry.name,
                protected.folds.fold_long_s,
                protected.folds.fold_kelvin,
            );
            if entry.file_type != FileType::Dir
                && std::str::from_utf8(&name_folded).is_ok_and(|n| BROWSER_FILES.contains(&n))
            {
                budget.charge_alloc(dir.len() + 1 + entry.name.len())?;
                return Ok(Some(join_bytes(&dir, &entry.name)));
            }
            if entry.file_type == FileType::Dir {
                if depth + 1 < limits::WALK_MAX_DEPTH {
                    budget.charge_alloc(dir.len() + 1 + entry.name.len())?;
                    stack.push((join_bytes(&dir, &entry.name), depth + 1));
                } else if !emitted_depth_cap {
                    emitted_depth_cap = true;
                    outcomes.push(PartResult {
                        part: part.clone(),
                        outcome: PartOutcome::Unread(UnreadReason::WalkCapReached(
                            FsCap::WalkDepth,
                        )),
                    });
                }
            }
        }
    }
    Ok(None)
}

/// Checks if a recursive root is an S8-exempted sensitive container (`home`,
/// ancestor of `home`, `~/.config`) or an ancestor of a CR-12/CR-17 credential
/// store (producing `StoreHit` -> `Ask / Allow (rule) / Allow (Any) / Allow (FullAccess)`).
fn root_contains_store_or_exempt_sensitive(
    folded_root: &[u8],
    shared: &SharedSensitive,
) -> Option<&'static str> {
    let home_folded = shared.home_folded();
    if under_bytes(home_folded, folded_root).is_some() {
        return Some("home-root");
    }
    let dot_config = join_folded(home_folded, b".config");
    if folded_root == dot_config.as_slice() {
        return Some("config-root");
    }
    let rest = under_bytes(folded_root, home_folded).and_then(|b| std::str::from_utf8(b).ok())?;
    if rest == ".ssh" {
        return Some("ssh-dir");
    }
    for &(f, family) in ASK_FILES {
        if under_str(f, rest).is_some() {
            return Some(family);
        }
    }
    for &(t, family) in ASK_TREES {
        if under_str(t, rest).is_some() {
            return Some(family);
        }
    }
    for &(dir, _, family) in ASK_NAME_PREFIXES {
        if under_str(dir, rest).is_some() {
            return Some(family);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Search filters (Step 1: selected entries; Step 2: probe names) (§5)
// ---------------------------------------------------------------------------

struct CompiledFilter {
    dialect: FilterDialect,
    matcher: GlobMatcher,
    /// True if pattern matches basename only (e.g. `grep --include`, or `rg -g` /
    /// `SearchGlob` without `/`).
    match_basename: bool,
    /// True if the pattern is anchored to the search root (e.g. `rg -g '/foo'`).
    root_anchored: bool,
    /// True if pattern contains `**` (needs recursive traversal even when
    /// `recursive == false` for `SearchGlob`).
    has_globstar: bool,
    /// Maximum depth needed when `recursive == false` and `!has_globstar`.
    max_nonrecursive_depth: usize,
    case_insensitive: bool,
}

fn compile_filter(
    filter: &SearchFilter,
    budget: &mut ComputeBudget,
) -> Result<Result<CompiledFilter, UnreadReason>, Exhausted> {
    let raw = filter.pattern.literal_bytes();
    budget.charge(raw.len().max(1) as u64)?;
    if raw.is_empty() || raw.contains(&0) {
        return Ok(Err(UnreadReason::UnsupportedFilterForm));
    }
    let Ok(pat_str) = std::str::from_utf8(raw) else {
        return Ok(Err(UnreadReason::UnsupportedFilterForm));
    };

    // Negated filters are outside the model (§5).
    if pat_str.starts_with('!') {
        return Ok(Err(UnreadReason::UnsupportedFilterForm));
    }

    // Reject `..` components in filters (selectors are handled separately in
    // `resolve_subject_paths`; `..` in `rg -g` or `grep --include` is outside the model).
    if pat_str.split('/').any(|c| c == "..") {
        return Ok(Err(UnreadReason::UnsupportedFilterForm));
    }

    match filter.dialect {
        FilterDialect::GrepInclude => {
            if pat_str.contains('/') {
                return Ok(Err(UnreadReason::UnsupportedFilterForm));
            }
            let matcher = match build_glob_matcher(pat_str, filter.case_insensitive, budget)? {
                Ok(m) => m,
                Err(u) => return Ok(Err(u)),
            };
            Ok(Ok(CompiledFilter {
                dialect: filter.dialect,
                matcher,
                match_basename: true,
                root_anchored: false,
                has_globstar: false,
                max_nonrecursive_depth: 1,
                case_insensitive: filter.case_insensitive,
            }))
        }
        FilterDialect::RgGlob => {
            let root_anchored = pat_str.starts_with('/');
            let trimmed = pat_str.trim_start_matches('/').trim_end_matches('/');
            if trimmed.is_empty() {
                return Ok(Err(UnreadReason::UnsupportedFilterForm));
            }
            let has_slash = trimmed.contains('/');
            let match_basename = !root_anchored && !has_slash;
            let has_globstar = trimmed.contains("**");
            let matcher = match build_glob_matcher(trimmed, filter.case_insensitive, budget)? {
                Ok(m) => m,
                Err(u) => return Ok(Err(u)),
            };
            Ok(Ok(CompiledFilter {
                dialect: filter.dialect,
                matcher,
                match_basename,
                root_anchored,
                has_globstar,
                max_nonrecursive_depth: trimmed.split('/').count(),
                case_insensitive: filter.case_insensitive,
            }))
        }
        FilterDialect::SearchGlob => {
            let trimmed = pat_str.trim_start_matches("./").trim_end_matches('/');
            if trimmed.is_empty() || trimmed.starts_with('/') || trimmed.starts_with('~') {
                return Ok(Err(UnreadReason::UnsupportedFilterForm));
            }
            let has_slash = trimmed.contains('/');
            let has_globstar = trimmed.contains("**");
            let match_basename = !has_slash;
            let matcher = match build_glob_matcher(trimmed, filter.case_insensitive, budget)? {
                Ok(m) => m,
                Err(u) => return Ok(Err(u)),
            };
            Ok(Ok(CompiledFilter {
                dialect: filter.dialect,
                matcher,
                match_basename,
                root_anchored: false,
                has_globstar,
                max_nonrecursive_depth: trimmed.split('/').count(),
                case_insensitive: filter.case_insensitive,
            }))
        }
    }
}

fn build_glob_matcher(
    pat: &str,
    case_insensitive: bool,
    budget: &mut ComputeBudget,
) -> Result<Result<GlobMatcher, UnreadReason>, Exhausted> {
    // Charge compilation work proportional to pattern length before invoking globset.
    budget.charge((pat.len() as u64).saturating_mul(8).max(8))?;
    budget.charge_alloc(pat.len().saturating_mul(16).max(64))?;
    match GlobBuilder::new(pat)
        .literal_separator(true)
        .case_insensitive(case_insensitive)
        .build()
    {
        Ok(g) => Ok(Ok(g.compile_matcher())),
        Err(_) => Ok(Err(UnreadReason::UnsupportedFilterForm)),
    }
}

impl CompiledFilter {
    fn matches_entry(
        &self,
        name: &[u8],
        rel: &[u8],
        entry_full: &[u8],
        recursive: bool,
        budget: &mut ComputeBudget,
    ) -> Result<Result<bool, UnreadReason>, Exhausted> {
        budget.charge((rel.len() + name.len() + 1) as u64)?;
        let _ = self.dialect;
        if self.match_basename {
            if !recursive && rel.contains(&b'/') {
                return Ok(Ok(false));
            }
            return Ok(match_raw_bytes_path(&self.matcher, name, entry_full));
        }
        if self.root_anchored {
            return Ok(match_raw_bytes_path(&self.matcher, rel, entry_full));
        }
        Ok(match_raw_bytes_path(&self.matcher, rel, entry_full))
    }

    fn matches_probe(
        &self,
        probe_rel: &str,
        probe_name: &str,
        recursive: bool,
        budget: &mut ComputeBudget,
    ) -> Result<bool, Exhausted> {
        budget.charge((probe_rel.len() + probe_name.len() + 1) as u64)?;
        if probe_rel.is_empty() {
            return Ok(false);
        }
        if self.match_basename {
            if !recursive && !self.has_globstar && probe_rel.contains('/') {
                return Ok(false);
            }
            return Ok(self.matcher.is_match(probe_name));
        }
        if !recursive && !self.has_globstar {
            let depth = probe_rel.split('/').count();
            if depth > self.max_nonrecursive_depth {
                return Ok(false);
            }
        }
        Ok(self.matcher.is_match(probe_rel))
    }
}

/// Checks one [`SearchFilter`] on `root_canon` in two steps (§5):
/// 1. Enumerate selected entries under `root_canon` within the walk cap and
///    resolve + check each one;
/// 2. Match the filter against `dev`'s probe names (Store hit for store probes,
///    HardDeny for hard-deny probes).
#[cfg(unix)]
fn match_raw_bytes_path(
    matcher: &GlobMatcher,
    candidate: &[u8],
    _full_path: &[u8],
) -> Result<bool, UnreadReason> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;
    Ok(matcher.is_match(Path::new(OsStr::from_bytes(candidate))))
}

#[cfg(not(unix))]
fn match_raw_bytes_path(
    matcher: &GlobMatcher,
    candidate: &[u8],
    full_path: &[u8],
) -> Result<bool, UnreadReason> {
    match std::str::from_utf8(candidate) {
        Ok(s) => Ok(matcher.is_match(std::path::Path::new(s))),
        Err(_) => Err(UnreadReason::UnresolvablePath {
            path: full_path.to_vec(),
        }),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn check_filter_on_root(
    root_canon: &[u8],
    folded_root: &[u8],
    filter: &SearchFilter,
    recursive: bool,
    filter_part: &PartId,
    protected: &ProtectedSet,
    shared: &SharedSensitive,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
    outcomes: &mut Vec<PartResult>,
    multiply_linked_file: &mut bool,
) -> Result<(), Exhausted> {
    let compiled = match compile_filter(filter, budget)? {
        Ok(c) => c,
        Err(reason) => {
            outcomes.push(PartResult {
                part: filter_part.clone(),
                outcome: PartOutcome::Unread(reason),
            });
            return Ok(());
        }
    };

    // Step 1: enumerate selected entries within walk cap and resolve each one.
    let max_depth: usize = if recursive || compiled.has_globstar {
        limits::WALK_MAX_DEPTH as usize
    } else {
        compiled
            .max_nonrecursive_depth
            .min(limits::WALK_MAX_DEPTH as usize)
    };

    budget.charge_alloc(root_canon.len())?;
    let mut stack: Vec<(Vec<u8>, Vec<u8>, usize)> = vec![(root_canon.to_vec(), Vec::new(), 0)];
    let mut visited_symlink_dirs: HashSet<Vec<u8>> = HashSet::new();
    let mut entries_seen = 0usize;
    let mut emitted_depth_cap = false;
    let follow_dir_symlinks = compiled.dialect == FilterDialect::SearchGlob;

    while let Some((dir, rel_dir, depth)) = stack.pop() {
        if depth >= max_depth {
            if (recursive || compiled.has_globstar) && !emitted_depth_cap {
                emitted_depth_cap = true;
                outcomes.push(PartResult {
                    part: filter_part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::WalkCapReached(FsCap::WalkDepth)),
                });
            }
            continue;
        }
        let entries = match resolver.read_dir(&dir, caps, budget) {
            Ok(e) => e,
            Err(ResolveError::NotFound | ResolveError::Io(FsIoError::NotADirectory)) => continue,
            Err(ResolveError::Io(_)) => {
                outcomes.push(PartResult {
                    part: filter_part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::UnresolvablePath { path: dir }),
                });
                continue;
            }
            Err(ResolveError::CapReached(c)) => {
                outcomes.push(PartResult {
                    part: filter_part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::from(c)),
                });
                break;
            }
            Err(ResolveError::Exhausted(e)) => return Err(e),
        };

        let mut capped = false;
        for entry in entries {
            if entry.name == b"." || entry.name == b".." {
                continue;
            }
            entries_seen += 1;
            if entries_seen > limits::WALK_MAX_ENTRIES {
                outcomes.push(PartResult {
                    part: filter_part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::WalkCapReached(FsCap::EntriesRead)),
                });
                capped = true;
                break;
            }
            budget.charge_alloc(dir.len() + rel_dir.len() + entry.name.len() * 2 + 2)?;
            let entry_full = join_bytes(&dir, &entry.name);
            let entry_rel = if rel_dir.is_empty() {
                entry.name.clone()
            } else {
                let mut v = Vec::with_capacity(rel_dir.len() + 1 + entry.name.len());
                v.extend_from_slice(&rel_dir);
                v.push(b'/');
                v.extend_from_slice(&entry.name);
                v
            };

            match compiled.matches_entry(
                &entry.name,
                &entry_rel,
                &entry_full,
                recursive || compiled.has_globstar,
                budget,
            )? {
                Ok(true) => {
                    let resolved = resolve_literal_impl(
                        &entry_full,
                        &protected.cwd,
                        &protected.home,
                        TargetOrigin::ExpandedFrom {
                            source: filter_part.clone(),
                            variant_only: false,
                        },
                        AccessMode::Read,
                        resolver,
                        caps,
                        budget,
                    )?;
                    if let TargetResolution::Resolved {
                        multiply_linked: true,
                        ..
                    } = &resolved.resolution
                    {
                        *multiply_linked_file = true;
                    }
                    let res = check_target_impl(&resolved, protected, budget)?;
                    if !matches!(res.outcome, PartOutcome::Clear) {
                        outcomes.push(res);
                    }
                }
                Ok(false) => {}
                Err(unread) => {
                    outcomes.push(PartResult {
                        part: filter_part.clone(),
                        outcome: PartOutcome::Unread(unread),
                    });
                }
            }

            let is_traversable_dir = match entry.file_type {
                FileType::Dir => true,
                FileType::Symlink if follow_dir_symlinks => {
                    match resolver.stat(&entry_full, caps, budget) {
                        Ok(md) if md.file_type == FileType::Dir => {
                            match resolver.canonicalize(&entry_full, caps, budget) {
                                Ok(canon) => visited_symlink_dirs.insert(canon),
                                Err(ResolveError::Exhausted(e)) => return Err(e),
                                Err(ResolveError::CapReached(c)) => {
                                    outcomes.push(PartResult {
                                        part: filter_part.clone(),
                                        outcome: PartOutcome::Unread(UnreadReason::from(c)),
                                    });
                                    capped = true;
                                    false
                                }
                                Err(_) => false,
                            }
                        }
                        Ok(_) | Err(ResolveError::NotFound) => false,
                        Err(ResolveError::Io(_)) => {
                            outcomes.push(PartResult {
                                part: filter_part.clone(),
                                outcome: PartOutcome::Unread(UnreadReason::UnresolvablePath {
                                    path: entry_full.clone(),
                                }),
                            });
                            false
                        }
                        Err(ResolveError::CapReached(c)) => {
                            outcomes.push(PartResult {
                                part: filter_part.clone(),
                                outcome: PartOutcome::Unread(UnreadReason::from(c)),
                            });
                            capped = true;
                            false
                        }
                        Err(ResolveError::Exhausted(e)) => return Err(e),
                    }
                }
                FileType::Symlink | FileType::File | FileType::Other => false,
            };
            if capped {
                break;
            }
            if is_traversable_dir && depth + 1 < max_depth {
                stack.push((entry_full, entry_rel, depth + 1));
            } else if is_traversable_dir
                && (recursive || compiled.has_globstar)
                && !emitted_depth_cap
            {
                emitted_depth_cap = true;
                outcomes.push(PartResult {
                    part: filter_part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::WalkCapReached(FsCap::WalkDepth)),
                });
            }
        }
        if capped {
            break;
        }
    }

    // Step 2: match filter against `dev`'s probe names (case-folded for
    // probes, since probe names are stored lowercase).
    let probe_matcher = if compiled.case_insensitive {
        compiled
    } else {
        // Probes are lowercase; match case-insensitively only if filter is
        // case-insensitive, otherwise match as compiled against lowercase probes
        // (and folded pattern if pattern has uppercase).
        let raw_folded = fold_listed_component(
            filter.pattern.literal_bytes(),
            protected.folds.fold_long_s,
            protected.folds.fold_kelvin,
        );
        if raw_folded != filter.pattern.literal_bytes()
            && let Ok(folded_str) = std::str::from_utf8(&raw_folded)
        {
            let folded_filter = SearchFilter {
                dialect: filter.dialect,
                pattern: crate::policy::model::Word::derived(folded_str.as_bytes().to_vec()),
                case_insensitive: true,
                argv_index: filter.argv_index,
            };
            match compile_filter(&folded_filter, budget)? {
                Ok(c) => c,
                Err(_) => compiled,
            }
        } else {
            compiled
        }
    };

    // 2a. Hard-deny probes (best-effort probe matching per §2 & brief).
    for (probe_bytes, hard_id) in shared.hard_deny_probes() {
        let Some(rel_bytes) = under_bytes(&probe_bytes, folded_root) else {
            continue;
        };
        let Ok(rel_str) = std::str::from_utf8(rel_bytes) else {
            continue;
        };
        let name = rel_str.rsplit('/').next().unwrap_or(rel_str);
        if probe_matcher.matches_probe(rel_str, name, recursive, budget)? {
            outcomes.push(PartResult {
                part: filter_part.clone(),
                outcome: PartOutcome::HardDenyHit(Hit {
                    hard_deny_id: Some(hard_id),
                    source: HitSource::BestEffort,
                    matched: HitMatch::ProbeName {
                        probe: String::from_utf8_lossy(&probe_bytes).into_owned(),
                    },
                    part: filter_part.clone(),
                }),
            });
            break;
        }
    }

    // 2b. Store probes (CR-12 + CR-17; independent part per §5).
    let mut store_probe_hit = false;
    for (probe_bytes, _family) in shared.store_probes() {
        let Some(rel_bytes) = under_bytes(&probe_bytes, folded_root) else {
            continue;
        };
        let Ok(rel_str) = std::str::from_utf8(rel_bytes) else {
            continue;
        };
        let name = rel_str.rsplit('/').next().unwrap_or(rel_str);
        if probe_matcher.matches_probe(rel_str, name, recursive, budget)? {
            outcomes.push(PartResult {
                part: filter_part.clone(),
                outcome: PartOutcome::StoreHit(Hit {
                    hard_deny_id: None,
                    source: HitSource::Model,
                    matched: HitMatch::ProbeName {
                        probe: String::from_utf8_lossy(&probe_bytes).into_owned(),
                    },
                    part: filter_part.clone(),
                }),
            });
            store_probe_hit = true;
            break;
        }
    }

    // Also match `dev`'s ASK_TREES directory-prefix rule (`deny.rs:697-728`):
    // e.g. `Search(paths=["~"], glob=".zsh_sessions/*.foo")` targets an
    // ASK_TREE directory by prefix even when the filename suffix isn't one of
    // the fixed probe extensions.
    if !store_probe_hit
        && let Ok(pat_folded_bytes) = std::str::from_utf8(&fold_listed_component(
            filter.pattern.literal_bytes(),
            protected.folds.fold_long_s,
            protected.folds.fold_kelvin,
        ))
    {
        let pat_trimmed = pat_folded_bytes
            .trim_start_matches("./")
            .trim_start_matches('/')
            .trim_end_matches('/');
        'ask_tree: for &(t, family) in ASK_TREES {
            let t_abs = join_folded(shared.home_folded(), t.as_bytes());
            let Some(tree_rel_bytes) = under_bytes(&t_abs, folded_root) else {
                continue;
            };
            if tree_rel_bytes.is_empty() {
                continue;
            }
            let Ok(tree_rel) = std::str::from_utf8(tree_rel_bytes) else {
                continue;
            };
            for (idx, _) in pat_trimmed.match_indices('/') {
                if idx == 0 {
                    continue;
                }
                let dir_pat = &pat_trimmed[..idx];
                if let Ok(m) = build_glob_matcher(dir_pat, true, budget)? {
                    budget.charge(tree_rel.len().max(1) as u64)?;
                    if m.is_match(tree_rel) {
                        outcomes.push(PartResult {
                            part: filter_part.clone(),
                            outcome: PartOutcome::StoreHit(Hit {
                                hard_deny_id: None,
                                source: HitSource::Model,
                                matched: HitMatch::StoreFamily { family },
                                part: filter_part.clone(),
                            }),
                        });
                        break 'ask_tree;
                    }
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Followed-link walk (§5, §10 row 15, §11 item 15, M1-32 `rg -L` verdict)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn walk_followed_links(
    root_canon: &[u8],
    part: &PartId,
    has_filter: bool,
    protected: &ProtectedSet,
    shared: &SharedSensitive,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
    outcomes: &mut Vec<PartResult>,
    multiply_linked_file: &mut bool,
) -> Result<(), Exhausted> {
    budget.charge_alloc(root_canon.len().saturating_mul(2))?;
    let mut visited_paths: HashSet<Vec<u8>> = HashSet::new();
    let mut visited_ids: HashSet<FileIdentity> = HashSet::new();
    visited_paths.insert(root_canon.to_vec());
    match resolver.stat(root_canon, caps, budget) {
        Ok(md) => {
            if let Some(id) = md.identity {
                visited_ids.insert(id);
            }
        }
        Err(ResolveError::NotFound) => {}
        Err(ResolveError::Io(_)) => {
            outcomes.push(PartResult {
                part: part.clone(),
                outcome: PartOutcome::Unread(UnreadReason::UnresolvablePath {
                    path: root_canon.to_vec(),
                }),
            });
            return Ok(());
        }
        Err(ResolveError::CapReached(c)) => {
            outcomes.push(PartResult {
                part: part.clone(),
                outcome: PartOutcome::Unread(UnreadReason::from(c)),
            });
            return Ok(());
        }
        Err(ResolveError::Exhausted(e)) => return Err(e),
    }

    let mut stack: Vec<(Vec<u8>, u8)> = vec![(root_canon.to_vec(), 0)];
    let mut seen = 0usize;
    let mut emitted_depth_cap = false;

    while let Some((dir, depth)) = stack.pop() {
        let entries = match resolver.read_dir(&dir, caps, budget) {
            Ok(e) => e,
            Err(ResolveError::NotFound | ResolveError::Io(FsIoError::NotADirectory)) => continue,
            Err(ResolveError::Io(_)) => {
                outcomes.push(PartResult {
                    part: part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::UnresolvablePath { path: dir }),
                });
                continue;
            }
            Err(ResolveError::CapReached(c)) => {
                outcomes.push(PartResult {
                    part: part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::from(c)),
                });
                return Ok(());
            }
            Err(ResolveError::Exhausted(e)) => return Err(e),
        };

        for entry in entries {
            if entry.name == b"." || entry.name == b".." {
                continue;
            }
            seen += 1;
            if seen > limits::WALK_MAX_ENTRIES {
                outcomes.push(PartResult {
                    part: part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::WalkCapReached(FsCap::EntriesRead)),
                });
                return Ok(());
            }
            budget.charge_alloc(dir.len() + entry.name.len() + 1)?;
            let child = join_bytes(&dir, &entry.name);
            match entry.file_type {
                FileType::Symlink => {
                    let target = resolve_literal_impl(
                        &child,
                        &protected.cwd,
                        &protected.home,
                        TargetOrigin::ExpandedFrom {
                            source: part.clone(),
                            variant_only: false,
                        },
                        AccessMode::Read,
                        resolver,
                        caps,
                        budget,
                    )?;
                    if let TargetResolution::Resolved {
                        multiply_linked: true,
                        ..
                    } = &target.resolution
                    {
                        *multiply_linked_file = true;
                    }

                    let checked = check_target_impl(&target, protected, budget)?;
                    match checked.outcome {
                        PartOutcome::HardDenyHit(hit) => {
                            if has_filter {
                                outcomes.push(PartResult {
                                    part: part.clone(),
                                    outcome: PartOutcome::Unread(
                                        UnreadReason::FollowedLinkUnderFilter { path: child },
                                    ),
                                });
                            } else {
                                outcomes.push(PartResult {
                                    part: part.clone(),
                                    outcome: PartOutcome::HardDenyHit(hit),
                                });
                            }
                        }
                        PartOutcome::StoreHit(hit) => {
                            if has_filter {
                                outcomes.push(PartResult {
                                    part: part.clone(),
                                    outcome: PartOutcome::Unread(
                                        UnreadReason::FollowedLinkUnderFilter { path: child },
                                    ),
                                });
                            } else {
                                outcomes.push(PartResult {
                                    part: part.clone(),
                                    outcome: PartOutcome::StoreHit(hit),
                                });
                            }
                        }
                        PartOutcome::Unread(reason) => {
                            outcomes.push(PartResult {
                                part: part.clone(),
                                outcome: PartOutcome::Unread(reason),
                            });
                        }
                        PartOutcome::Clear
                        | PartOutcome::Exhausted(_)
                        | PartOutcome::TooLarge(_) => {}
                    }

                    if let TargetResolution::Resolved {
                        canonical,
                        dangling_symlink: false,
                        ..
                    } = target.resolution
                    {
                        // Also check S8 on a followed link into a folder holding credentials
                        // (e.g. `proj/link_aws -> ~/.aws` with `~/.aws/credentials` present).
                        let folded = fold_path_listed(&canonical, protected.folds);
                        let mut link_s8_outcomes = Vec::new();
                        let s8 = check_s8_protected_root(
                            &canonical,
                            &folded,
                            part,
                            protected,
                            shared,
                            resolver,
                            caps,
                            budget,
                            &mut link_s8_outcomes,
                        )?;
                        if s8 {
                            if has_filter {
                                outcomes.push(PartResult {
                                    part: part.clone(),
                                    outcome: PartOutcome::Unread(
                                        UnreadReason::FollowedLinkUnderFilter {
                                            path: canonical.clone(),
                                        },
                                    ),
                                });
                            } else {
                                outcomes.extend(link_s8_outcomes);
                            }
                        } else {
                            // Preserve any I/O or walk-cap Unreads observed while checking the linked root.
                            outcomes.extend(link_s8_outcomes);
                            if let Some(family) =
                                root_contains_store_or_exempt_sensitive(&folded, shared)
                            {
                                if has_filter {
                                    outcomes.push(PartResult {
                                        part: part.clone(),
                                        outcome: PartOutcome::Unread(
                                            UnreadReason::FollowedLinkUnderFilter {
                                                path: canonical.clone(),
                                            },
                                        ),
                                    });
                                } else {
                                    outcomes.push(PartResult {
                                        part: part.clone(),
                                        outcome: PartOutcome::StoreHit(Hit {
                                            hard_deny_id: None,
                                            source: HitSource::Model,
                                            matched: HitMatch::StoreFamily { family },
                                            part: part.clone(),
                                        }),
                                    });
                                }
                            }
                        }

                        // If the symlink points to a directory, walk inside it (cycle-guarded).
                        match resolver.stat(&canonical, caps, budget) {
                            Ok(md) if md.file_type == FileType::Dir => {
                                let fresh_path = visited_paths.insert(canonical.clone());
                                let fresh_id = md.identity.is_none_or(|id| visited_ids.insert(id));
                                if fresh_path && fresh_id {
                                    if depth + 1 >= limits::WALK_MAX_DEPTH {
                                        if !emitted_depth_cap {
                                            emitted_depth_cap = true;
                                            outcomes.push(PartResult {
                                                part: part.clone(),
                                                outcome: PartOutcome::Unread(
                                                    UnreadReason::WalkCapReached(FsCap::WalkDepth),
                                                ),
                                            });
                                        }
                                    } else {
                                        stack.push((canonical, depth + 1));
                                    }
                                }
                            }
                            Ok(_) | Err(ResolveError::NotFound) => {}
                            Err(ResolveError::Io(_)) => {
                                outcomes.push(PartResult {
                                    part: part.clone(),
                                    outcome: PartOutcome::Unread(UnreadReason::UnresolvablePath {
                                        path: canonical,
                                    }),
                                });
                            }
                            Err(ResolveError::CapReached(c)) => {
                                outcomes.push(PartResult {
                                    part: part.clone(),
                                    outcome: PartOutcome::Unread(UnreadReason::from(c)),
                                });
                                return Ok(());
                            }
                            Err(ResolveError::Exhausted(e)) => return Err(e),
                        }
                    }
                }
                FileType::Dir => {
                    if depth + 1 >= limits::WALK_MAX_DEPTH {
                        if !emitted_depth_cap {
                            emitted_depth_cap = true;
                            outcomes.push(PartResult {
                                part: part.clone(),
                                outcome: PartOutcome::Unread(UnreadReason::WalkCapReached(
                                    FsCap::WalkDepth,
                                )),
                            });
                        }
                    } else if visited_paths.insert(child.clone()) {
                        stack.push((child, depth + 1));
                    }
                }
                FileType::File | FileType::Other => {}
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Bounded hard-link walk (§2, §5, §8, §11 item 7)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn walk_hard_links(
    root_canon: &[u8],
    part: &PartId,
    has_filter: bool,
    exempt_root: bool,
    protected: &ProtectedSet,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
    outcomes: &mut Vec<PartResult>,
    multiply_linked_file: &mut bool,
) -> Result<(), Exhausted> {
    budget.charge_alloc(root_canon.len())?;
    let mut stack: Vec<(Vec<u8>, u8)> = vec![(root_canon.to_vec(), 0)];
    let mut seen = 0usize;

    while let Some((dir, depth)) = stack.pop() {
        let entries = match resolver.read_dir(&dir, caps, budget) {
            Ok(e) => e,
            Err(ResolveError::Exhausted(e)) => return Err(e),
            // Global `FsCaps` failures are Unread (§8); only the local hard-link
            // cutoff (`HARD_LINK_WALK_ENTRIES` / `HARD_LINK_WALK_DEPTH`) counts as clear.
            Err(ResolveError::CapReached(c)) => {
                outcomes.push(PartResult {
                    part: part.clone(),
                    outcome: PartOutcome::Unread(UnreadReason::from(c)),
                });
                return Ok(());
            }
            Err(ResolveError::NotFound | ResolveError::Io(_)) => continue,
        };

        for entry in entries {
            if entry.name == b"." || entry.name == b".." {
                continue;
            }
            seen += 1;
            if seen > limits::HARD_LINK_WALK_ENTRIES {
                return Ok(());
            }
            match entry.file_type {
                FileType::Dir => {
                    let skip = std::str::from_utf8(&entry.name)
                        .is_ok_and(|n| limits::HARD_LINK_WALK_SKIP_DIRS.contains(&n));
                    if !skip && depth + 1 < limits::HARD_LINK_WALK_DEPTH {
                        budget.charge_alloc(dir.len() + 1 + entry.name.len())?;
                        stack.push((join_bytes(&dir, &entry.name), depth + 1));
                    }
                }
                FileType::File => {
                    budget.charge_alloc(dir.len() + 1 + entry.name.len())?;
                    let child = join_bytes(&dir, &entry.name);
                    match resolver.lstat(&child, caps, budget) {
                        Ok(md) if md.file_type == FileType::File => {
                            if md.nlink > 1 {
                                *multiply_linked_file = true;
                                // Only multiply-linked aliases under non-exempt,
                                // unfiltered roots produce identity hits here;
                                // singly-linked descendants and S8-exempt roots
                                // (`~`, ancestors of `~`, `~/.config`) keep
                                // their S8 exemptions (§5, §10 row 16, §11 item 12).
                                if !has_filter
                                    && !exempt_root
                                    && let Some(id) = md.identity
                                {
                                    if let Some((_, hard_id)) = protected
                                        .hard_deny_identities
                                        .iter()
                                        .find(|(pid, _)| *pid == id)
                                    {
                                        outcomes.push(PartResult {
                                            part: part.clone(),
                                            outcome: PartOutcome::HardDenyHit(Hit {
                                                hard_deny_id: Some(hard_id),
                                                source: HitSource::Model,
                                                matched: HitMatch::ProtectedPath { path: child },
                                                part: part.clone(),
                                            }),
                                        });
                                    } else if let Some((_, family)) = protected
                                        .store_identities
                                        .iter()
                                        .find(|(pid, _)| *pid == id)
                                    {
                                        outcomes.push(PartResult {
                                            part: part.clone(),
                                            outcome: PartOutcome::StoreHit(Hit {
                                                hard_deny_id: None,
                                                source: HitSource::Model,
                                                matched: HitMatch::StoreFamily { family },
                                                part: part.clone(),
                                            }),
                                        });
                                    }
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(ResolveError::Exhausted(e)) => return Err(e),
                        Err(ResolveError::CapReached(c)) => {
                            outcomes.push(PartResult {
                                part: part.clone(),
                                outcome: PartOutcome::Unread(UnreadReason::from(c)),
                            });
                            return Ok(());
                        }
                        Err(ResolveError::NotFound | ResolveError::Io(_)) => {}
                    }
                }
                FileType::Symlink | FileType::Other => {}
            }
        }
    }

    Ok(())
}
