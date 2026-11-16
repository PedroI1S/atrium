//! The bridge between webviews and `atrium-client` (contract §11).
//!
//! - One [`Client`] for the process. It connects as the app, and starts
//!   `atriumd` when nothing is listening.
//! - Each webview attaches once: the bridge opens a client window under the
//!   webview's label and forwards its deliveries, in order, as
//!   `{"seq": n, "delivery": <the client's JSON>}`. The webview acknowledges
//!   after applying.
//! - Commands go through [`run_command`], never through a window's queue.
//!   The client never times a command out, so the deadline is set here; a
//!   command that passes it is reported as an unknown outcome and never sent
//!   again.
//!
//! Nothing a webview sends selects anything by name: a command is a
//! [`DaemonCommand`] decoded by serde, and windows are addressed by the label
//! Tauri gives the calling webview (contract §12).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use atrium_client::{
    Client, ClientOptions, CommandError, ConnectionState, Outgoing, StartDaemon, Window,
};
use atrium_host::lifecycle::{self, Paths};
use atrium_host::peer;
use atrium_protocol::wire::{ClientRole, DaemonCommand, ErrorCode};
use serde::Serialize;
use serde_json::Value;
use tokio::task::JoinHandle;

/// Most commands answer at once: they read or change a row.
const COMMAND_DEADLINE: Duration = Duration::from_secs(10);
/// Commands that start a provider process or wait for one to stop.
const SESSION_DEADLINE: Duration = Duration::from_secs(45);
/// Writing a long thread to a file.
const EXPORT_DEADLINE: Duration = Duration::from_secs(120);

/// How long the daemon may take to answer `command`.
pub fn deadline(command: &DaemonCommand) -> Duration {
    match command {
        DaemonCommand::StartThread { .. }
        | DaemonCommand::ResumeThread { .. }
        | DaemonCommand::ArchiveThread { .. }
        | DaemonCommand::ProbeProviders { .. }
        | DaemonCommand::ShutdownDaemon { .. } => SESSION_DEADLINE,
        DaemonCommand::ExportThread { .. } => EXPORT_DEADLINE,
        _ => COMMAND_DEADLINE,
    }
}

/// Why a command has no value, as the webview sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandFailure {
    /// Nothing was sent.
    NotConnected { message: String },
    /// Nothing was sent.
    TooLarge { message: String },
    /// The command may or may not have run. Read the state again before
    /// repeating it.
    UnknownOutcome { message: String },
    /// The daemon's answer.
    Rejected { code: ErrorCode, message: String },
}

impl From<CommandError> for CommandFailure {
    fn from(error: CommandError) -> Self {
        let message = error.to_string();
        match error {
            CommandError::NotConnected => Self::NotConnected { message },
            CommandError::TooLarge { .. } => Self::TooLarge { message },
            CommandError::UnknownOutcome => Self::UnknownOutcome { message },
            CommandError::Rejected { code, message } => Self::Rejected { code, message },
        }
    }
}

/// Sends `command` and waits for its answer, at most `limit`.
pub async fn run_command(
    client: &Client,
    command: DaemonCommand,
    limit: Duration,
) -> Result<Value, CommandFailure> {
    match tokio::time::timeout(limit, client.command(command)).await {
        Ok(answer) => answer.map_err(CommandFailure::from),
        // Dropping the future doesn't take the command back (the client's
        // docs): the daemon may still run it.
        Err(_) => Err(CommandFailure::UnknownOutcome {
            message: format!("the daemon didn't answer within {} s", limit.as_secs()),
        }),
    }
}

/// Where a window's deliveries go. Returns `false` once the receiver is gone.
pub trait Sink: Send + 'static {
    fn deliver(&self, frame: String) -> bool;
}

impl<F: Fn(String) -> bool + Send + 'static> Sink for F {
    fn deliver(&self, frame: String) -> bool {
        self(frame)
    }
}

/// Maximum number of deliveries combined into one batch frame.
pub const MAX_BATCH_DELIVERIES: usize = 512;
/// Maximum byte length of a multi-delivery batch frame.
pub const MAX_BATCH_BYTES: usize = 256 * 1024;

/// One delivery as the webview receives it. `json` is the client's own JSON
/// and is embedded as it is, not encoded again.
pub fn frame(seq: u64, json: &str) -> String {
    format!("{{\"seq\":{seq},\"delivery\":{json}}}")
}

