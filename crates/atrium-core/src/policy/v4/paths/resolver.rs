//! Filesystem resolver implementations for rev 4 path checking (contract §2, §5, §8).
//!
//! - [`FsResolver`]: real-filesystem implementation of [`PathResolver`],
//!   precharging [`FsCaps`] and [`ComputeBudget`] before every operation and
//!   distinguishing absence (`ResolveError::NotFound`) from I/O errors
//!   (`ResolveError::Io`).
//! - [`fake`] (`#[cfg(test)]`): rich in-memory fake filesystem supporting
//!   symlinks, dangling links, loops, hard links, unreadable directories,
//!   case-insensitive volumes, NFC/NFD-equivalent volumes, and non-UTF-8 names.

use std::io::ErrorKind;
use std::path::Path;

use crate::policy::model::{
    ComputeBudget, DirEntry, FileIdentity, FileType, FsCaps, FsIoError, PathMetadata, ResolveError,
};
use crate::policy::v4::paths::PathResolver;

/// Real-filesystem [`PathResolver`] backed by `std::fs` (contract §2, §5, §8).
///
/// Charges filesystem operation caps ([`FsCaps`]) and the shared
/// [`ComputeBudget`] before each operation, and keeps `ENOENT`
/// (`ResolveError::NotFound`) strictly distinct from non-absence I/O errors
/// (`ResolveError::Io`).
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct FsResolver;

impl FsResolver {
    pub(crate) const fn new() -> Self {
        Self
    }
}

