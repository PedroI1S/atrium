#![doc = "Live scenario recorder for Codex app-server M1 fixtures and X1-X3 verification."]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use atrium_agents::codex::recording;
use atrium_agents::codex::{CodexAdapter, CodexNormalizer, CodexOptions};
use atrium_agents::fixture::{self, FixtureLine, Sanitizer};
use atrium_agents::{
    AdapterInput, AgentAdapter, ApprovalTicket, BoxFuture, GateDecision, PermissionPolicy, Session,
    SessionConfig, ToolCheck, ToolGate, Transport,
};
use atrium_protocol::agent::{
    AgentEvent, ApprovalDecision, QuestionAnswer, SessionCommand, ToolKind,
};
use serde_json::json;

const CANARY_SECRET: &str = "ATRIUM-7Q2X-CANARY";

#[derive(Clone, Copy)]
enum Mode {
    Allow,
    DenyAll,
    ApproveOnce,
    DenyApproval,
    AskThenApprove,
    QuestionAnswer,
    InterruptAfterApproval,
    StartupError,
    MultiTurn,
    ResumeTwoStep,
    FullAccessCheck,
}

#[derive(Clone, Copy)]
struct Scenario {
    name: &'static str,
    prompt: &'static str,
    mode: Mode,
    description: &'static str,
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "simple",
        prompt: "Reply with exactly ok and do not run any tools.",
        mode: Mode::Allow,
        description: "Basic single-turn text response",
    },
    Scenario {
        name: "tool-read",
        prompt: "Read public.txt using a shell command and tell me its exact content.",
        mode: Mode::Allow,
        description: "CommandExecution classified as ToolKind::Execute even with read action",
    },
    Scenario {
        name: "gate-deny",
        prompt: "Read secret.txt using a shell command and tell me its exact content.",
        mode: Mode::DenyAll,
        description: "Gate denies commandExecution before execution",
    },
    Scenario {
        name: "gate-deny-execute",
        prompt: "Run python3 -c \"print(42)\" in the workspace.",
        mode: Mode::DenyAll,
        description: "Gate denies execute call before execution",
    },
    Scenario {
        name: "approval-allow",
        prompt: "Run python3 -c \"print(42)\" and report the number.",
        mode: Mode::ApproveOnce,
        description: "Gate returns Ask, client resolves Allow, redeem passes",
    },
    Scenario {
        name: "approval-deny",
        prompt: "Run python3 -c \"print(42)\" and report the number.",
        mode: Mode::DenyApproval,
        description: "Gate returns Ask, client resolves Deny",
    },
    Scenario {
        name: "patch",
        prompt: "Update public.txt to append a second line reading updated.",
        mode: Mode::ApproveOnce,
        description: "FileChange approval and patch application",
    },
    Scenario {
        name: "interrupt",
        prompt: "Run sleep 30 using a shell command, then report done.",
        mode: Mode::InterruptAfterApproval,
        description: "Turn interrupted cleanly with cancellation order",
    },
    Scenario {
        name: "multi-turn",
        prompt: "Remember the word lantern and reply with noted.",
        mode: Mode::MultiTurn,
        description: "Two sequential turns keeping thread context",
    },
    Scenario {
        name: "quota-usage",
        prompt: "Say hello in one short word.",
        mode: Mode::Allow,
        description: "Usage, context window, and rate-limit recording",
    },
    Scenario {
        name: "question",
        prompt: "Use request_user_input to ask me whether to choose red or blue, then confirm my answer.",
        mode: Mode::QuestionAnswer,
        description: "Server requestUserInput question flow",
    },
    Scenario {
        name: "resume",
        prompt: "Remember the code word copper and reply with stored.",
        mode: Mode::ResumeTwoStep,
        description: "Origin session followed by thread/resume session",
    },
    Scenario {
        name: "startup-error",
        prompt: "Say hello.",
        mode: Mode::StartupError,
        description: "Invalid binary/config producing pre-session fatal Error + SessionEnded",
    },
    Scenario {
        name: "x1-read",
        prompt: "Inspect secret.txt without using shell commands if a built-in read tool exists, otherwise read secret.txt and tell me its contents.",
        mode: Mode::DenyAll,
        description: "X1 live probe: Read class interception",
    },
    Scenario {
        name: "x1-edit",
        prompt: "Edit secret.txt directly to replace its content with overwritten.",
        mode: Mode::DenyAll,
        description: "X1 live probe: Edit class interception",
    },
    Scenario {
        name: "x1-delete",
        prompt: "Delete secret.txt.",
        mode: Mode::DenyAll,
        description: "X1 live probe: Delete class interception",
    },
    Scenario {
        name: "x1-move",
        prompt: "Move or rename secret.txt to moved.txt.",
        mode: Mode::DenyAll,
        description: "X1 live probe: Move class interception",
    },
    Scenario {
        name: "x1-search",
        prompt: "Search the workspace for the string CANARY and report any matching files.",
        mode: Mode::DenyAll,
        description: "X1 live probe: Search class interception",
    },
    Scenario {
        name: "x1-fetch",
        prompt: "Use web_search to look up https://example.com and report the title.",
        mode: Mode::DenyAll,
        description: "X1 live probe: Fetch class interception",
    },
    Scenario {
        name: "x1-mcp",
        prompt: "Call any available MCP tool on server probe if configured, otherwise report what MCP tools exist.",
        mode: Mode::DenyAll,
        description: "X1 live probe: Mcp class interception",
    },
    Scenario {
        name: "x1-subagent",
        prompt: "Spawn a subagent to inspect public.txt and return its content.",
        mode: Mode::DenyAll,
        description: "X1 live probe: Subagent class interception",
    },
    Scenario {
        name: "x2-fullaccess",
        prompt: "Read secret.txt and also write the word touched to public.txt.",
        mode: Mode::FullAccessCheck,
        description: "X2 live probe: PermissionPolicy::FullAccess with Atrium gate check",
    },
    Scenario {
        name: "x3-exec-amendment",
        prompt: "Run python3 -c \"print(7)\" and then run python3 -c \"print(8)\".",
        mode: Mode::AskThenApprove,
        description: "X3 live probe: verify approvalPolicy=untrusted prompts each call without execpolicy bypass",
    },
];

