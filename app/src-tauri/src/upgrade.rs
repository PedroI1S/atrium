//! Replacing an older daemon (contract §10).
//!
//! An older `atriumd` answers this app's `Hello` with `Incompatible`, so the
//! app can't send it `ShutdownDaemon`. Instead the app signals it: `SIGTERM`
//! (it exits once no turn is running) or, after the user confirms, `SIGINT`
//! (running turns are interrupted, resumable, and it exits now).
//!
//! A pid alone could be anyone's process by now. The daemon's lock file holds
//! its identity (pid, start time, executable); the signal is sent only when
//! the process with that pid still has exactly that identity.

use std::io::Read;
use std::path::Path;

use atrium_host::supervise::{self, ProcessIdentity};
use rustix::process::{Pid, Signal};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// `SIGTERM`: exit once no turn is running.
    WhenIdle,
    /// `SIGINT`: interrupt running turns (resumable) and exit now.
    Now,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum UpgradeError {
    #[error("the daemon's lock file couldn't be read: {0}")]
    Lock(String),
    #[error("the process that answered is not the daemon that holds the lock")]
    NotTheDaemon,
    #[error("the daemon has already exited")]
    Gone,
    #[error("the daemon couldn't be signalled: {0}")]
    Signal(String),
}

/// The lock file is small; anything larger isn't one.
const MAX_LOCK_BYTES: u64 = 8192;

fn read_lock(lock: &Path) -> Result<ProcessIdentity, UpgradeError> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(lock)
        .map_err(|error| UpgradeError::Lock(error.to_string()))?;
    let mut bytes = Vec::new();
    file.take(MAX_LOCK_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|error| UpgradeError::Lock(error.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|error| UpgradeError::Lock(error.to_string()))
}

/// Signals the daemon `daemon_pid` (from `Incompatible`) to stop, after
/// checking it against the identity in `lock`.
pub fn stop_old_daemon(lock: &Path, daemon_pid: u32, how: Stop) -> Result<(), UpgradeError> {
    let recorded = read_lock(lock)?;
    if recorded.pid != daemon_pid {
        return Err(UpgradeError::NotTheDaemon);
    }
    let pid = i32::try_from(daemon_pid)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or(UpgradeError::NotTheDaemon)?;
    let signal = match how {
        Stop::WhenIdle => Signal::TERM,
        Stop::Now => Signal::INT,
    };
    // Linux: the process is pinned first, so that the identity check and the
    // signal are about the same process even if the pid is reused between
    // them. macOS has no such handle; there the check and the signal are
    // separated by a few microseconds.
    #[cfg(target_os = "linux")]
    let pinned = rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()).map_err(
        |error| {
            if error == rustix::io::Errno::SRCH {
                UpgradeError::Gone
            } else {
                UpgradeError::Signal(error.to_string())
            }
        },
    )?;
    match supervise::identify(daemon_pid) {
        Ok(Some(running)) if running == recorded => {}
        Ok(Some(_)) => return Err(UpgradeError::NotTheDaemon),
        Ok(None) => return Err(UpgradeError::Gone),
        Err(error) => return Err(UpgradeError::Signal(error.to_string())),
    }
    #[cfg(target_os = "linux")]
    let sent = rustix::process::pidfd_send_signal(&pinned, signal);
    #[cfg(not(target_os = "linux"))]
    let sent = rustix::process::kill_process(pid, signal);
    sent.map_err(|error| {
        if error == rustix::io::Errno::SRCH {
            UpgradeError::Gone
        } else {
            UpgradeError::Signal(error.to_string())
        }
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Child, Command};

    use super::*;

    fn sleeper() -> Child {
        Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("sleep starts")
    }

    fn lock_for(identity: &ProcessIdentity) -> (tempfile::TempDir, std::path::PathBuf) {
        let folder = tempfile::tempdir().unwrap();
        let lock = folder.path().join("atriumd.lock");
        std::fs::write(&lock, serde_json::to_vec(identity).unwrap()).unwrap();
        (folder, lock)
    }

    #[test]
    fn the_daemon_in_the_lock_file_gets_the_signal() {
        for (how, signal) in [(Stop::WhenIdle, 15), (Stop::Now, 2)] {
            let mut child = sleeper();
            let identity = supervise::identify(child.id())
                .unwrap()
                .expect("the child runs");
            let (_folder, lock) = lock_for(&identity);
            stop_old_daemon(&lock, child.id(), how).unwrap();
            let status = child.wait().unwrap();
            assert_eq!(status.signal(), Some(signal), "{how:?}");
        }
    }

    #[test]
    fn a_process_that_is_not_the_one_in_the_lock_file_is_left_alone() {
        let mut child = sleeper();
        let identity = supervise::identify(child.id()).unwrap().unwrap();

        // Same pid, another start time: the pid was reused.
        let reused = ProcessIdentity {
            start_time: identity.start_time + 1,
            ..identity.clone()
        };
        let (_folder, lock) = lock_for(&reused);
        assert_eq!(
            stop_old_daemon(&lock, child.id(), Stop::Now),
            Err(UpgradeError::NotTheDaemon)
        );

        // Same pid, another executable.
        let other = ProcessIdentity {
            executable: "/usr/bin/not-atriumd".into(),
            ..identity.clone()
        };
        let (_folder, lock) = lock_for(&other);
        assert_eq!(
            stop_old_daemon(&lock, child.id(), Stop::Now),
            Err(UpgradeError::NotTheDaemon)
        );

        // The lock names another pid than the one that answered.
        let (_folder, lock) = lock_for(&identity);
        assert_eq!(
            stop_old_daemon(&lock, std::process::id(), Stop::Now),
            Err(UpgradeError::NotTheDaemon)
        );

        assert!(
            child.try_wait().unwrap().is_none(),
            "the process was never signalled"
        );
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn a_daemon_that_already_exited_is_reported_as_gone() {
        let mut child = sleeper();
        let identity = supervise::identify(child.id()).unwrap().unwrap();
        let (_folder, lock) = lock_for(&identity);
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(
            stop_old_daemon(&lock, identity.pid, Stop::WhenIdle),
            Err(UpgradeError::Gone)
        );
    }

    #[test]
    fn a_lock_file_that_is_missing_unreadable_or_a_link_is_refused() {
        let folder = tempfile::tempdir().unwrap();
        let missing = folder.path().join("atriumd.lock");
        assert!(matches!(
            stop_old_daemon(&missing, 1, Stop::WhenIdle),
            Err(UpgradeError::Lock(_))
        ));
        std::fs::write(&missing, b"not json").unwrap();
        assert!(matches!(
            stop_old_daemon(&missing, 1, Stop::WhenIdle),
            Err(UpgradeError::Lock(_))
        ));

        let mut child = sleeper();
        let identity = supervise::identify(child.id()).unwrap().unwrap();
        let (_real_folder, real) = lock_for(&identity);
        let link = folder.path().join("link.lock");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(matches!(
            stop_old_daemon(&link, child.id(), Stop::Now),
            Err(UpgradeError::Lock(_))
        ));
        assert!(child.try_wait().unwrap().is_none());
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