#[cfg(unix)]
fn bytes_to_path(bytes: &[u8]) -> Result<&Path, ResolveError> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    if bytes.contains(&0) {
        return Err(ResolveError::Io(FsIoError::Other));
    }
    Ok(Path::new(OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
fn bytes_to_path(bytes: &[u8]) -> Result<&Path, ResolveError> {
    if bytes.contains(&0) {
        return Err(ResolveError::Io(FsIoError::Other));
    }
    let s = std::str::from_utf8(bytes).map_err(|_| ResolveError::Io(FsIoError::Other))?;
    Ok(Path::new(s))
}

#[cfg(unix)]
fn path_to_bytes(path: &Path) -> &[u8] {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes()
}

#[cfg(not(unix))]
fn path_to_bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_encoded_bytes()
}

fn map_file_type(ft: std::fs::FileType) -> FileType {
    if ft.is_symlink() {
        FileType::Symlink
    } else if ft.is_dir() {
        FileType::Dir
    } else if ft.is_file() {
        FileType::File
    } else {
        FileType::Other
    }
}

fn map_metadata(md: &std::fs::Metadata) -> PathMetadata {
    let file_type = map_file_type(md.file_type());
    #[cfg(unix)]
    let (identity, nlink) = {
        use std::os::unix::fs::MetadataExt;
        (
            Some(FileIdentity {
                dev: md.dev(),
                ino: md.ino(),
            }),
            md.nlink(),
        )
    };
    #[cfg(not(unix))]
    let (identity, nlink) = (None, 1);
    PathMetadata {
        file_type,
        identity,
        nlink,
    }
}

fn map_io_error(err: &std::io::Error, for_readlink: bool) -> ResolveError {
    match err.kind() {
        ErrorKind::NotFound => ResolveError::NotFound,
        ErrorKind::PermissionDenied => ResolveError::Io(FsIoError::PermissionDenied),
        ErrorKind::NotADirectory => ResolveError::Io(FsIoError::NotADirectory),
        ErrorKind::FilesystemLoop => ResolveError::Io(FsIoError::SymlinkLoop),
        ErrorKind::InvalidInput if for_readlink => ResolveError::Io(FsIoError::NotASymlink),
        _ => {
            // Fallback for platform-specific raw errno values if ErrorKind didn't classify them.
            #[cfg(unix)]
            if let Some(code) = err.raw_os_error() {
                if for_readlink && code == rustix::io::Errno::INVAL.raw_os_error() {
                    return ResolveError::Io(FsIoError::NotASymlink);
                }
                if code == rustix::io::Errno::LOOP.raw_os_error() {
                    return ResolveError::Io(FsIoError::SymlinkLoop);
                }
                if code == rustix::io::Errno::NOTDIR.raw_os_error() {
                    return ResolveError::Io(FsIoError::NotADirectory);
                }
                if code == rustix::io::Errno::ACCES.raw_os_error()
                    || code == rustix::io::Errno::PERM.raw_os_error()
                {
                    return ResolveError::Io(FsIoError::PermissionDenied);
                }
            }
            ResolveError::Io(FsIoError::Other)
        }
    }
}

impl PathResolver for FsResolver {
    fn stat(
        &self,
        path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<PathMetadata, ResolveError> {
        caps.charge_stat()?;
        budget.charge(path.len().max(1) as u64)?;
        let p = bytes_to_path(path)?;
        let md = std::fs::metadata(p).map_err(|e| map_io_error(&e, false))?;
        Ok(map_metadata(&md))
    }

    fn lstat(
        &self,
        path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<PathMetadata, ResolveError> {
        caps.charge_stat()?;
        budget.charge(path.len().max(1) as u64)?;
        let p = bytes_to_path(path)?;
        let md = std::fs::symlink_metadata(p).map_err(|e| map_io_error(&e, false))?;
        Ok(map_metadata(&md))
    }

    fn read_link(
        &self,
        path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<Vec<u8>, ResolveError> {
        caps.charge_link_read()?;
        budget.charge(path.len().max(1) as u64)?;
        if budget.remaining_alloc_bytes() == 0 {
            budget.charge_alloc(1)?;
        }
        let p = bytes_to_path(path)?;
        let target = std::fs::read_link(p).map_err(|e| map_io_error(&e, true))?;
        let bytes = path_to_bytes(&target);
        budget.charge_alloc(bytes.len())?;
        Ok(bytes.to_vec())
    }

    fn canonicalize(
        &self,
        path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<Vec<u8>, ResolveError> {
        caps.charge_canonicalize()?;
        budget.charge(path.len().max(1) as u64)?;
        if budget.remaining_alloc_bytes() == 0 {
            budget.charge_alloc(1)?;
        }
        let p = bytes_to_path(path)?;
        let canon = std::fs::canonicalize(p).map_err(|e| map_io_error(&e, false))?;
        let bytes = path_to_bytes(&canon);
        budget.charge_alloc(bytes.len())?;
        Ok(bytes.to_vec())
    }

    fn read_dir(
        &self,
        path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<Vec<DirEntry>, ResolveError> {
        caps.charge_dir_open()?;
        budget.charge(path.len().max(1) as u64)?;
        let p = bytes_to_path(path)?;
        let rd = std::fs::read_dir(p).map_err(|e| map_io_error(&e, false))?;
        let mut out = Vec::new();
        for entry_res in rd {
            caps.charge_entries_read(1)?;
            let entry = entry_res.map_err(|e| map_io_error(&e, false))?;
            let os_name = entry.file_name();
            let name_bytes = path_to_bytes(Path::new(&os_name));
            budget.charge(name_bytes.len().max(1) as u64)?;
            budget.charge_alloc(std::mem::size_of::<DirEntry>() + name_bytes.len())?;
            let ft = entry.file_type().map_err(|e| map_io_error(&e, false))?;
            out.push(DirEntry {
                name: name_bytes.to_vec(),
                file_type: map_file_type(ft),
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! In-memory fake [`PathResolver`] for deterministic unit testing of
    //! symlinks, dangling links, loops, hard links, unreadable folders,
    //! case-variant names, and NFC/NFD-variant names.

    use std::collections::{BTreeMap, VecDeque};

    use crate::policy::limits;
    use crate::policy::model::{
        ComputeBudget, DirEntry, FileIdentity, FileType, FsCaps, FsIoError, PathMetadata,
        ResolveError,
    };
    use crate::policy::v4::paths::PathResolver;
    use crate::policy::v4::paths::sensitive::{fold_extended_component, fold_listed_component};

    #[derive(Debug, Clone)]
    enum InodeKind {
        File,
        Symlink {
            target: Vec<u8>,
        },
        Dir {
            /// Stored on-disk entry names mapped to child inode ids.
            entries: BTreeMap<Vec<u8>, u64>,
            /// Optional I/O error returned on lookup/stat inside this dir or `read_dir`.
            read_error: Option<FsIoError>,
            /// Optional I/O error returned specifically on traversing through this dir.
            exec_error: Option<FsIoError>,
        },
    }

    #[derive(Debug, Clone)]
    struct Inode {
        id: FileIdentity,
        nlink: u64,
        kind: InodeKind,
        stat_error: Option<FsIoError>,
    }

    /// In-memory POSIX-like filesystem implementing [`PathResolver`].
    #[derive(Debug, Clone)]
    pub(crate) struct FakeFs {
        dev: u64,
        next_ino: u64,
        inodes: BTreeMap<u64, Inode>,
        root_ino: u64,
        /// When `true`, component lookup folds Unicode case + `ſ`→`s` + `K`→`k`
        /// (simulating a case-insensitive APFS/HFS+ volume) while keeping the
        /// stored on-disk spelling as the canonical name.
        pub(crate) case_insensitive: bool,
        /// When `true`, component lookup also equates NFC/NFD and unlisted
        /// Unicode case folds (simulating APFS/HFS+ normalization).
        pub(crate) nfc_nfd_equivalent: bool,
    }

    impl Default for FakeFs {
        fn default() -> Self {
            Self::new()
        }
    }

    impl FakeFs {
        pub(crate) fn new() -> Self {
            let mut inodes = BTreeMap::new();
            let root_ino = 1;
            inodes.insert(
                root_ino,
                Inode {
                    id: FileIdentity {
                        dev: 1,
                        ino: root_ino,
                    },
                    nlink: 2,
                    kind: InodeKind::Dir {
                        entries: BTreeMap::new(),
                        read_error: None,
                        exec_error: None,
                    },
                    stat_error: None,
                },
            );
            Self {
                dev: 1,
                next_ino: 2,
                inodes,
                root_ino,
                case_insensitive: false,
                nfc_nfd_equivalent: false,
            }
        }

        pub(crate) fn case_insensitive() -> Self {
            let mut fs = Self::new();
            fs.case_insensitive = true;
            fs
        }

        pub(crate) fn nfc_nfd_insensitive() -> Self {
            let mut fs = Self::new();
            fs.case_insensitive = true;
            fs.nfc_nfd_equivalent = true;
            fs
        }

        fn alloc_ino(&mut self) -> u64 {
            let ino = self.next_ino;
            self.next_ino += 1;
            ino
        }

        fn component_matches(&self, stored: &[u8], query: &[u8]) -> bool {
            if stored == query {
                return true;
            }
            if self.nfc_nfd_equivalent {
                return fold_extended_component(stored) == fold_extended_component(query);
            }
            if self.case_insensitive {
                return fold_listed_component(stored, true, true)
                    == fold_listed_component(query, true, true);
            }
            false
        }

        fn lookup_child(
            &self,
            dir_ino: u64,
            name: &[u8],
        ) -> Result<Option<(Vec<u8>, u64)>, FsIoError> {
            let inode = self.inodes.get(&dir_ino).ok_or(FsIoError::Other)?;
            match &inode.kind {
                InodeKind::Dir {
                    entries,
                    exec_error,
                    ..
                } => {
                    if let Some(err) = exec_error {
                        return Err(*err);
                    }
                    if let Some(&child) = entries.get(name) {
                        return Ok(Some((name.to_vec(), child)));
                    }
                    if self.case_insensitive || self.nfc_nfd_equivalent {
                        for (k, &v) in entries {
                            if self.component_matches(k, name) {
                                return Ok(Some((k.clone(), v)));
                            }
                        }
                    }
                    Ok(None)
                }
                _ => Err(FsIoError::NotADirectory),
            }
        }

        /// Creates a directory and all missing parent directories, returning its
        /// [`FileIdentity`].
        pub(crate) fn mkdir_all(&mut self, path: &[u8]) -> FileIdentity {
            let mut cur = self.root_ino;
            for seg in split_abs(path) {
                if seg.is_empty() || seg == b"." {
                    continue;
                }
                let existing = match self.lookup_child(cur, seg) {
                    Ok(v) => v,
                    Err(e) => panic!(
                        "mkdir_all failed at {:?}: {e:?}",
                        String::from_utf8_lossy(seg)
                    ),
                };
                if let Some((_, child)) = existing {
                    cur = child;
                } else {
                    let ino = self.alloc_ino();
                    self.inodes.insert(
                        ino,
                        Inode {
                            id: FileIdentity { dev: self.dev, ino },
                            nlink: 2,
                            kind: InodeKind::Dir {
                                entries: BTreeMap::new(),
                                read_error: None,
                                exec_error: None,
                            },
                            stat_error: None,
                        },
                    );
                    if let Some(Inode {
                        kind: InodeKind::Dir { entries, .. },
                        ..
                    }) = self.inodes.get_mut(&cur)
                    {
                        entries.insert(seg.to_vec(), ino);
                    }
                    cur = ino;
                }
            }
            self.inodes[&cur].id
        }

        /// Creates a regular file (creating parent directories as needed) and
        /// returns its [`FileIdentity`].
        pub(crate) fn add_file(&mut self, path: &[u8]) -> FileIdentity {
            let (parent, name) = split_parent_name(path);
            let parent_id = self.mkdir_all(parent);
            let ino = self.alloc_ino();
            let id = FileIdentity { dev: self.dev, ino };
            self.inodes.insert(
                ino,
                Inode {
                    id,
                    nlink: 1,
                    kind: InodeKind::File,
                    stat_error: None,
                },
            );
            if let Some(Inode {
                kind: InodeKind::Dir { entries, .. },
                ..
            }) = self.inodes.get_mut(&parent_id.ino)
            {
                entries.insert(name.to_vec(), ino);
            }
            id
        }

        /// Creates a symlink at `path` pointing to `target` (creating parent
        /// directories as needed) and returns the symlink's own [`FileIdentity`].
        pub(crate) fn add_symlink(&mut self, path: &[u8], target: &[u8]) -> FileIdentity {
            let (parent, name) = split_parent_name(path);
            let parent_id = self.mkdir_all(parent);
            let ino = self.alloc_ino();
            let id = FileIdentity { dev: self.dev, ino };
            self.inodes.insert(
                ino,
                Inode {
                    id,
                    nlink: 1,
                    kind: InodeKind::Symlink {
                        target: target.to_vec(),
                    },
                    stat_error: None,
                },
            );
            if let Some(Inode {
                kind: InodeKind::Dir { entries, .. },
                ..
            }) = self.inodes.get_mut(&parent_id.ino)
            {
                entries.insert(name.to_vec(), ino);
            }
            id
        }

        /// Creates a hard link at `link_path` referencing the existing inode at
        /// `existing_path`, incrementing `nlink`.
        pub(crate) fn add_hard_link(
            &mut self,
            existing_path: &[u8],
            link_path: &[u8],
        ) -> FileIdentity {
            let existing_ino = self
                .walk_internal(existing_path, false)
                .expect("hard-link source must exist")
                .0;
            let (parent, name) = split_parent_name(link_path);
            let parent_id = self.mkdir_all(parent);
            if let Some(inode) = self.inodes.get_mut(&existing_ino) {
                inode.nlink += 1;
            }
            if let Some(Inode {
                kind: InodeKind::Dir { entries, .. },
                ..
            }) = self.inodes.get_mut(&parent_id.ino)
            {
                entries.insert(name.to_vec(), existing_ino);
            }
            self.inodes[&existing_ino].id
        }

        /// Marks a directory unreadable (`read_dir` and child traversal return
        /// `err`, matching `chmod 000`).
        pub(crate) fn set_dir_unreadable(&mut self, path: &[u8], err: FsIoError) {
            let id = self.mkdir_all(path);
            if let Some(Inode {
                kind:
                    InodeKind::Dir {
                        read_error,
                        exec_error,
                        ..
                    },
                ..
            }) = self.inodes.get_mut(&id.ino)
            {
                *read_error = Some(err);
                *exec_error = Some(err);
            }
        }

        /// Marks a directory's `read_dir` unreadable while still allowing child
        /// `stat`/`lstat` (matching `chmod 0111`).
        pub(crate) fn set_dir_read_error(&mut self, path: &[u8], err: FsIoError) {
            let id = self.mkdir_all(path);
            if let Some(Inode {
                kind: InodeKind::Dir { read_error, .. },
                ..
            }) = self.inodes.get_mut(&id.ino)
            {
                *read_error = Some(err);
            }
        }

        /// Forces `stat`/`lstat` directly on `path` to fail with `err`.
        pub(crate) fn set_stat_error(&mut self, path: &[u8], err: FsIoError) {
            let ino = self
                .walk_internal(path, false)
                .expect("set_stat_error target must exist")
                .0;
            if let Some(inode) = self.inodes.get_mut(&ino) {
                inode.stat_error = Some(err);
            }
        }

        /// Walks `path` in the in-memory tree. Returns `(ino, canonical_path_bytes)`.
        fn walk_internal(
            &self,
            path: &[u8],
            follow_final: bool,
        ) -> Result<(u64, Vec<u8>), ResolveError> {
            if !path.starts_with(b"/") || path.contains(&0) {
                return Err(ResolveError::Io(FsIoError::Other));
            }
            let mut queue: VecDeque<Vec<u8>> = split_abs(path)
                .into_iter()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_vec())
                .collect();
            let mut stack: Vec<(u64, Vec<u8>)> = vec![(self.root_ino, Vec::new())];
            let mut hops: u8 = 0;

            while let Some(seg) = queue.pop_front() {
                if seg == b"." {
                    continue;
                }
                if seg == b".." {
                    if stack.len() > 1 {
                        stack.pop();
                    }
                    continue;
                }
                let cur_ino = stack.last().expect("root always present").0;
                let child = match self.lookup_child(cur_ino, &seg) {
                    Ok(Some(c)) => c,
                    Ok(None) => return Err(ResolveError::NotFound),
                    Err(e) => return Err(ResolveError::Io(e)),
                };
                let (stored_name, child_ino) = child;
                let inode = self.inodes.get(&child_ino).ok_or(ResolveError::NotFound)?;
                let is_last = queue.is_empty();
                if let InodeKind::Symlink { target } = &inode.kind
                    && (!is_last || follow_final)
                {
                    if hops >= limits::MAX_HOPS {
                        return Err(ResolveError::Io(FsIoError::SymlinkLoop));
                    }
                    hops += 1;
                    if target.starts_with(b"/") {
                        stack.truncate(1);
                    }
                    let target_segs: Vec<Vec<u8>> = split_abs(target)
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_vec())
                        .collect();
                    for s in target_segs.into_iter().rev() {
                        queue.push_front(s);
                    }
                    continue;
                }
                stack.push((child_ino, stored_name));
            }

            let final_ino = stack.last().expect("root always present").0;
            let mut canon = Vec::new();
            if stack.len() == 1 {
                canon.push(b'/');
            } else {
                for (_, name) in &stack[1..] {
                    canon.push(b'/');
                    canon.extend_from_slice(name);
                }
            }
            Ok((final_ino, canon))
        }

        fn inode_meta(&self, ino: u64) -> Result<PathMetadata, ResolveError> {
            let inode = self.inodes.get(&ino).ok_or(ResolveError::NotFound)?;
            if let Some(err) = inode.stat_error {
                return Err(ResolveError::Io(err));
            }
            let file_type = match inode.kind {
                InodeKind::File => FileType::File,
                InodeKind::Dir { .. } => FileType::Dir,
                InodeKind::Symlink { .. } => FileType::Symlink,
            };
            Ok(PathMetadata {
                file_type,
                identity: Some(inode.id),
                nlink: inode.nlink,
            })
        }
    }

    fn split_abs(path: &[u8]) -> Vec<&[u8]> {
        path.split(|&b| b == b'/').collect()
    }

    fn split_parent_name(path: &[u8]) -> (&[u8], &[u8]) {
        let trimmed = strip_trailing_slashes(path);
        match trimmed.iter().rposition(|&b| b == b'/') {
            Some(0) => (b"/", &trimmed[1..]),
            Some(pos) => (&trimmed[..pos], &trimmed[pos + 1..]),
            None => (b"/", trimmed),
        }
    }

    fn strip_trailing_slashes(path: &[u8]) -> &[u8] {
        let mut end = path.len();
        while end > 1 && path[end - 1] == b'/' {
            end -= 1;
        }
        &path[..end]
    }

    impl PathResolver for FakeFs {
        fn stat(
            &self,
            path: &[u8],
            caps: &mut FsCaps,
            budget: &mut ComputeBudget,
        ) -> Result<PathMetadata, ResolveError> {
            caps.charge_stat()?;
            budget.charge(path.len().max(1) as u64)?;
            let (ino, _) = self.walk_internal(path, true)?;
            self.inode_meta(ino)
        }

        fn lstat(
            &self,
            path: &[u8],
            caps: &mut FsCaps,
            budget: &mut ComputeBudget,
        ) -> Result<PathMetadata, ResolveError> {
            caps.charge_stat()?;
            budget.charge(path.len().max(1) as u64)?;
            let (ino, _) = self.walk_internal(path, false)?;
            self.inode_meta(ino)
        }

        fn read_link(
            &self,
            path: &[u8],
            caps: &mut FsCaps,
            budget: &mut ComputeBudget,
        ) -> Result<Vec<u8>, ResolveError> {
            caps.charge_link_read()?;
            budget.charge(path.len().max(1) as u64)?;
            let (ino, _) = self.walk_internal(path, false)?;
            let inode = self.inodes.get(&ino).ok_or(ResolveError::NotFound)?;
            if let Some(err) = inode.stat_error {
                return Err(ResolveError::Io(err));
            }
            match &inode.kind {
                InodeKind::Symlink { target } => {
                    budget.charge_alloc(target.len())?;
                    Ok(target.clone())
                }
                _ => Err(ResolveError::Io(FsIoError::NotASymlink)),
            }
        }

        fn canonicalize(
            &self,
            path: &[u8],
            caps: &mut FsCaps,
            budget: &mut ComputeBudget,
        ) -> Result<Vec<u8>, ResolveError> {
            caps.charge_canonicalize()?;
            budget.charge(path.len().max(1) as u64)?;
            let (ino, canon) = self.walk_internal(path, true)?;
            let _ = self.inode_meta(ino)?;
            budget.charge_alloc(canon.len())?;
            Ok(canon)
        }

        fn read_dir(
            &self,
            path: &[u8],
            caps: &mut FsCaps,
            budget: &mut ComputeBudget,
        ) -> Result<Vec<DirEntry>, ResolveError> {
            caps.charge_dir_open()?;
            budget.charge(path.len().max(1) as u64)?;
            let (ino, _) = self.walk_internal(path, true)?;
            let inode = self.inodes.get(&ino).ok_or(ResolveError::NotFound)?;
            if let Some(err) = inode.stat_error {
                return Err(ResolveError::Io(err));
            }
            match &inode.kind {
                InodeKind::Dir {
                    entries,
                    read_error,
                    ..
                } => {
                    if let Some(err) = read_error {
                        return Err(ResolveError::Io(*err));
                    }
                    let mut out = Vec::new();
                    for (name, &child_ino) in entries {
                        caps.charge_entries_read(1)?;
                        budget.charge(name.len().max(1) as u64)?;
                        budget.charge_alloc(std::mem::size_of::<DirEntry>() + name.len())?;
                        let child_ft = match self.inodes.get(&child_ino).map(|i| &i.kind) {
                            Some(InodeKind::File) => FileType::File,
                            Some(InodeKind::Dir { .. }) => FileType::Dir,
                            Some(InodeKind::Symlink { .. }) => FileType::Symlink,
                            None => FileType::Other,
                        };
                        out.push(DirEntry {
                            name: name.clone(),
                            file_type: child_ft,
                        });
                    }
                    Ok(out)
                }
                _ => Err(ResolveError::Io(FsIoError::NotADirectory)),
            }
        }
    }
}