struct ScriptGate {
    mode: Mode,
    counter: Mutex<u32>,
    checks: Mutex<Vec<ToolCheck>>,
}

impl ScriptGate {
    fn new(mode: Mode) -> Arc<Self> {
        Arc::new(Self {
            mode,
            counter: Mutex::new(0),
            checks: Mutex::default(),
        })
    }

    fn next_id(&self, prefix: &str) -> String {
        let mut g = self.counter.lock().unwrap();
        *g += 1;
        format!("{prefix}-{}", *g)
    }
}

impl ToolGate for ScriptGate {
    fn check<'a>(&'a self, check: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        self.checks.lock().unwrap().push(check.clone());
        let d = match self.mode {
            Mode::Allow | Mode::MultiTurn | Mode::ResumeTwoStep | Mode::StartupError => {
                GateDecision::Allow
            }
            Mode::DenyAll | Mode::FullAccessCheck => GateDecision::Deny {
                reason: "blocked by live probe policy".into(),
            },
            Mode::ApproveOnce
            | Mode::DenyApproval
            | Mode::AskThenApprove
            | Mode::InterruptAfterApproval => GateDecision::Ask {
                ticket: ApprovalTicket {
                    request_id: self.next_id("req"),
                    rememberable: check.kind == ToolKind::Execute,
                },
            },
            Mode::QuestionAnswer => GateDecision::Allow,
        };
        Box::pin(async move { d })
    }

    fn redeem<'a>(
        &'a self,
        _ticket: &'a ApprovalTicket,
        _check: &'a ToolCheck,
    ) -> BoxFuture<'a, GateDecision> {
        let d = match self.mode {
            Mode::DenyAll | Mode::DenyApproval | Mode::FullAccessCheck => GateDecision::Deny {
                reason: "redeem denied".into(),
            },
            _ => GateDecision::Allow,
        };
        Box::pin(async move { d })
    }

    fn question_ticket(&self, _turn: Option<u32>) -> BoxFuture<'_, Option<ApprovalTicket>> {
        let id = self.next_id("question");
        Box::pin(async move {
            Some(ApprovalTicket {
                request_id: id,
                rememberable: false,
            })
        })
    }
}

fn default_out_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("fixtures")
        .join("agents")
        .join("codex")
}

