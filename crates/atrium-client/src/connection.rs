//! The socket: connecting, the handshake, and the read and write loops.
//!
//! One task ([`run`]) owns the connection for the client's whole life. While
//! connected it has two helpers, one reading frames into the state and one
//! writing queued frames, so reading never waits on writing and neither
//! waits on a window.
//!
//! What ends a connection decides what happens next:
//! - the socket closed or stalled: reconnect at once, then with backoff;
//! - nothing is listening: call the start hook, keep trying with backoff;
//! - the daemon answered `Incompatible`, the socket isn't safe, or the
//!   daemon broke the protocol: stop, and wait for [`crate::Client::reconnect`].
//!   None of these starts a daemon.

use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use atrium_protocol::PROTOCOL_VERSION;
use atrium_protocol::wire::{ClientMessage, ServerMessage};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;

use crate::codec::{self, Frame};
use crate::{ClientOptions, ConnectionState, Shared};

pub(crate) enum Control {
    Reconnect,
    Shutdown,
}

/// For connecting, each handshake step and each write. Reads have no
/// deadline: an idle connection is normal.
const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// The start hook runs on the first failed attempt and every this many
/// after it.
const START_EVERY: u32 = 20;

/// Why a connection (or an attempt at one) ended.
enum Ended {
    /// The client is shutting down.
    Shutdown,
    /// Asked to try again now.
    Retry,
    /// It was up and dropped.
    Lost,
    /// Nothing is listening.
    Unreachable,
    /// Not to be retried without being asked.
    Refused(ConnectionState),
}

fn failed(message: impl Into<String>) -> Ended {
    Ended::Refused(ConnectionState::Failed {
        message: message.into(),
    })
}

struct Connection {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    daemon_version: String,
    daemon_pid: u32,
}

pub(crate) async fn run(
    shared: Arc<Shared>,
    options: ClientOptions,
    mut control: mpsc::UnboundedReceiver<Control>,
) {
    let mut attempt: u32 = 0;
    loop {
        attempt = attempt.saturating_add(1);
        shared.with_connection(|state| {
            state.set_connection(ConnectionState::Connecting { attempt });
        });
        let ended = tokio::select! {
            biased;
            message = control.recv() => match message {
                Some(Control::Reconnect) => Ended::Retry,
                Some(Control::Shutdown) | None => Ended::Shutdown,
            },
            connection = connect(&options) => match connection {
                Ok(connection) => {
                    attempt = 0;
                    session(&shared, connection, &mut control).await
                }
                Err(ended) => ended,
            },
        };
        match ended {
            Ended::Shutdown => break,
            Ended::Retry => attempt = 0,
            Ended::Lost => {
                shared.with_connection(|state| {
                    state.disconnected(ConnectionState::Connecting { attempt: 1 });
                });
            }
            Ended::Unreachable => {
                if attempt % START_EVERY == 1
                    && let Some(start) = options.start_daemon.clone()
                {
                    // Its result shows in whether the next attempts connect.
                    let _ = tokio::task::spawn_blocking(move || start()).await;
                }
                let delay = Duration::from_millis((25u64 << attempt.min(6)).min(1000));
                tokio::select! {
                    message = control.recv() => match message {
                        Some(Control::Reconnect) => attempt = 0,
                        Some(Control::Shutdown) | None => break,
                    },
                    () = tokio::time::sleep(delay) => {}
                }
            }
            Ended::Refused(state) => {
                shared.with_connection(|s| s.disconnected(state));
                match control.recv().await {
                    Some(Control::Reconnect) => attempt = 0,
                    Some(Control::Shutdown) | None => break,
                }
            }
        }
    }
    shared.with_connection(|state| state.close());
}

/// Connects and shakes hands.
async fn connect(options: &ClientOptions) -> Result<Connection, Ended> {
    match verify_socket(&options.socket) {
        Ok(()) => {}
        Err(SocketError::Missing) => return Err(Ended::Unreachable),
        Err(SocketError::Unsafe(why)) => {
            return Err(failed(format!(
                "the daemon's socket at {} isn't safe to use: {why}",
                options.socket.display()
            )));
        }
    }
    let stream = within(UnixStream::connect(&options.socket))
        .await
        .map_err(|_| Ended::Unreachable)?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::with_capacity(64 * 1024, reader);

    let hello = codec::encode(&ClientMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
        client_name: options.name.clone(),
        role: options.role.clone(),
    })
    .map_err(|_| failed("the client's name is too long for a frame"))?;
    within(writer.write_all(&hello))
        .await
        .map_err(|_| Ended::Unreachable)?;

    let answer = match within(codec::read_control(&mut reader)).await {
        Ok(payload) => payload,
        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
            return Err(failed(format!("the daemon's handshake is invalid: {e}")));
        }
        // It closed or stalled before answering: it may be starting up or
        // shutting down.
        Err(_) => return Err(Ended::Unreachable),
    };
    match serde_json::from_slice::<ServerMessage>(&answer) {
        Ok(ServerMessage::Welcome {
            protocol_version,
            daemon_version,
            daemon_pid,
            role,
        }) => {
            if protocol_version != PROTOCOL_VERSION || role != options.role {
                return Err(failed(
                    "the daemon welcomed the client with another protocol version or role",
                ));
            }
            Ok(Connection {
                reader,
                writer,
                daemon_version,
                daemon_pid,
            })
        }
        Ok(ServerMessage::Incompatible {
            protocol_version,
            message,
            daemon_version,
            daemon_pid,
            running_turns,
        }) => Err(Ended::Refused(ConnectionState::Incompatible {
            protocol_version,
            message,
            daemon_version,
            daemon_pid,
            running_turns,
        })),
        Ok(_) => Err(failed("the daemon answered `Hello` with something else")),
        Err(e) => Err(failed(format!("the daemon's handshake is invalid: {e}"))),
    }
}

