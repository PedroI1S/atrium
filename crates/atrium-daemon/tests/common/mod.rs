//! What the socket tests share: a daemon in a temporary home, a scripted
//! adapter the test drives event by event, and a plain client that speaks
//! the wire.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use atrium_agents::{
    AdapterError, AgentAdapter, BoxFuture, GateDecision, Session, SessionConfig, ToolCheck,
    Transport,
};
use atrium_daemon::{Daemon, Notification, Notifier, Options};
use atrium_host::lifecycle::Paths;
use atrium_host::supervise::{self, SupervisedChild};
use atrium_protocol::PROTOCOL_VERSION;
use atrium_protocol::agent::{
    AgentEvent, AgentEventEnvelope, MessageBody, MessageKind, Provider, SessionCommand, ToolKind,
    TurnOutcome,
};
use atrium_protocol::policy::{EnforcementDescriptor, Preset, ToolSubject};
use atrium_protocol::thread::{ThreadId, ThreadSummary};
use atrium_protocol::wire::{
    AgentLaunch, ClientMessage, ClientRole, CommandResult, DaemonCommand, ErrorCode, EventId,
    ServerMessage, StoredEvent,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

/// How long a test waits for something that should happen at once.
pub const SOON: Duration = Duration::from_secs(20);

pub async fn soon<T>(what: impl Future<Output = T>) -> T {
    tokio::time::timeout(SOON, what)
        .await
        .expect("it should have happened by now")
}

/// Polls until `check` holds.
pub async fn until(mut check: impl FnMut() -> bool) {
    soon(async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
}

pub fn paths(root: &Path) -> Paths {
    let (runtime_dir, data_dir) = (root.join("run"), root.join("data"));
    Paths {
        socket: runtime_dir.join("daemon.sock"),
        lock: runtime_dir.join("daemon.lock"),
        store: data_dir.join("atrium.db"),
        snapshots_dir: data_dir.join("snapshots"),
        logs_dir: data_dir.join("logs"),
        app_identity: data_dir.join("app-identity.json"),
        runtime_dir,
        data_dir,
    }
}

/// A short folder, so the socket's path fits `sun_path`.
pub fn folder() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("atriumd")
        .tempdir_in("/tmp")
        .expect("tempdir")
}

// ---------------------------------------------------------------------------
// The scripted adapter
// ---------------------------------------------------------------------------

/// What the scripted adapter does when a session starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// Hands the session to the test.
    Script,
    /// Fails to start.
    Fail,
    /// Never finishes starting.
    Hang,
    /// Spawns a real process (`sleep`) first, reported through `on_spawn`.
    Process,
}

pub struct Scripted {
    pub start: Mutex<Start>,
    scripts: mpsc::UnboundedSender<Script>,
}

impl Scripted {
    pub fn new() -> (Arc<Self>, mpsc::UnboundedReceiver<Script>) {
        let (scripts, started) = mpsc::unbounded_channel();
        (
            Arc::new(Self {
                start: Mutex::new(Start::Script),
                scripts,
            }),
            started,
        )
    }
}

fn descriptor() -> EnforcementDescriptor {
    EnforcementDescriptor::evaluate(
        Provider::Claude,
        Some("0.0.0-scripted".into()),
        false,
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )
}

impl AgentAdapter for Scripted {
    fn provider(&self) -> Provider {
        Provider::Claude
    }

