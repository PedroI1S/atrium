use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use atrium_agents::fixture::FixtureLine;
use atrium_agents::{
    AdapterInput, AgentAdapter, BoxFuture, GateDecision, PermissionPolicy, SessionConfig,
    ToolCheck, ToolGate, Transport,
};
use atrium_protocol::agent::{
    AgentEvent, ApprovalDecision, MessageKind, QuestionAnswer, RequestOutcome, SessionCommand,
    ToolKind, ToolStatus, TurnOutcome,
};
use serde_json::json;
use spike_agent_agy::{AgyAdapter, AgyNormalizer, FixtureSanitizer};
use tokio::sync::mpsc;

const SCENARIO_SECRET: &str = "TOP_SECRET_VALUE_994812_DO_NOT_LEAK";
const DEFAULT_MODEL: &str = "gemini-3.8-flash-medium";

pub const ALL_SCENARIOS: &[&str] = &[
    "simple",
    "tool-read",
    "gate-deny",
    "approval-allow",
    "approval-deny",
    "question",
    "interrupt",
    "multi-turn",
    "resume",
    "startup-error",
];

struct StaticGate {
    mode: GateMode,
}

#[derive(Clone, Copy)]
enum GateMode {
    AllowAll,
    DenySecret,
    AskCommands,
}

