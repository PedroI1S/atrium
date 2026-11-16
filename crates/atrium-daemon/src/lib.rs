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

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

pub use commands::{
    KEEP_RUNNING_SETTING, NOTIFY_APPROVAL_SETTING, NOTIFY_FAILED_SETTING, NOTIFY_QUESTION_SETTING,
};
pub use hub::{Notification, NotifyReason};
pub use upkeep::CommandNotifier;

use hub::Hub;
use sessions::Live;

pub const DAEMON_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Environment variable that disables `atriumd`'s automatic `PATH` extension
/// when set to `"1"`.
pub const NO_PATH_EXTRAS_ENV: &str = "ATRIUM_NO_PATH_EXTRAS";

/// Default `PATH` additions for GUI and service launches whose inherited `PATH`
/// is minimal (`/usr/bin:/bin:/usr/sbin:/sbin`).
///
/// Relative entries are resolved under `$HOME`; absolute entries are used as
/// given. User-local directories come first so a user-installed CLI precedes
/// Homebrew or system directories.
pub const DEFAULT_PATH_EXTRAS: &[&str] = &[
    ".local/bin",
    ".claude/local",
    ".npm-global/bin",
    ".bun/bin",
    ".cargo/bin",
    "/opt/homebrew/bin",
    "/usr/local/bin",
];

/// Maximum time `atriumd` waits for the user's login shell to print `PATH`.
pub const LOGIN_SHELL_PATH_TIMEOUT: Duration = Duration::from_millis(2000);

const PATH_START_MARKER: &[u8] = b"__ATRIUM_PATH_START__";
const PATH_END_MARKER: &[u8] = b"__ATRIUM_PATH_END__";
const MAX_LOGIN_SHELL_OUTPUT: u64 = 64 * 1024;

/// Returns `true` when `shell` refers to `fish`.
pub fn is_fish_shell(shell: &Path) -> bool {
    shell
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("fish"))
}

/// Command passed to `$SHELL -ilc` to print `PATH` between sentinel markers.
pub fn login_shell_path_command(shell: &Path) -> &'static str {
    if is_fish_shell(shell) {
        "printf '__ATRIUM_PATH_START__'; string join : -- $PATH; printf '__ATRIUM_PATH_END__'"
    } else {
        "printf '__ATRIUM_PATH_START__%s__ATRIUM_PATH_END__' \"$PATH\""
    }
}

/// Extracts the raw `PATH` bytes wrapped between the sentinel markers, ignoring
/// any shell startup noise (MOTD, prompts) printed before or after.
pub fn extract_marked_path(output: &[u8]) -> Option<OsString> {
    let start = output
        .windows(PATH_START_MARKER.len())
        .position(|w| w == PATH_START_MARKER)?
        + PATH_START_MARKER.len();
    let rest = &output[start..];
    let end = rest
        .windows(PATH_END_MARKER.len())
        .position(|w| w == PATH_END_MARKER)?;
    let slice = &rest[..end];
    if slice.is_empty() {
        return None;
    }
    Some(OsStr::from_bytes(slice).to_os_string())
}

/// Runs `$SHELL -ilc` with a short timeout to discover the user's login-shell
/// `PATH` (for `nvm`, `asdf`, `mise`, or custom init scripts). Never fails
/// startup: returns `None` if `shell` is unset, fails to spawn, exits non-zero,
/// times out, or emits no marked `PATH`.
pub fn discover_login_shell_path(shell: Option<&OsStr>, timeout: Duration) -> Option<OsString> {
    let shell = shell.filter(|s| !s.is_empty())?;
    let shell_path = Path::new(shell);
    let mut cmd = Command::new(shell_path);
    cmd.args(["-ilc", login_shell_path_command(shell_path)])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = cmd.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.take(MAX_LOGIN_SHELL_OUTPUT).read_to_end(&mut buf);
        let _ = tx.send(buf);
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    let bytes = rx
        .recv_timeout(remaining.max(Duration::from_millis(50)))
        .ok()?;
    extract_marked_path(&bytes)
}

/// Merges `existing` `PATH` entries with `candidates` that satisfy `exists`.
///
/// Keeps every non-empty entry from `existing` first, in order, then appends
/// each candidate (joining relative entries onto `home`, or skipping them if
/// `home` is empty) that `exists` accepts and is not already present.
/// De-duplicates entries (including trailing-slash variants) while preserving
/// the first occurrence's raw bytes, including non-UTF-8 entries.
pub fn provider_path<'a>(
    existing: Option<&OsStr>,
    home: &Path,
    candidates: impl IntoIterator<Item = &'a str>,
    exists: impl Fn(&Path) -> bool,
) -> OsString {
    provider_path_with_shell(existing, None, home, candidates, exists)
}

