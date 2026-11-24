//! Path resolution, glob expansion, recursive reads, and sensitive-path
//! matching (owned by P2; contract §5).

use crate::policy::model::{
    AccessMode, CommandUnit, ComputeBudget, DirEntry, Exhausted, FsCaps, GlobExpansion, LinkPolicy,
    PartId, PartResult, PathMetadata, ProtectedSet, RecursiveRootResult, ResolveError,
    ResolvedCallPaths, SearchFilter, Target, TargetOrigin, ToolEffects, Word,
};

/// Filesystem abstraction used by P2; implemented by `SessionGate`'s resolver
/// (and by fake/in-memory resolvers in pure unit tests).
///
/// Every method takes both `&mut FsCaps` and `&mut ComputeBudget`:
/// - Filesystem operation caps (`stat`, `readlink`, `canonicalize`, `dir_open`,
///   `entries_read`) are charged **before** touching the filesystem; running
///   out returns `Err(ResolveError::CapReached(_))`, which callers turn into
///   `Unread(WalkCapReached)`, never `Exhausted`.
/// - Compute work and heap allocations for returned buffers (`read_link`,
///   `canonicalize`, and per-entry `read_dir` buffers) are charged against
///   `budget` **before** allocating/appending; running out returns
///   `Err(ResolveError::Exhausted(_))`.
/// - Missing paths (`ENOENT`) return `Err(ResolveError::NotFound)`, strictly
///   distinct from I/O errors (`Err(ResolveError::Io(_))`, e.g. unreadable
///   directories or permission denied) so P2 can tell a legitimate nonexistent
///   suffix from an unresolvable path.
///
/// **Owned by:** P2 (trait frozen by P0) · **Contract:** §2, §5, §8.
pub(crate) trait PathResolver {
    /// Follows symlinks (`stat`). Charges `caps.charge_stat()` and compute
    /// before touching the filesystem.
    fn stat(
        &self,
        path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<PathMetadata, ResolveError>;

    /// Does not follow a final symlink (`lstat`). Charges `caps.charge_stat()`
    /// and compute before touching the filesystem.
    fn lstat(
        &self,
        path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<PathMetadata, ResolveError>;

    /// Reads the target of a symlink (`readlink`). Charges
    /// `caps.charge_link_read()`, compute, and allocation bytes against
    /// `budget` before allocating/returning the target buffer.
    fn read_link(
        &self,
        path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<Vec<u8>, ResolveError>;

    /// Canonicalizes an existing path (`realpath` / `fs::canonicalize`).
    /// Charges `caps.charge_canonicalize()`, compute, and allocation bytes
    /// against `budget` before allocating/returning the canonical path buffer.
    fn canonicalize(
        &self,
        path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<Vec<u8>, ResolveError>;

    /// Lists entries of a directory (`read_dir`), charging
    /// `caps.charge_dir_open()`, per-entry `caps.charge_entries_read(1)`, and
    /// per-entry `budget.charge_alloc(...)` **before** pushing each entry into
    /// the returned vector.
    fn read_dir(
        &self,
        path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<Vec<DirEntry>, ResolveError>;
}

/// Resolves one literal path (checking process-dependent paths, expanding
/// modeled `~`, canonicalizing the deepest existing ancestor, following up to
/// `limits::MAX_HOPS` symlinks, counting dangling links under their own name,
/// and collecting `(dev, ino)` identities of the path and existing ancestors).
///
/// **Owned by:** P2 · **Contract:** §3, §5, §8.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_literal(
    _raw: &[u8],
    _cwd: &[u8],
    _home: &[u8],
    _origin: TargetOrigin,
    _access: AccessMode,
    _resolver: &dyn PathResolver,
    _caps: &mut FsCaps,
    _budget: &mut ComputeBudget,
) -> Result<Target, Exhausted> {
    todo!("P2: implement §5 literal path resolution")
}

/// Expands an unquoted shell glob against `cwd` via `resolver`, separating
/// intersection matches (`sh`/`bash`/`zsh` defaults) from dialect-variant-only
/// matches (dot-matching, recursive `**`).
///
/// **Owned by:** P2 · **Contract:** §5, §8.
#[allow(clippy::too_many_arguments)]
pub(crate) fn expand_shell_glob(
    _pattern: &Word,
    _cwd: &[u8],
    _home: &[u8],
    _source: PartId,
    _access: AccessMode,
    _resolver: &dyn PathResolver,
    _caps: &mut FsCaps,
    _budget: &mut ComputeBudget,
) -> Result<GlobExpansion, Exhausted> {
    todo!("P2: implement §5 shell glob expansion")
}

/// Checks one resolved (or unread) [`Target`] against `protected`: identities,
/// listed name folds, and unlisted volume equivalences (§11 item 13).
///
/// **Owned by:** P2 · **Contract:** §5, §8.
pub(crate) fn check_target(
    _target: &Target,
    _protected: &ProtectedSet,
    _budget: &mut ComputeBudget,
) -> Result<PartResult, Exhausted> {
    todo!("P2: implement §5 target matching against protected set")
}

/// Evaluates a recursive read root: S8 protected-root policy (with home,
/// ancestors of home, and `~/.config` exceptions, plus ancestors of Atrium data
/// paths), filter enumeration + store probes, followed-link walk, and the
/// bounded hard-link walk (returning both per-part outcomes and whether any
/// multiply-linked regular file was encountered).
///
/// **Owned by:** P2 · **Contract:** §1 step 4, §2, §5, §8.
pub(crate) fn check_recursive_root(
    _root: &Target,
    _links: LinkPolicy,
    _filters: &[SearchFilter],
    _protected: &ProtectedSet,
    _resolver: &dyn PathResolver,
    _caps: &mut FsCaps,
    _budget: &mut ComputeBudget,
) -> Result<RecursiveRootResult, Exhausted> {
    todo!("P2: implement §5 recursive root, filter, link, and hard-link checks")
}

/// Resolves and checks all targets of a sequence of modeled command units,
/// tracking modeled `cd`, resolving filesystem-dependent derived destinations
/// (`DerivedDestinations::WhenSourceIsDir`, e.g. `mv` of a directory), marking
/// targets after a possible writer or inside a pipeline containing a possible
/// writer as `Unread`, and keeping hard-deny hits.
///
/// **Owned by:** P2 · **Contract:** §5, §8.
pub(crate) fn resolve_command_targets(
    _units: &[CommandUnit],
    _effects: &[ToolEffects],
    _start_cwd: &[u8],
    _protected: &ProtectedSet,
    _resolver: &dyn PathResolver,
    _caps: &mut FsCaps,
    _budget: &mut ComputeBudget,
) -> Result<ResolvedCallPaths, Exhausted> {
    todo!("P2: implement §5 command-line path resolution and intra-call writer tracking")
}

/// Resolves and checks paths and search selectors/filters for a file-tool
/// (`Read`/`Write`/`Edit`/`Search`/`Glob`) subject.
///
/// **Owned by:** P2 · **Contract:** §5, §8.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_subject_paths(
    _paths: &[String],
    _glob: Option<&str>,
    _recursive: bool,
    _access: AccessMode,
    _protected: &ProtectedSet,
    _resolver: &dyn PathResolver,
    _caps: &mut FsCaps,
    _budget: &mut ComputeBudget,
) -> Result<ResolvedCallPaths, Exhausted> {
    todo!("P2: implement §5 file-tool and search-tool path resolution")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::policy::model::{
        CapReached, ExhaustReason, FileIdentity, FileType, FsCap, FsIoError, PartOutcome,
        UnreadReason,
    };

    enum FakeNode {
        File {
            meta: PathMetadata,
        },
        Symlink {
            meta: PathMetadata,
            target: Vec<u8>,
        },
        Dir {
            meta: PathMetadata,
            entries: Vec<DirEntry>,
        },
        Inaccessible(FsIoError),
    }

    struct FakeResolver {
        nodes: HashMap<Vec<u8>, FakeNode>,
    }

    impl FakeResolver {
        fn fixture() -> Self {
            let mut nodes = HashMap::new();
            nodes.insert(
                b"/proj".to_vec(),
                FakeNode::Dir {
                    meta: PathMetadata {
                        file_type: FileType::Dir,
                        identity: Some(FileIdentity { dev: 1, ino: 10 }),
                        nlink: 2,
                    },
                    entries: vec![
                        DirEntry {
                            name: b"a.txt".to_vec(),
                            file_type: FileType::File,
                        },
                        DirEntry {
                            name: b"b.txt".to_vec(),
                            file_type: FileType::File,
                        },
                        DirEntry {
                            name: b"hardlinked.txt".to_vec(),
                            file_type: FileType::File,
                        },
                    ],
                },
            );
            nodes.insert(
                b"/proj/hardlinked.txt".to_vec(),
                FakeNode::File {
                    meta: PathMetadata {
                        file_type: FileType::File,
                        identity: Some(FileIdentity { dev: 1, ino: 11 }),
                        nlink: 2,
                    },
                },
            );
            nodes.insert(
                b"/proj/link".to_vec(),
                FakeNode::Symlink {
                    meta: PathMetadata {
                        file_type: FileType::Symlink,
                        identity: Some(FileIdentity { dev: 1, ino: 12 }),
                        nlink: 1,
                    },
                    target: b"/proj/hardlinked.txt".to_vec(),
                },
            );
            nodes.insert(
                b"/secret_dir".to_vec(),
                FakeNode::Inaccessible(FsIoError::PermissionDenied),
            );
            Self { nodes }
        }
    }

    impl PathResolver for FakeResolver {
        fn stat(
            &self,
            path: &[u8],
            caps: &mut FsCaps,
            budget: &mut ComputeBudget,
        ) -> Result<PathMetadata, ResolveError> {
            caps.charge_stat()?;
            budget.charge(path.len() as u64)?;
            match self.nodes.get(path) {
                None => Err(ResolveError::NotFound),
                Some(FakeNode::Inaccessible(err)) => Err(ResolveError::Io(*err)),
                Some(FakeNode::File { meta } | FakeNode::Dir { meta, .. }) => Ok(*meta),
                Some(FakeNode::Symlink { target, .. }) => match self.nodes.get(target) {
                    Some(FakeNode::File { meta } | FakeNode::Dir { meta, .. }) => Ok(*meta),
                    Some(FakeNode::Inaccessible(err)) => Err(ResolveError::Io(*err)),
                    _ => Err(ResolveError::NotFound),
                },
            }
        }

        fn lstat(
            &self,
            path: &[u8],
            caps: &mut FsCaps,
            budget: &mut ComputeBudget,
        ) -> Result<PathMetadata, ResolveError> {
            caps.charge_stat()?;
            budget.charge(path.len() as u64)?;
            match self.nodes.get(path) {
                None => Err(ResolveError::NotFound),
                Some(FakeNode::Inaccessible(err)) => Err(ResolveError::Io(*err)),
                Some(
                    FakeNode::File { meta }
                    | FakeNode::Dir { meta, .. }
                    | FakeNode::Symlink { meta, .. },
                ) => Ok(*meta),
            }
        }

        fn read_link(
            &self,
            path: &[u8],
            caps: &mut FsCaps,
            budget: &mut ComputeBudget,
        ) -> Result<Vec<u8>, ResolveError> {
            caps.charge_link_read()?;
            budget.charge(path.len() as u64)?;
            match self.nodes.get(path) {
                None => Err(ResolveError::NotFound),
                Some(FakeNode::Inaccessible(err)) => Err(ResolveError::Io(*err)),
                Some(FakeNode::Symlink { target, .. }) => {
                    budget.charge_alloc(target.len())?;
                    Ok(target.clone())
                }
                Some(_) => Err(ResolveError::Io(FsIoError::NotASymlink)),
            }
        }

        fn canonicalize(
            &self,
            path: &[u8],
            caps: &mut FsCaps,
            budget: &mut ComputeBudget,
        ) -> Result<Vec<u8>, ResolveError> {
            caps.charge_canonicalize()?;
            budget.charge(path.len() as u64)?;
            match self.nodes.get(path) {
                None => Err(ResolveError::NotFound),
                Some(FakeNode::Inaccessible(err)) => Err(ResolveError::Io(*err)),
                Some(FakeNode::Symlink { target, .. }) => {
                    budget.charge_alloc(target.len())?;
                    Ok(target.clone())
                }
                Some(_) => {
                    budget.charge_alloc(path.len())?;
                    Ok(path.to_vec())
                }
            }
        }

        fn read_dir(
            &self,
            path: &[u8],
            caps: &mut FsCaps,
            budget: &mut ComputeBudget,
        ) -> Result<Vec<DirEntry>, ResolveError> {
            caps.charge_dir_open()?;
            budget.charge(path.len() as u64)?;
            match self.nodes.get(path) {
                None => Err(ResolveError::NotFound),
                Some(FakeNode::Inaccessible(err)) => Err(ResolveError::Io(*err)),
                Some(FakeNode::Dir { entries, .. }) => {
                    let mut out = Vec::new();
                    for entry in entries {
                        caps.charge_entries_read(1)?;
                        budget.charge(entry.name.len() as u64)?;
                        budget.charge_alloc(std::mem::size_of::<DirEntry>() + entry.name.len())?;
                        out.push(entry.clone());
                    }
                    Ok(out)
                }
                Some(_) => Err(ResolveError::Io(FsIoError::NotADirectory)),
            }
        }
    }

    #[test]
    fn resolver_distinguishes_missing_paths_from_io_errors() {
        let resolver = FakeResolver::fixture();
        let mut caps = FsCaps::with_default_limits();
        let mut budget = ComputeBudget::with_default_limits();

        assert_eq!(
            resolver.stat(b"/proj/nonexistent", &mut caps, &mut budget),
            Err(ResolveError::NotFound)
        );
        assert_eq!(
            resolver.stat(b"/secret_dir", &mut caps, &mut budget),
            Err(ResolveError::Io(FsIoError::PermissionDenied))
        );
        assert_eq!(
            resolver.read_dir(b"/proj/nonexistent", &mut caps, &mut budget),
            Err(ResolveError::NotFound)
        );
        assert_eq!(
            resolver.read_dir(b"/secret_dir", &mut caps, &mut budget),
            Err(ResolveError::Io(FsIoError::PermissionDenied))
        );
        assert_eq!(
            resolver.read_dir(b"/proj/hardlinked.txt", &mut caps, &mut budget),
            Err(ResolveError::Io(FsIoError::NotADirectory))
        );
        assert_eq!(
            resolver.read_link(b"/proj/hardlinked.txt", &mut caps, &mut budget),
            Err(ResolveError::Io(FsIoError::NotASymlink))
        );
    }

    #[test]
    fn resolver_precharges_allocations_and_fs_caps() {
        let resolver = FakeResolver::fixture();

        // Exhausted allocation budget fails read_link, canonicalize, and read_dir with Exhausted.
        let mut caps = FsCaps::with_default_limits();
        let mut tiny_alloc = ComputeBudget::new(10_000, 8);
        assert_eq!(
            resolver.read_link(b"/proj/link", &mut caps, &mut tiny_alloc),
            Err(ResolveError::Exhausted(Exhausted {
                reason: ExhaustReason::AllocationBudget
            }))
        );
        assert!(tiny_alloc.is_exhausted());

        let mut tiny_canon = ComputeBudget::new(10_000, 4);
        assert_eq!(
            resolver.canonicalize(b"/proj/hardlinked.txt", &mut caps, &mut tiny_canon),
            Err(ResolveError::Exhausted(Exhausted {
                reason: ExhaustReason::AllocationBudget
            }))
        );

        let mut tiny_dir = ComputeBudget::new(10_000, std::mem::size_of::<DirEntry>() + 6);
        assert_eq!(
            resolver.read_dir(b"/proj", &mut caps, &mut tiny_dir),
            Err(ResolveError::Exhausted(Exhausted {
                reason: ExhaustReason::AllocationBudget
            }))
        );

        // Exhausted FsCaps returns CapReached (which turns into Unread, not Exhausted).
        let mut tiny_caps = FsCaps::new(1, 1, 10, 10, 10);
        let mut plenty = ComputeBudget::with_default_limits();
        let err = resolver
            .read_dir(b"/proj", &mut tiny_caps, &mut plenty)
            .unwrap_err();
        assert_eq!(
            err,
            ResolveError::CapReached(CapReached {
                cap: FsCap::EntriesRead
            })
        );
        assert_eq!(
            UnreadReason::from(CapReached {
                cap: FsCap::EntriesRead
            }),
            UnreadReason::WalkCapReached(FsCap::EntriesRead)
        );
        assert!(!plenty.is_exhausted());
    }

    #[test]
    fn recursive_root_result_carries_outcomes_and_multiply_linked_flag() {
        let resolver = FakeResolver::fixture();
        let mut caps = FsCaps::with_default_limits();
        let mut budget = ComputeBudget::with_default_limits();
        let meta = resolver
            .lstat(b"/proj/hardlinked.txt", &mut caps, &mut budget)
            .unwrap();
        assert!(meta.file_type == FileType::File && meta.nlink > 1);

        let res = RecursiveRootResult {
            outcomes: vec![PartResult {
                part: PartId::SubjectPath { path_index: 0 },
                outcome: PartOutcome::Clear,
            }],
            multiply_linked_file: meta.nlink > 1,
        };
        assert_eq!(res.outcomes.len(), 1);
        assert!(res.multiply_linked_file);
    }
}
