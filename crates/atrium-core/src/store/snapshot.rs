//! `VACUUM INTO` snapshots (mode 0600), pruned to `SNAPSHOTS_KEPT`.

use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use atrium_agents::BoxFuture;
use atrium_protocol::limits;

use super::{Shared, now_ms, sql_err};
use crate::{StoreError, StoreResult};

const PREFIX: &str = "atrium-";
const SUFFIX: &str = ".db";

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// `atrium-YYYYMMDD-HHMMSS-mmm.db` (UTC); sorts by time.
fn file_name(ms: u64) -> String {
    let secs = ms / 1000;
    let (y, mo, d) = civil((secs / 86_400) as i64);
    let rem = secs % 86_400;
    format!(
        "{PREFIX}{y:04}{mo:02}{d:02}-{:02}{:02}{:02}-{:03}{SUFFIX}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        ms % 1000
    )
}

/// Deletes all but the newest `SNAPSHOTS_KEPT` snapshot files; returns the
/// removed paths.
pub(crate) fn prune(dir: &Path) -> StoreResult<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(PREFIX) && n.ends_with(SUFFIX))
        })
        .collect();
    files.sort();
    let excess = files.len().saturating_sub(limits::SNAPSHOTS_KEPT);
    let removed: Vec<PathBuf> = files.drain(..excess).collect();
    for p in &removed {
        let _ = std::fs::remove_file(p);
    }
    Ok(removed)
}

pub(crate) fn snapshot(
    shared: Arc<Shared>,
    dir: PathBuf,
) -> BoxFuture<'static, StoreResult<PathBuf>> {
    Box::pin(async move {
        // Writes a file: refused while the volume is nearly full.
        let free_bytes = (shared.options.free_space)();
        if free_bytes < shared.options.min_free_disk_bytes {
            return Err(StoreError::ReadOnly { free_bytes });
        }
        let target = dir.clone();
        let path = shared
            .read(move |c| {
                secure_dir(&target)?;
                let mut ms = now_ms();
                // An empty private file first, so the snapshot is never
                // readable by others, not even briefly.
                let path = loop {
                    let p = target.join(file_name(ms));
                    match std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
                        .open(&p)
                    {
                        Ok(_) => break p,
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => ms += 1,
                        Err(e) => return Err(e.into()),
                    }
                };
                if let Err(e) = c.execute("VACUUM INTO ?1", [path.to_string_lossy()]) {
                    let _ = std::fs::remove_file(&path);
                    return Err(sql_err(e));
                }
                Ok(path)
            })
            .await?;
        let bytes = std::fs::metadata(&path)?.len();
        let (d, p) = (dir, path.clone());
        shared
            .call(move |w| w.record_snapshot(&d, &p, bytes))
            .await?;
        Ok(path)
    })
}

/// Creates `dir` (mode 0700) if missing and checks it is a real directory,
/// not a symlink, owned by us; a looser existing mode is tightened through an
/// `O_NOFOLLOW` handle.
fn secure_dir(dir: &Path) -> StoreResult<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let denied = |m: &str| {
        StoreError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            m.to_owned(),
        ))
    };
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.file_type().is_dir() {
        return Err(denied("snapshot folder is a symlink or not a directory"));
    }
    if meta.uid() != rustix::process::getuid().as_raw() {
        return Err(denied("snapshot folder is not owned by the current user"));
    }
    if meta.mode() & 0o777 != 0o700 {
        let handle = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(
                (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::DIRECTORY).bits() as i32,
            )
            .open(dir)?;
        handle.set_permissions(std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