fn entry_bytes(seq: u64, json: &str) -> usize {
    // `{"seq":` (7) + digits + `,"delivery":` (12) + json + `}` (1)
    20 + (seq.checked_ilog10().unwrap_or(0) as usize + 1) + json.len()
}

/// Encodes one or more deliveries into a bridge frame. A single delivery keeps
/// the unbatched `{"seq":...,"delivery":...}` shape; multiple deliveries are
/// wrapped in `{"batch":[...]}`. Each `Outgoing::json` is embedded verbatim.
pub fn batch_frame(items: &[Outgoing]) -> String {
    if let [only] = items {
        return frame(only.seq, &only.json);
    }
    let capacity = items
        .iter()
        .map(|item| entry_bytes(item.seq, &item.json))
        .sum::<usize>()
        + 12
        + items.len().saturating_sub(1);
    let mut out = String::with_capacity(capacity);
    out.push_str("{\"batch\":[");
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let _ = write!(out, "{{\"seq\":{},\"delivery\":{}}}", item.seq, item.json);
    }
    out.push_str("]}");
    out
}

fn try_recv_now(window: &mut Window) -> Poll<Option<Outgoing>> {
    let mut cx = Context::from_waker(Waker::noop());
    pin!(window.recv()).poll(&mut cx)
}

/// Forwards a window's deliveries, in order, until the window is replaced or
/// closed, the client shuts down, or the sink is gone.
///
/// After the first delivery arrives, any deliveries already queued are drained
/// into one batch (up to [`MAX_BATCH_DELIVERIES`] and [`MAX_BATCH_BYTES`]).
/// Never waits for more to arrive, so lone deliveries go out immediately.
pub async fn forward(mut window: Window, sink: impl Sink) {
    let mut carry: Option<Outgoing> = None;
    let mut batch: Vec<Outgoing> = Vec::new();
    loop {
        let first = match carry.take() {
            Some(item) => item,
            None => match window.recv().await {
                Some(item) => item,
                None => break,
            },
        };
        let mut total_bytes = 12 + entry_bytes(first.seq, &first.json);
        batch.push(first);
        let mut closed = false;

        if total_bytes < MAX_BATCH_BYTES {
            while batch.len() < MAX_BATCH_DELIVERIES {
                match try_recv_now(&mut window) {
                    Poll::Ready(Some(next)) => {
                        let next_bytes = 1 + entry_bytes(next.seq, &next.json);
                        if total_bytes + next_bytes > MAX_BATCH_BYTES {
                            carry = Some(next);
                            break;
                        }
                        total_bytes += next_bytes;
                        batch.push(next);
                    }
                    Poll::Ready(None) => {
                        closed = true;
                        break;
                    }
                    Poll::Pending => break,
                }
            }
        }

        let payload = batch_frame(&batch);
        batch.clear();
        if !sink.deliver(payload) || closed {
            break;
        }
    }
}

/// The process's client and the forwarding task of each attached webview.
pub struct Bridge {
    client: Client,
    /// The runtime the client runs on. Webviews attach from synchronous
    /// commands, which have no runtime of their own.
    runtime: tokio::runtime::Handle,
    forwarders: Mutex<HashMap<String, JoinHandle<()>>>,
}

impl Bridge {
    /// Must be called on a Tokio runtime.
    pub fn start(options: ClientOptions) -> Self {
        Self {
            client: Client::start(options),
            runtime: tokio::runtime::Handle::current(),
            forwarders: Mutex::new(HashMap::new()),
        }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Attaches the webview `label`: from now on `sink` receives its
    /// deliveries. Attaching again (the webview reloaded) starts over: the
    /// client replaces the window and the old task ends.
    pub fn attach(&self, label: &str, sink: impl Sink) {
        let window = self.client.open_window(label);
        let task = self.runtime.spawn(forward(window, sink));
        let previous = self
            .forwarders
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(label.into(), task);
        if let Some(previous) = previous {
            previous.abort();
        }
    }

    /// The webview is gone: its client window is closed and its queue freed.
    pub fn detach(&self, label: &str) {
        let task = self
            .forwarders
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(label);
        if let Some(task) = task {
            // Aborting drops the `Window`, which closes it in the client.
            task.abort();
        }
    }

    pub fn attached(&self, label: &str) -> bool {
        self.forwarders
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(label)
    }

    pub fn connection(&self) -> ConnectionState {
        self.client.connection().borrow().clone()
    }
}

/// `atriumd` ships next to the app's executable. A debug build may point
/// somewhere else with `ATRIUM_DAEMON`.
pub fn daemon_executable() -> io::Result<PathBuf> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("ATRIUM_DAEMON").filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let app = std::env::current_exe()?;
    let folder = app
        .parent()
        .ok_or_else(|| io::Error::other("the app's executable has no folder"))?;
    Ok(folder.join("atriumd"))
}

