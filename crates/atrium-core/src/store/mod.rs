//! SQLite store (contract §7). **Owned by M1-02.** Public items named in
//! `lib.rs` are contract; everything else here is the task's.
//!
//! Layout: one writer thread owns the only read-write connection and every
//! mutable in-memory state (each open message's running SHA-256); reads go
//! through a small pool of read-only connections.

mod codec;
mod migrate;
mod reader;
mod requests;
mod snapshot;
mod writer;

use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::thread::JoinHandle;

use atrium_agents::BoxFuture;
use atrium_host::supervise::ProcessIdentity;
use atrium_protocol::agent::AgentEventEnvelope;
use atrium_protocol::limits;
use atrium_protocol::policy::{EnforcementDescriptor, Rule, RuleId};
use atrium_protocol::thread::{ThreadId, ThreadState, ThreadSummary};
use atrium_protocol::wire::{EventId, StoredEvent};
use serde_json::Value;

use crate::attention::{AttentionInput, Pending};
use crate::{
    NewThread, OpenItems, RequestStore, Signal, StorageStatus, Store, StoreError, StoreResult,
};

use reader::ReaderPool;
use writer::Writer;

#[doc(hidden)]
pub use migrate::Migration;

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

/// Marks "use `statvfs` on the store's folder": [`StoreOptions::default`]
/// hands out this very `Arc`, and `open` swaps it for the real probe.
static DEFAULT_FREE_SPACE: LazyLock<Arc<dyn Fn() -> u64 + Send + Sync>> =
    LazyLock::new(|| Arc::new(|| u64::MAX));

impl Default for StoreOptions {
    fn default() -> Self {
        Self {
            transition: crate::attention::next_state,
            free_space: DEFAULT_FREE_SPACE.clone(),
            min_free_disk_bytes: limits::MIN_FREE_DISK_BYTES,
        }
    }
}

/// Jobs queued for the writer before callers wait.
const WRITER_QUEUE: usize = 64;

type Job = Box<dyn FnOnce(&mut Writer) + Send>;

pub(crate) struct Shared {
    tx: Mutex<Option<tokio::sync::mpsc::Sender<Job>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    pub(crate) readers: Arc<ReaderPool>,
    pub(crate) options: StoreOptions,
    pub(crate) integrity: Arc<AtomicU64>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        if let Ok(mut tx) = self.tx.lock() {
            tx.take();
        }
        if let Ok(mut t) = self.thread.lock()
            && let Some(handle) = t.take()
        {
            let _ = handle.join();
        }
    }
}

fn gone() -> StoreError {
    StoreError::Sqlite("the store's writer has stopped".into())
}

impl Shared {
    /// Runs `f` on the writer thread (one transaction inside) and awaits it.
    pub(crate) fn call<T, F>(self: &Arc<Self>, f: F) -> BoxFuture<'static, StoreResult<T>>
    where
        T: Send + 'static,
        F: FnOnce(&mut Writer) -> StoreResult<T> + Send + 'static,
    {
        let shared = self.clone();
        Box::pin(async move {
            let (otx, orx) = tokio::sync::oneshot::channel();
            let job: Job = Box::new(move |w| {
                let _ = otx.send(f(w));
            });
            let sender = {
                let guard = shared.tx.lock().map_err(|_| gone())?;
                guard.as_ref().ok_or_else(gone)?.clone()
            };
            // Bounded: a slow writer pushes back on callers.
            sender.send(job).await.map_err(|_| gone())?;
            orx.await.map_err(|_| gone())?
        })
    }

    /// Runs `f` on a pooled read-only connection, off the async threads.
    pub(crate) fn read<T, F>(self: &Arc<Self>, f: F) -> BoxFuture<'static, StoreResult<T>>
    where
        T: Send + 'static,
        F: FnOnce(&rusqlite::Connection) -> StoreResult<T> + Send + 'static,
    {
        let pool = self.readers.clone();
        Box::pin(async move {
            // One permit per pooled connection, taken before the blocking
            // task exists, so blocking-pool threads never park waiting.
            let permit = pool
                .permits
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| StoreError::Sqlite("reader pool closed".into()))?;
            tokio::task::spawn_blocking(move || {
                let out = pool.with(f);
                drop(permit);
                out
            })
            .await
            .map_err(|e| StoreError::Sqlite(format!("read task failed: {e}")))?
        })
    }
}

/// The production [`crate::Store`]: WAL, one writer task, a pool of
/// `limits::STORE_READERS` read-only connections, migrations from
/// `schema/*.sql` run at open. Each open message's running SHA-256 is kept in
/// the writer task's memory and recomputed from stored deltas after a crash.
pub struct SqliteStore {
    shared: Arc<Shared>,
    path: PathBuf,
}

impl std::fmt::Debug for SqliteStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

