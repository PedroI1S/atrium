//! The actor owns all transport operations, including reconnect/resubscribe.
//! Cancelling a receive or a request waiter cannot cancel that work. Commands
//! accepted by the actor may execute even if their caller drops its waiter.
//!
//! M0-07 should split Client into a cloneable ClientHandle and EventStream and
//! continuously drain the stream in a separate task. ReplayDone repeats after
//! every reconnect. The UI MUST dedupe `id <= last_ui_applied`, then acknowledge
//! with applied only after applying state (including across webview IPC).
use crate::{
    PROTOCOL_VERSION,
    codec::{self, Frame},
    daemon,
    wire::*,
};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::BufReader,
    net::{UnixStream, unix::OwnedWriteHalf},
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch},
};

pub const REQUEST_CAPACITY: usize = 16;
pub const EVENT_CAPACITY: usize = 32;
/// A single legal oversized frame may occupy the whole budget.
pub const EVENT_BYTES: usize = 2 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
fn protocol(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
fn closed() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "client actor stopped")
}
fn retryable(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::NotFound
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::UnexpectedEof
            | io::ErrorKind::TimedOut
            | io::ErrorKind::Interrupted
            | io::ErrorKind::WouldBlock
    )
}
#[derive(Debug)]
pub struct UnknownOutcome(pub io::Error);
impl std::fmt::Display for UnknownOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "command outcome unknown: {}", self.0)
    }
}
impl std::error::Error for UnknownOutcome {}
#[derive(Debug)]
pub enum CommandError {
    NotSent(io::Error),
    UnknownOutcome(UnknownOutcome),
}
impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSent(e) => write!(f, "command not sent: {e}"),
            Self::UnknownOutcome(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for CommandError {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Connected,
    Reconnecting { attempt: u32 },
    Failed(String),
    Closed,
}
#[derive(Clone, Default)]
struct Cursor {
    applied: Option<EventId>,
    generation: u64,
}
type Cursors = Arc<Mutex<BTreeMap<ThreadId, Cursor>>>;
struct Delivery {
    message: ServerMessage,
    generation: u64,
    weight: u32,
    _bytes: Option<OwnedSemaphorePermit>,
}
struct Transport {
    writer: OwnedWriteHalf,
    incoming: mpsc::Receiver<io::Result<ServerMessage>>,
    reader: tokio::task::JoinHandle<()>,
}
impl Drop for Transport {
    fn drop(&mut self) {
        self.reader.abort();
    }
}
impl Transport {
    async fn open(
        path: &Path,
        name: &str,
        #[cfg(test)] checkpoints: Option<&Checkpoints>,
    ) -> io::Result<Self> {
        daemon::verify_socket(path)?;
        #[cfg(test)]
        checkpoint(checkpoints, "connect").await;
        let stream = UnixStream::connect(path).await?;
        let (r, mut writer) = stream.into_split();
        let mut r = BufReader::new(r);
        #[cfg(test)]
        checkpoint(checkpoints, "hello_write").await;
        codec::write(
            &mut writer,
            &Frame::Control(ClientMessage::Hello {
                protocol_version: PROTOCOL_VERSION,
                client_name: name.into(),
            }),
        )
        .await?;
        #[cfg(test)]
        checkpoint(checkpoints, "welcome_read").await;
        match codec::read::<_, ServerMessage>(&mut r).await? {
            Frame::Control(ServerMessage::Welcome {
                protocol_version: PROTOCOL_VERSION,
                ..
            }) => (),
            other => return Err(protocol(format!("invalid daemon handshake: {other:?}"))),
        }
        let (tx, incoming) = mpsc::channel(1);
        let reader = tokio::spawn(async move {
            loop {
                let msg = match codec::read::<_, ServerMessage>(&mut r).await {
                    Ok(Frame::Control(m)) => Ok(m),
                    Ok(_) => Err(protocol("unexpected stream frame")),
                    Err(e) => Err(e),
                };
                let failed = msg.is_err();
                if tx.send(msg).await.is_err() || failed {
                    break;
                }
            }
        });
        Ok(Self {
            writer,
            incoming,
            reader,
        })
    }
    async fn send(&mut self, msg: ClientMessage) -> io::Result<()> {
        self.send_bytes(&codec::encode(&Frame::Control(msg))?).await
    }
    async fn send_bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
        use tokio::io::AsyncWriteExt;
        tokio::time::timeout(IO_TIMEOUT, self.writer.write_all(bytes))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "socket write timed out"))?
    }
    async fn read(&mut self) -> io::Result<ServerMessage> {
        self.incoming.recv().await.ok_or_else(closed)?
    }
}
type Reply<T> = oneshot::Sender<T>;
enum Request {
    Command(DaemonCommand, Reply<Result<CommandResult, CommandError>>),
    Subscribe(ThreadId, Option<EventId>, Reply<io::Result<CommandResult>>),
    Reconnect(Reply<io::Result<()>>),
}
// If the actor exits unexpectedly after starting a write, retain uncertainty.
struct CommandReply(Option<Reply<Result<CommandResult, CommandError>>>);
impl CommandReply {
    fn send(mut self, result: Result<CommandResult, CommandError>) {
        if let Some(reply) = self.0.take() {
            let _ = reply.send(result);
        }
    }
}
impl Drop for CommandReply {
    fn drop(&mut self) {
        if let Some(reply) = self.0.take() {
            let _ = reply.send(Err(CommandError::UnknownOutcome(UnknownOutcome(closed()))));
        }
    }
}
enum Pending {
    Command(u64, CommandReply),
    Subscribe(
        u64,
        ThreadId,
        Option<EventId>,
        Reply<io::Result<CommandResult>>,
    ),
}
impl Pending {
    fn not_sent(self, e: io::Error) {
        match self {
            Self::Command(_, reply) => reply.send(Err(CommandError::NotSent(e))),
            Self::Subscribe(_, _, _, reply) => {
                let _ = reply.send(Err(e));
            }
        }
    }
    fn fail(self, e: io::Error) {
        match self {
            Self::Command(_, reply) => {
                reply.send(Err(CommandError::UnknownOutcome(UnknownOutcome(e))));
            }
            Self::Subscribe(_, _, _, reply) => {
                let _ = reply.send(Err(e));
            }
        }
    }
}
/// Cloneable bounded request handle. Drain EventStream concurrently with requests;
/// a slow consumer intentionally backpressures socket reads and command replies.
#[derive(Clone)]
pub struct ClientHandle {
    requests: mpsc::Sender<Request>,
    cursors: Cursors,
    state: watch::Receiver<ConnectionState>,
}
impl ClientHandle {
    pub async fn connect(
        path: impl AsRef<Path>,
        name: impl Into<String>,
    ) -> io::Result<(Self, EventStream)> {
        Ok(Client::connect(path, name).await?.into_parts())
    }
    pub fn connection_state(&self) -> watch::Receiver<ConnectionState> {
        self.state.clone()
    }
    pub async fn command(&self, command: DaemonCommand) -> Result<CommandResult, CommandError> {
        let (tx, rx) = oneshot::channel();
        self.requests
            .send(Request::Command(command, tx))
            .await
            .map_err(|_| CommandError::NotSent(closed()))?;
        rx.await.map_err(|_| CommandError::NotSent(closed()))?
    }
    pub async fn subscribe(
        &self,
        thread: ThreadId,
        after: Option<EventId>,
    ) -> io::Result<CommandResult> {
        let (tx, rx) = oneshot::channel();
        self.requests
            .send(Request::Subscribe(thread, after, tx))
            .await
            .map_err(|_| closed())?;
        rx.await.map_err(|_| closed())?
    }
    pub async fn reconnect(&self) -> io::Result<()> {
        let (tx, rx) = oneshot::channel();
        self.requests
            .send(Request::Reconnect(tx))
            .await
            .map_err(|_| closed())?;
        rx.await.map_err(|_| closed())?
    }
    pub fn applied(&self, thread: &ThreadId, id: EventId) {
        if let Some(c) = self.cursors.lock().unwrap().get_mut(thread)
            && c.applied.is_none_or(|old| id > old)
        {
            c.applied = Some(id);
        }
    }
}
/// One bounded stream. Dropping it stops the actor, even if handles remain.
/// next() only receives from a channel: it is cancellation-safe at every await,
/// including while the actor handshakes or restores multiple subscriptions.
pub struct EventStream {
    incoming: mpsc::Receiver<Delivery>,
    cursors: Cursors,
    state: watch::Receiver<ConnectionState>,
}
impl EventStream {
    pub async fn next(&mut self) -> io::Result<ServerMessage> {
        loop {
            let delivery =
                self.incoming
                    .recv()
                    .await
                    .ok_or_else(|| match &*self.state.borrow() {
                        ConnectionState::Failed(message) => protocol(message.clone()),
                        _ => closed(),
                    })?;
            let cursors = self.cursors.lock().unwrap();
            match &delivery.message {
                ServerMessage::Event { event } => {
                    if cursors.get(&event.thread_id).is_none_or(|c| {
                        c.generation != delivery.generation
                            || c.applied.is_some_and(|id| event.id <= id)
                    }) {
                        continue;
                    }
                }
                ServerMessage::ReplayDone { thread_id, .. }
                    if cursors
                        .get(thread_id)
                        .is_none_or(|c| c.generation != delivery.generation) =>
                {
                    continue;
                }
                _ => (),
            }
            return Ok(delivery.message);
        }
    }
}
/// Convenience facade for serial CLI callers. Use into_parts() for UI use.
pub struct Client {
    handle: ClientHandle,
    events: EventStream,
}
impl Client {
    pub async fn connect(path: impl AsRef<Path>, name: impl Into<String>) -> io::Result<Self> {
        Self::connect_inner(
            path.as_ref().to_owned(),
            name.into(),
            #[cfg(test)]
            None,
        )
        .await
    }
    async fn connect_inner(
        path: PathBuf,
        name: String,
        #[cfg(test)] checkpoints: Option<Checkpoints>,
    ) -> io::Result<Self> {
        let transport = tokio::time::timeout(
            IO_TIMEOUT,
            Transport::open(
                &path,
                &name,
                #[cfg(test)]
                None,
            ),
        )
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "handshake timed out"))??;
        let (requests, receiver) = mpsc::channel(REQUEST_CAPACITY);
        let (output, incoming) = mpsc::channel(EVENT_CAPACITY);
        let (status, state) = watch::channel(ConnectionState::Connected);
        let cursors = Cursors::default();
        let handle = ClientHandle {
            requests,
            cursors: cursors.clone(),
            state: state.clone(),
        };
        let events = EventStream {
            incoming,
            cursors: cursors.clone(),
            state,
        };
        let actor = Actor {
            path,
            name,
            transport: Some(transport),
            requests: receiver,
            output,
            cursors,
            status,
            next_id: 0,
            budget: Arc::new(Semaphore::new(EVENT_BYTES)),
            #[cfg(test)]
            checkpoints,
        };
        tokio::spawn(actor.run());
        Ok(Self { handle, events })
    }
    /// Connect to an existing daemon or start the supplied executable detached.
    /// Startup errors go to `dir/daemon.log` (0600). Security/protocol errors never
    /// trigger a spawn. Multiple concurrent starters are resolved by the lock.
    pub async fn connect_or_spawn(
        dir: impl AsRef<Path>,
        daemon_exe: impl AsRef<Path>,
        name: impl Into<String>,
    ) -> io::Result<Self> {
        use std::{
            fs::{self, OpenOptions},
            os::unix::{
                fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
                process::CommandExt,
            },
            process::Stdio,
        };
        use tokio::io::AsyncBufReadExt;
        let dir = dir.as_ref();
        let name = name.into();
        let socket = dir.join("daemon.sock");
        match Self::connect(&socket, name.clone()).await {
            Ok(c) => return Ok(c),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) => {}
            Err(e) => return Err(e),
        }
        match fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e),
        }
        daemon::verify_dir_metadata(&fs::symlink_metadata(dir)?, nix::unistd::geteuid().as_raw())?;
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW)
            .open(dir.join("daemon.log"))?;
        log.set_permissions(fs::Permissions::from_mode(0o600))?;
        let mut command = tokio::process::Command::new(daemon_exe.as_ref());
        command
            .arg("--dir")
            .arg(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(log);
        command.as_std_mut().process_group(0);
        let mut child = command.spawn()?;
        let stdout = child.stdout.take().ok_or_else(closed)?;
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
        let mut ready = String::new();
        tokio::time::timeout(IO_TIMEOUT, BufReader::new(stdout).read_line(&mut ready))
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "daemon readiness timed out; see daemon.log",
                )
            })??;
        // If another starter won, its socket can be used once it publishes.
        let start = tokio::time::Instant::now();
        loop {
            match Self::connect(&socket, name.clone()).await {
                Ok(c) => return Ok(c),
                Err(e) if retryable(&e) && start.elapsed() < IO_TIMEOUT => {
                    tokio::time::sleep(Duration::from_millis(25)).await
                }
                Err(e) => {
                    return Err(io::Error::new(
                        e.kind(),
                        format!("daemon startup failed: {e}; see daemon.log"),
                    ));
                }
            }
        }
    }
    pub fn into_parts(self) -> (ClientHandle, EventStream) {
        (self.handle, self.events)
    }
    pub fn handle(&self) -> ClientHandle {
        self.handle.clone()
    }
    pub async fn hello(&mut self) -> io::Result<()> {
        if *self.handle.state.borrow() == ConnectionState::Connected {
            Ok(())
        } else {
            self.reconnect().await
        }
    }
    pub async fn subscribe(
        &mut self,
        thread: ThreadId,
        after: Option<EventId>,
    ) -> io::Result<CommandResult> {
        self.handle.subscribe(thread, after).await
    }
    pub async fn command(&mut self, command: DaemonCommand) -> Result<CommandResult, CommandError> {
        self.handle.command(command).await
    }
    pub async fn reconnect(&mut self) -> io::Result<()> {
        self.handle.reconnect().await
    }
    pub fn applied(&mut self, thread: &ThreadId, id: EventId) {
        self.handle.applied(thread, id);
    }
    pub async fn next(&mut self) -> io::Result<ServerMessage> {
        self.events.next().await
    }
}
struct Actor {
    path: PathBuf,
    name: String,
    transport: Option<Transport>,
    requests: mpsc::Receiver<Request>,
    output: mpsc::Sender<Delivery>,
    cursors: Cursors,
    status: watch::Sender<ConnectionState>,
    next_id: u64,
    budget: Arc<Semaphore>,
    #[cfg(test)]
    checkpoints: Option<Checkpoints>,
}
impl Actor {
    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }
    fn delivery(&self, message: ServerMessage) -> io::Result<Delivery> {
        let thread = match &message {
            ServerMessage::Event { event } => &event.thread_id,
            ServerMessage::ReplayDone { thread_id, .. } => thread_id,
            _ => return Err(protocol("unexpected server message")),
        };
        let generation = self
            .cursors
            .lock()
            .unwrap()
            .get(thread)
            .ok_or_else(|| protocol("event for unsubscribed thread"))?
            .generation;
        let weight = serde_json::to_vec(&message)
            .map_err(crate::error)?
            .len()
            .clamp(1, EVENT_BYTES) as u32;
        Ok(Delivery {
            message,
            generation,
            weight,
            _bytes: None,
        })
    }
    async fn restore_once(&mut self) -> io::Result<()> {
        self.transport = None;
        self.transport = Some(
            tokio::time::timeout(
                IO_TIMEOUT,
                Transport::open(
                    &self.path,
                    &self.name,
                    #[cfg(test)]
                    self.checkpoints.as_ref(),
                ),
            )
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "handshake timed out"))??,
        );
        let cursors = self.cursors.lock().unwrap().clone();
        for (thread, cursor) in cursors {
            let id = self.id();
            #[cfg(test)]
            checkpoint(self.checkpoints.as_ref(), "subscribe_write").await;
            self.transport
                .as_mut()
                .unwrap()
                .send(ClientMessage::Subscribe {
                    request_id: id,
                    thread_id: thread,
                    after: cursor.applied,
                })
                .await?;
            loop {
                #[cfg(test)]
                checkpoint(self.checkpoints.as_ref(), "subscribe_read").await;
                let message =
                    tokio::time::timeout(IO_TIMEOUT, self.transport.as_mut().unwrap().read())
                        .await
                        .map_err(|_| {
                            io::Error::new(io::ErrorKind::TimedOut, "subscribe timed out")
                        })??;
                match message {
                    ServerMessage::Response { request_id, result } if request_id == id => {
                        match result {
                            CommandResult::Ok { .. } => break,
                            CommandResult::Err { message } => return Err(protocol(message)),
                        }
                    }
                    message => {
                        #[cfg(test)]
                        checkpoint(self.checkpoints.as_ref(), "output_budget").await;
                        let mut delivery = self.delivery(message)?;
                        delivery._bytes = Some(
                            self.budget
                                .clone()
                                .acquire_many_owned(delivery.weight)
                                .await
                                .map_err(crate::error)?,
                        );
                        #[cfg(test)]
                        checkpoint(self.checkpoints.as_ref(), "output_channel").await;
                        self.output.send(delivery).await.map_err(|_| closed())?;
                    }
                }
            }
        }
        Ok(())
    }
    async fn restore(&mut self) -> io::Result<()> {
        let mut attempt = 0u32;
        loop {
            attempt = attempt.saturating_add(1);
            self.status
                .send_replace(ConnectionState::Reconnecting { attempt });
            // Invalidate all buffered deliveries from previous connections.
            for c in self.cursors.lock().unwrap().values_mut() {
                c.generation += 1;
            }
            let output = self.output.clone();
            let result = tokio::select! { _ = output.closed() => return Err(closed()), result = self.restore_once() => result };
            match result {
                Ok(()) => {
                    self.status.send_replace(ConnectionState::Connected);
                    return Ok(());
                }
                Err(e) if retryable(&e) => {
                    #[cfg(test)]
                    checkpoint(self.checkpoints.as_ref(), "backoff").await;
                    let delay = Duration::from_millis((25u64 << attempt.min(6)).min(1000));
                    tokio::select! { _ = self.output.closed() => return Err(closed()), _ = tokio::time::sleep(delay) => () }
                }
                Err(e) => return Err(e),
            }
        }
    }
    async fn run(mut self) {
        let result = self.drive().await;
        self.status.send_replace(match result {
            Ok(()) => ConnectionState::Closed,
            Err(e) => ConnectionState::Failed(e.to_string()),
        });
    }
    async fn drive(&mut self) -> io::Result<()> {
        let mut pending: Option<Pending> = None;
        let mut output: Option<Delivery> = None;
        let outbound = self.output.clone();
        let budget = self.budget.clone();
        loop {
            let transport = self.transport.as_mut().unwrap();
            tokio::select! {
                biased;
                _ = self.output.closed() => return Ok(()),
                permits = async {
                    let bytes = budget.clone().acquire_many_owned(output.as_ref().unwrap().weight).await.map_err(crate::error)?;
                    let slot = outbound.reserve().await.map_err(|_| closed())?;
                    Ok::<_,io::Error>((bytes,slot))
                }, if output.is_some() => {
                    let (bytes,slot) = permits?;
                    let mut delivery = output.take().unwrap(); delivery._bytes = Some(bytes); slot.send(delivery);
                }
                request = self.requests.recv(), if pending.is_none() => {
                    let Some(request) = request else { return Ok(()); };
                    if let Request::Reconnect(reply) = request {
                        output = None;
                        let r = self.restore().await;
                        let fatal = r.as_ref().err().map(|e| protocol(e.to_string()));
                        let _ = reply.send(r);
                        if let Some(e) = fatal { return Err(e); }
                        continue;
                    }
                    let id = self.id();
                    let message = match request {
                        Request::Command(command,reply) => { pending = Some(Pending::Command(id,CommandReply(Some(reply)))); ClientMessage::Command {request_id:id,command} }
                        Request::Subscribe(thread,after,reply) => { pending = Some(Pending::Subscribe(id,thread.clone(),after,reply)); ClientMessage::Subscribe {request_id:id,thread_id:thread,after} }
                        Request::Reconnect(_) => unreachable!()
                    };
                    let bytes=match codec::encode(&Frame::Control(message)) {
                        Ok(bytes)=>bytes,
                        Err(e)=>{pending.take().unwrap().not_sent(e);continue;}
                    };
                    // Once this write begins, a command can have an unknown outcome.
                    if let Err(e) = self.transport.as_mut().unwrap().send_bytes(&bytes).await {
                        pending.take().unwrap().fail(e);
                        output = None;
                        self.restore().await?;
                    }
                }
                message = transport.read(), if output.is_none() => {
                    match message {
                        Ok(ServerMessage::Response {request_id,result}) => {
                            match pending.take() {
                                Some(Pending::Command(id,reply)) if id==request_id => { reply.send(Ok(result)); }
                                Some(Pending::Subscribe(id,thread,after,reply)) if id==request_id => {
                                    if matches!(result,CommandResult::Ok {..}) {
                                        let mut cursors = self.cursors.lock().unwrap();
                                        let c = cursors.entry(thread).or_default(); c.generation += 1; c.applied = after;
                                    }
                                    let _ = reply.send(Ok(result));
                                }
                                other => { if let Some(p) = other { p.fail(protocol("response ID mismatch")); } return Err(protocol("unexpected response")); }
                            }
                        }
                        Ok(message) => output = Some(self.delivery(message)?),
                        Err(e) => {
                            let again = retryable(&e);
                            if let Some(p) = pending.take() { p.fail(io::Error::new(e.kind(),e.to_string())); }
                            if !again { return Err(e); }
                            self.restore().await?;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
type Checkpoints = mpsc::Sender<(&'static str, oneshot::Sender<()>)>;
#[cfg(test)]
async fn checkpoint(checkpoints: Option<&Checkpoints>, label: &'static str) {
    if let Some(checkpoints) = checkpoints {
        let (resume, wait) = oneshot::channel();
        checkpoints.send((label, resume)).await.unwrap();
        wait.await.unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};
    use tokio::net::UnixListener;
    async fn welcome(s: &mut UnixStream) {
        assert!(matches!(
            codec::read::<_, ClientMessage>(s).await.unwrap(),
            Frame::Control(ClientMessage::Hello { .. })
        ));
        codec::write(
            s,
            &Frame::Control(ServerMessage::Welcome {
                protocol_version: 1,
                daemon_version: "test".into(),
                daemon_pid: 1,
            }),
        )
        .await
        .unwrap();
    }
    async fn response(s: &mut UnixStream, id: u64) {
        codec::write(
            s,
            &Frame::Control(ServerMessage::Response {
                request_id: id,
                result: CommandResult::Ok {
                    value: serde_json::json!({"replay_until":null}),
                },
            }),
        )
        .await
        .unwrap();
    }
    async fn cancel_next(c: &mut Client) {
        use std::future::Future;
        let f = c.next();
        tokio::pin!(f);
        std::future::poll_fn(|cx| {
            assert!(f.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        // Dropping the polled future must not cancel actor work.
    }
    #[tokio::test]
    async fn cancelled_next_at_every_reconnect_await() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let socket = dir.path().join("s");
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        let (close, closed) = oneshot::channel();
        let (finish, finished) = oneshot::channel();
        let (hooks, mut steps) = mpsc::channel(1);
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            welcome(&mut s).await;
            for _ in 0..2 {
                let Frame::Control(ClientMessage::Subscribe { request_id, .. }) =
                    codec::read(&mut s).await.unwrap()
                else {
                    panic!()
                };
                response(&mut s, request_id).await;
            }
            closed.await.unwrap();
            drop(s);
            let (mut s, _) = listener.accept().await.unwrap();
            welcome(&mut s).await;
            for index in 0..2 {
                let Frame::Control(ClientMessage::Subscribe { request_id, .. }) =
                    codec::read(&mut s).await.unwrap()
                else {
                    panic!()
                };
                if index == 1 {
                    // a replay marker interleaved before the second response
                    codec::write(
                        &mut s,
                        &Frame::Control(ServerMessage::ReplayDone {
                            thread_id: ThreadId("a".into()),
                            last: None,
                        }),
                    )
                    .await
                    .unwrap();
                }
                response(&mut s, request_id).await;
            }
            codec::write(
                &mut s,
                &Frame::Control(ServerMessage::ReplayDone {
                    thread_id: ThreadId("b".into()),
                    last: None,
                }),
            )
            .await
            .unwrap();
            finished.await.unwrap();
        });
        let mut c = Client::connect_inner(socket, "cancel".into(), Some(hooks))
            .await
            .unwrap();
        for t in ["a", "b"] {
            c.subscribe(ThreadId(t.into()), None).await.unwrap();
        }
        close.send(()).unwrap();
        let expected = [
            "connect",
            "hello_write",
            "welcome_read",
            "subscribe_write",
            "subscribe_read",
            "subscribe_write",
            "subscribe_read",
            "output_budget",
            "output_channel",
            "subscribe_read",
        ];
        for (index, label) in expected.into_iter().enumerate() {
            let (actual, resume) = steps.recv().await.unwrap();
            assert_eq!(actual, label);
            // The interleaved marker becomes visible before the final response.
            // Drain it before checking the next receive waits at this checkpoint.
            if index == 9 {
                assert!(matches!(
                    c.next().await.unwrap(),
                    ServerMessage::ReplayDone { .. }
                ));
            }
            cancel_next(&mut c).await;
            assert!(matches!(
                *c.handle.state.borrow(),
                ConnectionState::Reconnecting { .. }
            ));
            resume.send(()).unwrap();
        }
        let message = c.next().await.unwrap();
        assert!(matches!(message,ServerMessage::ReplayDone {thread_id,..} if thread_id.0=="b"));
        assert_eq!(*c.handle.state.borrow(), ConnectionState::Connected);
        finish.send(()).unwrap();
        server.await.unwrap();
    }
    #[tokio::test]
    async fn bounded_event_channel_does_not_block_request_submission() {
        // A real consumer may pause while a command is pending. The actor must
        // backpressure and retain a bounded queue, then finish when drained.
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let socket = dir.path().join("s");
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        let (sent, all_sent) = oneshot::channel();
        let (finish, finished) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            welcome(&mut s).await;
            let Frame::Control(ClientMessage::Subscribe { request_id, .. }) =
                codec::read(&mut s).await.unwrap()
            else {
                panic!()
            };
            response(&mut s, request_id).await;
            for _ in 0..EVENT_CAPACITY + 4 {
                codec::write(
                    &mut s,
                    &Frame::Control(ServerMessage::ReplayDone {
                        thread_id: ThreadId("t".into()),
                        last: None,
                    }),
                )
                .await
                .unwrap();
            }
            sent.send(()).unwrap();
            let Frame::Control(ClientMessage::Command { request_id, .. }) =
                codec::read(&mut s).await.unwrap()
            else {
                panic!()
            };
            response(&mut s, request_id).await;
            finished.await.unwrap();
        });
        let mut c = Client::connect(socket, "bounded").await.unwrap();
        c.subscribe(ThreadId("t".into()), None).await.unwrap();
        all_sent.await.unwrap();
        let handle = c.handle();
        let command = tokio::spawn(async move { handle.command(DaemonCommand::ListThreads).await });
        for _ in 0..EVENT_CAPACITY + 4 {
            c.next().await.unwrap();
        }
        assert!(matches!(
            command.await.unwrap().unwrap(),
            CommandResult::Ok { .. }
        ));
        assert!(c.events.incoming.max_capacity() == EVENT_CAPACITY);
        finish.send(()).unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_next_during_reconnect_backoff() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let socket = dir.path().join("s");
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        let (close, closed) = oneshot::channel();
        let server_path = socket.clone();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            welcome(&mut s).await;
            closed.await.unwrap();
            fs::remove_file(server_path).unwrap();
            drop(listener);
            drop(s);
        });
        let (hooks, mut steps) = mpsc::channel(1);
        let mut c = Client::connect_inner(socket.clone(), "backoff".into(), Some(hooks))
            .await
            .unwrap();
        close.send(()).unwrap();
        server.await.unwrap();
        let (label, resume) = steps.recv().await.unwrap();
        assert_eq!(label, "backoff");
        cancel_next(&mut c).await;
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        let (finish, finished) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            welcome(&mut s).await;
            finished.await.unwrap();
        });
        resume.send(()).unwrap();
        for label in ["connect", "hello_write", "welcome_read"] {
            let (actual, resume) = steps.recv().await.unwrap();
            assert_eq!(actual, label);
            cancel_next(&mut c).await;
            resume.send(()).unwrap();
        }
        c.handle
            .connection_state()
            .wait_for(|s| *s == ConnectionState::Connected)
            .await
            .unwrap();
        finish.send(()).unwrap();
        server.await.unwrap();
    }
}
