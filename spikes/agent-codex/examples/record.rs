mod support;

use atrium_agents::*;
use atrium_protocol::agent::*;
use spike_agent_codex::CodexAdapter;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Catalog discovery is a separate, reaped process; catalog traffic is not
/// part of the subsequently recorded agent session.
async fn catalog_model(config: &SessionConfig) -> Result<String, Box<dyn std::error::Error>> {
    let mut child = tokio::process::Command::new("codex")
        .args(["app-server"])
        .current_dir(&config.cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let result=tokio::time::timeout(STARTUP_TIMEOUT,async {
        stdin.write_all(b"{\"id\":1,\"method\":\"initialize\",\"params\":{\"clientInfo\":{\"name\":\"atrium_m0\",\"version\":\"0.0.0\"}}}\n").await?;
        while let Some(line)=lines.next_line().await? {
            let v:serde_json::Value=serde_json::from_str(&line)?;
            if v.get("error").is_some() {return Err(std::io::Error::other(v["error"].to_string()));}
            if v["id"]==1 {stdin.write_all(b"{\"method\":\"initialized\"}\n{\"id\":2,\"method\":\"model/list\",\"params\":{}}\n").await?;}
            if v["id"]==2 {
                let mut models=v["result"]["data"].as_array().into_iter().flatten().filter(|m|m["hidden"]!=true).filter_map(|m|m["model"].as_str()).collect::<Vec<_>>();
                models.sort_by_key(|m|(!m.contains("mini"),!m.contains("nano"),*m));
                return models.first().map(|m|m.to_string()).ok_or_else(||std::io::Error::other("empty model catalog"));
            }
        }
        Err(std::io::Error::other("app-server closed before model/list; check sandbox permissions"))
    }).await;
    let _ = child.kill().await;
    let _ = child.wait().await;
    Ok(result??)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let scenario = args.first().map(String::as_str).unwrap_or("simple");
    if !support::SCENARIOS.contains(&scenario) {
        return Err(format!(
            "unknown scenario {scenario}; choose {:?}",
            support::SCENARIOS
        )
        .into());
    }
    if args.iter().any(|a| a == "--hand-made") {
        support::save(
            scenario,
            &support::handmade(scenario),
            "hand-made; app-server was blocked by worker sandbox before handshake",
        )?;
        println!("Wrote hand-made {scenario} fixture and golden events");
        return Ok(());
    }
    let mut config = support::config(scenario);
    std::fs::create_dir_all(&config.cwd)?;
    std::fs::write(config.cwd.join("public.txt"), "public\n")?;
    std::fs::write(
        config.cwd.join("secret.txt"),
        "FIXTURE_SECRET_MUST_NOT_APPEAR",
    )?;
    // model/list has no numeric price field. Prefer a mini/nano catalog entry;
    // CODEX_MODEL lets the verifier select the cheapest entitled model explicitly.
    config.model = Some(std::env::var("CODEX_MODEL").unwrap_or(catalog_model(&config).await?));
    println!(
        "Catalog selection: {:?} (verify price; model/list does not expose it)",
        config.model
    );
    let recording = support::recording();
    let adapter = CodexAdapter {
        recording: Some(recording.clone()),
    };
    if scenario == "startup-error" {
        config.model = Some("invalid-atrium-fixture-model".into());
    }
    if scenario == "patch" {
        config.permissions = PermissionPolicy::Ask;
    }
    // Resume must prove a native id persisted across two distinct processes.
    if scenario == "resume" {
        let mut first = adapter.start(config.clone(), Transport::Local).await?;
        first
            .commands
            .send(SessionCommand::SendUserMessage {
                text: "Reply hello".into(),
            })
            .await?;
        let mut native = None;
        while let Some(e) = first.events.recv().await {
            match e.event {
                AgentEvent::SessionStarted {
                    native_session_id, ..
                } => native = Some(native_session_id),
                AgentEvent::TurnCompleted { .. } => {
                    first.commands.send(SessionCommand::Shutdown).await?;
                }
                AgentEvent::SessionEnded { .. } => break,
                _ => {}
            }
        }
        config.resume = native;
        if config.resume.is_none() {
            return Err("no native resume id".into());
        }
        // First process is retained separately for native-id provenance.
        support::save(
            "resume-origin",
            &recording.lock().unwrap(),
            "live; first process for resume",
        )?;
        recording.lock().unwrap().clear();
    }
    let result = adapter.start(config, Transport::Local).await;
    if let Ok(mut session) = result {
        session
            .commands
            .send(SessionCommand::SendUserMessage {
                text: support::prompt(scenario).into(),
            })
            .await?;
        let mut turns = 0;
        let finished = tokio::time::timeout(std::time::Duration::from_secs(120), async {
            while let Some(e) = session.events.recv().await {
                match e.event {
                    AgentEvent::ApprovalRequested { request_id, .. } => {
                        session
                            .commands
                            .send(SessionCommand::ResolveApproval {
                                request_id,
                                decision: if scenario == "approval-deny" {
                                    ApprovalDecision::Deny { message: None }
                                } else {
                                    ApprovalDecision::Allow
                                },
                            })
                            .await?
                    }
                    AgentEvent::QuestionAsked {
                        request_id,
                        questions,
                    } => {
                        session
                            .commands
                            .send(SessionCommand::AnswerQuestion {
                                request_id,
                                answers: questions
                                    .into_iter()
                                    .map(|q| QuestionAnswer {
                                        question_id: q.id,
                                        selected: q
                                            .options
                                            .first()
                                            .map(|o| vec![o.id.clone()])
                                            .unwrap_or_default(),
                                        text: None,
                                    })
                                    .collect(),
                            })
                            .await?
                    }
                    AgentEvent::ToolCallStarted { .. } if scenario == "interrupt" => {
                        session.commands.send(SessionCommand::Interrupt).await?
                    }
                    AgentEvent::TurnCompleted { .. } => {
                        turns += 1;
                        session
                            .commands
                            .send(if scenario == "multi-turn" && turns == 1 {
                                SessionCommand::SendUserMessage {
                                    text: "Say goodbye".into(),
                                }
                            } else {
                                SessionCommand::Shutdown
                            })
                            .await?;
                    }
                    AgentEvent::SessionEnded { .. } => break,
                    _ => {}
                }
            }
            Ok::<(), Box<dyn std::error::Error>>(())
        })
        .await;
        if finished.is_err() {
            session.commands.send(SessionCommand::Shutdown).await?;
            while session.events.recv().await.is_some() {}
        }
        finished??;
    } else if scenario != "startup-error" {
        // Save startup failures to a separate name, preserving the golden scenario.
        support::save(
            &format!("{scenario}-startup-failure"),
            &recording.lock().unwrap(),
            "live; startup failure",
        )?;
        return Err(result.unwrap_err().into());
    }
    support::save(scenario, &recording.lock().unwrap(), "live")?;
    Ok(())
}
