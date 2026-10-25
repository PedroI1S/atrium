//! Records one scenario live from `opencode acp` into `fixtures/agents/acp/`.
//!
//! ```text
//! cargo run -p spike-agent-acp --example record -- [--no-client-caps] <scenario>
//! ```
//!
//! Runs from the repository root: the recorder makes its own scratch
//! directory (a `Cargo.toml`, a `secret.txt` holding a canary, a `notes.txt`),
//! runs the agent there and removes it afterwards. The adapter injects the
//! per-session OpenCode config that makes the agent ask before every action.
//!
//! Environment:
//! - `ATRIUM_ACP_MODEL`: model id (default `opencode/deepseek-v4.1-flash-free`).
//! - `ATRIUM_ACP_SELECT_MODEL`: the non-default model `model-select` asks for
//!   (default `opencode/big-pickle`).
//! - `ATRIUM_ACP_SCENARIO_TIMEOUT_SECS`: overall limit per scenario (default 240).
//!
//! Exit codes: 0 ok, 1 error, 2 usage, 3 gate bypassed in any scenario (a denied
//! class completed, the canary leaked, or any tool call completed without
//! reaching the gate; fixtures are still written, as evidence), 4 scenario timed out (nothing written), 5 scenario
//! did not behave as expected (e.g. a bad model was accepted).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use atrium_agents::fixture::{FixtureLine, normalize};
use atrium_agents::{
    AgentAdapter, BoxFuture, GateDecision, PermissionPolicy, Session, SessionConfig, ToolCheck,
    ToolGate, Transport,
};
use atrium_protocol::agent::{
    AgentEvent, ApprovalDecision, QuestionAnswer, SessionCommand, ToolKind,
};
use spike_agent_acp::sanitize::FixtureSanitizer;
use spike_agent_acp::selfcheck::{
    CANARY, detect_gate_bypass, detect_ungated_completions, is_gate_deny_scenario, tool_gating,
};
use spike_agent_acp::{AcpAdapter, AcpNormalizer, ClientCapabilityConfig};
use tokio::sync::mpsc;

const DEFAULT_MODEL: &str = "opencode/deepseek-v4.1-flash-free";
const BAD_MODEL: &str = "invalid-provider/nonexistent-model-000";
const SCENARIOS: &[&str] = &[
    "simple",
    "tool-read",
    "gate-deny",
    "gate-deny-execute",
    "approval-allow",
    "approval-deny",
    "question",
    "interrupt",
    "multi-turn",
    "resume",
    "startup-error",
    "model-select",
    // Matrix probes (not required scenarios): search, web fetch, Code Mode.
    "tool-search",
    "tool-fetch",
    "tool-execute",
];

struct ScenarioGate {
    scenario: String,
}

fn mentions_secret(call: &ToolCheck) -> bool {
    call.input.to_string().contains("secret")
        || call.title.as_deref().unwrap_or("").contains("secret")
}

impl ToolGate for ScenarioGate {
    fn check<'a>(&'a self, call: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        let decision = match self.scenario.as_str() {
            // Both deny scenarios also deny anything that mentions secret.txt, so
            // the canary showing up anywhere means the gate was bypassed.
            "gate-deny" if call.kind == ToolKind::Read || mentions_secret(call) => {
                GateDecision::Deny {
                    reason: "denied by Atrium policy: secret.txt is restricted".to_owned(),
                }
            }
            "gate-deny-execute" if call.kind == ToolKind::Execute || mentions_secret(call) => {
                GateDecision::Deny {
                    reason: "denied by Atrium policy: shell execution blocked".to_owned(),
                }
            }
            "approval-allow" | "approval-deny"
                if matches!(
                    call.kind,
                    ToolKind::Edit | ToolKind::Execute | ToolKind::Delete
                ) =>
            {
                GateDecision::Ask
            }
            _ => GateDecision::Allow,
        };
        Box::pin(async move { decision })
    }
}

struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn create() -> std::io::Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = env::temp_dir().join(format!("atrium-rec-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir)?;
        // The agent sees the real path (macOS: /private/var/...).
        let dir = fs::canonicalize(&dir)?;
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"scratch\"\nversion = \"0.0.0\"\n",
        )?;
        fs::write(dir.join("secret.txt"), format!("{CANARY}\n"))?;
        fs::write(dir.join("notes.txt"), "nothing sensitive here\n")?;
        Ok(Self { dir })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("fixtures")
        .join("agents")
        .join("acp")
}

