//! Agent sessions: starting an incarnation, pumping its events into the
//! store, and closing what it left open (contract §5, §6.1, §10).
//!
//! One task per session. It owns the adapter's event receiver, so events of
//! a thread are stored in the order the adapter produced them, a batch per
//! transaction. Whatever happens to the adapter (it fails to start, its
//! stream ends without `SessionEnded`, the store refuses a batch), the task
//! ends the same way: the incarnation's closing events are in the store,
//! built from the store's projections of what is still open, never from the
//! history.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use atrium_agents::{
    AgentAdapter, PermissionPolicy, SHUTDOWN_GRACE, Session, SessionConfig, Transport,
};
use atrium_core::policy::{SessionGate, SessionGateConfig, StoreRules};
use atrium_core::{ApprovalLedger, RequestKey, Signal, StoreError, StoreResult};
use atrium_host::supervise::{self, ProcessIdentity};
use atrium_protocol::agent::{
    AgentEvent, AgentEventEnvelope, MessageBody, MessageKind, Provider, RequestOutcome,
    SessionCommand, ToolStatus, TurnOutcome,
};
use atrium_protocol::policy::Preset;
use atrium_protocol::thread::{Cleanup, ThreadId, ThreadSummary};
use atrium_protocol::wire::{ErrorCode, EventId, StoredEvent};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use crate::{Failure, Inner, failure, fake, now_ms, store_failure};

/// Events stored per transaction, at most.
const BATCH_EVENTS: usize = 256;

/// How long a session that was told to stop has to deliver its closing
/// events (the adapter's own cleanup is bounded by `SHUTDOWN_GRACE`). After
/// that the daemon lets go of it and closes the incarnation itself.
const STOP_PATIENCE: Duration = Duration::from_secs(SHUTDOWN_GRACE.as_secs() * 3);

/// How long a command may wait for room in a session's command queue.
const COMMAND_PATIENCE: Duration = Duration::from_secs(5);

/// A thread's running session, as the rest of the daemon sees it.
pub(crate) struct Live {
    incarnation: AtomicU32,
    /// `None` until the adapter has started.
    commands: Mutex<Option<mpsc::Sender<SessionCommand>>>,
    turn: AtomicBool,
    stop: watch::Sender<bool>,
}

impl Live {
    fn new() -> Self {
        Self {
            incarnation: AtomicU32::new(0),
            commands: Mutex::new(None),
            turn: AtomicBool::new(false),
            stop: watch::channel(false).0,
        }
    }

    pub(crate) fn incarnation(&self) -> u32 {
        self.incarnation.load(Ordering::Relaxed)
    }

    pub(crate) fn turn_running(&self) -> bool {
        self.turn.load(Ordering::Relaxed)
    }

