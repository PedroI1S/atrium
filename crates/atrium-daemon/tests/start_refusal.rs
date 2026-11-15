use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use atrium_client::{Client, ClientOptions, ConnectionState, StartDaemon};
use atrium_host::lifecycle::{InstanceLock, Paths, secure_dir, spawn_detached_wait};
use atrium_protocol::wire::ClientRole;
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
async fn held_instance_lock_refusal_reaches_stderr_and_client() {
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

    let client = Client::start(ClientOptions {
        socket: paths.socket.clone(),
        name: "n9-lock-test".into(),
        role: ClientRole::Dev,
        start_daemon: Some(start_hook_for(home.path(), &paths)),
    });
    let message = wait_for_failed(&client).await;
    assert!(
        message.contains("already running") && message.contains(&std::process::id().to_string()),
        "client failure must carry lock refusal, got: {message}"
    );
    client.shutdown();
    drop(lock);
}