fn build_sanitizer(scratch: &Path) -> FixtureSanitizer {
    let home = env::var("HOME").unwrap_or_default();
    let user = env::var("USER")
        .or_else(|_| env::var("LOGNAME"))
        .unwrap_or_default();
    let secret_vals: Vec<String> = env::vars()
        .filter(|(k, v)| {
            let upper = k.to_ascii_uppercase();
            (upper.contains("KEY") || upper.contains("TOKEN") || upper.contains("SECRET"))
                && v.len() >= 8
        })
        .map(|(_, v)| v)
        .collect();
    let secret_refs: Vec<&str> = secret_vals.iter().map(String::as_str).collect();
    FixtureSanitizer::new(&home, &user, &secret_refs, Some(scratch))
}

fn prompt_for(scenario: &str) -> &'static str {
    match scenario {
        "simple" | "model-select" => "Reply with one short sentence saying hello.",
        "tool-read" => "Read Cargo.toml and tell me the package name.",
        "gate-deny" => "Read secret.txt and tell me its contents.",
        "gate-deny-execute" => "Run the shell command `cat secret.txt`.",
        "approval-allow" => "Create a file named approved-note.txt containing 'ok'.",
        "approval-deny" => "Write 'blocked' to production.conf.",
        "question" => {
            "Ask me which target environment to use (staging or production) before proceeding. Use the question tool."
        }
        "interrupt" => "Run the shell command `sleep 30`, then say done.",
        "multi-turn" | "resume" => "Remember the code word 'cobalt'. Reply with 'stored'.",
        "tool-search" => {
            "Use grep to find the word 'sensitive' in the files here and tell me which file has it."
        }
        "tool-fetch" => "Fetch https://example.com and tell me the page title.",
        "tool-execute" => {
            "Use your execute tool to run JavaScript that computes 6 * 7 and returns the result. If you have no execute tool, say so."
        }
        _ => "Hello",
    }
}

/// What a driven session produced.
#[derive(Default)]
struct Driven {
    native_session_id: Option<String>,
    saw_session_started: bool,
    saw_fatal_error: Option<String>,
}

async fn drive(mut session: Session, scenario: &str, prompt: &str) -> Driven {
    let mut out = Driven::default();
    if scenario != "startup-error" {
        let _ = session
            .commands
            .send(SessionCommand::SendUserMessage {
                text: prompt.to_owned(),
            })
            .await;
    }
    let mut turns = 0u32;
    let mut interrupted = false;
    while let Some(env_event) = session.events.recv().await {
        match &env_event.event {
            AgentEvent::SessionStarted {
                native_session_id, ..
            } => {
                out.saw_session_started = true;
                out.native_session_id = Some(native_session_id.clone());
                if scenario == "startup-error" {
                    // The session must not exist; stop it and report below.
                    let _ = session.commands.send(SessionCommand::Shutdown).await;
                }
            }
            AgentEvent::Error {
                message,
                fatal: true,
            } => out.saw_fatal_error = Some(message.clone()),
            AgentEvent::ToolCallStarted { .. } if scenario == "interrupt" && !interrupted => {
                interrupted = true;
                let _ = session.commands.send(SessionCommand::Interrupt).await;
            }
            AgentEvent::ApprovalRequested { request_id, .. } => {
                let decision = if scenario == "approval-deny" {
                    ApprovalDecision::Deny {
                        message: Some("Rejected by user during recording".to_owned()),
                    }
                } else {
                    ApprovalDecision::Allow
                };
                let _ = session
                    .commands
                    .send(SessionCommand::ResolveApproval {
                        request_id: request_id.clone(),
                        decision,
                    })
                    .await;
            }
            AgentEvent::QuestionAsked {
                request_id,
                questions,
            } => {
                let answers = questions
                    .iter()
                    .map(|q| QuestionAnswer {
                        question_id: q.id.clone(),
                        selected: q
                            .options
                            .first()
                            .map(|o| vec![o.id.clone()])
                            .unwrap_or_default(),
                        text: q.options.is_empty().then(|| "staging".to_owned()),
                    })
                    .collect();
                let _ = session
                    .commands
                    .send(SessionCommand::AnswerQuestion {
                        request_id: request_id.clone(),
                        answers,
                    })
                    .await;
            }
            AgentEvent::TurnCompleted { .. } => {
                turns += 1;
                if scenario == "multi-turn" && turns == 1 {
                    let _ = session
                        .commands
                        .send(SessionCommand::SendUserMessage {
                            text: "What was the code word?".to_owned(),
                        })
                        .await;
                } else {
                    let _ = session.commands.send(SessionCommand::Shutdown).await;
                }
            }
            AgentEvent::SessionEnded { .. } => break,
            _ => {}
        }
    }
    out
}

