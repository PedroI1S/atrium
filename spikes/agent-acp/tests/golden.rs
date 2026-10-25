use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use atrium_agents::fixture::{self, Sanitizer};
use atrium_agents::{AdapterInput, MAX_QUEUED_MESSAGES, Normalizer};
use atrium_protocol::agent::{
    AgentEvent, ApprovalDecision, MessageKind, Provider, RequestOutcome, SessionCommand,
    ToolStatus, TurnOutcome,
};
use spike_agent_acp::AcpNormalizer;

const SCENARIOS: &[&str] = &[
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
];

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("fixtures")
        .join("agents")
        .join("acp")
}

fn assert_contract_invariants(scenario: &str, events: &[AgentEvent]) {
    assert!(
        !events.is_empty(),
        "{scenario}: event list must not be empty"
    );

    if scenario == "startup-error" {
        assert!(
            matches!(events.first(), Some(AgentEvent::Error { fatal: true, .. })),
            "{scenario}: first event of pre-session failure must be Error {{ fatal: true }}, got: {:?}",
            events.first()
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::SessionStarted { .. })),
            "{scenario}: pre-session failure must not emit SessionStarted"
        );
        assert!(
            matches!(events.last(), Some(AgentEvent::SessionEnded { .. })),
            "{scenario}: last event must be SessionEnded, got: {:?}",
            events.last()
        );
        return;
    }

    assert!(
        matches!(
            events.first(),
            Some(AgentEvent::SessionStarted {
                provider: Provider::Acp,
                ..
            })
        ),
        "{scenario}: first event must be SessionStarted {{ provider: Acp }}, got: {:?}",
        events.first()
    );
    assert!(
        matches!(events.last(), Some(AgentEvent::SessionEnded { .. })),
        "{scenario}: last event must be SessionEnded, got: {:?}",
        events.last()
    );

    let mut msg_buffers: BTreeMap<String, (MessageKind, String)> = BTreeMap::new();
    let mut open_tools: BTreeMap<String, usize> = BTreeMap::new();
    let mut finished_tools: BTreeMap<String, ToolStatus> = BTreeMap::new();
    let mut open_requests: BTreeMap<String, usize> = BTreeMap::new();
    let mut resolved_requests: BTreeMap<String, RequestOutcome> = BTreeMap::new();

    for (idx, ev) in events.iter().enumerate() {
        match ev {
            AgentEvent::MessageDelta {
                message_id,
                kind,
                text,
            } => {
                let entry = msg_buffers
                    .entry(message_id.clone())
                    .or_insert((*kind, String::new()));
                assert_eq!(
                    entry.0, *kind,
                    "{scenario}: message kind changed mid-stream"
                );
                entry.1.push_str(text);
            }
            AgentEvent::MessageCompleted {
                message_id,
                kind,
                text,
            } => {
                if let Some((expected_kind, accumulated)) = msg_buffers.remove(message_id) {
                    assert_eq!(expected_kind, *kind);
                    assert_eq!(
                        &accumulated, text,
                        "{scenario}: MessageCompleted text must equal concatenation of deltas"
                    );
                }
            }
            AgentEvent::ToolCallStarted { call_id, input, .. } => {
                assert!(
                    input.is_object() || input.is_array(),
                    "{scenario}: ToolCallStarted input must be a complete JSON value"
                );
                assert!(
                    !open_tools.contains_key(call_id) && !finished_tools.contains_key(call_id),
                    "{scenario}: duplicate ToolCallStarted for {call_id}"
                );
                open_tools.insert(call_id.clone(), idx);
            }
            AgentEvent::ToolCallUpdated { call_id, .. } => {
                assert!(
                    open_tools.contains_key(call_id),
                    "{scenario}: ToolCallUpdated for non-open tool call {call_id}"
                );
            }
            AgentEvent::ToolCallFinished {
                call_id, status, ..
            } => {
                assert!(
                    open_tools.remove(call_id).is_some(),
                    "{scenario}: ToolCallFinished without matching ToolCallStarted for {call_id}"
                );
                assert!(
                    finished_tools.insert(call_id.clone(), *status).is_none(),
                    "{scenario}: duplicate ToolCallFinished for {call_id}"
                );
            }
            AgentEvent::ApprovalRequested { request_id, .. }
            | AgentEvent::QuestionAsked { request_id, .. } => {
                assert!(
                    open_requests.insert(request_id.clone(), idx).is_none()
                        && !resolved_requests.contains_key(request_id),
                    "{scenario}: duplicate request_id {request_id}"
                );
            }
            AgentEvent::RequestResolved {
                request_id,
                outcome,
            } => {
                assert!(
                    open_requests.remove(request_id).is_some(),
                    "{scenario}: RequestResolved for unknown or already-resolved {request_id}"
                );
                resolved_requests.insert(request_id.clone(), *outcome);
            }
            _ => {}
        }
    }

    assert!(
        msg_buffers.is_empty(),
        "{scenario}: unclosed message deltas remaining: {msg_buffers:?}"
    );
    assert!(
        open_tools.is_empty(),
        "{scenario}: unclosed tool calls remaining: {open_tools:?}"
    );
    assert!(
        open_requests.is_empty(),
        "{scenario}: unresolved approval/question requests remaining: {open_requests:?}"
    );

    match scenario {
        "gate-deny" | "gate-deny-execute" => {
            assert!(
                finished_tools.values().any(|s| *s == ToolStatus::Denied),
                "{scenario}: expected at least one ToolStatus::Denied, got {finished_tools:?}"
            );
        }
        "approval-allow" => {
            assert!(
                resolved_requests
                    .values()
                    .any(|o| *o == RequestOutcome::Approved),
                "{scenario}: expected RequestOutcome::Approved"
            );
            assert!(
                finished_tools.values().any(|s| *s == ToolStatus::Succeeded),
                "{scenario}: expected ToolStatus::Succeeded"
            );
        }
        "approval-deny" => {
            assert!(
                resolved_requests
                    .values()
                    .any(|o| *o == RequestOutcome::Denied),
                "{scenario}: expected RequestOutcome::Denied"
            );
            assert!(
                finished_tools.values().any(|s| *s == ToolStatus::Denied),
                "{scenario}: expected ToolStatus::Denied"
            );
        }
        "question" => {
            assert!(
                resolved_requests
                    .values()
                    .any(|o| *o == RequestOutcome::Answered),
                "{scenario}: expected RequestOutcome::Answered"
            );
        }
        "interrupt" => {
            assert!(
                finished_tools.values().any(|s| *s == ToolStatus::Cancelled),
                "{scenario}: expected open tool call to end Cancelled"
            );
            assert!(
                events.iter().any(|e| matches!(
                    e,
                    AgentEvent::TurnCompleted {
                        outcome: TurnOutcome::Interrupted,
                        ..
                    }
                )),
                "{scenario}: expected TurnCompleted with Interrupted"
            );
        }
        "multi-turn" => {
            let turn_count = events
                .iter()
                .filter(|e| matches!(e, AgentEvent::TurnCompleted { .. }))
                .count();
            assert_eq!(turn_count, 2, "{scenario}: expected 2 completed turns");
        }
        "resume" => {
            assert!(
                matches!(
                    events.first(),
                    Some(AgentEvent::SessionStarted {
                        native_session_id,
                        ..
                    }) if native_session_id == "ses_multi_01"
                ),
                "{scenario}: expected resumed session id ses_multi_01"
            );
        }
        _ => {}
    }
}

