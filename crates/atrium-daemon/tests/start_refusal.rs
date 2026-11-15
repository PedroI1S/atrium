use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use atrium_client::{Client, ClientOptions, ConnectionState, StartDaemon};
use atrium_host::lifecycle::{InstanceLock, Paths, default_paths, secure_dir, spawn_detached_wait};
use atrium_protocol::PROTOCOL_VERSION;
use atrium_protocol::wire::{ClientMessage, ClientRole, FrameKind, ServerMessage};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::Notify;
use tokio::time::timeout;

fn temp_home(prefix: &str) -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix(prefix)
        .tempdir()
        .expect("tempdir");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).expect("chmod");
    dir
}

fn paths_for(root: &Path) -> Paths {
    Paths {
        runtime_dir: root.to_path_buf(),
        data_dir: root.to_path_buf(),
        socket: root.join("daemon.sock"),
        lock: root.join("daemon.lock"),
        store: root.join("atrium.db"),
        snapshots_dir: root.join("snapshots"),
        logs_dir: root.join("logs"),
        app_identity: root.join("app-identity.json"),
    }
}

fn run_atriumd(home: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_atriumd"))
        .env("ATRIUM_HOME", home)
        .env("HOME", home)
        .output()
        .expect("run atriumd")
}

fn start_hook_for(home: &Path, paths: &Paths) -> StartDaemon {
    let home = home.to_path_buf();
    let log = paths.logs_dir.join("atriumd.log");
    let socket = paths.socket.clone();
    Arc::new(move || {
        let args = vec![
            format!("ATRIUM_HOME={}", home.display()),
            format!("HOME={}", home.display()),
            env!("CARGO_BIN_EXE_atriumd").to_string(),
        ];
        spawn_detached_wait(
            Path::new("env"),
            &args,
            &log,
            &socket,
            Duration::from_millis(1000),
        )
        .map(|_| ())
    })
}

async fn wait_for_failed(client: &Client) -> String {
    let mut rx = client.connection();
    timeout(Duration::from_secs(5), async {
        loop {
            let state = rx.borrow_and_update().clone();
            if let ConnectionState::Failed { message } = state {
                return message;
            }
            rx.changed().await.expect("connection watch open");
        }
    })
    .await
    .expect("client reaches ConnectionState::Failed in time")
}

#[tokio::test]
async fn loose_0755_logs_dir_refusal_reaches_stderr_and_client() {
    let home = temp_home("atrium-n9-0755-");
    let paths = paths_for(home.path());
    std::fs::create_dir(&paths.logs_dir).expect("create logs dir");
    std::fs::set_permissions(&paths.logs_dir, std::fs::Permissions::from_mode(0o755))
        .expect("chmod 0755");

    let out = run_atriumd(home.path());
    assert!(
        !out.status.success(),
        "atriumd must refuse 0755 logs dir and exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "expected one line on stderr, got: {stderr:?}"
    );
    assert!(
        stderr.contains(&paths.logs_dir.display().to_string()),
        "stderr must name the folder: {stderr}"
    );
    assert!(
        stderr.contains("0755") && stderr.contains("expected 0700"),
        "stderr must name mode and expected mode: {stderr}"
    );

    let client = Client::start(ClientOptions {
        socket: paths.socket.clone(),
        name: "n9-test".into(),
        role: ClientRole::Dev,
        start_daemon: Some(start_hook_for(home.path(), &paths)),
    });
    let message = wait_for_failed(&client).await;
    assert!(
        message.contains(&paths.logs_dir.display().to_string())
            && message.contains("0755")
            && message.contains("expected 0700"),
        "client failure must carry refusal reason, got: {message}"
    );
    client.shutdown();
}

