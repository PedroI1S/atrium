//! Smoke test against the compiled `atriumd` binary: verifies that the real
//! daemon registers both native adapters (`Claude` and `Codex`) without
//! invoking any real provider CLI.

mod common;

use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use atrium_daemon::{NO_PATH_EXTRAS_ENV, NOTIFY_FAILED_SETTING};
use atrium_protocol::agent::{AgentEvent, Provider};
use atrium_protocol::policy::{EnforcementLevel, Preset};
use atrium_protocol::thread::{ThreadState, ThreadSummary};
use atrium_protocol::wire::{AgentLaunch, DaemonCommand, ErrorCode, ProviderProbe};
use common::{SOON, Wire, folder};
use serde_json::json;

struct DaemonProcess {
    child: Option<Child>,
    stderr_path: PathBuf,
}

impl DaemonProcess {
    fn spawn(
        atrium_home: &Path,
        home: &Path,
        path: &Path,
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

    async fn wait_for_socket(&mut self, socket: &Path) {
        let deadline = tokio::time::Instant::now() + SOON;
        loop {
            if socket.exists() && tokio::net::UnixStream::connect(socket).await.is_ok() {
                return;
            }
            if let Some(child) = self.child.as_mut()
                && let Some(status) = child.try_wait().expect("poll atriumd")
            {
                self.child = None;
                let stderr = std::fs::read_to_string(&self.stderr_path).unwrap_or_default();
                panic!("atriumd exited early ({status}) before creating socket: {stderr}");
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for socket at {}",
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

#[tokio::test]
async fn atriumd_registers_claude_and_codex_adapters() {
    let root = folder();
    let atrium_home = root.path().join("atrium");
    let fake_home = root.path().join("home");
    let empty_bin = root.path().join("empty-bin");
    create_private_dir(&atrium_home);
    create_private_dir(&fake_home);
    create_private_dir(&empty_bin);

    let socket = atrium_home.join("daemon.sock");
    let stderr_path = root.path().join("atriumd.stderr");
    let mut daemon = DaemonProcess::spawn(&atrium_home, &fake_home, &empty_bin, &stderr_path, true);
    daemon.wait_for_socket(&socket).await;

    let mut wire = Wire::dev(&socket).await;
    // Avoid firing OS desktop notifications when the missing CLI fails a thread.
    wire.ok(DaemonCommand::SetSetting {
        key: NOTIFY_FAILED_SETTING.into(),
        value: json!(false),
    })
    .await;

    let probes: Vec<ProviderProbe> = serde_json::from_value(
        wire.ok(DaemonCommand::ProbeProviders {
            preset: Preset::Home,
        })
        .await,
    )
    .expect("decode ProviderProbe list");

    let providers: Vec<Provider> = probes.iter().map(|probe| probe.provider).collect();
    assert_eq!(
        providers,
        vec![Provider::Claude, Provider::Codex],
        "expected both Claude and Codex in ProbeProviders, got {probes:?}"
    );
    for probe in &probes {
        let descriptor = probe
            .enforcement
            .as_ref()
            .unwrap_or_else(|| panic!("expected enforcement descriptor for {:?}", probe.provider));
        assert_eq!(descriptor.provider, probe.provider);
        assert_eq!(descriptor.cli_version, None);
        assert_eq!(descriptor.level, EnforcementLevel::Partial);
        assert_eq!(probe.error, None);
    }

    for provider in [Provider::Claude, Provider::Codex] {
        let answer = wire
            .command(DaemonCommand::StartThread {
                agent: AgentLaunch::Native {
                    provider,
                    model: None,
                    effort: None,
                    preset: Preset::Home,
                },
                message: None,
            })
            .await;

        let summary: ThreadSummary = match answer {
            Ok(value) => serde_json::from_value(value).expect("decode ThreadSummary"),
            Err((ErrorCode::Invalid, message)) if message.contains("no adapter for") => {
                panic!("StartThread({provider:?}) did not find a registered adapter: {message}");
            }
            Err((code, message)) => {
                panic!("StartThread({provider:?}) failed unexpectedly ({code:?}): {message}");
            }
        };

        let failed = wire
            .thread_when(&summary.thread_id, |state| {
                matches!(state.state, ThreadState::Failed { .. })
            })
            .await;
        assert_eq!(failed.provider, provider);

        let missing_cause = std::fs::File::open(root.path().join("definitely-missing"))
            .unwrap_err()
            .to_string();
        let expected_error = format!("failed to start the provider: {missing_cause}");
        let history = wire.history(&summary.thread_id).await;
        let has_spawn_error = history.iter().any(|stored| {
            matches!(
                &stored.envelope.event,
                AgentEvent::Error {
                    fatal: true,
                    message,
                    ..
                } if *message == expected_error
            )
        });
        assert!(
            has_spawn_error,
            "expected adapter spawn failure in history for {provider:?}, got {history:?}"
        );
    }

    wire.ok(DaemonCommand::ShutdownDaemon { force: false })
        .await;
    drop(wire);
    daemon.wait_exit().await;
}

#[tokio::test]
async fn atriumd_discovers_claude_in_home_local_bin_when_path_minimal() {
    let root = folder();
    let atrium_home = root.path().join("atrium");
    let fake_home = root.path().join("home");
    let empty_bin = root.path().join("empty-bin");
    let local_bin = fake_home.join(".local/bin");
    create_private_dir(&atrium_home);
    create_private_dir(&fake_home);
    create_private_dir(&empty_bin);
    create_private_dir(&local_bin);

    // Fake CLI that only answers `--version` for ProbeProviders and refuses
    // anything else, so it can never act as a real provider.
    let fake_claude = local_bin.join("claude");
    std::fs::write(
        &fake_claude,
        "#!/bin/sh\nif [ \"$#\" -eq 1 ] && [ \"$1\" = \"--version\" ]; then\n    printf '9.9.9 (Claude Code)\\n'\n    exit 0\nfi\nexit 1\n",
    )
    .expect("write fake claude");
    std::fs::set_permissions(&fake_claude, std::fs::Permissions::from_mode(0o700))
        .expect("chmod fake claude");

    let socket = atrium_home.join("daemon.sock");
    let stderr_path = root.path().join("atriumd.stderr");
    let mut daemon =
        DaemonProcess::spawn(&atrium_home, &fake_home, &empty_bin, &stderr_path, false);
    daemon.wait_for_socket(&socket).await;

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
        .find(|probe| probe.provider == Provider::Claude)
        .unwrap_or_else(|| panic!("expected Claude in ProbeProviders, got {probes:?}"));
    assert_eq!(claude.error, None);
    let descriptor = claude
        .enforcement
        .as_ref()
        .expect("expected enforcement descriptor for Claude");
    assert_eq!(descriptor.cli_version.as_deref(), Some("9.9.9"));

    wire.ok(DaemonCommand::ShutdownDaemon { force: false })
        .await;
    drop(wire);
    daemon.wait_exit().await;
}
