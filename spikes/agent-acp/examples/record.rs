use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use atrium_agents::fixture::{FixtureLine, Sanitizer, normalize};
use atrium_agents::{
    AgentAdapter, BoxFuture, GateDecision, PermissionPolicy, SessionConfig, ToolCheck, ToolGate,
    Transport,
};
use atrium_protocol::agent::{
    AgentEvent, ApprovalDecision, QuestionAnswer, SessionCommand, ToolKind,
};
use spike_agent_acp::{AcpAdapter, AcpNormalizer, ClientCapabilityConfig};
use tokio::sync::mpsc;

struct ScenarioGate {
    scenario: String,
}

impl ToolGate for ScenarioGate {
    fn check<'a>(&'a self, call: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        let scenario = self.scenario.clone();
        let kind = call.kind;
        let input_str = call.input.to_string();
        let title_str = call.title.clone().unwrap_or_default();

        Box::pin(async move {
            match scenario.as_str() {
                "gate-deny" => {
                    if kind == ToolKind::Read
                        || input_str.contains("secret")
                        || title_str.contains("secret")
                    {
                        GateDecision::Deny {
                            reason: "denied by Atrium policy: secret.txt is restricted".to_owned(),
                        }
                    } else {
                        GateDecision::Allow
                    }
                }
                "gate-deny-execute" => {
                    if kind == ToolKind::Execute {
                        GateDecision::Deny {
                            reason: "denied by Atrium policy: shell execution blocked".to_owned(),
                        }
                    } else {
                        GateDecision::Allow
                    }
                }
                "approval-allow" | "approval-deny" => {
                    if matches!(kind, ToolKind::Edit | ToolKind::Execute | ToolKind::Delete) {
                        GateDecision::Ask
                    } else {
                        GateDecision::Allow
                    }
                }
                _ => GateDecision::Allow,
            }
        })
    }
}

fn default_fixtures_dir() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("fixtures")
        .join("agents")
        .join("acp")
}