async fn within<T>(io: impl Future<Output = io::Result<T>>) -> io::Result<T> {
    tokio::time::timeout(IO_TIMEOUT, io)
        .await
        .unwrap_or_else(|_| Err(io::ErrorKind::TimedOut.into()))
}

/// Runs a connection until it ends.
async fn session(
    shared: &Arc<Shared>,
    connection: Connection,
    control: &mut mpsc::UnboundedReceiver<Control>,
) -> Ended {
    let Connection {
        reader,
        writer,
        daemon_version,
        daemon_pid,
    } = connection;
    let (frames, queue) = mpsc::unbounded_channel();
    shared.with_connection(|state| state.connected(frames, daemon_version, daemon_pid));
    let mut read = tokio::spawn(read_loop(reader, shared.clone()));
    let mut write = tokio::spawn(write_loop(writer, queue));
    let ended = loop {
        tokio::select! {
            // A task that panicked counts as a lost connection.
            ended = &mut read => break ended.unwrap_or(Ended::Lost),
            _ = &mut write => break Ended::Lost,
            message = control.recv() => match message {
                // Already connected.
                Some(Control::Reconnect) => {}
                Some(Control::Shutdown) | None => break Ended::Shutdown,
            },
        }
    };
    read.abort();
    write.abort();
    ended
}

/// Reads frames into the state. The only reader of the socket, and never
/// cancelled between frames except with the connection.
async fn read_loop(mut reader: BufReader<OwnedReadHalf>, shared: Arc<Shared>) -> Ended {
    loop {
        let payload = match codec::read_control(&mut reader).await {
            Ok(payload) => payload,
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                return failed(format!("the daemon sent an invalid frame: {e}"));
            }
            Err(_) => return Ended::Lost,
        };
        let Ok(json) = String::from_utf8(payload) else {
            return failed("the daemon sent a frame that isn't UTF-8");
        };
        // Parsed outside the lock: a large event takes a while.
        let message = match serde_json::from_str::<ServerMessage>(&json) {
            Ok(message) => message,
            Err(e) => {
                return failed(format!(
                    "the daemon sent a message the client can't read: {e}"
                ));
            }
        };
        if let Err(violation) = shared.with(|state| state.on_message(message, Arc::from(json))) {
            return failed(format!("the daemon broke the protocol: {violation}"));
        }
    }
}

/// Writes queued frames. Ends when the queue closes (the state dropped the
/// connection) or a write fails or stalls.
async fn write_loop(mut writer: OwnedWriteHalf, mut queue: mpsc::UnboundedReceiver<Frame>) {
    while let Some(frame) = queue.recv().await {
        if within(writer.write_all(&frame)).await.is_err() {
            return;
        }
    }
}

enum SocketError {
    /// No socket (or no folder): the daemon isn't running.
    Missing,
    Unsafe(String),
}

/// The socket's folder must be a real directory, ours, closed to other
/// users, and the socket must be a socket of ours (`wire`, "Socket
/// security"). Otherwise something else could be answering.
fn verify_socket(socket: &Path) -> Result<(), SocketError> {
    let me = rustix::process::geteuid().as_raw();
    let stat = |path: &Path| match std::fs::symlink_metadata(path) {
        Ok(meta) => Ok(meta),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(SocketError::Missing),
        Err(e) => Err(SocketError::Unsafe(format!("{}: {e}", path.display()))),
    };
    let Some(folder) = socket.parent().filter(|p| !p.as_os_str().is_empty()) else {
        return Err(SocketError::Unsafe("the path has no folder".into()));
    };
    let meta = stat(folder)?;
    if !meta.file_type().is_dir() {
        return Err(SocketError::Unsafe(
            "its folder is a symlink or not a directory".into(),
        ));
    }
    if meta.uid() != me {
        return Err(SocketError::Unsafe(
            "its folder belongs to another user".into(),
        ));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(SocketError::Unsafe(
            "its folder is open to other users".into(),
        ));
    }
    let meta = stat(socket)?;
    if !meta.file_type().is_socket() {
        return Err(SocketError::Unsafe("it is not a socket".into()));
    }
    if meta.uid() != me {
        return Err(SocketError::Unsafe("it belongs to another user".into()));
    }
    Ok(())
}
