//! Peer verification for the daemon socket (contract §4, D-045).
//!
//! `App` authority is granted only when the connecting process is the Atrium
//! app. The check is done once per connection, at `Hello`, and pins the
//! process (audit token on macOS, pidfd on Linux) so pid reuse can't swap it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// What the daemon accepts as "the app".
///
/// - Release builds compile it in: `MacSigned` with the designated
///   requirement, or `Linux` with the packaged app path and `dev_ino: None`
///   (the daemon checks at runtime that the file and its folders are owned
///   by root and not writable by the user, so a same-user process can't
///   replace it). The data folder can't change it.
/// - Development builds read `Paths::app_identity` (mode 0600, owner checked),
///   written by the dev app at start. That file is as strong as the user's
///   own account, which is the documented same-UID limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppIdentity {
    /// macOS, signed build: the designated requirement (bundle id + team id).
    MacSigned { requirement: String },
    /// macOS, unsigned development build: executable path + cdhash.
    MacUnsigned { executable: PathBuf, cdhash: String },
    /// Linux: the installed binary by path. `dev_ino` (device, inode) is
    /// set only in development identities; release builds can't know it.
    Linux {
        executable: PathBuf,
        dev_ino: Option<(u64, u64)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInfo {
    pub pid: u32,
    pub uid: u32,
    pub executable: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    #[error("peer is another user (uid {0})")]
    OtherUser(u32),
    #[error("peer is not the Atrium app: {0}")]
    NotApp(String),
    #[error("peer couldn't be inspected: {0}")]
    Inspect(String),
}

/// Verifies the peer of a connected unix socket against `expected`. Any
/// failure is an error; there is no partial success. The uid is checked
/// first.
/// - macOS: `LOCAL_PEERTOKEN` → `SecCodeCreateWithAuditToken`; signed:
///   `SecCodeCheckValidity` against the requirement; unsigned: the cdhash
///   from the code object (`kSecCodeInfoUnique`), never a hash of the file on
///   disk.
/// - Linux: `SO_PEERCRED`, then the peer's pidfd from `SO_PEERPIDFD`
///   (kernel 6.5+; without it, App authority is refused: no `pidfd_open`
///   fallback, which would race with pid reuse). `stat` `/proc/<pid>/exe`:
///   it must be `executable` (and match `dev_ino` when set; otherwise the
///   ownership check above), then `pidfd_send_signal(pidfd, 0)` must still
///   succeed, proving the process inspected is the one connected.
pub fn verify_app_peer(
    stream: &tokio::net::UnixStream,
    expected: &AppIdentity,
) -> Result<PeerInfo, PeerError> {
    let _ = (stream, expected);
    unimplemented!("M1-10")
}

/// The identity of the running executable, for the app to write at install.
pub fn current_app_identity() -> std::io::Result<AppIdentity> {
    unimplemented!("M1-10")
}
