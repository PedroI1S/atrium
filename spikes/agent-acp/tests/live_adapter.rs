use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use atrium_agents::{
    AgentAdapter, BoxFuture, EVENT_BUFFER, GateDecision, PermissionPolicy, SHUTDOWN_GRACE,
    SessionConfig, ToolCheck, ToolGate, Transport,
};
use atrium_protocol::agent::{
    AgentEvent, ApprovalDecision, QuestionAnswer, RequestOutcome, SessionCommand, ToolKind,
    ToolStatus, TurnOutcome,
};
use serde_json::{Value, json};
use spike_agent_acp::{AcpAdapter, ClientCapabilityConfig};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn main() {
    if let Ok(scenario) = std::env::var("ATRIUM_MOCK_ACP_SCENARIO") {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build mock runtime");
        rt.block_on(run_mock_acp_agent(&scenario));
        return;
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build test runtime");

    println!("running 4 live_adapter tests");
    rt.block_on(async {
        print!("test saturation_test ... ");
        saturation_test().await;
        println!("ok");

        print!("test live_fs_read_and_gate_deny_test ... ");
        live_fs_read_and_gate_deny_test().await;
        println!("ok");

        print!("test live_approval_and_question_test ... ");
        live_approval_and_question_test().await;
        println!("ok");

        print!("test live_startup_error_test ... ");
        live_startup_error_test().await;
        println!("ok");
    });
    println!("\ntest result: ok. 4 passed; 0 failed; 0 ignored\n");
}

struct SelectiveGate {
    secret_marker: String,
    ask_edit: bool,
}

impl ToolGate for SelectiveGate {
    fn check<'a>(&'a self, call: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        let raw_str = call.input.to_string();
        let title_str = call.title.clone().unwrap_or_default();
        let is_secret =
            raw_str.contains(&self.secret_marker) || title_str.contains(&self.secret_marker);
        let kind = call.kind;
        let ask_edit = self.ask_edit;

        Box::pin(async move {
            if is_secret {
                GateDecision::Deny {
                    reason: format!("blocked access to secret creates/reads ({kind:?})"),
                }
            } else if ask_edit && kind == ToolKind::Edit {
                GateDecision::Ask
            } else {
                GateDecision::Allow
            }
        })
    }
}

fn adapter_for_scenario(
    scenario: &str,
    cwd: PathBuf,
    gate: Arc<dyn ToolGate>,
) -> (AcpAdapter, SessionConfig) {
    let exe = std::env::current_exe().expect("current_exe");
    let adapter = AcpAdapter::with_command(exe, Vec::<String>::new())
        .with_client_capabilities(ClientCapabilityConfig::all());
    let cfg = SessionConfig {
        cwd,
        model: None,
        effort: None,
        permissions: PermissionPolicy::Ask,
        gate,
        mcp_servers: Vec::new(),
        resume: None,
        env: vec![("ATRIUM_MOCK_ACP_SCENARIO".to_owned(), scenario.to_owned())],
        program: None,
    };
    (adapter, cfg)
}

async fn saturation_test() {
    for scenario in ["saturate", "saturate_stubborn"] {
        let cwd = std::env::current_dir().expect("cwd");
        let gate = Arc::new(SelectiveGate {
            secret_marker: "secret.txt".to_owned(),
            ask_edit: false,
        });
        let (adapter, cfg) = adapter_for_scenario(scenario, cwd, gate);

        let session = adapter
            .start(cfg, Transport::Local)
            .await
            .expect("start session");
        session
            .commands
            .send(SessionCommand::SendUserMessage {
                text: "Flood the event stream".to_owned(),
            })
            .await
            .expect("send user message");

        // Let the mock agent flood more than EVENT_BUFFER (1024) notifications
        // while we NEVER read from `session.events`.
        tokio::time::sleep(Duration::from_millis(350)).await;

        let start = Instant::now();
        session
            .commands
            .send(SessionCommand::Interrupt)
            .await
            .expect("Interrupt must not block when session.events is full and unread");

        session
            .commands
            .send(SessionCommand::Shutdown)
            .await
            .expect("Shutdown must complete cleanly when session.events is full and unread");

        let elapsed = start.elapsed();
        assert!(
            elapsed < SHUTDOWN_GRACE + Duration::from_secs(1),
            "Shutdown took {elapsed:?} for {scenario}, exceeding SHUTDOWN_GRACE ({SHUTDOWN_GRACE:?})"
        );
        drop(session);
    }
}

