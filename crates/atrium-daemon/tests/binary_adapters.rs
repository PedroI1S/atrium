//! Smoke test against the compiled `atriumd` binary: verifies that the real
//! daemon registers both native adapters (`Claude` and `Codex`) without
//! invoking any real provider CLI.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use atrium_daemon::NOTIFY_FAILED_SETTING;
use atrium_protocol::agent::{AgentEvent, Provider};
use atrium_protocol::policy::{EnforcementLevel, Preset};
use atrium_protocol::thread::{ThreadState, ThreadSummary};
use atrium_protocol::wire::{AgentLaunch, DaemonCommand, ErrorCode, ProviderProbe};
use common::{SOON, Wire, folder};
use serde_json::json;

struct DaemonProcess {
    child: Option<Child>,
}

impl DaemonProcess {
    fn spawn(atrium_home: &Path, home: &Path, empty_path: &Path) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_atriumd"))
            .arg("--dev-socket")
            .env("ATRIUM_HOME", atrium_home)
            .env("HOME", home)
            .env("PATH", empty_path)
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn atriumd");
        Self { child: Some(child) }
    }

    async fn wait_for_socket(&mut self, socket: &Path) {
        let deadline = tokio::time::Instant::now() + SOON;
        loop {
            if socket.exists() {
                return;
            }
            if let Some(child) = self.child.as_mut()
                && let Some(status) = child.try_wait().expect("poll atriumd")
            {
                let child = self.child.take().expect("child present");
                let out = child.wait_with_output().expect("read stderr");
                let stderr = String::from_utf8_lossy(&out.stderr);
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
                assert!(status.success(), "atriumd exited with {status}");
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
    let mut daemon = DaemonProcess::spawn(&atrium_home, &fake_home, &empty_bin);
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

        let history = wire.history(&summary.thread_id).await;
        let has_spawn_error = history.iter().any(|stored| {
            matches!(
                &stored.envelope.event,
                AgentEvent::Error {
                    fatal: true,
                    message,
                    ..
                } if message.contains("failed to start the provider")
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
