//! `atriumd` (PLAN §3.1; contract §4, §5, §6.2, §7, §8, §10).
//!
//! The one process that owns the store, the agent sessions and the approval
//! ledger. It wires the domain crates together behind the wire protocol:
//!
//! ```text
//! adapter ──events──▶ sessions ──append──▶ store
//!                        │ (same lock)       │
//!                        ▼                   ▼ pages
//!                       hub ──live──▶ conn ◀── feeder (replay, catching up)
//!                        │              ▲
//!                        └ thread list, │ responses
//!                          inbox        │
//!        client ──frames──▶ server ──▶ commands ──▶ ledger, store, sessions
//! ```
//!
//! - `server`: the socket, the handshake and roles, each connection's reader.
//! - `conn`: each connection's two outgoing lanes and its subscriptions.
//! - `hub`: who is told what (live subscribers, the inbox, notifications).
//! - `sessions`: starting an incarnation, pumping its events into the store,
//!   closing what it left open.
//! - `commands`: every `DaemonCommand`.
//! - `upkeep`: recovery at start, the storage watch, daily snapshots.
//! - `fake`: the synthetic agent behind `AgentLaunch::Fake`.
//!
//! **One rule holds the replay/live boundary together:** an append and the
//! publishing of what it stored happen under `Inner::publish`, and a
//! subscription goes live under the same lock, after finding nothing more to
//! read. So every event reaches a subscriber exactly once, from the store or
//! from the hub.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use atrium_agents::AgentAdapter;
use atrium_core::policy::{DefaultPolicy, Ledger};
use atrium_core::store::{SqliteStore, StoreOptions};
use atrium_core::{PolicyEngine, Store, StoreError};
use atrium_host::lifecycle::{InstanceLock, LifecycleError, Paths, secure_dir};
use atrium_host::peer::AppIdentity;
use atrium_protocol::agent::Provider;
use atrium_protocol::limits::MAX_CONNECTION_QUEUE_BYTES;
use atrium_protocol::thread::{ThreadId, ThreadSummary};
use atrium_protocol::wire::ErrorCode;
use tokio::sync::{Notify, watch};

mod codec;
mod commands;
mod conn;
mod fake;
mod hub;
mod server;
mod sessions;
mod upkeep;

pub use commands::KEEP_RUNNING_SETTING;
pub use hub::{Notification, NotifyReason};
pub use upkeep::CommandNotifier;

use hub::Hub;
use sessions::Live;

pub const DAEMON_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Posts a notification to the desktop. Called off the hot path, at most
/// once per thread per `limits::NOTIFY_MIN_INTERVAL`.
pub trait Notifier: Send + Sync {
    fn notify(&self, notification: &Notification);
}

/// Posts nothing.
#[derive(Debug, Default)]
pub struct NoNotifier;

impl Notifier for NoNotifier {
    fn notify(&self, _: &Notification) {}
}

/// What the daemon accepts as "the app" (contract §4).
#[derive(Debug, Clone)]
pub enum AppIdentitySource {
    /// No identity is known: every `App` connection is refused.
    None,
    /// Compiled into release daemons.
    Fixed(AppIdentity),
    /// Development builds: `Paths::app_identity`, read at each `Hello` (the
    /// dev app rewrites it when it is rebuilt). The file must be the user's
    /// and mode 0600.
    File(PathBuf),
}

pub struct Options {
    pub paths: Paths,
    /// The user's home directory: the cwd of Home threads.
    pub home: PathBuf,
    /// Accept `ClientRole::Dev`. Only honoured by debug builds.
    pub dev_socket: bool,
    pub app_identity: AppIdentitySource,
    /// One per provider that can be launched. (`AgentLaunch::Fake` needs none.)
    pub adapters: Vec<Arc<dyn AgentAdapter>>,
    pub notifier: Arc<dyn Notifier>,
    pub store: StoreOptions,
    /// Let the "keep agents running" setting install and remove the login
    /// service. Off in tests, which must not touch the user's services.
    pub manage_service: bool,
    /// Each connection's event-lane bound. A test seam; production uses
    /// [`Options::new`]'s `MAX_CONNECTION_QUEUE_BYTES`.
    pub connection_queue_bytes: usize,
    /// How often the daemon looks whether a daily snapshot is due. A test
    /// seam; production uses [`Options::new`]'s ten minutes.
    pub snapshot_check_every: std::time::Duration,
}

