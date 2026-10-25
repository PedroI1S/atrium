//! The actor owns all transport operations, including reconnect/resubscribe.
//! Cancelling a receive or a request waiter cannot cancel that work. Commands
//! accepted by the actor may execute even if their caller drops its waiter.
//!
//! M0-07 should split Client into a cloneable ClientHandle and EventStream and
//! continuously drain the stream in a separate task. ReplayDone repeats after
//! every reconnect. The UI MUST dedupe `id <= last_ui_applied`, then acknowledge
//! with applied only after applying state (including across webview IPC).
use crate::{codec::{self, Frame}, daemon, wire::*, PROTOCOL_VERSION};
use std::{collections::BTreeMap, io, path::{Path, PathBuf}, sync::{Arc, Mutex}, time::Duration};
use tokio::{io::BufReader, net::{UnixStream, unix::OwnedWriteHalf}, sync::{mpsc, oneshot, watch, Semaphore, OwnedSemaphorePermit}};

pub const REQUEST_CAPACITY: usize = 16;
pub const EVENT_CAPACITY: usize = 32;
/// A single legal oversized frame may occupy the whole budget.
pub const EVENT_BYTES: usize = 2 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
fn protocol(message: impl Into<String>) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, message.into()) }
fn closed() -> io::Error { io::Error::new(io::ErrorKind::BrokenPipe, "client actor stopped") }
fn retryable(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused | io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted | io::ErrorKind::BrokenPipe | io::ErrorKind::UnexpectedEof | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock)
}
#[derive(Debug)]
pub struct UnknownOutcome(pub io::Error);
impl std::fmt::Display for UnknownOutcome { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f,"command outcome unknown: {}",self.0) } }
impl std::error::Error for UnknownOutcome {}
#[derive(Debug)]
pub enum CommandError { NotSent(io::Error), UnknownOutcome(UnknownOutcome) }
impl std::fmt::Display for CommandError { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { match self { Self::NotSent(e) => write!(f,"command not sent: {e}"), Self::UnknownOutcome(e) => e.fmt(f) } } }
impl std::error::Error for CommandError {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionState { Connected, Reconnecting { attempt: u32 }, Failed(String), Closed }
#[derive(Clone, Default)]
struct Cursor { applied: Option<EventId>, generation: u64 }
type Cursors = Arc<Mutex<BTreeMap<ThreadId, Cursor>>>;
struct Delivery { message: ServerMessage, generation: u64, weight: u32, _bytes: Option<OwnedSemaphorePermit> }
struct Transport { writer: OwnedWriteHalf, incoming: mpsc::Receiver<io::Result<ServerMessage>>, reader: tokio::task::JoinHandle<()> }
impl Drop for Transport { fn drop(&mut self) { self.reader.abort(); } }
impl Transport {
    async fn open(path: &Path, name: &str) -> io::Result<Self> {
        daemon::verify_socket(path)?;
        let stream = UnixStream::connect(path).await?;
        let (r, mut writer) = stream.into_split();
        let mut r = BufReader::new(r);
        codec::write(&mut writer, &Frame::Control(ClientMessage::Hello { protocol_version: PROTOCOL_VERSION, client_name: name.into() })).await?;
        match codec::read::<_,ServerMessage>(&mut r).await? {
            Frame::Control(ServerMessage::Welcome { protocol_version: PROTOCOL_VERSION, .. }) => (),
            other => return Err(protocol(format!("invalid daemon handshake: {other:?}")))
        }
        let (tx, incoming) = mpsc::channel(1);
        let reader = tokio::spawn(async move { loop {
            let msg = match codec::read::<_,ServerMessage>(&mut r).await { Ok(Frame::Control(m)) => Ok(m), Ok(_) => Err(protocol("unexpected stream frame")), Err(e) => Err(e) };
            let failed = msg.is_err();
            if tx.send(msg).await.is_err() || failed { break; }
        } });
        Ok(Self { writer, incoming, reader })
    }
    async fn send(&mut self, msg: ClientMessage) -> io::Result<()> {
        tokio::time::timeout(IO_TIMEOUT, codec::write(&mut self.writer, &Frame::Control(msg))).await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut,"socket write timed out"))?
    }
    async fn read(&mut self) -> io::Result<ServerMessage> { self.incoming.recv().await.ok_or_else(closed)? }
}
type Reply<T> = oneshot::Sender<T>;
enum Request {
    Command(DaemonCommand, Reply<Result<CommandResult,CommandError>>),
    Subscribe(ThreadId, Option<EventId>, Reply<io::Result<CommandResult>>),
    Reconnect(Reply<io::Result<()>>),
}
// If the actor exits unexpectedly after starting a write, retain uncertainty.
struct CommandReply(Option<Reply<Result<CommandResult,CommandError>>>);
impl CommandReply { fn send(mut self, result: Result<CommandResult,CommandError>) { if let Some(reply) = self.0.take() { let _ = reply.send(result); } } }
impl Drop for CommandReply { fn drop(&mut self) { if let Some(reply) = self.0.take() { let _ = reply.send(Err(CommandError::UnknownOutcome(UnknownOutcome(closed())))); } } }
enum Pending {
    Command(u64, CommandReply),
    Subscribe(u64, ThreadId, Option<EventId>, Reply<io::Result<CommandResult>>),
}
impl Pending {
    fn fail(self, e: io::Error) { match self {
        Self::Command(_, reply) => { let _ = reply.send(Err(CommandError::UnknownOutcome(UnknownOutcome(e)))); }
        Self::Subscribe(_,_,_, reply) => { let _ = reply.send(Err(e)); }
    } }
}
/// Cloneable bounded request handle. Drain EventStream concurrently with requests;
/// a slow consumer intentionally backpressures socket reads and command replies.
#[derive(Clone)]
pub struct ClientHandle { requests: mpsc::Sender<Request>, cursors: Cursors, state: watch::Receiver<ConnectionState> }
impl ClientHandle {
    pub async fn connect(path: impl AsRef<Path>, name: impl Into<String>) -> io::Result<(Self, EventStream)> { Ok(Client::connect(path,name).await?.into_parts()) }
    pub fn connection_state(&self) -> watch::Receiver<ConnectionState> { self.state.clone() }
    pub async fn command(&self, command: DaemonCommand) -> Result<CommandResult, CommandError> {
        let (tx, rx) = oneshot::channel();
        self.requests.send(Request::Command(command,tx)).await.map_err(|_| CommandError::NotSent(closed()))?;
        rx.await.map_err(|_| CommandError::NotSent(closed()))?
    }
    pub async fn subscribe(&self, thread: ThreadId, after: Option<EventId>) -> io::Result<CommandResult> {
        let (tx,rx) = oneshot::channel(); self.requests.send(Request::Subscribe(thread,after,tx)).await.map_err(|_| closed())?; rx.await.map_err(|_| closed())?
    }
    pub async fn reconnect(&self) -> io::Result<()> {
        let (tx,rx) = oneshot::channel(); self.requests.send(Request::Reconnect(tx)).await.map_err(|_| closed())?; rx.await.map_err(|_| closed())?
    }
    pub fn applied(&self, thread: &ThreadId, id: EventId) {
        if let Some(c) = self.cursors.lock().unwrap().get_mut(thread) && c.applied.is_none_or(|old| id > old) { c.applied = Some(id); }
    }
}
/// One bounded stream. Dropping it stops the actor, even if handles remain.
/// next() only receives from a channel: it is cancellation-safe at every await,
/// including while the actor handshakes or restores multiple subscriptions.
pub struct EventStream { incoming: mpsc::Receiver<Delivery>, cursors: Cursors, state: watch::Receiver<ConnectionState> }
impl EventStream {
    pub async fn next(&mut self) -> io::Result<ServerMessage> {
        loop {
            let delivery = self.incoming.recv().await.ok_or_else(|| match &*self.state.borrow() { ConnectionState::Failed(message) => protocol(message.clone()), _ => closed() })?;
            let cursors = self.cursors.lock().unwrap();
            match &delivery.message {
                ServerMessage::Event { event } => {
                    if cursors.get(&event.thread_id).is_none_or(|c| c.generation != delivery.generation || c.applied.is_some_and(|id| event.id <= id)) { continue; }
                }
                ServerMessage::ReplayDone { thread_id, .. } => {
                    if cursors.get(thread_id).is_none_or(|c| c.generation != delivery.generation) { continue; }
                }
                _ => ()
            }
            return Ok(delivery.message);
        }
    }
}
/// Convenience facade for serial CLI callers. Use into_parts() for UI use.
pub struct Client { handle: ClientHandle, events: EventStream }
impl Client {
    pub async fn connect(path: impl AsRef<Path>, name: impl Into<String>) -> io::Result<Self> {
        let path = path.as_ref().to_owned(); let name = name.into();
        let transport = tokio::time::timeout(IO_TIMEOUT,Transport::open(&path,&name)).await.map_err(|_| io::Error::new(io::ErrorKind::TimedOut,"handshake timed out"))??;
        let (requests, receiver) = mpsc::channel(REQUEST_CAPACITY);
        let (output,incoming) = mpsc::channel(EVENT_CAPACITY);
        let (status,state) = watch::channel(ConnectionState::Connected);
        let cursors = Cursors::default();
        let handle = ClientHandle { requests, cursors: cursors.clone(), state: state.clone() };
        let events = EventStream { incoming, cursors: cursors.clone(), state };
        let actor = Actor { path, name, transport: Some(transport), requests: receiver, output, cursors, status, next_id: 0, budget: Arc::new(Semaphore::new(EVENT_BYTES)) };
        tokio::spawn(actor.run());
        Ok(Self { handle, events })
    }
    pub fn into_parts(self) -> (ClientHandle,EventStream) { (self.handle,self.events) }
    pub fn handle(&self) -> ClientHandle { self.handle.clone() }
    pub async fn hello(&mut self) -> io::Result<()> { if *self.handle.state.borrow() == ConnectionState::Connected { Ok(()) } else { self.reconnect().await } }
    pub async fn subscribe(&mut self, thread: ThreadId, after: Option<EventId>) -> io::Result<CommandResult> { self.handle.subscribe(thread,after).await }
    pub async fn command(&mut self, command: DaemonCommand) -> Result<CommandResult,CommandError> { self.handle.command(command).await }
    pub async fn reconnect(&mut self) -> io::Result<()> { self.handle.reconnect().await }
    pub fn applied(&mut self, thread: &ThreadId, id: EventId) { self.handle.applied(thread,id); }
    pub async fn next(&mut self) -> io::Result<ServerMessage> { self.events.next().await }
}
struct Actor { path: PathBuf, name: String, transport: Option<Transport>, requests: mpsc::Receiver<Request>, output: mpsc::Sender<Delivery>, cursors: Cursors, status: watch::Sender<ConnectionState>, next_id: u64, budget: Arc<Semaphore> }
impl Actor {
    fn id(&mut self) -> u64 { self.next_id += 1; self.next_id }
    fn delivery(&self, message: ServerMessage) -> io::Result<Delivery> {
        let thread = match &message { ServerMessage::Event {event} => &event.thread_id, ServerMessage::ReplayDone {thread_id,..} => thread_id, _ => return Err(protocol("unexpected server message")) };
        let generation = self.cursors.lock().unwrap().get(thread).ok_or_else(|| protocol("event for unsubscribed thread"))?.generation;
        let weight = serde_json::to_vec(&message).map_err(crate::error)?.len().clamp(1,EVENT_BYTES) as u32;
        Ok(Delivery { message, generation, weight, _bytes: None })
    }
    async fn restore_once(&mut self) -> io::Result<()> {
        self.transport = None;
        self.transport = Some(tokio::time::timeout(IO_TIMEOUT,Transport::open(&self.path,&self.name)).await.map_err(|_| io::Error::new(io::ErrorKind::TimedOut,"handshake timed out"))??);
        let cursors = self.cursors.lock().unwrap().clone();
        for (thread,cursor) in cursors {
            let id = self.id();
            self.transport.as_mut().unwrap().send(ClientMessage::Subscribe {request_id:id,thread_id:thread,after:cursor.applied}).await?;
            loop {
                let message = tokio::time::timeout(IO_TIMEOUT,self.transport.as_mut().unwrap().read()).await.map_err(|_| io::Error::new(io::ErrorKind::TimedOut,"subscribe timed out"))??;
                match message {
                    ServerMessage::Response {request_id,result} if request_id == id => { match result { CommandResult::Ok {..} => break, CommandResult::Err {message} => return Err(protocol(message)) } }
                    message => { let mut delivery = self.delivery(message)?; delivery._bytes = Some(self.budget.clone().acquire_many_owned(delivery.weight).await.map_err(crate::error)?); self.output.send(delivery).await.map_err(|_| closed())?; }
                }
            }
        }
        Ok(())
    }
    async fn restore(&mut self) -> io::Result<()> {
        let mut attempt = 0u32;
        loop {
            attempt = attempt.saturating_add(1);
            self.status.send_replace(ConnectionState::Reconnecting {attempt});
            // Invalidate all buffered deliveries from previous connections.
            for c in self.cursors.lock().unwrap().values_mut() { c.generation += 1; }
            let output = self.output.clone();
            let result = tokio::select! { _ = output.closed() => return Err(closed()), result = self.restore_once() => result };
            match result {
                Ok(()) => { self.status.send_replace(ConnectionState::Connected); return Ok(()); }
                Err(e) if retryable(&e) => {
                    let delay = Duration::from_millis((25u64 << attempt.min(5)).min(1000));
                    tokio::select! { _ = self.output.closed() => return Err(closed()), _ = tokio::time::sleep(delay) => () }
                }
                Err(e) => return Err(e)
            }
        }
    }
    async fn run(mut self) {
        let result = self.drive().await;
        self.status.send_replace(match result { Ok(()) => ConnectionState::Closed, Err(e) => ConnectionState::Failed(e.to_string()) });
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
                    // Once this write begins, a command can have an unknown outcome.
                    if let Err(e) = self.transport.as_mut().unwrap().send(message).await {
                        pending.take().unwrap().fail(e);
                        output = None;
                        self.restore().await?;
                    }
                }
                message = transport.read(), if output.is_none() => {
                    match message {
                        Ok(ServerMessage::Response {request_id,result}) => {
                            match pending.take() {
                                Some(Pending::Command(id,reply)) if id==request_id => { let _ = reply.send(Ok(result)); }
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
