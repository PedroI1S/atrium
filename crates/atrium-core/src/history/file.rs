//! The file an export is written into.

use std::fs::File;
use std::io;
use std::path::Path;

use rustix::fs::{Mode, OFlags};

/// Creates an export file: new file only (`O_CREAT | O_EXCL`), `O_NOFOLLOW`,
/// mode 0600, in a directory owned by the user. The daemon writes the export
/// into it with [`super::export_thread`].
///
/// The folder is opened first and the file is created relative to that
/// handle, so the folder that was checked is the folder the file lands in,
/// whatever happens to the path in between.
///
/// Errors: `AlreadyExists` when something is already at `path` (a symlink
/// included, dangling or not), `PermissionDenied` when the folder belongs to
/// another user, `InvalidInput` when `path` has no file name.
pub fn create_export_file(path: &Path) -> io::Result<File> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the export path has no file name",
        )
    })?;
    let folder = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let folder = rustix::fs::open(
        folder,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    if rustix::fs::fstat(&folder)?.st_uid != rustix::process::geteuid().as_raw() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the export folder is not owned by the current user",
        ));
    }
    let private = Mode::RUSR | Mode::WUSR;
    let file = rustix::fs::openat(
        &folder,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        private,
    )?;
    // The umask can only have removed bits; make it exactly 0600.
    rustix::fs::fchmod(&file, private)?;
    Ok(File::from(file))
}
