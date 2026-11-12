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
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use atrium_client::{Client, ClientOptions, CommandError, ConnectionState, StartDaemon, Window};
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

/// One delivery as the webview receives it. `json` is the client's own JSON
/// and is embedded as it is, not encoded again.
pub fn frame(seq: u64, json: &str) -> String {
    format!("{{\"seq\":{seq},\"delivery\":{json}}}")
}

/// Forwards a window's deliveries, in order, until the window is replaced or
/// closed, the client shuts down, or the sink is gone.
pub async fn forward(mut window: Window, sink: impl Sink) {
    while let Some(outgoing) = window.recv().await {
        if !sink.deliver(frame(outgoing.seq, &outgoing.json)) {
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
    let logs_dir = paths.logs_dir.clone();
    Arc::new(move || {
        let executable = daemon_executable()?;
        if !executable.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("atriumd was not found at {}", executable.display()),
            ));
        }
        lifecycle::secure_dir(&logs_dir).map_err(io::Error::other)?;
        lifecycle::spawn_detached(&executable, &[], &log).map(|_| ())
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

    use atrium_protocol::thread::ThreadId;
    use atrium_protocol::wire::{ClientMessage, CommandResult, ServerMessage};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{UnixListener, UnixStream};

    use super::*;

    /// A daemon that speaks just enough of the wire: `Hello`, `SubscribeInbox`
    /// and commands, answered by `answer` (or never, when it returns `None`).
    struct Scripted {
        folder: tempfile::TempDir,
    }

    async fn read_message(stream: &mut UnixStream) -> Option<ClientMessage> {
        let mut header = [0u8; 4];
        stream.read_exact(&mut header).await.ok()?;
        let length = u32::from_be_bytes(header) as usize;
        let mut body = vec![0u8; length];
        stream.read_exact(&mut body).await.ok()?;
        serde_json::from_slice(&body[1..]).ok()
    }

    async fn write_message(stream: &mut UnixStream, message: &ServerMessage) -> bool {
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
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    tokio::spawn(async move {
                        while let Some(message) = read_message(&mut stream).await {
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
                                && !write_message(&mut stream, &reply).await
                            {
                                break;
                            }
                        }
                    });
                }
            });
            Self { folder }
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

    fn sink() -> (impl Sink, mpsc::Receiver<String>) {
        let (sender, receiver) = mpsc::channel();
        (move |frame: String| sender.send(frame).is_ok(), receiver)
    }

    fn next(receiver: &mpsc::Receiver<String>) -> Value {
        let frame = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("a delivery arrives");
        serde_json::from_str(&frame).expect("a frame is JSON")
    }

    #[test]
    fn a_frame_embeds_the_delivery_as_it_is() {
        let json = r#"{"type":"connection","state":{"type":"closed"}}"#;
        let parsed: Value = serde_json::from_str(&frame(7, json)).unwrap();
        assert_eq!(parsed["seq"], 7);
        assert_eq!(parsed["delivery"]["state"]["type"], "closed");
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
            let frame = next(&receiver);
            let seq = frame["seq"].as_u64().unwrap();
            assert_eq!(seq, last + 1, "numbers rise by one");
            last = seq;
            handle.ack(seq);
            kinds.push(frame["delivery"]["type"].as_str().unwrap().to_string());
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
        assert_eq!(next(&first_receiver)["seq"], 1);
        let (second, second_receiver) = sink();
        bridge.attach("main", second);
        assert_eq!(
            next(&second_receiver)["seq"],
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
}
