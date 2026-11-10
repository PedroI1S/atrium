//! Everything the client knows, and every decision it makes, with no I/O.
//!
//! [`State`] sits behind one mutex. The connection task feeds it the
//! daemon's messages; windows take deliveries from it and acknowledge them;
//! callers send commands through it. Nothing here waits: a method changes the
//! state, queues frames for the daemon on an unbounded channel, wakes the
//! windows that have something new, and returns.
//!
//! # Threads
//!
//! A thread is subscribed to once, while some window views it, and its
//! history is kept in a [`ThreadCache`]. Its [`Phase`]:
//!
//! ```text
//! Detached ──connected──▶ Subscribing ──Ok──▶ Replaying ──ReplayDone──▶ Live
//!     ▲                        │ Err                │                     │
//!     └────── disconnected ────┴──▶ Failed          └── lagged ──▶ Subscribing
//! ```
//!
//! Events are applied only while `Replaying` or `Live`. While `Subscribing`
//! they belong to an earlier subscription and are dropped: the new one
//! replays from the cursor. The client relies on the daemon sending nothing
//! of an earlier subscription after it has answered the new `Subscribe`
//! (D-058). So a `ReplayDone` while `Replaying` is this replay's: one that
//! doesn't end where the answer said, or comes before the events up to
//! there, is a protocol violation.
//!
//! # Windows
//!
//! Each window has a queue of deliveries, bounded over what it has not
//! acknowledged: those waiting to be taken and those taken ("in flight"; a
//! webview that stopped reading still holds them). A live delivery that
//! doesn't fit empties the waiting part and marks everything the window
//! shows as stale. Stale things are sent again in full when there is room,
//! which is when the window acknowledges: the connection state, the inbox,
//! and a snapshot of each thread it views. So a window that falls behind
//! skips ahead to the present instead of holding anyone else back.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::Arc;

use atrium_protocol::limits;
use atrium_protocol::thread::{InboxItem, ThreadId, ThreadSummary};
use atrium_protocol::wire::{
    ClientMessage, CommandResult, DaemonCommand, ErrorCode, EventId, ServerMessage, StoredEvent,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Notify, mpsc, oneshot};

use crate::cache::ThreadCache;
use crate::codec::{self, Frame};
use crate::{CommandError, ConnectionState, Delivery, MAX_CACHED_THREAD_BYTES, Outgoing, Page};

/// The byte bounds, changeable so tests don't need megabytes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub(crate) window_queue_bytes: usize,
    pub(crate) snapshot_bytes: usize,
    pub(crate) cache_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            window_queue_bytes: limits::MAX_WINDOW_QUEUE_BYTES,
            snapshot_bytes: limits::MAX_LOADED_TRANSCRIPT_BYTES,
            cache_bytes: MAX_CACHED_THREAD_BYTES,
        }
    }
}

enum Phase {
    /// No connection. The cache is kept for when it returns.
    Detached,
    /// `Subscribe` sent; `resumed` when it carried a cursor.
    Subscribing {
        request: u64,
        resumed: bool,
    },
    /// Subscribed; events up to `until` are on their way.
    Replaying {
        until: Option<EventId>,
    },
    Live,
    Failed {
        code: ErrorCode,
        message: String,
    },
}

/// What a window has of a thread it views.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    /// Nothing current: a snapshot (or the failure) is owed.
    Stale,
    /// A snapshot, and every event since.
    Live,
    /// It was told the thread failed.
    Failed,
}

struct ThreadSub {
    cache: ThreadCache,
    phase: Phase,
    /// Labels of the windows viewing it.
    viewers: BTreeSet<String>,
}

struct Listed {
    summary: ThreadSummary,
    /// Its size as counted against a window's queue.
    bytes: usize,
}

struct Queued {
    json: Arc<str>,
    bytes: usize,
}

struct WindowState {
    /// Tells this window's handles from those of an earlier window with the
    /// same label.
    generation: u64,
    notify: Arc<Notify>,
    /// The number of the last delivery taken.
    seq: u64,
    waiting: VecDeque<Queued>,
    waiting_bytes: usize,
    /// Taken and not acknowledged: `(seq, bytes)`, oldest first.
    in_flight: VecDeque<(u64, usize)>,
    in_flight_bytes: usize,
    connection_stale: bool,
    wants_inbox: bool,
    inbox_stale: bool,
    threads: BTreeMap<ThreadId, View>,
    resyncs: u64,
}

impl WindowState {
    fn fits(&self, bytes: usize, max: usize) -> bool {
        let used = self.waiting_bytes + self.in_flight_bytes;
        // An empty queue takes anything, or a delivery larger than the bound
        // could never be sent.
        used == 0 || used.saturating_add(bytes) <= max
    }

    fn push(&mut self, json: Arc<str>, bytes: usize) {
        self.waiting.push_back(Queued { json, bytes });
        self.waiting_bytes += bytes;
        self.notify.notify_one();
    }

    /// A live delivery: queued if it fits, otherwise the window falls behind.
    fn send(&mut self, json: Arc<str>, bytes: usize, max: usize) {
        if self.fits(bytes, max) {
            self.push(json, bytes);
        } else {
            self.fall_behind();
        }
    }

    fn fall_behind(&mut self) {
        self.waiting.clear();
        self.waiting_bytes = 0;
        self.connection_stale = true;
        self.inbox_stale = self.wants_inbox;
        for view in self.threads.values_mut() {
            if *view == View::Live {
                *view = View::Stale;
            }
        }
        self.resyncs += 1;
    }

    fn is_stale(&self) -> bool {
        self.connection_stale
            || self.inbox_stale
            || self.threads.values().any(|view| *view == View::Stale)
    }
}

struct PendingCommand {
    reply: oneshot::Sender<Result<Value, CommandError>>,
    /// Set for `ArchiveThread`: the thread to drop from the list on success.
    archives: Option<ThreadId>,
}

/// What windows are shown, as opposed to the windows themselves.
struct Known {
    limits: Limits,
    connection: ConnectionState,
    threads: BTreeMap<ThreadId, Listed>,
    inbox: Vec<InboxItem>,
    inbox_bytes: usize,
    /// The inbox subscription has answered on this connection or an earlier
    /// one.
    inbox_loaded: bool,
    subs: BTreeMap<ThreadId, ThreadSub>,
}

pub(crate) enum Take {
    Item(Outgoing),
    Empty,
    /// The window was closed or replaced, or the client shut down.
    Closed,
}

pub(crate) struct State {
    known: Known,
    windows: BTreeMap<String, WindowState>,
    generations: u64,
    /// Frames to the daemon; `None` while there is no connection.
    writer: Option<mpsc::UnboundedSender<Frame>>,
    requests: u64,
    commands: HashMap<u64, PendingCommand>,
    subscribes: HashMap<u64, ThreadId>,
    inbox_request: Option<u64>,
    closed: bool,
}

