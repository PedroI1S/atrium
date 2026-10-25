//! Interactive Claude Code session. Events go to stdout as JSON lines.
//!
//! Stdin, one JSON object per line:
//! - a `SessionCommand` (`{"type":"send_user_message","text":"hi"}`,
//!   `{"type":"resolve_approval",...}`, `{"type":"interrupt"}`, ...);
//! - a `GateDecision` (`{"type":"allow"}`, `{"type":"deny","reason":"..."}`,
//!   `{"type":"ask"}`), which answers the oldest pending `gate_request`
//!   line that was printed to stdout.
//!
//! Usage: `cargo run -p spike-agent-claude --example chat -- [--cwd DIR]
//! [--model M] [--permissions ask|read-only|full-access] [--resume ID]
//! [--program PATH] [--setting-sources S (empty = no external settings)] [--gate-timeout-secs N]`
//! With stdin at EOF every undecided gate request is denied.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use atrium_agents::{
    AgentAdapter, BoxFuture, GateDecision, PermissionPolicy, SessionConfig, ToolCheck, ToolGate,
    Transport,
};
use atrium_protocol::agent::{AgentEvent, SessionCommand};
use spike_agent_claude::{ClaudeAdapter, ClaudeOptions};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::oneshot;

#[derive(Default)]
struct StdinGate {
    waiting: Mutex<VecDeque<oneshot::Sender<GateDecision>>>,
}

impl ToolGate for StdinGate {
    fn check<'a>(&'a self, check: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();
            self.waiting.lock().expect("gate lock").push_back(tx);
            println!("{}", serde_json::json!({ "gate_request": check }));
            rx.await.unwrap_or(GateDecision::Deny {
                reason: "no decision (stdin closed)".into(),
            })
        })
    }
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let gate = Arc::new(StdinGate::default());
    let permissions = match arg(&args, "--permissions").as_deref() {
        Some("read-only") => PermissionPolicy::ReadOnly,
        Some("full-access") => PermissionPolicy::FullAccess,
        _ => PermissionPolicy::Ask,
    };
    let config = SessionConfig {
        cwd: arg(&args, "--cwd")
            .map(PathBuf::from)
            .unwrap_or_else(|| ".".into()),
        model: Some(arg(&args, "--model").unwrap_or_else(|| "haiku".into())),
        effort: None,
        permissions,
        gate: gate.clone(),
        mcp_servers: Vec::new(),
        resume: arg(&args, "--resume"),
        env: Vec::new(),
        program: arg(&args, "--program").map(PathBuf::from),
    };
    let gate_secs = arg(&args, "--gate-timeout-secs")
        .and_then(|s| s.parse().ok())
        .unwrap_or(120);
    let adapter = ClaudeAdapter::new(ClaudeOptions {
        setting_sources: arg(&args, "--setting-sources"),
        gate_deadline: Duration::from_secs(gate_secs),
        ..ClaudeOptions::default()
    });
    let mut session = match adapter.start(config, Transport::Local).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("start failed: {e}");
            std::process::exit(1);
        }
    };
    // Say up front when Claude Code's own settings put hooks outside the gate.
    if let Some(e) = adapter.last_enforcement() {
        eprintln!(
            "enforcement: {:?}{}",
            e.interception,
            e.reason.map(|r| format!(" ({r})")).unwrap_or_default()
        );
    }
    let commands = session.commands.clone();
    let reader_gate = gate.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(decision) = serde_json::from_str::<GateDecision>(&line) {
                let waiting = reader_gate.waiting.lock().expect("gate lock").pop_front();
                match waiting {
                    Some(tx) => {
                        let _ = tx.send(decision);
                    }
                    None => eprintln!("no gate request is waiting"),
                }
            } else if let Ok(cmd) = serde_json::from_str::<SessionCommand>(&line) {
                if commands.send(cmd).await.is_err() {
                    break;
                }
            } else {
                eprintln!("not a command or gate decision: {line}");
            }
        }
        // stdin closed: deny what is undecided, then end the session.
        reader_gate.waiting.lock().expect("gate lock").clear();
        let _ = commands.send(SessionCommand::Shutdown).await;
    });
    while let Some(env) = session.events.recv().await {
        println!("{}", serde_json::to_string(&env).expect("event serializes"));
        if matches!(env.event, AgentEvent::SessionEnded { .. }) {
            break;
        }
    }
}
