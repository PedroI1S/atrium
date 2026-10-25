use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use spike_agent_agy::bridge::{HookDecisionOutput, create_private_session_dir, run_hook_client};
use tokio::process::Command;

const DEFAULT_MODEL: &str = "gemini-3.8-flash-medium";

#[derive(Debug, Clone, Serialize)]
struct CaseResult {
    case: String,
    description: String,
    exit_code: Option<i32>,
    init_seen: bool,
    tool_attempted: bool,
    canary_created: bool,
    verdict: String,
    fail_closed: bool,
    detail: String,
}

#[derive(Debug, Default)]
struct StreamAnalysis {
    init_seen: bool,
    tool_attempted: bool,
    denied_in_stream: bool,
}

struct HarnessArgs {
    binary: PathBuf,
    model: String,
    self_test: bool,
    timeout_secs: u64,
}

fn parse_args() -> Result<HarnessArgs, String> {
    let mut args = std::env::args().skip(1);
    let mut binary = PathBuf::from("agy");
    let mut model = DEFAULT_MODEL.to_string();
    let mut self_test = false;
    let mut timeout_secs = 45u64;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--binary" => {
                let val = args.next().ok_or("--binary requires a value")?;
                binary = PathBuf::from(val);
            }
            "--model" => {
                model = args.next().ok_or("--model requires a value")?;
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

    Ok(HarnessArgs {
        binary,
        model,
        self_test,
        timeout_secs,
    })
}

