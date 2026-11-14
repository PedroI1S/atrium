use std::io::{BufRead, BufReader};
#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use atrium_host::lifecycle::{
    InstanceLock, LifecycleError, Paths, install_keep_running, secure_dir,
};
#[cfg(target_os = "linux")]
use atrium_host::peer::{AppIdentity, PeerError, verify_app_peer};
use atrium_host::supervise::{ProcessIdentity, identify, kill_recorded, spawn_group, terminate};
use atrium_protocol::thread::Cleanup;
#[cfg(target_os = "linux")]
use tokio::net::UnixStream;
use tokio::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ah-adv-{prefix}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .expect("create temp dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct KillPidOnDrop(u32);

impl Drop for KillPidOnDrop {
    fn drop(&mut self) {
        if let Ok(raw) = i32::try_from(self.0)
            && raw > 1
        {
            // SAFETY: `raw > 1` targets a specific child pid created by this test.
            unsafe {
                libc::kill(raw, libc::SIGKILL);
            }
        }
    }
}

struct KillGroupOnDrop(u32);

impl Drop for KillGroupOnDrop {
    fn drop(&mut self) {
        if let Ok(raw) = i32::try_from(self.0)
            && raw > 1
        {
            // SAFETY: `raw > 1`, so `-raw <= -2` targets only the child process group created by this test.
            unsafe {
                libc::kill(-raw, libc::SIGKILL);
            }
        }
    }
}

fn pid_exists(pid: u32) -> bool {
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    if raw <= 0 {
        return false;
    }
    // SAFETY: `kill(raw, 0)` with `raw > 0` performs an existence check without sending a signal.
    let rc = unsafe { libc::kill(raw, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[test]
fn identify_rejects_degenerate_pids_and_unreaped_zombies() {
    assert_eq!(identify(0).unwrap(), None);
    assert_eq!(identify(i32::MAX as u32 + 1).unwrap(), None);
    assert_eq!(identify(u32::MAX).unwrap(), None);
    assert_eq!(identify(999_999_999).unwrap(), None);

    let mut child = StdCommand::new("/bin/sh")
        .arg("-c")
        .arg("exit 0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn short-lived child");
    let pid = child.id();

    let mut saw_zombie = false;
    for _ in 0..200 {
        #[cfg(target_os = "linux")]
        if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            && stat.contains(") Z ")
        {
            saw_zombie = true;
            break;
        }
        #[cfg(not(target_os = "linux"))]
        if identify(pid).unwrap().is_none() {
            saw_zombie = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        saw_zombie,
        "child {pid} should reach zombie state before wait()"
    );
    assert_eq!(
        identify(pid).unwrap(),
        None,
        "identify must return Ok(None) for an unreaped zombie"
    );

    let _ = child.wait();
    assert_eq!(identify(pid).unwrap(), None);
}

fn spawn_group_retry_etxtbsy(path: &Path, arg: &str) -> atrium_host::supervise::SupervisedChild {
    for _ in 0..40 {
        let mut cmd = Command::new(path);
        cmd.arg(arg)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match spawn_group(cmd) {
            Ok(child) => return child,
            Err(err) if err.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(err) => panic!("spawn_group failed: {err:?}"),
        }
    }
    panic!("spawn_group still hit ETXTBSY after retries");
}

#[tokio::test]
async fn identify_handles_weird_comm_with_parens_and_spaces() {
    let tmp = TempDir::new("comm");
    let weird_bin = tmp.path().join("evil ) R 1 1 1 (name");
    let sleep_path = if Path::new("/bin/sleep").exists() {
        Path::new("/bin/sleep")
    } else {
        Path::new("/usr/bin/sleep")
    };
    std::fs::copy(sleep_path, &weird_bin).expect("copy sleep binary");
    std::fs::set_permissions(&weird_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut sup = spawn_group_retry_etxtbsy(&weird_bin, "30");
    let _guard = KillGroupOnDrop(sup.identity.pgid);

    let live = identify(sup.identity.pid)
        .expect("identify")
        .expect("live process");
    assert_eq!(live.pid, sup.identity.pid);
    assert_eq!(live.pgid, sup.identity.pid);
    assert_eq!(live.start_time, sup.identity.start_time);
    assert_eq!(live.executable, std::fs::canonicalize(&weird_bin).unwrap());

    let term = terminate(&mut sup, Duration::ZERO)
        .await
        .expect("terminate");
    assert!(term.killed);
}

#[tokio::test]
async fn kill_recorded_refuses_degenerate_and_self_identities_without_signalling() {
    let me = identify(std::process::id())
        .expect("identify self")
        .expect("self is live");

    for bad in [
        ProcessIdentity {
            pid: 0,
            pgid: 200,
            start_time: me.start_time,
            executable: me.executable.clone(),
        },
        ProcessIdentity {
            pid: 1,
            pgid: 1,
            start_time: me.start_time,
            executable: me.executable.clone(),
        },
        ProcessIdentity {
            pid: me.pid,
            pgid: 0,
            start_time: me.start_time,
            executable: me.executable.clone(),
        },
        ProcessIdentity {
            pid: me.pid,
            pgid: 1,
            start_time: me.start_time,
            executable: me.executable.clone(),
        },
        ProcessIdentity {
            pid: u32::MAX,
            pgid: 200,
            start_time: me.start_time,
            executable: me.executable.clone(),
        },
        ProcessIdentity {
            pid: me.pid,
            pgid: u32::MAX,
            start_time: me.start_time,
            executable: me.executable.clone(),
        },
        ProcessIdentity {
            pid: i32::MAX as u32 + 1,
            pgid: 200,
            start_time: me.start_time,
            executable: me.executable.clone(),
        },
    ] {
        assert_eq!(kill_recorded(&bad), Cleanup::Unverified);
    }

    assert_eq!(
        kill_recorded(&me),
        Cleanup::Unverified,
        "kill_recorded must refuse to signal the caller's own process group"
    );
}

#[tokio::test]
async fn kill_recorded_unverified_when_leader_exited_and_reaped_but_grandchild_remains_in_pgid() {
    if std::env::var_os("ATRIUM_ADV_ORPHAN_PGID_HELPER").is_some() {
        let mut cmd = StdCommand::new("/bin/sh");
        cmd.arg("-c")
            .arg("sleep 30 &\n bg=$!\n echo \"GROUP:$!\"\n exit 0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // SAFETY: `setpgid(0, 0)` is async-signal-safe and touches no shared memory.
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn().expect("spawn leader");
        let leader_pid = child.id();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).expect("read grandchild pid");
        let bg_pid: u32 = line
            .trim()
            .strip_prefix("GROUP:")
            .expect("GROUP prefix")
            .parse()
            .expect("bg pid");
        let _ = child.wait();
        println!("ORPHAN:{leader_pid}:{bg_pid}");
        return;
    }

    let out = StdCommand::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg(
            "kill_recorded_unverified_when_leader_exited_and_reaped_but_grandchild_remains_in_pgid",
        )
        .arg("--nocapture")
        .arg("--quiet")
        .env("ATRIUM_ADV_ORPHAN_PGID_HELPER", "1")
        .output()
        .expect("run helper");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .find_map(|l| l.strip_prefix("ORPHAN:"))
        .expect("ORPHAN line");
    let mut parts = line.split(':');
    let leader_pid: u32 = parts.next().unwrap().parse().unwrap();
    let bg_pid: u32 = parts.next().unwrap().parse().unwrap();
    let _guard_group = KillGroupOnDrop(leader_pid);
    let _guard_bg = KillPidOnDrop(bg_pid);

    assert_eq!(
        identify(leader_pid).unwrap(),
        None,
        "reaped leader must not be identifiable"
    );
    assert!(pid_exists(bg_pid), "grandchild must still be alive in pgid");

    let recorded = ProcessIdentity {
        pid: leader_pid,
        pgid: leader_pid,
        start_time: 12345,
        executable: PathBuf::from("/bin/sh"),
    };
    assert_eq!(
        kill_recorded(&recorded),
        Cleanup::Unverified,
        "leader is gone while group is non-empty: cannot verify group ownership, must return Unverified without killing"
    );
    assert!(
        pid_exists(bg_pid),
        "unverified group must not be signalled when leader is gone"
    );
}

#[tokio::test]
async fn unlinked_running_executable_is_rejected_by_kill_recorded_and_verify_app_peer() {
    let tmp = TempDir::new("deleted-bin");
    let bin_copy = tmp.path().join("ephemeral-sleep");
    let sleep_path = if Path::new("/bin/sleep").exists() {
        Path::new("/bin/sleep")
    } else {
        Path::new("/usr/bin/sleep")
    };
    std::fs::copy(sleep_path, &bin_copy).expect("copy sleep");
    std::fs::set_permissions(&bin_copy, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut sup = spawn_group_retry_etxtbsy(&bin_copy, "30");
    let _guard = KillGroupOnDrop(sup.identity.pgid);
    let recorded = sup.identity.clone();

    std::fs::remove_file(&bin_copy).expect("unlink running binary");

    #[cfg(target_os = "linux")]
    {
        let current = identify(recorded.pid)
            .expect("identify")
            .expect("still running");
        assert!(
            current.executable.to_string_lossy().ends_with(" (deleted)"),
            "expected (deleted) suffix on Linux, got {:?}",
            current.executable
        );
        assert_eq!(
            kill_recorded(&recorded),
            Cleanup::Unverified,
            "unlinked binary path no longer matches recorded executable"
        );
        assert!(pid_exists(recorded.pid));
    }

    let term = terminate(&mut sup, Duration::ZERO)
        .await
        .expect("terminate");
    assert!(term.killed);
    assert!(!pid_exists(recorded.pid));
}

// On macOS `/usr/bin/python3` is a stub that execs another Python, so the
// recorded executable no longer matches, `kill_recorded` refuses (Unverified)
// without signalling, and the leader never becomes a zombie: the premise of
// this test holds only on Linux.
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "M1-13: python3 stub re-execs on macOS; zombie-leader premise is Linux-only"
)]
#[tokio::test]
async fn kill_recorded_setsid_grandchild_and_unreaped_zombie_leader_boundaries() {
    let mut cmd = Command::new("/usr/bin/python3");
    cmd.arg("-c")
        .arg(
            "import os, subprocess, sys, time\n\
             p = subprocess.Popen(['/bin/sleep', '30'], start_new_session=True)\n\
             sys.stdout.write(f'SETSID:{p.pid}\\n')\n\
             sys.stdout.flush()\n\
             time.sleep(30)\n",
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut sup = spawn_group(cmd).expect("spawn python setsid parent");
    let recorded = sup.identity.clone();
    let _guard_group = KillGroupOnDrop(recorded.pgid);

    let stdout = sup.child.stdout.take().unwrap();
    let mut reader = tokio::io::BufReader::new(stdout);
    let mut line = String::new();
    tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut line)
        .await
        .expect("read setsid pid");
    let setsid_pid: u32 = line
        .trim()
        .strip_prefix("SETSID:")
        .expect("SETSID prefix")
        .parse()
        .expect("setsid pid");
    let _guard_setsid = KillPidOnDrop(setsid_pid);

    let cleanup_zombie = tokio::task::spawn_blocking({
        let recorded = recorded.clone();
        move || kill_recorded(&recorded)
    })
    .await
    .unwrap();
    assert_eq!(
        cleanup_zombie,
        Cleanup::Unverified,
        "unreaped zombie leader keeps process group alive in kernel"
    );

    let _ = sup.child.wait().await;
    assert_eq!(
        kill_recorded(&recorded),
        Cleanup::Verified,
        "once leader is reaped and group is gone, kill_recorded returns Verified even though setsid grandchild left the group"
    );
    assert!(
        pid_exists(setsid_pid),
        "setsid grandchild is outside the process-group boundary"
    );
}

#[tokio::test]
async fn terminate_twice_and_fast_exiting_spawn_group_behave_safely() {
    let mut fast_cmd = Command::new("/bin/sh");
    fast_cmd
        .arg("-c")
        .arg("exit 0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut fast = spawn_group(fast_cmd).expect("spawn fast-exiting command");
    let term = terminate(&mut fast, Duration::from_millis(200))
        .await
        .expect("terminate fast child");
    assert_eq!(term.exit_code, Some(0));
    assert!(!term.killed);

    let err = terminate(&mut fast, Duration::ZERO)
        .await
        .expect_err("second terminate must fail");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn linux_verify_app_peer_rejects_relative_symlink_mismatch_and_deleted_exe() {
    if std::env::var_os("ATRIUM_ADV_DELETED_PEER_SUBPROCESS").is_some() {
        let sock = std::env::var_os("ATRIUM_ADV_DELETED_PEER_SOCK").unwrap();
        let _conn = std::os::unix::net::UnixStream::connect(sock).expect("child connect");
        println!("CONNECTED");
        std::thread::sleep(Duration::from_secs(30));
        return;
    }

    let tmp = TempDir::new("peer-linux");
    let sock_path = tmp.path().join("peer.sock");
    let listener = UnixListener::bind(&sock_path).expect("bind");
    listener.set_nonblocking(true).unwrap();
    let tokio_listener = tokio::net::UnixListener::from_std(listener).unwrap();

    let client = UnixStream::connect(&sock_path).await.expect("connect");
    let (server_stream, _) = tokio_listener.accept().await.expect("accept");

    let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
    let meta = std::fs::metadata(&exe).unwrap();

    let rel_err = verify_app_peer(
        &server_stream,
        &AppIdentity::Linux {
            executable: PathBuf::from("relative/atrium-app"),
            dev_ino: None,
        },
    )
    .expect_err("relative executable must be rejected");
    assert!(matches!(rel_err, PeerError::NotApp(_)));

    let symlink_exe = tmp.path().join("symlink-exe");
    std::os::unix::fs::symlink(&exe, &symlink_exe).unwrap();
    let sym_err = verify_app_peer(
        &server_stream,
        &AppIdentity::Linux {
            executable: symlink_exe,
            dev_ino: Some((meta.dev(), meta.ino())),
        },
    )
    .expect_err("symlink executable path must not match canonical /proc/pid/exe target");
    assert!(
        matches!(&sym_err, PeerError::NotApp(msg) if msg.contains("executable mismatch")),
        "got {sym_err:?}"
    );

    // SAFETY: `geteuid` takes no arguments and has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        let non_root_err = verify_app_peer(
            &server_stream,
            &AppIdentity::Linux {
                executable: exe.clone(),
                dev_ino: None,
            },
        )
        .expect_err("non-root executable with dev_ino: None must be rejected");
        assert!(
            matches!(&non_root_err, PeerError::NotApp(msg) if msg.contains("not owned by root")),
            "got {non_root_err:?}"
        );
    }

    let ok_with_ino = verify_app_peer(
        &server_stream,
        &AppIdentity::Linux {
            executable: exe.clone(),
            dev_ino: Some((meta.dev(), meta.ino())),
        },
    )
    .expect("canonical executable with matching dev_ino succeeds");
    assert_eq!(ok_with_ino.pid, std::process::id());

    for bad_pair in [(meta.dev() ^ 1, meta.ino()), (meta.dev(), meta.ino() ^ 1)] {
        let ino_err = verify_app_peer(
            &server_stream,
            &AppIdentity::Linux {
                executable: exe.clone(),
                dev_ino: Some(bad_pair),
            },
        )
        .expect_err("mismatched dev_ino must be rejected");
        assert!(
            matches!(&ino_err, PeerError::NotApp(msg) if msg.contains("dev/ino mismatch")),
            "got {ino_err:?}"
        );
    }

    drop(client);
    drop(server_stream);

    let copied_bin = tmp.path().join("copied-test-bin");
    std::fs::copy(&exe, &copied_bin).expect("copy test binary");
    std::fs::set_permissions(&copied_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let copied_meta = std::fs::metadata(&copied_bin).unwrap();

    let mut child = loop {
        match StdCommand::new(&copied_bin)
            .arg("--exact")
            .arg("linux_verify_app_peer_rejects_relative_symlink_mismatch_and_deleted_exe")
            .arg("--nocapture")
            .arg("--quiet")
            .env("ATRIUM_ADV_DELETED_PEER_SUBPROCESS", "1")
            .env("ATRIUM_ADV_DELETED_PEER_SOCK", &sock_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => break c,
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => panic!("spawn copied test binary: {e:?}"),
        }
    };
    let _child_guard = KillPidOnDrop(child.id());

    let (peer_stream, _) = tokio_listener.accept().await.expect("accept child");
    std::fs::remove_file(&copied_bin).expect("unlink running child binary");

    std::fs::copy(&exe, &copied_bin).expect("replace unlinked binary at same path");

    for dev_ino in [None, Some((copied_meta.dev(), copied_meta.ino()))] {
        let del_err = verify_app_peer(
            &peer_stream,
            &AppIdentity::Linux {
                executable: copied_bin.clone(),
                dev_ino,
            },
        )
        .expect_err("unlinked running peer executable must be rejected");
        assert!(
            matches!(&del_err, PeerError::NotApp(msg) if msg.contains("executable mismatch")),
            "expected executable mismatch on (deleted) link, got {del_err:?}"
        );
    }

    let deleted_lookalike = tmp.path().join("copied-test-bin (deleted)");
    std::fs::copy(&exe, &deleted_lookalike).expect("create lookalike (deleted) file");
    let lookalike_err = verify_app_peer(
        &peer_stream,
        &AppIdentity::Linux {
            executable: deleted_lookalike,
            dev_ino: None,
        },
    )
    .expect_err("lookalike (deleted) file on disk must not match unlinked peer inode");
    assert!(
        matches!(&lookalike_err, PeerError::NotApp(msg) if msg.contains("inode does not match")),
        "expected inode mismatch on lookalike (deleted) file, got {lookalike_err:?}"
    );

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn lifecycle_rejects_hostile_executable_paths_and_symlinked_lock_files() {
    for bad in [
        Path::new(""),
        Path::new("relative/atriumd"),
        Path::new("/opt/atrium/bin/atriumd\nExecStart=/bin/sh"),
        Path::new("/opt/atrium/bin/atriumd\rExecStart=/bin/sh"),
    ] {
        let err = install_keep_running(bad).expect_err("must reject hostile executable path");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    let tmp = TempDir::new("lock-adv");
    let runtime_dir = tmp.path().join("rt");
    let data_dir = tmp.path().join("data");
    secure_dir(&runtime_dir).unwrap();
    let paths = Paths {
        socket: runtime_dir.join("daemon.sock"),
        lock: runtime_dir.join("daemon.lock"),
        store: data_dir.join("atrium.db"),
        snapshots_dir: data_dir.join("snapshots"),
        logs_dir: data_dir.join("logs"),
        app_identity: data_dir.join("app-identity.json"),
        runtime_dir: runtime_dir.clone(),
        data_dir,
    };

    let _sock = UnixListener::bind(&paths.socket).expect("bind socket");
    let target = tmp.path().join("outside-lock");
    std::fs::write(&target, "{}").unwrap();
    std::os::unix::fs::symlink(&target, &paths.lock).unwrap();

    let err = InstanceLock::acquire(&paths).expect_err("symlink lock must be rejected");
    assert!(matches!(err, LifecycleError::UnsafeDir(_, _)), "{err:?}");
    assert!(
        paths.socket.symlink_metadata().is_ok(),
        "rejected lock acquisition must not unlink existing socket"
    );
}
