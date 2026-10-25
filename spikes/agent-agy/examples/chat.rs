use std::io::BufRead;
use std::path::PathBuf;
use std::sync::Arc;

use atrium_agents::{
    AgentAdapter, BoxFuture, GateDecision, PermissionPolicy, SessionConfig, ToolCheck, ToolGate,
    Transport,
};
use atrium_protocol::agent::{ApprovalDecision, QuestionAnswer, SessionCommand};
use spike_agent_agy::{AgyAdapter, ApprovalMode};

struct AskGate;

impl ToolGate for AskGate {
    fn check<'a>(&'a self, _call: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        Box::pin(async { GateDecision::Ask })
    }
}

struct ChatArgs {
    cwd: PathBuf,
    model: Option<String>,
    prompt: Option<String>,
    approval_mode: ApprovalMode,
    program: Option<PathBuf>,
}

fn parse_args() -> Result<ChatArgs, String> {
    let mut args = std::env::args().skip(1);
    let mut cwd = std::env::current_dir().map_err(|err| err.to_string())?;
    let mut model = None;
    let mut prompt = None;
    let mut approval_mode = ApprovalMode::Block;
    let mut program = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cwd" => {
                let val = args.next().ok_or("--cwd requires a value")?;
                cwd = PathBuf::from(val);
            }
            "--model" => {
                model = Some(args.next().ok_or("--model requires a value")?);
            }
            "--prompt" => {
                prompt = Some(args.next().ok_or("--prompt requires a value")?);
            }
            "--approval-mode" => {
                let val = args.next().ok_or("--approval-mode requires a value")?;
                approval_mode = match val.as_str() {
                    "block" => ApprovalMode::Block,
                    "deny-and-retry" => ApprovalMode::DenyAndRetry,
                    other => return Err(format!("unknown --approval-mode: {other}")),
                };
            }
            "--binary" => {
                let val = args.next().ok_or("--binary requires a value")?;
                program = Some(PathBuf::from(val));
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    Ok(ChatArgs {
        cwd,
        model,
        prompt,
        approval_mode,
        program,
    })
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("chat failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let ChatArgs {
        cwd,
        model,
        prompt,
        approval_mode,
        program,
    } = parse_args()?;
    let adapter = AgyAdapter::new().with_approval_mode(approval_mode);
    let gate: Arc<dyn ToolGate> = Arc::new(AskGate);

    let config = SessionConfig {
        cwd,
        model,
        effort: None,
        permissions: PermissionPolicy::Ask,
        gate,
        mcp_servers: Vec::new(),
        resume: None,
        env: Vec::new(),
        program,
    };

    let mut session = adapter
        .start(config, Transport::Local)
        .await
        .map_err(|err| err.to_string())?;

    if let Some(first_prompt) = prompt {
        let _ = session
            .commands
            .send(SessionCommand::SendUserMessage { text: first_prompt })
            .await;
    }

    let commands = session.commands.clone();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else {
                break;
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let cmd = if trimmed == "quit" || trimmed == "exit" {
                SessionCommand::Shutdown
            } else if trimmed == "interrupt" {
                SessionCommand::Interrupt
            } else if let Some(rest) = trimmed.strip_prefix("approve ") {
                SessionCommand::ResolveApproval {
                    request_id: rest.trim().to_string(),
                    decision: ApprovalDecision::Allow,
                }
            } else if let Some(rest) = trimmed.strip_prefix("deny ") {
                let mut parts = rest.trim().splitn(2, ' ');
                let req_id = parts.next().unwrap_or_default().to_string();
                let message = parts.next().map(ToString::to_string);
                SessionCommand::ResolveApproval {
                    request_id: req_id,
                    decision: ApprovalDecision::Deny { message },
                }
            } else if let Some(rest) = trimmed.strip_prefix("answer ") {
                let mut parts = rest.trim().splitn(2, ' ');
                let req_id = parts.next().unwrap_or_default().to_string();
                let kv = parts.next().unwrap_or_default();
                let (q_id, val) = kv.split_once('=').unwrap_or(("q-0", kv));
                SessionCommand::AnswerQuestion {
                    request_id: req_id,
                    answers: vec![QuestionAnswer {
                        question_id: q_id.to_string(),
                        selected: Vec::new(),
                        text: Some(val.to_string()),
                    }],
                }
            } else {
                SessionCommand::SendUserMessage {
                    text: trimmed.to_string(),
                }
            };
            let is_shutdown = matches!(cmd, SessionCommand::Shutdown);
            if commands.blocking_send(cmd).is_err() || is_shutdown {
                break;
            }
        }
    });

    while let Some(env) = session.events.recv().await {
        if let Ok(json) = serde_json::to_string(&env) {
            println!("{json}");
        }
    }

    Ok(())
}
