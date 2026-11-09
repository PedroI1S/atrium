use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use atrium_host::lifecycle::{
    InstanceLock, LifecycleError, Paths, default_paths, install_keep_running, remove_keep_running,
    secure_dir, spawn_detached,
};
use atrium_host::supervise::{ProcessIdentity, identify};

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = PathBuf::from("/tmp").join(format!(
            "ah-lc-{prefix}-{}-{}",
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

fn make_paths(root: &Path) -> Paths {
    let runtime_dir = root.join("rt");
    let data_dir = root.join("data");
    Paths {
        socket: runtime_dir.join("daemon.sock"),
        lock: runtime_dir.join("daemon.lock"),
        store: data_dir.join("atrium.db"),
        snapshots_dir: data_dir.join("snapshots"),
        logs_dir: data_dir.join("logs"),
        app_identity: data_dir.join("app-identity.json"),
        runtime_dir,
        data_dir,
    }
}

#[test]
fn secure_dir_creates_0700_and_refuses_symlink_0755_and_other_owner() {
    let tmp = TempDir::new("secdir");

    // 1. Creates missing directory with mode 0700, idempotently.
    let good = tmp.path().join("nested/private");
    secure_dir(&good).expect("create 0700 dir");
    let meta = std::fs::symlink_metadata(&good).unwrap();
    assert!(meta.is_dir());
    assert!(!meta.file_type().is_symlink());
    assert_eq!(meta.mode() & 0o7777, 0o700);
    secure_dir(&good).expect("idempotent on valid 0700 dir");

    // 2. Refuses a symlink to a valid 0700 directory, and a dangling symlink.
    let link_to_good = tmp.path().join("sym-good");
    std::os::unix::fs::symlink(&good, &link_to_good).unwrap();
    let err_sym = secure_dir(&link_to_good).expect_err("must refuse symlink");
    assert!(
        matches!(&err_sym, LifecycleError::UnsafeDir(p, msg) if p == &link_to_good && msg.contains("symlink")),
        "unexpected error: {err_sym:?}"
    );

    let dangling = tmp.path().join("sym-dangling");
    std::os::unix::fs::symlink(tmp.path().join("missing-target"), &dangling).unwrap();
    let err_dangling = secure_dir(&dangling).expect_err("must refuse dangling symlink");
    assert!(
        matches!(&err_dangling, LifecycleError::UnsafeDir(p, msg) if p == &dangling && msg.contains("symlink")),
        "unexpected error: {err_dangling:?}"
    );

    // 3. Refuses a 0755 directory (and 0750).
    let loose = tmp.path().join("loose-0755");
    std::fs::DirBuilder::new()
        .mode(0o755)
        .create(&loose)
        .unwrap();
    std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o755)).unwrap();
    let err_loose = secure_dir(&loose).expect_err("must refuse 0755 dir");
    assert!(
        matches!(&err_loose, LifecycleError::UnsafeDir(p, msg) if p == &loose && msg.contains("0755")),
        "unexpected error: {err_loose:?}"
    );

    // 4. Refuses a regular file.
    let file_path = tmp.path().join("not-a-dir");
    std::fs::write(&file_path, "x").unwrap();
    assert!(matches!(
        secure_dir(&file_path),
        Err(LifecycleError::UnsafeDir(_, _))
    ));

    // 5. Refuses a directory owned by someone else (tested without root against
    // root-owned system directories when running as a non-root user).
    // SAFETY: `geteuid` has no preconditions.
    let euid = unsafe { libc::geteuid() };
    if euid != 0 {
        let root_dir = Path::new("/usr");
        let err_owner = secure_dir(root_dir).expect_err("must refuse uid 0 directory");
        assert!(
            matches!(&err_owner, LifecycleError::UnsafeDir(p, msg) if p == root_dir && msg.contains("owned by uid 0")),
            "expected ownership rejection on /usr, got {err_owner:?}"
        );

        #[cfg(target_os = "macos")]
        if std::fs::symlink_metadata("/private/var/root").is_ok() {
            let err_root_home = secure_dir(Path::new("/private/var/root"))
                .expect_err("must refuse /private/var/root");
            assert!(
                matches!(&err_root_home, LifecycleError::UnsafeDir(_, msg) if msg.contains("owned by uid 0")),
                "expected ownership rejection on /private/var/root, got {err_root_home:?}"
            );
        }
    }
}