/// The hook the client runs when nothing listens on the socket.
pub fn start_daemon_hook(paths: &Paths) -> StartDaemon {
    let log = paths.logs_dir.join("atriumd.log");
    let socket = paths.socket.clone();
    Arc::new(move || {
        let executable = daemon_executable()?;
        if !executable.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("atriumd was not found at {}", executable.display()),
            ));
        }
        lifecycle::spawn_detached_wait(
            &executable,
            &[],
            &log,
            &socket,
            std::time::Duration::from_millis(500),
        )
        .map(|_| ())
    })
}

/// A development build tells the daemon which executable is the app
/// (contract §4): its own identity, in a file only the user can read. A
/// release daemon has the identity compiled in and ignores the file.
pub fn write_dev_identity(paths: &Paths) -> io::Result<()> {
    lifecycle::secure_dir(&paths.data_dir).map_err(io::Error::other)?;
    let identity = peer::current_app_identity()?;
    let bytes = serde_json::to_vec(&identity).map_err(io::Error::other)?;
    crate::shell::write_private(&paths.app_identity, &bytes)
}

pub fn client_options(paths: &Paths) -> ClientOptions {
    ClientOptions {
        socket: paths.socket.clone(),
        name: format!("atrium-app {}", env!("CARGO_PKG_VERSION")),
        role: ClientRole::App,
        start_daemon: Some(start_daemon_hook(paths)),
    }
}