/// Sends stale state to a window, in order, for as long as it fits.
fn pump(window: &mut WindowState, known: &Known) {
    let max = known.limits.window_queue_bytes;
    if window.connection_stale {
        let Some(json) = to_json(&Delivery::Connection {
            state: known.connection.clone(),
        }) else {
            return;
        };
        if !window.fits(json.len(), max) {
            return;
        }
        window.connection_stale = false;
        let bytes = json.len();
        window.push(json, bytes);
    }
    if window.inbox_stale && known.inbox_loaded {
        let bytes = known.threads.values().map(|t| t.bytes).sum::<usize>() + known.inbox_bytes;
        if !window.fits(bytes, max) {
            return;
        }
        let Some(json) = inbox_json(known) else {
            return;
        };
        window.inbox_stale = false;
        window.push(json, bytes);
    }
    let stale: Vec<ThreadId> = window
        .threads
        .iter()
        .filter(|(_, view)| **view == View::Stale)
        .map(|(thread_id, _)| thread_id.clone())
        .collect();
    for thread_id in stale {
        let Some(sub) = known.subs.get(&thread_id) else {
            continue;
        };
        let (json, shown) = match &sub.phase {
            Phase::Live => {
                let span = sub.cache.tail(known.limits.snapshot_bytes);
                if !window.fits(span.bytes, max) {
                    return;
                }
                let Some(json) = sub.cache.snapshot_json(&thread_id, span) else {
                    continue;
                };
                (Arc::from(json), View::Live)
            }
            Phase::Failed { code, message } => {
                let Some(json) = to_json(&Delivery::ThreadFailed {
                    thread_id: thread_id.clone(),
                    code: *code,
                    message: message.clone(),
                }) else {
                    continue;
                };
                if !window.fits(json.len(), max) {
                    return;
                }
                (json, View::Failed)
            }
            // Still loading: the snapshot comes when the replay is done.
            Phase::Detached | Phase::Subscribing { .. } | Phase::Replaying { .. } => continue,
        };
        let bytes = json.len();
        window.push(json, bytes);
        window.threads.insert(thread_id, shown);
    }
}

fn to_json<T: Serialize>(value: &T) -> Option<Arc<str>> {
    serde_json::to_string(value).ok().map(Arc::from)
}

/// `Delivery::Inbox` from what is known, without copying it.
fn inbox_json(known: &Known) -> Option<Arc<str>> {
    #[derive(Serialize)]
    struct Inbox<'a> {
        r#type: &'static str,
        threads: Vec<&'a ThreadSummary>,
        inbox: &'a [InboxItem],
    }
    to_json(&Inbox {
        r#type: "inbox",
        threads: known.threads.values().map(|t| &t.summary).collect(),
        inbox: &known.inbox,
    })
}

fn size_of<T: Serialize>(value: &T) -> usize {
    serde_json::to_vec(value).map_or(0, |json| json.len())
}

impl State {
    pub(crate) fn new(limits: Limits) -> Self {
        Self {
            known: Known {
                limits,
                connection: ConnectionState::Connecting { attempt: 1 },
                threads: BTreeMap::new(),
                inbox: Vec::new(),
                inbox_bytes: 0,
                inbox_loaded: false,
                subs: BTreeMap::new(),
            },
            windows: BTreeMap::new(),
            generations: 0,
            writer: None,
            requests: 0,
            commands: HashMap::new(),
            subscribes: HashMap::new(),
            inbox_request: None,
            closed: false,
        }
    }

    pub(crate) fn connection(&self) -> &ConnectionState {
        &self.known.connection
    }

    fn request(&mut self) -> u64 {
        self.requests += 1;
        self.requests
    }

    fn send(&mut self, message: &ClientMessage) {
        if let Some(writer) = &self.writer
            && let Ok(frame) = codec::encode(message)
        {
            // A closed channel means the connection is going down; the
            // disconnect that follows cleans up.
            let _ = writer.send(frame);
        }
    }

    fn pump_all(&mut self) {
        for window in self.windows.values_mut() {
            pump(window, &self.known);
        }
    }

    fn pump_viewers(&mut self, thread_id: &ThreadId) {
        let Some(sub) = self.known.subs.get(thread_id) else {
            return;
        };
        for label in &sub.viewers {
            if let Some(window) = self.windows.get_mut(label) {
                pump(window, &self.known);
            }
        }
    }

    // -----------------------------------------------------------------------
    // The connection
    // -----------------------------------------------------------------------

    /// The handshake succeeded: everything is subscribed to again, threads
    /// from where their caches end.
    pub(crate) fn connected(
        &mut self,
        writer: mpsc::UnboundedSender<Frame>,
        daemon_version: String,
        daemon_pid: u32,
    ) {
        if self.closed {
            return;
        }
        self.writer = Some(writer);
        let request = self.request();
        self.inbox_request = Some(request);
        self.send(&ClientMessage::SubscribeInbox {
            request_id: request,
        });
        let threads: Vec<ThreadId> = self.known.subs.keys().cloned().collect();
        for thread_id in threads {
            self.subscribe(&thread_id);
        }
        self.set_connection(ConnectionState::Connected {
            daemon_version,
            daemon_pid,
        });
    }

    /// The connection is gone (or never came up). Commands in flight have an
    /// unknown outcome; caches and what windows show stay as they are.
    pub(crate) fn disconnected(&mut self, state: ConnectionState) {
        self.writer = None;
        for (_, pending) in self.commands.drain() {
            let _ = pending.reply.send(Err(CommandError::UnknownOutcome));
        }
        self.subscribes.clear();
        self.inbox_request = None;
        for sub in self.known.subs.values_mut() {
            if !matches!(sub.phase, Phase::Failed { .. }) {
                sub.phase = Phase::Detached;
            }
        }
        self.set_connection(state);
    }

    pub(crate) fn set_connection(&mut self, state: ConnectionState) {
        if self.closed || self.known.connection == state {
            return;
        }
        self.known.connection = state;
        for window in self.windows.values_mut() {
            window.connection_stale = true;
        }
        self.pump_all();
    }

    /// The client is shutting down: windows drain what they have and end.
    pub(crate) fn close(&mut self) {
        self.disconnected(ConnectionState::Closed);
        self.closed = true;
        for window in self.windows.values() {
            window.notify.notify_one();
        }
    }

    /// Sends `Subscribe` for a thread from where its cache ends.
    fn subscribe(&mut self, thread_id: &ThreadId) {
        if self.writer.is_none() {
            return;
        }
        let request = self.request();
        let Some(sub) = self.known.subs.get_mut(thread_id) else {
            return;
        };
        let after = sub.cache.last();
        sub.phase = Phase::Subscribing {
            request,
            resumed: after.is_some(),
        };
        self.subscribes.insert(request, thread_id.clone());
        self.send(&ClientMessage::Subscribe {
            request_id: request,
            thread_id: thread_id.clone(),
            after,
        });
    }

    // -----------------------------------------------------------------------
    // From the daemon
    // -----------------------------------------------------------------------