#[test]
fn all_fixtures_pass_golden_and_invariants() {
    let dir = fixtures_dir();
    let home = std::env::var("HOME").unwrap_or_default();
    let user = std::env::var("USER").unwrap_or_default();
    let sanitizer = Sanitizer::new(&home, &user, &["TOP_SECRET_CANARY_VALUE_9941"]);

    for scenario in SCENARIOS {
        let jsonl_path = dir.join(format!("{scenario}.jsonl"));
        let events_path = dir.join(format!("{scenario}.events.json"));
        let md_path = dir.join(format!("{scenario}.md"));

        assert!(jsonl_path.exists(), "missing {}", jsonl_path.display());
        assert!(md_path.exists(), "missing {}", md_path.display());

        let mut normalizer = AcpNormalizer::new();
        fixture::assert_golden(&jsonl_path, &mut normalizer);

        let lines = fixture::read(&jsonl_path).expect("fixture reads");
        let events = fixture::normalize(&lines, &mut AcpNormalizer::new());
        assert_contract_invariants(scenario, &events);

        let raw_jsonl = fs::read_to_string(&jsonl_path).expect("read jsonl");
        let raw_events = fs::read_to_string(&events_path).expect("read events.json");
        let raw_md = fs::read_to_string(&md_path).expect("read md");

        assert!(
            sanitizer.is_clean(&raw_jsonl),
            "{scenario}.jsonl failed Sanitizer::is_clean"
        );
        assert!(
            sanitizer.is_clean(&raw_events),
            "{scenario}.events.json failed Sanitizer::is_clean"
        );
        assert!(
            sanitizer.is_clean(&raw_md),
            "{scenario}.md failed Sanitizer::is_clean"
        );
        assert!(
            raw_md.contains("hand-made") || raw_md.contains("live"),
            "{scenario}.md must state whether it is hand-made or live"
        );
    }
}