pub(crate) fn sql_err(e: rusqlite::Error) -> StoreError {
    StoreError::Sqlite(e.to_string())
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Free bytes available to unprivileged writers on the volume holding `dir`.
fn statvfs_free(dir: &Path) -> u64 {
    match rustix::fs::statvfs(dir) {
        Ok(s) => s.f_bavail.saturating_mul(s.f_frsize),
        // Fail closed: if the volume can't be probed, treat it as full.
        Err(_) => 0,
    }
}

impl SqliteStore {
    /// Opens (creating if needed, mode 0600) and migrates `path`. A failed
    /// migration returns `StoreError::Migration` and leaves the file as it was.
    pub async fn open(path: &Path, options: StoreOptions) -> StoreResult<Self> {
        Self::open_with(path, options, migrate::MIGRATIONS).await
    }

    /// [`SqliteStore::open`] with an explicit migration list. Test seam.
    #[doc(hidden)]
    pub async fn open_with(
        path: &Path,
        options: StoreOptions,
        migrations: &'static [Migration],
    ) -> StoreResult<Self> {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || Self::open_blocking(path, options, migrations))
            .await
            .map_err(|e| StoreError::Sqlite(format!("open task failed: {e}")))?
    }

    fn open_blocking(
        path: PathBuf,
        mut options: StoreOptions,
        migrations: &'static [Migration],
    ) -> StoreResult<Self> {
        let path = secure_database_file(&path)?;
        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        if Arc::ptr_eq(&options.free_space, &DEFAULT_FREE_SPACE) {
            let dir = dir.clone();
            options.free_space = Arc::new(move || statvfs_free(&dir));
        }

        let mut conn = rusqlite::Connection::open(&path).map_err(sql_err)?;
        // Migrations run before WAL is switched on, in the rollback journal,
        // so a failed one leaves the file byte-identical.
        migrate::run(&mut conn, migrations)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(sql_err)?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(sql_err)?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(sql_err)?;
        conn.pragma_update(None, "cache_size", -(limits::SQLITE_CACHE_KIB as i64))
            .map_err(sql_err)?;
        conn.pragma_update(None, "journal_size_limit", 64i64 * 1024 * 1024)
            .map_err(sql_err)?;

        let readers = Arc::new(ReaderPool::open(&path, limits::STORE_READERS)?);
        let integrity = Arc::new(AtomicU64::new(0));
        let writer = Writer::new(conn, options.clone(), integrity.clone());
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Job>(WRITER_QUEUE);
        let handle = std::thread::Builder::new()
            .name("atrium-store-writer".into())
            .spawn(move || {
                let mut writer = writer;
                while let Some(job) = rx.blocking_recv() {
                    // A panicking job drops its reply channel (the caller gets
                    // an error); the writer lives on, its transaction rolled
                    // back and its hashers rebuilt from stored events.
                    let r =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&mut writer)));
                    if r.is_err() {
                        writer.recover_from_panic();
                    }
                }
            })?;
        Ok(Self {
            shared: Arc::new(Shared {
                tx: Mutex::new(Some(tx)),
                thread: Mutex::new(Some(handle)),
                readers,
                options,
                integrity,
            }),
            path,
        })
    }

    /// How many `MessageCompleted` events did not match their open message's
    /// chars, deltas or hash (stored as given, but logged).
    pub fn integrity_errors(&self) -> u64 {
        self.shared.integrity.load(Ordering::Relaxed)
    }
}

