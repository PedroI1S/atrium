//! Small, single-consumer client for M0-07. `connect` performs `hello`;
//! `subscribe` returns the wire result (including replay_until). Consume
//! `next()` until ReplayDone, then continue for live events. Call `applied`
//! only after updating consumer state. Persist these cursors across app restarts.
//!
//! `next` reconnects and resubscribes automatically. Commands are sent once;
//! missing responses return UnknownOutcome. Re-read ListThreads before deciding
//! whether to repeat a command. One mutable Client serializes API calls.
//! Do not cancel command/subscribe futures after they have begun writing.
use crate::{
    PROTOCOL_VERSION,
    codec::{self, Frame},
    error,
    wire::*,
};
use std::{
    collections::{BTreeMap, VecDeque},
    io,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    net::{UnixStream, unix::OwnedWriteHalf},
    sync::mpsc,
};
#[derive(Debug)]
pub struct UnknownOutcome(pub io::Error);
impl std::fmt::Display for UnknownOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "command outcome unknown: {}", self.0)
    }
}
impl std::error::Error for UnknownOutcome {}
struct Transport {
    w: OwnedWriteHalf,
    rx: mpsc::Receiver<io::Result<ServerMessage>>,
    reader: tokio::task::JoinHandle<()>,
}
impl Drop for Transport {
    fn drop(&mut self) {
        self.reader.abort();
    }
}
pub struct Client {
    path: PathBuf,
    name: String,
    transport: Option<Transport>,
    request: u64,
    cursors: BTreeMap<ThreadId, Option<EventId>>,
    queued: VecDeque<ServerMessage>,
}
impl Client {
    pub async fn connect(path: impl AsRef<Path>, name: impl Into<String>) -> io::Result<Self> {
        let mut c = Self {
            path: path.as_ref().into(),
            name: name.into(),
            transport: None,
            request: 0,
            cursors: BTreeMap::new(),
            queued: VecDeque::new(),
        };
        c.hello().await?;
        Ok(c)
    }
    /// Idempotent on an already handshaken connection.
    pub async fn hello(&mut self) -> io::Result<()> {
        if self.transport.is_some() {
            return Ok(());
        }
        let mut stream = UnixStream::connect(&self.path).await?;
        codec::write(
            &mut stream,
            &Frame::Control(ClientMessage::Hello {
                protocol_version: PROTOCOL_VERSION,
                client_name: self.name.clone(),
            }),
        )
        .await?;
        match codec::read::<_, ServerMessage>(&mut stream).await? {
            Frame::Control(ServerMessage::Welcome {
                protocol_version: PROTOCOL_VERSION,
                ..
            }) => (),
            _ => return Err(error("incompatible daemon handshake")),
        }
        let (mut r, w) = stream.into_split();
        let (tx, rx) = mpsc::channel(128);
        let reader = tokio::spawn(async move {
            loop {
                let msg = match codec::read::<_, ServerMessage>(&mut r).await {
                    Ok(Frame::Control(msg)) => Ok(msg),
                    Ok(_) => Err(error("stream frames unsupported by Client")),
                    Err(e) => Err(e),
                };
                let failed = msg.is_err();
                if tx.send(msg).await.is_err() || failed {
                    break;
                }
            }
        });
        self.transport = Some(Transport { w, rx, reader });
        Ok(())
    }
    fn id(&mut self) -> u64 {
        self.request += 1;
        self.request
    }
    async fn send(&mut self, msg: ClientMessage) -> io::Result<()> {
        let t = self
            .transport
            .as_mut()
            .ok_or_else(|| error("disconnected"))?;
        codec::write(&mut t.w, &Frame::Control(msg)).await
    }
    async fn read(&mut self) -> io::Result<ServerMessage> {
        self.transport
            .as_mut()
            .ok_or_else(|| error("disconnected"))?
            .rx
            .recv()
            .await
            .ok_or_else(|| error("connection closed"))?
    }
    async fn response(&mut self, request: u64) -> io::Result<CommandResult> {
        loop {
            let msg = self.read().await?;
            if let ServerMessage::Response { request_id, result } = msg {
                if request_id == request {
                    return Ok(result);
                }
            } else {
                self.queued.push_back(msg);
            }
        }
    }
    pub async fn subscribe(
        &mut self,
        thread: ThreadId,
        after: Option<EventId>,
    ) -> io::Result<CommandResult> {
        let id = self.id();
        let r = async {
            self.send(ClientMessage::Subscribe {
                request_id: id,
                thread_id: thread.clone(),
                after,
            })
            .await?;
            self.response(id).await
        }
        .await;
        match r {
            Ok(result) => {
                if matches!(result, CommandResult::Ok { .. }) {
                    self.cursors.insert(thread.clone(), after);
                    self.queued.retain(|m| !matches!(m, ServerMessage::Event {event} if event.thread_id == thread) && !matches!(m, ServerMessage::ReplayDone {thread_id,..} if *thread_id == thread));
                }
                Ok(result)
            }
            Err(e) => {
                self.transport = None;
                Err(e)
            }
        }
    }
    /// Records a monotonic consumer cursor. Reading an event never advances it.
    pub fn applied(&mut self, thread: &ThreadId, id: EventId) {
        if let Some(cursor) = self.cursors.get_mut(thread)
            && cursor.is_none_or(|old| id > old)
        {
            *cursor = Some(id);
        }
    }
    /// Restore after a deliberate UI disconnect, or let `next` do it on EOF.
    pub async fn reconnect(&mut self) -> io::Result<()> {
        self.transport = None;
        self.queued.clear();
        self.hello().await?;
        let cursors = self.cursors.clone();
        for (thread, after) in cursors {
            match self.subscribe(thread, after).await? {
                CommandResult::Ok { .. } => (),
                CommandResult::Err { message } => return Err(error(message)),
            }
        }
        Ok(())
    }
    /// Never retries this command, even if its response is lost.
    pub async fn command(
        &mut self,
        command: DaemonCommand,
    ) -> Result<CommandResult, UnknownOutcome> {
        let id = self.id();
        let r = async {
            self.send(ClientMessage::Command {
                request_id: id,
                command,
            })
            .await?;
            self.response(id).await
        }
        .await;
        r.map_err(|e| {
            self.transport = None;
            UnknownOutcome(e)
        })
    }
    /// Receives replay markers and events, suppressing events already applied.
    /// Cancelling this method is safe: the persistent reader retains frame state.
    pub async fn next(&mut self) -> io::Result<ServerMessage> {
        loop {
            let msg = if let Some(msg) = self.queued.pop_front() {
                Ok(msg)
            } else {
                self.read().await
            };
            match msg {
                Ok(ServerMessage::Event { event }) => {
                    if self
                        .cursors
                        .get(&event.thread_id)
                        .is_some_and(|cursor| cursor.is_some_and(|id| event.id <= id))
                    {
                        continue;
                    }
                    return Ok(ServerMessage::Event { event });
                }
                Ok(msg) => return Ok(msg),
                Err(_) => loop {
                    match self.reconnect().await {
                        Ok(()) => break,
                        Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
                    }
                },
            }
        }
    }
}
