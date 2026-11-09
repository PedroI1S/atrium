//! Process supervision shared by every adapter and the daemon's crash
//! recovery (contract §10).
//!
//! - Every provider process starts as the leader of its own process group, so
//!   tool descendants are killed with it. Process groups don't contain
//!   descendants that deliberately detach (`setsid`); that's documented, not
//!   solved, until the sandbox.
//! - Identity is pid + start time + executable. A recorded identity that no
//!   longer matches is never killed: the thread becomes
//!   `Interrupted { cleanup: Unverified }`.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use atrium_protocol::thread::Cleanup;
use serde::{Deserialize, Serialize};

/// Enough to recognise a process again after a daemon restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    /// The process group id; equals `pid` for processes started here.
    pub pgid: u32,
    /// Platform start time (macOS `proc_pidinfo` start, Linux `/proc/<pid>/stat`
    /// field 22), in the platform's units. Only compared for equality.
    pub start_time: u64,
    pub executable: PathBuf,
}

/// A child started by [`spawn_group`].
#[derive(Debug)]
pub struct SupervisedChild {
    pub child: tokio::process::Child,
    pub identity: ProcessIdentity,
}

/// How a supervised process ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Termination {
    pub exit_code: Option<i32>,
    /// The group had to be killed after the grace period.
    pub killed: bool,
}

/// Spawns `command` as the leader of a new process group, with
/// `kill_on_drop`, and records its identity.
pub fn spawn_group(command: tokio::process::Command) -> io::Result<SupervisedChild> {
    let _ = command;
    unimplemented!("M1-10")
}

/// After the adapter's provider-specific soft stop: waits up to `grace` for
/// the leader, then `SIGKILL`s the whole group, and reaps the leader. Also
/// kills the group when the leader exited on its own but descendants remain.
pub async fn terminate(child: &mut SupervisedChild, grace: Duration) -> io::Result<Termination> {
    let _ = (child, grace);
    unimplemented!("M1-10")
}

/// The identity of a running process, or `None` if there is none with `pid`.
pub fn identify(pid: u32) -> io::Result<Option<ProcessIdentity>> {
    let _ = pid;
    unimplemented!("M1-10")
}

/// Daemon recovery: kills the recorded process group only if `recorded` still
/// identifies the same process. `Verified` if it was killed or is already
/// gone; `Unverified` if the pid now belongs to something else or can't be
/// inspected.
pub fn kill_recorded(recorded: &ProcessIdentity) -> Cleanup {
    let _ = recorded;
    unimplemented!("M1-10")
}