    fn sender(&self) -> Option<mpsc::Sender<SessionCommand>> {
        self.commands
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_sender(&self, sender: Option<mpsc::Sender<SessionCommand>>) {
        *self.commands.lock().unwrap_or_else(PoisonError::into_inner) = sender;
    }

    /// Tells the session to end. Works at any point of its life, the start
    /// included.
    pub(crate) fn stop(&self) {
        self.stop.send_replace(true);
        if let Some(sender) = self.sender() {
            // A full queue is fine: the pump sees the flag and, if the
            // session doesn't end, lets go of it.
            let _ = sender.try_send(SessionCommand::Shutdown);
        }
    }

    /// Passes a command to the session.
    pub(crate) async fn send(&self, command: SessionCommand) -> Result<(), Failure> {
        let not_running = || failure(ErrorCode::NotRunning, "the session isn't running");
        let sender = self.sender().ok_or_else(not_running)?;
        match tokio::time::timeout(COMMAND_PATIENCE, sender.send(command)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(not_running()),
            Err(_) => Err(failure(
                ErrorCode::Internal,
                "the session isn't taking commands",
            )),
        }
    }
}

/// One incarnation's provider process, as the daemon knows it.
#[derive(Default)]
struct Process {
    /// Set as soon as the adapter reports the process (`on_spawn`).
    identity: Mutex<Option<ProcessIdentity>>,
    /// Orders the store's record of it: `true` once the incarnation is
    /// over, after which a report that comes late is not recorded.
    over: tokio::sync::Mutex<bool>,
}

impl Process {
    fn identity(&self) -> Option<ProcessIdentity> {
        self.identity
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn seen(&self, identity: &ProcessIdentity) {
        *self.identity.lock().unwrap_or_else(PoisonError::into_inner) = Some(identity.clone());
    }
}

pub(crate) enum Agent {
    Native(Arc<dyn AgentAdapter>),
    Fake(fake::Params),
}

/// How to start an incarnation.
pub(crate) struct Launch {
    pub(crate) agent: Agent,
    pub(crate) provider: Provider,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
    pub(crate) preset: Preset,
    /// The native session id to resume.
    pub(crate) resume: Option<String>,
    /// Sent as the first user message once the session has started.
    pub(crate) message: Option<String>,
}

/// How a session's event stream ended.
#[derive(Default)]
struct Pumped {
    /// `SessionEnded` was stored, with this exit code.
    ended: Option<Option<i32>>,
    /// The sequence number after the last one the adapter used.
    next_seq: u64,
    /// A fatal error to record when the daemon closes the incarnation.
    error: Option<String>,
    /// Why it ended, for `SessionEnded.reason`.
    reason: Option<String>,
    /// The thread has a newer incarnation: nothing of this one is closed.
    stale: bool,
}

fn is_closing(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::MessageCompleted { .. }
            | AgentEvent::ToolCallFinished { .. }
            | AgentEvent::RequestResolved { .. }
            | AgentEvent::TurnCompleted { .. }
            | AgentEvent::Error { .. }
            | AgentEvent::SessionEnded { .. }
    )
}

/// Whether a batch can change what a thread's summary shows. Streaming text
/// and tool output alone don't, so a long answer isn't a stream of updates.
fn shows_in_summary(events: &[StoredEvent], unread: bool) -> bool {
    events.iter().any(|stored| match &stored.envelope.event {
        // The first assistant text of a reply sets `unread`.
        AgentEvent::MessageDelta { kind, .. } => !unread && *kind != MessageKind::User,
        AgentEvent::ToolCallUpdated { .. }
        | AgentEvent::Usage { .. }
        | AgentEvent::ContextUpdated { .. }
        | AgentEvent::QuotaUpdated { .. }
        | AgentEvent::PlanUpdated { .. }
        | AgentEvent::Unmapped { .. } => false,
        _ => true,
    })
}

impl Inner {
    /// Begins a new incarnation of `thread` and starts its session in the
    /// background. Returns the thread as it is while starting.
    pub(crate) async fn launch(
        self: &Arc<Self>,
        thread: &ThreadId,
        launch: Launch,
    ) -> Result<ThreadSummary, Failure> {
        let live = Arc::new(Live::new());
        {
            let mut sessions = self.sessions();
            if sessions.contains_key(thread) {
                return Err(failure(
                    ErrorCode::Invalid,
                    "the thread already has a session",
                ));
            }
            sessions.insert(thread.clone(), live.clone());
        }
        let begun = async {
            let incarnation = self.store.begin_incarnation(thread).await?;
            let summary = self.store.signal(thread, Signal::Starting).await?;
            StoreResult::Ok((incarnation, summary))
        }
        .await;
        let (incarnation, summary) = match begun {
            Ok(begun) => begun,
            Err(error) => {
                self.sessions().remove(thread);
                return Err(store_failure(error));
            }
        };
        live.incarnation.store(incarnation, Ordering::Relaxed);
        self.refresh_thread(thread).await;
        tokio::spawn(run(self.clone(), thread.clone(), incarnation, live, launch));
        Ok(summary)
    }

    fn session_config(
        self: &Arc<Self>,
        thread: &ThreadId,
        incarnation: u32,
        launch: &Launch,
        process: &Arc<Process>,
    ) -> SessionConfig {
        let gate = SessionGate::new(
            SessionGateConfig {
                thread: thread.clone(),
                incarnation,
                cwd: self.home.clone(),
                home: self.home.clone(),
                atrium_data_paths: self.data_paths.clone(),
                atrium_protected_paths: self.protected_paths.clone(),
                preset: launch.preset,
                provider: launch.provider,
            },
            self.policy.clone(),
            self.ledger.clone(),
            Arc::new(StoreRules(self.store.clone())),
        );
        // The process is recorded as soon as it exists, before the
        // handshake, so a daemon that dies meanwhile finds it at its next
        // start. The callback can't wait, so a task does the write.
        let (spawned, mut identities) = mpsc::unbounded_channel::<ProcessIdentity>();
        let (store, recorded, slot) = (self.store.clone(), thread.clone(), process.clone());
        tokio::spawn(async move {
            while let Some(identity) = identities.recv().await {
                let over = slot.over.lock().await;
                if !*over {
                    let _ = store
                        .set_process(&recorded, incarnation, Some(identity))
                        .await;
                }
            }
        });
        let slot = process.clone();
        SessionConfig {
            cwd: self.home.clone(),
            model: launch.model.clone(),
            effort: launch.effort.clone(),
            permissions: match launch.preset {
                Preset::Home => PermissionPolicy::ReadOnly,
                Preset::FullAccess => PermissionPolicy::FullAccess,
                Preset::Careful | Preset::Normal | Preset::Trusted => PermissionPolicy::Ask,
            },
            gate: Arc::new(gate),
            // MCP servers are the daemon's to add, never the client's; M1
            // has none.
            mcp_servers: Vec::new(),
            resume: launch.resume.clone(),
            env: Vec::new(),
            program: None,
            on_spawn: Some(Arc::new(move |identity: &ProcessIdentity| {
                slot.seen(identity);
                let _ = spawned.send(identity.clone());
            })),
        }
    }