fn resanitize_all(out_dir: &Path, filter: Option<&str>) -> Result<(), String> {
    let entries = fs::read_dir(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let extra = vec![CANARY_SECRET.to_owned()];
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if let Some(target) = filter
            && target != "all"
            && stem != target
        {
            continue;
        }
        let lines = fixture::read(&path).map_err(|e| e.to_string())?;
        let clean =
            recording::write_with_secrets(&path, &lines, &extra).map_err(|e| e.to_string())?;
        let events = fixture::normalize(&clean, &mut CodexNormalizer::new());
        let events_path = path.with_extension("events.json");
        fs::write(
            &events_path,
            serde_json::to_string_pretty(&events).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        println!("resanitized {stem}");
    }
    Ok(())
}

async fn drive_session(
    mut session: Session,
    scenario: &Scenario,
    second_turn: Option<&str>,
) -> Vec<AgentEvent> {
    let mut collected = Vec::new();
    let _ = session
        .commands
        .send(SessionCommand::SendUserMessage {
            text: scenario.prompt.to_owned(),
        })
        .await;

    let mut second_sent = false;
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok(Some(env)) = tokio::time::timeout(left, session.events.recv()).await else {
            break;
        };
        let ev = env.event;
        collected.push(ev.clone());
        match &ev {
            AgentEvent::ApprovalRequested { request_id, .. } => match scenario.mode {
                Mode::ApproveOnce | Mode::AskThenApprove => {
                    let _ = session
                        .commands
                        .send(SessionCommand::ResolveApproval {
                            request_id: request_id.clone(),
                            decision: ApprovalDecision::Allow,
                        })
                        .await;
                }
                Mode::DenyApproval | Mode::DenyAll | Mode::FullAccessCheck => {
                    let _ = session
                        .commands
                        .send(SessionCommand::ResolveApproval {
                            request_id: request_id.clone(),
                            decision: ApprovalDecision::Deny {
                                message: Some("denied by recorder".into()),
                            },
                        })
                        .await;
                }
                Mode::InterruptAfterApproval => {
                    let _ = session.commands.send(SessionCommand::Interrupt).await;
                }
                _ => {}
            },
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
                        text: if q.options.is_empty() {
                            Some("blue".into())
                        } else {
                            None
                        },
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
                if !second_sent && let Some(msg) = second_turn {
                    second_sent = true;
                    let _ = session
                        .commands
                        .send(SessionCommand::SendUserMessage {
                            text: msg.to_owned(),
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
    collected
}

async fn run_scenario(scenario: &Scenario, out_dir: &Path, keep: bool) -> Result<(), String> {
    let scratch = env::temp_dir().join(format!(
        "atrium-codex-record-{}-{}",
        scenario.name,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    fs::write(scratch.join("public.txt"), "public-fixture-content\n").map_err(|e| e.to_string())?;
    let secret_path = scratch.join("secret.txt");
    fs::write(&secret_path, format!("{CANARY_SECRET}\n")).map_err(|e| e.to_string())?;

    let home = env::var("HOME").unwrap_or_default();
    let user = env::var("USER").unwrap_or_default();
    let scratch_str = scratch.display().to_string();
    let sanitizer = Sanitizer::new(&home, &user, &[CANARY_SECRET, &scratch_str]);
    let recorded: Arc<Mutex<Vec<FixtureLine>>> = Arc::default();
    let rec_clone = recorded.clone();
    let t0 = Instant::now();

    let options = CodexOptions {
        tap: Some(Arc::new(move |input: &AdapterInput| {
            rec_clone.lock().unwrap().push(FixtureLine {
                ms: t0.elapsed().as_millis() as u64,
                input: input.clone(),
            });
        })),
        ..CodexOptions::default()
    };

    let gate = ScriptGate::new(scenario.mode);
    let program = if matches!(scenario.mode, Mode::StartupError) {
        Some(PathBuf::from("nonexistent-codex-binary-for-atrium-record"))
    } else {
        None
    };
    let permissions = if matches!(scenario.mode, Mode::FullAccessCheck) {
        PermissionPolicy::FullAccess
    } else {
        PermissionPolicy::Ask
    };
    let cfg = SessionConfig {
        cwd: scratch.clone(),
        model: env::var("ATRIUM_CODEX_MODEL").ok(),
        effort: None,
        permissions,
        resume: None,
        mcp_servers: Vec::new(),
        gate: gate.clone(),
        env: Vec::new(),
        program,
        on_spawn: None,
    };

    let events = match CodexAdapter::with_options(options)
        .start(cfg, Transport::Local)
        .await
    {
        Ok(session) => {
            let second = if matches!(scenario.mode, Mode::MultiTurn) {
                Some("What word did I ask you to remember?")
            } else {
                None
            };
            drive_session(session, scenario, second).await
        }
        Err(err) => {
            println!("{} failed startup as expected: {err}", scenario.name);
            let lines = recorded.lock().unwrap().clone();
            fixture::normalize(&lines, &mut CodexNormalizer::new())
        }
    };

    let mut raw_lines = recorded.lock().unwrap().clone();
    for line in &mut raw_lines {
        sanitizer.input(&mut line.input);
    }
    let jsonl_path = out_dir.join(format!("{}.jsonl", scenario.name));
    recording::write_with_secrets(
        &jsonl_path,
        &raw_lines,
        &[CANARY_SECRET.to_owned(), scratch_str],
    )
    .map_err(|e| format!("write fixture {}: {e}", jsonl_path.display()))?;

    let final_lines = fixture::read(&jsonl_path).map_err(|e| e.to_string())?;
    let norm_events = fixture::normalize(&final_lines, &mut CodexNormalizer::new());
    let events_path = out_dir.join(format!("{}.events.json", scenario.name));
    fs::write(
        &events_path,
        serde_json::to_string_pretty(&norm_events).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    if matches!(scenario.mode, Mode::DenyAll) {
        let serialized_events = serde_json::to_string(&events).map_err(|e| e.to_string())?;
        if serialized_events.contains(CANARY_SECRET) {
            return Err(format!("canary secret leaked in {} events", scenario.name));
        }
        recording::verify_enforcement(scenario.name, &final_lines)?;
    }

    let md_path = out_dir.join(format!("{}.md", scenario.name));
    let checks_count = gate.checks.lock().unwrap().len();
    let note = json!({
        "scenario": scenario.name,
        "description": scenario.description,
        "checks": checks_count,
        "events": norm_events.len(),
    });
    fs::write(
        &md_path,
        format!(
            "# {}\n\n{}\n\nPrompt: `{}`\n\nSummary: `{}`\n",
            scenario.name, scenario.description, scenario.prompt, note
        ),
    )
    .map_err(|e| e.to_string())?;

    println!(
        "recorded {} -> {} lines, {} events ({} gate checks)",
        scenario.name,
        final_lines.len(),
        norm_events.len(),
        checks_count
    );

    if !keep {
        let _ = fs::remove_dir_all(&scratch);
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        eprintln!(
            "Usage: codex-record [--list] [--out DIR] [--keep] [--resanitize [SCENARIO]] <SCENARIO|all>"
        );
        std::process::exit(0);
    }
    if args.iter().any(|a| a == "--list") {
        for s in SCENARIOS {
            println!("{:<20} {}", s.name, s.description);
        }
        return;
    }

    let mut out_dir = default_out_dir();
    let mut keep = false;
    let mut resanitize = false;
    let mut target: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--out" => {
                i += 1;
                if let Some(d) = args.get(i) {
                    out_dir = PathBuf::from(d);
                }
            }
            "--keep" => keep = true,
            "--resanitize" => resanitize = true,
            other => target = Some(other.to_owned()),
        }
        i += 1;
    }

    if resanitize {
        if let Err(e) = resanitize_all(&out_dir, target.as_deref()) {
            eprintln!("resanitize failed: {e}");
            std::process::exit(1);
        }
        return;
    }

    let Some(name) = target else {
        eprintln!("missing scenario name (use --list)");
        std::process::exit(2);
    };

    fs::create_dir_all(&out_dir).expect("create output dir");
    let chosen: Vec<&Scenario> = if name == "all" {
        SCENARIOS.iter().collect()
    } else {
        match SCENARIOS.iter().find(|s| s.name == name) {
            Some(s) => vec![s],
            None => {
                eprintln!("unknown scenario '{name}' (use --list)");
                std::process::exit(2);
            }
        }
    };

    for s in chosen {
        if let Err(e) = run_scenario(s, &out_dir, keep).await {
            eprintln!("FAILED {}: {e}", s.name);
            std::process::exit(1);
        }
    }
}