impl Options {
    pub fn new(paths: Paths, home: PathBuf) -> Self {
        Self {
            paths,
            home,
            dev_socket: false,
            app_identity: AppIdentitySource::None,
            adapters: Vec::new(),
            notifier: Arc::new(NoNotifier),
            store: StoreOptions::default(),
            manage_service: false,
            connection_queue_bytes: MAX_CONNECTION_QUEUE_BYTES,
            snapshot_check_every: std::time::Duration::from_secs(10 * 60),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Where the daemon is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Running,
    /// Asked to stop: it does once no turn is running.
    Draining,
    Stopping,
}

/// A refused command: the code clients branch on, and words for people.
pub(crate) type Failure = (ErrorCode, String);

pub(crate) fn failure(code: ErrorCode, message: impl Into<String>) -> Failure {
    (code, message.into())
}

pub(crate) fn store_failure(error: StoreError) -> Failure {
    let code = match &error {
        StoreError::ReadOnly { .. } => ErrorCode::StorageReadOnly,
        StoreError::UnknownThread(_) => ErrorCode::UnknownThread,
        _ => ErrorCode::Internal,
    };
    (code, error.to_string())
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

pub(crate) struct Inner {
    pub(crate) options: Options,
    pub(crate) store: Arc<dyn Store>,
    pub(crate) ledger: Arc<Ledger>,
    pub(crate) policy: Arc<dyn PolicyEngine>,
    pub(crate) adapters: HashMap<Provider, Arc<dyn AgentAdapter>>,
    hub: Mutex<Hub>,
    /// Held for an append and the publishing of what it stored, and for a
    /// subscription going live (see the module docs).
    pub(crate) publish: tokio::sync::Mutex<()>,
    /// Held from reading a thread's summary to showing it, so the list never
    /// goes back to an older summary when two tasks refresh the same thread.
    refresh: tokio::sync::Mutex<()>,
    sessions: Mutex<HashMap<ThreadId, Arc<Live>>>,
    pub(crate) conn_ids: AtomicU64,
    pub(crate) rule_ids: AtomicU64,
    pub(crate) phase: watch::Sender<Phase>,
    /// A turn or a session ended.
    pub(crate) quieter: Notify,
    /// Home, with symlinks resolved.
    pub(crate) home: PathBuf,
    /// Atrium's data and protected paths, resolved, for the session gates.
    pub(crate) data_paths: Vec<PathBuf>,
    pub(crate) protected_paths: Vec<PathBuf>,
    /// Dropped last: it removes the socket.
    _lock: InstanceLock,
}

impl Inner {
    pub(crate) fn hub(&self) -> MutexGuard<'_, Hub> {
        self.hub.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn sessions(&self) -> MutexGuard<'_, HashMap<ThreadId, Arc<Live>>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sessions with a turn open.
    pub(crate) fn running_turns(&self) -> u32 {
        self.sessions()
            .values()
            .filter(|live| live.turn_running())
            .count() as u32
    }

    pub(crate) fn phase(&self) -> Phase {
        *self.phase.borrow()
    }

    /// Reads a thread's summary from the store and shows it to the inbox
    /// subscribers, posting the notification the change calls for. Every
    /// change to a listed thread goes through here, one at a time, so what
    /// is shown is always the newest the store had.
    pub(crate) async fn refresh_thread(&self, thread: &ThreadId) {
        let _refreshing = self.refresh.lock().await;
        let Ok(Some(summary)) = self.store.thread(thread).await else {
            return;
        };
        let notification = self.hub().thread_changed(summary, now_ms());
        if let Some(notification) = notification {
            self.options.notifier.notify(&notification);
        }
    }

    /// A thread was created: it joins the list.
    pub(crate) async fn thread_added(&self, summary: ThreadSummary) {
        let _refreshing = self.refresh.lock().await;
        self.hub().thread_added(summary);
    }

    /// A thread was archived: it leaves the list.
    pub(crate) async fn thread_removed(&self, thread: &ThreadId) {
        let _refreshing = self.refresh.lock().await;
        self.hub().thread_removed(thread);
    }

    pub(crate) fn request_stop(&self, force: bool) {
        self.phase.send_if_modified(|phase| {
            let next = match (*phase, force) {
                (Phase::Stopping, _) => return false,
                (_, true) => Phase::Stopping,
                (Phase::Draining, false) => return false,
                (Phase::Running, false) => Phase::Draining,
            };
            *phase = next;
            true
        });
        self.quieter.notify_waiters();
    }
}

/// A running daemon.
pub struct Daemon {
    inner: Arc<Inner>,
    serving: Option<tokio::task::JoinHandle<()>>,
}

fn resolved(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

impl Daemon {
    /// Takes the instance lock, opens the store, recovers what a previous
    /// daemon left running, and only then starts accepting clients.
    pub async fn start(options: Options) -> Result<Self, StartError> {
        let paths = options.paths.clone();
        secure_dir(&paths.runtime_dir)?;
        secure_dir(&paths.data_dir)?;
        // Before the stale socket is removed (contract §10).
        let lock = InstanceLock::acquire(&paths)?;
        let store: Arc<SqliteStore> =
            Arc::new(SqliteStore::open(&paths.store, options.store.clone()).await?);
        let store: Arc<dyn Store> = store;
        let ledger = Arc::new(Ledger::new(store.requests()));

        let home = resolved(&options.home);
        let beside_store = |suffix: &str| {
            let mut name = resolved(&paths.store).into_os_string();
            name.push(suffix);
            PathBuf::from(name)
        };
        let data_paths = vec![
            resolved(&paths.store),
            beside_store("-wal"),
            beside_store("-shm"),
            beside_store("-journal"),
            resolved(&paths.snapshots_dir),
            resolved(&paths.logs_dir),
        ];
        let mut protected_paths = vec![resolved(&paths.runtime_dir), resolved(&paths.data_dir)];
        if let Ok(exe) = std::env::current_exe() {
            protected_paths.push(resolved(&exe));
        }

        let adapters = options
            .adapters
            .iter()
            .map(|adapter| (adapter.provider(), adapter.clone()))
            .collect();
        let (phase, _) = watch::channel(Phase::Running);
        let inner = Arc::new(Inner {
            options,
            store,
            ledger,
            policy: Arc::new(DefaultPolicy::new()),
            adapters,
            hub: Mutex::new(Hub::default()),
            publish: tokio::sync::Mutex::new(()),
            refresh: tokio::sync::Mutex::new(()),
            sessions: Mutex::new(HashMap::new()),
            conn_ids: AtomicU64::new(1),
            rule_ids: AtomicU64::new(1),
            phase,
            quieter: Notify::new(),
            home,
            data_paths,
            protected_paths,
            _lock: lock,
        });

        // Before any client is accepted or any session starts (contract §5).
        upkeep::recover(&inner).await?;
        let threads = inner.store.threads().await?;
        inner.hub().load(threads);

        let listener = server::bind(&inner.options.paths.socket)?;
        tokio::spawn(upkeep::watch_storage(inner.clone()));
        tokio::spawn(upkeep::snapshot_daily(inner.clone()));
        let serving = tokio::spawn({
            let inner = inner.clone();
            async move {
                server::serve(inner.clone(), listener).await;
                // No client is left: end the sessions, storing their
                // closing events.
                sessions::stop_all(&inner).await;
            }
        });
        Ok(Self {
            inner,
            serving: Some(serving),
        })
    }

    pub fn socket(&self) -> &Path {
        &self.inner.options.paths.socket
    }

    /// `force: false` stops once no turn is running (`SIGTERM`); `true`
    /// stops sessions now, resumable (`SIGINT`).
    pub fn stop(&self, force: bool) {
        self.inner.request_stop(force);
    }

    /// Returns once the daemon has stopped: it was asked to (by
    /// [`Daemon::stop`] or `ShutdownDaemon`) and, unless forced, no turn was
    /// running; its connections are closed and its sessions ended. Call
    /// [`Daemon::stopped`] next, which lets go of the socket and the lock.
    pub async fn served(&mut self) {
        if let Some(serving) = self.serving.as_mut() {
            let _ = serving.await;
            self.serving = None;
        }
    }

    /// Waits until the daemon has been asked to stop and has stopped:
    /// connections closed, sessions ended and their closing events stored,
    /// the socket removed.
    pub async fn stopped(mut self) {
        self.served().await;
        // The lock (and with it the socket) goes when the last task holding
        // the state lets go. Nothing is left running by now, but a task that
        // has finished may not have been dropped yet.
        let mut inner = self.inner;
        for _ in 0..200 {
            match Arc::try_unwrap(inner) {
                Ok(last) => {
                    drop(last);
                    return;
                }
                Err(shared) => inner = shared,
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        eprintln!("atriumd: something still held the daemon's state at exit");
    }
}