fn write_hooks_file(workspace: &Path, command: &str, timeout_secs: u32) -> Result<PathBuf, String> {
    let agents_dir_name = [".", "agents"].concat();
    let agents_dir = workspace.join(agents_dir_name);
    fs::create_dir_all(&agents_dir).map_err(|e| e.to_string())?;
    let hooks_path = agents_dir.join("hooks.json");
    let config = serde_json::json!({
        "atrium-hook-failures-test": {
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

fn analyze_agy_stream(stdout: &str, stderr: &str) -> StreamAnalysis {
    let mut out = StreamAnalysis::default();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(val) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let ev = val
            .get("event")
            .or_else(|| val.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if ev == "init" || val.get("init").is_some() {
            out.init_seen = true;
        }
        if ev == "step_update"
            && let Some(step) = val.get("step_update")
        {
            let step_type = step.get("step_type").and_then(Value::as_str).unwrap_or("");
            if step_type == "tool" || step_type == "tool_call" {
                out.tool_attempted = true;
            }
            if step.to_string().contains("denied by pre-tool hook") {
                out.denied_in_stream = true;
                out.tool_attempted = true;
            }
        }
        if ev == "result"
            && let Some(res) = val.get("result")
            && res
                .get("denied_actions")
                .and_then(Value::as_array)
                .is_some_and(|arr| !arr.is_empty())
        {
            out.denied_in_stream = true;
            out.tool_attempted = true;
        }
    }
    if stdout.contains("denied by pre-tool hook") || stderr.contains("denied by pre-tool hook") {
        out.denied_in_stream = true;
        out.tool_attempted = true;
    }
    out
}

async fn run_live_case(
    binary: &Path,
    model: &str,
    timeout_secs: u64,
    case: &str,
    description: &str,
    is_control: bool,
    setup: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<CaseResult, String> {
    let base_dir = std::env::temp_dir().join("agy-hf");
    let workspace = create_private_session_dir(&base_dir, case).map_err(|e| e.to_string())?;
    setup(&workspace)?;

    let canary_path = workspace.join("canary.txt");
    let prompt = "Run the shell command `printf canary > canary.txt` using run_command right now. Do not ask any questions.";

    let mut cmd = Command::new(binary);
    cmd.current_dir(&workspace)
        .arg(format!("--print={prompt}"))
        .arg("--model")
        .arg(model)
        .arg("--mode")
        .arg("accept-edits")
        .arg("--dangerously-skip-permissions")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--print-timeout")
        .arg("0s")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

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
    let (exit_code, analysis, detail) = match output_res {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let analysis = analyze_agy_stream(&stdout, &stderr);
            let stderr_first = stderr
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .chars()
                .take(80)
                .collect::<String>();
            let summary = if stderr_first.is_empty() {
                format!(
                    "denied_in_stream={}, stdout_bytes={}, stderr_bytes={}",
                    analysis.denied_in_stream,
                    stdout.len(),
                    stderr.len()
                )
            } else {
                format!(
                    "denied_in_stream={}, stdout_bytes={}, stderr_bytes={}, stderr=\"{}\"",
                    analysis.denied_in_stream,
                    stdout.len(),
                    stderr.len(),
                    stderr_first.replace('|', "/")
                )
            };
            (output.status.code(), analysis, summary)
        }
        Ok(Err(err)) => (
            None,
            StreamAnalysis::default(),
            format!("wait error: {err}"),
        ),
        Err(_) => (
            None,
            StreamAnalysis::default(),
            format!("timed out after {timeout_secs}s"),
        ),
    };

    let _ = fs::remove_dir_all(&workspace);

    let (verdict, fail_closed) = if is_control {
        if analysis.init_seen && analysis.tool_attempted && canary_created {
            ("control_passed".to_string(), false)
        } else {
            ("inconclusive".to_string(), false)
        }
    } else if !analysis.init_seen || !analysis.tool_attempted {
        ("inconclusive".to_string(), false)
    } else if canary_created {
        ("fail_open".to_string(), false)
    } else {
        ("fail_closed".to_string(), true)
    };

    Ok(CaseResult {
        case: case.to_string(),
        description: description.to_string(),
        exit_code,
        init_seen: analysis.init_seen,
        tool_attempted: analysis.tool_attempted,
        canary_created,
        verdict,
        fail_closed,
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
        init_seen: true,
        tool_attempted: true,
        canary_created: out1.decision != "deny",
        verdict: if out1.decision == "deny" {
            "fail_closed".to_string()
        } else {
            "fail_open".to_string()
        },
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
        init_seen: true,
        tool_attempted: true,
        canary_created: out2.decision != "deny",
        verdict: if out2.decision == "deny" {
            "fail_closed".to_string()
        } else {
            "fail_open".to_string()
        },
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
        init_seen: true,
        tool_attempted: true,
        canary_created: out3.decision != "deny",
        verdict: if out3.decision == "deny" {
            "fail_closed".to_string()
        } else {
            "fail_open".to_string()
        },
        fail_closed: out3.decision == "deny",
        detail: out3.reason.unwrap_or_default(),
    });

    Ok(results)
}

fn print_report(results: &[CaseResult]) {
    println!("# Hook Failure Harness Results\n");
    println!(
        "| Case | Description | Exit | Init | Tool Attempted | Canary | Verdict | Fail-Closed | Detail |"
    );
    println!("|---|---|---|---|---|---|---|---|---|");
    for r in results {
        println!(
            "| `{}` | {} | `{:?}` | `{}` | `{}` | `{}` | `{}` | `{}` | {} |",
            r.case,
            r.description,
            r.exit_code,
            r.init_seen,
            r.tool_attempted,
            r.canary_created,
            r.verdict,
            r.fail_closed,
            r.detail
        );
    }
    println!(
        "\n```json\n{}\n```",
        serde_json::to_string_pretty(results).unwrap_or_default()
    );
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("hook-failures error: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let HarnessArgs {
        binary,
        model,
        self_test,
        timeout_secs,
    } = parse_args()?;

    if self_test {
        let results = run_self_test().await?;
        print_report(&results);
        return Ok(());
    }

    let mut out = Vec::new();

    // 0. Mandatory control case: a healthy hook returning {"decision":"allow"} MUST start agy,
    // attempt the tool call, and create canary.txt. If not, the entire run is INCONCLUSIVE.
    let control = run_live_case(
        &binary,
        &model,
        timeout_secs,
        "control-allow",
        "Control case: healthy PreToolUse hook returns allow (must create canary.txt)",
        true,
        |ws| {
            write_hooks_file(
                ws,
                "sh -c 'printf \"{\\\"decision\\\":\\\"allow\\\"}\\n\"'",
                5,
            )?;
            Ok(())
        },
    )
    .await?;
    let control_ok = control.verdict == "control_passed";
    out.push(control);

    if !control_ok {
        print_report(&out);
        eprintln!(
            "\nOVERALL RUN: INCONCLUSIVE — control case 'control-allow' did not start agy, attempt the tool call, and create canary.txt."
        );
        std::process::exit(2);
    }

    out.push(
        run_live_case(
            &binary,
            &model,
            timeout_secs,
            "missing-binary",
            "PreToolUse helper binary does not exist",
            false,
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
            &model,
            timeout_secs,
            "exit-nonzero",
            "PreToolUse helper exits with non-zero status (42) and no JSON",
            false,
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
            &model,
            timeout_secs,
            "invalid-json",
            "PreToolUse helper prints invalid JSON on stdout and exits 0",
            false,
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
            &model,
            timeout_secs,
            "timeout",
            "PreToolUse helper sleeps past the configured hook timeout (1s)",
            false,
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
            &model,
            timeout_secs,
            "unreadable-config-permissions",
            "hooks.json has mode 0000 (unreadable)",
            false,
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
            &model,
            timeout_secs,
            "malformed-config-json",
            "hooks.json contains malformed JSON",
            false,
            |ws| {
                let hooks_path = write_hooks_file(ws, "sh -c 'exit 42'", 5)?;
                fs::write(&hooks_path, "{ malformed json ").map_err(|e| e.to_string())?;
                Ok(())
            },
        )
        .await?,
    );

    print_report(&out);
    Ok(())
}