impl ToolGate for StaticGate {
    fn check<'a>(&'a self, check: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        Box::pin(async move {
            match self.mode {
                GateMode::AllowAll => GateDecision::Allow,
                GateMode::DenySecret => {
                    let s = check.input.to_string();
                    if s.contains("secret.txt")
                        || matches!(
                            check.kind,
                            ToolKind::Read | ToolKind::Execute | ToolKind::Search
                        )
                        || check.name == "read_file"
                        || check.name == "view_file"
                        || check.name == "run_command"
                    {
                        GateDecision::Deny {
                            reason: "access to secret.txt is denied by Atrium policy".into(),
                        }
                    } else {
                        GateDecision::Allow
                    }
                }
                GateMode::AskCommands => {
                    if check.kind == ToolKind::Execute || check.name == "run_command" {
                        GateDecision::Ask
                    } else {
                        GateDecision::Allow
                    }
                }
            }
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut raw_args = std::env::args().skip(1).peekable();
    let mut fixtures_dir = locate_fixtures_dir()?;
    let mut program: Option<PathBuf> = None;

    while let Some(peek) = raw_args.peek() {
        match peek.as_str() {
            "--binary" => {
                raw_args.next();
                let val = raw_args.next().ok_or("--binary requires a value")?;
                program = Some(PathBuf::from(val));
            }
            "--fixtures-dir" => {
                raw_args.next();
                let val = raw_args.next().ok_or("--fixtures-dir requires a value")?;
                fixtures_dir = PathBuf::from(val);
            }
            _ => break,
        }
    }

    let args: Vec<String> = raw_args.collect();

    if args.first().map(String::as_str) == Some("--generate-handmade") {
        let target = args.get(1).map(String::as_str).unwrap_or("all");
        generate_handmade(&fixtures_dir, target)?;
        return Ok(());
    }

    let Some(scenario) = args.first().map(String::as_str) else {
        eprintln!(
            "Usage:\n  cargo run -p spike-agent-agy --example record -- [--binary <path>] [--fixtures-dir <dir>] <scenario|all>\n  cargo run -p spike-agent-agy --example record -- --generate-handmade [scenario|all]\n\nScenarios: {}",
            ALL_SCENARIOS.join(", ")
        );
        std::process::exit(2);
    };

    if scenario == "all" {
        // Run every scenario, then fail if any did: one bad scenario must not hide the rest.
        let mut failed = Vec::new();
        for &sc in ALL_SCENARIOS {
            if let Err(e) = record_live_scenario(&fixtures_dir, sc, program.clone()).await {
                eprintln!("FAILED {sc}: {e}");
                failed.push(sc);
            }
        }
        if !failed.is_empty() {
            eprintln!("\nscenarios that failed: {}", failed.join(", "));
            std::process::exit(1);
        }
    } else if ALL_SCENARIOS.contains(&scenario) {
        record_live_scenario(&fixtures_dir, scenario, program).await?;
    } else {
        eprintln!(
            "Unknown scenario '{scenario}'. Valid scenarios: {}",
            ALL_SCENARIOS.join(", ")
        );
        std::process::exit(2);
    }

    Ok(())
}

fn locate_fixtures_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .ok_or("failed to locate workspace root from CARGO_MANIFEST_DIR")?;
    let dir = workspace_root.join("fixtures").join("agents").join("agy");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn build_sanitizer(scratch_dirs: &[&Path]) -> FixtureSanitizer {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/user".to_owned());
    let user = std::env::var("USER").unwrap_or_else(|_| "user".to_owned());
    FixtureSanitizer::new(&home, &user, &[SCENARIO_SECRET], scratch_dirs)
}

/// The prompt each scenario sends first (also written into the fixture's `.md`).
fn scenario_prompt(scenario: &str) -> Option<&'static str> {
    match scenario {
        "simple" => Some("Reply with exactly the word PONG and nothing else."),
        "tool-read" => Some("Use read_file to read hello.txt and state its contents."),
        "gate-deny" => {
            Some("Read secret.txt using read_file, or run cat secret.txt with run_command.")
        }
        "approval-allow" => Some("Run `echo approved-ok` using run_command."),
        "approval-deny" => Some("Run `echo should-be-denied` using run_command."),
        "question" => {
            Some("Call ask_question to ask the user which build profile (Debug or Release) to use.")
        }
        "interrupt" => Some("Run `sleep 10` using run_command."),
        "multi-turn" => Some("Reply with TURN_ONE_OK."),
        "resume" => Some("What codeword did I ask you to remember?"),
        "startup-error" => Some("Hello"),
        _ => None,
    }
}

const SESSION_DEADLINE: Duration = Duration::from_secs(90);
const INTERRUPT_GRACE: Duration = Duration::from_secs(5);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(15);

fn base_config(
    cwd: &Path,
    model: Option<String>,
    gate_mode: GateMode,
    resume: Option<String>,
    program: Option<PathBuf>,
) -> SessionConfig {
    SessionConfig {
        cwd: cwd.to_path_buf(),
        model,
        effort: None,
        permissions: PermissionPolicy::Ask,
        gate: Arc::new(StaticGate { mode: gate_mode }),
        mcp_servers: Vec::new(),
        resume,
        env: Vec::new(),
        program,
    }
}

/// Runs the throwaway first session of `resume` and returns its native session id. Waits until
/// the session is completely gone (events channel closed, which happens after the adapter has
/// removed its hook config from the scratch dir), so the resumed session starts clean.
async fn seed_resume_session(
    scratch_dir: &Path,
    program: Option<PathBuf>,
) -> Result<String, Box<dyn std::error::Error>> {
    let (seed_tx, _seed_rx) = mpsc::unbounded_channel();
    let seed_adapter = AgyAdapter::new().with_recorder(seed_tx);
    let cfg = base_config(
        scratch_dir,
        Some(DEFAULT_MODEL.to_owned()),
        GateMode::AllowAll,
        None,
        program,
    );
    let mut session = seed_adapter.start(cfg, Transport::Local).await?;
    session
        .commands
        .send(SessionCommand::SendUserMessage {
            text: "Remember the codeword COBALT-42 and reply OK.".into(),
        })
        .await?;

    let mut native_id: Option<String> = None;
    let mut completed = false;
    let outcome = tokio::time::timeout(SESSION_DEADLINE, async {
        while let Some(env) = session.events.recv().await {
            match env.event {
                AgentEvent::SessionStarted {
                    native_session_id, ..
                } => native_id = Some(native_session_id),
                AgentEvent::TurnCompleted { .. } => {
                    completed = true;
                    break;
                }
                AgentEvent::SessionEnded { .. } => break,
                _ => {}
            }
        }
    })
    .await;
    let _ = session.commands.send(SessionCommand::Shutdown).await;
    let drained = tokio::time::timeout(DRAIN_TIMEOUT, async {
        while session.events.recv().await.is_some() {}
    })
    .await;

    if outcome.is_err() {
        return Err("resume seed session timed out".into());
    }
    if drained.is_err() {
        return Err("resume seed session did not shut down".into());
    }
    let id = native_id
        .ok_or("resume seed session failed to reach SessionStarted; aborting live recording")?;
    if !completed {
        return Err("resume seed session ended before its turn completed".into());
    }
    Ok(id)
}

async fn record_live_scenario(
    fixtures_dir: &Path,
    scenario: &str,
    program: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Recording live scenario: {scenario}");
    let scratch_base = std::env::temp_dir().join("atrium-agy-record-scratch");
    let scratch_dir = spike_agent_agy::create_private_session_dir(&scratch_base, scenario)?;
    let result = record_in_scratch(fixtures_dir, scenario, program, &scratch_dir).await;
    let _ = fs::remove_dir_all(&scratch_dir);
    result
}

async fn record_in_scratch(
    fixtures_dir: &Path,
    scenario: &str,
    program: Option<PathBuf>,
    scratch_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    // Built while the scratch dir exists so its realpath (`/private/var/...`) is known.
    let sanitizer = build_sanitizer(&[scratch_dir]);

    if scenario == "tool-read" {
        fs::write(
            scratch_dir.join("hello.txt"),
            "Hello from Atrium tool-read scenario.\n",
        )?;
    } else if scenario == "gate-deny" {
        fs::write(scratch_dir.join("secret.txt"), SCENARIO_SECRET)?;
    }

    let resume_id = if scenario == "resume" {
        Some(seed_resume_session(scratch_dir, program.clone()).await?)
    } else {
        None
    };

    let (rec_tx, mut rec_rx) = mpsc::unbounded_channel();
    let adapter = AgyAdapter::new().with_recorder(rec_tx);

    let gate_mode = match scenario {
        "gate-deny" => GateMode::DenySecret,
        "approval-allow" | "approval-deny" => GateMode::AskCommands,
        _ => GateMode::AllowAll,
    };
    let model = if scenario == "startup-error" {
        Some("invalid-model-does-not-exist-000".to_owned())
    } else {
        Some(DEFAULT_MODEL.to_owned())
    };
    let cfg = base_config(scratch_dir, model, gate_mode, resume_id, program);

    let mut session = adapter.start(cfg, Transport::Local).await?;

    if let Some(prompt) = scenario_prompt(scenario) {
        session
            .commands
            .send(SessionCommand::SendUserMessage {
                text: prompt.to_owned(),
            })
            .await?;
    }

    let mut turn_count = 0u32;
    let mut timed_out = false;
    let mut shutdown_sent = false;
    let deadline = tokio::time::Instant::now() + SESSION_DEADLINE;
    let mut interrupt_deadline: Option<tokio::time::Instant> = None;

    loop {
        let wake = interrupt_deadline.map_or(deadline, |d| d.min(deadline));
        tokio::select! {
            _ = tokio::time::sleep_until(wake) => {
                if tokio::time::Instant::now() >= deadline {
                    timed_out = true;
                    let _ = session.commands.send(SessionCommand::Shutdown).await;
                    break;
                }
                // Interrupt grace elapsed without the session ending on its own.
                interrupt_deadline = None;
                if !shutdown_sent {
                    shutdown_sent = true;
                    let _ = session.commands.send(SessionCommand::Shutdown).await;
                }
            }
            ev_opt = session.events.recv() => {
                let Some(env) = ev_opt else {
                    break;
                };
                match &env.event {
                    AgentEvent::ApprovalRequested { request_id, .. } => {
                        let decision = if scenario == "approval-allow" {
                            ApprovalDecision::Allow
                        } else {
                            ApprovalDecision::Deny {
                                message: Some("denied by user in approval-deny scenario".into()),
                            }
                        };
                        let _ = session
                            .commands
                            .send(SessionCommand::ResolveApproval {
                                request_id: request_id.clone(),
                                decision,
                            })
                            .await;
                    }
                    AgentEvent::QuestionAsked { request_id, questions } => {
                        let q_id = questions
                            .first()
                            .map(|q| q.id.clone())
                            .unwrap_or_else(|| "q-0".to_owned());
                        let opt_id = questions
                            .first()
                            .and_then(|q| q.options.first())
                            .map(|o| o.id.clone())
                            .unwrap_or_else(|| "opt-0".to_owned());
                        let _ = session
                            .commands
                            .send(SessionCommand::AnswerQuestion {
                                request_id: request_id.clone(),
                                answers: vec![QuestionAnswer {
                                    question_id: q_id,
                                    selected: vec![opt_id],
                                    text: None,
                                }],
                            })
                            .await;
                    }
                    AgentEvent::ToolCallStarted { .. }
                        if scenario == "interrupt" && interrupt_deadline.is_none() && !shutdown_sent =>
                    {
                        // Interrupt mid-tool-call, then give agy a few seconds to end the turn
                        // itself before the harness shuts it down.
                        let _ = session.commands.send(SessionCommand::Interrupt).await;
                        interrupt_deadline = Some(tokio::time::Instant::now() + INTERRUPT_GRACE);
                    }
                    AgentEvent::TurnCompleted { .. } => {
                        turn_count += 1;
                        if scenario == "multi-turn" && turn_count == 1 {
                            let _ = session
                                .commands
                                .send(SessionCommand::SendUserMessage {
                                    text: "Now reply with TURN_TWO_OK.".into(),
                                })
                                .await;
                        } else if !shutdown_sent {
                            shutdown_sent = true;
                            let _ = session.commands.send(SessionCommand::Shutdown).await;
                        }
                    }
                    AgentEvent::SessionEnded { .. } => {
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    // Let the actor finish (kill + reap + Exit input recorded) before collecting the lines.
    let drained = tokio::time::timeout(DRAIN_TIMEOUT, async {
        while session.events.recv().await.is_some() {}
    })
    .await;

    let mut lines = Vec::new();
    while let Ok(line) = rec_rx.try_recv() {
        lines.push(line);
    }

    if timed_out {
        dump_failure(scenario, &lines, &sanitizer);
        return Err(format!(
            "scenario '{scenario}' did not finish within {}s; nothing written",
            SESSION_DEADLINE.as_secs()
        )
        .into());
    }
    if drained.is_err() {
        dump_failure(scenario, &lines, &sanitizer);
        return Err(format!(
            "scenario '{scenario}': session did not shut down within {}s; nothing written",
            DRAIN_TIMEOUT.as_secs()
        )
        .into());
    }

    let res = write_fixture_files(
        fixtures_dir,
        scenario,
        lines.clone(),
        &sanitizer,
        "live",
        "agy 1.4.7",
        DEFAULT_MODEL,
    );
    if res.is_err() {
        dump_failure(scenario, &lines, &sanitizer);
    }
    res
}

/// Writes the sanitized recording of a failed scenario next to the OS temp dir (never into
/// the repo), so the orchestrator can see what happened without spending quota on a re-run.
fn dump_failure(scenario: &str, lines: &[FixtureLine], sanitizer: &FixtureSanitizer) {
    let dir = std::env::temp_dir().join("atrium-agy-record-failures");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let mut out = String::new();
    for line in lines {
        let mut line = line.clone();
        sanitizer.input(&mut line.input);
        if let Ok(s) = serde_json::to_string(&line) {
            out.push_str(&s);
            out.push('\n');
        }
    }
    let path = dir.join(format!("{scenario}.sanitized.jsonl"));
    if fs::write(&path, out).is_ok() {
        eprintln!(
            "  sanitized recording of the failed run: {}",
            path.display()
        );
    }
}

fn assistant_text(events: &[AgentEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::MessageCompleted {
                kind: MessageKind::Assistant,
                text,
                ..
            } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn count_finished(events: &[AgentEvent], status: ToolStatus) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCallFinished { status: s, .. } if *s == status))
        .count()
}

fn outcomes(events: &[AgentEvent]) -> Vec<RequestOutcome> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::RequestResolved { outcome, .. } => Some(*outcome),
            _ => None,
        })
        .collect()
}

/// What each scenario must show for the fixture to be worth committing. Returns every unmet
/// expectation, so a failed live run says what is wrong in one go.
fn check_expectations(scenario: &str, events: &[AgentEvent]) -> Result<(), Vec<String>> {
    let mut problems: Vec<String> = Vec::new();
    let mut expect = |ok: bool, what: &str| {
        if !ok {
            problems.push(what.to_owned());
        }
    };

    let started = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::SessionStarted { .. }))
        .count();
    let completed_turns = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                AgentEvent::TurnCompleted {
                    outcome: TurnOutcome::Completed,
                    ..
                }
            )
        })
        .count();
    let tool_started = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCallStarted { .. }))
        .count();
    let tool_finished = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCallFinished { .. }))
        .count();
    let approvals = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ApprovalRequested { .. }))
        .count();
    let questions = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::QuestionAsked { .. }))
        .count();
    let resolved = outcomes(events);
    let text = assistant_text(events).to_lowercase();
    let ran = count_finished(events, ToolStatus::Succeeded);
    let denied = count_finished(events, ToolStatus::Denied);

    expect(
        matches!(events.last(), Some(AgentEvent::SessionEnded { .. })),
        "the last event is SessionEnded",
    );
    expect(
        tool_started == tool_finished,
        "every ToolCallStarted has a ToolCallFinished",
    );
    expect(
        approvals + questions == resolved.len(),
        "every ApprovalRequested / QuestionAsked has exactly one RequestResolved",
    );
    expect(
        !events.iter().any(|e| {
            matches!(e, AgentEvent::Error { message, .. } if message.contains("gate bypass suspected"))
        }),
        "the adapter's gate-bypass tripwire did not fire",
    );

    if scenario != "startup-error" {
        expect(started == 1, "exactly one SessionStarted");
        expect(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::Error { fatal: true, .. })),
            "no fatal Error",
        );
    }

    match scenario {
        "simple" => {
            expect(text.contains("pong"), "the reply contains PONG");
            expect(completed_turns == 1, "one completed turn");
            expect(tool_started == 0, "no tool calls");
        }
        "tool-read" => {
            expect(
                events.iter().any(|e| {
                    matches!(
                        e,
                        AgentEvent::ToolCallStarted {
                            kind: ToolKind::Read,
                            ..
                        }
                    )
                }),
                "a read tool call started",
            );
            expect(ran >= 1, "the read tool call succeeded");
            expect(
                text.contains("hello from atrium"),
                "the reply quotes hello.txt",
            );
            expect(completed_turns >= 1, "a completed turn");
        }
        "gate-deny" => {
            expect(denied >= 1, "at least one tool call was denied");
            expect(ran == 0, "no tool call succeeded");
        }
        "approval-allow" => {
            expect(approvals >= 1, "an ApprovalRequested");
            expect(
                resolved.contains(&RequestOutcome::Approved),
                "the approval resolved as approved",
            );
            expect(ran >= 1, "the approved command ran");
            expect(completed_turns >= 1, "a completed turn");
        }
        "approval-deny" => {
            expect(approvals >= 1, "an ApprovalRequested");
            expect(
                resolved.contains(&RequestOutcome::Denied),
                "the approval resolved as denied",
            );
            expect(denied >= 1, "the denied command ended denied");
            expect(ran == 0, "no tool call succeeded");
        }
        "question" => {
            expect(questions >= 1, "a QuestionAsked");
            expect(
                resolved.contains(&RequestOutcome::Answered),
                "the question resolved as answered",
            );
        }
        "interrupt" => {
            expect(
                tool_started >= 1,
                "a tool call started before the interrupt",
            );
            expect(
                events.iter().any(|e| {
                    matches!(
                        e,
                        AgentEvent::TurnCompleted {
                            outcome: TurnOutcome::Interrupted,
                            ..
                        }
                    )
                }),
                "the turn ended interrupted",
            );
            expect(ran == 0, "the interrupted tool call did not succeed");
        }
        "multi-turn" => {
            expect(completed_turns >= 2, "two completed turns in one session");
            expect(text.contains("turn_one_ok"), "the reply to turn 1");
            expect(text.contains("turn_two_ok"), "the reply to turn 2");
        }
        "resume" => {
            expect(
                text.contains("cobalt-42"),
                "the resumed session remembers COBALT-42",
            );
            expect(completed_turns >= 1, "a completed turn");
        }
        "startup-error" => {
            expect(started == 0, "no SessionStarted");
            expect(
                events
                    .iter()
                    .any(|e| matches!(e, AgentEvent::Error { fatal: true, .. })),
                "a fatal Error",
            );
        }
        _ => {}
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

fn write_fixture_files(
    fixtures_dir: &Path,
    scenario: &str,
    mut lines: Vec<FixtureLine>,
    sanitizer: &FixtureSanitizer,
    source_kind: &str,
    cli_version: &str,
    model: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // The scenario secret is a `Sanitizer` secret, so it would silently turn into
    // `<redacted>` below. Look for it in the raw recording first: if it is there, the agent
    // read the file, and the gate did not hold.
    let raw: String = lines
        .iter()
        .filter_map(|l| serde_json::to_string(l).ok())
        .collect::<Vec<_>>()
        .join("\n");
    if raw.contains(SCENARIO_SECRET) {
        return Err(format!(
            "scenario '{scenario}' ({source_kind}): the secret value is in the raw recording, \
             so the agent read secret.txt despite the gate; refusing to write a fixture"
        )
        .into());
    }

    for line in &mut lines {
        sanitizer.input(&mut line.input);
    }

    let mut jsonl_out = String::new();
    for line in &lines {
        let s = serde_json::to_string(line)?;
        jsonl_out.push_str(&s);
        jsonl_out.push('\n');
    }
    if !sanitizer.is_clean(&jsonl_out) {
        return Err(format!(
            "Sanitizer::is_clean failed on {scenario}.jsonl (a temp-dir path, home path, user \
             name or secret survived); nothing written"
        )
        .into());
    }

    let mut normalizer = AgyNormalizer::new();
    let events = atrium_agents::fixture::normalize(&lines, &mut normalizer);

    if let Err(problems) = check_expectations(scenario, &events) {
        let errs: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::Error { message, .. } => Some(message.as_str()),
                _ => None,
            })
            .collect();
        return Err(format!(
            "scenario '{scenario}' ({source_kind}) did not meet its expectations; refusing to \
             write a broken fixture:\n  - {}\n  agent errors: {errs:?}",
            problems.join("\n  - ")
        )
        .into());
    }

    let events_json = serde_json::to_string_pretty(&events)? + "\n";
    if !sanitizer.is_clean(&events_json) {
        return Err(format!(
            "Sanitizer::is_clean failed on {scenario}.events.json; nothing written"
        )
        .into());
    }

    let jsonl_path = fixtures_dir.join(format!("{scenario}.jsonl"));
    let events_path = fixtures_dir.join(format!("{scenario}.events.json"));
    let md_path = fixtures_dir.join(format!("{scenario}.md"));

    fs::write(&jsonl_path, &jsonl_out)?;
    fs::write(&events_path, &events_json)?;

    let prompt_line = scenario_prompt(scenario)
        .map(|p| format!("- Prompt: `{p}`\n"))
        .unwrap_or_default();
    let paths_line = if source_kind == "live" {
        "- Paths: the scratch working directory is recorded as `/scratch`, other temp-dir roots as `/scratch-tmp`, the home directory as `~`.\n"
    } else {
        ""
    };
    let md_content = format!(
        "# {scenario}\n\n- Status: `{source_kind}`\n- CLI: `{cli_version}`\n- Model: `{model}`\n- Command: `agy --print= --input-format stream-json --output-format stream-json --mode accept-edits --dangerously-skip-permissions --print-timeout 0s`\n{prompt_line}- Scenario: `{scenario}` (`docs/contracts/m0.md` §3.3)\n{paths_line}"
    );
    fs::write(&md_path, md_content)?;
    println!("Wrote {}", jsonl_path.display());
    Ok(())
}