async fn live_fs_read_and_gate_deny_test() {
    let temp_dir = std::env::temp_dir().join(format!("atrium-acp-test-{}", std::process::id()));
    tokio::fs::create_dir_all(&temp_dir)
        .await
        .expect("create temp dir");
    let allowed_path = temp_dir.join("allowed.txt");
    let secret_path = temp_dir.join("secret.txt");
    let canary = "TOP_SECRET_CANARY_VALUE_9941";

    tokio::fs::write(&allowed_path, "Public documentation content\n")
        .await
        .expect("write allowed.txt");
    tokio::fs::write(&secret_path, canary)
        .await
        .expect("write secret.txt");

    let gate = Arc::new(SelectiveGate {
        secret_marker: "secret.txt".to_owned(),
        ask_edit: false,
    });
    let (adapter, cfg) = adapter_for_scenario("gate_fs_and_exec", temp_dir.clone(), gate);
    let (rec_tx, mut rec_rx) = tokio::sync::mpsc::unbounded_channel();
    let adapter = adapter.with_recorder(rec_tx);

    let mut session = adapter
        .start(cfg, Transport::Local)
        .await
        .expect("start session");
    session
        .commands
        .send(SessionCommand::SendUserMessage {
            text: "Read allowed.txt, then try secret.txt and cat secret.txt".to_owned(),
        })
        .await
        .expect("send user message");

    let mut events = Vec::new();
    while let Some(env) = session.events.recv().await {
        let is_turn_done = matches!(env.event, AgentEvent::TurnCompleted { .. });
        events.push(env.event);
        if is_turn_done {
            break;
        }
    }

    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .expect("shutdown");
    while let Some(env) = session.events.recv().await {
        events.push(env.event);
    }

    let events_json = serde_json::to_string(&events).expect("serialize events");
    assert!(
        !events_json.contains(canary),
        "Canary value leaked into normalized events!"
    );
    assert!(
        events_json.contains("Public documentation content"),
        "Allowed file content should have been read and returned"
    );

    let mut recorded_lines = Vec::new();
    while let Ok(line) = rec_rx.try_recv() {
        recorded_lines.push(line);
    }
    let rec_json = serde_json::to_string(&recorded_lines).expect("serialize recorded lines");
    assert!(
        !rec_json.contains(canary),
        "Canary value leaked into recorded wire transcript!"
    );

    let denied_count = events
        .iter()
        .filter(|ev| {
            matches!(
                ev,
                AgentEvent::ToolCallFinished {
                    status: ToolStatus::Denied,
                    ..
                }
            )
        })
        .count();
    assert_eq!(
        denied_count, 2,
        "Expected both secret fs/read_text_file and secret terminal/create to be Denied"
    );

    let _ = tokio::fs::remove_dir_all(&temp_dir).await;
}

async fn live_approval_and_question_test() {
    let cwd = std::env::current_dir().expect("cwd");
    let gate = Arc::new(SelectiveGate {
        secret_marker: "secret.txt".to_owned(),
        ask_edit: true,
    });
    let (adapter, cfg) = adapter_for_scenario("approval_and_question", cwd, gate);

    let mut session = adapter
        .start(cfg, Transport::Local)
        .await
        .expect("start session");
    session
        .commands
        .send(SessionCommand::SendUserMessage {
            text: "Write output.txt and ask me a question".to_owned(),
        })
        .await
        .expect("send user message");

    let mut saw_approval_resolved = false;
    let mut saw_question_resolved = false;

    while let Some(env) = session.events.recv().await {
        match &env.event {
            AgentEvent::ApprovalRequested { request_id, .. } => {
                session
                    .commands
                    .send(SessionCommand::ResolveApproval {
                        request_id: request_id.clone(),
                        decision: ApprovalDecision::Allow,
                    })
                    .await
                    .expect("send ResolveApproval");
            }
            AgentEvent::QuestionAsked { request_id, .. } => {
                session
                    .commands
                    .send(SessionCommand::AnswerQuestion {
                        request_id: request_id.clone(),
                        answers: vec![QuestionAnswer {
                            question_id: "env".to_owned(),
                            selected: vec!["staging".to_owned()],
                            text: None,
                        }],
                    })
                    .await
                    .expect("send AnswerQuestion");
            }
            AgentEvent::RequestResolved {
                request_id,
                outcome,
            } => {
                if request_id == "req-perm-live" && *outcome == RequestOutcome::Approved {
                    saw_approval_resolved = true;
                }
                if request_id == "req-elicit-live" && *outcome == RequestOutcome::Answered {
                    saw_question_resolved = true;
                }
            }
            AgentEvent::TurnCompleted {
                outcome: TurnOutcome::Completed,
                ..
            } => break,
            _ => {}
        }
    }

    assert!(
        saw_approval_resolved,
        "Expected ApprovalRequested -> Approved"
    );
    assert!(saw_question_resolved, "Expected QuestionAsked -> Answered");

    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .expect("shutdown");
}