#[test]
fn instance_lock_records_identity_removes_only_stale_socket_and_recovers_after_crash() {
    if let Ok(sock_dir) = std::env::var("ATRIUM_LOCK_HOLDER_SUBPROCESS") {
        let paths = make_paths(Path::new(&sock_dir));
        let _lock = InstanceLock::acquire(&paths).expect("child acquire lock");
        let _listener =
            std::os::unix::net::UnixListener::bind(&paths.socket).expect("child bind sock");
        println!("READY:{}", std::process::id());
        let _ = std::io::Write::flush(&mut std::io::stdout());
        std::thread::sleep(Duration::from_secs(60));
        return;
    }

    let tmp = TempDir::new("lock");
    let paths = make_paths(tmp.path());
    secure_dir(&paths.runtime_dir).unwrap();

    // Plant a stale socket file before acquiring the lock.
    {
        let stale_listener =
            std::os::unix::net::UnixListener::bind(&paths.socket).expect("bind stale");
        drop(stale_listener);
    }
    assert!(paths.socket.symlink_metadata().is_ok());

    let lock = InstanceLock::acquire(&paths).expect("acquire lock");
    assert!(
        paths.socket.symlink_metadata().is_err(),
        "stale socket must be removed after acquiring lock"
    );

    // Lock file must be mode 0600 and hold our ProcessIdentity as JSON.
    let lock_meta = std::fs::symlink_metadata(&paths.lock).unwrap();
    assert_eq!(lock_meta.mode() & 0o7777, 0o600);
    let recorded: ProcessIdentity =
        serde_json::from_slice(&std::fs::read(&paths.lock).unwrap()).expect("parse lock JSON");
    assert_eq!(recorded.pid, std::process::id());
    let me = identify(std::process::id()).unwrap().unwrap();
    assert_eq!(recorded.executable, me.executable);
    assert_eq!(recorded.start_time, me.start_time);

    // Bind a live socket while holding `lock`; a competing `acquire` must fail
    // with `AlreadyRunning(Some(my_pid))` BEFORE touching `paths.socket`.
    let live_listener =
        std::os::unix::net::UnixListener::bind(&paths.socket).expect("bind live socket");
    let second = InstanceLock::acquire(&paths).expect_err("second lock must fail");
    assert!(
        matches!(second, LifecycleError::AlreadyRunning(Some(pid)) if pid == std::process::id()),
        "expected AlreadyRunning(Some(my_pid)), got {second:?}"
    );
    assert!(
        paths.socket.symlink_metadata().is_ok(),
        "failed acquire must NEVER delete a live daemon's socket"
    );
    drop(live_listener);
    drop(lock);
    assert!(
        paths.socket.symlink_metadata().is_err(),
        "dropping InstanceLock cleans up socket while lock is still held"
    );

    // Subprocess crash simulation: child holds lock + socket, gets SIGKILLed,
    // leaving stale socket behind; next `acquire` recovers cleanly.
    let crash_root = tmp.path().join("crash");
    let crash_paths = make_paths(&crash_root);
    secure_dir(&crash_paths.runtime_dir).unwrap();
    let mut holder = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("instance_lock_records_identity_removes_only_stale_socket_and_recovers_after_crash")
        .arg("--nocapture")
        .arg("--quiet")
        .env("ATRIUM_LOCK_HOLDER_SUBPROCESS", &crash_root)
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn lock holder");

    let mut reader = std::io::BufReader::new(holder.stdout.take().unwrap());
    let child_pid: u32 = loop {
        let mut line = String::new();
        let n = std::io::BufRead::read_line(&mut reader, &mut line).unwrap();
        assert!(n > 0, "lock holder exited before signalling READY");
        if let Some(rest) = line.trim().strip_prefix("READY:") {
            break rest.parse().expect("child pid");
        }
    };

    let busy = InstanceLock::acquire(&crash_paths).expect_err("child holds lock");
    assert!(
        matches!(busy, LifecycleError::AlreadyRunning(Some(pid)) if pid == child_pid),
        "expected child pid {child_pid}, got {busy:?}"
    );
    assert!(crash_paths.socket.symlink_metadata().is_ok());

    let _ = holder.kill();
    let _ = holder.wait();
    // Crashed child left stale socket on disk:
    assert!(crash_paths.socket.symlink_metadata().is_ok());

    let recovered = InstanceLock::acquire(&crash_paths).expect("recover lock after SIGKILL");
    assert!(crash_paths.socket.symlink_metadata().is_err());
    let new_recorded: ProcessIdentity =
        serde_json::from_slice(&std::fs::read(&crash_paths.lock).unwrap()).unwrap();
    assert_eq!(new_recorded.pid, std::process::id());
    drop(recovered);
}

