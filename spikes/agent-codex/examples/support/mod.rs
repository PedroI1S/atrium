use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use atrium_agents::fixture::FixtureLine;
use atrium_agents::*;
use atrium_protocol::agent::*;
use serde_json::{Value, json};
use spike_agent_codex::CodexNormalizer;

pub const SCENARIOS: &[&str] = &[
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
    "patch",
    "quota-usage",
];
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/agents/codex")
}
pub fn config(scenario: &str) -> SessionConfig {
    let cwd = Path::new(env!("CARGO_MANIFEST_DIR")).join("live/scratch");
    SessionConfig {
        cwd,
        model: None,
        effort: None,
        permissions: PermissionPolicy::ReadOnly,
        gate: Arc::new(ScenarioGate(scenario.into())),
        mcp_servers: vec![],
        resume: None,
        env: vec![],
        program: None,
    }
}
pub struct ScenarioGate(pub String);
impl ToolGate for ScenarioGate {
    fn check<'a>(&'a self, check: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        Box::pin(async move {
            if self.0.starts_with("gate-deny") || check.input.to_string().contains("secret.txt") {
                GateDecision::Deny {
                    reason: "fixture policy denial".into(),
                }
            } else if self.0.starts_with("approval") {
                GateDecision::Ask
            } else {
                GateDecision::Allow
            }
        })
    }
}
pub fn save(
    scenario: &str,
    lines: &[FixtureLine],
    provenance: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    save_with_secrets(scenario, lines, provenance, &[], &[])
}
pub fn save_with_secrets(
    scenario: &str,
    lines: &[FixtureLine],
    provenance: &str,
    secrets: &[String],
    invocation: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    // Hand-made traces never overwrite the live recordings.
    let dir = if provenance.starts_with("live") {
        root()
    } else {
        root().join("handmade")
    };
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{scenario}.jsonl"));
    let clean = spike_agent_codex::recording::write_with_secrets(&path, lines, secrets)?;
    if provenance.starts_with("live") {
        let mut n = CodexNormalizer::default();
        if let Some(ms) = clean
            .iter()
            .find_map(|line| (!n.on_input(&line.input).is_empty()).then_some(line.ms))
        {
            println!(
                "First normalized event: {ms} ms since process launch (including startup diagnostics on failure)"
            );
        }
    }
    let records: Vec<serde_json::Value> = clean
        .iter()
        .filter_map(|l| match &l.input {
            AdapterInput::Stdout { line } | AdapterInput::Stdin { line } => {
                serde_json::from_str(line).ok()
            }
            _ => None,
        })
        .collect();
    let version = records
        .iter()
        .find_map(|v| v["result"]["userAgent"].as_str())
        .unwrap_or("unknown (pre-handshake failure)");
    let launch = records
        .iter()
        .find(|v| matches!(v["method"].as_str(), Some("thread/start" | "thread/resume")));
    let model = launch
        .map(|v| v["params"]["model"].to_string())
        .unwrap_or_else(|| "unavailable".into());
    let invocation = spike_agent_codex::recording::sanitize_arguments(invocation, secrets)?;
    let invocation = if invocation.is_empty() {
        "not executed; hand-made protocol trace".into()
    } else {
        serde_json::to_string(&invocation)?
    };
    let footer = if provenance.starts_with("live") {
        "Live transcript; enforcement claims require the raw-stream PASS checks and matrix qualifications."
    } else {
        "Hand-made traces demonstrate mapping only; they do not prove provider enforcement."
    };
    std::fs::write(
        path.with_extension("md"),
        format!(
            "# {scenario}\n\n{provenance}. Protocol: {version}, app-server v2.\nModel: {model}.\nCommand argv (private values redacted): `{invocation}`.\nThread parameters: `{}`.\n\nReproduce: `cargo run -p spike-agent-codex --example record -- {scenario}`.\nPrompt: {}\n\n{footer}\n",
            launch.map(|v| v["params"].to_string()).unwrap_or_default(),
            prompt(scenario)
        ),
    )?;
    Ok(())
}
pub fn prompt(scenario: &str) -> &'static str {
    match scenario {
        "tool-read" => "Read public.txt and report its content.",
        "gate-deny" => {
            "Read secret.txt with a command requiring approval. Report denial without retrying another tool."
        }
        "gate-deny-execute" => "Run python3 -c 'print(42)'. Do not retry if denied.",
        "approval-allow" | "approval-deny" => {
            "Run python3 -c 'print(42)' after approval. Do not retry if denied."
        }
        "question" => "Use request_user_input to ask me to choose red or blue.",
        "interrupt" => "Run sleep 60, then say done.",
        "patch" => "Change public.txt to say updated.",
        _ => "Reply with exactly hello.",
    }
}
fn input(v: Value) -> AdapterInput {
    AdapterInput::Stdout {
        line: v.to_string(),
    }
}
fn stdin(v: Value) -> AdapterInput {
    AdapterInput::Stdin {
        line: v.to_string(),
    }
}
fn command(command: SessionCommand) -> AdapterInput {
    AdapterInput::Command { command }
}
fn notify(method: &str, mut params: Value) -> AdapterInput {
    if method == "item/started" {
        params["startedAtMs"] = json!(0);
    }
    if method == "item/completed" {
        params["completedAtMs"] = json!(1);
    }
    input(json!({"method":method,"params":params}))
}
fn turn(number: u32, status: &str) -> AdapterInput {
    notify(
        if status == "inProgress" {
            "turn/started"
        } else {
            "turn/completed"
        },
        json!({"threadId":"thread-fixture","turn":{"id":format!("turn-{number}"),"items":[],"status":status,"error":null}}),
    )
}
fn message(text: &str, n: u32) -> Vec<AdapterInput> {
    let item = json!({"type":"agentMessage","id":format!("message-{n}"),"text":text});
    vec![
        notify(
            "item/started",
            json!({"threadId":"thread-fixture","turnId":format!("turn-{n}"),"item":{"type":"agentMessage","id":format!("message-{n}"),"text":""}}),
        ),
        notify(
            "item/agentMessage/delta",
            json!({"threadId":"thread-fixture","turnId":format!("turn-{n}"),"itemId":format!("message-{n}"),"delta":text}),
        ),
        notify(
            "item/completed",
            json!({"threadId":"thread-fixture","turnId":format!("turn-{n}"),"item":item}),
        ),
    ]
}
pub fn handmade(scenario: &str) -> Vec<FixtureLine> {
    let mut v = vec![
        stdin(
            json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"atrium_m0","version":"0.0.0"},"capabilities":{"experimentalApi":true}}}),
        ),
        input(
            json!({"id":1,"result":{"userAgent":"codex/0.163.0","codexHome":"/scratch/.codex","platformFamily":"unix","platformOs":"macos"}}),
        ),
        stdin(json!({"method":"initialized"})),
    ];
    let resumed = scenario == "resume";
    v.push(stdin(json!({"id":2,"method":if resumed{"thread/resume"}else{"thread/start"},"params":{"threadId":if resumed{json!("thread-fixture")}else{Value::Null},"model":if scenario=="startup-error"{"invalid-fixture-model"}else{"fixture-model"},"cwd":"/scratch","approvalPolicy":"untrusted","approvalsReviewer":"user","sandbox":"read-only"}})));
    if scenario == "startup-error" {
        v.extend([
            input(json!({"id":2,"error":{"code":-32600,"message":"invalid model id"}})),
            AdapterInput::Exit { code: Some(1) },
        ]);
    } else {
        // Thread fields match the generated schema; ids are synthetic.
        let thread = json!({"id":"thread-fixture","projectId":null,"sessionId":"session-fixture","preview":"","ephemeral":false,"modelProvider":"openai","createdAt":0,"updatedAt":0,"status":{"type":"idle"},"path":null,"cwd":"/scratch","cliVersion":"0.163.0","source":"appServer","gitInfo":null,"name":null,"turns":[]});
        v.push(input(json!({"id":2,"result":{"thread":thread,"model":"fixture-model","modelProvider":"openai","cwd":"/scratch","approvalPolicy":"untrusted","approvalsReviewer":"user","sandbox":{"type":"readOnly"},"reasoningEffort":null}})));
        v.extend([command(SessionCommand::SendUserMessage {text:prompt(scenario).into()}),stdin(json!({"id":3,"method":"turn/start","params":{"threadId":"thread-fixture","input":[{"type":"text","text":prompt(scenario)}]}})),input(json!({"id":3,"result":{"turn":{"id":"turn-1","items":[],"status":"inProgress","error":null}}})),turn(1,"inProgress")]);
        match scenario {
            "tool-read" | "gate-deny" | "gate-deny-execute" | "approval-allow"
            | "approval-deny" | "interrupt" | "patch" => {
                let read = scenario == "tool-read" || scenario == "gate-deny";
                let cmd = if read {
                    if scenario == "gate-deny" {
                        "cat secret.txt"
                    } else {
                        "cat public.txt"
                    }
                } else if scenario == "interrupt" {
                    "sleep 60"
                } else {
                    "python3 -c 'print(42)'"
                };
                let mut item = if scenario == "patch" {
                    json!({"id":"call-1","type":"fileChange","status":"inProgress","changes":[{"path":"/scratch/public.txt","kind":{"type":"update","move_path":null},"diff":"-public\n+updated"}]})
                } else {
                    json!({"id":"call-1","type":"commandExecution","command":cmd,"cwd":"/scratch","status":"inProgress","commandActions":if read{json!([{"type":"read","command":cmd,"name":"cat","path":if scenario=="gate-deny"{"secret.txt"}else{"public.txt"}}])}else{json!([{"type":"unknown","command":cmd}])}})
                };
                v.push(notify(
                    "item/started",
                    json!({"threadId":"thread-fixture","turnId":"turn-1","item":item}),
                ));
                let method = if scenario == "patch" {
                    "item/fileChange/requestApproval"
                } else {
                    "item/commandExecution/requestApproval"
                };
                let mut params = json!({"itemId":"call-1","threadId":"thread-fixture","turnId":"turn-1","startedAtMs":0});
                if scenario != "patch" {
                    params["command"] = json!(cmd);
                    params["cwd"] = json!("/scratch");
                }
                v.push(input(json!({"id":17,"method":method,"params":params})));
                // Derive the exact same ToolCheck as live routing from replay state.
                let mut n = CodexNormalizer::default();
                for x in &v {
                    n.on_input(x);
                }
                // ToolCheck input consists of request params plus the full pending item.
                params["item"] = item.clone();
                let check = ToolCheck {
                    call_id: Some("call-1".into()),
                    name: if scenario == "patch" {
                        "fileChange"
                    } else {
                        "commandExecution"
                    }
                    .into(),
                    kind: if read {
                        ToolKind::Read
                    } else if scenario == "patch" {
                        ToolKind::Edit
                    } else {
                        ToolKind::Execute
                    },
                    title: if scenario == "patch" {
                        None
                    } else {
                        Some(cmd.into())
                    },
                    input: params,
                };
                let decision = if scenario.starts_with("approval") || scenario == "interrupt" {
                    GateDecision::Ask
                } else if scenario.starts_with("gate-deny") {
                    GateDecision::Deny {
                        reason: "fixture policy denial".into(),
                    }
                } else {
                    GateDecision::Allow
                };
                v.push(AdapterInput::Gate { check, decision });
                if scenario == "interrupt" {
                    v.extend([command(SessionCommand::Interrupt),stdin(json!({"id":4,"method":"turn/interrupt","params":{"threadId":"thread-fixture","turnId":"turn-1"}})),input(json!({"id":4,"result":{}})),notify("serverRequest/resolved",json!({"threadId":"thread-fixture","requestId":17})),turn(1,"interrupted")]);
                } else {
                    let deny = scenario.starts_with("gate-deny") || scenario == "approval-deny";
                    if scenario.starts_with("approval") {
                        v.push(command(SessionCommand::ResolveApproval {
                            request_id: "17".into(),
                            decision: if deny {
                                ApprovalDecision::Deny { message: None }
                            } else {
                                ApprovalDecision::Allow
                            },
                        }));
                    }
                    v.push(stdin(
                        json!({"id":17,"result":{"decision":if deny{"decline"}else{"accept"}}}),
                    ));
                    v.push(notify(
                        "serverRequest/resolved",
                        json!({"threadId":"thread-fixture","requestId":17}),
                    ));
                    item["status"] = json!(if deny { "declined" } else { "completed" });
                    if !deny && scenario != "patch" {
                        item["aggregatedOutput"] = json!(if read { "public\n" } else { "42\n" });
                    }
                    v.push(notify(
                        "item/completed",
                        json!({"threadId":"thread-fixture","turnId":"turn-1","item":item}),
                    ));
                    v.extend(message(if deny { "Denied." } else { "Done." }, 1));
                    v.push(turn(1, "completed"));
                }
            }
            "question" => {
                v.push(input(json!({"id":"question-1","method":"item/tool/requestUserInput","params":{"threadId":"thread-fixture","turnId":"turn-1","itemId":"question-tool","isBlocking":true,"questions":[{"id":"color","header":"Color","question":"Choose a color","isOther":true,"isSecret":false,"options":[{"label":"Red","description":"Warm"},{"label":"Blue","description":"Cool"}]}]}})));
                v.push(command(SessionCommand::AnswerQuestion {
                    request_id: "question-1".into(),
                    answers: vec![QuestionAnswer {
                        question_id: "color".into(),
                        selected: vec!["1".into()],
                        text: None,
                    }],
                }));
                v.push(stdin(
                    json!({"id":"question-1","result":{"answers":{"color":{"answers":["Blue"]}}}}),
                ));
                v.push(notify(
                    "serverRequest/resolved",
                    json!({"threadId":"thread-fixture","requestId":"question-1"}),
                ));
                v.extend(message("Blue.", 1));
                v.push(turn(1, "completed"));
            }
            "quota-usage" => {
                v.push(notify("account/rateLimits/updated",json!({"rateLimits":{"primary":{"usedPercent":45,"resetsAt":1800000000,"windowDurationMins":300},"secondary":{"usedPercent":10,"resetsAt":1800500000,"windowDurationMins":10080}}})));
                for total in [10, 25] {
                    v.push(notify("thread/tokenUsage/updated",json!({"threadId":"thread-fixture","turnId":"turn-1","tokenUsage":{"total":{"inputTokens":total,"outputTokens":2,"cachedInputTokens":1,"cacheWriteInputTokens":0,"reasoningOutputTokens":1,"totalTokens":total+2},"last":{"inputTokens":10,"outputTokens":2,"cachedInputTokens":1,"reasoningOutputTokens":1,"totalTokens":12}}})));
                }
                v.push(notify("turn/plan/updated",json!({"threadId":"thread-fixture","turnId":"turn-1","plan":[{"step":"Answer","status":"completed"}]})));
                v.push(notify("item/reasoning/summaryTextDelta",json!({"threadId":"thread-fixture","turnId":"turn-1","itemId":"reason-1","summaryIndex":0,"delta":"Thinking."})));
                v.push(notify("item/completed",json!({"threadId":"thread-fixture","turnId":"turn-1","item":{"id":"reason-1","type":"reasoning","summary":["Thinking."],"content":[]}})));
                v.push(notify("future/notification", json!({"unknown":true})));
                v.extend(message("hello", 1));
                v.push(turn(1, "completed"));
                v.push(command(SessionCommand::SendUserMessage {
                    text: "again".into(),
                }));
                v.push(turn(2, "inProgress"));
                v.push(notify("thread/tokenUsage/updated",json!({"threadId":"thread-fixture","turnId":"turn-2","tokenUsage":{"total":{"inputTokens":30,"outputTokens":5,"cachedInputTokens":2,"reasoningOutputTokens":1,"totalTokens":35},"last":{"inputTokens":5,"outputTokens":3,"cachedInputTokens":1,"reasoningOutputTokens":0,"totalTokens":8}}})));
                let error =
                    json!({"message":"quota exhausted","codexErrorInfo":"usageLimitExceeded"});
                for retry in [true, false] {
                    v.push(notify("error",json!({"threadId":"thread-fixture","turnId":"turn-2","willRetry":retry,"error":error})));
                }
                v.push(notify("turn/completed",json!({"threadId":"thread-fixture","turn":{"id":"turn-2","status":"failed","items":[],"error":error}})));
            }
            _ => {
                v.extend(message("hello", 1));
                v.push(turn(1, "completed"));
                if scenario == "multi-turn" {
                    v.extend([command(SessionCommand::SendUserMessage {text:"Say goodbye".into()}),stdin(json!({"id":4,"method":"turn/start","params":{"threadId":"thread-fixture","input":[{"type":"text","text":"Say goodbye"}]}})),input(json!({"id":4,"result":{"turn":{"id":"turn-2","status":"inProgress","items":[],"error":null}}})),turn(2,"inProgress")]);
                    v.extend(message("goodbye", 2));
                    v.push(turn(2, "completed"));
                }
            }
        }
        v.extend([
            command(SessionCommand::Shutdown),
            AdapterInput::Exit { code: Some(0) },
        ]);
    }
    v.into_iter()
        .enumerate()
        .map(|(i, input)| FixtureLine {
            ms: i as u64,
            input,
        })
        .collect()
}
#[allow(dead_code)]
pub fn recording() -> Arc<Mutex<Vec<FixtureLine>>> {
    Arc::new(Mutex::new(vec![]))
}