/// Like [`provider_path`], also merging absolute directories from `shell_path`
/// (the login shell's `PATH`). Order: `existing` entries first, then
/// `$HOME`-relative `candidates`, then `shell_path` entries, then absolute
/// `candidates`.
pub fn provider_path_with_shell<'a>(
    existing: Option<&OsStr>,
    shell_path: Option<&OsStr>,
    home: &Path,
    candidates: impl IntoIterator<Item = &'a str>,
    exists: impl Fn(&Path) -> bool,
) -> OsString {
    let mut entries: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    let mut push_if_new = |resolved: PathBuf, check_exists: bool| {
        if resolved.as_os_str().is_empty() || seen.contains(&resolved) {
            return;
        }
        if std::env::join_paths(std::iter::once(&resolved)).is_err() {
            return;
        }
        if (!check_exists || exists(&resolved)) && seen.insert(resolved.clone()) {
            entries.push(resolved);
        }
    };

    if let Some(raw) = existing {
        for entry in std::env::split_paths(raw) {
            push_if_new(entry, false);
        }
    }

    let has_home = !home.as_os_str().is_empty();
    let mut absolute_candidates: Vec<PathBuf> = Vec::new();
    for candidate in candidates {
        let candidate_path = Path::new(candidate);
        if candidate_path.is_absolute() {
            absolute_candidates.push(candidate_path.to_path_buf());
        } else if has_home {
            push_if_new(home.join(candidate_path), true);
        }
    }

    if let Some(raw) = shell_path {
        for entry in std::env::split_paths(raw) {
            if entry.is_absolute() {
                push_if_new(entry, true);
            }
        }
    }

    for resolved in absolute_candidates {
        push_if_new(resolved, true);
    }

    std::env::join_paths(&entries)
        .unwrap_or_else(|_| existing.map(OsStr::to_os_string).unwrap_or_default())
}