    /// Stores one batch and publishes it, under the publish lock, then lets
    /// the ledger and the thread list follow what was stored.
    pub(crate) async fn store_batch(
        &self,
        thread: &ThreadId,
        incarnation: u32,
        batch: Vec<AgentEventEnvelope>,
    ) -> StoreResult<Vec<StoredEvent>> {
        let stored = {
            let _publishing = self.publish.lock().await;
            let stored = self.store.append(thread, incarnation, batch).await?;
            self.hub().publish(thread, &stored);
            stored
        };
        // The ledger's expiry is driven by what was stored (contract §5).
        for event in &stored {
            match &event.envelope.event {
                AgentEvent::RequestResolved {
                    request_id,
                    outcome: RequestOutcome::Cancelled | RequestOutcome::Expired,
                } => {
                    let key = RequestKey {
                        thread: thread.clone(),
                        incarnation,
                        request_id: request_id.clone(),
                    };
                    // Already final (answered, denied, redeemed) is fine.
                    let _ = self.ledger.withdraw(&key).await;
                }
                AgentEvent::TurnCompleted { turn, .. } => {
                    self.ledger.expire_turn(thread, incarnation, *turn).await;
                }
                AgentEvent::SessionEnded { .. } => {
                    self.ledger.expire_incarnation(thread, incarnation).await;
                }
                _ => {}
            }
        }
        let unread = self.hub().thread(thread).is_some_and(|shown| shown.unread);
        if shows_in_summary(&stored, unread) {
            self.refresh_thread(thread).await;
        }
        Ok(stored)
    }

    /// The thread's last stored event.
    async fn last_stored(&self, thread: &ThreadId) -> Option<StoredEvent> {
        let last = self.store.last_event(thread).await.ok()??;
        let before = last.0.checked_sub(1).map(EventId);
        self.store
            .events_after(thread, before, 1)
            .await
            .ok()?
            .into_iter()
            .next()
    }

    /// Stores the closing events of an incarnation that didn't close itself,
    /// in the contract's order: an error (if any), open messages, open tool
    /// calls, pending requests, the open turn, `SessionEnded`. Does nothing
    /// (and returns `false`) if the incarnation's `SessionEnded` is already
    /// stored.
    pub(crate) async fn close_incarnation(
        &self,
        thread: &ThreadId,
        incarnation: u32,
        next_seq: u64,
        error: Option<String>,
        reason: Option<String>,
    ) -> bool {
        let mut seq = next_seq;
        if let Some(last) = self.last_stored(thread).await
            && last.envelope.incarnation == incarnation
        {
            if matches!(last.envelope.event, AgentEvent::SessionEnded { .. }) {
                return false;
            }
            seq = seq.max(last.envelope.seq.saturating_add(1));
        }
        let open = match self.store.open_items(thread, incarnation).await {
            Ok(open) => open,
            Err(error) => {
                eprintln!("atriumd: can't read what {thread:?} left open: {error}");
                return false;
            }
        };
        let failed = error.is_some();
        let mut events = Vec::new();
        if let Some(message) = error {
            events.push(AgentEvent::Error {
                message,
                fatal: true,
                details: Vec::new(),
            });
        }
        for message in open.messages {
            events.push(AgentEvent::MessageCompleted {
                message_id: message.message_id,
                kind: message.kind,
                body: MessageBody::Streamed {
                    chars: message.chars,
                    deltas: message.deltas,
                    sha256: message.sha256,
                },
            });
        }
        for call_id in open.tool_calls {
            events.push(AgentEvent::ToolCallFinished {
                call_id,
                status: ToolStatus::Cancelled,
                output: None,
                error: None,
                truncated_bytes: 0,
            });
        }
        for request_id in open.requests {
            events.push(AgentEvent::RequestResolved {
                request_id,
                outcome: RequestOutcome::Cancelled,
            });
        }
        if let Some(turn) = open.turn {
            events.push(AgentEvent::TurnCompleted {
                turn,
                outcome: if failed {
                    TurnOutcome::Failed
                } else {
                    TurnOutcome::Interrupted
                },
                usage: None,
            });
        }
        events.push(AgentEvent::SessionEnded {
            exit_code: None,
            reason,
        });
        let at_ms = now_ms();
        let batch = events
            .into_iter()
            .zip(seq..)
            .map(|(event, seq)| AgentEventEnvelope {
                incarnation: 0,
                seq,
                at_ms,
                event,
            })
            .collect();
        // Closing events are accepted while the store is read-only.
        match self.store_batch(thread, incarnation, batch).await {
            Ok(_) => true,
            Err(error) => {
                eprintln!("atriumd: can't store the closing events of {thread:?}: {error}");
                false
            }
        }
    }