#[test]
fn spawn_detached_creates_new_session_and_enforces_private_log_0600_nofollow() {
    let tmp = TempDir::new("detached");
    let log = tmp.path().join("logs/daemon.log");

    let pid = spawn_detached(
        Path::new("/bin/sh"),
        &[
            "-c".to_string(),
            "echo out-line; echo err-line >&2; sleep 30".to_string(),
        ],
        &log,
    )
    .expect("spawn_detached");

    // Ensure cleanup even on assertion failure.
    struct DetachKill(u32);
    impl Drop for DetachKill {
        fn drop(&mut self) {
            if let Ok(raw) = i32::try_from(self.0)
                && raw > 1
            {
                // SAFETY: Killing detached process spawned by this test.
                unsafe {
                    libc::kill(raw, libc::SIGKILL);
                }
            }
        }
    }
    let _guard = DetachKill(pid);

    // Verify new session (`getsid(pid) == pid`).
    // SAFETY: `getsid` takes a `pid_t` and has no memory-safety preconditions.
    let sid = unsafe { libc::getsid(pid as i32) };
    assert_eq!(sid, pid as i32, "detached process must lead a new session");

    let log_meta = std::fs::symlink_metadata(&log).unwrap();
    assert_eq!(log_meta.mode() & 0o7777, 0o600);

    for _ in 0..100 {
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        if text.contains("out-line") && text.contains("err-line") {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("out-line"));
    assert!(text.contains("err-line"));

    // Kill the detached process and verify the background reaper reaps it.
    // SAFETY: Killing detached process spawned by this test.
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
    for _ in 0..100 {
        // SAFETY: Signal 0 checks existence.
        if unsafe { libc::kill(pid as i32, 0) } != 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let noop_args = ["-c".to_string(), "exit 0".to_string()];

    // Refuses symlink log (O_NOFOLLOW):
    let sym_log = tmp.path().join("sym.log");
    std::os::unix::fs::symlink(&log, &sym_log).unwrap();
    assert!(spawn_detached(Path::new("/bin/sh"), &noop_args, &sym_log).is_err());

    // Refuses looser mode (0644) existing log:
    let loose_log = tmp.path().join("loose.log");
    std::fs::write(&loose_log, "hi").unwrap();
    std::fs::set_permissions(&loose_log, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(spawn_detached(Path::new("/bin/sh"), &noop_args, &loose_log).is_err());

    // Refuses log owned by another user (when non-root):
    // SAFETY: `geteuid` has no preconditions.
    if unsafe { libc::geteuid() } != 0 && Path::new("/etc/hosts").exists() {
        assert!(spawn_detached(Path::new("/bin/sh"), &noop_args, Path::new("/etc/hosts")).is_err());
    }
}

#[test]
fn default_paths_and_keep_running_service_use_temporary_home_and_xdg_only() {
    if let Ok(mode) = std::env::var("ATRIUM_ENV_ISOLATED_SUBTEST") {
        match mode.as_str() {
            "atrium-home" => {
                let custom = PathBuf::from(std::env::var_os("ATRIUM_HOME").unwrap());
                let p = default_paths().expect("default_paths with ATRIUM_HOME");
                #[cfg(debug_assertions)]
                {
                    assert_eq!(p.runtime_dir, custom);
                    assert_eq!(p.data_dir, custom);
                    assert_eq!(p.socket, custom.join("daemon.sock"));
                    assert_eq!(p.lock, custom.join("daemon.lock"));
                    assert_eq!(p.store, custom.join("atrium.db"));
                    assert_eq!(p.snapshots_dir, custom.join("snapshots"));
                    assert_eq!(p.logs_dir, custom.join("logs"));
                    assert_eq!(p.app_identity, custom.join("app-identity.json"));
                }
                #[cfg(not(debug_assertions))]
                {
                    assert_ne!(
                        p.runtime_dir, custom,
                        "release builds must ignore ATRIUM_HOME"
                    );
                    assert_ne!(p.data_dir, custom);
                }
            }
            "keep-running" => {
                #[cfg(target_os = "macos")]
                let fake_home = PathBuf::from(std::env::var_os("HOME").unwrap());
                #[cfg(target_os = "linux")]
                let fake_xdg = PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").unwrap());

                // Verify default_paths without ATRIUM_HOME falls under fake HOME / XDG.
                let p = default_paths().expect("default_paths with fake HOME");
                #[cfg(target_os = "macos")]
                {
                    let expected = fake_home.join("Library/Application Support/Atrium");
                    assert_eq!(p.runtime_dir, expected);
                    assert_eq!(p.data_dir, expected);
                }
                #[cfg(target_os = "linux")]
                {
                    let fake_runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap());
                    let fake_data = PathBuf::from(std::env::var_os("XDG_DATA_HOME").unwrap());
                    assert_eq!(p.runtime_dir, fake_runtime.join("atrium"));
                    assert_eq!(p.data_dir, fake_data.join("atrium"));
                }

                let daemon_exe = Path::new("/opt/atrium/bin/atriumd");
                install_keep_running(daemon_exe).expect("install_keep_running");

                #[cfg(target_os = "macos")]
                {
                    let plist = fake_home.join("Library/LaunchAgents/dev.atrium.daemon.plist");
                    assert!(
                        plist.is_file(),
                        "LaunchAgent plist must be created in temp HOME"
                    );
                    let content = std::fs::read_to_string(&plist).unwrap();
                    assert!(content.contains("<string>dev.atrium.daemon</string>"));
                    assert!(content.contains("<string>/opt/atrium/bin/atriumd</string>"));
                    assert!(content.contains("<key>RunAtLoad</key>"));
                }
                #[cfg(target_os = "linux")]
                {
                    let unit = fake_xdg.join("systemd/user/atriumd.service");
                    let want = fake_xdg.join("systemd/user/default.target.wants/atriumd.service");
                    assert!(
                        unit.is_file(),
                        "systemd unit must be created in temp XDG_CONFIG_HOME"
                    );
                    assert!(want.symlink_metadata().unwrap().file_type().is_symlink());
                }

                remove_keep_running().expect("remove_keep_running");
                // Second remove must be idempotent:
                remove_keep_running().expect("idempotent remove_keep_running");

                #[cfg(target_os = "macos")]
                {
                    let plist = fake_home.join("Library/LaunchAgents/dev.atrium.daemon.plist");
                    assert!(!plist.exists(), "LaunchAgent plist must be removed");
                }
                #[cfg(target_os = "linux")]
                {
                    let unit = fake_xdg.join("systemd/user/atriumd.service");
                    let want = fake_xdg.join("systemd/user/default.target.wants/atriumd.service");
                    assert!(unit.symlink_metadata().is_err());
                    assert!(want.symlink_metadata().is_err());
                }
            }
            other => panic!("unknown subtest {other}"),
        }
        return;
    }

    let tmp = TempDir::new("env");
    let atrium_home = tmp.path().join("custom-atrium-home");
    let status1 = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("default_paths_and_keep_running_service_use_temporary_home_and_xdg_only")
        .arg("--nocapture")
        .arg("--quiet")
        .env("ATRIUM_ENV_ISOLATED_SUBTEST", "atrium-home")
        .env("ATRIUM_HOME", &atrium_home)
        .status()
        .expect("run atrium-home subtest");
    assert!(status1.success());

    let fake_home = tmp.path().join("fake-home");
    let fake_xdg = tmp.path().join("fake-xdg-config");
    let fake_runtime = tmp.path().join("fake-xdg-runtime");
    let fake_data = tmp.path().join("fake-xdg-data");
    std::fs::create_dir_all(&fake_home).unwrap();
    std::fs::create_dir_all(&fake_xdg).unwrap();
    std::fs::create_dir_all(&fake_runtime).unwrap();
    std::fs::create_dir_all(&fake_data).unwrap();

    // Seed sibling user service files in the temporary config dirs and verify
    // `install_keep_running` / `remove_keep_running` never touch them.
    let unrelated_plist = fake_home.join("Library/LaunchAgents/com.user.keepme.plist");
    std::fs::create_dir_all(unrelated_plist.parent().unwrap()).unwrap();
    std::fs::write(&unrelated_plist, "<plist>untouched</plist>").unwrap();

    let unrelated_unit = fake_xdg.join("systemd/user/keepme.service");
    std::fs::create_dir_all(unrelated_unit.parent().unwrap()).unwrap();
    std::fs::write(&unrelated_unit, "[Unit]\nDescription=KeepMe\n").unwrap();

    let real_home = std::env::var_os("HOME").map(PathBuf::from);
    let real_plist_existed_before = real_home.as_ref().is_some_and(|h| {
        h.join("Library/LaunchAgents/dev.atrium.daemon.plist")
            .exists()
    });

    let status2 = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("default_paths_and_keep_running_service_use_temporary_home_and_xdg_only")
        .arg("--nocapture")
        .arg("--quiet")
        .env("ATRIUM_ENV_ISOLATED_SUBTEST", "keep-running")
        .env_remove("ATRIUM_HOME")
        .env("HOME", &fake_home)
        .env("XDG_CONFIG_HOME", &fake_xdg)
        .env("XDG_RUNTIME_DIR", &fake_runtime)
        .env("XDG_DATA_HOME", &fake_data)
        .status()
        .expect("run keep-running subtest");
    assert!(status2.success());

    assert_eq!(
        std::fs::read_to_string(&unrelated_plist).unwrap(),
        "<plist>untouched</plist>"
    );
    assert_eq!(
        std::fs::read_to_string(&unrelated_unit).unwrap(),
        "[Unit]\nDescription=KeepMe\n"
    );

    if let Some(h) = real_home {
        assert_eq!(
            h.join("Library/LaunchAgents/dev.atrium.daemon.plist")
                .exists(),
            real_plist_existed_before,
            "real HOME LaunchAgents must never be touched by tests"
        );
    }
}
