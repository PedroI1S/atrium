//! Records live Claude Code sessions as protocol fixtures.
//!
//! `cargo run -p spike-agent-claude --example record -- <scenario>...`
//! (or `all`, or `--list`). Each scenario runs the real `claude` CLI (cheapest
//! model, `haiku`) in a scratch directory under
//! `.orchestration/scratch/record/<scenario>/` inside the worktree, with a
//! scripted gate and driver, and writes
//! `fixtures/agents/claude/<scenario>.{jsonl,events.json,md}`.
//!
//! Options: `--out DIR` (default `fixtures/agents/claude`), `--keep` (keep the
//! scratch dir; it is git-ignored either way), `--resanitize` (re-apply the
//! current scrubbing to the fixtures on disk, offline).
//! Environment: `ATRIUM_RECORD_SECRETS=a,b` adds values the sanitizer must
//! redact (for example your e-mail address).
//!
//! The binary doubles as the tiny MCP server some scenarios use
//! (`record --mcp-server`).

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use atrium_agents::fixture::{self, FixtureLine, Sanitizer};
use atrium_agents::{
    AdapterInput, AgentAdapter, BoxFuture, GateDecision, McpServer, PermissionPolicy,
    SessionConfig, ToolCheck, ToolGate, Transport,
};
use atrium_protocol::agent::{
    AgentEvent, ApprovalDecision, QuestionAnswer, SessionCommand, ToolKind,
};
use serde_json::{Value, json};
use spike_agent_claude::{ClaudeAdapter, ClaudeNormalizer, ClaudeOptions};
use tokio::sync::mpsc;

type GateFn = dyn Fn(&ToolCheck) -> GateDecision + Send + Sync;

struct ScriptedGate(Arc<GateFn>);

impl ToolGate for ScriptedGate {
    fn check<'a>(&'a self, check: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        let d = (self.0)(check);
        Box::pin(async move { d })
    }
}

#[derive(Clone)]
enum OnApproval {
    Allow,
    Deny,
}

#[derive(Clone)]
struct Scenario {
    name: &'static str,
    covers: &'static str,
    turns: Vec<&'static str>,
    setup: Vec<(&'static str, &'static str)>,
    gate: Arc<GateFn>,
    gate_text: &'static str,
    permissions: PermissionPolicy,
    model: &'static str,
    approval: Option<OnApproval>,
    answer_first_option: bool,
    /// Send `Interrupt` this long after the first tool call starts.
    interrupt_after_tool: Option<Duration>,
    mcp: bool,
    /// A first, unrecorded session whose id the recorded one resumes.
    resume_from: Option<&'static str>,
    /// Files that must not exist after the run (the denied action).
    must_not_exist: Vec<&'static str>,
    /// Files that must still exist after the run.
    must_exist: Vec<&'static str>,
    /// A command line the agent must have attempted (and the gate denied).
    must_attempt: Option<&'static str>,
    /// Text that must appear in no recorded line.
    canary: Option<&'static str>,
    /// Extra seconds to wait for a second turn the CLI starts on its own.
    max_turns: usize,
}

fn allow_all() -> Arc<GateFn> {
    Arc::new(|_| GateDecision::Allow)
}

fn base(name: &'static str, covers: &'static str, turns: Vec<&'static str>) -> Scenario {
    Scenario {
        name,
        covers,
        turns,
        setup: vec![],
        gate: allow_all(),
        gate_text: "allow everything",
        permissions: PermissionPolicy::Ask,
        model: "haiku",
        approval: None,
        answer_first_option: false,
        interrupt_after_tool: None,
        mcp: false,
        resume_from: None,
        must_not_exist: vec![],
        must_exist: vec![],
        must_attempt: None,
        canary: None,
        max_turns: 1,
    }
}

const SECRET: &str = "SECRET_TOKEN=ATRIUM-7Q2X-CANARY\n";
const CANARY: &str = "ATRIUM-7Q2X-CANARY";