fn generate_handmade(fixtures_dir: &Path, target: &str) -> Result<(), Box<dyn std::error::Error>> {
    // First ensure m0-00-smoke.events.json is generated from m0-00-smoke.jsonl.
    let smoke_jsonl = fixtures_dir.join("m0-00-smoke.jsonl");
    if smoke_jsonl.exists() {
        let smoke_lines = atrium_agents::fixture::read(&smoke_jsonl)?;
        let mut normalizer = AgyNormalizer::new();
        let events = atrium_agents::fixture::normalize(&smoke_lines, &mut normalizer);
        let events_json = serde_json::to_string_pretty(&events)? + "\n";
        let sanitizer = build_sanitizer(&[]);
        assert!(sanitizer.is_clean(&events_json));
        fs::write(fixtures_dir.join("m0-00-smoke.events.json"), events_json)?;
    }

    let scenarios_to_build: Vec<&str> = if target == "all" {
        ALL_SCENARIOS.to_vec()
    } else {
        vec![target]
    };

    for sc in scenarios_to_build {
        let lines = handmade_scenario_lines(sc);
        let model = if sc == "startup-error" {
            "invalid-model-does-not-exist"
        } else {
            DEFAULT_MODEL
        };
        let sanitizer = build_sanitizer(&[]);
        write_fixture_files(
            fixtures_dir,
            sc,
            lines,
            &sanitizer,
            "hand-made",
            "agy 1.4.7",
            model,
        )?;
    }
    Ok(())
}