    fn tested_versions(&self) -> &'static [&'static str] {
        &[]
    }

    fn probe<'a>(&'a self, _: &'a SessionConfig) -> BoxFuture<'a, EnforcementDescriptor> {
        Box::pin(async { descriptor() })
    }

    fn start(
        &self,
        config: SessionConfig,
        _: Transport,
    ) -> BoxFuture<'_, Result<Session, AdapterError>> {
        let start = *self.start.lock().expect("lock");
        Box::pin(async move {
            let mut process = None;
            match start {
                Start::Script => {}
                Start::Fail => return Err(AdapterError::Handshake("no such model".into())),
                Start::Hang => std::future::pending::<()>().await,
                Start::Process => {
                    let mut command = tokio::process::Command::new("sleep");
                    command.arg("600");
                    let child = supervise::spawn_group(command)?;
                    if let Some(on_spawn) = &config.on_spawn {
                        on_spawn(&child.identity);
                    }
                    process = Some(child);
                }
            }
            let (events, receiver) = mpsc::channel(atrium_agents::EVENT_BUFFER);
            let (commands, inbox) = mpsc::channel(atrium_agents::COMMAND_BUFFER);
            let identity = process.as_ref().map(|child| child.identity.clone());
            let _ = self.scripts.send(Script {
                config,
                events: Some(events),
                commands: inbox,
                seq: 0,
                process,
            });
            Ok(Session {
                descriptor: descriptor(),
                process: identity,
                events: receiver,
                commands,
            })
        })
    }
}

/// One scripted session, in the test's hands.
pub struct Script {
    pub config: SessionConfig,
    events: Option<mpsc::Sender<AgentEventEnvelope>>,
    pub commands: mpsc::Receiver<SessionCommand>,
    seq: u64,
    pub process: Option<SupervisedChild>,
}

pub fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl Script {
    pub async fn emit(&mut self, event: AgentEvent) {
        let envelope = AgentEventEnvelope {
            incarnation: 0,
            seq: self.seq,
            at_ms: 1,
            event,
        };
        self.seq += 1;
        soon(self.events.as_ref().expect("still open").send(envelope))
            .await
            .expect("the daemon is reading");
    }

    pub async fn started(&mut self, native_session_id: &str) {
        self.emit(AgentEvent::SessionStarted {
            provider: Provider::Claude,
            native_session_id: native_session_id.into(),
            model: None,
            tools: Vec::new(),
            enforcement: descriptor(),
        })
        .await;
    }

    pub async fn turn(&mut self, turn: u32) {
        self.emit(AgentEvent::TurnStarted { turn }).await;
    }

    pub async fn delta(&mut self, message_id: &str, text: &str) {
        self.emit(AgentEvent::MessageDelta {
            message_id: message_id.into(),
            kind: MessageKind::Assistant,
            text: text.into(),
        })
        .await;
    }

    /// Closes a message that was streamed as `deltas`.
    pub async fn completed(&mut self, message_id: &str, deltas: &[&str]) {
        let text = deltas.concat();
        self.emit(AgentEvent::MessageCompleted {
            message_id: message_id.into(),
            kind: MessageKind::Assistant,
            body: MessageBody::Streamed {
                chars: text.chars().count() as u64,
                deltas: deltas.len() as u64,
                sha256: sha256_hex(&text),
            },
        })
        .await;
    }

    pub async fn turn_done(&mut self, turn: u32) {
        self.emit(AgentEvent::TurnCompleted {
            turn,
            outcome: TurnOutcome::Completed,
            usage: None,
        })
        .await;
    }

    pub async fn ended(&mut self, exit_code: Option<i32>) {
        self.emit(AgentEvent::SessionEnded {
            exit_code,
            reason: None,
        })
        .await;
        self.events = None;
    }

    /// The event stream ends with no closing events, as when a provider
    /// process dies.
    pub fn die(&mut self) {
        self.events = None;
    }

    /// A command that writes, which Home's policy asks about.
    pub fn write_check(turn: u32, file: &str) -> ToolCheck {
        let line = format!("touch {file}");
        ToolCheck {
            call_id: None,
            name: "Bash".into(),
            kind: ToolKind::Execute,
            title: Some(line.clone()),
            input: json!({ "command": line }),
            subject: ToolSubject::Command { line, cwd: None },
            turn,
        }
    }

    /// Asks the session's real gate about a write and, as an adapter
    /// would, emits `ApprovalRequested` with the ticket. Returns the
    /// request id.
    pub async fn ask(&mut self, turn: u32, file: &str) -> String {
        self.ask_ticket(turn, file).await.0.request_id
    }

    /// [`Script::ask`], returning the ticket and the check it is bound to.
    pub async fn ask_ticket(
        &mut self,
        turn: u32,
        file: &str,
    ) -> (atrium_agents::ApprovalTicket, ToolCheck) {
        let check = Self::write_check(turn, file);
        let GateDecision::Ask { ticket } = self.config.gate.check(&check).await else {
            panic!("Home asks about a write");
        };
        self.emit(AgentEvent::ApprovalRequested {
            request_id: ticket.request_id.clone(),
            call_id: None,
            name: check.name.clone(),
            kind: check.kind,
            title: check.title.clone(),
            input: check.input.clone(),
            reason: None,
            rememberable: ticket.rememberable,
        })
        .await;
        (ticket, check)
    }

    /// The next command the session was sent.
    pub async fn command(&mut self) -> SessionCommand {
        soon(self.commands.recv()).await.expect("a command")
    }

    /// Nothing reaches the session for a little while.
    pub async fn no_command(&mut self) {
        let waited = tokio::time::timeout(Duration::from_millis(150), self.commands.recv()).await;
        assert!(waited.is_err(), "the session was sent {waited:?}");
    }
}