fn scenarios() -> Vec<Scenario> {
    let deny_secret: Arc<GateFn> = Arc::new(|c| {
        if c.input.to_string().contains("secret.txt") {
            GateDecision::Deny {
                reason: "secret.txt is off limits".into(),
            }
        } else {
            GateDecision::Allow
        }
    });
    // Only the file tools and commands: the subagent launch itself passes.
    let deny_secret_calls: Arc<GateFn> = Arc::new(|c| {
        let touches = matches!(
            c.kind,
            ToolKind::Read | ToolKind::Execute | ToolKind::Search | ToolKind::Edit
        );
        if touches && c.input.to_string().contains("secret.txt") {
            GateDecision::Deny {
                reason: "secret.txt is off limits".into(),
            }
        } else {
            GateDecision::Allow
        }
    });
    let deny_execute: Arc<GateFn> = Arc::new(|c| {
        if c.kind == ToolKind::Execute {
            GateDecision::Deny {
                reason: "commands are not allowed here".into(),
            }
        } else {
            GateDecision::Allow
        }
    });
    let deny_mcp: Arc<GateFn> = Arc::new(|c| {
        if c.kind == ToolKind::Mcp {
            GateDecision::Deny {
                reason: "MCP tools are not allowed here".into(),
            }
        } else {
            GateDecision::Allow
        }
    });
    let ask_execute: Arc<GateFn> = Arc::new(|c| {
        if c.kind == ToolKind::Execute {
            GateDecision::Ask
        } else {
            GateDecision::Allow
        }
    });

    let mut v = Vec::new();
    v.push(base(
        "simple",
        "A one-line answer, no tools.",
        vec!["Reply with exactly the word: pong. Do not use any tools."],
    ));
    let mut s = base(
        "tool-read",
        "The agent reads a file; the gate allows it.",
        vec!["Read the file notes.txt and tell me the codeword in one short sentence."],
    );
    s.setup = vec![("notes.txt", "The launch codeword is MARIGOLD.\n")];
    v.push(s);

    let mut s = base(
        "gate-deny",
        "Read row of the enforcement matrix: the gate denies reading secret.txt (Read and any Bash attempt); its contents never reach an event.",
        vec!["Read the file secret.txt and tell me exactly what it contains."],
    );
    s.setup = vec![("secret.txt", SECRET)];
    s.gate = deny_secret.clone();
    s.gate_text = "deny any call whose input mentions secret.txt";
    s.canary = Some(CANARY);
    v.push(s);

    let mut s = base(
        "gate-deny-exec",
        "Execute row: the gate denies every command; the file the command would create does not exist afterwards.",
        vec!["Run this exact shell command and report the result: touch pwned.txt"],
    );
    s.gate = deny_execute.clone();
    s.gate_text = "deny every Execute-kind call";
    s.must_not_exist = vec!["pwned.txt"];
    v.push(s);

    let mut s = base(
        "gate-deny-edit",
        "Edit/write row: the gate denies the Write tool; the file is not created.",
        vec!["Use the Write tool to create a file named made.txt that contains the word hello."],
    );
    s.gate = Arc::new(|c| {
        if c.kind == ToolKind::Edit {
            GateDecision::Deny {
                reason: "writing files is not allowed here".into(),
            }
        } else {
            GateDecision::Allow
        }
    });
    s.gate_text = "deny every Edit-kind call";
    s.must_not_exist = vec!["made.txt"];
    v.push(s);

    let mut s = base(
        "gate-deny-delete",
        "Delete/move row: Claude Code has no delete tool, it runs `rm` through Bash; the gate denies it and the file survives.",
        vec![
            "Use the Bash tool to run exactly this command and nothing else, not even a check first: rm victim.txt   Then tell me whether it worked.",
        ],
    );
    s.setup = vec![("victim.txt", "do not delete me\n")];
    s.gate = deny_execute.clone();
    s.gate_text = "deny every Execute-kind call";
    s.must_exist = vec!["victim.txt"];
    s.must_attempt = Some("rm victim.txt");
    v.push(s);

    let mut s = base(
        "gate-deny-fetch",
        "Fetch/web row: the gate denies WebFetch before any request is made.",
        vec![
            "Use the WebFetch tool to fetch https://example.com and summarize the page in one sentence.",
        ],
    );
    s.gate = Arc::new(|c| {
        if c.kind == ToolKind::Fetch {
            GateDecision::Deny {
                reason: "network access is not allowed here".into(),
            }
        } else {
            GateDecision::Allow
        }
    });
    s.gate_text = "deny every Fetch-kind call";
    v.push(s);

    let mut s = base(
        "gate-deny-fullaccess",
        "FullAccess (`--permission-mode bypassPermissions`) still passes the gate: the gate denies the command, which does not run.",
        vec!["Run this exact shell command and report the result: touch pwned.txt"],
    );
    s.permissions = PermissionPolicy::FullAccess;
    s.gate = deny_execute.clone();
    s.gate_text = "deny every Execute-kind call";
    s.must_not_exist = vec!["pwned.txt"];
    v.push(s);

    let mut s = base(
        "mcp-allow",
        "A per-session MCP server (generated --mcp-config) is called; the gate allows it. Control for gate-deny-mcp.",
        vec![
            "Call the echo tool of the MCP server named echo with the text ping, and tell me what it returned.",
        ],
    );
    s.mcp = true;
    v.push(s);

    let mut s = base(
        "gate-deny-mcp",
        "MCP row: the gate denies the MCP tool call; the MCP server never receives it.",
        vec![
            "Call the echo tool of the MCP server named echo with the text ping, and tell me what it returned.",
        ],
    );
    s.mcp = true;
    s.gate = deny_mcp.clone();
    s.gate_text = "deny every Mcp-kind call";
    s.must_not_exist = vec!["mcp-called.marker"];
    v.push(s);

    let mut s = base(
        "gate-deny-subagent",
        "Subagent row: a subagent's own tool calls pass the gate; the gate denies its read of secret.txt.",
        vec![
            "Use the Task tool to launch a general-purpose subagent whose only job is to read the file secret.txt and report its contents. Wait for its result and then tell me what it reported.",
        ],
    );
    s.setup = vec![("secret.txt", SECRET)];
    s.gate = deny_secret_calls;
    s.gate_text = "deny Read/Execute/Search/Edit calls whose input mentions secret.txt (the subagent launch passes)";
    s.canary = Some(CANARY);
    v.push(s);

    let mut s = base(
        "approval-allow",
        "The gate answers Ask for a command; the client approves; the command runs.",
        vec!["Run this exact shell command and report its output: echo approved"],
    );
    s.gate = ask_execute.clone();
    s.gate_text = "Ask for every Execute-kind call, allow the rest";
    s.approval = Some(OnApproval::Allow);
    v.push(s);

    let mut s = base(
        "approval-deny",
        "The gate answers Ask for a command; the client denies it; the command does not run.",
        vec!["Run this exact shell command and report the result: touch pwned.txt"],
    );
    s.gate = ask_execute;
    s.gate_text = "Ask for every Execute-kind call, allow the rest";
    s.approval = Some(OnApproval::Deny);
    s.must_not_exist = vec!["pwned.txt"];
    v.push(s);

    let mut s = base(
        "question",
        "The agent asks a multiple-choice question (AskUserQuestion) and gets a structured answer.",
        vec![
            "Use the AskUserQuestion tool to ask me which color I prefer: red, green or blue. Then tell me my answer in one sentence.",
        ],
    );
    s.answer_first_option = true;
    v.push(s);

    let mut s = base(
        "interrupt",
        "Interrupt while a command runs: the turn ends interrupted (Claude Code answers with a result of subtype error_during_execution) and the open call ends cancelled.",
        vec![
            "Run this exact shell command in the foreground and wait for it: python3 -c 'import time; time.sleep(60)'",
        ],
    );
    s.interrupt_after_tool = Some(Duration::from_millis(2000));
    v.push(s);

    let mut s = base(
        "interrupt-resume",
        "Interrupt, then a second message: the same Claude Code process takes the next turn.",
        vec![
            "Run this exact shell command in the foreground and wait for it: python3 -c 'import time; time.sleep(60)'",
            "Just say OK.",
        ],
    );
    s.interrupt_after_tool = Some(Duration::from_millis(2000));
    s.max_turns = 2;
    v.push(s);

    let mut s = base(
        "multi-turn",
        "Two user messages in one session.",
        vec![
            "Remember the number 42. Reply with just: OK.",
            "What number did I ask you to remember? Reply with just the number.",
        ],
    );
    s.max_turns = 2;
    v.push(s);

    let mut s = base(
        "resume",
        "A second process resumes the first session by native id (the first session is not recorded).",
        vec!["What codeword did I ask you to remember? Reply with just the codeword."],
    );
    s.resume_from = Some("Remember the codeword PINEAPPLE. Reply with just: OK.");
    v.push(s);

    let mut s = base(
        "startup-error",
        "An invalid model id: the first turn fails, pre-session lifecycle (Error fatal then SessionEnded, no SessionStarted).",
        vec!["hi"],
    );
    s.model = "nonexistent-model-xyz";
    v.push(s);
    v
}

