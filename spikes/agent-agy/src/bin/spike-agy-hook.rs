use std::io::{self, Read};
use std::path::PathBuf;
use std::time::Duration;

use spike_agent_agy::bridge::{DEFAULT_HOOK_CLIENT_TIMEOUT, HookDecisionOutput, run_hook_client};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut socket_path: Option<PathBuf> =
        std::env::var_os("ATRIUM_AGY_GATE_SOCKET").map(PathBuf::from);
    let mut timeout = DEFAULT_HOOK_CLIENT_TIMEOUT;
    let mut simulate_exit_nonzero = false;
    let mut simulate_invalid_json = false;
    let mut simulate_sleep_ms: Option<u64> = None;

    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--socket" => {
                if let Some(val) = args.get(idx + 1) {
                    socket_path = Some(PathBuf::from(val));
                    idx += 2;
                } else {
                    emit_and_exit(&HookDecisionOutput::deny("missing value for --socket"));
                    return;
                }
            }
            "--timeout-ms" => {
                if let Some(val) = args.get(idx + 1).and_then(|s| s.parse::<u64>().ok()) {
                    timeout = Duration::from_millis(val);
                    idx += 2;
                } else {
                    emit_and_exit(&HookDecisionOutput::deny("invalid --timeout-ms"));
                    return;
                }
            }
            "--simulate-exit-nonzero" => {
                simulate_exit_nonzero = true;
                idx += 1;
            }
            "--simulate-invalid-json" => {
                simulate_invalid_json = true;
                idx += 1;
            }
            "--simulate-sleep-ms" => {
                if let Some(val) = args.get(idx + 1).and_then(|s| s.parse::<u64>().ok()) {
                    simulate_sleep_ms = Some(val);
                    idx += 2;
                } else {
                    emit_and_exit(&HookDecisionOutput::deny("invalid --simulate-sleep-ms"));
                    return;
                }
            }
            _ => {
                idx += 1;
            }
        }
    }

    if simulate_exit_nonzero {
        std::process::exit(1);
    }

    if simulate_invalid_json {
        println!("<<<NOT_VALID_JSON>>>");
        return;
    }

    if let Some(ms) = simulate_sleep_ms {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    let Some(sock) = socket_path else {
        emit_and_exit(&HookDecisionOutput::deny(
            "no gate socket path provided to spike-agy-hook",
        ));
        return;
    };

    let mut stdin_bytes = Vec::new();
    if let Err(e) = io::stdin()
        .take(10 * 1024 * 1024)
        .read_to_end(&mut stdin_bytes)
    {
        emit_and_exit(&HookDecisionOutput::deny(format!(
            "failed to read hook stdin: {e}"
        )));
        return;
    }

    let decision = run_hook_client(&sock, &stdin_bytes, timeout).await;
    emit_and_exit(&decision);
}

fn emit_and_exit(output: &HookDecisionOutput) {
    match serde_json::to_string(output) {
        Ok(json) => println!("{json}"),
        Err(_) => println!("{{\"decision\":\"deny\",\"reason\":\"serialization error\"}}"),
    }
}