// ---------------------------------------------------------------------------
// The daemon under test
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct Notes(pub Mutex<Vec<Notification>>);

impl Notifier for Notes {
    fn notify(&self, notification: &Notification) {
        self.0.lock().expect("lock").push(notification.clone());
    }
}

pub struct Rig {
    pub folder: tempfile::TempDir,
    pub paths: Paths,
    pub home: PathBuf,
    pub daemon: Option<Daemon>,
    pub adapter: Arc<Scripted>,
    pub scripts: mpsc::UnboundedReceiver<Script>,
    pub notes: Arc<Notes>,
    /// What the store sees as free space on its volume.
    pub free_bytes: Arc<AtomicU64>,
}

impl Rig {
    pub async fn start() -> Self {
        Self::start_with(|_| {}).await
    }

    pub async fn start_with(change: impl FnOnce(&mut Options)) -> Self {
        let mut rig = Self::in_folder(folder());
        rig.boot(change).await;
        rig
    }

    /// Everything but the daemon, for a home that already exists.
    pub fn in_folder(folder: tempfile::TempDir) -> Self {
        let paths = paths(folder.path());
        let home = folder.path().join("home");
        std::fs::create_dir_all(&home).expect("home");
        let (adapter, scripts) = Scripted::new();
        Self {
            folder,
            paths,
            home,
            daemon: None,
            adapter,
            scripts,
            notes: Arc::default(),
            free_bytes: Arc::new(AtomicU64::new(u64::MAX)),
        }
    }

    pub fn options(&self) -> Options {
        let mut options = Options::new(self.paths.clone(), self.home.clone());
        options.dev_socket = true;
        options.adapters = vec![self.adapter.clone()];
        options.notifier = self.notes.clone();
        let free = self.free_bytes.clone();
        options.store.free_space = Arc::new(move || free.load(Ordering::Relaxed));
        options
    }

    /// Starts (or starts again) the daemon in this home.
    pub async fn boot(&mut self, change: impl FnOnce(&mut Options)) {
        let mut options = self.options();
        change(&mut options);
        self.daemon = Some(Daemon::start(options).await.expect("the daemon starts"));
    }

    pub fn socket(&self) -> &Path {
        &self.paths.socket
    }

    pub async fn dev(&self) -> Wire {
        Wire::dev(self.socket()).await
    }

    /// The next session the daemon started on the scripted adapter.
    pub async fn script(&mut self) -> Script {
        soon(self.scripts.recv()).await.expect("a session")
    }

    /// Stops the daemon and waits until it is gone.
    pub async fn stop(&mut self, force: bool) {
        if let Some(daemon) = self.daemon.take() {
            daemon.stop(force);
            soon(daemon.stopped()).await;
        }
    }

    pub fn notes(&self) -> Vec<Notification> {
        self.notes.0.lock().expect("lock").clone()
    }
}

pub fn native() -> AgentLaunch {
    AgentLaunch::Native {
        provider: Provider::Claude,
        model: None,
        effort: None,
        preset: Preset::Home,
    }
}

// ---------------------------------------------------------------------------
// A plain wire client
// ---------------------------------------------------------------------------

