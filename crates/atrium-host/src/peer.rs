//! Peer verification for the daemon socket (contract §4, D-045).
//!
//! `App` authority is granted only when the connecting process is the Atrium
//! app. The check is done once per connection, at `Hello`, and pins the
//! process (audit token on macOS, pidfd on Linux) so pid reuse can't swap it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// What the daemon accepts as "the app". Written by the app at install to
/// `$ATRIUM_HOME/app-identity.json` (mode 0600, owner checked when read).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppIdentity {
    /// macOS, signed build: the designated requirement (bundle id + team id).
    MacSigned { requirement: String },
    /// macOS, unsigned development build: executable path + cdhash.
    MacUnsigned { executable: PathBuf, cdhash: String },
    /// Linux: the installed binary, by path and (device, inode).
    Linux {
        executable: PathBuf,
        dev: u64,
        ino: u64,
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
/// failure is an error; there is no partial success.
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
