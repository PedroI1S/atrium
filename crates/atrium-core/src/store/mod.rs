//! SQLite store (contract §7). **Owned by M1-02.** Public items named in
//! `lib.rs` are contract; everything else here is the task's.

use std::path::Path;
use std::sync::Arc;

use atrium_protocol::thread::ThreadState;

use crate::StoreResult;
use crate::attention::{AttentionInput, Pending};

/// The state machine the store applies on every append and signal.
pub type TransitionFn = fn(&ThreadState, Pending, AttentionInput<'_>) -> ThreadState;

/// Test seams. Production uses [`StoreOptions::default`].
#[derive(Clone)]
pub struct StoreOptions {
    /// Default: `attention::next_state`.
    pub transition: TransitionFn,
    /// Free bytes on the data volume. Default: `statvfs` on the store's folder.
    pub free_space: Arc<dyn Fn() -> u64 + Send + Sync>,
    /// Default: `limits::MIN_FREE_DISK_BYTES`.
    pub min_free_disk_bytes: u64,
}

/// The production [`crate::Store`]: WAL, one writer task, a pool of
/// `limits::STORE_READERS` read-only connections, migrations from
/// `schema/*.sql` run at open. Each open message's running SHA-256 is kept in
/// the writer task's memory and recomputed from stored deltas after a crash.
#[derive(Debug)]
pub struct SqliteStore {
    _private: (),
}

impl SqliteStore {
    /// Opens (creating if needed, mode 0600) and migrates `path`. A failed
    /// migration returns `StoreError::Migration` and leaves the file as it was.
    pub async fn open(path: &Path, options: StoreOptions) -> StoreResult<Self> {
        let _ = (path, options);
        unimplemented!("M1-02")
    }
}