    /// Reads a session's events into the store until its stream ends.
    async fn pump(
        &self,
        thread: &ThreadId,
        incarnation: u32,
        live: &Live,
        mut events: mpsc::Receiver<AgentEventEnvelope>,
    ) -> Pumped {
        let mut out = Pumped::default();
        let mut stop = live.stop.subscribe();
        let mut batch = Vec::new();
        // The store refused a batch: from then on only closing events are
        // kept, and the daemon closes whatever is still open at the end.
        let mut closing_only = false;
        let mut deadline: Option<Instant> = None;
        loop {
            let received = tokio::select! {
                received = events.recv_many(&mut batch, BATCH_EVENTS) => received,
                _ = stop.wait_for(|stop| *stop), if deadline.is_none() => {
                    deadline = Some(Instant::now() + STOP_PATIENCE);
                    continue;
                }
                () = async { tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)).await },
                    if deadline.is_some() =>
                {
                    out.reason.get_or_insert_with(|| "the session didn't stop in time".into());
                    break;
                }
            };
            if received == 0 {
                break;
            }
            let mut ended = None;
            for (at, envelope) in batch.iter().enumerate() {
                out.next_seq = out.next_seq.max(envelope.seq.saturating_add(1));
                match &envelope.event {
                    AgentEvent::TurnStarted { .. } => live.turn.store(true, Ordering::Relaxed),
                    AgentEvent::TurnCompleted { .. } => live.turn.store(false, Ordering::Relaxed),
                    AgentEvent::SessionEnded { exit_code, .. } => {
                        live.turn.store(false, Ordering::Relaxed);
                        ended = Some((at, *exit_code));
                        break;
                    }
                    _ => {}
                }
            }
            // `SessionEnded` is always the last event.
            if let Some((at, _)) = ended {
                batch.truncate(at + 1);
            }
            if closing_only {
                batch.retain(|envelope| is_closing(&envelope.event));
            }
            if !batch.is_empty() {
                match self
                    .store_batch(thread, incarnation, std::mem::take(&mut batch))
                    .await
                {
                    Ok(_) => {
                        if let Some((_, exit_code)) = ended {
                            out.ended = Some(exit_code);
                        }
                    }
                    Err(StoreError::StaleIncarnation { .. }) => {
                        out.stale = true;
                        break;
                    }
                    // Not an error of the session's: the thread is
                    // interrupted, and resumable when space returns. Its
                    // closing events are still accepted.
                    Err(StoreError::ReadOnly { .. }) => {
                        closing_only = true;
                        out.reason.get_or_insert_with(|| "storage is full".into());
                        if self
                            .store
                            .signal(thread, Signal::StorageReadOnly)
                            .await
                            .is_ok()
                        {
                            self.refresh_thread(thread).await;
                        }
                        live.stop();
                    }
                    // The batch is lost (an event over `MAX_EVENT_BYTES`, a
                    // store that fails): what follows would be a history
                    // with a hole in it. The session is let go of here, and
                    // the daemon closes the incarnation with the error.
                    Err(other) => {
                        out.error = Some(format!("an event couldn't be stored: {other}"));
                        break;
                    }
                }
            }
            self.quieter.notify_waiters();
            if ended.is_some() {
                break;
            }
        }
        out
    }
}