/// Slow commands that may run beside their connections' readers at once.
const MAX_SLOW_COMMANDS: usize = 8;

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
    /// Wraps the opened store before anything uses it. A test seam;
    /// production uses [`Options::new`]'s `None`.
    #[allow(clippy::type_complexity)]
    pub store_wrapper: Option<Arc<dyn Fn(Arc<dyn Store>) -> Arc<dyn Store> + Send + Sync>>,
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
            store_wrapper: None,
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
    /// Slow commands (`commands::is_slow`) running beside their
    /// connections' readers, across the daemon.
    pub(crate) slow: Arc<tokio::sync::Semaphore>,
    /// Held while the keep-running setting and the login service change.
    pub(crate) service: tokio::sync::Mutex<()>,
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
        secure_dir(&paths.logs_dir)?;
        // Before the stale socket is removed (contract §10).
        let lock = InstanceLock::acquire(&paths)?;
        let store: Arc<SqliteStore> =
            Arc::new(SqliteStore::open(&paths.store, options.store.clone()).await?);
        let store: Arc<dyn Store> = match &options.store_wrapper {
            Some(wrap) => wrap(store),
            None => store,
        };
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
            slow: Arc::new(tokio::sync::Semaphore::new(MAX_SLOW_COMMANDS)),
            service: tokio::sync::Mutex::new(()),
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
        commands::reconcile_service(&inner).await;
        let threads = inner.store.threads().await?;
        let settings = inner.store.settings().await?;
        {
            let mut hub = inner.hub();
            hub.load(threads);
            hub.load_notify_settings(&settings);
        }

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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use super::{
        DEFAULT_PATH_EXTRAS, extract_marked_path, is_fish_shell, login_shell_path_command,
        provider_path, provider_path_with_shell,
    };

    #[test]
    fn provider_path_keeps_existing_first_and_appends_in_order() {
        let home = Path::new("/home/alice");
        let allowed: HashSet<&Path> = [
            Path::new("/home/alice/.local/bin"),
            Path::new("/home/alice/.cargo/bin"),
            Path::new("/opt/homebrew/bin"),
        ]
        .into_iter()
        .collect();

        let merged = provider_path(
            Some(OsStr::new("/custom/bin:/usr/bin")),
            home,
            DEFAULT_PATH_EXTRAS.iter().copied(),
            |path| allowed.contains(path),
        );

        let parts: Vec<PathBuf> = std::env::split_paths(&merged).collect();
        assert_eq!(
            parts,
            vec![
                PathBuf::from("/custom/bin"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/home/alice/.local/bin"),
                PathBuf::from("/home/alice/.cargo/bin"),
                PathBuf::from("/opt/homebrew/bin"),
            ]
        );
    }

    #[test]
    fn provider_path_skips_missing_dirs() {
        let merged = provider_path(
            Some(OsStr::new("/usr/bin:/bin")),
            Path::new("/home/alice"),
            DEFAULT_PATH_EXTRAS.iter().copied(),
            |_| false,
        );
        assert_eq!(merged, OsStr::new("/usr/bin:/bin"));
    }

    #[test]
    fn provider_path_deduplicates_including_trailing_slashes() {
        let home = Path::new("/home/alice");
        let merged = provider_path(
            Some(OsStr::new(
                "/home/alice/.local/bin/:/opt/homebrew/bin:/home/alice/.local/bin:/usr/bin/:/usr/bin",
            )),
            home,
            DEFAULT_PATH_EXTRAS.iter().copied(),
            |_| true,
        );

        let parts: Vec<PathBuf> = std::env::split_paths(&merged).collect();
        assert_eq!(
            parts,
            vec![
                PathBuf::from("/home/alice/.local/bin/"),
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/bin/"),
                PathBuf::from("/home/alice/.claude/local"),
                PathBuf::from("/home/alice/.npm-global/bin"),
                PathBuf::from("/home/alice/.bun/bin"),
                PathBuf::from("/home/alice/.cargo/bin"),
                PathBuf::from("/usr/local/bin"),
            ]
        );
    }

    #[test]
    fn provider_path_handles_unset_and_empty_path() {
        let home = Path::new("/home/alice");
        let only_local = |path: &Path| path == Path::new("/home/alice/.local/bin");

        let from_none = provider_path(None, home, DEFAULT_PATH_EXTRAS.iter().copied(), only_local);
        assert_eq!(from_none, OsStr::new("/home/alice/.local/bin"));

        let from_empty = provider_path(
            Some(OsStr::new("")),
            home,
            DEFAULT_PATH_EXTRAS.iter().copied(),
            only_local,
        );
        assert_eq!(from_empty, OsStr::new("/home/alice/.local/bin"));

        let from_colons = provider_path(
            Some(OsStr::new(":/usr/bin::/bin:")),
            home,
            DEFAULT_PATH_EXTRAS.iter().copied(),
            only_local,
        );
        assert_eq!(
            from_colons,
            OsStr::new("/usr/bin:/bin:/home/alice/.local/bin")
        );

        let all_empty = provider_path(None, home, DEFAULT_PATH_EXTRAS.iter().copied(), |_| false);
        assert_eq!(all_empty, OsStr::new(""));
    }

    #[test]
    fn provider_path_preserves_non_utf8_entries() {
        let existing = OsStr::from_bytes(b"/opt/\xff/bin:/usr/bin:/opt/\xff/bin/");
        let merged = provider_path(
            Some(existing),
            Path::new("/home/alice"),
            DEFAULT_PATH_EXTRAS.iter().copied(),
            |path| path == Path::new("/home/alice/.local/bin"),
        );

        assert_eq!(
            merged.as_bytes(),
            b"/opt/\xff/bin:/usr/bin:/home/alice/.local/bin"
        );
    }

    #[test]
    fn provider_path_skips_home_candidates_when_home_empty() {
        let merged = provider_path(
            Some(OsStr::new("/usr/bin")),
            Path::new(""),
            DEFAULT_PATH_EXTRAS.iter().copied(),
            |_| true,
        );
        assert_eq!(
            merged,
            OsStr::new("/usr/bin:/opt/homebrew/bin:/usr/local/bin")
        );
    }

    #[test]
    fn provider_path_with_shell_merges_and_deduplicates_in_order() {
        let home = Path::new("/home/alice");
        let existing = OsStr::new("/custom/bin:/usr/bin");
        let shell = OsStr::new(
            "/home/alice/.nvm/versions/node/v22/bin:relative/dir:/usr/bin:/opt/homebrew/bin:/home/alice/.local/bin",
        );
        let merged = provider_path_with_shell(
            Some(existing),
            Some(shell),
            home,
            DEFAULT_PATH_EXTRAS.iter().copied(),
            |p| {
                matches!(
                    p.to_str(),
                    Some(
                        "/home/alice/.local/bin"
                            | "/home/alice/.nvm/versions/node/v22/bin"
                            | "/opt/homebrew/bin"
                            | "/usr/local/bin"
                    )
                )
            },
        );
        let parts: Vec<PathBuf> = std::env::split_paths(&merged).collect();
        assert_eq!(
            parts,
            vec![
                PathBuf::from("/custom/bin"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/home/alice/.local/bin"),
                PathBuf::from("/home/alice/.nvm/versions/node/v22/bin"),
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin"),
            ]
        );
    }

    #[test]
    fn fish_shell_detection_and_marker_extraction() {
        assert!(is_fish_shell(Path::new("/opt/homebrew/bin/fish")));
        assert!(is_fish_shell(Path::new("fish")));
        assert!(!is_fish_shell(Path::new("/bin/zsh")));
        assert!(!is_fish_shell(Path::new("/bin/bash")));

        assert!(
            login_shell_path_command(Path::new("/opt/homebrew/bin/fish")).contains("string join")
        );
        assert!(login_shell_path_command(Path::new("/bin/zsh")).contains("$PATH"));

        let noisy = b"Welcome to fish!\n__ATRIUM_PATH_START__/nvm/bin:/opt/\xff/bin__ATRIUM_PATH_END__\nbye\n";
        assert_eq!(
            extract_marked_path(noisy).as_deref(),
            Some(OsStr::from_bytes(b"/nvm/bin:/opt/\xff/bin"))
        );
        assert_eq!(
            extract_marked_path(b"__ATRIUM_PATH_START____ATRIUM_PATH_END__"),
            None
        );
        assert_eq!(extract_marked_path(b"no markers here"), None);
    }
}