    /// Applies one message; `json` is its frame's payload. `Err` is a
    /// protocol violation: the connection must be closed.
    pub(crate) fn on_message(
        &mut self,
        message: ServerMessage,
        json: Arc<str>,
    ) -> Result<(), String> {
        match message {
            ServerMessage::Event { event } => self.on_event(event, json),
            ServerMessage::ReplayDone { thread_id, last } => {
                // Outside `Replaying` it is late (the thread was left, or
                // subscribed to again) and means nothing.
                if let Some(sub) = self.known.subs.get_mut(&thread_id)
                    && let Phase::Replaying { until } = sub.phase
                {
                    // Going live on anything else would show a history with
                    // a hole; waiting for a better one would wait for ever.
                    if until != last {
                        return Err(format!(
                            "`ReplayDone` up to {last:?} for a replay up to {until:?}"
                        ));
                    }
                    if until.is_some_and(|until| sub.cache.last().is_none_or(|have| have < until)) {
                        return Err(format!(
                            "`ReplayDone` before the events up to {until:?} (the last was {:?})",
                            sub.cache.last()
                        ));
                    }
                    sub.phase = Phase::Live;
                    self.pump_viewers(&thread_id);
                }
            }
            ServerMessage::Response { request_id, result } => {
                return self.on_response(request_id, result);
            }
            ServerMessage::ThreadUpdated { summary } => {
                let bytes = json.len();
                self.known
                    .threads
                    .insert(summary.thread_id.clone(), Listed { summary, bytes });
                self.send_to_inbox_windows(json);
            }
            ServerMessage::InboxUpdated { items } => {
                self.known.inbox = items;
                self.known.inbox_bytes = json.len();
                self.send_to_inbox_windows(json);
            }
            ServerMessage::SubscriptionLagged { thread_id, .. } => {
                // The daemon dropped the subscription because this
                // connection's queue was full. Pick up from the cache.
                if self
                    .known
                    .subs
                    .get(&thread_id)
                    .is_some_and(|sub| matches!(sub.phase, Phase::Replaying { .. } | Phase::Live))
                {
                    self.subscribe(&thread_id);
                }
            }
            ServerMessage::Welcome { .. } | ServerMessage::Incompatible { .. } => {
                return Err("a handshake message after the handshake".into());
            }
        }
        Ok(())
    }

    fn on_event(&mut self, event: StoredEvent, json: Arc<str>) {
        let max = self.known.limits.window_queue_bytes;
        let Some(sub) = self.known.subs.get_mut(&event.thread_id) else {
            return;
        };
        if !matches!(sub.phase, Phase::Replaying { .. } | Phase::Live) {
            return;
        }
        let thread_id = event.thread_id.clone();
        let bytes = json.len();
        if !sub.cache.append(event, bytes) {
            return;
        }
        for label in &sub.viewers {
            if let Some(window) = self.windows.get_mut(label)
                && window.threads.get(&thread_id) == Some(&View::Live)
            {
                window.send(json.clone(), bytes, max);
            }
        }
    }

    /// A live delivery to the windows that show the inbox.
    fn send_to_inbox_windows(&mut self, json: Arc<str>) {
        let max = self.known.limits.window_queue_bytes;
        let bytes = json.len();
        for window in self.windows.values_mut() {
            // A window whose inbox is stale gets all of it soon.
            if window.wants_inbox && !window.inbox_stale {
                window.send(json.clone(), bytes, max);
            }
        }
    }

    fn on_response(&mut self, request: u64, result: CommandResult) -> Result<(), String> {
        if let Some(pending) = self.commands.remove(&request) {
            let result = match result {
                CommandResult::Ok { value } => {
                    if let Some(thread_id) = pending.archives {
                        self.archived(thread_id);
                    }
                    Ok(value)
                }
                CommandResult::Err { code, message } => {
                    Err(CommandError::Rejected { code, message })
                }
            };
            // The caller may have stopped waiting; the command ran anyway.
            let _ = pending.reply.send(result);
            return Ok(());
        }
        if let Some(thread_id) = self.subscribes.remove(&request) {
            return self.on_subscribed(thread_id, request, result);
        }
        if self.inbox_request == Some(request) {
            self.inbox_request = None;
            return self.on_inbox(result);
        }
        Err(format!(
            "a response to request {request}, which wasn't made"
        ))
    }

    fn on_subscribed(
        &mut self,
        thread_id: ThreadId,
        request: u64,
        result: CommandResult,
    ) -> Result<(), String> {
        let Some(sub) = self.known.subs.get_mut(&thread_id) else {
            return Ok(());
        };
        let Phase::Subscribing {
            request: current,
            resumed,
        } = sub.phase
        else {
            return Ok(());
        };
        if current != request {
            // An answer to a subscription that was since replaced.
            return Ok(());
        }
        match result {
            CommandResult::Ok { value } => {
                // `{"replay_until": EventId | null}`. The key must be there:
                // without the bound, the replay's end can't be recognised.
                let until = value
                    .get("replay_until")
                    .and_then(|until| Option::<EventId>::deserialize(until).ok());
                let Some(until) = until else {
                    return Err("a subscribe response without `replay_until`".into());
                };
                sub.phase = Phase::Replaying { until };
                // A window that was told the thread failed gets a snapshot.
                self.stale_views(&thread_id, View::Failed);
            }
            // The daemon doesn't have the events the cache ends with (its
            // store was restored from a snapshot, say): load the thread again.
            CommandResult::Err { code, .. } if resumed && code != ErrorCode::UnknownThread => {
                sub.cache.reset();
                self.stale_views(&thread_id, View::Live);
                self.subscribe(&thread_id);
            }
            CommandResult::Err { code, message } => {
                sub.cache.reset();
                sub.phase = Phase::Failed { code, message };
                self.stale_views(&thread_id, View::Live);
                self.pump_viewers(&thread_id);
            }
        }
        Ok(())
    }

    /// Marks the viewers' `from` views of a thread as stale.
    fn stale_views(&mut self, thread_id: &ThreadId, from: View) {
        let Some(sub) = self.known.subs.get(thread_id) else {
            return;
        };
        for label in &sub.viewers {
            if let Some(view) = self
                .windows
                .get_mut(label)
                .and_then(|window| window.threads.get_mut(thread_id))
                && *view == from
            {
                *view = View::Stale;
            }
        }
    }

    fn on_inbox(&mut self, result: CommandResult) -> Result<(), String> {
        #[derive(Deserialize)]
        struct Inbox {
            threads: Vec<ThreadSummary>,
            inbox: Vec<InboxItem>,
        }
        let inbox = match result {
            CommandResult::Ok { value } => serde_json::from_value::<Inbox>(value)
                .map_err(|e| format!("an inbox response the client can't read: {e}"))?,
            CommandResult::Err { message, .. } => {
                return Err(format!("the inbox subscription was refused: {message}"));
            }
        };
        self.known.threads = inbox
            .threads
            .into_iter()
            .map(|summary| {
                let bytes = size_of(&summary);
                (summary.thread_id.clone(), Listed { summary, bytes })
            })
            .collect();
        self.known.inbox_bytes = size_of(&inbox.inbox);
        self.known.inbox = inbox.inbox;
        self.known.inbox_loaded = true;
        for window in self.windows.values_mut() {
            window.inbox_stale = window.wants_inbox;
        }
        self.pump_all();
        Ok(())
    }

    /// `ArchiveThread` succeeded. The daemon sends no update for a thread
    /// that leaves the list, so the client tells its windows.
    fn archived(&mut self, thread_id: ThreadId) {
        if self.known.threads.remove(&thread_id).is_none() {
            return;
        }
        if let Some(json) = to_json(&Delivery::ThreadRemoved { thread_id }) {
            self.send_to_inbox_windows(json);
        }
    }

    // -----------------------------------------------------------------------
    // Commands
    // -----------------------------------------------------------------------

