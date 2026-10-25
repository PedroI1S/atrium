//! Daemon, socket security, child ownership and replay delivery.
use crate::{
    PROTOCOL_VERSION,
    agent::*,
    codec::{self, Frame},
    error, now_ms,
    store::{self, Store},
    wire::*,
};
use nix::{
    sys::signal::{Signal, kill},
    unistd::{Pid, geteuid},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File, OpenOptions},
    io,
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, PermissionsExt, FileTypeExt, OpenOptionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    process::Command,
    sync::{Mutex, mpsc, oneshot, Semaphore},
};

pub fn default_dir() -> PathBuf {
    std::env::temp_dir().join("atrium-spike")
}
pub fn verify_dir_metadata(meta: &fs::Metadata, uid: u32) -> io::Result<()> {
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || meta.uid() != uid
        || meta.mode() & 0o7777 != 0o700
    {
        return Err(error(
            "daemon directory must be a real directory owned by current user with mode 0700",
        ));
    }
    Ok(())
}
/// Check the endpoint before connecting, including for reconnect attempts.
pub fn verify_socket(path: &Path) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput,"socket has no directory"))?;
    verify_dir_metadata(&fs::symlink_metadata(dir)?, geteuid().as_raw())
        .map_err(|e| io::Error::new(io::ErrorKind::PermissionDenied,e))?;
    let meta = fs::symlink_metadata(path)?;
    if !meta.file_type().is_socket() || meta.uid() != geteuid().as_raw() || meta.mode() & 0o7777 != 0o600 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied,"socket must be owned by current user with mode 0600"));
    }
    Ok(())
}
pub struct SocketGuard {
    _lock: File,
    pub socket: PathBuf,
}
impl SocketGuard {
    pub fn acquire(dir: &Path) -> io::Result<Self> {
        match fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e),
        }
        verify_dir_metadata(&fs::symlink_metadata(dir)?, geteuid().as_raw())?;
        let socket = dir.join("daemon.sock");
        if socket.as_os_str().as_encoded_bytes().len() > 103 {
            return Err(error(
                "socket path exceeds portable 103-byte sun_path limit; use a shorter --dir",
            ));
        }
        let lock_path = dir.join("daemon.lock");
        if fs::symlink_metadata(&lock_path)
            .is_ok_and(|m| !m.is_file() || m.file_type().is_symlink())
        {
            return Err(error("invalid lock file"));
        }
        let lock = OpenOptions::new()
            .create(true)
            .mode(0o600)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        lock.set_permissions(fs::Permissions::from_mode(0o600))?;
        lock.try_lock()
            .map_err(|e| error(format!("another daemon holds the lock: {e}")))?;
        match fs::remove_file(&socket) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        Ok(Self {
            _lock: lock,
            socket,
        })
    }
}
impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket);
    }
}

/// Native process identity. Unavailable or incomplete identity fails closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub pid: u32,
    pub start: String,
    pub exe: String,
}
pub fn identity(pid: u32) -> io::Result<Identity> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut system = System::new();
    let id = Pid::from_u32(pid);
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[id]),
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
    );
    let process = system
        .process(id)
        .ok_or_else(|| error("process identity unavailable"))?;
    let exe = process
        .exe()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| error("executable unavailable"))?;
    if process.start_time() == 0 {
        return Err(error("process start time unavailable"));
    }
    Ok(Identity {
        pid,
        start: process.start_time().to_string(),
        exe: exe.to_string_lossy().into_owned(),
    })
}
/// Resident bytes for a single process, using the same native inspection API.
pub fn rss(pid: u32) -> io::Result<u64> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let mut system = System::new();
    let id = Pid::from_u32(pid);
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[id]),
        true,
        ProcessRefreshKind::nothing().with_memory(),
    );
    system
        .process(id)
        .map(|p| p.memory())
        .filter(|n| *n != 0)
        .ok_or_else(|| error("RSS unavailable"))
}
pub fn reap_if_matching(saved: &Identity) -> io::Result<bool> {
    if identity(saved.pid).ok().as_ref() != Some(saved) {
        eprintln!(
            "orphan {} identity ambiguous or changed; left alone",
            saved.pid
        );
        return Ok(false);
    }
    signal_result(kill(Pid::from_raw(saved.pid as i32), Signal::SIGKILL))?;
    Ok(true)
}

