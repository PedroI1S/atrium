//! Prints the enforcement descriptor `ClaudeAdapter::probe` gives for this
//! machine, sanitized (home folder as `~`, user name and the like scrubbed),
//! as pretty JSON.
//!
//! `cargo run -p atrium-agents --example claude-probe [-- <project dir>]`
//!
//! The project directory defaults to the current one; the CLI is found on
//! `PATH` (or `~/.local/bin`). Nothing is started: `probe` only runs
//! `claude --version` and reads configuration files.

use std::path::PathBuf;
use std::sync::Arc;

use atrium_agents::claude::ClaudeAdapter;
use atrium_agents::fixture::Sanitizer;
use atrium_agents::{
    AgentAdapter, ApprovalTicket, BoxFuture, GateDecision, PermissionPolicy, SessionConfig,
    ToolCheck, ToolGate,
};

/// `probe` never asks the gate anything.
struct NoGate;

impl ToolGate for NoGate {
    fn check<'a>(&'a self, _: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        Box::pin(async {
            GateDecision::Deny {
                reason: "probe only".into(),
            }
        })
    }

    fn redeem<'a>(
        &'a self,
        _: &'a ApprovalTicket,
        _: &'a ToolCheck,
    ) -> BoxFuture<'a, GateDecision> {
        Box::pin(async {
            GateDecision::Deny {
                reason: "probe only".into(),
            }
        })
    }

    fn question_ticket(&self, _: Option<u32>) -> BoxFuture<'_, Option<ApprovalTicket>> {
        Box::pin(async { None })
    }
}

#[tokio::main]
async fn main() {
    let cwd = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .expect("a project directory");
    let config = SessionConfig {
        cwd,
        model: None,
        effort: None,
        permissions: PermissionPolicy::Ask,
        gate: Arc::new(NoGate),
        mcp_servers: vec![],
        resume: None,
        env: vec![],
        program: None,
        on_spawn: None,
    };
    let adapter = ClaudeAdapter::default();
    let descriptor = adapter.probe(&config).await;
    let home = std::env::var("HOME").unwrap_or_default();
    let user = std::env::var("USER").unwrap_or_default();
    let sanitizer = Sanitizer::new(&home, &user, &[]);
    let json = serde_json::to_string_pretty(&descriptor).expect("serializable");
    let clean = sanitizer.text(&json);
    assert!(
        sanitizer.is_clean(&clean),
        "the output still names the user"
    );
    println!("{clean}");
}