#[test]
fn resolving_unknown_or_duplicate_requests_emits_non_fatal_error() {
    let mut norm = AcpNormalizer::new();
    let events = norm.on_input(&AdapterInput::Command {
        command: SessionCommand::ResolveApproval {
            request_id: "nonexistent-req".into(),
            decision: ApprovalDecision::Allow,
        },
    });
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], AgentEvent::Error { fatal: false, .. }));

    let q_events = norm.on_input(&AdapterInput::Command {
        command: SessionCommand::AnswerQuestion {
            request_id: "nonexistent-q".into(),
            answers: Vec::new(),
        },
    });
    assert_eq!(q_events.len(), 1);
    assert!(matches!(
        &q_events[0],
        AgentEvent::Error { fatal: false, .. }
    ));
}

#[test]
fn message_queue_overflow_emits_non_fatal_error() {
    let mut norm = AcpNormalizer::new();
    norm.on_input(&AdapterInput::Stdin {
        line: r#"{"jsonrpc":"2.0","id":1,"method":"session/new","params":{"cwd":"workspace","mcpServers":[]}}"#.into(),
    });
    norm.on_input(&AdapterInput::Stdout {
        line: r#"{"jsonrpc":"2.0","id":1,"result":{"sessionId":"ses_q"}}"#.into(),
    });
    let started = norm.on_input(&AdapterInput::Stdin {
        line: r#"{"jsonrpc":"2.0","id":2,"method":"session/prompt","params":{"sessionId":"ses_q","prompt":[{"type":"text","text":"turn 1"}]}}"#.into(),
    });
    assert!(matches!(
        started.as_slice(),
        [AgentEvent::TurnStarted { turn: 1 }]
    ));

    for i in 0..MAX_QUEUED_MESSAGES {
        let evs = norm.on_input(&AdapterInput::Command {
            command: SessionCommand::SendUserMessage {
                text: format!("queued {i}"),
            },
        });
        assert!(evs.is_empty(), "queued message {i} should be accepted");
    }

    let overflow = norm.on_input(&AdapterInput::Command {
        command: SessionCommand::SendUserMessage {
            text: "overflow".into(),
        },
    });
    assert_eq!(overflow.len(), 1);
    assert!(matches!(
        &overflow[0],
        AgentEvent::Error { fatal: false, .. }
    ));
}

#[test]
fn unexpected_exit_closes_open_items_in_contract_order() {
    let mut norm = AcpNormalizer::new();
    norm.on_input(&AdapterInput::Stdin {
        line: r#"{"jsonrpc":"2.0","id":1,"method":"session/new","params":{"cwd":"workspace","mcpServers":[]}}"#.into(),
    });
    norm.on_input(&AdapterInput::Stdout {
        line: r#"{"jsonrpc":"2.0","id":1,"result":{"sessionId":"ses_exit"}}"#.into(),
    });
    norm.on_input(&AdapterInput::Stdin {
        line: r#"{"jsonrpc":"2.0","id":2,"method":"session/prompt","params":{"sessionId":"ses_exit","prompt":[{"type":"text","text":"work"}]}}"#.into(),
    });
    norm.on_input(&AdapterInput::Stdout {
        line: r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"ses_exit","update":{"sessionUpdate":"tool_call","toolCallId":"tc_open","title":"run","kind":"execute","status":"in_progress"}}}"#.into(),
    });
    norm.on_input(&AdapterInput::Stdout {
        line: r#"{"jsonrpc":"2.0","id":"q_open","method":"elicitation/create","params":{"mode":"form","sessionId":"ses_exit","message":"Confirm?","requestedSchema":{"type":"object","properties":{}}}}"#.into(),
    });
    norm.on_input(&AdapterInput::Stdout {
        line: r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"ses_exit","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"partial"}}}}"#.into(),
    });

    let closing = norm.on_input(&AdapterInput::Exit { code: Some(1) });
    assert_eq!(closing.len(), 5, "closing events: {closing:?}");
    assert!(matches!(&closing[0], AgentEvent::MessageCompleted { text, .. } if text == "partial"));
    assert!(matches!(
        &closing[1],
        AgentEvent::ToolCallFinished {
            call_id,
            status: ToolStatus::Cancelled,
            ..
        } if call_id == "tc_open"
    ));
    assert!(matches!(
        &closing[2],
        AgentEvent::RequestResolved {
            request_id,
            outcome: RequestOutcome::Cancelled,
        } if request_id == "q_open"
    ));
    assert!(matches!(
        &closing[3],
        AgentEvent::TurnCompleted {
            turn: 1,
            outcome: TurnOutcome::Failed,
            ..
        }
    ));
    assert!(matches!(
        &closing[4],
        AgentEvent::SessionEnded {
            exit_code: Some(1),
            ..
        }
    ));
}