fn init_line(conv_id: &str, model: &str) -> String {
    json!({
        "event": "init",
        "conversation_id": conv_id,
        "init": {
            "model": model,
            "cwd": "~/Documents/atrium-wt/M0-03-agy-adapter",
            "tools": [
                "ask_question",
                "call_mcp_tool",
                "edit_file",
                "find_by_name",
                "grep_search",
                "list_dir",
                "read_file",
                "run_command",
                "search_web",
                "view_file",
                "write_file"
            ],
            "permission_mode": "always-proceed"
        }
    })
    .to_string()
}

fn user_stdin_line(text: &str) -> String {
    spike_agent_agy::StdinFormat::default().format_message(text)
}

fn handmade_scenario_lines(scenario: &str) -> Vec<FixtureLine> {
    match scenario {
        "simple" => {
            let conv = "11111111-0001-4000-8000-000000000001";
            vec![
                FixtureLine {
                    ms: 0,
                    input: AdapterInput::Command {
                        command: SessionCommand::SendUserMessage {
                            text: "Reply with PONG.".into(),
                        },
                    },
                },
                FixtureLine {
                    ms: 1,
                    input: AdapterInput::Stdin {
                        line: user_stdin_line("Reply with PONG."),
                    },
                },
                FixtureLine {
                    ms: 15,
                    input: AdapterInput::Stdout {
                        line: init_line(conv, DEFAULT_MODEL),
                    },
                },
                FixtureLine {
                    ms: 20,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 0,
                                "state": "DONE",
                                "step_type": "user_input"
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 120,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 1,
                                "state": "ACTIVE",
                                "step_type": "agent_response",
                                "text_delta": "PO"
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 140,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 1,
                                "state": "DONE",
                                "step_type": "agent_response",
                                "text_delta": "NG",
                                "duration_seconds": 0.12,
                                "usage": {
                                    "input_tokens": 120,
                                    "output_tokens": 4,
                                    "thinking_tokens": 8,
                                    "cache_read_tokens": 0,
                                    "total_tokens": 124
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 145,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "result",
                            "result": {
                                "conversation_id": conv,
                                "status": "SUCCESS",
                                "response": "PONG",
                                "duration_seconds": 0.14,
                                "num_turns": 1,
                                "usage": {
                                    "input_tokens": 120,
                                    "output_tokens": 4,
                                    "thinking_tokens": 8,
                                    "cache_read_tokens": 0,
                                    "total_tokens": 124
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 150,
                    input: AdapterInput::Exit { code: Some(0) },
                },
            ]
        }

        "tool-read" => {
            let conv = "11111111-0002-4000-8000-000000000002";
            let params = json!({"Path": "~/scratch/hello.txt"});
            vec![
                FixtureLine {
                    ms: 0,
                    input: AdapterInput::Command {
                        command: SessionCommand::SendUserMessage {
                            text: "Read hello.txt and tell me what it says.".into(),
                        },
                    },
                },
                FixtureLine {
                    ms: 1,
                    input: AdapterInput::Stdin {
                        line: user_stdin_line("Read hello.txt and tell me what it says."),
                    },
                },
                FixtureLine {
                    ms: 10,
                    input: AdapterInput::Stdout {
                        line: init_line(conv, DEFAULT_MODEL),
                    },
                },
                FixtureLine {
                    ms: 15,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 0,
                                "state": "DONE",
                                "step_type": "user_input"
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 60,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 1,
                                "state": "DONE",
                                "step_type": "agent_response",
                                "duration_seconds": 0.04,
                                "usage": {
                                    "input_tokens": 200,
                                    "output_tokens": 18,
                                    "thinking_tokens": 10,
                                    "cache_read_tokens": 0,
                                    "total_tokens": 218
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 65,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 2,
                                "state": "ACTIVE",
                                "step_type": "tool",
                                "tool_name": "read_file",
                                "tool_info": {
                                    "name": "read_file",
                                    "parameters": params
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 68,
                    input: AdapterInput::HookRequest {
                        id: "req-1".into(),
                        payload: json!({
                            "toolCall": {
                                "name": "read_file",
                                "args": params
                            },
                            "workspacePaths": ["~/scratch"]
                        }),
                    },
                },
                FixtureLine {
                    ms: 70,
                    input: AdapterInput::Gate {
                        check: ToolCheck {
                            call_id: Some("step-2".into()),
                            name: "read_file".into(),
                            kind: ToolKind::Read,
                            title: Some("~/scratch/hello.txt".into()),
                            input: params.clone(),
                        },
                        decision: GateDecision::Allow,
                    },
                },
                FixtureLine {
                    ms: 72,
                    input: AdapterInput::HookResponse {
                        id: "req-1".into(),
                        payload: json!({"decision": "allow"}),
                    },
                },
                FixtureLine {
                    ms: 85,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 2,
                                "state": "DONE",
                                "step_type": "tool",
                                "tool_name": "read_file",
                                "duration_seconds": 0.02,
                                "tool_info": {
                                    "name": "read_file",
                                    "parameters": params,
                                    "output": "1: Hello from Atrium tool-read scenario.\n"
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 130,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 3,
                                "state": "ACTIVE",
                                "step_type": "agent_response",
                                "text_delta": "The file says: Hello from Atrium tool-read scenario."
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 140,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 3,
                                "state": "DONE",
                                "step_type": "agent_response",
                                "duration_seconds": 0.05,
                                "usage": {
                                    "input_tokens": 250,
                                    "output_tokens": 14,
                                    "thinking_tokens": 6,
                                    "cache_read_tokens": 0,
                                    "total_tokens": 264
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 145,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "result",
                            "result": {
                                "conversation_id": conv,
                                "status": "SUCCESS",
                                "response": "The file says: Hello from Atrium tool-read scenario.",
                                "duration_seconds": 0.13,
                                "num_turns": 1,
                                "usage": {
                                    "input_tokens": 450,
                                    "output_tokens": 32,
                                    "thinking_tokens": 16,
                                    "cache_read_tokens": 0,
                                    "total_tokens": 482
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 150,
                    input: AdapterInput::Exit { code: Some(0) },
                },
            ]
        }

        "gate-deny" => {
            let conv = "11111111-0003-4000-8000-000000000003";
            let read_params = json!({"Path": "~/scratch/secret.txt"});
            let exec_params = json!({"CommandLine": "cat ~/scratch/secret.txt"});
            vec![
                FixtureLine {
                    ms: 0,
                    input: AdapterInput::Command {
                        command: SessionCommand::SendUserMessage {
                            text: "Read secret.txt in the scratch directory.".into(),
                        },
                    },
                },
                FixtureLine {
                    ms: 1,
                    input: AdapterInput::Stdin {
                        line: user_stdin_line("Read secret.txt in the scratch directory."),
                    },
                },
                FixtureLine {
                    ms: 10,
                    input: AdapterInput::Stdout {
                        line: init_line(conv, DEFAULT_MODEL),
                    },
                },
                FixtureLine {
                    ms: 15,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 0,
                                "state": "DONE",
                                "step_type": "user_input"
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 50,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 1,
                                "state": "DONE",
                                "step_type": "agent_response",
                                "duration_seconds": 0.03,
                                "usage": {
                                    "input_tokens": 210,
                                    "output_tokens": 20,
                                    "thinking_tokens": 12,
                                    "cache_read_tokens": 0,
                                    "total_tokens": 230
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                // 1. Agent attempts `read_file` on `secret.txt`; gate denies it before execution.
                FixtureLine {
                    ms: 55,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 2,
                                "state": "ACTIVE",
                                "step_type": "tool",
                                "tool_name": "read_file",
                                "tool_info": {
                                    "name": "read_file",
                                    "parameters": read_params
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 58,
                    input: AdapterInput::HookRequest {
                        id: "req-1".into(),
                        payload: json!({
                            "toolCall": {
                                "name": "read_file",
                                "args": read_params
                            },
                            "workspacePaths": ["~/scratch"]
                        }),
                    },
                },
                FixtureLine {
                    ms: 60,
                    input: AdapterInput::Gate {
                        check: ToolCheck {
                            call_id: Some("step-2".into()),
                            name: "read_file".into(),
                            kind: ToolKind::Read,
                            title: Some("~/scratch/secret.txt".into()),
                            input: read_params.clone(),
                        },
                        decision: GateDecision::Deny {
                            reason: "reading secret.txt is denied by Atrium policy".into(),
                        },
                    },
                },
                FixtureLine {
                    ms: 62,
                    input: AdapterInput::HookResponse {
                        id: "req-1".into(),
                        payload: json!({
                            "decision": "deny",
                            "reason": "reading secret.txt is denied by Atrium policy"
                        }),
                    },
                },
                FixtureLine {
                    ms: 66,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 2,
                                "state": "ERROR",
                                "step_type": "tool",
                                "tool_name": "read_file",
                                "duration_seconds": 0.01,
                                "tool_info": {
                                    "name": "read_file",
                                    "parameters": read_params,
                                    "error": {
                                        "type": "TOOL_ERROR",
                                        "message": "tool call denied by pre-tool hook: reading secret.txt is denied by Atrium policy"
                                    }
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                // 2. Agent attempts `run_command` (`cat ~/scratch/secret.txt`); gate denies it too.
                FixtureLine {
                    ms: 90,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 3,
                                "state": "DONE",
                                "step_type": "agent_response",
                                "duration_seconds": 0.02,
                                "usage": {
                                    "input_tokens": 260,
                                    "output_tokens": 22,
                                    "thinking_tokens": 10,
                                    "cache_read_tokens": 0,
                                    "total_tokens": 282
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 95,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 4,
                                "state": "ACTIVE",
                                "step_type": "tool",
                                "tool_name": "run_command",
                                "tool_info": {
                                    "name": "run_command",
                                    "parameters": exec_params
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 98,
                    input: AdapterInput::HookRequest {
                        id: "req-2".into(),
                        payload: json!({
                            "toolCall": {
                                "name": "run_command",
                                "args": exec_params
                            },
                            "workspacePaths": ["~/scratch"]
                        }),
                    },
                },
                FixtureLine {
                    ms: 100,
                    input: AdapterInput::Gate {
                        check: ToolCheck {
                            call_id: Some("step-4".into()),
                            name: "run_command".into(),
                            kind: ToolKind::Execute,
                            title: Some("cat ~/scratch/secret.txt".into()),
                            input: exec_params.clone(),
                        },
                        decision: GateDecision::Deny {
                            reason: "executing cat on secret.txt is denied by Atrium policy".into(),
                        },
                    },
                },
                FixtureLine {
                    ms: 102,
                    input: AdapterInput::HookResponse {
                        id: "req-2".into(),
                        payload: json!({
                            "decision": "deny",
                            "reason": "executing cat on secret.txt is denied by Atrium policy"
                        }),
                    },
                },
                FixtureLine {
                    ms: 106,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 4,
                                "state": "ERROR",
                                "step_type": "tool",
                                "tool_name": "run_command",
                                "duration_seconds": 0.01,
                                "tool_info": {
                                    "name": "run_command",
                                    "parameters": exec_params,
                                    "error": {
                                        "type": "TOOL_ERROR",
                                        "message": "tool call denied by pre-tool hook: executing cat on secret.txt is denied by Atrium policy"
                                    }
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 140,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 5,
                                "state": "DONE",
                                "step_type": "agent_response",
                                "text_delta": "Both read_file and run_command on secret.txt were denied by Atrium policy.",
                                "duration_seconds": 0.03,
                                "usage": {
                                    "input_tokens": 310,
                                    "output_tokens": 18,
                                    "thinking_tokens": 8,
                                    "cache_read_tokens": 0,
                                    "total_tokens": 328
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 145,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "result",
                            "result": {
                                "conversation_id": conv,
                                "status": "SUCCESS",
                                "response": "Both read_file and run_command on secret.txt were denied by Atrium policy.",
                                "duration_seconds": 0.14,
                                "num_turns": 1,
                                "usage": {
                                    "input_tokens": 780,
                                    "output_tokens": 60,
                                    "thinking_tokens": 30,
                                    "cache_read_tokens": 0,
                                    "total_tokens": 840
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 150,
                    input: AdapterInput::Exit { code: Some(0) },
                },
            ]
        }

        "approval-allow" => {
            let conv = "11111111-0004-4000-8000-000000000004";
            let cmd_params = json!({"CommandLine": "echo approved-ok"});
            vec![
                FixtureLine {
                    ms: 0,
                    input: AdapterInput::Command {
                        command: SessionCommand::SendUserMessage {
                            text: "Run echo approved-ok.".into(),
                        },
                    },
                },
                FixtureLine {
                    ms: 1,
                    input: AdapterInput::Stdin {
                        line: user_stdin_line("Run echo approved-ok."),
                    },
                },
                FixtureLine {
                    ms: 10,
                    input: AdapterInput::Stdout {
                        line: init_line(conv, DEFAULT_MODEL),
                    },
                },
                FixtureLine {
                    ms: 15,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 0,
                                "state": "DONE",
                                "step_type": "user_input"
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 40,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 1,
                                "state": "DONE",
                                "step_type": "agent_response",
                                "duration_seconds": 0.02,
                                "usage": {
                                    "input_tokens": 180,
                                    "output_tokens": 15,
                                    "thinking_tokens": 5,
                                    "cache_read_tokens": 0,
                                    "total_tokens": 195
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 45,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 2,
                                "state": "ACTIVE",
                                "step_type": "tool",
                                "tool_name": "run_command",
                                "tool_info": {
                                    "name": "run_command",
                                    "parameters": cmd_params
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 48,
                    input: AdapterInput::HookRequest {
                        id: "req-1".into(),
                        payload: json!({
                            "toolCall": {
                                "name": "run_command",
                                "args": cmd_params
                            },
                            "workspacePaths": ["~/scratch"]
                        }),
                    },
                },
                FixtureLine {
                    ms: 50,
                    input: AdapterInput::Gate {
                        check: ToolCheck {
                            call_id: Some("step-2".into()),
                            name: "run_command".into(),
                            kind: ToolKind::Execute,
                            title: Some("echo approved-ok".into()),
                            input: cmd_params.clone(),
                        },
                        decision: GateDecision::Ask,
                    },
                },
                FixtureLine {
                    ms: 90,
                    input: AdapterInput::Command {
                        command: SessionCommand::ResolveApproval {
                            request_id: "req-1".into(),
                            decision: ApprovalDecision::Allow,
                        },
                    },
                },
                FixtureLine {
                    ms: 92,
                    input: AdapterInput::HookResponse {
                        id: "req-1".into(),
                        payload: json!({"decision": "allow"}),
                    },
                },
                FixtureLine {
                    ms: 110,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 2,
                                "state": "DONE",
                                "step_type": "tool",
                                "tool_name": "run_command",
                                "duration_seconds": 0.06,
                                "tool_info": {
                                    "name": "run_command",
                                    "parameters": cmd_params,
                                    "output": "Command exited with code 0.\n\nCommand output:\napproved-ok\n"
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 140,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 3,
                                "state": "DONE",
                                "step_type": "agent_response",
                                "text_delta": "The command ran and printed approved-ok.",
                                "duration_seconds": 0.03,
                                "usage": {
                                    "input_tokens": 220,
                                    "output_tokens": 12,
                                    "thinking_tokens": 4,
                                    "cache_read_tokens": 0,
                                    "total_tokens": 232
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 145,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "result",
                            "result": {
                                "conversation_id": conv,
                                "status": "SUCCESS",
                                "response": "The command ran and printed approved-ok.",
                                "duration_seconds": 0.14,
                                "num_turns": 1,
                                "usage": {
                                    "input_tokens": 400,
                                    "output_tokens": 27,
                                    "thinking_tokens": 9,
                                    "cache_read_tokens": 0,
                                    "total_tokens": 427
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 150,
                    input: AdapterInput::Exit { code: Some(0) },
                },
            ]
        }

        "approval-deny" => {
            let conv = "11111111-0005-4000-8000-000000000005";
            let cmd_params = json!({"CommandLine": "echo should-be-denied"});
            vec![
                FixtureLine {
                    ms: 0,
                    input: AdapterInput::Command {
                        command: SessionCommand::SendUserMessage {
                            text: "Run echo should-be-denied.".into(),
                        },
                    },
                },
                FixtureLine {
                    ms: 1,
                    input: AdapterInput::Stdin {
                        line: user_stdin_line("Run echo should-be-denied."),
                    },
                },
                FixtureLine {
                    ms: 10,
                    input: AdapterInput::Stdout {
                        line: init_line(conv, DEFAULT_MODEL),
                    },
                },
                FixtureLine {
                    ms: 15,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 0,
                                "state": "DONE",
                                "step_type": "user_input"
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 40,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 1,
                                "state": "DONE",
                                "step_type": "agent_response",
                                "duration_seconds": 0.02,
                                "usage": {
                                    "input_tokens": 180,
                                    "output_tokens": 15,
                                    "thinking_tokens": 5,
                                    "cache_read_tokens": 0,
                                    "total_tokens": 195
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 45,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 2,
                                "state": "ACTIVE",
                                "step_type": "tool",
                                "tool_name": "run_command",
                                "tool_info": {
                                    "name": "run_command",
                                    "parameters": cmd_params
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 48,
                    input: AdapterInput::HookRequest {
                        id: "req-1".into(),
                        payload: json!({
                            "toolCall": {
                                "name": "run_command",
                                "args": cmd_params
                            },
                            "workspacePaths": ["~/scratch"]
                        }),
                    },
                },
                FixtureLine {
                    ms: 50,
                    input: AdapterInput::Gate {
                        check: ToolCheck {
                            call_id: Some("step-2".into()),
                            name: "run_command".into(),
                            kind: ToolKind::Execute,
                            title: Some("echo should-be-denied".into()),
                            input: cmd_params.clone(),
                        },
                        decision: GateDecision::Ask,
                    },
                },
                FixtureLine {
                    ms: 90,
                    input: AdapterInput::Command {
                        command: SessionCommand::ResolveApproval {
                            request_id: "req-1".into(),
                            decision: ApprovalDecision::Deny {
                                message: Some("denied by user in approval-deny scenario".into()),
                            },
                        },
                    },
                },
                FixtureLine {
                    ms: 92,
                    input: AdapterInput::HookResponse {
                        id: "req-1".into(),
                        payload: json!({
                            "decision": "deny",
                            "reason": "denied by user in approval-deny scenario"
                        }),
                    },
                },
                FixtureLine {
                    ms: 105,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 2,
                                "state": "ERROR",
                                "step_type": "tool",
                                "tool_name": "run_command",
                                "duration_seconds": 0.05,
                                "tool_info": {
                                    "name": "run_command",
                                    "parameters": cmd_params,
                                    "error": {
                                        "type": "TOOL_ERROR",
                                        "message": "tool call denied by pre-tool hook: denied by user in approval-deny scenario"
                                    }
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 140,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 3,
                                "state": "DONE",
                                "step_type": "agent_response",
                                "text_delta": "The command was denied by the user.",
                                "duration_seconds": 0.03,
                                "usage": {
                                    "input_tokens": 220,
                                    "output_tokens": 10,
                                    "thinking_tokens": 4,
                                    "cache_read_tokens": 0,
                                    "total_tokens": 230
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 145,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "result",
                            "result": {
                                "conversation_id": conv,
                                "status": "SUCCESS",
                                "response": "The command was denied by the user.",
                                "duration_seconds": 0.14,
                                "num_turns": 1,
                                "usage": {
                                    "input_tokens": 400,
                                    "output_tokens": 25,
                                    "thinking_tokens": 9,
                                    "cache_read_tokens": 0,
                                    "total_tokens": 425
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 150,
                    input: AdapterInput::Exit { code: Some(0) },
                },
            ]
        }

        "question" => {
            let conv = "11111111-0006-4000-8000-000000000006";
            let q_params = json!({
                "questions": [
                    {
                        "question": "Which build profile should we use?",
                        "options": ["(Recommended) Debug", "Release"],
                        "is_multi_select": false
                    }
                ]
            });
            vec![
                FixtureLine {
                    ms: 0,
                    input: AdapterInput::Command {
                        command: SessionCommand::SendUserMessage {
                            text: "Ask me which build profile to use.".into(),
                        },
                    },
                },
                FixtureLine {
                    ms: 1,
                    input: AdapterInput::Stdin {
                        line: user_stdin_line("Ask me which build profile to use."),
                    },
                },
                FixtureLine {
                    ms: 10,
                    input: AdapterInput::Stdout {
                        line: init_line(conv, DEFAULT_MODEL),
                    },
                },
                FixtureLine {
                    ms: 15,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 0,
                                "state": "DONE",
                                "step_type": "user_input"
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 40,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 1,
                                "state": "DONE",
                                "step_type": "agent_response",
                                "duration_seconds": 0.02,
                                "usage": {
                                    "input_tokens": 190,
                                    "output_tokens": 20,
                                    "thinking_tokens": 6,
                                    "cache_read_tokens": 0,
                                    "total_tokens": 210
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 45,
                    input: AdapterInput::HookRequest {
                        id: "req-q1".into(),
                        payload: json!({
                            "toolCall": {
                                "name": "ask_question",
                                "args": q_params
                            },
                            "workspacePaths": ["~/scratch"]
                        }),
                    },
                },
                FixtureLine {
                    ms: 48,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 2,
                                "state": "ACTIVE",
                                "step_type": "tool",
                                "tool_name": "ask_question",
                                "tool_info": {
                                    "name": "ask_question",
                                    "parameters": q_params
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 90,
                    input: AdapterInput::Command {
                        command: SessionCommand::AnswerQuestion {
                            request_id: "req-q1".into(),
                            answers: vec![QuestionAnswer {
                                question_id: "q-0".into(),
                                selected: vec!["opt-0".into()],
                                text: None,
                            }],
                        },
                    },
                },
                FixtureLine {
                    ms: 92,
                    input: AdapterInput::HookResponse {
                        id: "req-q1".into(),
                        payload: json!({
                            "decision": "allow",
                            "reason": "User answered: q-0: Debug"
                        }),
                    },
                },
                FixtureLine {
                    ms: 100,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 2,
                                "state": "DONE",
                                "step_type": "tool",
                                "tool_name": "ask_question",
                                "duration_seconds": 0.05,
                                "tool_info": {
                                    "name": "ask_question",
                                    "parameters": q_params,
                                    "output": "Selected: Debug"
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 130,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 3,
                                "state": "DONE",
                                "step_type": "agent_response",
                                "text_delta": "Proceeding with the Debug build profile.",
                                "duration_seconds": 0.03,
                                "usage": {
                                    "input_tokens": 230,
                                    "output_tokens": 10,
                                    "thinking_tokens": 4,
                                    "cache_read_tokens": 0,
                                    "total_tokens": 240
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 140,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "result",
                            "result": {
                                "conversation_id": conv,
                                "status": "SUCCESS",
                                "response": "Proceeding with the Debug build profile.",
                                "duration_seconds": 0.14,
                                "num_turns": 1,
                                "usage": {
                                    "input_tokens": 420,
                                    "output_tokens": 30,
                                    "thinking_tokens": 10,
                                    "cache_read_tokens": 0,
                                    "total_tokens": 450
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 145,
                    input: AdapterInput::Exit { code: Some(0) },
                },
            ]
        }

        "interrupt" => {
            let conv = "11111111-0007-4000-8000-000000000007";
            let cmd_params = json!({"CommandLine": "sleep 10"});
            vec![
                FixtureLine {
                    ms: 0,
                    input: AdapterInput::Command {
                        command: SessionCommand::SendUserMessage {
                            text: "Run sleep 10.".into(),
                        },
                    },
                },
                FixtureLine {
                    ms: 1,
                    input: AdapterInput::Stdin {
                        line: user_stdin_line("Run sleep 10."),
                    },
                },
                FixtureLine {
                    ms: 10,
                    input: AdapterInput::Stdout {
                        line: init_line(conv, DEFAULT_MODEL),
                    },
                },
                FixtureLine {
                    ms: 15,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 0,
                                "state": "DONE",
                                "step_type": "user_input"
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 40,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 1,
                                "state": "DONE",
                                "step_type": "agent_response",
                                "duration_seconds": 0.02,
                                "usage": {
                                    "input_tokens": 160,
                                    "output_tokens": 12,
                                    "thinking_tokens": 4,
                                    "cache_read_tokens": 0,
                                    "total_tokens": 172
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 45,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 2,
                                "state": "ACTIVE",
                                "step_type": "tool",
                                "tool_name": "run_command",
                                "tool_info": {
                                    "name": "run_command",
                                    "parameters": cmd_params
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 48,
                    input: AdapterInput::HookRequest {
                        id: "req-1".into(),
                        payload: json!({
                            "toolCall": {
                                "name": "run_command",
                                "args": cmd_params
                            },
                            "workspacePaths": ["~/scratch"]
                        }),
                    },
                },
                FixtureLine {
                    ms: 50,
                    input: AdapterInput::Gate {
                        check: ToolCheck {
                            call_id: Some("step-2".into()),
                            name: "run_command".into(),
                            kind: ToolKind::Execute,
                            title: Some("sleep 10".into()),
                            input: cmd_params,
                        },
                        decision: GateDecision::Allow,
                    },
                },
                FixtureLine {
                    ms: 52,
                    input: AdapterInput::HookResponse {
                        id: "req-1".into(),
                        payload: json!({"decision": "allow"}),
                    },
                },
                FixtureLine {
                    ms: 80,
                    input: AdapterInput::Command {
                        command: SessionCommand::Interrupt,
                    },
                },
                FixtureLine {
                    ms: 95,
                    input: AdapterInput::Exit { code: Some(130) },
                },
            ]
        }

        "multi-turn" => {
            let conv = "11111111-0008-4000-8000-000000000008";
            vec![
                FixtureLine {
                    ms: 0,
                    input: AdapterInput::Command {
                        command: SessionCommand::SendUserMessage {
                            text: "Reply with TURN_ONE_OK.".into(),
                        },
                    },
                },
                FixtureLine {
                    ms: 1,
                    input: AdapterInput::Stdin {
                        line: user_stdin_line("Reply with TURN_ONE_OK."),
                    },
                },
                FixtureLine {
                    ms: 10,
                    input: AdapterInput::Stdout {
                        line: init_line(conv, DEFAULT_MODEL),
                    },
                },
                FixtureLine {
                    ms: 15,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 0,
                                "state": "DONE",
                                "step_type": "user_input"
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 40,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 1,
                                "state": "DONE",
                                "step_type": "agent_response",
                                "text_delta": "TURN_ONE_OK",
                                "duration_seconds": 0.02,
                                "usage": {
                                    "input_tokens": 110,
                                    "output_tokens": 5,
                                    "thinking_tokens": 4,
                                    "cache_read_tokens": 0,
                                    "total_tokens": 115
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 45,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "result",
                            "result": {
                                "conversation_id": conv,
                                "status": "SUCCESS",
                                "response": "TURN_ONE_OK",
                                "duration_seconds": 0.04,
                                "num_turns": 1,
                                "usage": {
                                    "input_tokens": 110,
                                    "output_tokens": 5,
                                    "thinking_tokens": 4,
                                    "cache_read_tokens": 0,
                                    "total_tokens": 115
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                // Turn 2
                FixtureLine {
                    ms: 60,
                    input: AdapterInput::Command {
                        command: SessionCommand::SendUserMessage {
                            text: "Now reply with TURN_TWO_OK.".into(),
                        },
                    },
                },
                FixtureLine {
                    ms: 61,
                    input: AdapterInput::Stdin {
                        line: user_stdin_line("Now reply with TURN_TWO_OK."),
                    },
                },
                FixtureLine {
                    ms: 70,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 2,
                                "state": "DONE",
                                "step_type": "user_input"
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 100,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 3,
                                "state": "DONE",
                                "step_type": "agent_response",
                                "text_delta": "TURN_TWO_OK",
                                "duration_seconds": 0.02,
                                "usage": {
                                    "input_tokens": 130,
                                    "output_tokens": 5,
                                    "thinking_tokens": 4,
                                    "cache_read_tokens": 100,
                                    "total_tokens": 135
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 105,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "result",
                            "result": {
                                "conversation_id": conv,
                                "status": "SUCCESS",
                                "response": "TURN_TWO_OK",
                                "duration_seconds": 0.04,
                                "num_turns": 2,
                                "usage": {
                                    "input_tokens": 130,
                                    "output_tokens": 5,
                                    "thinking_tokens": 4,
                                    "cache_read_tokens": 100,
                                    "total_tokens": 135
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 110,
                    input: AdapterInput::Exit { code: Some(0) },
                },
            ]
        }

        "resume" => {
            let conv = "11111111-0009-4000-8000-000000000009";
            vec![
                FixtureLine {
                    ms: 0,
                    input: AdapterInput::Command {
                        command: SessionCommand::SendUserMessage {
                            text: "What codeword did I ask you to remember?".into(),
                        },
                    },
                },
                FixtureLine {
                    ms: 1,
                    input: AdapterInput::Stdin {
                        line: user_stdin_line("What codeword did I ask you to remember?"),
                    },
                },
                FixtureLine {
                    ms: 12,
                    input: AdapterInput::Stdout {
                        line: init_line(conv, DEFAULT_MODEL),
                    },
                },
                FixtureLine {
                    ms: 18,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 2,
                                "state": "DONE",
                                "step_type": "user_input"
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 55,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "step_update",
                            "step_update": {
                                "conversation_id": conv,
                                "step_index": 3,
                                "state": "DONE",
                                "step_type": "agent_response",
                                "text_delta": "The codeword you asked me to remember is COBALT-42.",
                                "duration_seconds": 0.03,
                                "usage": {
                                    "input_tokens": 165,
                                    "output_tokens": 12,
                                    "thinking_tokens": 5,
                                    "cache_read_tokens": 120,
                                    "total_tokens": 177
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 60,
                    input: AdapterInput::Stdout {
                        line: json!({
                            "event": "result",
                            "result": {
                                "conversation_id": conv,
                                "status": "SUCCESS",
                                "response": "The codeword you asked me to remember is COBALT-42.",
                                "duration_seconds": 0.05,
                                "num_turns": 1,
                                "usage": {
                                    "input_tokens": 165,
                                    "output_tokens": 12,
                                    "thinking_tokens": 5,
                                    "cache_read_tokens": 120,
                                    "total_tokens": 177
                                }
                            }
                        })
                        .to_string(),
                    },
                },
                FixtureLine {
                    ms: 65,
                    input: AdapterInput::Exit { code: Some(0) },
                },
            ]
        }

        "startup-error" => vec![
            FixtureLine {
                ms: 5,
                input: AdapterInput::Stderr {
                    line: "Error: invalid or unknown model 'invalid-model-does-not-exist'".into(),
                },
            },
            FixtureLine {
                ms: 10,
                input: AdapterInput::Exit { code: Some(1) },
            },
        ],

        other => panic!("unknown scenario: {other}"),
    }
}
