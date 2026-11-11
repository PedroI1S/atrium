//! The socket: accepting connections, the handshake and roles (contract §4),
//! each connection's reader, and subscriptions' feeders.

use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use atrium_host::peer::{self, AppIdentity};
use atrium_protocol::PROTOCOL_VERSION;
use atrium_protocol::limits::{HELLO_TIMEOUT, MAX_CONNECTIONS, REPLAY_PAGE_EVENTS};
use atrium_protocol::thread::ThreadId;
use atrium_protocol::wire::{
    Authority, ClientMessage, ClientRole, ErrorCode, EventId, ServerMessage,
};
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;

use crate::conn::{self, Coalesce, Conn, SubId};
use crate::hub::encode_event;
use crate::{AppIdentitySource, DAEMON_VERSION, Inner, Phase, codec, commands};

/// How long a refusal (`Incompatible`) may take to write.
const REFUSAL_PATIENCE: Duration = Duration::from_secs(2);

/// How long connections get to take their last answers when the daemon
/// stops.
const FINISH_PATIENCE: Duration = Duration::from_secs(2);

/// The largest app-identity file read.
const MAX_IDENTITY_BYTES: u64 = 64 * 1024;

/// Binds the socket, closed to everyone but the user. Its directory is
/// already private (`secure_dir`), which is what keeps others out between
/// the bind and the `chmod`.
pub(crate) fn bind(socket: &Path) -> io::Result<UnixListener> {
    let listener = UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Accepts connections until the daemon is told to stop (and, unless
/// forced, no turn is running), then lets every connection take its last
/// answers and closes it.
pub(crate) async fn serve(inner: Arc<Inner>, listener: UnixListener) {
    // Counts every accepted connection, greeted or not.
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut connections = JoinSet::new();
    let mut phase = inner.phase.subscribe();
    loop {
        let quieter = inner.quieter.notified();
        tokio::pin!(quieter);
        quieter.as_mut().enable();
        match inner.phase() {
            Phase::Stopping => break,
            Phase::Draining if inner.running_turns() == 0 => {
                inner.request_stop(true);
                break;
            }
            Phase::Draining | Phase::Running => {}
        }
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => match slots.clone().try_acquire_owned() {
                    Ok(slot) => {
                        connections.spawn(connection(inner.clone(), stream, slot));
                    }
                    Err(_) => {
                        connections.spawn(refuse(inner.clone(), stream, "too many clients"));
                    }
                },
                // Out of file descriptors, most likely: let some close.
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            },
            _ = phase.changed() => {}
            () = &mut quieter => {}
            // Reaps finished connections so the set doesn't grow.
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
    drop(listener);
    inner.hub().finish_all();
    let finished = async { while connections.join_next().await.is_some() {} };
    if tokio::time::timeout(FINISH_PATIENCE, finished)
        .await
        .is_err()
    {
        connections.shutdown().await;
    }
}

fn incompatible(inner: &Inner, message: &str) -> ServerMessage {
    ServerMessage::Incompatible {
        protocol_version: PROTOCOL_VERSION,
        message: message.to_owned(),
        daemon_version: DAEMON_VERSION.to_owned(),
        daemon_pid: std::process::id(),
        running_turns: inner.running_turns(),
    }
}

/// Answers `Incompatible` and closes.
async fn refuse(inner: Arc<Inner>, mut stream: UnixStream, message: &str) {
    if let Some(frame) = codec::encode(&incompatible(&inner, message)) {
        let _ = tokio::time::timeout(REFUSAL_PATIENCE, async {
            stream.write_all(&frame).await?;
            stream.shutdown().await
        })
        .await;
    }
}

/// Reads the development app identity (contract §4): a regular file of the
/// user's, closed to others, not a symlink.
fn read_identity(path: &Path) -> Result<AppIdentity, String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| format!("the app identity can't be read: {error}"))?;
    let meta = file
        .metadata()
        .map_err(|error| format!("the app identity can't be read: {error}"))?;
    if !meta.is_file() {
        return Err("the app identity is not a regular file".into());
    }
    if meta.uid() != rustix::process::geteuid().as_raw() {
        return Err("the app identity belongs to another user".into());
    }
    if meta.mode() & 0o077 != 0 {
        return Err("the app identity is readable by others".into());
    }
    let mut text = Vec::new();
    file.take(MAX_IDENTITY_BYTES)
        .read_to_end(&mut text)
        .map_err(|error| format!("the app identity can't be read: {error}"))?;
    serde_json::from_slice(&text).map_err(|error| format!("the app identity is invalid: {error}"))
}

