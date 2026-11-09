//! Daemon lifecycle (PLAN §3.1, contract §10).

use std::path::{Path, PathBuf};

/// Held for the daemon's whole life. Taken *before* a stale socket is removed.
#[derive(Debug)]
pub struct InstanceLock {
    _private: (),
}

#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    #[error("another daemon is running (pid {0:?})")]
    AlreadyRunning(Option<u32>),
    /// The runtime directory is a symlink, not owned by the user, or not 0700.
    #[error("unsafe runtime directory {0}: {1}")]
    UnsafeDir(PathBuf, String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// `~/Library/Application Support/Atrium` on macOS, `$XDG_RUNTIME_DIR/atrium`
/// (socket, lock) and `$XDG_DATA_HOME/atrium` (store) on Linux.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub runtime_dir: PathBuf,
    pub data_dir: PathBuf,
    pub socket: PathBuf,
    pub lock: PathBuf,
}

pub fn default_paths() -> std::io::Result<Paths> {
    unimplemented!("M1-10")
}

/// Creates the directory 0700 if missing; refuses a symlink, another owner
/// or a looser mode.
pub fn secure_dir(dir: &Path) -> Result<(), LifecycleError> {
    let _ = dir;
    unimplemented!("M1-10")
}

impl InstanceLock {
    pub fn acquire(paths: &Paths) -> Result<Self, LifecycleError> {
        let _ = paths;
        unimplemented!("M1-10")
    }
}

/// Starts `atriumd` detached (new session, stdio to the log file), for the
/// app when it finds no daemon. Returns its pid.
pub fn spawn_detached(executable: &Path, args: &[String], log: &Path) -> std::io::Result<u32> {
    let _ = (executable, args, log);
    unimplemented!("M1-10")
}

/// "Keep agents running after quitting": a launchd LaunchAgent (macOS) or a
/// systemd user unit (Linux) that starts `executable` at login.
pub fn install_keep_running(executable: &Path) -> std::io::Result<()> {
    let _ = executable;
    unimplemented!("M1-10")
}

pub fn remove_keep_running() -> std::io::Result<()> {
    unimplemented!("M1-10")
}
