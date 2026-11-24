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
                if code == rustix::io::Errno::ACCESS.raw_os_error()
                    || code == rustix::io::Errno::PERM.raw_os_error()
                {
                    return ResolveError::Io(FsIoError::PermissionDenied);
                }
            }
            ResolveError::Io(FsIoError::Other)
        }
    }
}

#[cfg(unix)]
fn map_errno(errno: rustix::io::Errno, for_readlink: bool) -> ResolveError {
    if errno == rustix::io::Errno::NOENT {
        return ResolveError::NotFound;
    }
    if for_readlink && errno == rustix::io::Errno::INVAL {
        return ResolveError::Io(FsIoError::NotASymlink);
    }
    if errno == rustix::io::Errno::LOOP {
        return ResolveError::Io(FsIoError::SymlinkLoop);
    }
    if errno == rustix::io::Errno::NOTDIR {
        return ResolveError::Io(FsIoError::NotADirectory);
    }
    if errno == rustix::io::Errno::ACCESS || errno == rustix::io::Errno::PERM {
        return ResolveError::Io(FsIoError::PermissionDenied);
    }
    ResolveError::Io(FsIoError::Other)
}

#[cfg(unix)]
fn map_rustix_file_type(ft: rustix::fs::FileType) -> Option<FileType> {
    match ft {
        rustix::fs::FileType::RegularFile => Some(FileType::File),
        rustix::fs::FileType::Directory => Some(FileType::Dir),
        rustix::fs::FileType::Symlink => Some(FileType::Symlink),
        rustix::fs::FileType::Unknown => None,
        _ => Some(FileType::Other),
    }
}

/// Longest path the kernel returns from `readlink` or `realpath`
/// (`PATH_MAX`): 1,024 bytes on Apple platforms, 4,096 elsewhere.
#[cfg(any(target_os = "macos", target_os = "ios"))]
const PATH_MAX: usize = 1024;
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
const PATH_MAX: usize = 4096;

/// `rustix` and `std` copy a path into a heap `CString` when it does not fit
/// their stack buffers (256 bytes in `rustix`, 384 in `std`). Charges that
/// copy before the call that makes it (§8).
fn charge_path_conversion(path: &[u8], budget: &mut ComputeBudget) -> Result<(), ResolveError> {
    if path.len() >= 256 {
        budget.charge_alloc(path.len() + 1)?;
    }
    Ok(())
}

/// Charges one auxiliary metadata call made inside another operation: the
/// `stat` cap and its compute, before the call (§8, round-2 finding 4).
fn charge_aux_stat(
    name_len: usize,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
) -> Result<(), ResolveError> {
    caps.charge_stat()?;
    budget.charge(name_len.max(1) as u64)?;
    Ok(())
}

/// Pushes `entry` onto `out`, charging the vector's growth before it
/// reallocates (the name's own bytes are charged by the caller, before the
/// name is copied).
fn push_entry_charged(
    out: &mut Vec<DirEntry>,
    entry: DirEntry,
    budget: &mut ComputeBudget,
) -> Result<(), ResolveError> {
    if out.len() == out.capacity() {
        let new_cap = out.capacity().saturating_mul(2).max(8);
        budget.charge_alloc(new_cap.saturating_mul(std::mem::size_of::<DirEntry>()))?;
        out.reserve_exact(new_cap - out.len());
    }
    out.push(entry);
    Ok(())
}