pub struct Wire {
    stream: UnixStream,
    next_request: u64,
    /// Messages read while waiting for a response.
    pub aside: VecDeque<ServerMessage>,
}

pub type Answer = Result<Value, (ErrorCode, String)>;

impl Wire {
    pub async fn connect(socket: &Path) -> Self {
        Self {
            stream: UnixStream::connect(socket).await.expect("connects"),
            next_request: 1,
            aside: VecDeque::new(),
        }
    }

    /// Connects and says `Hello`; returns the daemon's first message.
    pub async fn hello(socket: &Path, role: ClientRole) -> (Self, Option<ServerMessage>) {
        let mut wire = Self::connect(socket).await;
        wire.send(&ClientMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: "test".into(),
            role,
        })
        .await;
        let first = wire.read().await;
        (wire, first)
    }

    pub async fn dev(socket: &Path) -> Self {
        let (wire, first) = Self::hello(socket, ClientRole::Dev).await;
        assert!(
            matches!(first, Some(ServerMessage::Welcome { .. })),
            "{first:?}"
        );
        wire
    }

    pub async fn send_raw(&mut self, kind: u8, payload: &[u8]) {
        let mut frame = ((payload.len() + 1) as u32).to_be_bytes().to_vec();
        frame.push(kind);
        frame.extend_from_slice(payload);
        // A daemon that already closed is fine: the read says so.
        let _ = self.stream.write_all(&frame).await;
    }

    pub async fn send(&mut self, message: &ClientMessage) {
        let payload = serde_json::to_vec(message).expect("serializes");
        self.send_raw(0, &payload).await;
    }

    /// The next message off the socket; `None` when the daemon closed it.
    pub async fn read(&mut self) -> Option<ServerMessage> {
        soon(async {
            let len = self.stream.read_u32().await.ok()?;
            let kind = self.stream.read_u8().await.ok()?;
            assert_eq!(kind, 0, "M1 has no stream frames");
            let mut payload = vec![0; len as usize - 1];
            self.stream.read_exact(&mut payload).await.ok()?;
            Some(serde_json::from_slice(&payload).expect("a server message"))
        })
        .await
    }

    /// The next message, those set aside first.
    pub async fn next(&mut self) -> ServerMessage {
        match self.aside.pop_front() {
            Some(message) => message,
            None => self.read().await.expect("the connection is open"),
        }
    }

    /// Whether the daemon closed the connection (anything it sent first is
    /// dropped).
    pub async fn closed(&mut self) -> bool {
        soon(async {
            let mut sink = [0u8; 4096];
            loop {
                match self.stream.read(&mut sink).await {
                    Ok(0) | Err(_) => return true,
                    Ok(_) => {}
                }
            }
        })
        .await
    }

    /// Reads until the response to `request_id`, setting the rest aside.
    pub async fn response(&mut self, request_id: u64) -> Answer {
        loop {
            match self.read().await.expect("the connection is open") {
                ServerMessage::Response {
                    request_id: id,
                    result,
                } if id == request_id => {
                    return match result {
                        CommandResult::Ok { value } => Ok(value),
                        CommandResult::Err { code, message } => Err((code, message)),
                    };
                }
                other => self.aside.push_back(other),
            }
        }
    }

    pub fn request_id(&mut self) -> u64 {
        self.next_request += 1;
        self.next_request
    }

    pub async fn command(&mut self, command: DaemonCommand) -> Answer {
        let request_id = self.request_id();
        self.send(&ClientMessage::Command {
            request_id,
            command,
        })
        .await;
        self.response(request_id).await
    }

    pub async fn ok(&mut self, command: DaemonCommand) -> Value {
        let shown = format!("{command:?}");
        self.command(command)
            .await
            .unwrap_or_else(|error| panic!("{shown} failed: {error:?}"))
    }

    pub async fn refused(&mut self, command: DaemonCommand) -> ErrorCode {
        let shown = format!("{command:?}");
        match self.command(command).await {
            Ok(value) => panic!("{shown} was accepted: {value}"),
            Err((code, _)) => code,
        }
    }

    pub async fn subscribe(&mut self, thread: &ThreadId, after: Option<EventId>) -> Answer {
        let request_id = self.request_id();
        self.send(&ClientMessage::Subscribe {
            request_id,
            thread_id: thread.clone(),
            after,
        })
        .await;
        self.response(request_id).await
    }

    /// Subscribes and returns `replay_until`.
    pub async fn subscribed(
        &mut self,
        thread: &ThreadId,
        after: Option<EventId>,
    ) -> Option<EventId> {
        let value = self.subscribe(thread, after).await.expect("subscribes");
        serde_json::from_value(value["replay_until"].clone()).expect("replay_until")
    }

    pub async fn subscribe_inbox(&mut self) -> Value {
        let request_id = self.request_id();
        self.send(&ClientMessage::SubscribeInbox { request_id })
            .await;
        self.response(request_id).await.expect("the inbox")
    }

    pub async fn start_thread(
        &mut self,
        agent: AgentLaunch,
        message: Option<&str>,
    ) -> ThreadSummary {
        let value = self
            .ok(DaemonCommand::StartThread {
                agent,
                message: message.map(str::to_owned),
            })
            .await;
        serde_json::from_value(value).expect("a summary")
    }

    pub async fn thread(&mut self, thread: &ThreadId) -> ThreadSummary {
        let value = self
            .ok(DaemonCommand::GetThread {
                thread_id: thread.clone(),
            })
            .await;
        serde_json::from_value(value).expect("a summary")
    }

    /// Polls `GetThread` until the summary satisfies `check`.
    pub async fn thread_when(
        &mut self,
        thread: &ThreadId,
        check: impl Fn(&ThreadSummary) -> bool,
    ) -> ThreadSummary {
        soon(async {
            loop {
                let summary = self.thread(thread).await;
                if check(&summary) {
                    return summary;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
    }

    /// The thread's whole history, by a fresh replay.
    pub async fn history(&mut self, thread: &ThreadId) -> Vec<StoredEvent> {
        let replay_until = self.subscribed(thread, None).await;
        let mut events = Vec::new();
        loop {
            match self.read().await.expect("the connection is open") {
                ServerMessage::Event { event } if event.thread_id == *thread => events.push(event),
                ServerMessage::ReplayDone { thread_id, last } if thread_id == *thread => {
                    assert_eq!(last, replay_until);
                    break;
                }
                other => self.aside.push_back(other),
            }
        }
        self.send(&ClientMessage::Unsubscribe {
            thread_id: thread.clone(),
        })
        .await;
        events
    }
}

/// The events' payloads, for comparing against what was scripted.
pub fn kinds(events: &[StoredEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|stored| match &stored.envelope.event {
            AgentEvent::SessionStarted { .. } => "session_started",
            AgentEvent::EnforcementChanged { .. } => "enforcement_changed",
            AgentEvent::TurnStarted { .. } => "turn_started",
            AgentEvent::MessageDelta { .. } => "message_delta",
            AgentEvent::MessageCompleted { .. } => "message_completed",
            AgentEvent::ToolCallStarted { .. } => "tool_call_started",
            AgentEvent::ToolCallUpdated { .. } => "tool_call_updated",
            AgentEvent::ToolCallFinished { .. } => "tool_call_finished",
            AgentEvent::ApprovalRequested { .. } => "approval_requested",
            AgentEvent::QuestionAsked { .. } => "question_asked",
            AgentEvent::RequestResolved { .. } => "request_resolved",
            AgentEvent::PlanUpdated { .. } => "plan_updated",
            AgentEvent::Usage { .. } => "usage",
            AgentEvent::ContextUpdated { .. } => "context_updated",
            AgentEvent::QuotaUpdated { .. } => "quota_updated",
            AgentEvent::RateLimited { .. } => "rate_limited",
            AgentEvent::TurnCompleted { .. } => "turn_completed",
            AgentEvent::Error { .. } => "error",
            AgentEvent::Unmapped { .. } => "unmapped",
            AgentEvent::SessionEnded { .. } => "session_ended",
        })
        .collect()
}