async fn live_startup_error_test() {
    let cwd = std::env::current_dir().expect("cwd");
    let gate = Arc::new(SelectiveGate {
        secret_marker: "secret.txt".to_owned(),
        ask_edit: false,
    });
    let (adapter, cfg) = adapter_for_scenario("startup_error", cwd, gate);

    let mut session = adapter
        .start(cfg, Transport::Local)
        .await
        .expect("start returns Session carrying fatal startup events");

    let mut events = Vec::new();
    while let Some(env) = session.events.recv().await {
        events.push(env.event);
    }

    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::SessionStarted { .. })),
        "Pre-session failure must not emit SessionStarted"
    );
    assert!(
        matches!(events.first(), Some(AgentEvent::Error { fatal: true, .. })),
        "Pre-session failure must start with Error {{ fatal: true }}, got: {events:?}"
    );
    assert!(
        matches!(events.last(), Some(AgentEvent::SessionEnded { .. })),
        "Pre-session failure must end with SessionEnded, got: {events:?}"
    );
}

async fn write_rpc(stdout: &mut tokio::io::Stdout, msg: &Value) {
    let line = format!("{}\n", serde_json::to_string(msg).expect("json"));
    let _ = stdout.write_all(line.as_bytes()).await;
    let _ = stdout.flush().await;
}