/// One session, from the adapter's start to the incarnation's end.
async fn run(
    inner: Arc<Inner>,
    thread: ThreadId,
    incarnation: u32,
    live: Arc<Live>,
    mut launch: Launch,
) {
    enum Started {
        Session(Session),
        Stopped,
        Failed(String),
    }
    let mut stop = live.stop.subscribe();
    let process = Arc::new(Process::default());
    let started = match &launch.agent {
        Agent::Fake(params) => Started::Session(fake::start(*params, launch.message.take())),
        Agent::Native(adapter) => {
            let config = inner.session_config(&thread, incarnation, &launch, &process);
            tokio::select! {
                started = adapter.start(config, Transport::Local) => match started {
                    Ok(session) => Started::Session(session),
                    Err(error) => Started::Failed(error.to_string()),
                },
                // Dropping the start drops the adapter's child with it.
                _ = stop.wait_for(|stop| *stop) => Started::Stopped,
            }
        }
    };
    let pumped = match started {
        Started::Session(session) => {
            let Session {
                descriptor,
                process: reported,
                events,
                commands,
            } = session;
            let _ = inner
                .store
                .set_enforcement(&thread, incarnation, descriptor)
                .await;
            if let Some(reported) = reported {
                process.seen(&reported);
                let _ = inner
                    .store
                    .set_process(&thread, incarnation, Some(reported))
                    .await;
            }
            live.set_sender(Some(commands.clone()));
            if *stop.borrow() {
                // Asked to stop while it was starting.
                let _ = commands.try_send(SessionCommand::Shutdown);
            } else if let Some(text) = launch.message.take() {
                let _ = commands
                    .send(SessionCommand::SendUserMessage { text })
                    .await;
            }
            drop(commands);
            inner.pump(&thread, incarnation, &live, events).await
        }
        Started::Stopped => Pumped {
            reason: Some("stopped before the session started".into()),
            ..Pumped::default()
        },
        Started::Failed(error) => Pumped {
            error: Some(error),
            ..Pumped::default()
        },
    };
    live.set_sender(None);
    live.turn.store(false, Ordering::Relaxed);
    if !pumped.stale {
        let ending = match pumped.ended {
            Some(code) => Signal::Exited { code },
            // The adapter didn't end its session (it failed to start, its
            // stream stopped, it didn't stop in time), so nothing says its
            // process is gone: the daemon checks, kills what is left of
            // its group, and says so when it can't tell.
            None => {
                let cleanup = match process.identity() {
                    Some(identity) => {
                        tokio::task::spawn_blocking(move || supervise::kill_recorded(&identity))
                            .await
                            .unwrap_or(Cleanup::Unverified)
                    }
                    None => Cleanup::Verified,
                };
                inner
                    .close_incarnation(
                        &thread,
                        incarnation,
                        pumped.next_seq,
                        pumped.error,
                        pumped.reason,
                    )
                    .await;
                match cleanup {
                    Cleanup::Verified => Signal::Exited { code: None },
                    Cleanup::Unverified => Signal::Recovered { cleanup },
                }
            }
        };
        {
            let mut over = process.over.lock().await;
            *over = true;
            let _ = inner.store.set_process(&thread, incarnation, None).await;
        }
        if inner.store.signal(&thread, ending).await.is_ok() {
            inner.refresh_thread(&thread).await;
        }
    }
    {
        let mut sessions = inner.sessions();
        if sessions
            .get(&thread)
            .is_some_and(|current| Arc::ptr_eq(current, &live))
        {
            sessions.remove(&thread);
        }
    }
    inner.quieter.notify_waiters();
}

/// Stops every session and waits for them to end (the daemon is stopping).
pub(crate) async fn stop_all(inner: &Arc<Inner>) {
    let deadline = Instant::now() + STOP_PATIENCE + Duration::from_secs(2);
    loop {
        let quieter = inner.quieter.notified();
        tokio::pin!(quieter);
        quieter.as_mut().enable();
        let live: Vec<Arc<Live>> = inner.sessions().values().cloned().collect();
        if live.is_empty() {
            return;
        }
        for session in live {
            session.stop();
        }
        if tokio::time::timeout_at(deadline, quieter).await.is_err() {
            eprintln!("atriumd: sessions were still ending at exit");
            return;
        }
    }
}
