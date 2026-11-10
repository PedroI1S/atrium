//! The app's connection to `atriumd` (PLAN §3.1; contract §11).
//!
//! One [`Client`] per app process. It owns the socket connection and is the
//! only place in the app that holds daemon state (D-028): the thread list,
//! the inbox, and the history of each thread some window views. Windows get
//! that state through their own bounded queues, so a webview that is hidden,
//! slow or hung can't delay another window or a command.
//!
//! ```text
//! atriumd ══ socket ══▶ Client ──▶ window "main"     queue ──▶ webview
//!                         │   └──▶ window "thread-7" queue ──▶ webview
//!                         └ commands, one response each, never retried
//! ```
//!
//! # For the bridge (M1-09a)
//!
//! - [`Client::start`] once. It connects in the background and keeps
//!   reconnecting; [`Client::connection`] and the `Connection` deliveries say
//!   where it stands.
//! - For each webview, [`Client::open_window`] and a task that forwards
//!   [`Window::recv`] to the webview, `seq` and JSON as they are.
//! - The webview applies a delivery, then acknowledges it
//!   ([`WindowHandle::ack`]). It asks for what it shows with
//!   [`WindowHandle::show_inbox`] and [`WindowHandle::view`], and pages older
//!   history with [`WindowHandle::page_before`].
//! - A [`Delivery::Snapshot`] replaces the window's state for that thread; a
//!   [`Delivery::Inbox`] replaces its thread list and inbox. Both can arrive
//!   at any time: after a reconnect, or after the window fell behind.
//! - Commands go through [`Client::command`], from `invoke`, never through a
//!   window's queue.
//!
//! The state machine is in `state.rs`, the history cache in `cache.rs`, the
//! socket in `connection.rs`.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use atrium_protocol::thread::ThreadId;
use atrium_protocol::wire::{ClientRole, DaemonCommand, EventId};
use serde_json::Value;
use tokio::sync::{Notify, mpsc, oneshot, watch};

mod cache;
mod codec;
mod connection;
mod delivery;
mod state;

pub use delivery::{CommandError, ConnectionState, Delivery, Outgoing, Page};

use connection::Control;
use state::{Limits, State, Take};

/// The most history the client holds for one thread, counted as the events'
/// size on the wire after folding. A thread past this loses its oldest part
/// from the client (`truncated` in snapshots and pages); the daemon still
/// has it.
pub const MAX_CACHED_THREAD_BYTES: usize = 64 * 1024 * 1024;

/// Starts `atriumd` (the app passes `atrium_host::lifecycle::spawn_detached`
/// with its paths). Called from a blocking thread when nothing is listening
/// on the socket.
pub type StartDaemon = Arc<dyn Fn() -> io::Result<()> + Send + Sync>;

#[derive(Clone)]
pub struct ClientOptions {
    /// The daemon's socket (`atrium_host::lifecycle::Paths::socket`).
    pub socket: PathBuf,
    /// Sent in `Hello`, for the daemon's log.
    pub name: String,
    /// `App` for the app; `Dev` for tests and harnesses.
    pub role: ClientRole,
    pub start_daemon: Option<StartDaemon>,
}

impl fmt::Debug for ClientOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientOptions")
            .field("socket", &self.socket)
            .field("name", &self.name)
            .field("start_daemon", &self.start_daemon.is_some())
            .finish_non_exhaustive()
    }
}

pub(crate) struct Shared {
    state: Mutex<State>,
    connection: watch::Sender<ConnectionState>,
}

impl Shared {
    /// The state is only ever changed by short, non-panicking sections; if
    /// one did panic, carrying on with the state is better than poisoning
    /// every window.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn with<T>(&self, change: impl FnOnce(&mut State) -> T) -> T {
        change(&mut self.lock())
    }

    /// Like [`Shared::with`], for changes that can move the connection
    /// state: publishes it afterwards.
    pub(crate) fn with_connection<T>(&self, change: impl FnOnce(&mut State) -> T) -> T {
        let mut state = self.lock();
        let out = change(&mut state);
        self.connection.send_if_modified(|current| {
            let changed = current != state.connection();
            if changed {
                *current = state.connection().clone();
            }
            changed
        });
        out
    }
}

/// The connection to the daemon. Cheap to clone; the connection ends when
/// the last clone is dropped or [`Client::shutdown`] is called.
#[derive(Clone)]
pub struct Client {
    shared: Arc<Shared>,
    control: mpsc::UnboundedSender<Control>,
}

impl Client {
    /// Starts connecting, in a task on the current Tokio runtime, and
    /// returns at once.
    pub fn start(options: ClientOptions) -> Self {
        Self::start_with(options, Limits::default())
    }

    fn start_with(options: ClientOptions, limits: Limits) -> Self {
        let state = State::new(limits);
        let (connection, _) = watch::channel(state.connection().clone());
        let shared = Arc::new(Shared {
            state: Mutex::new(state),
            connection,
        });
        let (control, commands) = mpsc::unbounded_channel();
        tokio::spawn(connection::run(shared.clone(), options, commands));
        Self { shared, control }
    }

    /// The connection's state, to watch.
    pub fn connection(&self) -> watch::Receiver<ConnectionState> {
        self.shared.connection.subscribe()
    }