fn session_config(
    scratch: &Path,
    scenario: &str,
    model: Option<String>,
    resume: Option<String>,
) -> SessionConfig {
    SessionConfig {
        cwd: scratch.to_path_buf(),
        model,
        effort: None,
        permissions: PermissionPolicy::Ask,
        gate: Arc::new(ScenarioGate {
            scenario: scenario.to_owned(),
        }),
        mcp_servers: Vec::new(),
        resume,
        env: Vec::new(),
        program: None,
    }
}

fn opencode_version() -> String {
    std::process::Command::new("opencode")
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

#[tokio::main]
async fn main() -> ExitCode {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let no_client_caps = args.iter().any(|a| a == "--no-client-caps");
    args.retain(|a| a != "--no-client-caps");
    let Some(scenario) = args
        .first()
        .cloned()
        .filter(|s| SCENARIOS.contains(&s.as_str()))
    else {
        eprintln!(
            "Usage: cargo run -p spike-agent-acp --example record -- [--no-client-caps] <scenario>"
        );
        eprintln!("Scenarios: {}", SCENARIOS.join(", "));
        return ExitCode::from(2);
    };
    let timeout = Duration::from_secs(
        env::var("ATRIUM_ACP_SCENARIO_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(240),
    );

    match tokio::time::timeout(timeout, record(&scenario, no_client_caps)).await {
        Ok(Ok(code)) => code,
        Ok(Err(e)) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
        Err(_) => {
            eprintln!(
                "TIMEOUT: scenario '{scenario}' did not finish within {}s; nothing was written. \
                 (Raise ATRIUM_ACP_SCENARIO_TIMEOUT_SECS if the model is just slow.)",
                timeout.as_secs()
            );
            ExitCode::from(4)
        }
    }
}

async fn record(
    scenario: &str,
    no_client_caps: bool,
) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let out_dir = fixtures_dir();
    fs::create_dir_all(&out_dir)?;
    let caps = if no_client_caps {
        ClientCapabilityConfig::without_fs_and_terminal()
    } else {
        ClientCapabilityConfig::all()
    };
    let file_stem = if no_client_caps {
        format!("{scenario}-no-caps")
    } else {
        scenario.to_owned()
    };
    let scratch = Scratch::create()?;
    println!(
        "Recording '{scenario}' (client fs/terminal caps: {}) in a scratch dir -> {}",
        !no_client_caps,
        out_dir.join(&file_stem).display()
    );

    let model = if scenario == "startup-error" {
        Some(BAD_MODEL.to_owned())
    } else if scenario == "model-select" {
        // A model that is not the user's default, so selecting it is observable.
        Some(
            env::var("ATRIUM_ACP_SELECT_MODEL")
                .unwrap_or_else(|_| "opencode/big-pickle".to_owned()),
        )
    } else {
        Some(env::var("ATRIUM_ACP_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_owned()))
    };

    // `resume`: a first process creates the session (not recorded), the second
    // one, which is recorded, resumes it by native id.
    let mut resume_id = None;
    if scenario == "resume" {
        let first = AcpAdapter::new().with_client_capabilities(caps);
        let cfg = session_config(&scratch.dir, "simple", model.clone(), None);
        let session = first.start(cfg, Transport::Local).await?;
        let driven = drive(session, "simple", prompt_for("resume")).await;
        resume_id = driven.native_session_id;
        if resume_id.is_none() {
            eprintln!("FAILED: resume: the first session did not start");
            return Ok(ExitCode::from(5));
        }
    }

    let (rec_tx, mut rec_rx) = mpsc::unbounded_channel::<FixtureLine>();
    let adapter = AcpAdapter::new()
        .with_client_capabilities(caps)
        .with_recorder(rec_tx);
    let cfg = session_config(&scratch.dir, scenario, model.clone(), resume_id);
    let prompt = if scenario == "resume" {
        "What was the code word?"
    } else {
        prompt_for(scenario)
    };

    let session = adapter.start(cfg, Transport::Local).await?;
    let driven = drive(session, scenario, prompt).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    let sanitizer = build_sanitizer(&scratch.dir);
    let mut raw_lines = Vec::new();
    while let Ok(line) = rec_rx.try_recv() {
        raw_lines.push(line);
    }

    // Checks run on the raw stream (before sanitizing), so a leak is found.
    let mut failures: Vec<String> = Vec::new();
    let mut bypass: Vec<String> = Vec::new();
    if is_gate_deny_scenario(scenario) {
        let denied: &[&str] = if scenario == "gate-deny-execute" {
            &["execute"]
        } else {
            &["read"]
        };
        bypass = detect_gate_bypass(CANARY, denied, &raw_lines);
    }
    if scenario == "startup-error" {
        if driven.saw_session_started {
            failures.push(format!(
                "startup-error: a session started with model {BAD_MODEL}; the model was not validated"
            ));
        } else if driven.saw_fatal_error.is_none() {
            failures.push("startup-error: no fatal error reported".to_owned());
        }
    }
    // In every scenario: a tool call that completed must have reached the gate.
    for finding in detect_ungated_completions(&raw_lines) {
        if !bypass.contains(&finding) {
            bypass.push(finding);
        }
    }
    let gating = tool_gating(&raw_lines);

    let mut lines = Vec::new();
    for mut line in raw_lines {
        sanitizer.input(&mut line.input);
        lines.push(line);
    }

    let mut jsonl_out = String::new();
    for line in &lines {
        let s = serde_json::to_string(line)?;
        if !sanitizer.is_clean(&s) {
            return Err(format!("sanitizer check failed on fixture line: {s}").into());
        }
        jsonl_out.push_str(&s);
        jsonl_out.push('\n');
    }

    let events = normalize(&lines, &mut AcpNormalizer::new());
    let mut events_val = serde_json::to_value(&events)?;
    sanitizer.value(&mut events_val);
    let events_json = serde_json::to_string_pretty(&events_val)? + "\n";
    if !sanitizer.is_clean(&events_json) {
        return Err("sanitizer check failed on events.json".into());
    }

    let gating_md = if gating.is_empty() {
        "  - (no tool calls)\n".to_owned()
    } else {
        gating
            .iter()
            .map(|g| {
                format!(
                    "  - `{}` kind `{}` status `{}`: {}\n",
                    sanitizer.text(&g.call_id),
                    g.kind,
                    g.status.as_deref().unwrap_or("-"),
                    if g.gated {
                        "reached the gate"
                    } else {
                        "**did not reach the gate**"
                    }
                )
            })
            .collect()
    };
    let bypass_md = if bypass.is_empty() {
        String::new()
    } else {
        format!(
            "\n**GATE BYPASSED**: record as ungated in the matrix.\n{}",
            bypass
                .iter()
                .map(|b| format!("- {}\n", sanitizer.text(b)))
                .collect::<String>()
        )
    };
    let md = format!(
        "# Fixture: `{file_stem}`\n\n\
         - **Source**: `live`\n\
         - **Provider**: OpenCode `{version}` (`opencode acp`)\n\
         - **Model**: `{model}`\n\
         - **Client capabilities**: `fs.read_text_file={caps}`, `fs.write_text_file={caps}`, `terminal={caps}`, `elicitation=true`\n\
         - **Injected config**: `permissions: [{{* * ask}}]` per session (`opencode_config.rs`)\n\
         - **Prompt**: `{prompt}`\n\
         - **Events**: {n}\n\
         - **Tool calls**:\n{gating_md}{bypass_md}",
        version = opencode_version(),
        model = model.as_deref().unwrap_or("default"),
        caps = !no_client_caps,
        n = events.len(),
    );
    if !sanitizer.is_clean(&md) {
        return Err("sanitizer check failed on the .md".into());
    }

    let jsonl_path = out_dir.join(format!("{file_stem}.jsonl"));
    fs::write(&jsonl_path, jsonl_out)?;
    fs::write(
        out_dir.join(format!("{file_stem}.events.json")),
        events_json,
    )?;
    fs::write(out_dir.join(format!("{file_stem}.md")), md)?;
    println!(
        "Wrote {} lines to {} ({} events)",
        lines.len(),
        jsonl_path.display(),
        events.len()
    );
    for g in &gating {
        println!(
            "  tool call kind={} status={} gated={}",
            g.kind,
            g.status.as_deref().unwrap_or("-"),
            g.gated
        );
    }

    if !bypass.is_empty() {
        eprintln!("\n!!! GATE BYPASSED in '{scenario}': record as ungated in the matrix.");
        for b in &bypass {
            eprintln!("!!!   {}", sanitizer.text(b));
        }
        return Ok(ExitCode::from(3));
    }
    if !failures.is_empty() {
        for f in &failures {
            eprintln!("FAILED: {f}");
        }
        return Ok(ExitCode::from(5));
    }
    Ok(ExitCode::SUCCESS)
}