#[tokio::test]
async fn symlinked_logs_dir_refusal_reaches_stderr_and_client() {
    let home = temp_home("atrium-n9-sym-");
    let paths = paths_for(home.path());
    let target = home.path().join("real-logs");
    secure_dir(&target).expect("create target 0700");
    std::os::unix::fs::symlink(&target, &paths.logs_dir).expect("symlink logs");

    let out = run_atriumd(home.path());
    assert!(
        !out.status.success(),
        "atriumd must refuse symlinked logs dir"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(stderr.lines().count(), 1, "one line on stderr: {stderr:?}");
    assert!(
        stderr.contains(&paths.logs_dir.display().to_string())
            && stderr.contains("symlink")
            && stderr.contains("expected"),
        "stderr must name folder, symlink, and expected: {stderr}"
    );

    let client = Client::start(ClientOptions {
        socket: paths.socket.clone(),
        name: "n9-sym-test".into(),
        role: ClientRole::Dev,
        start_daemon: Some(start_hook_for(home.path(), &paths)),
    });
    let message = wait_for_failed(&client).await;
    assert!(
        message.contains("symlink") && message.contains(&paths.logs_dir.display().to_string()),
        "client failure must carry symlink refusal, got: {message}"
    );
    client.shutdown();
}

#[tokio::test]
async fn held_instance_lock_refusal_reaches_stderr_and_client_keeps_retrying() {
    let home = temp_home("atrium-n9-lock-");
    let paths = paths_for(home.path());
    secure_dir(&paths.runtime_dir).expect("runtime dir");
    secure_dir(&paths.logs_dir).expect("logs dir");
    let lock = InstanceLock::acquire(&paths).expect("hold instance lock");

    let out = run_atriumd(home.path());
    assert!(
        !out.status.success(),
        "atriumd must refuse when lock is held"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(stderr.lines().count(), 1, "one line on stderr: {stderr:?}");
    assert!(
        stderr.contains("already running") && stderr.contains(&std::process::id().to_string()),
        "stderr must name already running and holder pid: {stderr}"
    );

    let refused = Arc::new(Notify::new());
    let refused_signal = Arc::clone(&refused);
    let inner = start_hook_for(home.path(), &paths);
    let hook: StartDaemon = Arc::new(move || {
        let res = inner();
        assert!(res.is_err(), "start_hook must see AlreadyRunning error");
        refused_signal.notify_one();
        res
    });

    let client = Client::start(ClientOptions {
        socket: paths.socket.clone(),
        name: "n9-lock-test".into(),
        role: ClientRole::Dev,
        start_daemon: Some(hook),
    });

    timeout(Duration::from_secs(5), refused.notified())
        .await
        .expect("start_hook runs and sees refusal");

    let mut rx = client.connection();
    timeout(Duration::from_secs(5), async {
        loop {
            let state = rx.borrow_and_update().clone();
            assert!(
                !matches!(state, ConnectionState::Failed { .. }),
                "held instance lock must not park client in Failed: {state:?}"
            );
            if matches!(state, ConnectionState::Connecting { attempt } if attempt >= 3) {
                break;
            }
            rx.changed().await.expect("connection watch open");
        }
    })
    .await
    .expect("client keeps retrying in Connecting");

    client.shutdown();
    drop(lock);
}

#[tokio::test]
async fn held_instance_lock_connects_once_socket_appears() {
    let home = temp_home("atrium-n9-lock-race-");
    let paths = paths_for(home.path());
    secure_dir(&paths.runtime_dir).expect("runtime dir");
    secure_dir(&paths.logs_dir).expect("logs dir");
    let lock = InstanceLock::acquire(&paths).expect("hold instance lock");

    let refused = Arc::new(Notify::new());
    let refused_signal = Arc::clone(&refused);
    let inner = start_hook_for(home.path(), &paths);
    let hook: StartDaemon = Arc::new(move || {
        let res = inner();
        if res.is_err() {
            refused_signal.notify_one();
        }
        res
    });

    let client = Client::start(ClientOptions {
        socket: paths.socket.clone(),
        name: "n9-lock-race-test".into(),
        role: ClientRole::Dev,
        start_daemon: Some(hook),
    });

    // Wait for the first start attempt to fail with "another daemon is already running".
    timeout(Duration::from_secs(5), refused.notified())
        .await
        .expect("start_hook sees refused start");

    assert!(
        !matches!(
            *client.connection().borrow(),
            ConnectionState::Failed { .. }
        ),
        "client must not park in Failed after raced start"
    );

    // The lock holder now binds the socket shortly after the refused start.
    let listener = UnixListener::bind(&paths.socket).expect("bind socket while holding lock");
    let accept_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept client");
        let len = stream.read_u32().await.expect("hello len");
        let kind = stream.read_u8().await.expect("hello kind");
        assert_eq!(kind, FrameKind::Control as u8);
        let mut buf = vec![0u8; len as usize - 1];
        stream.read_exact(&mut buf).await.expect("hello body");
        let msg: ClientMessage = serde_json::from_slice(&buf).expect("parse hello");
        assert!(matches!(msg, ClientMessage::Hello { .. }));

        let welcome = ServerMessage::Welcome {
            protocol_version: PROTOCOL_VERSION,
            daemon_version: "test".into(),
            daemon_pid: std::process::id(),
            role: ClientRole::Dev,
        };
        let payload = serde_json::to_vec(&welcome).expect("encode welcome");
        stream
            .write_u32(u32::try_from(payload.len() + 1).expect("len u32"))
            .await
            .expect("write welcome len");
        stream
            .write_u8(FrameKind::Control as u8)
            .await
            .expect("write welcome kind");
        stream
            .write_all(&payload)
            .await
            .expect("write welcome body");
        stream.flush().await.expect("flush welcome");
        // Keep stream open until client shuts down.
        let mut discard = [0u8; 16];
        let _ = stream.read(&mut discard).await;
    });

    let mut rx = client.connection();
    timeout(Duration::from_secs(5), async {
        loop {
            let state = rx.borrow_and_update().clone();
            if let ConnectionState::Failed { message } = state {
                panic!("client unexpectedly entered Failed: {message}");
            }
            if matches!(state, ConnectionState::Connected { .. }) {
                break;
            }
            rx.changed().await.expect("connection watch open");
        }
    })
    .await
    .expect("client reaches Connected once socket appears");

    client.shutdown();
    let _ = accept_task.await;
    drop(lock);
}

