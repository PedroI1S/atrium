//! M1-28 follow-ups for `atrium-daemon` (M1-22 login-shell `PATH` discovery).
//!
//! Never runs a real provider CLI or real user shell: uses a fake `$SHELL` and
//! fake `claude` executable inside a temp folder.

mod common;

use std::ffi::OsStr;
use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use atrium_daemon::{NO_PATH_EXTRAS_ENV, discover_login_shell_path};
use atrium_protocol::agent::Provider;
use atrium_protocol::policy::Preset;
use atrium_protocol::wire::{DaemonCommand, ProviderProbe};
use common::{SOON, Wire, folder};
use tokio::net::UnixStream;

struct DaemonProcess {
    child: Option<Child>,
    stderr_path: PathBuf,
}

impl DaemonProcess {
    fn spawn(
        atrium_home: &Path,
        home: &Path,
        path: &Path,
        shell: Option<&Path>,
        stderr_path: &Path,
        disable_path_extras: bool,
    ) -> Self {
        let stderr_file = File::create(stderr_path).expect("create stderr file");
        let mut command = Command::new(env!("CARGO_BIN_EXE_atriumd"));
        command
            .arg("--dev-socket")
            .env("ATRIUM_HOME", atrium_home)
            .env("HOME", home)
            .env("PATH", path)
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr_file));
        match shell {
            Some(s) => {
                command.env("SHELL", s);
            }
            None => {
                command.env_remove("SHELL");
            }
        }
        if disable_path_extras {
            command.env(NO_PATH_EXTRAS_ENV, "1");
        } else {
            command.env_remove(NO_PATH_EXTRAS_ENV);
        }
        let child = command.spawn().expect("spawn atriumd");
        Self {
            child: Some(child),
            stderr_path: stderr_path.to_path_buf(),
        }
    }

    async fn wait_for_listen(&mut self, socket: &Path) {
        let deadline = tokio::time::Instant::now() + SOON;
        loop {
            if socket.exists() && UnixStream::connect(socket).await.is_ok() {
                return;
            }
            if let Some(child) = self.child.as_mut()
                && let Some(status) = child.try_wait().expect("poll atriumd")
            {
                self.child = None;
                let stderr = std::fs::read_to_string(&self.stderr_path).unwrap_or_default();
                panic!("atriumd exited early ({status}) before listening: {stderr}");
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for socket listen at {}",
                socket.display()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn wait_exit(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let deadline = tokio::time::Instant::now() + SOON;
        loop {
            if let Some(status) = child.try_wait().expect("poll atriumd exit") {
                if !status.success() {
                    let stderr = std::fs::read_to_string(&self.stderr_path).unwrap_or_default();
                    panic!("atriumd exited with {status}: {stderr}");
                }
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("timed out waiting for atriumd to exit");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn create_private_dir(path: &Path) {
    std::fs::create_dir_all(path).expect("create dir");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).expect("chmod 0700");
}

fn write_executable(path: &Path, body: &str) {
    std::fs::write(path, body).expect("write script");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).expect("chmod 0700");
}

fn write_fake_claude(bin_dir: &Path, version: &str) {
    let script = format!(
        "#!/bin/sh\nif [ \"$#\" -eq 1 ] && [ \"$1\" = \"--version\" ]; then\n    printf '{version} (Claude Code)\\n'\n    exit 0\nfi\nexit 1\n"
    );
    write_executable(&bin_dir.join("claude"), &script);
}

#[test]
fn discover_login_shell_path_handles_noise_failures_and_timeouts() {
    let root = folder();
    let noisy_sh = root.path().join("noisy-sh");
    write_executable(
        &noisy_sh,
        "#!/bin/sh\nprintf 'MOTD banner\\n'\nPATH='/fake/nvm/bin:/fake/asdf/bin'\n/bin/sh -c \"$2\"\nprintf '\\ntrailing\\n'\n",
    );
    let found = discover_login_shell_path(Some(noisy_sh.as_os_str()), Duration::from_secs(2));
    assert_eq!(
        found.as_deref(),
        Some(OsStr::new("/fake/nvm/bin:/fake/asdf/bin"))
    );

    let failing_sh = root.path().join("failing-sh");
    write_executable(&failing_sh, "#!/bin/sh\nexit 1\n");
    assert_eq!(
        discover_login_shell_path(Some(failing_sh.as_os_str()), Duration::from_secs(2)),
        None
    );

    let hanging_sh = root.path().join("hanging-sh");
    write_executable(&hanging_sh, "#!/bin/sh\nsleep 10\n");
    let started = Instant::now();
    assert_eq!(
        discover_login_shell_path(Some(hanging_sh.as_os_str()), Duration::from_millis(120)),
        None
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "timed out shell stalled for {:?}",
        started.elapsed()
    );

    let missing_sh = root.path().join("does-not-exist");
    assert_eq!(
        discover_login_shell_path(Some(missing_sh.as_os_str()), Duration::from_secs(1)),
        None
    );
    assert_eq!(
        discover_login_shell_path(None, Duration::from_secs(1)),
        None
    );
}

fn process_alive(pid: i32) -> bool {
    let Some(pid) = rustix::process::Pid::from_raw(pid) else {
        return false;
    };
    !matches!(
        rustix::process::test_kill_process(pid),
        Err(rustix::io::Errno::SRCH)
    )
}

fn kill_if_alive(pid: i32) {
    if let Some(pid) = rustix::process::Pid::from_raw(pid) {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
    }
}

#[test]
fn discover_login_shell_path_kills_process_group_descendants_on_timeout() {
    let root = folder();
    let pid_file = root.path().join("sleep.pid");
    let script = root.path().join("timeout-descendant-sh");
    write_executable(
        &script,
        &format!(
            "#!/bin/sh\n/bin/sh -c 'echo $$ > \"{}\"; exec sleep 600'\ntrue\n",
            pid_file.display()
        ),
    );

    let started = Instant::now();
    assert_eq!(
        discover_login_shell_path(Some(script.as_os_str()), Duration::from_millis(300)),
        None
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "timed-out login shell stalled for {:?}",
        started.elapsed()
    );

    let pid_text = std::fs::read_to_string(&pid_file).expect("descendant wrote its pid");
    let descendant_pid: i32 = pid_text.trim().parse().expect("parse descendant pid");
    let still_alive = process_alive(descendant_pid);
    if still_alive {
        kill_if_alive(descendant_pid);
    }
    assert!(
        !still_alive,
        "timeout left descendant sleep process {descendant_pid} alive"
    );
}

#[test]
fn discover_login_shell_path_cleans_up_background_descendant_retaining_stdout() {
    let root = folder();

    // 1. Shell emits marked PATH and exits 0 while a background child still holds stdout open.
    let pid_file_ok = root.path().join("bg-ok.pid");
    let bg_ok_sh = root.path().join("bg-ok-sh");
    write_executable(
        &bg_ok_sh,
        &format!(
            "#!/bin/sh\n/bin/sh -c 'echo $$ > \"{}\"; exec sleep 600' &\ni=0\nwhile [ ! -s \"{}\" ] && [ \"$i\" -lt 100 ]; do i=$((i + 1)); sleep 0.01; done\nPATH='/fake/bg/bin'\n/bin/sh -c \"$2\"\nexit 0\n",
            pid_file_ok.display(),
            pid_file_ok.display(),
        ),
    );

    let started = Instant::now();
    let found = discover_login_shell_path(Some(bg_ok_sh.as_os_str()), Duration::from_secs(5));
    let elapsed = started.elapsed();
    let bg_ok_pid: i32 = std::fs::read_to_string(&pid_file_ok)
        .expect("bg child wrote pid")
        .trim()
        .parse()
        .expect("parse bg pid");
    let ok_alive = process_alive(bg_ok_pid);
    if ok_alive {
        kill_if_alive(bg_ok_pid);
    }
    assert_eq!(found.as_deref(), Some(OsStr::new("/fake/bg/bin")));
    assert!(
        elapsed < Duration::from_secs(2),
        "shell with background stdout holder stalled for {elapsed:?}"
    );
    assert!(
        !ok_alive,
        "background descendant {bg_ok_pid} survived after shell exited 0"
    );

    // 2. Shell exits non-zero while a background child still holds stdout open.
    let pid_file_err = root.path().join("bg-err.pid");
    let bg_err_sh = root.path().join("bg-err-sh");
    write_executable(
        &bg_err_sh,
        &format!(
            "#!/bin/sh\n/bin/sh -c 'echo $$ > \"{}\"; exec sleep 600' &\ni=0\nwhile [ ! -s \"{}\" ] && [ \"$i\" -lt 100 ]; do i=$((i + 1)); sleep 0.01; done\nexit 1\n",
            pid_file_err.display(),
            pid_file_err.display(),
        ),
    );

    let started = Instant::now();
    let res = discover_login_shell_path(Some(bg_err_sh.as_os_str()), Duration::from_secs(5));
    let elapsed = started.elapsed();
    let bg_err_pid: i32 = std::fs::read_to_string(&pid_file_err)
        .expect("bg err child wrote pid")
        .trim()
        .parse()
        .expect("parse bg err pid");
    let err_alive = process_alive(bg_err_pid);
    if err_alive {
        kill_if_alive(bg_err_pid);
    }
    assert_eq!(res, None);
    assert!(
        elapsed < Duration::from_secs(2),
        "failing shell with background stdout holder stalled for {elapsed:?}"
    );
    assert!(
        !err_alive,
        "background descendant {bg_err_pid} survived after shell exited 1"
    );
}

#[tokio::test]
async fn atriumd_merges_login_shell_path_and_skips_under_no_path_extras() {
    let root = folder();
    let atrium_home = root.path().join("atrium");
    let fake_home = root.path().join("home");
    let empty_bin = root.path().join("empty-bin");
    let nvm_bin = fake_home.join(".nvm/versions/node/v22/bin");
    create_private_dir(&atrium_home);
    create_private_dir(&fake_home);
    create_private_dir(&empty_bin);
    create_private_dir(&nvm_bin);
    write_fake_claude(&nvm_bin, "9.8.7");

    // Fake POSIX login shell that prints its own PATH including nvm_bin when
    // invoked with `-ilc ...`.
    let fake_zsh = root.path().join("zsh");
    let args_log = root.path().join("zsh-args.txt");
    write_executable(
        &fake_zsh,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\nPATH='{}:/usr/bin'\n/bin/sh -c \"$2\"\n",
            args_log.display(),
            nvm_bin.display()
        ),
    );

    // 1. Normal start discovers nvm_bin via fake $SHELL.
    let socket = atrium_home.join("daemon.sock");
    let stderr_path = root.path().join("atriumd.stderr");
    let mut daemon = DaemonProcess::spawn(
        &atrium_home,
        &fake_home,
        &empty_bin,
        Some(&fake_zsh),
        &stderr_path,
        false,
    );
    daemon.wait_for_listen(&socket).await;

    let mut wire = Wire::dev(&socket).await;
    let probes: Vec<ProviderProbe> = serde_json::from_value(
        wire.ok(DaemonCommand::ProbeProviders {
            preset: Preset::Home,
        })
        .await,
    )
    .expect("decode ProviderProbe list");
    let claude = probes
        .iter()
        .find(|p| p.provider == Provider::Claude)
        .expect("Claude probe");
    assert_eq!(
        claude
            .enforcement
            .as_ref()
            .and_then(|e| e.cli_version.as_deref()),
        Some("9.8.7")
    );
    let logged_args = std::fs::read_to_string(&args_log).expect("read fake shell args");
    assert!(logged_args.starts_with("-ilc "), "{logged_args:?}");

    wire.ok(DaemonCommand::ShutdownDaemon { force: false })
        .await;
    drop(wire);
    daemon.wait_exit().await;

    // 2. Under ATRIUM_NO_PATH_EXTRAS=1, $SHELL is skipped and nvm_bin is not added.
    let atrium_home_2 = root.path().join("atrium-2");
    create_private_dir(&atrium_home_2);
    let socket_2 = atrium_home_2.join("daemon.sock");
    let stderr_2 = root.path().join("atriumd-2.stderr");
    let mut daemon_2 = DaemonProcess::spawn(
        &atrium_home_2,
        &fake_home,
        &empty_bin,
        Some(&fake_zsh),
        &stderr_2,
        true,
    );
    daemon_2.wait_for_listen(&socket_2).await;

    let mut wire_2 = Wire::dev(&socket_2).await;
    let probes_2: Vec<ProviderProbe> = serde_json::from_value(
        wire_2
            .ok(DaemonCommand::ProbeProviders {
                preset: Preset::Home,
            })
            .await,
    )
    .expect("decode ProviderProbe list");
    let claude_2 = probes_2
        .iter()
        .find(|p| p.provider == Provider::Claude)
        .expect("Claude probe");
    assert_eq!(
        claude_2
            .enforcement
            .as_ref()
            .and_then(|e| e.cli_version.as_deref()),
        None
    );

    wire_2
        .ok(DaemonCommand::ShutdownDaemon { force: false })
        .await;
    drop(wire_2);
    daemon_2.wait_exit().await;
}

#[tokio::test]
async fn atriumd_discovers_fish_login_shell_path_and_never_fails_on_bad_shell() {
    let root = folder();
    let atrium_home = root.path().join("atrium");
    let fake_home = root.path().join("home");
    let empty_bin = root.path().join("empty-bin");
    let mise_bin = fake_home.join(".local/share/mise/shims");
    create_private_dir(&atrium_home);
    create_private_dir(&fake_home);
    create_private_dir(&empty_bin);
    create_private_dir(&mise_bin);
    write_fake_claude(&mise_bin, "9.8.8");

    // Fake `fish` shell: faithfully reproduces real fish behavior where
    // `string join` (even with `-n` / `--no-empty`) always emits a trailing
    // newline, while `printf '...%s...' (string join : -- $PATH)` strips the
    // newline via command substitution. `mise_bin` is placed last so any
    // trailing newline corrupts it and fails the directory-existence filter.
    let fake_fish = root.path().join("fish");
    write_executable(
        &fake_fish,
        &format!(
            "#!/bin/sh\ncase \"$2\" in\n  \"printf '__ATRIUM_PATH_START__%s__ATRIUM_PATH_END__' (string join\"*\")\")\n    printf '__ATRIUM_PATH_START__/usr/bin:%s__ATRIUM_PATH_END__' '{}'\n    ;;\n  *\"string join\"*)\n    printf '__ATRIUM_PATH_START__/usr/bin:%s\\n__ATRIUM_PATH_END__' '{}'\n    ;;\n  *)\n    exit 2\n    ;;\nesac\n",
            mise_bin.display(),
            mise_bin.display(),
        ),
    );

    let socket = atrium_home.join("daemon.sock");
    let stderr_path = root.path().join("atriumd-fish.stderr");
    let mut daemon = DaemonProcess::spawn(
        &atrium_home,
        &fake_home,
        &empty_bin,
        Some(&fake_fish),
        &stderr_path,
        false,
    );
    daemon.wait_for_listen(&socket).await;

    let mut wire = Wire::dev(&socket).await;
    let probes: Vec<ProviderProbe> = serde_json::from_value(
        wire.ok(DaemonCommand::ProbeProviders {
            preset: Preset::Home,
        })
        .await,
    )
    .expect("decode ProviderProbe list");
    let claude = probes
        .iter()
        .find(|p| p.provider == Provider::Claude)
        .expect("Claude probe");
    assert_eq!(
        claude
            .enforcement
            .as_ref()
            .and_then(|e| e.cli_version.as_deref()),
        Some("9.8.8")
    );
    wire.ok(DaemonCommand::ShutdownDaemon { force: false })
        .await;
    drop(wire);
    daemon.wait_exit().await;

    // A failing $SHELL never stops atriumd from starting.
    let atrium_home_bad = root.path().join("atrium-bad-shell");
    create_private_dir(&atrium_home_bad);
    let failing_shell = root.path().join("bad-sh");
    write_executable(&failing_shell, "#!/bin/sh\nexit 1\n");
    let socket_bad = atrium_home_bad.join("daemon.sock");
    let stderr_bad = root.path().join("atriumd-bad.stderr");
    let mut daemon_bad = DaemonProcess::spawn(
        &atrium_home_bad,
        &fake_home,
        &empty_bin,
        Some(&failing_shell),
        &stderr_bad,
        false,
    );
    daemon_bad.wait_for_listen(&socket_bad).await;
    let mut wire_bad = Wire::dev(&socket_bad).await;
    wire_bad
        .ok(DaemonCommand::ShutdownDaemon { force: false })
        .await;
    drop(wire_bad);
    daemon_bad.wait_exit().await;
}