fn signal_result(result: Result<(), nix::errno::Errno>) -> io::Result<()> {
    match result { Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()), Err(e) => Err(error(e)) }
}
/// macOS physical footprint includes compressed/private memory absent from RSS.
pub fn phys_footprint(pid: u32) -> io::Result<Option<u64>> {
    #[cfg(target_os = "macos")]
    { libproc::libproc::pid_rusage::pidrusage::<libproc::libproc::pid_rusage::RUsageInfoV2>(pid as i32)
        .map(|r| Some(r.ri_phys_footprint)).map_err(error) }
    #[cfg(not(target_os = "macos"))]
    { let _ = pid; Ok(None) }
}
static READERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn memory_trace(label: &str) {
    if std::env::var_os("ATRIUM_MEMORY_DIAGNOSTICS").is_some() {
        eprintln!("memory phase={label} readers={} rss={:?} footprint={:?}",
            READERS.load(std::sync::atomic::Ordering::Relaxed), rss(std::process::id()), phys_footprint(std::process::id()));
    }
}
struct ReaderCount;
impl ReaderCount { fn new() -> Self { READERS.fetch_add(1,std::sync::atomic::Ordering::Relaxed); memory_trace("reader_open"); Self } }
impl Drop for ReaderCount { fn drop(&mut self) { READERS.fetch_sub(1,std::sync::atomic::Ordering::Relaxed); memory_trace("reader_closed"); } }
/// Reconstruct only open lifecycle objects; complete them before SessionEnded.
pub fn closing(
    events: impl IntoIterator<Item = AgentEventEnvelope>,
    reason: &str,
) -> Vec<AgentEventEnvelope> {
    let mut seq = 0;
    let mut ended = false;
    let mut messages = BTreeMap::<String, (MessageKind, String)>::new();
    let mut tools = BTreeMap::<String, ()>::new();
    let mut requests = BTreeMap::<String, ()>::new();
    let mut turn = None;
    for e in events {
        seq = e.seq + 1;
        match e.event {
            AgentEvent::MessageDelta {
                message_id,
                kind,
                text,
            } => messages
                .entry(message_id)
                .or_insert((kind, String::new()))
                .1
                .push_str(&text),
            AgentEvent::MessageCompleted { message_id, .. } => {
                messages.remove(&message_id);
            }
            AgentEvent::ToolCallStarted { call_id, .. } => {
                tools.insert(call_id, ());
            }
            AgentEvent::ToolCallFinished { call_id, .. } => {
                tools.remove(&call_id);
            }
            AgentEvent::ApprovalRequested { request_id, .. }
            | AgentEvent::QuestionAsked { request_id, .. } => {
                requests.insert(request_id, ());
            }
            AgentEvent::RequestResolved { request_id, .. } => {
                requests.remove(&request_id);
            }
            AgentEvent::TurnStarted { turn: n } => turn = Some(n),
            AgentEvent::TurnCompleted { .. } => turn = None,
            AgentEvent::SessionEnded { .. } => ended = true,
            _ => (),
        }
    }
    if ended {
        return vec![];
    }
    let was_empty = seq == 0;
    let mut out = vec![];
    let mut emit = |event| {
        out.push(AgentEventEnvelope {
            seq,
            at_ms: now_ms(),
            event,
        });
        seq += 1;
    };
    if was_empty {
        emit(AgentEvent::Error {
            message: reason.into(),
            fatal: true,
        });
    }
    for (message_id, (kind, text)) in messages {
        emit(AgentEvent::MessageCompleted {
            message_id,
            kind,
            text,
        });
    }
    for (call_id, ()) in tools {
        emit(AgentEvent::ToolCallFinished {
            call_id,
            status: ToolStatus::Cancelled,
            output: None,
            error: None,
        });
    }
    for (request_id, ()) in requests {
        emit(AgentEvent::RequestResolved {
            request_id,
            outcome: RequestOutcome::Cancelled,
        });
    }
    if let Some(turn) = turn {
        emit(AgentEvent::TurnCompleted {
            turn,
            outcome: TurnOutcome::Interrupted,
            usage: None,
        });
    }
    emit(AgentEvent::SessionEnded {
        exit_code: None,
        reason: Some(reason.into()),
    });
    out
}
async fn close_thread(store: &Store, thread: ThreadId, reason: &str) -> io::Result<()> {
    let c = store::open_reader(&store.path)?;
    // This spike reconstructs open state on shutdown; M1 should persist an open
    // object projection so it need not scan the thread.
    let mut after = None;
    let mut all = vec![];
    loop {
        let page = store::page(&c, &thread, after, None)?;
        if page.is_empty() {
            break;
        }
        after = page.last().map(|e| e.id);
        all.extend(page.into_iter().map(|e| e.envelope));
    }
    if std::env::var_os("ATRIUM_MEMORY_DIAGNOSTICS").is_some() {
        let bytes: usize = all.iter().map(|e| match &e.event { AgentEvent::MessageDelta {text,..} | AgentEvent::MessageCompleted {text,..} => text.len(), _ => 0 }).sum();
        eprintln!("lifecycle_scan events={} message_text_bytes={bytes}", all.len());
        memory_trace("history_loaded");
    }
    store.append(thread.clone(), closing(all, reason)).await?;
    memory_trace("history_released");
    store
        .call(move |c| {
            c.execute("UPDATE threads SET running=0 WHERE id=?1", [thread.0])
                .map_err(error)?;
            Ok(())
        })
        .await
}
async fn recover(store: &Store) -> io::Result<()> {
    let rows = store
        .call(|c| {
            let mut q = c
                .prepare("SELECT id,pid,start,exe FROM threads WHERE running=1")
                .map_err(error)?;
            let rows = q
                .query_map([], |r| {
                    Ok((
                        ThreadId(r.get(0)?),
                        r.get::<_, Option<u32>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                    ))
                })
                .map_err(error)?;
            rows.map(|r| r.map_err(error))
                .collect::<io::Result<Vec<_>>>()
        })
        .await?;
    for (thread, pid, start, exe) in rows {
        if let (Some(pid), Some(start), Some(exe)) = (pid, start, exe) {
            let _ = reap_if_matching(&Identity { pid, start, exe })?;
        } else {
            eprintln!(
                "thread {} has incomplete process identity; left alone",
                thread.0
            );
        }
        close_thread(store, thread, "Interrupted: daemon restarted").await?;
    }
    Ok(())
}
struct State {
    store: Store,
    children: Mutex<HashMap<ThreadId, mpsc::Sender<StdinRequest>>>,
    fake: PathBuf,
    replay_gate: Option<ReplayGate>,
}
impl State {
    async fn command(self: &Arc<Self>, command: DaemonCommand) -> io::Result<serde_json::Value> {
        match command {
            DaemonCommand::ListThreads => self.store.call(|c| {
                let mut q = c.prepare("SELECT t.id,t.running,(SELECT max(id) FROM events WHERE thread_id=t.id) FROM threads t ORDER BY t.id").map_err(error)?;
                let rows = q.query_map([], |r| Ok(json!({"thread_id": r.get::<_,String>(0)?, "running": r.get::<_,bool>(1)?, "last_event": r.get::<_,Option<i64>>(2)?}))).map_err(error)?;
                Ok(serde_json::Value::Array(rows.map(|r| r.map_err(error)).collect::<io::Result<Vec<_>>>()?))
            }).await,
            DaemonCommand::StartThread { agent: AgentLaunch::Native { .. }, .. } => Err(error("Native agents are unsupported in M0")),
            DaemonCommand::StartThread { agent, cwd } => {
                if let AgentLaunch::Fake { tokens_per_second, chunk_chars, .. } = &agent
                    && (*tokens_per_second == 0 || *chunk_chars == 0 || *chunk_chars > 1_000_000)
                { return Err(error("invalid fake rate/chunk size")); }

                static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let thread = ThreadId(format!("{}-{}-{}", now_ms(), std::process::id(), COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
                let mut cmd = Command::new(&self.fake);
                cmd.kill_on_drop(true);
                cmd.arg(serde_json::to_string(&agent).map_err(error)?).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
                cmd.as_std_mut().process_group(0);
                let mut child = cmd.spawn()?;
                let pid = child.id().ok_or_else(|| error("missing child PID"))?;
                let saved = match identity(pid) { Ok(v) => v, Err(e) => { child.kill().await?; return Err(e); } };
                let id = thread.clone();
                if let Err(e) = self.store.call(move |c| { c.execute("INSERT INTO threads(id,running,pid,start,exe) VALUES(?1,1,?2,?3,?4)", rusqlite::params![id.0,saved.pid,saved.start,saved.exe]).map_err(error)?; Ok(()) }).await { child.kill().await?; return Err(e); }
                let mut stdin = child.stdin.take().ok_or_else(|| error("missing stdin"))?;
                stdin.write_all(b"go\n").await?;
                let (commands, writer) = spawn_stdin_writer(stdin);
                self.children.lock().await.insert(thread.clone(), commands);
                let stdout = child.stdout.take().ok_or_else(|| error("missing stdout"))?;
                let state = self.clone(); let id = thread.clone();
                tokio::spawn(async move {
                    let mut lines = BufReader::new(stdout).lines();
                    loop {
                        match lines.next_line().await {
                            Ok(Some(line)) => match serde_json::from_str(&line) {
                                Ok(event) => if let Err(e) = state.store.append(id.clone(), vec![event]).await { eprintln!("store: {e}"); break; },
                                Err(e) => { eprintln!("invalid child event: {e}"); break; }
                            },
                            Ok(None) => break,
                            Err(e) => { eprintln!("child stdout: {e}"); break; }
                        }
                    }
                    let _ = child.kill().await; // also waits/reaps; harmless after normal exit
                    let _ = child.wait().await;
                    state.children.lock().await.remove(&id);
                    writer.abort(); let _ = writer.await;
                    if let Err(e) = close_thread(&state.store, id, "Interrupted: child exited").await { eprintln!("close: {e}"); }
                });
                Ok(json!({"thread_id": thread}))
            }
            DaemonCommand::ThreadCommand { thread_id, command } => self.forward(thread_id, command).await,
            DaemonCommand::StopThread { thread_id } => self.forward(thread_id, SessionCommand::Shutdown).await,
        }
    }
    async fn forward(
        &self,
        thread: ThreadId,
        command: SessionCommand,
    ) -> io::Result<serde_json::Value> {
        let sender = self.children.lock().await.get(&thread).cloned()
            .ok_or_else(|| error("thread is not running"))?;
        let mut line = serde_json::to_vec(&command).map_err(error)?; line.push(b'\n');
        let (reply, rx) = oneshot::channel();
        sender.try_send(StdinRequest { line, reply }).map_err(|_| error("child command queue full or closed"))?;
        tokio::time::timeout(std::time::Duration::from_secs(5),rx).await.map_err(error)?.map_err(error)??;
        Ok(json!({}))
    }
}
struct StdinRequest { line: Vec<u8>, reply: oneshot::Sender<io::Result<()>> }
fn spawn_stdin_writer<W: tokio::io::AsyncWrite + Unpin + Send + 'static>(mut stdin: W) -> (mpsc::Sender<StdinRequest>, tokio::task::JoinHandle<()>) {
    let (sender, mut commands) = mpsc::channel::<StdinRequest>(16);
    let writer = tokio::spawn(async move {
        while let Some(request) = commands.recv().await {
            let result = tokio::time::timeout(std::time::Duration::from_secs(5),stdin.write_all(&request.line)).await.map_err(error).and_then(|r| r);
            let failed = result.is_err(); let _ = request.reply.send(result); if failed { break; }
        }
    }); (sender, writer)
}
/// Deterministic replay test seam: pause after response, before the first page.
#[derive(Clone)]
pub struct ReplayGate { pub entered: mpsc::Sender<ThreadId>, pub release: Arc<Semaphore> }
fn result(r: io::Result<serde_json::Value>) -> CommandResult {
    match r {
        Ok(value) => CommandResult::Ok { value },
        Err(e) => CommandResult::Err {
            message: e.to_string(),
        },
    }
}
struct Subscription {
    cursor: Option<EventId>,
    boundary: Option<EventId>,
    replay: bool,
}
async fn send(w: &mut tokio::net::unix::OwnedWriteHalf, message: ServerMessage) -> io::Result<()> {
    codec::write(w, &Frame::Control(message)).await
}
async fn connection(stream: UnixStream, state: Arc<State>) -> io::Result<()> {
    let (r, mut w) = stream.into_split();
    let mut r = BufReader::new(r);
    match codec::read::<_, ClientMessage>(&mut r).await? {
        Frame::Control(ClientMessage::Hello {
            protocol_version, ..
        }) if protocol_version == PROTOCOL_VERSION => {
            send(
                &mut w,
                ServerMessage::Welcome {
                    protocol_version: PROTOCOL_VERSION,
                    daemon_version: env!("CARGO_PKG_VERSION").into(),
                    daemon_pid: std::process::id(),
                },
            )
            .await?
        }
        Frame::Control(ClientMessage::Hello { .. }) => {
            send(
                &mut w,
                ServerMessage::Incompatible {
                    protocol_version: PROTOCOL_VERSION,
                    message: "protocol version mismatch".into(),
                },
            )
            .await?;
            return Ok(());
        }
        _ => return Err(error("Hello required")),
    }
    let (tx, mut rx) = mpsc::channel(16);
    let reader = tokio::spawn(async move {
        loop {
            let msg = codec::read::<_, ClientMessage>(&mut r).await;
            let failed = msg.is_err();
            if tx.send(msg).await.is_err() || failed {
                break;
            }
        }
    });
    // Abort even on early returns: no reader task remains after socket failure.
    struct ReaderGuard(tokio::task::JoinHandle<()>);
    impl Drop for ReaderGuard {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _reader = ReaderGuard(reader);
    let _reader_count = ReaderCount::new();
    let mut c = store::open_reader(&state.store.path)?;
    let mut changed = state.store.changed.clone();
    let mut subscriptions = BTreeMap::<ThreadId, Subscription>::new();
    loop {
        // Each pass sends at most 128 events per thread, then services requests.
        // The watch receiver was created before the first snapshot query.
        let mut progress = false;
        if let Ok(msg) = rx.try_recv() {
            handle(msg?, &state, &mut c, &mut subscriptions, &mut w).await?;
        }
        for (thread, sub) in &mut subscriptions {
            let events = store::page(
                &c,
                thread,
                sub.cursor,
                if sub.replay {
                    sub.boundary.or(Some(EventId(0)))
                } else {
                    None
                },
            )?;
            let empty = events.is_empty();
            progress |= !empty;
            for event in events {
                sub.cursor = Some(event.id);
                send(&mut w, ServerMessage::Event { event }).await?;
            }
            if sub.replay && (empty || sub.cursor == sub.boundary) {
                sub.replay = false;
                progress = true;
                send(
                    &mut w,
                    ServerMessage::ReplayDone {
                        thread_id: thread.clone(),
                        last: sub.boundary,
                    },
                )
                .await?;
            }
        }
        if progress {
            tokio::task::yield_now().await;
            continue;
        }
        tokio::select! {
            msg = rx.recv() => { let msg = msg.ok_or_else(|| error("client closed"))??; handle(msg, &state, &mut c, &mut subscriptions, &mut w).await?; }
            change = changed.changed() => { change.map_err(error)?; }
        }
    }
}
async fn handle(
    msg: Frame<ClientMessage>,
    state: &Arc<State>,
    c: &mut rusqlite::Connection,
    subscriptions: &mut BTreeMap<ThreadId, Subscription>,
    w: &mut tokio::net::unix::OwnedWriteHalf,
) -> io::Result<()> {
    match msg {
        Frame::Control(ClientMessage::Subscribe {
            request_id,
            thread_id,
            after,
        }) => {
            let exists = c
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM threads WHERE id=?1)",
                    [&thread_id.0],
                    |r| r.get::<_, bool>(0),
                )
                .map_err(error)?;
            let boundary = store::last(c, &thread_id)?;
            let subscribed = thread_id.clone();
            let r = if !exists {
                Err(error("unknown thread"))
            } else if after.is_some_and(|a| a > boundary.unwrap_or(EventId(0))) {
                Err(error("cursor past end of thread"))
            } else {
                subscriptions.insert(
                    thread_id,
                    Subscription {
                        cursor: after,
                        boundary,
                        replay: true,
                    },
                );
                Ok(json!({"replay_until": boundary}))
            };
            let success = r.is_ok();
            send(w, ServerMessage::Response { request_id, result: result(r) }).await?;
            if success && let Some(gate) = &state.replay_gate {
                gate.entered.send(subscribed).await.map_err(error)?;
                gate.release.acquire().await.map_err(error)?.forget();
            }
            Ok(())
        }
        Frame::Control(ClientMessage::Unsubscribe { thread_id }) => {
            subscriptions.remove(&thread_id);
            Ok(())
        }
        Frame::Control(ClientMessage::Command {
            request_id,
            command,
        }) => {
            let r = state.command(command).await;
            send(
                w,
                ServerMessage::Response {
                    request_id,
                    result: result(r),
                },
            )
            .await
        }
        _ => Err(error("unexpected frame after handshake")),
    }
}
/// Runs until the process is stopped. Keep `--dir` short on macOS.
pub async fn run(dir: &Path, fake: PathBuf) -> io::Result<()> {
    let guard = SocketGuard::acquire(dir)?;
    let store = Store::open(&dir.join("events.sqlite"))?;
    serve(guard, store, fake, None, None).await
}
/// In-process harness with the same single writer used to seed fixtures.
pub async fn run_with_store(dir: &Path, fake: PathBuf, store: Store) -> io::Result<()> {
    let guard = SocketGuard::acquire(dir)?;
    serve(guard, store, fake, None, None).await
}
pub async fn run_with_replay_gate(dir: &Path, fake: PathBuf, store: Store, gate: ReplayGate, ready: oneshot::Sender<()>) -> io::Result<()> {
    serve(SocketGuard::acquire(dir)?, store, fake, Some(gate), Some(ready)).await
}
async fn serve(guard: SocketGuard, store: Store, fake: PathBuf, replay_gate: Option<ReplayGate>, ready: Option<oneshot::Sender<()>>) -> io::Result<()> {
    recover(&store).await?;
    // Publish only after setting permissions; no permissive public socket window.
    let staging = guard.socket.with_file_name(".bind");
    match fs::remove_file(&staging) { Ok(()) => (), Err(e) if e.kind()==io::ErrorKind::NotFound => (), Err(e) => return Err(e) }
    let listener = UnixListener::bind(&staging)?;
    fs::set_permissions(&staging, fs::Permissions::from_mode(0o600))?;
    fs::rename(&staging,&guard.socket)?;
    let state = Arc::new(State {
        store,
        children: Mutex::new(HashMap::new()),
        fake,
        replay_gate,
    });
    if let Some(ready) = ready { let _ = ready.send(()); }
    println!("READY");
    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = connection(stream, state).await {
                eprintln!("client disconnected: {e}");
            }
        });
    }
}