#[tokio::test]
async fn default_paths_0755_logs_dir_refuses_open_private_log_and_falls_back_to_stderr_pipe() {
    if std::env::var_os("ATRIUM_N9_SUBPROCESS").as_deref()
        == Some(std::ffi::OsStr::new("pipe-fallback"))
    {
        let paths = default_paths().expect("default_paths with ATRIUM_HOME");
        let log = paths.logs_dir.join("atriumd.log");
        let socket = paths.socket.clone();
        let hook: StartDaemon = Arc::new(move || {
            spawn_detached_wait(
                Path::new(env!("CARGO_BIN_EXE_atriumd")),
                &[],
                &log,
                &socket,
                Duration::from_millis(500),
            )
            .map(|_| ())
        });
        let client = Client::start(ClientOptions {
            socket: paths.socket.clone(),
            name: "n9-pipe-fallback-test".into(),
            role: ClientRole::Dev,
            start_daemon: Some(hook),
        });
        let message = wait_for_failed(&client).await;
        assert!(
            message.contains(&paths.logs_dir.display().to_string())
                && message.contains("0755")
                && message.contains("expected 0700"),
            "client failure must carry piped stderr refusal, got: {message}"
        );
        assert!(
            !paths.logs_dir.join("atriumd.log").exists(),
            "open_private_log must refuse 0755 logs_dir without creating atriumd.log"
        );
        client.shutdown();
        return;
    }

    let home = temp_home("atrium-n9-pipe-");
    let logs_dir = home.path().join("logs");
    std::fs::create_dir(&logs_dir).expect("create logs dir");
    std::fs::set_permissions(&logs_dir, std::fs::Permissions::from_mode(0o755))
        .expect("chmod 0755");

    let status = Command::new(std::env::current_exe().expect("current_exe"))
        .arg("--exact")
        .arg("default_paths_0755_logs_dir_refuses_open_private_log_and_falls_back_to_stderr_pipe")
        .env("ATRIUM_N9_SUBPROCESS", "pipe-fallback")
        .env("ATRIUM_HOME", home.path())
        .env("HOME", home.path())
        .status()
        .expect("run pipe-fallback subtest");
    assert!(status.success(), "pipe-fallback subtest failed: {status}");
}