fn build_sanitizer() -> Sanitizer {
    let home = env::var("HOME").unwrap_or_default();
    let user = env::var("USER")
        .or_else(|_| env::var("LOGNAME"))
        .unwrap_or_default();
    let mut secret_vals = Vec::new();
    for (k, v) in env::vars() {
        let upper = k.to_ascii_uppercase();
        if (upper.contains("KEY") || upper.contains("TOKEN") || upper.contains("SECRET"))
            && v.len() >= 8
        {
            secret_vals.push(v);
        }
    }
    let secret_refs: Vec<&str> = secret_vals.iter().map(String::as_str).collect();
    Sanitizer::new(&home, &user, &secret_refs)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let mut no_client_caps = false;
    args.retain(|arg| {
        if arg == "--no-client-caps" {
            no_client_caps = true;
            false
        } else {
            true
        }
    });

    let Some(scenario) = args.first().cloned() else {
        eprintln!(
            "Usage: cargo run -p spike-agent-acp --example record -- [--no-client-caps] <scenario>"
        );
        eprintln!(
            "Scenarios: simple, tool-read, gate-deny, gate-deny-execute, approval-allow, approval-deny, question, interrupt, multi-turn, resume, startup-error"
        );
        std::process::exit(2);
    };

    let out_dir = default_fixtures_dir();
    fs::create_dir_all(&out_dir)?;

    let caps = if no_client_caps {
        ClientCapabilityConfig::without_fs_and_terminal()
    } else {
        ClientCapabilityConfig::all()
    };

    let file_stem = if no_client_caps {
        format!("{scenario}-no-caps")
    } else {
        scenario.clone()
    };

    println!(
        "Recording scenario '{scenario}' (client fs/terminal caps: {}) -> {}",
        !no_client_caps,
        out_dir.join(&file_stem).display()
    );

    let (rec_tx, mut rec_rx) = mpsc::unbounded_channel::<FixtureLine>();
    let adapter = AcpAdapter::new()
        .with_client_capabilities(caps)
        .with_recorder(rec_tx);

    let cwd = env::current_dir()?;
    let model = if scenario == "startup-error" {
        Some("invalid-provider/nonexistent-model-000".to_owned())
    } else {
        env::var("ATRIUM_ACP_MODEL").ok()
    };

    let config = SessionConfig {
        cwd,
        model: model.clone(),
        effort: None,
        permissions: PermissionPolicy::Ask,
        gate: Arc::new(ScenarioGate {
            scenario: scenario.clone(),
        }),
        mcp_servers: Vec::new(),
        resume: None,
        env: Vec::new(),
        program: None,
    };

    let prompt = match scenario.as_str() {
        "simple" => "Reply with one short sentence saying hello from OpenCode ACP.",
        "tool-read" => "Read Cargo.toml and tell me the package name.",
        "gate-deny" => "Read secret.txt and tell me its contents.",
        "gate-deny-execute" => "Run the shell command `cat secret.txt`.",
        "approval-allow" => "Create a temporary file named approved-note.txt containing 'ok'.",
        "approval-deny" => "Write 'blocked' to production.conf.",
        "question" => "Ask me which target environment to use before proceeding.",
        "interrupt" => "Write a detailed 500-word essay about distributed consensus.",
        "multi-turn" | "resume" => "Remember the code word 'cobalt'. Reply with 'stored'.",
        "startup-error" => "Hello",
        other => {
            return Err(format!("unknown scenario: {other}").into());
        }
    };

    let mut session = adapter.start(config, Transport::Local).await?;
    if scenario != "startup-error" {
        session
            .commands
            .send(SessionCommand::SendUserMessage {
                text: prompt.to_owned(),
            })
            .await?;
    }

    let mut turn_count = 0u32;
    let mut interrupted = false;

    while let Some(env_event) = session.events.recv().await {
        match &env_event.event {
            AgentEvent::MessageDelta { .. } if scenario == "interrupt" && !interrupted => {
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
                    .map(|q| {
                        let selected = q
                            .options
                            .first()
                            .map(|o| vec![o.id.clone()])
                            .unwrap_or_default();
                        QuestionAnswer {
                            question_id: q.id.clone(),
                            selected,
                            text: if q.options.is_empty() {
                                Some("staging".to_owned())
                            } else {
                                None
                            },
                        }
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
                turn_count += 1;
                if scenario == "multi-turn" && turn_count == 1 {
                    session
                        .commands
                        .send(SessionCommand::SendUserMessage {
                            text: "What was the code word?".to_owned(),
                        })
                        .await?;
                } else {
                    let _ = session.commands.send(SessionCommand::Shutdown).await;
                }
            }
            AgentEvent::SessionEnded { .. } => break,
            _ => {}
        }
    }

    tokio::time::sleep(Duration::from_millis(50)).await;

    let sanitizer = build_sanitizer();
    let mut lines = Vec::new();
    while let Ok(mut line) = rec_rx.try_recv() {
        sanitizer.input(&mut line.input);
        lines.push(line);
    }

    let mut jsonl_out = String::new();
    for line in &lines {
        let s = serde_json::to_string(line)?;
        assert!(
            sanitizer.is_clean(&s),
            "Sanitizer check failed on fixture line: {s}"
        );
        jsonl_out.push_str(&s);
        jsonl_out.push('\n');
    }

    let mut normalizer = AcpNormalizer::new();
    let mut events = normalize(&lines, &mut normalizer);
    let mut events_val = serde_json::to_value(&events)?;
    sanitizer.value(&mut events_val);
    events = serde_json::from_value(events_val)?;
    let events_json = serde_json::to_string_pretty(&events)? + "\n";
    assert!(
        sanitizer.is_clean(&events_json),
        "Sanitizer check failed on events.json"
    );

    let jsonl_path = out_dir.join(format!("{file_stem}.jsonl"));
    let events_path = out_dir.join(format!("{file_stem}.events.json"));
    let md_path = out_dir.join(format!("{file_stem}.md"));

    fs::write(&jsonl_path, jsonl_out)?;
    fs::write(&events_path, events_json)?;

    let md_content = format!(
        "# Fixture: `{file_stem}`\n\n\
         - **Source**: `live`\n\
         - **Provider**: OpenCode (`opencode acp`)\n\
         - **Client Capabilities**: `fs.read_text_file={0}`, `fs.write_text_file={0}`, `terminal={0}`, `elicitation=true`\n\
         - **Prompt**: `{prompt}`\n\
         - **Events**: {1}\n",
        !no_client_caps,
        events.len()
    );
    fs::write(&md_path, md_content)?;

    println!(
        "Wrote {} lines to {} and {} events to {}",
        lines.len(),
        jsonl_path.display(),
        events.len(),
        events_path.display()
    );
    Ok(())
}