async fn run_mock_acp_agent(scenario: &str) {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(stdin).lines();

    if scenario == "startup_error" {
        if let Ok(Some(line)) = lines.next_line().await {
            let msg: Value = serde_json::from_str(&line).unwrap_or_default();
            let id = msg.get("id").cloned().unwrap_or(json!(1));
            write_rpc(
                &mut stdout,
                &json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32603,
                        "message": "Mock provider fatal startup failure"
                    }
                }),
            )
            .await;
        }
        return;
    }

    let mut session_id = "ses_mock_1".to_owned();
    let mut cwd = String::new();

    while let Ok(Some(line)) = lines.next_line().await {
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let id = msg.get("id").cloned();

        match method {
            "initialize" => {
                write_rpc(
                    &mut stdout,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "protocolVersion": 1,
                            "agentCapabilities": {
                                "loadSession": true,
                                "promptCapabilities": { "embeddedContext": true },
                                "mcpCapabilities": { "http": true, "sse": true }
                            },
                            "agentInfo": {
                                "name": "mock-opencode-acp",
                                "version": "1.2.15"
                            }
                        }
                    }),
                )
                .await;
            }
            "session/new" => {
                if let Some(c) = msg
                    .get("params")
                    .and_then(|p| p.get("cwd"))
                    .and_then(Value::as_str)
                {
                    cwd = c.to_owned();
                }
                write_rpc(
                    &mut stdout,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "sessionId": session_id
                        }
                    }),
                )
                .await;
            }
            "session/prompt" => {
                let prompt_id = id.expect("prompt id");
                match scenario {
                    "saturate" | "saturate_stubborn" => {
                        let total = EVENT_BUFFER * 2 + 256;
                        for i in 0..total {
                            write_rpc(
                                &mut stdout,
                                &json!({
                                    "jsonrpc": "2.0",
                                    "method": "session/update",
                                    "params": {
                                        "sessionId": session_id,
                                        "update": {
                                            "sessionUpdate": "agent_message_chunk",
                                            "content": {
                                                "type": "text",
                                                "text": format!("delta-{i} ")
                                            }
                                        }
                                    }
                                }),
                            )
                            .await;
                        }
                        if scenario == "saturate_stubborn" {
                            // Never exit on EOF; wait to be killed by AcpAdapter within SHUTDOWN_GRACE.
                            loop {
                                tokio::time::sleep(Duration::from_secs(60)).await;
                            }
                        }
                    }
                    "gate_fs_and_exec" => {
                        let allowed = PathBuf::from(&cwd).join("allowed.txt");
                        let secret = PathBuf::from(&cwd).join("secret.txt");

                        // 1. Read allowed.txt via client fs/read_text_file
                        write_rpc(
                            &mut stdout,
                            &json!({
                                "jsonrpc": "2.0",
                                "id": "req-fs-allowed",
                                "method": "fs/read_text_file",
                                "params": {
                                    "sessionId": session_id,
                                    "path": allowed.to_string_lossy()
                                }
                            }),
                        )
                        .await;
                        let allowed_resp_line =
                            lines.next_line().await.ok().flatten().unwrap_or_default();
                        let allowed_resp: Value =
                            serde_json::from_str(&allowed_resp_line).unwrap_or_default();
                        let allowed_text = allowed_resp
                            .get("result")
                            .and_then(|r| r.get("content"))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned();

                        write_rpc(
                            &mut stdout,
                            &json!({
                                "jsonrpc": "2.0",
                                "method": "session/update",
                                "params": {
                                    "sessionId": session_id,
                                    "update": {
                                        "sessionUpdate": "agent_message_chunk",
                                        "content": {
                                            "type": "text",
                                            "text": format!("Allowed content: {allowed_text}")
                                        }
                                    }
                                }
                            }),
                        )
                        .await;

                        // 2. Try reading secret.txt via client fs/read_text_file (must be denied by gate)
                        write_rpc(
                            &mut stdout,
                            &json!({
                                "jsonrpc": "2.0",
                                "id": "req-fs-secret",
                                "method": "fs/read_text_file",
                                "params": {
                                    "sessionId": session_id,
                                    "path": secret.to_string_lossy()
                                }
                            }),
                        )
                        .await;
                        let _secret_resp = lines.next_line().await;

                        // 3. Try reading secret.txt via terminal/create (must also be denied by gate)
                        write_rpc(
                            &mut stdout,
                            &json!({
                                "jsonrpc": "2.0",
                                "id": "req-term-secret",
                                "method": "terminal/create",
                                "params": {
                                    "sessionId": session_id,
                                    "command": "cat",
                                    "args": [secret.to_string_lossy()]
                                }
                            }),
                        )
                        .await;
                        let _term_resp = lines.next_line().await;

                        write_rpc(
                            &mut stdout,
                            &json!({
                                "jsonrpc": "2.0",
                                "id": prompt_id,
                                "result": {
                                    "stopReason": "end_turn"
                                }
                            }),
                        )
                        .await;
                    }
                    "approval_and_question" => {
                        write_rpc(
                            &mut stdout,
                            &json!({
                                "jsonrpc": "2.0",
                                "id": "req-perm-live",
                                "method": "session/request_permission",
                                "params": {
                                    "sessionId": session_id,
                                    "toolCall": {
                                        "toolCallId": "call_write_live",
                                        "title": "Write output.txt",
                                        "kind": "edit",
                                        "status": "pending",
                                        "rawInput": { "filePath": "output.txt", "content": "ok" },
                                        "locations": [{ "path": "output.txt" }]
                                    },
                                    "options": [
                                        { "optionId": "allow_once", "name": "Allow once", "kind": "allow_once" },
                                        { "optionId": "reject_once", "name": "Reject", "kind": "reject_once" }
                                    ]
                                }
                            }),
                        )
                        .await;
                        let _perm_resp = lines.next_line().await;

                        write_rpc(
                            &mut stdout,
                            &json!({
                                "jsonrpc": "2.0",
                                "id": "req-elicit-live",
                                "method": "elicitation/create",
                                "params": {
                                    "mode": "form",
                                    "sessionId": session_id,
                                    "message": "Select target environment",
                                    "requestedSchema": {
                                        "type": "object",
                                        "properties": {
                                            "env": {
                                                "type": "string",
                                                "title": "Environment",
                                                "enum": ["staging", "production"]
                                            }
                                        },
                                        "required": ["env"]
                                    }
                                }
                            }),
                        )
                        .await;
                        let _elicit_resp = lines.next_line().await;

                        write_rpc(
                            &mut stdout,
                            &json!({
                                "jsonrpc": "2.0",
                                "id": prompt_id,
                                "result": {
                                    "stopReason": "end_turn"
                                }
                            }),
                        )
                        .await;
                    }
                    _ => {}
                }
            }
            "session/cancel" => {
                session_id.push_str("_cancelled");
            }
            _ => {}
        }
    }
}