/// Whether the connection gets the role it asked for (contract §4). There
/// is no lesser role to fall back to.
fn grant(inner: &Inner, stream: &UnixStream, role: &ClientRole) -> Result<(), String> {
    match role {
        ClientRole::App => {
            let identity = match &inner.options.app_identity {
                AppIdentitySource::None => return Err("no app identity is known".into()),
                AppIdentitySource::Fixed(identity) => identity.clone(),
                AppIdentitySource::File(path) => read_identity(path)?,
            };
            peer::verify_app_peer(stream, &identity)
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
        ClientRole::Dev => {
            if !(cfg!(debug_assertions) && inner.options.dev_socket) {
                return Err("dev connections aren't accepted".into());
            }
            let peer = stream.peer_cred().map_err(|error| error.to_string())?;
            if peer.uid() != rustix::process::geteuid().as_raw() {
                return Err("the peer is another user".into());
            }
            Ok(())
        }
        ClientRole::Agent { .. } => Err("agent connections aren't accepted".into()),
    }
}

/// One connection, from its `Hello` to its end.
async fn connection(inner: Arc<Inner>, mut stream: UnixStream, _slot: OwnedSemaphorePermit) {
    let hello = tokio::time::timeout(HELLO_TIMEOUT, codec::read_client(&mut stream)).await;
    let Ok(Ok(payload)) = hello else {
        return;
    };
    let Ok(ClientMessage::Hello {
        protocol_version,
        role,
        ..
    }) = serde_json::from_slice(&payload)
    else {
        return;
    };
    if protocol_version != PROTOCOL_VERSION {
        return refuse(inner, stream, "protocol version mismatch").await;
    }
    if let Err(why) = grant(&inner, &stream, &role) {
        eprintln!("atriumd: refused a connection: {why}");
        return refuse(inner, stream, "unverified client").await;
    }
    if inner.phase() == Phase::Stopping {
        return;
    }

    let (mut reader, writer) = stream.into_split();
    let conn = Arc::new(Conn::with_limit(
        inner.conn_ids.fetch_add(1, Ordering::Relaxed),
        role.clone(),
        inner.options.connection_queue_bytes,
    ));
    conn.control(
        &ServerMessage::Welcome {
            protocol_version: PROTOCOL_VERSION,
            daemon_version: DAEMON_VERSION.to_owned(),
            daemon_pid: std::process::id(),
            role,
        },
        Coalesce::Keep,
    );
    inner.hub().add_conn(&conn);
    let writing = tokio::spawn({
        let conn = conn.clone();
        async move { conn::write_loop(&conn, writer).await }
    });

    loop {
        let payload = tokio::select! {
            read = codec::read_client(&mut reader) => match read {
                Ok(payload) => payload,
                // The client left, or sent a frame that isn't one.
                Err(_) => break,
            },
            () = conn.closed() => break,
        };
        let Ok(message) = serde_json::from_slice::<ClientMessage>(&payload) else {
            break;
        };
        if !handle(&inner, &conn, message).await {
            break;
        }
    }
    conn.close();
    inner.hub().remove_conn(conn.id);
    let _ = writing.await;
}

/// Handles one message. `false` closes the connection.
async fn handle(inner: &Arc<Inner>, conn: &Arc<Conn>, message: ClientMessage) -> bool {
    // M1 roles that are granted all have human control; the check stays so
    // a later role can't gain subscriptions or commands by default.
    let authorized = |needed: Authority| conn.role.authority() >= needed;
    match message {
        // Only valid as the first message.
        ClientMessage::Hello { .. } => return false,
        ClientMessage::Subscribe {
            request_id,
            thread_id,
            after,
        } => {
            if !authorized(Authority::HumanControl) {
                conn.err(request_id, ErrorCode::Forbidden, "not allowed");
            } else {
                subscribe(inner, conn, request_id, thread_id, after).await;
            }
        }
        ClientMessage::Unsubscribe { thread_id } => conn.unsubscribe(&thread_id),
        ClientMessage::SubscribeInbox { request_id } => {
            if !authorized(Authority::HumanControl) {
                conn.err(request_id, ErrorCode::Forbidden, "not allowed");
            } else {
                inner.hub().subscribe_inbox(conn, request_id);
            }
        }
        ClientMessage::Command {
            request_id,
            command,
        } => {
            if !authorized(command.authority()) {
                conn.err(request_id, ErrorCode::Forbidden, "not allowed");
            } else if commands::is_slow(&command) {
                // Doesn't hold up the connection's other messages.
                let (inner, conn) = (inner.clone(), conn.clone());
                tokio::spawn(async move {
                    let result = commands::run(&inner, &conn, command).await;
                    answer(&conn, request_id, result);
                });
            } else {
                let result = commands::run(inner, conn, command).await;
                answer(conn, request_id, result);
            }
        }
    }
    true
}

fn answer(conn: &Conn, request_id: u64, result: Result<serde_json::Value, crate::Failure>) {
    match result {
        Ok(value) => conn.ok(request_id, value),
        Err((code, message)) => conn.err(request_id, code, message),
    }
}

/// Answers a `Subscribe` and starts feeding the subscription.
async fn subscribe(
    inner: &Arc<Inner>,
    conn: &Arc<Conn>,
    request_id: u64,
    thread: ThreadId,
    after: Option<EventId>,
) {
    let last = match inner.store.thread(&thread).await {
        Ok(Some(_)) => inner.store.last_event(&thread).await,
        Ok(None) => {
            return conn.err(request_id, ErrorCode::UnknownThread, "unknown thread");
        }
        Err(error) => Err(error),
    };
    let replay_until = match last {
        Ok(last) => last,
        Err(error) => return conn.err(request_id, ErrorCode::Internal, error.to_string()),
    };
    if after > replay_until {
        return conn.err(
            request_id,
            ErrorCode::Invalid,
            "the cursor is beyond the thread's last event",
        );
    }
    let Some(sub) = conn.subscribe(&thread) else {
        return conn.err(request_id, ErrorCode::Invalid, "too many subscriptions");
    };
    // Queued right after the old subscription's frames were removed, so
    // nothing of it can follow this answer (D-058).
    conn.ok(request_id, json!({ "replay_until": replay_until }));
    tokio::spawn(feed(
        inner.clone(),
        conn.clone(),
        sub,
        thread,
        after,
        replay_until,
    ));
}

/// Feeds a subscription from the store: the replay up to `replay_until`,
/// `ReplayDone`, whatever was stored meanwhile, and then, once a read under
/// the publish lock finds nothing more, it goes live. The lock is never
/// held while waiting for the client.
async fn feed(
    inner: Arc<Inner>,
    conn: Arc<Conn>,
    sub: SubId,
    thread: ThreadId,
    after: Option<EventId>,
    replay_until: Option<EventId>,
) {
    let mut cursor = after;
    let mut replayed = false;
    let done = || {
        codec::encode(&ServerMessage::ReplayDone {
            thread_id: thread.clone(),
            last: replay_until,
        })
    };
    loop {
        let mut page = match inner
            .store
            .events_after(&thread, cursor, REPLAY_PAGE_EVENTS)
            .await
        {
            Ok(page) => page,
            Err(error) => return broken(&conn, &thread, &error),
        };
        if page.is_empty() {
            // Everything stored when the subscription began has been fed.
            if !replayed {
                let Some(frame) = done() else {
                    return;
                };
                if !conn.feed(sub, None, frame).await {
                    return;
                }
                replayed = true;
            }
            let _publishing = inner.publish.lock().await;
            page = match inner
                .store
                .events_after(&thread, cursor, REPLAY_PAGE_EVENTS)
                .await
            {
                Ok(page) => page,
                Err(error) => return broken(&conn, &thread, &error),
            };
            if page.is_empty() {
                inner.hub().go_live(&conn, sub, &thread);
                return;
            }
        }
        for event in page {
            if !replayed && Some(event.id) > replay_until {
                let Some(frame) = done() else {
                    return;
                };
                if !conn.feed(sub, None, frame).await {
                    return;
                }
                replayed = true;
            }
            cursor = Some(event.id);
            // An event too large for a frame can't be stored in the first
            // place (`MAX_EVENT_BYTES`).
            if let Some(frame) = encode_event(&event)
                && !conn.feed(sub, Some(event.id), frame).await
            {
                return;
            }
        }
    }
}

/// The store couldn't be read for a subscription. Closing the connection is
/// the one signal a client always handles: it reconnects and subscribes
/// again from its cursor.
fn broken(conn: &Conn, thread: &ThreadId, error: &atrium_core::StoreError) {
    eprintln!("atriumd: can't read {thread:?} for a subscription: {error}");
    conn.close();
}