    /// Sends a command. Its answer goes to `reply`; an `Err` here means
    /// nothing was sent.
    pub(crate) fn command(
        &mut self,
        command: DaemonCommand,
        reply: oneshot::Sender<Result<Value, CommandError>>,
    ) -> Result<(), CommandError> {
        let Some(writer) = &self.writer else {
            return Err(CommandError::NotConnected);
        };
        let archives = match &command {
            DaemonCommand::ArchiveThread { thread_id } => Some(thread_id.clone()),
            _ => None,
        };
        let request = self.requests + 1;
        let frame = codec::encode(&ClientMessage::Command {
            request_id: request,
            command,
        })
        .map_err(|bytes| CommandError::TooLarge { bytes })?;
        writer.send(frame).map_err(|_| CommandError::NotConnected)?;
        self.requests = request;
        self.commands
            .insert(request, PendingCommand { reply, archives });
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Windows
    // -----------------------------------------------------------------------

    /// Registers a window, replacing one with the same label (its handles go
    /// dead). Returns the new window's generation and wake-up.
    pub(crate) fn open_window(&mut self, label: &str) -> (u64, Arc<Notify>) {
        if let Some(old) = self.windows.remove(label) {
            self.drop_views(label, old.threads.into_keys().collect());
            old.notify.notify_one();
        }
        self.generations += 1;
        let notify = Arc::new(Notify::new());
        let mut window = WindowState {
            generation: self.generations,
            notify: notify.clone(),
            seq: 0,
            waiting: VecDeque::new(),
            waiting_bytes: 0,
            in_flight: VecDeque::new(),
            in_flight_bytes: 0,
            connection_stale: true,
            wants_inbox: false,
            inbox_stale: false,
            threads: BTreeMap::new(),
            resyncs: 0,
        };
        pump(&mut window, &self.known);
        self.windows.insert(label.to_owned(), window);
        (self.generations, notify)
    }

    fn window(&mut self, label: &str, generation: u64) -> Option<&mut WindowState> {
        self.windows
            .get_mut(label)
            .filter(|window| window.generation == generation)
    }

    pub(crate) fn has_window(&self, label: &str) -> Option<(u64, Arc<Notify>)> {
        self.windows
            .get(label)
            .map(|window| (window.generation, window.notify.clone()))
    }

    pub(crate) fn close_window(&mut self, label: &str, generation: u64) {
        if self.window(label, generation).is_none() {
            return;
        }
        if let Some(window) = self.windows.remove(label) {
            self.drop_views(label, window.threads.into_keys().collect());
            window.notify.notify_one();
        }
    }

    /// The next delivery for a window, moved to "in flight".
    pub(crate) fn take(&mut self, label: &str, generation: u64) -> Take {
        let closed = self.closed;
        let Some(window) = self
            .windows
            .get_mut(label)
            .filter(|window| window.generation == generation)
        else {
            return Take::Closed;
        };
        // What the window is owed may fit now that it is reading again.
        pump(window, &self.known);
        match window.waiting.pop_front() {
            Some(queued) => {
                window.waiting_bytes -= queued.bytes;
                window.seq += 1;
                window.in_flight.push_back((window.seq, queued.bytes));
                window.in_flight_bytes += queued.bytes;
                Take::Item(Outgoing {
                    seq: window.seq,
                    json: queued.json,
                })
            }
            None if closed => Take::Closed,
            None => Take::Empty,
        }
    }

    /// The window applied every delivery up to `seq`.
    pub(crate) fn ack(&mut self, label: &str, generation: u64, seq: u64) {
        let Some(window) = self
            .windows
            .get_mut(label)
            .filter(|window| window.generation == generation)
        else {
            return;
        };
        while let Some(&(oldest, bytes)) = window.in_flight.front() {
            if oldest > seq {
                break;
            }
            window.in_flight.pop_front();
            window.in_flight_bytes -= bytes;
        }
        pump(window, &self.known);
    }

    pub(crate) fn show_inbox(&mut self, label: &str, generation: u64, show: bool) {
        let Some(window) = self
            .windows
            .get_mut(label)
            .filter(|window| window.generation == generation)
        else {
            return;
        };
        window.wants_inbox = show;
        window.inbox_stale = show;
        pump(window, &self.known);
    }

    /// The window starts viewing a thread (or asks for it afresh): a
    /// snapshot follows once the thread is loaded.
    pub(crate) fn view(&mut self, label: &str, generation: u64, thread_id: ThreadId) {
        let Some(window) = self.window(label, generation) else {
            return;
        };
        window.threads.insert(thread_id.clone(), View::Stale);
        match self.known.subs.get_mut(&thread_id) {
            Some(sub) => {
                sub.viewers.insert(label.to_owned());
            }
            None => {
                let cache = ThreadCache::new(self.known.limits.cache_bytes);
                self.known.subs.insert(
                    thread_id.clone(),
                    ThreadSub {
                        cache,
                        phase: Phase::Detached,
                        viewers: BTreeSet::from([label.to_owned()]),
                    },
                );
                self.subscribe(&thread_id);
            }
        }
        if let Some(window) = self.windows.get_mut(label) {
            pump(window, &self.known);
        }
    }

    pub(crate) fn unview(&mut self, label: &str, generation: u64, thread_id: &ThreadId) {
        let Some(window) = self.window(label, generation) else {
            return;
        };
        if window.threads.remove(thread_id).is_some() {
            self.drop_views(label, vec![thread_id.clone()]);
        }
    }

    /// `label` no longer views `threads`. A thread nobody views is
    /// unsubscribed and its cache freed.
    fn drop_views(&mut self, label: &str, threads: Vec<ThreadId>) {
        for thread_id in threads {
            let Some(sub) = self.known.subs.get_mut(&thread_id) else {
                continue;
            };
            sub.viewers.remove(label);
            if !sub.viewers.is_empty() {
                continue;
            }
            let subscribed = matches!(
                sub.phase,
                Phase::Subscribing { .. } | Phase::Replaying { .. } | Phase::Live
            );
            self.known.subs.remove(&thread_id);
            if subscribed {
                self.send(&ClientMessage::Unsubscribe { thread_id });
            }
        }
    }

    /// History before `before`, for a window that views the thread.
    pub(crate) fn page_before(
        &mut self,
        label: &str,
        generation: u64,
        thread_id: &ThreadId,
        before: EventId,
        max_bytes: usize,
    ) -> Option<Page> {
        let window = self.window(label, generation)?;
        if !window.threads.contains_key(thread_id) {
            return None;
        }
        let cache = &self.known.subs.get(thread_id)?.cache;
        let span = cache.before(before, max_bytes);
        Some(Page {
            events: cache.events(span),
            has_older: cache.has_older(span),
            truncated: cache.truncated(),
        })
    }

    /// Whether the window is owed state it hasn't been sent.
    pub(crate) fn is_stale(&mut self, label: &str, generation: u64) -> bool {
        self.window(label, generation)
            .is_some_and(|window| window.is_stale())
    }

    /// How many times the window fell behind and lost its queue.
    pub(crate) fn resyncs(&mut self, label: &str, generation: u64) -> u64 {
        self.window(label, generation)
            .map_or(0, |window| window.resyncs)
    }

    /// Bytes of history held for a thread, and its sequence gaps.
    pub(crate) fn cached(&self, thread_id: &ThreadId) -> Option<(usize, u64)> {
        let cache = &self.known.subs.get(thread_id)?.cache;
        Some((cache.bytes(), cache.gaps()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::tests::{completed, delta, stored, turn};
    use atrium_protocol::agent::{AgentEvent, Provider};
    use atrium_protocol::thread::{InboxReason, Location, ThreadState};
    use serde_json::json;

    const SMALL: Limits = Limits {
        window_queue_bytes: 2_000,
        snapshot_bytes: 1_000_000,
        cache_bytes: usize::MAX,
    };

    fn t() -> ThreadId {
        ThreadId("t".into())
    }

    fn connected() -> ConnectionState {
        ConnectionState::Connected {
            daemon_version: "1.0".into(),
            daemon_pid: 7,
        }
    }

    fn event(id: u64, event: AgentEvent) -> ServerMessage {
        ServerMessage::Event {
            event: stored(id, event),
        }
    }

    fn event_of(thread: &str, id: u64, event: AgentEvent) -> ServerMessage {
        let mut stored = stored(id, event);
        stored.thread_id = ThreadId(thread.into());
        ServerMessage::Event { event: stored }
    }

    fn ok(request_id: u64, value: Value) -> ServerMessage {
        ServerMessage::Response {
            request_id,
            result: CommandResult::Ok { value },
        }
    }

    fn err(request_id: u64, code: ErrorCode) -> ServerMessage {
        ServerMessage::Response {
            request_id,
            result: CommandResult::Err {
                code,
                message: "no".into(),
            },
        }
    }

    fn done(thread: &str, last: Option<u64>) -> ServerMessage {
        ServerMessage::ReplayDone {
            thread_id: ThreadId(thread.into()),
            last: last.map(EventId),
        }
    }

    fn summary(id: &str, title: &str) -> ThreadSummary {
        ThreadSummary {
            thread_id: ThreadId(id.into()),
            title: title.into(),
            provider: Provider::Claude,
            model: None,
            location: Location::Home,
            state: ThreadState::Idle,
            enforcement: None,
            incarnation: 1,
            native_session_id: None,
            last_event: None,
            unread: false,
            pending_approvals: 0,
            pending_questions: 0,
            attention_since_ms: None,
            attention_detail: None,
            created_at_ms: 1,
            updated_at_ms: 2,
        }
    }

    fn subscribe(request_id: u64, thread: &str, after: Option<u64>) -> ClientMessage {
        ClientMessage::Subscribe {
            request_id,
            thread_id: ThreadId(thread.into()),
            after: after.map(EventId),
        }
    }

    struct Win {
        label: &'static str,
        generation: u64,
    }

    /// A state with a connection whose frames the test reads.
    struct Rig {
        state: State,
        frames: mpsc::UnboundedReceiver<Frame>,
    }

    impl Rig {
        /// Connected, with the inbox subscription (request 1) not yet
        /// answered.
        fn new(limits: Limits) -> Self {
            let mut state = State::new(limits);
            let (writer, frames) = mpsc::unbounded_channel();
            state.connected(writer, "1.0".into(), 7);
            let mut rig = Self { state, frames };
            assert_eq!(
                rig.sent(),
                [ClientMessage::SubscribeInbox { request_id: 1 }]
            );
            rig
        }

        fn reconnect(&mut self) {
            let (writer, frames) = mpsc::unbounded_channel();
            self.frames = frames;
            self.state.connected(writer, "1.0".into(), 7);
        }

        /// What the client sent the daemon since the last call.
        fn sent(&mut self) -> Vec<ClientMessage> {
            let mut messages = Vec::new();
            while let Ok(frame) = self.frames.try_recv() {
                messages.push(serde_json::from_slice(&frame[5..]).expect("a client message"));
            }
            messages
        }

        /// The daemon sends `message`. Returns the frame's JSON.
        fn daemon(&mut self, message: ServerMessage) -> Arc<str> {
            let json: Arc<str> = serde_json::to_string(&message).expect("json").into();
            self.state
                .on_message(message, json.clone())
                .expect("within the protocol");
            json
        }

        fn open(&mut self, label: &'static str) -> Win {
            let (generation, _) = self.state.open_window(label);
            Win { label, generation }
        }

        fn take(&mut self, window: &Win) -> Option<Outgoing> {
            match self.state.take(window.label, window.generation) {
                Take::Item(outgoing) => Some(outgoing),
                Take::Empty | Take::Closed => None,
            }
        }

        /// Takes everything waiting, without acknowledging.
        fn take_all(&mut self, window: &Win) -> Vec<Outgoing> {
            std::iter::from_fn(|| self.take(window)).collect()
        }

        /// A window that keeps up: takes what is waiting and acknowledges
        /// it, until nothing more comes.
        fn drain(&mut self, window: &Win) -> Vec<Delivery> {
            let mut deliveries = Vec::new();
            loop {
                let taken = self.take_all(window);
                let Some(last) = taken.last() else {
                    return deliveries;
                };
                self.state.ack(window.label, window.generation, last.seq);
                deliveries.extend(taken.iter().map(|o| o.delivery().expect("a delivery")));
            }
        }

        fn view(&mut self, window: &Win, thread: &str) {
            self.state
                .view(window.label, window.generation, ThreadId(thread.into()));
        }

        /// `window` views thread "t", loaded with a turn start and the
        /// message "Hello" in two deltas (events 1 to 3), and has drained.
        fn viewing(&mut self, window: &Win, request: u64) {
            self.view(window, "t");
            assert_eq!(self.sent(), [subscribe(request, "t", None)]);
            self.daemon(ok(request, json!({ "replay_until": 3 })));
            self.daemon(event(1, turn(1)));
            self.daemon(event(2, delta("m1", "Hel")));
            self.daemon(event(3, delta("m1", "lo")));
            self.daemon(done("t", Some(3)));
            self.drain(window);
        }
    }

    fn hello_snapshot(last: u64, text: &str) -> Delivery {
        Delivery::Snapshot {
            thread_id: t(),
            events: vec![stored(1, turn(1)), stored(2, delta("m1", text))],
            has_older: false,
            truncated: false,
            last: Some(EventId(last)),
        }
    }

    #[test]
    fn a_view_starts_with_a_snapshot_and_goes_on_with_live_events() {
        let mut rig = Rig::new(SMALL);
        let main = rig.open("main");
        assert_eq!(
            rig.drain(&main),
            [Delivery::Connection { state: connected() }]
        );

        rig.view(&main, "t");
        assert!(rig.state.is_stale(main.label, main.generation));
        assert_eq!(rig.sent(), [subscribe(2, "t", None)]);
        // Before the answer, an event can only be from an earlier
        // subscription.
        rig.daemon(event(9, turn(9)));
        rig.daemon(ok(2, json!({ "replay_until": 3 })));
        rig.daemon(event(1, turn(1)));
        rig.daemon(event(2, delta("m1", "Hel")));
        rig.daemon(event(3, delta("m1", "lo")));
        assert_eq!(rig.drain(&main), [], "nothing while the thread loads");
        rig.daemon(done("t", Some(3)));
        assert_eq!(rig.drain(&main), [hello_snapshot(3, "Hello")]);
        assert!(!rig.state.is_stale(main.label, main.generation));

        // Live events are the daemon's own JSON, untouched.
        let json = rig.daemon(event(4, delta("m1", " there")));
        let outgoing = rig.take(&main).expect("the event");
        assert_eq!(outgoing.json, json);
        assert_eq!(
            outgoing.delivery().expect("a delivery"),
            Delivery::Event {
                event: stored(4, delta("m1", " there"))
            }
        );
        // A repeat isn't delivered twice.
        rig.daemon(event(4, delta("m1", " there")));
        assert!(rig.take(&main).is_none());
        assert_eq!(rig.sent(), []);
    }

    #[test]
    fn two_windows_share_one_subscription() {
        let mut rig = Rig::new(SMALL);
        let main = rig.open("main");
        rig.viewing(&main, 2);

        let pop = rig.open("pop");
        rig.view(&pop, "t");
        assert_eq!(rig.sent(), [], "the thread is already loaded");
        assert_eq!(
            rig.drain(&pop),
            [
                Delivery::Connection { state: connected() },
                hello_snapshot(3, "Hello")
            ]
        );

        rig.daemon(event(4, completed("m1")));
        let live = [Delivery::Event {
            event: stored(4, completed("m1")),
        }];
        assert_eq!(rig.drain(&main), live);
        assert_eq!(rig.drain(&pop), live);

        rig.state.unview(main.label, main.generation, &t());
        assert_eq!(rig.sent(), [], "someone still views it");
        rig.daemon(event(5, turn(2)));
        assert_eq!(rig.drain(&main), []);
        assert_eq!(rig.drain(&pop).len(), 1);

        rig.state.close_window(pop.label, pop.generation);
        assert_eq!(rig.sent(), [ClientMessage::Unsubscribe { thread_id: t() }]);
        assert!(rig.state.cached(&t()).is_none());
        // What still arrives for it is ignored.
        rig.daemon(event(6, turn(3)));
    }

    #[test]
    fn a_window_that_stops_acknowledging_holds_nobody_back() {
        let mut rig = Rig::new(SMALL);
        let main = rig.open("main");
        rig.viewing(&main, 2);
        let hung = rig.open("hung");
        rig.view(&hung, "t");
        // It reads its snapshot, and from here on never acknowledges.
        assert_eq!(rig.take_all(&hung).len(), 2);

        let mut received = Vec::new();
        let mut text = String::from("Hello");
        for id in 4..104 {
            let word = format!(" word{id}");
            text.push_str(&word);
            rig.daemon(event(id, delta("m1", &word)));
            // The webview is stuck, but its bridge still takes deliveries.
            rig.take_all(&hung);
            received.extend(rig.drain(&main));
        }
        // The window that keeps up got every event, in order.
        let expected: Vec<Delivery> = (4..104)
            .map(|id| Delivery::Event {
                event: stored(id, delta("m1", &format!(" word{id}"))),
            })
            .collect();
        assert_eq!(received, expected);
        // A command goes out and is answered.
        let (reply, mut answer) = oneshot::channel();
        rig.state
            .command(DaemonCommand::ListThreads, reply)
            .expect("sent");
        assert_eq!(rig.sent().len(), 1);
        rig.daemon(ok(3, json!([])));
        assert_eq!(answer.try_recv().expect("answered"), Ok(json!([])));

        // The stuck one fell behind once, and is sent nothing more.
        assert_eq!(rig.state.resyncs(hung.label, hung.generation), 1);
        assert!(rig.state.is_stale(hung.label, hung.generation));
        assert!(rig.take(&hung).is_none());
        rig.daemon(event(104, completed("m1")));
        assert!(rig.take(&hung).is_none());

        // It wakes up and acknowledges what it was given: it gets the
        // present, not the events it missed.
        let seq = rig.state.windows["hung"].seq;
        rig.state.ack(hung.label, hung.generation, seq);
        assert_eq!(
            rig.drain(&hung),
            [
                Delivery::Connection { state: connected() },
                Delivery::Snapshot {
                    thread_id: t(),
                    events: vec![
                        stored(1, turn(1)),
                        stored(2, delta("m1", &text)),
                        stored(104, completed("m1")),
                    ],
                    has_older: false,
                    truncated: false,
                    last: Some(EventId(104)),
                },
            ]
        );
        // And is live again.
        rig.daemon(event(105, turn(2)));
        assert_eq!(rig.drain(&hung).len(), 1);
        assert_eq!(rig.state.resyncs(main.label, main.generation), 0);
    }

    #[test]
    fn a_window_whose_bridge_stops_taking_resyncs_when_it_takes_again() {
        let mut rig = Rig::new(SMALL);
        let idle = rig.open("idle");
        rig.viewing(&idle, 2);
        for id in 4..104 {
            rig.daemon(event(id, delta("m1", "0123456789")));
        }
        assert_eq!(rig.state.resyncs(idle.label, idle.generation), 1);
        // Nothing is in flight, so what it is owed fits at once.
        let deliveries = rig.drain(&idle);
        assert_eq!(deliveries.len(), 2);
        assert_eq!(deliveries[0], Delivery::Connection { state: connected() });
        let Delivery::Snapshot { events, last, .. } = &deliveries[1] else {
            panic!("{:?}", deliveries[1]);
        };
        assert_eq!(*last, Some(EventId(103)));
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn snapshots_that_do_not_fit_together_are_sent_one_at_a_time() {
        let mut rig = Rig::new(Limits {
            window_queue_bytes: 600,
            ..SMALL
        });
        let main = rig.open("main");
        rig.drain(&main);
        for (request, thread) in [(2, "a"), (3, "b")] {
            rig.view(&main, thread);
            rig.daemon(ok(request, json!({ "replay_until": 1 })));
            rig.daemon(event_of(thread, request, delta("m1", &"x".repeat(300))));
        }
        rig.daemon(done("a", Some(1)));
        rig.daemon(done("b", Some(1)));
        // The first fills the queue; the second waits for room.
        let first = rig.take_all(&main);
        assert_eq!(first.len(), 1);
        assert!(rig.state.is_stale(main.label, main.generation));
        assert_eq!(rig.state.resyncs(main.label, main.generation), 0);
        rig.state.ack(main.label, main.generation, first[0].seq);
        assert_eq!(rig.take_all(&main).len(), 1);
        assert!(!rig.state.is_stale(main.label, main.generation));
    }

    #[test]
    fn a_reconnect_continues_from_the_cursor() {
        let mut rig = Rig::new(SMALL);
        let main = rig.open("main");
        rig.viewing(&main, 2);

        let (reply, mut answer) = oneshot::channel();
        rig.state
            .command(DaemonCommand::ListThreads, reply)
            .expect("sent");
        rig.state
            .disconnected(ConnectionState::Connecting { attempt: 1 });
        // In flight when the connection dropped: nobody knows if it ran.
        assert_eq!(
            answer.try_recv().expect("answered"),
            Err(CommandError::UnknownOutcome)
        );
        let (reply, _) = oneshot::channel();
        assert_eq!(
            rig.state.command(DaemonCommand::ListThreads, reply),
            Err(CommandError::NotConnected)
        );
        assert_eq!(
            rig.drain(&main),
            [Delivery::Connection {
                state: ConnectionState::Connecting { attempt: 1 }
            }]
        );

        rig.reconnect();
        assert_eq!(
            rig.sent(),
            [
                ClientMessage::SubscribeInbox { request_id: 4 },
                subscribe(5, "t", Some(3)),
            ]
        );
        rig.daemon(ok(5, json!({ "replay_until": 5 })));
        rig.daemon(event(3, delta("m1", "lo")));
        rig.daemon(event(4, delta("m1", " again")));
        rig.daemon(event(5, completed("m1")));
        rig.daemon(done("t", Some(5)));
        // No snapshot: the window had everything up to 3, and gets the rest.
        assert_eq!(
            rig.drain(&main),
            [
                Delivery::Connection { state: connected() },
                Delivery::Event {
                    event: stored(4, delta("m1", " again"))
                },
                Delivery::Event {
                    event: stored(5, completed("m1"))
                },
            ]
        );
    }

    #[test]
    fn a_cursor_the_daemon_rejects_reloads_the_thread() {
        let mut rig = Rig::new(SMALL);
        let main = rig.open("main");
        rig.viewing(&main, 2);
        rig.state
            .disconnected(ConnectionState::Connecting { attempt: 1 });
        rig.reconnect();
        rig.drain(&main);
        assert_eq!(rig.sent().len(), 2);

        // The daemon's store ends before event 3.
        rig.daemon(err(4, ErrorCode::Invalid));
        assert_eq!(rig.sent(), [subscribe(5, "t", None)]);
        rig.daemon(ok(5, json!({ "replay_until": 2 })));
        rig.daemon(event(1, turn(1)));
        rig.daemon(event(2, delta("m1", "Other")));
        rig.daemon(done("t", Some(2)));
        assert_eq!(rig.drain(&main), [hello_snapshot(2, "Other")]);

        // A second refusal, with no cursor to blame, fails the thread.
        rig.daemon(ServerMessage::SubscriptionLagged {
            thread_id: t(),
            last_sent: None,
        });
        assert_eq!(rig.sent(), [subscribe(6, "t", Some(2))]);
        rig.daemon(err(6, ErrorCode::Invalid));
        assert_eq!(rig.sent(), [subscribe(7, "t", None)]);
        rig.daemon(err(7, ErrorCode::Internal));
        assert_eq!(
            rig.drain(&main),
            [Delivery::ThreadFailed {
                thread_id: t(),
                code: ErrorCode::Internal,
                message: "no".into(),
            }]
        );
    }

    #[test]
    fn a_lagged_subscription_picks_up_where_the_cache_ends() {
        let mut rig = Rig::new(SMALL);
        let main = rig.open("main");
        rig.viewing(&main, 2);
        rig.daemon(event(4, delta("m1", " a")));
        rig.daemon(ServerMessage::SubscriptionLagged {
            thread_id: t(),
            last_sent: Some(EventId(4)),
        });
        assert_eq!(rig.sent(), [subscribe(3, "t", Some(4))]);
        // Until the answer, nothing for the thread is applied.
        rig.daemon(event(7, delta("m1", " d")));
        rig.daemon(ok(3, json!({ "replay_until": 6 })));
        for (id, text) in [(5, " b"), (6, " c"), (7, " d")] {
            rig.daemon(event(id, delta("m1", text)));
        }
        rig.daemon(done("t", Some(6)));
        let got: Vec<u64> = rig
            .drain(&main)
            .into_iter()
            .map(|delivery| match delivery {
                Delivery::Event { event } => event.id.0,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(got, [4, 5, 6, 7]);
    }

    #[test]
    fn an_unknown_thread_fails_and_can_load_after_a_reconnect() {
        let mut rig = Rig::new(SMALL);
        let main = rig.open("main");
        rig.drain(&main);
        rig.view(&main, "t");
        rig.daemon(err(2, ErrorCode::UnknownThread));
        assert_eq!(rig.sent(), [subscribe(2, "t", None)], "not tried again");
        assert_eq!(
            rig.drain(&main),
            [Delivery::ThreadFailed {
                thread_id: t(),
                code: ErrorCode::UnknownThread,
                message: "no".into(),
            }]
        );
        // Another window asking is told the same, without asking the daemon.
        let pop = rig.open("pop");
        rig.view(&pop, "t");
        assert_eq!(rig.drain(&pop).len(), 2);
        assert_eq!(rig.sent(), []);

        rig.state
            .disconnected(ConnectionState::Connecting { attempt: 1 });
        rig.reconnect();
        assert_eq!(rig.sent()[1], subscribe(4, "t", None));
        rig.daemon(ok(4, json!({ "replay_until": null })));
        rig.daemon(done("t", None));
        let empty = Delivery::Snapshot {
            thread_id: t(),
            events: Vec::new(),
            has_older: false,
            truncated: false,
            last: None,
        };
        assert_eq!(rig.drain(&main).last(), Some(&empty));
        assert_eq!(rig.drain(&pop).last(), Some(&empty));
    }

    #[test]
    fn the_inbox_goes_to_the_windows_that_show_it() {
        let mut rig = Rig::new(SMALL);
        let main = rig.open("main");
        let pop = rig.open("pop");
        rig.drain(&main);
        rig.drain(&pop);
        rig.state.show_inbox(main.label, main.generation, true);
        assert_eq!(rig.drain(&main), [], "not loaded yet");

        let item = InboxItem {
            thread_id: Some(ThreadId("a".into())),
            reason: InboxReason::Approval,
            since_ms: 5,
            title: "A".into(),
            detail: Some("rm -rf build".into()),
        };
        rig.daemon(ok(
            1,
            json!({ "threads": [summary("a", "A"), summary("b", "B")], "inbox": [item] }),
        ));
        assert_eq!(
            rig.drain(&main),
            [Delivery::Inbox {
                threads: vec![summary("a", "A"), summary("b", "B")],
                inbox: vec![item.clone()],
            }]
        );

        // Updates are the daemon's own JSON.
        let json = rig.daemon(ServerMessage::ThreadUpdated {
            summary: summary("b", "B2"),
        });
        assert_eq!(rig.take(&main).expect("the update").json, json);
        let json = rig.daemon(ServerMessage::InboxUpdated { items: Vec::new() });
        assert_eq!(rig.take(&main).expect("the update").json, json);
        assert_eq!(rig.drain(&pop), [], "it doesn't show the inbox");

        // Archiving a thread from here takes it off every list.
        let (reply, mut answer) = oneshot::channel();
        rig.state
            .command(
                DaemonCommand::ArchiveThread {
                    thread_id: ThreadId("a".into()),
                },
                reply,
            )
            .expect("sent");
        rig.daemon(ok(2, Value::Null));
        assert_eq!(answer.try_recv().expect("answered"), Ok(Value::Null));
        assert_eq!(
            rig.drain(&main),
            [Delivery::ThreadRemoved {
                thread_id: ThreadId("a".into())
            }]
        );

        // A window that starts showing the inbox gets what is known now.
        rig.state.show_inbox(pop.label, pop.generation, true);
        assert_eq!(
            rig.drain(&pop),
            [Delivery::Inbox {
                threads: vec![summary("b", "B2")],
                inbox: Vec::new(),
            }]
        );

        // After a reconnect, everything again, as the daemon has it now.
        rig.state
            .disconnected(ConnectionState::Connecting { attempt: 1 });
        rig.reconnect();
        rig.daemon(ok(
            3,
            json!({ "threads": [summary("c", "C")], "inbox": [] }),
        ));
        let fresh = Delivery::Inbox {
            threads: vec![summary("c", "C")],
            inbox: Vec::new(),
        };
        assert_eq!(rig.drain(&main).last(), Some(&fresh));
        assert_eq!(rig.drain(&pop).last(), Some(&fresh));
    }

    #[test]
    fn a_reopened_window_starts_over_and_the_old_one_goes_dead() {
        let mut rig = Rig::new(SMALL);
        let old = rig.open("main");
        rig.viewing(&old, 2);
        rig.daemon(event(4, turn(2)));

        let new = rig.open("main");
        assert_eq!(
            rig.sent(),
            [ClientMessage::Unsubscribe { thread_id: t() }],
            "the old window's view went with it"
        );
        assert!(matches!(
            rig.state.take(old.label, old.generation),
            Take::Closed
        ));
        rig.state.ack(old.label, old.generation, 99);
        rig.state.view(old.label, old.generation, t());
        assert_eq!(rig.sent(), []);
        assert!(
            rig.state
                .page_before(old.label, old.generation, &t(), EventId(9), 100)
                .is_none()
        );
        // Closing through the old handle doesn't close the new window.
        rig.state.close_window(old.label, old.generation);
        assert_eq!(
            rig.drain(&new),
            [Delivery::Connection { state: connected() }]
        );
    }

    #[test]
    fn older_history_is_paged_in_whole_segments() {
        let mut rig = Rig::new(SMALL);
        let main = rig.open("main");
        rig.viewing(&main, 2);
        rig.daemon(event(4, completed("m1")));
        rig.daemon(event(5, delta("m2", "Bye")));
        let page = rig
            .state
            .page_before(main.label, main.generation, &t(), EventId(5), 1)
            .expect("viewed");
        assert_eq!(
            page,
            Page {
                events: vec![stored(2, delta("m1", "Hello")), stored(4, completed("m1"))],
                has_older: true,
                truncated: false,
            }
        );
        let pop = rig.open("pop");
        assert!(
            rig.state
                .page_before(pop.label, pop.generation, &t(), EventId(5), 1)
                .is_none(),
            "it doesn't view the thread"
        );
    }

    #[test]
    fn what_the_daemon_must_not_send_is_a_violation() {
        let violations = [
            ok(9, Value::Null),
            ServerMessage::Welcome {
                protocol_version: 1,
                daemon_version: "1.0".into(),
                daemon_pid: 7,
                role: atrium_protocol::wire::ClientRole::App,
            },
            // The inbox subscription refused, or answered with nonsense.
            err(1, ErrorCode::Forbidden),
            ok(1, json!({ "threads": 3 })),
        ];
        for message in violations {
            let mut rig = Rig::new(SMALL);
            let json: Arc<str> = serde_json::to_string(&message).expect("json").into();
            assert!(rig.state.on_message(message, json).is_err());
        }
        // A subscribe answer without its replay bound.
        let mut rig = Rig::new(SMALL);
        let main = rig.open("main");
        rig.view(&main, "t");
        let message = ok(2, json!({}));
        let json: Arc<str> = serde_json::to_string(&message).expect("json").into();
        assert!(rig.state.on_message(message, json).is_err());

        // A replay that ends somewhere else than announced, or before its
        // events came.
        for (replayed, last) in [(2, Some(1)), (2, None), (1, Some(2))] {
            let mut rig = Rig::new(SMALL);
            let main = rig.open("main");
            rig.view(&main, "t");
            rig.daemon(ok(2, json!({ "replay_until": 2 })));
            for id in 1..=replayed {
                rig.daemon(event(id, turn(id as u32)));
            }
            let message = done("t", last);
            let json: Arc<str> = serde_json::to_string(&message).expect("json").into();
            assert!(
                rig.state.on_message(message, json).is_err(),
                "{replayed} events, then done at {last:?}"
            );
        }

        // Not violations: answers and events that arrive late.
        let mut rig = Rig::new(SMALL);
        let main = rig.open("main");
        rig.view(&main, "t");
        rig.state.unview(main.label, main.generation, &t());
        rig.daemon(ok(2, json!({ "replay_until": 1 })));
        rig.daemon(event(1, turn(1)));
        rig.daemon(done("t", Some(1)));
        rig.daemon(ServerMessage::SubscriptionLagged {
            thread_id: t(),
            last_sent: None,
        });
        // They leave no trace: nothing is kept of the thread, nothing is
        // sent to the daemon, and the window only has the connection state.
        assert!(rig.state.cached(&t()).is_none());
        assert_eq!(
            rig.sent(),
            [
                ClientMessage::Subscribe {
                    request_id: 2,
                    thread_id: t(),
                    after: None
                },
                ClientMessage::Unsubscribe { thread_id: t() }
            ]
        );
        assert!(
            rig.drain(&main)
                .iter()
                .all(|delivery| matches!(delivery, Delivery::Connection { .. }))
        );
        // The same `ReplayDone` while live (a second one) changes nothing.
        let mut rig = Rig::new(SMALL);
        let main = rig.open("main");
        rig.view(&main, "t");
        rig.daemon(ok(2, json!({ "replay_until": null })));
        rig.daemon(done("t", None));
        rig.daemon(done("t", Some(5)));
        rig.daemon(event(1, turn(1)));
        assert!(
            rig.drain(&main)
                .iter()
                .any(|delivery| matches!(delivery, Delivery::Event { .. }))
        );
    }

    #[test]
    fn a_command_is_answered_once_and_refused_when_too_large() {
        let mut rig = Rig::new(SMALL);
        let (reply, mut answer) = oneshot::channel();
        let too_large = DaemonCommand::RenameThread {
            thread_id: t(),
            title: "x".repeat(limits::MAX_CLIENT_FRAME_LEN as usize),
        };
        assert!(matches!(
            rig.state.command(too_large, reply),
            Err(CommandError::TooLarge { .. })
        ));
        assert_eq!(rig.sent(), []);
        assert!(answer.try_recv().is_err());

        let (reply, mut answer) = oneshot::channel();
        rig.state
            .command(DaemonCommand::ListInbox, reply)
            .expect("sent");
        assert_eq!(
            rig.sent(),
            [ClientMessage::Command {
                request_id: 2,
                command: DaemonCommand::ListInbox
            }]
        );
        rig.daemon(err(2, ErrorCode::StorageReadOnly));
        assert_eq!(
            answer.try_recv().expect("answered"),
            Err(CommandError::Rejected {
                code: ErrorCode::StorageReadOnly,
                message: "no".into()
            })
        );
        // A caller that stopped waiting doesn't trouble the answer.
        let (reply, answer) = oneshot::channel();
        rig.state
            .command(DaemonCommand::ListInbox, reply)
            .expect("sent");
        drop(answer);
        rig.daemon(ok(3, json!([])));
    }

    #[test]
    fn closing_tells_the_windows_and_ends_them() {
        let mut rig = Rig::new(SMALL);
        let main = rig.open("main");
        rig.drain(&main);
        rig.state.close();
        assert_eq!(
            rig.take(&main)
                .expect("the state")
                .delivery()
                .expect("a delivery"),
            Delivery::Connection {
                state: ConnectionState::Closed
            }
        );
        assert!(matches!(
            rig.state.take(main.label, main.generation),
            Take::Closed
        ));
    }
}
