use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use spike_agent_agy::bridge::{HookDecisionOutput, run_hook_client};
use tokio::process::Command;

#[derive(Debug, Clone, Serialize)]
struct CaseResult {
    case: String,
    description: String,
    exit_code: Option<i32>,
    canary_created: bool,
    fail_closed: bool,
    detail: String,
}

fn parse_args() -> Result<(PathBuf, Option<String>, bool, u64), String> {
    let mut args = std::env::args().skip(1);
    let mut binary = PathBuf::from("agy");
    let mut model = None;
    let mut self_test = false;
    let mut timeout_secs = 45u64;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--binary" => {
                let val = args.next().ok_or("--binary requires a value")?;
                binary = PathBuf::from(val);
            }
            "--model" => {
                model = Some(args.next().ok_or("--model requires a value")?);
            }
            "--self-test" => {
                self_test = true;
            }
            "--timeout-secs" => {
                let val = args.next().ok_or("--timeout-secs requires a value")?;
                timeout_secs = val
                    .parse::<u64>()
                    .map_err(|e| format!("invalid --timeout-secs: {e}"))?;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    Ok((binary, model, self_test, timeout_secs))
}

fn write_hooks_file(workspace: &Path, command: &str, timeout_secs: u32) -> Result<PathBuf, String> {
    let agents_dir_name = [".", "agents"].concat();
    let agents_dir = workspace.join(agents_dir_name);
    fs::create_dir_all(&agents_dir).map_err(|e| e.to_string())?;
    let hooks_path = agents_dir.join("hooks.json");
    let config = serde_json::json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "*",
                    "hooks": [
                        {
                            "type": "command",
                            "command": command,
                            "timeout": timeout_secs
                        }
                    ]
                }
            ]
        }
    });
    let text = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    fs::write(&hooks_path, text).map_err(|e| e.to_string())?;
    Ok(hooks_path)
}

