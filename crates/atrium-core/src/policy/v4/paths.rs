//! Path resolution, glob expansion, recursive reads, and sensitive-path
//! matching (owned by P2; contract §5).

use crate::policy::model::{
    AccessMode, CapReached, CommandUnit, ComputeBudget, DirEntry, Exhausted, FsCaps, GlobExpansion,
    LinkPolicy, PartId, PartResult, PathMetadata, ProtectedSet, ResolvedCallPaths, SearchFilter,
    Target, TargetOrigin, ToolEffects, Word,
};

/// Filesystem abstraction used by P2; implemented by `SessionGate`'s resolver
/// (and by fake/in-memory resolvers in pure unit tests).
///
/// Every method takes `&mut FsCaps` and charges the corresponding cap
/// **before** touching the filesystem. Running out returns `Err(CapReached)`,
/// which callers turn into `Unread`, never `Exhausted`.
///
/// **Owned by:** P2 (trait frozen by P0) · **Contract:** §5, §8.
pub(crate) trait PathResolver {
    /// Follows symlinks (`stat`). Returns `Ok(None)` if the path does not exist
    /// or cannot be accessed.
    fn stat(&self, path: &[u8], caps: &mut FsCaps) -> Result<Option<PathMetadata>, CapReached>;

    /// Does not follow a final symlink (`lstat`). Returns `Ok(None)` if the
    /// path does not exist or cannot be accessed.
    fn lstat(&self, path: &[u8], caps: &mut FsCaps) -> Result<Option<PathMetadata>, CapReached>;

    /// Reads the target of a symlink (`readlink`). Returns `Ok(None)` if not a
    /// symlink or unreadable.
    fn read_link(&self, path: &[u8], caps: &mut FsCaps) -> Result<Option<Vec<u8>>, CapReached>;

    /// Canonicalizes an existing path (`realpath` / `fs::canonicalize`).
    /// Returns `Ok(None)` if the path does not exist or cannot be canonicalized.
    fn canonicalize(&self, path: &[u8], caps: &mut FsCaps) -> Result<Option<Vec<u8>>, CapReached>;

    /// Lists entries of a directory (`read_dir`), charging directory open and
    /// entries read against `caps`. Returns `Ok(None)` if unreadable.
    fn read_dir(&self, path: &[u8], caps: &mut FsCaps)
    -> Result<Option<Vec<DirEntry>>, CapReached>;
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
/// bounded hard-link walk.
///
/// **Owned by:** P2 · **Contract:** §2, §5, §8.
pub(crate) fn check_recursive_root(
    _root: &Target,
    _links: LinkPolicy,
    _filters: &[SearchFilter],
    _protected: &ProtectedSet,
    _resolver: &dyn PathResolver,
    _caps: &mut FsCaps,
    _budget: &mut ComputeBudget,
) -> Result<Vec<PartResult>, Exhausted> {
    todo!("P2: implement §5 recursive root, filter, link, and hard-link checks")
}

/// Resolves and checks all targets of a sequence of modeled command units,
/// tracking modeled `cd`, marking targets after a possible writer or inside a
/// pipeline containing a possible writer as `Unread`, and keeping hard-deny
/// hits.
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