/// Canonicalizes through `/proc/self/fd`: `open(O_PATH)` follows every link
/// without reading metadata, and the kernel writes the real path into a
/// stack buffer, so the result is charged at its exact size before the
/// `Vec` that holds it is allocated. Returns `Ok(None)` when `/proc` cannot
/// answer (not mounted, or a path outside the process root), and the caller
/// falls back.
#[cfg(target_os = "linux")]
fn canonicalize_via_proc(
    p: &Path,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
) -> Result<Option<Vec<u8>>, ResolveError> {
    use rustix::fs::{CWD, Mode, OFlags, open, readlinkat_raw};

    let fd =
        open(p, OFlags::PATH | OFlags::CLOEXEC, Mode::empty()).map_err(|e| map_errno(e, false))?;
    // "/proc/self/fd/" + decimal fd + NUL, built on the stack.
    let mut link = [0u8; 40];
    let prefix = b"/proc/self/fd/";
    link[..prefix.len()].copy_from_slice(prefix);
    let mut digits = [0u8; 20];
    let mut n = {
        use std::os::fd::AsRawFd;
        u64::try_from(fd.as_raw_fd()).map_err(|_| ResolveError::Io(FsIoError::Other))?
    };
    let mut d = 0usize;
    loop {
        digits[d] = b'0' + (n % 10) as u8;
        d += 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    let mut end = prefix.len();
    for i in (0..d).rev() {
        link[end] = digits[i];
        end += 1;
    }
    let Ok(link_cstr) = std::ffi::CStr::from_bytes_with_nul(&link[..=end]) else {
        return Ok(None);
    };
    // Reading the `/proc` link is a link read: charged first (round-3 finding 5).
    caps.charge_link_read()?;
    budget.charge(end as u64)?;
    let mut buf = [0u8; PATH_MAX];
    let Ok(len) = readlinkat_raw(CWD, link_cstr, &mut buf) else {
        return Ok(None);
    };
    if len == 0 || len >= PATH_MAX || buf[0] != b'/' {
        return Ok(None);
    }
    budget.charge(len as u64)?;
    budget.charge_alloc(len)?;
    Ok(Some(buf[..len].to_vec()))
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
        charge_path_conversion(path, budget)?;
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
        charge_path_conversion(path, budget)?;
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
        charge_path_conversion(path, budget)?;
        let p = bytes_to_path(path)?;
        #[cfg(unix)]
        {
            // `readlink` writes into a stack buffer of `PATH_MAX` bytes (no
            // metadata call, no heap growth); `EINVAL` means "not a link".
            // The target is then charged at its exact size before the `Vec`
            // holding it is allocated (§8, round-2 findings 4 and 5).
            let mut buf = [0u8; PATH_MAX];
            let len = rustix::fs::readlinkat_raw(rustix::fs::CWD, p, &mut buf)
                .map_err(|e| map_errno(e, true))?;
            if len >= PATH_MAX {
                // Truncated: a target this long cannot be resolved anyway.
                return Err(ResolveError::Io(FsIoError::Other));
            }
            budget.charge(len as u64)?;
            budget.charge_alloc(len)?;
            Ok(buf[..len].to_vec())
        }
        #[cfg(not(unix))]
        {
            // `std::fs::read_link` allocates its result before returning it,
            // so the largest result is charged first.
            budget.charge_alloc(PATH_MAX.saturating_mul(2))?;
            let target = std::fs::read_link(p).map_err(|e| map_io_error(&e, true))?;
            Ok(path_to_bytes(&target).to_vec())
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
        charge_path_conversion(path, budget)?;
        let p = bytes_to_path(path)?;
        #[cfg(target_os = "linux")]
        if let Some(real) = canonicalize_via_proc(p, caps, budget)? {
            return Ok(real);
        }
        // Fallback (no `/proc`, or another platform): `realpath` allocates a
        // buffer of at most `PATH_MAX` bytes and `std` copies the result into
        // a `Vec` of the same bound, so both are charged before the call; its
        // internal metadata calls are not visible here, so one `stat` per
        // component of `path` is charged first (round-2 finding 4).
        let components = path.split(|&b| b == b'/').filter(|c| !c.is_empty()).count();
        for _ in 0..components.max(1) {
            charge_aux_stat(path.len(), caps, budget)?;
        }
        budget.charge_alloc(PATH_MAX.saturating_mul(2))?;
        let canon = std::fs::canonicalize(p).map_err(|e| map_io_error(&e, false))?;
        #[cfg(unix)]
        let bytes = {
            use std::os::unix::ffi::OsStringExt;
            canon.into_os_string().into_vec()
        };
        #[cfg(not(unix))]
        let bytes = path_to_bytes(&canon).to_vec();
        Ok(bytes)
    }

    fn read_dir(
        &self,
        path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<Vec<DirEntry>, ResolveError> {
        caps.charge_dir_open()?;
        budget.charge(path.len().max(1) as u64)?;
        charge_path_conversion(path, budget)?;
        let p = bytes_to_path(path)?;
        #[cfg(unix)]
        {
            use rustix::fs::{AtFlags, Mode, OFlags, RawDir, open, statat};

            let dir_fd = open(
                p,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| map_errno(e, false))?;
            let mut stack_buf = [std::mem::MaybeUninit::<u8>::uninit(); 4096];
            let mut raw_dir = RawDir::new(&dir_fd, &mut stack_buf);
            let mut out: Vec<DirEntry> = Vec::new();
            loop {
                // Charge entry read and work BEFORE advancing the directory iterator.
                caps.charge_entries_read(1)?;
                budget.charge(1)?;
                let Some(entry_res) = raw_dir.next() else {
                    break;
                };
                let entry = entry_res.map_err(|e| map_errno(e, false))?;
                let name_cstr = entry.file_name();
                let name_bytes = name_cstr.to_bytes();
                if name_bytes == b"." || name_bytes == b".." {
                    continue;
                }
                budget.charge(name_bytes.len().saturating_sub(1) as u64)?;
                let file_type = match map_rustix_file_type(entry.file_type()) {
                    Some(ft) => ft,
                    None => {
                        // `DT_UNKNOWN`: one `fstatat`, charged first (round-2 finding 4).
                        charge_aux_stat(name_bytes.len(), caps, budget)?;
                        let st = statat(&dir_fd, name_cstr, AtFlags::SYMLINK_NOFOLLOW)
                            .map_err(|e| map_errno(e, false))?;
                        map_rustix_file_type(rustix::fs::FileType::from_raw_mode(st.st_mode))
                            .unwrap_or(FileType::Other)
                    }
                };
                budget.charge_alloc(name_bytes.len())?;
                let mut name = Vec::with_capacity(name_bytes.len());
                name.extend_from_slice(name_bytes);
                push_entry_charged(&mut out, DirEntry { name, file_type }, budget)?;
            }
            Ok(out)
        }
        #[cfg(not(unix))]
        {
            let mut rd = std::fs::read_dir(p).map_err(|e| map_io_error(&e, false))?;
            let mut out: Vec<DirEntry> = Vec::new();
            loop {
                caps.charge_entries_read(1)?;
                budget.charge(1)?;
                // `std` allocates the entry (and its name) while advancing,
                // so the largest name is charged first.
                budget.charge_alloc(PATH_MAX)?;
                let Some(entry_res) = rd.next() else {
                    break;
                };
                let entry = entry_res.map_err(|e| map_io_error(&e, false))?;
                let os_name = entry.file_name();
                let name_bytes = path_to_bytes(Path::new(&os_name));
                budget.charge(name_bytes.len().saturating_sub(1) as u64)?;
                charge_aux_stat(name_bytes.len(), caps, budget)?;
                let ft = entry.file_type().map_err(|e| map_io_error(&e, false))?;
                budget.charge_alloc(name_bytes.len())?;
                push_entry_charged(
                    &mut out,
                    DirEntry {
                        name: name_bytes.to_vec(),
                        file_type: map_file_type(ft),
                    },
                    budget,
                )?;
            }
            Ok(out)
        }
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

    /// Independent test-only ASCII/basic case folding for `FakeFs::case_insensitive`
    /// (does NOT reuse production `sensitive.rs` folding functions).
    fn fake_fold_case(bytes: &[u8]) -> Vec<u8> {
        match std::str::from_utf8(bytes) {
            Ok(s) => s
                .chars()
                .flat_map(|ch| match ch {
                    '\u{017F}' => vec!['s'],
                    '\u{212A}' => vec!['k'],
                    other => other.to_lowercase().collect(),
                })
                .collect::<String>()
                .into_bytes(),
            Err(_) => bytes.to_ascii_lowercase(),
        }
    }

    /// Independent test-only volume equivalence table for `FakeFs::nfc_nfd_insensitive`
    /// (does NOT reuse production `sensitive.rs` folding/normalization functions).
    fn fake_volume_key(bytes: &[u8], extra_aliases: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
        for (a, b) in extra_aliases {
            if bytes == a.as_slice() || bytes == b.as_slice() {
                return a.clone();
            }
        }
        let Ok(s) = std::str::from_utf8(bytes) else {
            return fake_fold_case(bytes);
        };
        let mut out = String::with_capacity(s.len());
        for ch in s.chars() {
            match ch {
                '\u{00C9}' | '\u{00E9}' => out.push_str("e\u{0301}"),
                '\u{01FA}' | '\u{01FB}' => out.push_str("a\u{030A}\u{0301}"),
                '\u{212B}' | '\u{00C5}' | '\u{00E5}' => out.push_str("a\u{030A}"),
                '\u{1E08}' | '\u{1E09}' => out.push_str("c\u{0327}\u{0301}"),
                '\u{AC00}' => out.push_str("\u{1100}\u{1161}"),
                '\u{00DF}' | '\u{1E9E}' => out.push_str("ss"),
                '\u{0130}' | '\u{0131}' => out.push('i'),
                '\u{FB01}' => out.push_str("fi"),
                '\u{00AD}' | '\u{200B}' | '\u{FEFF}' => {}
                other => {
                    for lc in other.to_lowercase() {
                        out.push(lc);
                    }
                }
            }
        }
        out.into_bytes()
    }

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
        /// Unicode case folds (simulating APFS/HFS+ normalization) using an
        /// independent test-only equivalence mapping.
        pub(crate) nfc_nfd_equivalent: bool,
        /// Optional explicit component-name equivalence pairs `(canon, alias)`
        /// for independent conformance tests.
        extra_equivalences: Vec<(Vec<u8>, Vec<u8>)>,
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
                extra_equivalences: Vec::new(),
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

        pub(crate) fn add_equivalent_component(&mut self, a: &[u8], b: &[u8]) {
            self.extra_equivalences.push((a.to_vec(), b.to_vec()));
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
                return fake_volume_key(stored, &self.extra_equivalences)
                    == fake_volume_key(query, &self.extra_equivalences);
            }
            if self.case_insensitive {
                return fake_fold_case(stored) == fake_fold_case(query);
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

        /// Gives the existing directory at `existing_dir` a second name at
        /// `path` with the same identity (a bind mount): both names list the
        /// same entries, and each canonicalizes to its own spelling.
        pub(crate) fn bind_dir(&mut self, existing_dir: &[u8], path: &[u8]) -> FileIdentity {
            let (ino, _) = self
                .walk_internal(existing_dir, true)
                .expect("bind source must exist");
            assert!(
                matches!(self.inodes[&ino].kind, InodeKind::Dir { .. }),
                "bind source must be a directory"
            );
            let (parent, name) = split_parent_name(path);
            let parent_id = self.mkdir_all(parent);
            if let Some(Inode {
                kind: InodeKind::Dir { entries, .. },
                ..
            }) = self.inodes.get_mut(&parent_id.ino)
            {
                entries.insert(name.to_vec(), ino);
            }
            self.inodes[&ino].id
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
                if seg == b"." || seg == b".." {
                    let cur_ino = stack.last().expect("root always present").0;
                    match self.inodes.get(&cur_ino) {
                        Some(Inode {
                            kind: InodeKind::Dir { exec_error, .. },
                            ..
                        }) => {
                            if let Some(err) = exec_error {
                                return Err(ResolveError::Io(*err));
                            }
                        }
                        Some(_) => return Err(ResolveError::Io(FsIoError::NotADirectory)),
                        None => return Err(ResolveError::NotFound),
                    }
                    if seg == b".." && stack.len() > 1 {
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
                    let mut iter = entries.iter();
                    loop {
                        caps.charge_entries_read(1)?;
                        budget.charge(1)?;
                        let Some((name, &child_ino)) = iter.next() else {
                            break;
                        };
                        budget.charge(name.len().saturating_sub(1) as u64)?;
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