/// The folder for the app's own files (settings, session), next to the
/// daemon's data and as private.
pub fn app_dir(paths: &Paths) -> io::Result<PathBuf> {
    lifecycle::secure_dir(&paths.data_dir).map_err(io::Error::other)?;
    let folder: &Path = &paths.data_dir;
    let folder = folder.join("app");
    lifecycle::secure_dir(&folder).map_err(io::Error::other)?;
    Ok(folder)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Instant;

    use atrium_protocol::agent::Provider;
    use atrium_protocol::thread::{Location, ThreadId, ThreadState, ThreadSummary};
    use atrium_protocol::wire::{ClientMessage, CommandResult, ServerMessage};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixListener;
    use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

    use super::*;

    type PeerSenders = Arc<Mutex<Vec<tokio::sync::mpsc::UnboundedSender<Vec<ServerMessage>>>>>;

    /// A daemon that speaks just enough of the wire: `Hello`, `SubscribeInbox`
    /// and commands, answered by `answer` (or never, when it returns `None`).
    struct Scripted {
        folder: tempfile::TempDir,
        peers: PeerSenders,
    }

    async fn read_message(stream: &mut OwnedReadHalf) -> Option<ClientMessage> {
        let mut header = [0u8; 4];
        stream.read_exact(&mut header).await.ok()?;
        let length = u32::from_be_bytes(header) as usize;
        let mut body = vec![0u8; length];
        stream.read_exact(&mut body).await.ok()?;
        serde_json::from_slice(&body[1..]).ok()
    }

    async fn write_message(stream: &mut OwnedWriteHalf, message: &ServerMessage) -> bool {
        let payload = serde_json::to_vec(message).unwrap();
        let mut frame = Vec::with_capacity(payload.len() + 5);
        frame.extend_from_slice(&(payload.len() as u32 + 1).to_be_bytes());
        frame.push(0);
        frame.extend_from_slice(&payload);
        stream.write_all(&frame).await.is_ok()
    }

    impl Scripted {
        fn start(answer: fn(&DaemonCommand) -> Option<CommandResult>) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let folder = tempfile::tempdir().unwrap();
            std::fs::set_permissions(folder.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            let listener = UnixListener::bind(folder.path().join("atriumd.sock")).unwrap();
            let peers: PeerSenders = Arc::new(Mutex::new(Vec::new()));
            let peers_accept = peers.clone();
            tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let peers_conn = peers_accept.clone();
                    tokio::spawn(async move {
                        let (mut reader, mut writer) = stream.into_split();
                        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                        peers_conn
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push(tx.clone());
                        let writer_task = tokio::spawn(async move {
                            while let Some(batch) = rx.recv().await {
                                for message in batch {
                                    if !write_message(&mut writer, &message).await {
                                        return;
                                    }
                                }
                            }
                        });
                        while let Some(message) = read_message(&mut reader).await {
                            let reply = match message {
                                ClientMessage::Hello { role, .. } => Some(ServerMessage::Welcome {
                                    protocol_version: atrium_protocol::PROTOCOL_VERSION,
                                    daemon_version: "test".into(),
                                    daemon_pid: 1,
                                    role,
                                }),
                                ClientMessage::SubscribeInbox { request_id } => {
                                    Some(ServerMessage::Response {
                                        request_id,
                                        result: CommandResult::Ok {
                                            value: serde_json::json!({"threads": [], "inbox": []}),
                                        },
                                    })
                                }
                                ClientMessage::Command {
                                    request_id,
                                    command,
                                } => answer(&command)
                                    .map(|result| ServerMessage::Response { request_id, result }),
                                _ => None,
                            };
                            if let Some(reply) = reply
                                && tx.send(vec![reply]).is_err()
                            {
                                break;
                            }
                        }
                        drop(tx);
                        writer_task.abort();
                    });
                }
            });
            Self { folder, peers }
        }

        fn push(&self, messages: Vec<ServerMessage>) {
            let mut peers = self.peers.lock().unwrap_or_else(PoisonError::into_inner);
            peers.retain(|tx| tx.send(messages.clone()).is_ok());
        }

        fn options(&self) -> ClientOptions {
            ClientOptions {
                socket: self.folder.path().join("atriumd.sock"),
                name: "bridge-test".into(),
                role: ClientRole::Dev,
                start_daemon: None,
            }
        }
    }

    async fn connected(bridge: &Bridge) {
        let mut connection = bridge.client().connection();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if matches!(*connection.borrow(), ConnectionState::Connected { .. }) {
                    return;
                }
                connection.changed().await.unwrap();
            }
        })
        .await
        .expect("the client connects");
    }

    async fn client_connected(client: &Client) {
        let mut connection = client.connection();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if matches!(*connection.borrow(), ConnectionState::Connected { .. }) {
                    return;
                }
                connection.changed().await.unwrap();
            }
        })
        .await
        .expect("the client connects");
    }

    fn thread_updated(index: usize, title: String) -> ServerMessage {
        ServerMessage::ThreadUpdated {
            summary: ThreadSummary {
                thread_id: ThreadId(format!("t-{index}")),
                title,
                provider: Provider::Claude,
                model: None,
                location: Location::Home,
                state: ThreadState::Running,
                enforcement: None,
                incarnation: 1,
                native_session_id: None,
                last_event: None,
                unread: false,
                pending_approvals: 0,
                pending_questions: 0,
                attention_since_ms: None,
                attention_detail: None,
                created_at_ms: 1_700_000_000_000,
                updated_at_ms: 1_700_000_000_000 + index as u64,
            },
        }
    }

    fn sink() -> (impl Sink, mpsc::Receiver<String>) {
        let (sender, receiver) = mpsc::channel();
        (move |frame: String| sender.send(frame).is_ok(), receiver)
    }

    fn next_raw(receiver: &mpsc::Receiver<String>) -> String {
        receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("a delivery arrives")
    }

    fn next(receiver: &mpsc::Receiver<String>) -> Value {
        let frame = next_raw(receiver);
        serde_json::from_str(&frame).expect("a frame is JSON")
    }

    fn unpack(frame: Value) -> Vec<(u64, Value)> {
        if let Some(items) = frame.get("batch").and_then(Value::as_array) {
            items
                .iter()
                .map(|item| (item["seq"].as_u64().unwrap(), item["delivery"].clone()))
                .collect()
        } else {
            vec![(frame["seq"].as_u64().unwrap(), frame["delivery"].clone())]
        }
    }

    #[test]
    fn a_frame_embeds_the_delivery_as_it_is() {
        let json = r#"{"type":"connection","state":{"type":"closed"}}"#;
        let parsed: Value = serde_json::from_str(&frame(7, json)).unwrap();
        assert_eq!(parsed["seq"], 7);
        assert_eq!(parsed["delivery"]["state"]["type"], "closed");
    }

    #[test]
    fn batch_frame_packs_multiple_deliveries_and_preserves_lone_frame() {
        let lone = Outgoing {
            seq: 7,
            json: Arc::from(r#"{"type":"connection","state":{"type":"closed"}}"#),
        };
        assert_eq!(
            batch_frame(std::slice::from_ref(&lone)),
            frame(7, &lone.json)
        );

        let items = vec![
            Outgoing {
                seq: 9,
                json: Arc::from(r#"{"type":"thread_removed","thread_id":"t1"}"#),
            },
            Outgoing {
                seq: 10,
                json: Arc::from(r#"{"type":"inbox_updated","items":[]}"#),
            },
        ];
        let packed = batch_frame(&items);
        let expected_len = 12
            + entry_bytes(items[0].seq, &items[0].json)
            + 1
            + entry_bytes(items[1].seq, &items[1].json);
        assert_eq!(packed.len(), expected_len);
        let parsed: Value = serde_json::from_str(&packed).unwrap();
        let batch = parsed["batch"].as_array().unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0]["seq"], 9);
        assert_eq!(batch[0]["delivery"]["thread_id"], "t1");
        assert_eq!(batch[1]["seq"], 10);
        assert_eq!(batch[1]["delivery"]["type"], "inbox_updated");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn deliveries_arrive_in_order_with_rising_numbers() {
        let daemon = Scripted::start(|_| None);
        let bridge = Bridge::start(daemon.options());
        let (sink, receiver) = sink();
        bridge.attach("main", sink);
        let handle = bridge.client().window("main").expect("the window is open");
        handle.show_inbox(true);
        let mut last = 0;
        let mut kinds = Vec::new();
        while !kinds.contains(&"inbox".to_string()) {
            for (seq, delivery) in unpack(next(&receiver)) {
                assert_eq!(seq, last + 1, "numbers rise by one");
                last = seq;
                handle.ack(seq);
                kinds.push(delivery["type"].as_str().unwrap().to_string());
            }
        }
        assert_eq!(kinds.first().map(String::as_str), Some("connection"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn attaching_again_replaces_the_window_and_ends_the_old_task() {
        let daemon = Scripted::start(|_| None);
        let bridge = Bridge::start(daemon.options());
        connected(&bridge).await;
        let (first, first_receiver) = sink();
        bridge.attach("main", first);
        assert_eq!(unpack(next(&first_receiver))[0].0, 1);
        let (second, second_receiver) = sink();
        bridge.attach("main", second);
        assert_eq!(
            unpack(next(&second_receiver))[0].0,
            1,
            "a reloaded webview starts over"
        );
        // The first sink's sender is dropped with its task: nothing more
        // arrives and the channel closes.
        loop {
            match first_receiver.recv_timeout(Duration::from_secs(5)) {
                Ok(_) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => panic!("the old task never ended"),
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn detaching_closes_the_window_in_the_client() {
        let daemon = Scripted::start(|_| None);
        let bridge = Bridge::start(daemon.options());
        let (sink, receiver) = sink();
        bridge.attach("view-1", sink);
        next(&receiver);
        assert!(bridge.attached("view-1"));
        bridge.detach("view-1");
        assert!(!bridge.attached("view-1"));
        tokio::time::timeout(Duration::from_secs(5), async {
            while bridge.client().window("view-1").is_some() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the client forgets the window");
        bridge.detach("view-1");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_sink_that_is_gone_ends_the_forwarding() {
        let daemon = Scripted::start(|_| None);
        let bridge = Bridge::start(daemon.options());
        bridge.attach("main", |_frame: String| false);
        tokio::time::timeout(Duration::from_secs(5), async {
            while bridge.client().window("main").is_some() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the window closes with its task");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_command_returns_the_daemons_value_or_its_refusal() {
        let daemon = Scripted::start(|command| {
            Some(match command {
                DaemonCommand::ListThreads => CommandResult::Ok {
                    value: serde_json::json!([]),
                },
                _ => CommandResult::Err {
                    code: ErrorCode::UnknownThread,
                    message: "no such thread".into(),
                },
            })
        });
        let bridge = Bridge::start(daemon.options());
        connected(&bridge).await;
        let value = run_command(
            bridge.client(),
            DaemonCommand::ListThreads,
            COMMAND_DEADLINE,
        )
        .await;
        assert_eq!(value, Ok(serde_json::json!([])));
        let refused = run_command(
            bridge.client(),
            DaemonCommand::MarkRead {
                thread_id: ThreadId("t1".into()),
            },
            COMMAND_DEADLINE,
        )
        .await;
        assert_eq!(
            refused,
            Err(CommandFailure::Rejected {
                code: ErrorCode::UnknownThread,
                message: "no such thread".into()
            })
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_command_past_its_deadline_is_an_unknown_outcome() {
        let daemon = Scripted::start(|_| None);
        let bridge = Bridge::start(daemon.options());
        connected(&bridge).await;
        let outcome = run_command(
            bridge.client(),
            DaemonCommand::ListThreads,
            Duration::from_millis(100),
        )
        .await;
        assert!(
            matches!(outcome, Err(CommandFailure::UnknownOutcome { .. })),
            "{outcome:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_command_with_no_connection_is_not_sent() {
        let folder = tempfile::tempdir().unwrap();
        let bridge = Bridge::start(ClientOptions {
            socket: folder.path().join("nothing-here.sock"),
            name: "bridge-test".into(),
            role: ClientRole::Dev,
            start_daemon: None,
        });
        let outcome = run_command(
            bridge.client(),
            DaemonCommand::ListThreads,
            COMMAND_DEADLINE,
        )
        .await;
        assert!(
            matches!(outcome, Err(CommandFailure::NotConnected { .. })),
            "{outcome:?}"
        );
    }

    #[test]
    fn failures_are_tagged_for_the_webview() {
        let failure = CommandFailure::from(CommandError::Rejected {
            code: ErrorCode::Expired,
            message: "the request expired".into(),
        });
        assert_eq!(
            serde_json::to_value(&failure).unwrap(),
            serde_json::json!({"kind": "rejected", "code": "expired", "message": "the request expired"})
        );
        let unknown =
            serde_json::to_value(CommandFailure::from(CommandError::UnknownOutcome)).unwrap();
        assert_eq!(unknown["kind"], "unknown_outcome");
    }

    #[test]
    fn slow_commands_get_longer_deadlines() {
        assert_eq!(deadline(&DaemonCommand::ListThreads), COMMAND_DEADLINE);
        assert_eq!(
            deadline(&DaemonCommand::ShutdownDaemon { force: false }),
            SESSION_DEADLINE
        );
        let export = DaemonCommand::ExportThread {
            thread_id: ThreadId("t1".into()),
            format: atrium_protocol::wire::ExportFormat::Markdown,
            path: "/tmp/x.md".into(),
        };
        assert_eq!(deadline(&export), EXPORT_DEADLINE);
    }

    fn list_threads_ok(command: &DaemonCommand) -> Option<CommandResult> {
        match command {
            DaemonCommand::ListThreads => Some(CommandResult::Ok {
                value: serde_json::json!([]),
            }),
            _ => None,
        }
    }

    async fn flush_barrier(client: &Client) {
        assert_eq!(
            run_command(client, DaemonCommand::ListThreads, COMMAND_DEADLINE).await,
            Ok(serde_json::json!([]))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn batching_keeps_order_and_sequence_numbers() {
        let daemon = Scripted::start(list_threads_ok);
        let client = Client::start(daemon.options());
        client_connected(&client).await;

        let window = client.open_window("main");
        let handle = window.handle();
        handle.show_inbox(true);

        let messages: Vec<ServerMessage> = (0..1_200)
            .map(|i| thread_updated(i, format!("msg-{i}")))
            .collect();
        daemon.push(messages);
        flush_barrier(&client).await;

        let (sink, receiver) = sink();
        let task = tokio::spawn(forward(window, sink));

        let mut frame_count = 0usize;
        let mut last_seq = 0u64;
        let mut titles = Vec::new();
        while titles.len() < 1_200 {
            let parsed = next(&receiver);
            frame_count += 1;
            for (seq, delivery) in unpack(parsed) {
                assert_eq!(seq, last_seq + 1, "sequence numbers must rise by 1");
                last_seq = seq;
                handle.ack(seq);
                if delivery["type"] == "thread_updated" {
                    titles.push(delivery["summary"]["title"].as_str().unwrap().to_string());
                }
            }
        }

        let expected: Vec<String> = (0..1_200).map(|i| format!("msg-{i}")).collect();
        assert_eq!(titles, expected);
        assert!(
            frame_count <= 4,
            "1,200 queued deliveries should coalesce into <= 4 frames, got {frame_count}"
        );

        drop(handle);
        client.shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("forward ends");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_lone_delivery_is_not_delayed() {
        let daemon = Scripted::start(list_threads_ok);
        let bridge = Bridge::start(daemon.options());
        connected(&bridge).await;

        let (sink, receiver) = sink();
        bridge.attach("main", sink);
        let handle = bridge.client().window("main").expect("window is open");
        handle.show_inbox(true);

        // Drain initial `connection` and `inbox` deliveries.
        let mut last_seq = 0;
        let mut saw_inbox = false;
        while !saw_inbox {
            for (seq, delivery) in unpack(next(&receiver)) {
                last_seq = seq;
                handle.ack(seq);
                if delivery["type"] == "inbox" {
                    saw_inbox = true;
                }
            }
        }

        let sent_at = Instant::now();
        daemon.push(vec![thread_updated(1, "lone".into())]);
        let raw = next_raw(&receiver);
        let elapsed = sent_at.elapsed();

        let parsed: Value = serde_json::from_str(&raw).unwrap();
        assert!(
            parsed.get("batch").is_none(),
            "a lone delivery must use the unbatched frame shape: {raw}"
        );
        assert_eq!(parsed["seq"], last_seq + 1);
        assert_eq!(parsed["delivery"]["summary"]["title"], "lone");
        assert!(
            elapsed < Duration::from_millis(50),
            "lone delivery was delayed by {elapsed:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn batch_bounds_are_respected_for_count_and_bytes() {
        let daemon = Scripted::start(list_threads_ok);
        let client = Client::start(daemon.options());
        client_connected(&client).await;

        // Part A: count bound (MAX_BATCH_DELIVERIES = 512).
        let window = client.open_window("count");
        let handle = window.handle();
        handle.show_inbox(true);
        let count_messages: Vec<ServerMessage> = (0..(MAX_BATCH_DELIVERIES + 100))
            .map(|i| thread_updated(i, format!("c-{i}")))
            .collect();
        daemon.push(count_messages);
        flush_barrier(&client).await;

        let (sink_a, receiver_a) = sink();
        let task_a = tokio::spawn(forward(window, sink_a));
        let mut titles_a = Vec::new();
        let mut last_seq_a = 0;
        while titles_a.len() < MAX_BATCH_DELIVERIES + 100 {
            let raw = next_raw(&receiver_a);
            let parsed: Value = serde_json::from_str(&raw).unwrap();
            let items = unpack(parsed);
            assert!(
                items.len() <= MAX_BATCH_DELIVERIES,
                "batch exceeded MAX_BATCH_DELIVERIES: {}",
                items.len()
            );
            if items.len() > 1 {
                assert!(
                    raw.len() <= MAX_BATCH_BYTES,
                    "batch frame exceeded MAX_BATCH_BYTES: {}",
                    raw.len()
                );
            }
            for (seq, delivery) in items {
                assert_eq!(seq, last_seq_a + 1);
                last_seq_a = seq;
                handle.ack(seq);
                if delivery["type"] == "thread_updated" {
                    titles_a.push(delivery["summary"]["title"].as_str().unwrap().to_string());
                }
            }
        }
        assert_eq!(titles_a.len(), MAX_BATCH_DELIVERIES + 100);
        drop(handle);
        drop(receiver_a);
        task_a.abort();
        let _ = tokio::time::timeout(Duration::from_secs(5), task_a)
            .await
            .expect("forward ends");

        // Part B: byte bound (MAX_BATCH_BYTES = 256 KiB) and oversized lone delivery.
        let window_b = client.open_window("bytes");
        let handle_b = window_b.handle();
        handle_b.show_inbox(true);
        let medium = "m".repeat(30 * 1024);
        let oversized = "x".repeat(300 * 1024);
        let mut byte_messages: Vec<ServerMessage> = (0..20)
            .map(|i| thread_updated(i, format!("{i}:{medium}")))
            .collect();
        byte_messages.push(thread_updated(20, format!("20:{oversized}")));
        byte_messages.push(thread_updated(21, "21:tail".into()));
        daemon.push(byte_messages);
        flush_barrier(&client).await;

        let (sink_b, receiver_b) = sink();
        let task_b = tokio::spawn(forward(window_b, sink_b));
        let mut seen_b = Vec::new();
        let mut last_seq_b = 0;
        let mut batch_frames_b = 0usize;
        while seen_b.len() < 22 {
            let raw = next_raw(&receiver_b);
            let parsed: Value = serde_json::from_str(&raw).unwrap();
            let is_batch = parsed.get("batch").is_some();
            let items = unpack(parsed);
            if is_batch {
                batch_frames_b += 1;
                assert!(
                    raw.len() <= MAX_BATCH_BYTES,
                    "multi-delivery batch frame exceeded MAX_BATCH_BYTES: {}",
                    raw.len()
                );
            }
            for (seq, delivery) in items {
                assert_eq!(seq, last_seq_b + 1);
                last_seq_b = seq;
                handle_b.ack(seq);
                if delivery["type"] == "thread_updated" {
                    seen_b.push(delivery["summary"]["title"].as_str().unwrap().to_string());
                }
            }
        }
        assert!(
            batch_frames_b >= 2,
            "expected multiple byte-bounded batches"
        );
        assert_eq!(seen_b[20].len(), 3 + oversized.len());
        assert_eq!(seen_b[21], "21:tail");

        drop(handle_b);
        client.shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(5), task_b)
            .await
            .expect("forward ends");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_gone_sink_ends_forwarding_mid_stream() {
        let daemon = Scripted::start(list_threads_ok);
        let client = Client::start(daemon.options());
        client_connected(&client).await;

        let window = client.open_window("main");
        let handle = window.handle();
        handle.show_inbox(true);
        let messages: Vec<ServerMessage> = (0..1_200)
            .map(|i| thread_updated(i, format!("g-{i}")))
            .collect();
        daemon.push(messages);
        flush_barrier(&client).await;
        drop(handle);

        let delivered = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let delivered_sink = delivered.clone();
        let task = tokio::spawn(forward(window, move |_frame: String| {
            // Accept the first batch, then report the receiver is gone.
            delivered_sink.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0
        }));

        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("forward ends once sink returns false")
            .expect("forward task did not panic");
        assert!(
            client.window("main").is_none(),
            "window closes when task drops it"
        );
        assert_eq!(
            delivered.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "second delivery fails and stops forwarding immediately"
        );
        client.shutdown();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn micro_measurement_frames_per_10000_deliveries() {
        let daemon = Scripted::start(list_threads_ok);
        let client = Client::start(daemon.options());
        client_connected(&client).await;

        let window = client.open_window("bench");
        let handle = window.handle();
        handle.show_inbox(true);

        const TOTAL: usize = 10_000;
        let messages: Vec<ServerMessage> = (0..TOTAL)
            .map(|i| thread_updated(i % 10, format!("tok-{i}")))
            .collect();
        daemon.push(messages);
        flush_barrier(&client).await;

        let (sink, receiver) = sink();
        let started = Instant::now();
        let task = tokio::spawn(forward(window, sink));

        let mut frames = 0usize;
        let mut updates = 0usize;
        let mut max_batch_len = 0usize;
        let mut max_frame_bytes = 0usize;
        let mut last_seq = 0u64;
        while updates < TOTAL {
            let raw = next_raw(&receiver);
            frames += 1;
            max_frame_bytes = max_frame_bytes.max(raw.len());
            let parsed: Value = serde_json::from_str(&raw).unwrap();
            let items = unpack(parsed);
            max_batch_len = max_batch_len.max(items.len());
            for (seq, delivery) in items {
                assert_eq!(seq, last_seq + 1);
                last_seq = seq;
                handle.ack(seq);
                if delivery["type"] == "thread_updated" {
                    updates += 1;
                }
            }
        }
        let elapsed = started.elapsed();
        eprintln!(
            "bridge batching micro-measurement ({TOTAL} queued deliveries): before={TOTAL} frames, after={frames} frames, max_batch_len={max_batch_len}, max_frame_bytes={max_frame_bytes}, elapsed={elapsed:?}"
        );
        assert_eq!(updates, TOTAL);
        assert!(
            frames < TOTAL / 100,
            "expected >=100x fewer frames, got {frames}"
        );

        drop(handle);
        client.shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("forward ends");
    }
}