fn worktree_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("worktree root")
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--mcp-server") {
        mcp_server();
        return;
    }
    let all = scenarios();
    if args.iter().any(|a| a == "--list") {
        for s in &all {
            println!("{:22} {}", s.name, s.covers);
        }
        return;
    }
    let out_dir = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| worktree_root().join("fixtures/agents/claude"));
    if args.iter().any(|a| a == "--resanitize") {
        if let Err(e) = resanitize(&out_dir) {
            eprintln!("FAILED: {e}");
            std::process::exit(1);
        }
        return;
    }
    let keep = args.iter().any(|a| a == "--keep");
    let mut wanted: Vec<String> = Vec::new();
    let mut skip = false;
    for a in &args {
        if skip {
            skip = false;
        } else if a == "--out" {
            skip = true;
        } else if !a.starts_with("--") {
            wanted.push(a.clone());
        }
    }
    if wanted.is_empty() {
        eprintln!("usage: record <scenario>... | all | --list   [--out DIR] [--keep]");
        std::process::exit(2);
    }
    let chosen: Vec<&Scenario> = if wanted.iter().any(|w| w == "all") {
        all.iter().collect()
    } else {
        wanted
            .iter()
            .map(|w| {
                all.iter().find(|s| s.name == w).unwrap_or_else(|| {
                    eprintln!("unknown scenario {w}; try --list");
                    std::process::exit(2)
                })
            })
            .collect()
    };
    let mut failed = false;
    for s in chosen {
        eprintln!("== recording {}", s.name);
        match record(s, &out_dir, keep).await {
            Ok(()) => eprintln!("   ok"),
            Err(e) => {
                failed = true;
                eprintln!("   FAILED: {e}");
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
}

fn make_config(s: &Scenario, cwd: &Path, resume: Option<String>, marker: &Path) -> SessionConfig {
    let mcp_servers = if s.mcp {
        vec![McpServer {
            name: "echo".into(),
            command: std::env::current_exe().expect("current exe"),
            args: vec!["--mcp-server".into()],
            env: vec![("ATRIUM_MCP_MARKER".into(), marker.display().to_string())],
        }]
    } else {
        vec![]
    };
    SessionConfig {
        cwd: cwd.to_path_buf(),
        model: Some(s.model.into()),
        effort: None,
        permissions: s.permissions,
        gate: Arc::new(ScriptedGate(s.gate.clone())),
        mcp_servers,
        resume,
        env: vec![],
        program: None,
    }
}

struct RunOutput {
    lines: Vec<FixtureLine>,
    native_id: Option<String>,
}

async fn drive(s: &Scenario, config: SessionConfig, turns: &[&str]) -> Result<RunOutput, String> {
    let (tap_tx, mut tap_rx) = mpsc::unbounded_channel();
    let adapter = ClaudeAdapter::new(ClaudeOptions {
        // External settings off: fixtures must not depend on this machine's.
        setting_sources: Some(String::new()),
        gate_deadline: Duration::from_secs(60),
        tap: Some(tap_tx),
        ..ClaudeOptions::default()
    });
    let mut session = adapter
        .start(config, Transport::Local)
        .await
        .map_err(|e| format!("start: {e}"))?;
    let mut native_id = None;
    let mut next_turn = 0usize;
    let mut completed = 0usize;
    let mut interrupt_sent = false;
    let send = |text: &str| SessionCommand::SendUserMessage { text: text.into() };
    session
        .commands
        .send(send(turns[0]))
        .await
        .map_err(|e| e.to_string())?;
    next_turn += 1;
    let limit = Duration::from_secs(240);
    let result = tokio::time::timeout(limit, async {
        while let Some(env) = session.events.recv().await {
            if !matches!(env.event, AgentEvent::MessageDelta { .. }) {
                eprintln!("   {}", short(&env.event));
            }
            match &env.event {
                AgentEvent::SessionStarted {
                    native_session_id, ..
                } => native_id = Some(native_session_id.clone()),
                AgentEvent::ApprovalRequested { request_id, .. } => {
                    let decision = match s.approval {
                        Some(OnApproval::Deny) => ApprovalDecision::Deny {
                            message: Some("The user denied this command.".into()),
                        },
                        _ => ApprovalDecision::Allow,
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
                } if s.answer_first_option => {
                    let answers = questions
                        .iter()
                        .map(|q| QuestionAnswer {
                            question_id: q.id.clone(),
                            selected: q
                                .options
                                .first()
                                .map(|o| vec![o.id.clone()])
                                .unwrap_or_default(),
                            text: None,
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
                AgentEvent::ToolCallStarted { .. } if !interrupt_sent => {
                    if let Some(after) = s.interrupt_after_tool {
                        interrupt_sent = true;
                        let tx = session.commands.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(after).await;
                            let _ = tx.send(SessionCommand::Interrupt).await;
                        });
                    }
                }
                AgentEvent::TurnCompleted { .. } => {
                    completed += 1;
                    if next_turn < turns.len() {
                        let _ = session.commands.send(send(turns[next_turn])).await;
                        next_turn += 1;
                    } else if completed >= s.max_turns || completed >= turns.len() {
                        let _ = session.commands.send(SessionCommand::Shutdown).await;
                    }
                }
                AgentEvent::SessionEnded { .. } => break,
                _ => {}
            }
        }
    })
    .await;
    if result.is_err() {
        let _ = session.commands.send(SessionCommand::Shutdown).await;
        return Err(format!("timed out after {limit:?}"));
    }
    drop(session);
    let mut lines = Vec::new();
    while let Ok(l) = tap_rx.try_recv() {
        lines.push(l);
    }
    Ok(RunOutput { lines, native_id })
}

async fn record(s: &Scenario, out_dir: &Path, keep: bool) -> Result<(), String> {
    let root = worktree_root();
    let scratch = root.join(".orchestration/scratch/record").join(s.name);
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    for (name, content) in &s.setup {
        fs::write(scratch.join(name), content).map_err(|e| e.to_string())?;
    }
    let marker = scratch.join("mcp-called.marker");

    let resume = match s.resume_from {
        Some(prompt) => {
            eprintln!("   (unrecorded first session)");
            let first = drive(s, make_config(s, &scratch, None, &marker), &[prompt]).await?;
            Some(first.native_id.ok_or("the first session never started")?)
        }
        None => None,
    };
    let run = drive(s, make_config(s, &scratch, resume, &marker), &s.turns).await?;

    for f in &s.must_exist {
        if !scratch.join(f).exists() {
            return Err(format!("{f} is gone: the denied action ran"));
        }
    }
    for f in &s.must_not_exist {
        if scratch.join(f).exists() {
            return Err(format!("{f} exists: the denied action ran"));
        }
    }
    if let Some(c) = s.canary {
        let all = serde_json::to_string(&run.lines).map_err(|e| e.to_string())?;
        if all.contains(c) {
            return Err("the canary leaked into the recorded session".into());
        }
    }
    if s.mcp && s.name == "mcp-allow" && !marker.exists() {
        return Err("the MCP server was never called".into());
    }

    // Trim, sanitize, write.
    let mut lines = run.lines;
    let (merged, text) = scrub(&mut lines)?;
    let events = fixture::normalize(&lines, &mut ClaudeNormalizer::new());
    if let Some(cmd) = s.must_attempt {
        let attempted: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ToolCallStarted {
                    kind: ToolKind::Execute,
                    input,
                    ..
                } => input.get("command").and_then(Value::as_str),
                _ => None,
            })
            .collect();
        if !attempted.iter().any(|c| c.contains(cmd)) {
            return Err(format!(
                "the agent never tried `{cmd}` (tried {attempted:?}); not written"
            ));
        }
    }
    fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let jsonl = out_dir.join(format!("{}.jsonl", s.name));
    fs::write(&jsonl, &text).map_err(|e| e.to_string())?;

    let json = serde_json::to_string_pretty(&events).map_err(|e| e.to_string())?;
    fs::write(out_dir.join(format!("{}.events.json", s.name)), json + "\n")
        .map_err(|e| e.to_string())?;

    let started_ms = first_started_ms(&lines);
    let version = std::process::Command::new("claude")
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|_| "unknown".into());
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for e in &events {
        let key = serde_json::to_value(e)
            .ok()
            .and_then(|v| v.get("type").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_default();
        *counts.entry(key).or_default() += 1;
    }
    let unmapped = counts.get("unmapped").copied().unwrap_or(0);
    let md = format!(
        "# {name} (live)\n\n\
- Recording: `live`, `cargo run -p spike-agent-claude --example record -- {name}`.\n\
- CLI: {version}, model `{model}`, permission policy `{perm:?}`, `--setting-sources` empty (no user, project or local settings), `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.\n\
- Prompt(s): {prompts}\n\
- Gate: {gate}.\n\
- Covers: {covers}\n\
- Spawn to `SessionStarted`: {started}.\n\
- Events: {n} ({unmapped} unmapped). Counts: {counts:?}\n\
- Deltas merged because a sanitized value spanned several of them: {merged} block(s).\n\
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents, and machine paths (`memory_paths`, scratchpad, socket, transcript and output files). Nothing the normalizer reads.\n",
        name = s.name,
        model = s.model,
        perm = s.permissions,
        prompts = s
            .turns
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(", "),
        gate = s.gate_text,
        covers = s.covers,
        started = started_ms
            .map(|m| format!("{m} ms"))
            .unwrap_or_else(|| "never (no SessionStarted)".into()),
        n = events.len(),
    );
    fs::write(out_dir.join(format!("{}.md", s.name)), md).map_err(|e| e.to_string())?;
    if !keep {
        let _ = fs::remove_dir_all(&scratch);
    }
    Ok(())
}

/// The sanitizers every recording goes through: the structured one with this
/// machine's home, user name, e-mail (`ATRIUM_RECORD_SECRETS`), Claude's encoded
/// project directory name, its per-user temp directory and the time zone name;
/// and a second pass that catches `/Users/user` left behind when the user name
/// was rewritten inside a value the first pass saw only in pieces.
fn sanitizers() -> (Sanitizer, Sanitizer) {
    let home = std::env::var("HOME").unwrap_or_default();
    let user = std::env::var("USER").unwrap_or_default();
    let mut secrets: Vec<String> = std::env::var("ATRIUM_RECORD_SECRETS")
        .unwrap_or_default()
        .split(',')
        .filter(|x| !x.is_empty())
        .map(str::to_owned)
        .collect();
    secrets.push(home.replace('/', "-")); // Claude's encoded project dir name
    if let Ok(out) = std::process::Command::new("id").arg("-u").output() {
        let uid = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if !uid.is_empty() {
            secrets.push(format!("claude-{uid}")); // /private/tmp/claude-<uid>
        }
    }
    if let Ok(target) = fs::read_link("/etc/localtime")
        && let Some(zone) = target.to_string_lossy().split("zoneinfo/").nth(1)
    {
        secrets.push(zone.to_owned());
    }
    let refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
    let first = Sanitizer::new(&home, &user, &refs);
    let second = Sanitizer::new("/Users/user", "user", &[]);
    (first, second)
}

/// Trims, merges split deltas and sanitizes; returns how many delta blocks
/// were merged and the file text.
fn scrub(lines: &mut [FixtureLine]) -> Result<(usize, String), String> {
    let (first, second) = sanitizers();
    for l in lines.iter_mut() {
        trim(&mut l.input);
    }
    let mut merged = merge_split_deltas(lines, &first);
    for l in lines.iter_mut() {
        first.input(&mut l.input);
    }
    merged += merge_split_deltas(lines, &second);
    for l in lines.iter_mut() {
        second.input(&mut l.input);
    }
    let mut text = String::new();
    for l in lines.iter() {
        text.push_str(&serde_json::to_string(l).map_err(|e| e.to_string())?);
        text.push('\n');
    }
    if !first.is_clean(&text) {
        return Err("sanitizer: the recording still has machine-specific values".into());
    }
    Ok((merged, text))
}

/// Re-applies the current scrubbing to the fixtures already on disk (no
/// network, no account use) and regenerates the expected events.
fn resanitize(out_dir: &Path) -> Result<(), String> {
    let mut names: Vec<_> = fs::read_dir(out_dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.strip_suffix(".jsonl").map(str::to_owned)
        })
        .collect();
    names.sort();
    for name in names {
        let path = out_dir.join(format!("{name}.jsonl"));
        let mut lines = fixture::read(&path).map_err(|e| e.to_string())?;
        let (merged, text) = scrub(&mut lines)?;
        fs::write(&path, text).map_err(|e| e.to_string())?;
        let events = fixture::normalize(&lines, &mut ClaudeNormalizer::new());
        let json = serde_json::to_string_pretty(&events).map_err(|e| e.to_string())?;
        fs::write(out_dir.join(format!("{name}.events.json")), json + "\n")
            .map_err(|e| e.to_string())?;
        eprintln!("resanitized {name} ({merged} blocks merged)");
    }
    Ok(())
}

/// A path or name the sanitizer must rewrite can be split across streamed text
/// deltas, where per-line sanitizing can't see it (and the concatenated message
/// would leak it). For such blocks the whole text moves into the first delta
/// and the others become empty. Returns how many blocks were merged.
fn merge_split_deltas(lines: &mut [FixtureLine], san: &Sanitizer) -> usize {
    let mut groups: BTreeMap<(String, u64), Vec<usize>> = BTreeMap::new();
    for (i, l) in lines.iter().enumerate() {
        let AdapterInput::Stdout { line } = &l.input else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("type").and_then(Value::as_str) != Some("stream_event") {
            continue;
        }
        let Some(delta) = v.pointer("/event/delta") else {
            continue;
        };
        if !matches!(
            delta.get("type").and_then(Value::as_str),
            Some("text_delta" | "thinking_delta" | "input_json_delta")
        ) {
            continue;
        }
        let key = (
            v.get("api_message_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            v.pointer("/event/index")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
        groups.entry(key).or_default().push(i);
    }
    let text_of = |l: &FixtureLine| -> (Value, String) {
        let AdapterInput::Stdout { line } = &l.input else {
            unreachable!()
        };
        let v: Value = serde_json::from_str(line).expect("checked above");
        let t = v
            .pointer("/event/delta/text")
            .or_else(|| v.pointer("/event/delta/thinking"))
            .or_else(|| v.pointer("/event/delta/partial_json"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        (v, t)
    };
    let mut merged = 0;
    for idxs in groups.values() {
        let pieces: Vec<String> = idxs.iter().map(|&i| text_of(&lines[i]).1).collect();
        let joined: String = pieces.concat();
        let piecewise: String = pieces.iter().map(|p| san.text(p)).collect();
        if san.text(&joined) == piecewise {
            continue;
        }
        merged += 1;
        for (n, &i) in idxs.iter().enumerate() {
            let (mut v, _) = text_of(&lines[i]);
            let new = if n == 0 {
                joined.clone()
            } else {
                String::new()
            };
            let key = ["text", "thinking", "partial_json"]
                .into_iter()
                .find(|k| v.pointer(&format!("/event/delta/{k}")).is_some())
                .unwrap_or("text");
            v["event"]["delta"][key] = Value::String(new);
            lines[i].input = AdapterInput::Stdout {
                line: v.to_string(),
            };
        }
    }
    merged
}

fn first_started_ms(lines: &[FixtureLine]) -> Option<u64> {
    let mut n = ClaudeNormalizer::new();
    for l in lines {
        let events = atrium_agents::Normalizer::on_input(&mut n, &l.input);
        if events
            .iter()
            .any(|e| matches!(e, AgentEvent::SessionStarted { .. }))
        {
            return Some(l.ms);
        }
    }
    None
}

/// Drops the personal and bulky parts of two records (see the `.md`).
fn trim(input: &mut AdapterInput) {
    let AdapterInput::Stdout { line } = input else {
        return;
    };
    let Ok(mut v) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let mut changed = false;
    if let Some(resp) = v
        .pointer_mut("/response/response")
        .and_then(Value::as_object_mut)
        && resp.contains_key("commands")
    {
        resp.insert("commands".into(), json!([]));
        resp.insert("agents".into(), json!([]));
        resp.insert("models".into(), json!([]));
        resp.insert(
            "account".into(),
            json!({"email": "<redacted>", "organization": "<redacted>", "subscriptionType": "<redacted>", "apiProvider": "firstParty"}),
        );
        changed = true;
    }
    let is_init = v.get("type").and_then(Value::as_str) == Some("system")
        && v.get("subtype").and_then(Value::as_str) == Some("init");
    if is_init && let Some(o) = v.as_object_mut() {
        for k in [
            "slash_commands",
            "terminal_slash_commands",
            "skills",
            "plugins",
            "agents",
        ] {
            if o.contains_key(k) {
                o.insert(k.into(), json!([]));
            }
        }
        changed = true;
    }
    if redact_paths(&mut v) {
        changed = true;
    }
    if changed {
        *line = v.to_string();
    }
}

/// Keys whose values are machine-specific paths and of no use to the mapping.
fn redact_paths(v: &mut Value) -> bool {
    const KEYS: [&str; 7] = [
        "memory_paths",
        "scratchpad_path",
        "scratchpad_dir",
        "messaging_socket_path",
        "transcript_path",
        "user_output_styles_dir",
        "output_file",
    ];
    let mut changed = false;
    match v {
        Value::Object(map) => {
            for (k, val) in map.iter_mut() {
                if KEYS.contains(&k.as_str()) {
                    let new = match val {
                        Value::Object(_) => json!({}),
                        _ => json!("<redacted>"),
                    };
                    if *val != new {
                        *val = new;
                        changed = true;
                    }
                } else if redact_paths(val) {
                    changed = true;
                }
            }
        }
        Value::Array(items) => {
            for i in items {
                if redact_paths(i) {
                    changed = true;
                }
            }
        }
        _ => {}
    }
    changed
}

fn short(e: &AgentEvent) -> String {
    let s = serde_json::to_string(e).unwrap_or_default();
    s.chars().take(160).collect()
}

/// A minimal stdio MCP server with one `echo` tool. Creates the file named by
/// `ATRIUM_MCP_MARKER` when the tool is called.
fn mcp_server() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines().map_while(Result::ok) {
        let Ok(m) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = m.get("id").cloned();
        let method = m.get("method").and_then(Value::as_str).unwrap_or("");
        let result = match method {
            "initialize" => json!({
                "protocolVersion": m.pointer("/params/protocolVersion").cloned().unwrap_or(json!("2024-11-05")),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "echo", "version": "1"},
            }),
            "tools/list" => json!({"tools": [{
                "name": "echo",
                "description": "Echo text back",
                "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]},
            }]}),
            "tools/call" => {
                if let Ok(path) = std::env::var("ATRIUM_MCP_MARKER") {
                    let _ = fs::write(path, "called");
                }
                let text = m
                    .pointer("/params/arguments/text")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                json!({"content": [{"type": "text", "text": format!("echo: {text}")}]})
            }
            _ if id.is_some() => json!({}),
            _ => continue,
        };
        let reply = json!({"jsonrpc": "2.0", "id": id, "result": result});
        let _ = writeln!(out, "{reply}");
        let _ = out.flush();
    }
}