    /// Sends a command and waits for the daemon's answer: its value, or why
    /// it was refused.
    ///
    /// A command is sent once. If the connection drops first, this returns
    /// [`CommandError::UnknownOutcome`] and the caller decides, after
    /// looking at the state again, whether to send it again. Dropping the
    /// returned future doesn't take the command back.
    pub async fn command(&self, command: DaemonCommand) -> Result<Value, CommandError> {
        let (reply, answer) = oneshot::channel();
        self.shared.with(|state| state.command(command, reply))?;
        answer.await.unwrap_or(Err(CommandError::UnknownOutcome))
    }

    /// Registers a window and returns its receiving end. A window already
    /// open under `label` is replaced: its `recv` ends and its handles do
    /// nothing (a webview that reloaded starts over).
    pub fn open_window(&self, label: &str) -> Window {
        let (generation, notify) = self.shared.with(|state| state.open_window(label));
        Window {
            handle: WindowHandle {
                shared: self.shared.clone(),
                label: label.into(),
                generation,
            },
            notify,
        }
    }

    /// A handle to an open window, for `invoke` handlers.
    pub fn window(&self, label: &str) -> Option<WindowHandle> {
        let (generation, _) = self.shared.with(|state| state.has_window(label))?;
        Some(WindowHandle {
            shared: self.shared.clone(),
            label: label.into(),
            generation,
        })
    }

    /// Tries to connect again now: after `Incompatible` or `Failed`, which
    /// are not retried on their own, or to cut a backoff short.
    pub fn reconnect(&self) {
        let _ = self.control.send(Control::Reconnect);
    }

    /// Closes the connection. Windows receive what is queued, then end.
    pub fn shutdown(&self) {
        let _ = self.control.send(Control::Shutdown);
    }

    /// Bytes of history held for a thread some window views.
    pub fn cached_bytes(&self, thread_id: &ThreadId) -> Option<usize> {
        self.shared
            .with(|state| state.cached(thread_id))
            .map(|(bytes, _)| bytes)
    }
}

/// A window's receiving end. Dropping it closes the window.
pub struct Window {
    handle: WindowHandle,
    notify: Arc<Notify>,
}

impl Window {
    /// The next delivery, in order. `None` once the window was replaced or
    /// the client shut down. Cancellation-safe: a delivery is either
    /// returned or still queued.
    ///
    /// What was returned and not yet acknowledged counts against the
    /// window's queue (`limits::MAX_WINDOW_QUEUE_BYTES`), so the consumer
    /// must acknowledge ([`WindowHandle::ack`]) as it applies deliveries.
    pub async fn recv(&mut self) -> Option<Outgoing> {
        loop {
            let handle = &self.handle;
            match handle
                .shared
                .with(|state| state.take(&handle.label, handle.generation))
            {
                Take::Item(outgoing) => return Some(outgoing),
                Take::Closed => return None,
                // A wake-up sent before this point is kept by `Notify`.
                Take::Empty => self.notify.notified().await,
            }
        }
    }

    pub fn handle(&self) -> WindowHandle {
        self.handle.clone()
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        let handle = &self.handle;
        handle
            .shared
            .with(|state| state.close_window(&handle.label, handle.generation));
    }
}

/// What a window can ask of the client. Clonable; every method does nothing
/// once the window is closed or replaced.
#[derive(Clone)]
pub struct WindowHandle {
    shared: Arc<Shared>,
    label: Arc<str>,
    generation: u64,
}

impl WindowHandle {
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The window has applied every delivery up to and including `seq`.
    pub fn ack(&self, seq: u64) {
        self.shared
            .with(|state| state.ack(&self.label, self.generation, seq));
    }

    /// Whether the window shows the thread list and the inbox. Turning it on
    /// sends [`Delivery::Inbox`], then updates.
    pub fn show_inbox(&self, show: bool) {
        self.shared
            .with(|state| state.show_inbox(&self.label, self.generation, show));
    }

    /// Starts viewing a thread: a [`Delivery::Snapshot`] follows once the
    /// thread is loaded (or [`Delivery::ThreadFailed`]), then its events.
    /// Viewing a thread again asks for a fresh snapshot.
    pub fn view(&self, thread_id: ThreadId) {
        self.shared
            .with(|state| state.view(&self.label, self.generation, thread_id));
    }

    pub fn unview(&self, thread_id: &ThreadId) {
        self.shared
            .with(|state| state.unview(&self.label, self.generation, thread_id));
    }

    /// History before the event `before` (the first event the view has), up
    /// to about `max_bytes`, in whole segments so that no item is cut.
    /// `None` when the window doesn't view the thread.
    pub fn page_before(
        &self,
        thread_id: &ThreadId,
        before: EventId,
        max_bytes: usize,
    ) -> Option<Page> {
        self.shared.with(|state| {
            state.page_before(&self.label, self.generation, thread_id, before, max_bytes)
        })
    }

    /// Whether the window is owed state it hasn't been sent yet: it fell
    /// behind, or a thread it views is still loading.
    pub fn needs_resync(&self) -> bool {
        self.shared
            .with(|state| state.is_stale(&self.label, self.generation))
    }

    /// How many times the window fell behind and lost its queue.
    pub fn resyncs(&self) -> u64 {
        self.shared
            .with(|state| state.resyncs(&self.label, self.generation))
    }
}
