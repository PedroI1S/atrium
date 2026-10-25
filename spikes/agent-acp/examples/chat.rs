use std::env;
use std::io::{self, Write};
use std::sync::Arc;

use atrium_agents::{
    AgentAdapter, BoxFuture, GateDecision, PermissionPolicy, SessionConfig, ToolCheck, ToolGate,
    Transport,
};
use atrium_protocol::agent::{
    AgentEvent, ApprovalDecision, MessageKind, QuestionAnswer, SessionCommand, ToolKind,
};
use spike_agent_acp::{AcpAdapter, ClientCapabilityConfig};
use tokio::io::{AsyncBufReadExt, BufReader};

struct InteractiveGate;

impl ToolGate for InteractiveGate {
    fn check<'a>(&'a self, call: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        let kind = call.kind;
        Box::pin(async move {
            match kind {
                ToolKind::Read | ToolKind::Search | ToolKind::Think => GateDecision::Allow,
                _ => GateDecision::Ask,
            }
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let no_caps = args.iter().any(|a| a == "--no-client-caps");
    let resume = args
        .windows(2)
        .find(|w| w[0] == "--resume")
        .map(|w| w[1].clone());
    let model = args
        .windows(2)
        .find(|w| w[0] == "--model")
        .map(|w| w[1].clone());

    let caps = if no_caps {
        ClientCapabilityConfig::without_fs_and_terminal()
    } else {
        ClientCapabilityConfig::all()
    };

    let adapter = AcpAdapter::new().with_client_capabilities(caps);
    let config = SessionConfig {
        cwd: env::current_dir()?,
        model,
        effort: None,
        permissions: PermissionPolicy::Ask,
        gate: Arc::new(InteractiveGate),
        mcp_servers: Vec::new(),
        resume,
        env: Vec::new(),
        program: None,
    };

    let mut session = adapter.start(config, Transport::Local).await?;
    let commands = session.commands.clone();

    println!("Connected to ACP agent (`opencode acp`). Type a message, `:interrupt`, or `:quit`.");
    print!("> ");
    io::stdout().flush()?;

    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();

    loop {
        tokio::select! {
            maybe_env = session.events.recv() => {
                let Some(env_ev) = maybe_env else {
                    break;
                };
                match env_ev.event {
                    AgentEvent::SessionStarted { native_session_id, model, .. } => {
                        println!("\n[session started: id={native_session_id}, model={model:?}]");
                    }
                    AgentEvent::TurnStarted { turn } => {
                        println!("[turn {turn} started]");
                    }
                    AgentEvent::MessageDelta { kind: MessageKind::Assistant, text, .. } => {
                        print!("{text}");
                        io::stdout().flush()?;
                    }
                    AgentEvent::MessageDelta { kind: MessageKind::Reasoning, text, .. } => {
                        eprint!("{text}");
                    }
                    AgentEvent::MessageCompleted { .. } => {
                        println!();
                    }
                    AgentEvent::ToolCallStarted { call_id, name, kind, title, .. } => {
                        println!("\n[tool started: {call_id} {name} ({kind:?}) {title:?}]");
                    }
                    AgentEvent::ToolCallFinished { call_id, status, error, .. } => {
                        println!("[tool finished: {call_id} status={status:?} err={error:?}]");
                    }
                    AgentEvent::ApprovalRequested { request_id, name, kind, title, .. } => {
                        println!("\n[approval requested ({request_id}): {name} ({kind:?}) {title:?} -> auto-allowing in demo]");
                        let _ = commands
                            .send(SessionCommand::ResolveApproval {
                                request_id,
                                decision: ApprovalDecision::Allow,
                            })
                            .await;
                    }
                    AgentEvent::QuestionAsked { request_id, questions } => {
                        println!("\n[question asked ({request_id}): {questions:?}]");
                        let answers = questions
                            .into_iter()
                            .map(|q| QuestionAnswer {
                                question_id: q.id,
                                selected: q.options.first().map(|o| vec![o.id.clone()]).unwrap_or_default(),
                                text: None,
                            })
                            .collect();
                        let _ = commands
                            .send(SessionCommand::AnswerQuestion {
                                request_id,
                                answers,
                            })
                            .await;
                    }
                    AgentEvent::TurnCompleted { turn, outcome, usage } => {
                        println!("\n[turn {turn} completed: {outcome:?}, usage={usage:?}]");
                        print!("> ");
                        io::stdout().flush()?;
                    }
                    AgentEvent::Error { message, fatal } => {
                        eprintln!("\n[error (fatal={fatal}): {message}]");
                    }
                    AgentEvent::SessionEnded { exit_code, reason } => {
                        println!("\n[session ended: code={exit_code:?}, reason={reason:?}]");
                        break;
                    }
                    _ => {}
                }
            }
            maybe_line = lines.next_line() => {
                let Some(line) = maybe_line? else {
                    let _ = commands.send(SessionCommand::Shutdown).await;
                    break;
                };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if trimmed == ":quit" || trimmed == ":q" {
                    let _ = commands.send(SessionCommand::Shutdown).await;
                } else if trimmed == ":interrupt" || trimmed == ":i" {
                    let _ = commands.send(SessionCommand::Interrupt).await;
                } else {
                    let _ = commands
                        .send(SessionCommand::SendUserMessage {
                            text: trimmed.to_owned(),
                        })
                        .await;
                }
            }
        }
    }

    Ok(())
}