async fn run_live_case(
    binary: &Path,
    model: Option<&str>,
    timeout_secs: u64,
    case: &str,
    description: &str,
    setup: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<CaseResult, String> {
    let unique = format!(
        "atrium-agy-hook-failure-{}-{}-{}",
        case,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let workspace = std::env::temp_dir().join(unique);
    fs::create_dir_all(&workspace).map_err(|e| e.to_string())?;
    setup(&workspace)?;

    let canary_path = workspace.join("canary.txt");
    let prompt = "Run the shell command `printf canary > canary.txt` using run_command right now. Do not ask questions.";

    let mut cmd = Command::new(binary);
    cmd.current_dir(&workspace)
        .arg("-p")
        .arg(prompt)
        .arg("--mode")
        .arg("agent")
        .arg("--dangerously-skip-permissions")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--workspace")
        .arg(&workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(m) = model {
        cmd.arg("--model").arg(m);
    }

    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn {}: {e}", binary.display()))?;

    let output_res =
        tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output()).await;

    // Restore permissions in case unreadable-config chmodded 0000
    let agents_dir = workspace.join([".", "agents"].concat());
    let hooks_file = agents_dir.join("hooks.json");
    if hooks_file.exists() {
        let _ = fs::set_permissions(&hooks_file, fs::Permissions::from_mode(0o600));
    }

    let canary_created = canary_path.exists();
    let (exit_code, detail) = match output_res {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let denied = stdout.contains("denied by pre-tool hook")
                || stdout.contains("denied_actions")
                || stderr.contains("denied");
            let summary = format!(
                "denied_in_stream={denied}, stdout_bytes={}, stderr_bytes={}",
                stdout.len(),
                stderr.len()
            );
            (output.status.code(), summary)
        }
        Ok(Err(err)) => (None, format!("wait error: {err}")),
        Err(_) => (None, format!("timed out after {timeout_secs}s")),
    };

    let _ = fs::remove_dir_all(&workspace);

    Ok(CaseResult {
        case: case.to_string(),
        description: description.to_string(),
        exit_code,
        canary_created,
        fail_closed: !canary_created,
        detail,
    })
}

async fn run_self_test() -> Result<Vec<CaseResult>, String> {
    let mut results = Vec::new();

    // 1. Missing socket -> run_hook_client fails closed
    let missing_sock = std::env::temp_dir().join(format!(
        "atrium-agy-missing-sock-{}.sock",
        std::process::id()
    ));
    let out1: HookDecisionOutput = run_hook_client(
        &missing_sock,
        br#"{"toolCall":{"name":"run_command","args":{"CommandLine":"ls"}}}"#,
        Duration::from_millis(200),
    )
    .await;
    results.push(CaseResult {
        case: "bridge-missing-socket".to_string(),
        description: "spike-agy-hook cannot connect to missing Unix socket".to_string(),
        exit_code: Some(0),
        canary_created: out1.decision != "deny",
        fail_closed: out1.decision == "deny",
        detail: out1.reason.unwrap_or_default(),
    });

    // 2. Malformed stdin JSON -> run_hook_client fails closed
    let out2: HookDecisionOutput =
        run_hook_client(&missing_sock, b"not-json{{{", Duration::from_millis(200)).await;
    results.push(CaseResult {
        case: "bridge-malformed-stdin".to_string(),
        description: "spike-agy-hook receives malformed JSON on stdin".to_string(),
        exit_code: Some(0),
        canary_created: out2.decision != "deny",
        fail_closed: out2.decision == "deny",
        detail: out2.reason.unwrap_or_default(),
    });

    // 3. Empty stdin -> run_hook_client fails closed
    let out3: HookDecisionOutput =
        run_hook_client(&missing_sock, b"   ", Duration::from_millis(200)).await;
    results.push(CaseResult {
        case: "bridge-empty-stdin".to_string(),
        description: "spike-agy-hook receives empty payload on stdin".to_string(),
        exit_code: Some(0),
        canary_created: out3.decision != "deny",
        fail_closed: out3.decision == "deny",
        detail: out3.reason.unwrap_or_default(),
    });

    Ok(results)
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("hook-failures error: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let (binary, model, self_test, timeout_secs) = parse_args()?;
    let results = if self_test {
        run_self_test().await?
    } else {
        let model_ref = model.as_deref();
        let mut out = Vec::new();

        out.push(
            run_live_case(
                &binary,
                model_ref,
                timeout_secs,
                "missing-binary",
                "PreToolUse helper binary does not exist",
                |ws| {
                    let missing = ws.join("nonexistent-spike-agy-hook");
                    write_hooks_file(ws, &missing.to_string_lossy(), 5)?;
                    Ok(())
                },
            )
            .await?,
        );

        out.push(
            run_live_case(
                &binary,
                model_ref,
                timeout_secs,
                "exit-nonzero",
                "PreToolUse helper exits with non-zero status (42) and no JSON",
                |ws| {
                    write_hooks_file(ws, "sh -c 'exit 42'", 5)?;
                    Ok(())
                },
            )
            .await?,
        );

        out.push(
            run_live_case(
                &binary,
                model_ref,
                timeout_secs,
                "invalid-json",
                "PreToolUse helper prints invalid JSON on stdout and exits 0",
                |ws| {
                    write_hooks_file(ws, "sh -c 'printf not-valid-json'", 5)?;
                    Ok(())
                },
            )
            .await?,
        );

        out.push(
            run_live_case(
                &binary,
                model_ref,
                timeout_secs,
                "timeout",
                "PreToolUse helper sleeps past the configured hook timeout (1s)",
                |ws| {
                    write_hooks_file(ws, "sh -c 'sleep 5'", 1)?;
                    Ok(())
                },
            )
            .await?,
        );

        out.push(
            run_live_case(
                &binary,
                model_ref,
                timeout_secs,
                "unreadable-config-permissions",
                "hooks.json has mode 0000 (unreadable)",
                |ws| {
                    let hooks_path = write_hooks_file(ws, "sh -c 'exit 42'", 5)?;
                    fs::set_permissions(&hooks_path, fs::Permissions::from_mode(0o000))
                        .map_err(|e| e.to_string())?;
                    Ok(())
                },
            )
            .await?,
        );

        out.push(
            run_live_case(
                &binary,
                model_ref,
                timeout_secs,
                "malformed-config-json",
                "hooks.json contains malformed JSON",
                |ws| {
                    let hooks_path = write_hooks_file(ws, "sh -c 'exit 42'", 5)?;
                    fs::write(&hooks_path, "{ malformed json ").map_err(|e| e.to_string())?;
                    Ok(())
                },
            )
            .await?,
        );

        out
    };

    println!("# Hook Failure Harness Results\n");
    println!("| Case | Description | Exit Code | Canary Created | Fail-Closed | Detail |");
    println!("|---|---|---|---|---|---|");
    for r in &results {
        println!(
            "| `{}` | {} | `{:?}` | `{}` | `{}` | {} |",
            r.case, r.description, r.exit_code, r.canary_created, r.fail_closed, r.detail
        );
    }
    println!(
        "\n```json\n{}\n```",
        serde_json::to_string_pretty(&results).unwrap_or_default()
    );

    Ok(())
}