impl Store for SqliteStore {
    fn create_thread(&self, new: NewThread) -> BoxFuture<'_, StoreResult<ThreadSummary>> {
        self.shared.call(move |w| w.create_thread(new))
    }

    fn begin_incarnation(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<u32>> {
        let thread = thread.clone();
        self.shared.call(move |w| w.begin_incarnation(&thread))
    }

    fn set_process(
        &self,
        thread: &ThreadId,
        incarnation: u32,
        process: Option<ProcessIdentity>,
    ) -> BoxFuture<'_, StoreResult<()>> {
        let thread = thread.clone();
        self.shared
            .call(move |w| w.set_process(&thread, incarnation, process))
    }

    fn append(
        &self,
        thread: &ThreadId,
        incarnation: u32,
        events: Vec<AgentEventEnvelope>,
    ) -> BoxFuture<'_, StoreResult<Vec<StoredEvent>>> {
        let thread = thread.clone();
        self.shared
            .call(move |w| w.append(&thread, incarnation, events))
    }

    fn signal(
        &self,
        thread: &ThreadId,
        signal: Signal,
    ) -> BoxFuture<'_, StoreResult<ThreadSummary>> {
        let thread = thread.clone();
        self.shared.call(move |w| w.signal(&thread, signal))
    }

    fn events_after(
        &self,
        thread: &ThreadId,
        after: Option<EventId>,
        limit: usize,
    ) -> BoxFuture<'_, StoreResult<Vec<StoredEvent>>> {
        let thread = thread.clone();
        self.shared
            .read(move |c| reader::events_after(c, &thread, after, limit))
    }

    fn last_event(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<Option<EventId>>> {
        let thread = thread.clone();
        self.shared.read(move |c| reader::last_event(c, &thread))
    }

    fn thread(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<Option<ThreadSummary>>> {
        let thread = thread.clone();
        self.shared.read(move |c| codec::summary(c, &thread))
    }

    fn threads(&self) -> BoxFuture<'_, StoreResult<Vec<ThreadSummary>>> {
        self.shared.read(reader::threads)
    }

    fn archive(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<()>> {
        let thread = thread.clone();
        self.shared.call(move |w| w.archive(&thread))
    }

    fn rename(&self, thread: &ThreadId, title: &str) -> BoxFuture<'_, StoreResult<ThreadSummary>> {
        let thread = thread.clone();
        let title = title.to_owned();
        self.shared.call(move |w| w.rename(&thread, &title))
    }

    fn mark_read(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<()>> {
        let thread = thread.clone();
        self.shared.call(move |w| w.mark_read(&thread))
    }

    fn set_enforcement(
        &self,
        thread: &ThreadId,
        incarnation: u32,
        descriptor: EnforcementDescriptor,
    ) -> BoxFuture<'_, StoreResult<()>> {
        let thread = thread.clone();
        self.shared
            .call(move |w| w.set_enforcement(&thread, incarnation, descriptor))
    }

    fn resume_id(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<Option<String>>> {
        let thread = thread.clone();
        self.shared.read(move |c| reader::resume_id(c, &thread))
    }

    fn open_items(
        &self,
        thread: &ThreadId,
        incarnation: u32,
    ) -> BoxFuture<'_, StoreResult<OpenItems>> {
        let thread = thread.clone();
        // On the writer: the running hashes live there.
        self.shared
            .call(move |w| w.open_items(&thread, incarnation))
    }

    fn live_processes(&self) -> BoxFuture<'_, StoreResult<Vec<(ThreadId, u32, ProcessIdentity)>>> {
        self.shared.read(reader::live_processes)
    }

    fn rules(&self) -> BoxFuture<'_, StoreResult<(u64, Vec<Rule>)>> {
        self.shared.read(reader::rules)
    }

    fn create_rule(&self, rule: Rule) -> BoxFuture<'_, StoreResult<Rule>> {
        self.shared.call(move |w| w.create_rule(rule))
    }

    fn delete_rule(&self, rule: &RuleId) -> BoxFuture<'_, StoreResult<()>> {
        let rule = rule.clone();
        self.shared.call(move |w| w.delete_rule(&rule))
    }

    fn settings(&self) -> BoxFuture<'_, StoreResult<Vec<(String, Value)>>> {
        self.shared.read(reader::settings)
    }

    fn set_setting(&self, key: &str, value: Value) -> BoxFuture<'_, StoreResult<()>> {
        let key = key.to_owned();
        self.shared.call(move |w| w.set_setting(&key, value))
    }

    fn storage_status(&self) -> StorageStatus {
        let free_bytes = (self.shared.options.free_space)();
        if free_bytes < self.shared.options.min_free_disk_bytes {
            StorageStatus::ReadOnly { free_bytes }
        } else {
            StorageStatus::Ok { free_bytes }
        }
    }

    fn snapshot(&self, dir: &Path) -> BoxFuture<'_, StoreResult<PathBuf>> {
        snapshot::snapshot(self.shared.clone(), dir.to_path_buf())
    }

    fn requests(&self) -> Arc<dyn RequestStore> {
        Arc::new(requests::Requests {
            shared: self.shared.clone(),
        })
    }
}

/// Opens (creating, mode 0600) the database file without following symlinks,
/// checks it is a regular file of ours, tightens an existing file to 0600
/// through the open handle, and rejects symlinked SQLite sidecar files.
/// Returns the canonical path.
fn secure_database_file(path: &Path) -> StoreResult<PathBuf> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let denied = |m: &str| {
        StoreError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            m.to_owned(),
        ))
    };
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| denied("database path has no file name"))?;
    let path = parent.canonicalize()?.join(name);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(&path)
        .map_err(|e| {
            if e.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error()) {
                denied("database path is a symlink")
            } else {
                StoreError::Io(e)
            }
        })?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(denied("database path is not a regular file"));
    }
    if meta.uid() != rustix::process::getuid().as_raw() {
        return Err(denied("database file is not owned by the current user"));
    }
    if meta.mode() & 0o777 != 0o600 {
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut side = path.clone().into_os_string();
        side.push(suffix);
        if let Ok(m) = std::fs::symlink_metadata(&side)
            && !m.file_type().is_file()
        {
            return Err(denied("a SQLite sidecar file is a symlink"));
        }
    }
    Ok(path)
}
