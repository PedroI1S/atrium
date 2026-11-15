#![doc = "Unit tests for CodexNormalizer M1 contract compliance."]

use std::collections::BTreeMap;

use atrium_agents::codex::normalizer::{
    CodexNormalizer, DEADLINE_INTERRUPT, DEADLINE_RECORD_TOO_LARGE, coverage,
};
use atrium_agents::codex::tools;
use atrium_agents::{
    AdapterInput, ApprovalTicket, GateDecision, MAX_QUEUED_MESSAGES, Normalizer, ToolCheck,
};
use atrium_protocol::agent::{
    AgentEvent, ApprovalDecision, MessageBody, MessageKind, PlanStatus, Provider, QuestionAnswer,
    RequestOutcome, SessionCommand, ToolKind, ToolStatus, TurnOutcome,
};
use atrium_protocol::limits::{
    MAX_DIAGNOSTIC_LINE_BYTES, MAX_DIAGNOSTIC_LINES, MAX_INLINE_MESSAGE_BYTES,
    MAX_TOOL_OUTPUT_BYTES, MAX_UNMAPPED_RAW_BYTES,
};
use atrium_protocol::policy::{
    Coverage, EnforcementDescriptor, EnforcementLevel, ToolClass, ToolSubject,
};
use serde_json::{Value, json};

fn init_seq(norm: &mut CodexNormalizer) -> Vec<AgentEvent> {
    let mut out = Vec::new();
    let desc = EnforcementDescriptor::evaluate(
        Provider::Codex,
        Some("0.163.0".into()),
        true,
        coverage(),
        Vec::new(),
        Vec::new(),
    );
    out.extend(norm.on_input(&AdapterInput::Enforcement { descriptor: desc }));
    out.extend(
        norm.on_input(&AdapterInput::Stdout {
            line: json!({
                "id": "atrium-init",
                "result": { "userAgent": "codex/0.163.0" }
            })
            .to_string(),
        }),
    );
    out.extend(
        norm.on_input(&AdapterInput::Stdout {
            line: json!({
                "id": "atrium-thread",
                "result": {
                    "thread": { "id": "thread-1", "cliVersion": "0.163.0" },
                    "model": "gpt-5"
                }
            })
            .to_string(),
        }),
    );
    out
}

#[test]
fn pre_session_stderr_caps_at_50_lines_of_2_kib_and_emits_two_events() {
    let mut norm = CodexNormalizer::new();
    for i in 0..80 {
        let long = format!("{i}:{}", "z".repeat(4000));
        let evs = norm.on_input(&AdapterInput::Stderr { line: long });
        assert!(
            evs.is_empty(),
            "pre-session stderr must be buffered silently"
        );
    }
    let evs = norm.on_input(&AdapterInput::Exit { code: Some(2) });
    assert_eq!(evs.len(), 2);
    let AgentEvent::Error {
        fatal: true,
        details,
        ..
    } = &evs[0]
    else {
        panic!("expected fatal Error, got {evs:?}");
    };
    assert_eq!(details.len(), MAX_DIAGNOSTIC_LINES);
    for d in details {
        assert!(d.len() <= MAX_DIAGNOSTIC_LINE_BYTES);
    }
    assert!(details.last().unwrap().starts_with("79:"));
    assert!(matches!(
        &evs[1],
        AgentEvent::SessionEnded {
            exit_code: Some(2),
            ..
        }
    ));
    assert!(norm.has_ended());
    // Further exits do not duplicate SessionEnded.
    assert!(
        norm.on_input(&AdapterInput::Exit { code: Some(2) })
            .is_empty()
    );
}

#[test]
fn oversize_inline_completion_splits_into_deltas_with_sha256() {
    let mut norm = CodexNormalizer::new();
    init_seq(&mut norm);
    let _ = norm.on_input(&AdapterInput::Command {
        command: SessionCommand::SendUserMessage { text: "hi".into() },
    });
    let _ = norm.on_input(&AdapterInput::Stdout {
        line: json!({ "method": "turn/started", "params": { "turn": { "id": "t1" } } }).to_string(),
    });
    let big = "a".repeat(MAX_INLINE_MESSAGE_BYTES + 123);
    let evs = norm.on_input(&AdapterInput::Stdout {
        line: json!({
            "method": "item/completed",
            "params": {
                "turnId": "t1",
                "item": { "type": "agentMessage", "id": "m1", "text": big }
            }
        })
        .to_string(),
    });
    let deltas: Vec<&str> = evs
        .iter()
        .filter_map(|e| match e {
            AgentEvent::MessageDelta {
                kind: MessageKind::Assistant,
                text,
                ..
            } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(deltas.len() >= 2);
    let joined = deltas.concat();
    assert_eq!(joined.len(), MAX_INLINE_MESSAGE_BYTES + 123);
    let Some(AgentEvent::MessageCompleted {
        body:
            MessageBody::Streamed {
                chars,
                deltas: count,
                sha256,
            },
        ..
    }) = evs.last()
    else {
        panic!("expected Streamed completion, got {evs:?}");
    };
    assert_eq!(*chars, joined.chars().count() as u64);
    assert_eq!(*count, deltas.len() as u64);
    assert_eq!(sha256.len(), 64);
}

#[test]
fn tool_output_caps_at_1_mib_and_reports_truncated_bytes() {
    let mut norm = CodexNormalizer::new();
    init_seq(&mut norm);
    let _ = norm.on_input(&AdapterInput::Stdout {
        line: json!({ "method": "turn/started", "params": { "turn": { "id": "t1" } } }).to_string(),
    });
    let _ = norm.on_input(&AdapterInput::Stdout {
        line: json!({
            "method": "item/started",
            "params": {
                "turnId": "t1",
                "item": { "type": "commandExecution", "id": "c1", "command": "yes", "cwd": "/scratch", "status": "inProgress" }
            }
        })
        .to_string(),
    });
    let chunk = "b".repeat(MAX_TOOL_OUTPUT_BYTES + 500);
    let d_evs = norm.on_input(&AdapterInput::Stdout {
        line: json!({
            "method": "item/commandExecution/outputDelta",
            "params": { "itemId": "c1", "delta": chunk }
        })
        .to_string(),
    });
    assert!(matches!(
        d_evs.as_slice(),
        [AgentEvent::ToolCallUpdated {
            output_delta: Some(s),
            ..
        }] if s.len() == MAX_TOOL_OUTPUT_BYTES
    ));
    let f_evs = norm.on_input(&AdapterInput::Stdout {
        line: json!({
            "method": "item/completed",
            "params": {
                "turnId": "t1",
                "item": { "type": "commandExecution", "id": "c1", "command": "yes", "cwd": "/scratch", "status": "completed", "aggregatedOutput": "more" }
            }
        })
        .to_string(),
    });
    assert!(f_evs.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Succeeded,
            truncated_bytes: 504,
            ..
        }
    )));
}

#[test]
fn unmapped_record_caps_raw_payload_and_record_too_large_is_fatal() {
    let mut norm = CodexNormalizer::new();
    init_seq(&mut norm);
    let big = "u".repeat(MAX_UNMAPPED_RAW_BYTES + 1024);
    let evs = norm.on_input(&AdapterInput::Stdout {
        line: json!({ "method": "vendor/custom", "params": { "blob": big } }).to_string(),
    });
    assert!(matches!(
        evs.as_slice(),
        [AgentEvent::Unmapped {
            raw: Value::Object(map),
            ..
        }] if map.get("truncated").and_then(Value::as_str).is_some_and(|s| s.len() <= MAX_UNMAPPED_RAW_BYTES)
            && map.get("originalBytes").and_then(Value::as_u64).is_some()
    ));

    let dl = norm.on_input(&AdapterInput::Deadline {
        what: DEADLINE_RECORD_TOO_LARGE.into(),
    });
    assert!(matches!(
        dl.as_slice(),
        [AgentEvent::Error { fatal: true, .. }]
    ));
    assert!(norm.wants_end());
    let ended = norm.on_input(&AdapterInput::Exit { code: Some(1) });
    assert!(matches!(
        ended.as_slice(),
        [AgentEvent::SessionEnded { .. }]
    ));
    assert!(norm.has_ended());
}

#[test]
fn queued_messages_beyond_max_are_rejected_and_marked_rejected() {
    let mut norm = CodexNormalizer::new();
    init_seq(&mut norm);
    // Start turn 1.
    let _ = norm.on_input(&AdapterInput::Command {
        command: SessionCommand::SendUserMessage { text: "t1".into() },
    });
    let _ = norm.on_input(&AdapterInput::Stdin {
        line: json!({ "id": "atrium-turn-1", "method": "turn/start", "params": { "threadId": "thread-1", "input": [{ "type": "text", "text": "t1" }] } }).to_string(),
    });
    assert!(norm.turn_in_flight());
    // Queue MAX_QUEUED_MESSAGES more turns while turn 1 is open.
    for i in 0..MAX_QUEUED_MESSAGES {
        let evs = norm.on_input(&AdapterInput::Command {
            command: SessionCommand::SendUserMessage {
                text: format!("q{i}"),
            },
        });
        assert!(evs.is_empty(), "queued message {i} should succeed");
        assert!(!norm.take_rejected());
    }
    // The next one must fail non-fatally and set `take_rejected`.
    let rej = norm.on_input(&AdapterInput::Command {
        command: SessionCommand::SendUserMessage {
            text: "overflow".into(),
        },
    });
    assert!(norm.take_rejected());
    assert!(matches!(
        rej.as_slice(),
        [AgentEvent::Error { fatal: false, .. }]
    ));
    // Completing turn 1 promotes q0 cleanly.
    let done1 = norm.on_input(&AdapterInput::Stdout {
        line: json!({ "method": "turn/completed", "params": { "turn": { "id": "t1", "status": "completed" } } }).to_string(),
    });
    assert!(done1.iter().any(|e| matches!(
        e,
        AgentEvent::TurnCompleted {
            turn: 1,
            outcome: TurnOutcome::Completed,
            ..
        }
    )));
    let next_turn = norm.on_input(&AdapterInput::Stdin {
        line: json!({ "id": "atrium-turn-2", "method": "turn/start", "params": { "threadId": "thread-1", "input": [{ "type": "text", "text": "q0" }] } }).to_string(),
    });
    assert!(next_turn.iter().any(|e| matches!(
        e,
        AgentEvent::MessageCompleted {
            kind: MessageKind::User,
            body: MessageBody::Inline { text },
            ..
        } if text == "q0"
    )));
}

#[test]
fn interrupt_deadline_closes_open_turn_with_interrupted() {
    let mut norm = CodexNormalizer::new();
    init_seq(&mut norm);
    let _ = norm.on_input(&AdapterInput::Command {
        command: SessionCommand::SendUserMessage {
            text: "work".into(),
        },
    });
    let _ = norm.on_input(&AdapterInput::Stdout {
        line: json!({ "method": "turn/started", "params": { "turn": { "id": "t1" } } }).to_string(),
    });
    let dl = norm.on_input(&AdapterInput::Deadline {
        what: DEADLINE_INTERRUPT.into(),
    });
    assert!(dl.is_empty());
    let evs = norm.on_input(&AdapterInput::Exit { code: None });
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::TurnCompleted {
            turn: 1,
            outcome: TurnOutcome::Interrupted,
            ..
        }
    )));
    assert!(matches!(
        evs.last(),
        Some(AgentEvent::SessionEnded {
            reason: Some(r),
            ..
        }) if r.contains("interrupt")
    ));
    assert!(norm.has_ended());
}

#[test]
fn redeem_denial_overrides_resolve_approval_allow() {
    let mut norm = CodexNormalizer::new();
    init_seq(&mut norm);
    let _ = norm.on_input(&AdapterInput::Command {
        command: SessionCommand::SendUserMessage { text: "run".into() },
    });
    let _ = norm.on_input(&AdapterInput::Stdout {
        line: json!({ "method": "turn/started", "params": { "turn": { "id": "t1" } } }).to_string(),
    });
    let payload = json!({
        "itemId": "c-1",
        "command": "echo hi",
        "cwd": "/scratch",
    });
    let check = tools::tool_check(
        Some("c-1".into()),
        "commandExecution".into(),
        &payload,
        "/scratch",
        1,
    );
    let ticket = ApprovalTicket {
        request_id: "tk-1".into(),
        rememberable: true,
    };
    let gate_evs = norm.on_input(&AdapterInput::Gate {
        check: check.clone(),
        decision: GateDecision::Ask {
            ticket: ticket.clone(),
        },
    });
    assert!(gate_evs.is_empty());

    let req_evs = norm.on_input(&AdapterInput::Stdout {
        line: json!({
            "id": "native-1",
            "method": "item/commandExecution/requestApproval",
            "params": payload
        })
        .to_string(),
    });
    assert!(req_evs.iter().any(|e| matches!(
        e,
        AgentEvent::ApprovalRequested { request_id, .. } if request_id == "tk-1"
    )));

    let res_evs = norm.on_input(&AdapterInput::Command {
        command: SessionCommand::ResolveApproval {
            request_id: "tk-1".into(),
            decision: ApprovalDecision::Allow,
        },
    });
    assert!(res_evs.iter().any(|e| matches!(
        e,
        AgentEvent::RequestResolved {
            request_id,
            outcome: RequestOutcome::Approved,
        } if request_id == "tk-1"
    )));

    let redeem_evs = norm.on_input(&AdapterInput::Redeem {
        ticket,
        check,
        decision: GateDecision::Deny {
            reason: "revoked".into(),
        },
    });
    assert!(redeem_evs.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Denied,
            error: Some(err),
            ..
        } if err == "revoked"
    )));
}

#[test]
fn question_ticket_none_does_not_ask_client_and_some_asks() {
    let mut norm = CodexNormalizer::new();
    init_seq(&mut norm);
    let _ = norm.on_input(&AdapterInput::Command {
        command: SessionCommand::SendUserMessage { text: "ask".into() },
    });
    let _ = norm.on_input(&AdapterInput::Stdout {
        line: json!({ "method": "turn/started", "params": { "turn": { "id": "t1" } } }).to_string(),
    });
    let _ = norm.on_input(&AdapterInput::Stdout {
        line: json!({
            "id": "nq-1",
            "method": "item/tool/requestUserInput",
            "params": {
                "questions": [{
                    "id": "q1",
                    "question": "Pick",
                    "options": [{ "label": "A" }]
                }]
            }
        })
        .to_string(),
    });
    let none_evs = norm.on_input(&AdapterInput::QuestionTicket {
        id: "nq-1".into(),
        ticket: None,
    });
    assert!(none_evs.is_empty());

    let _ = norm.on_input(&AdapterInput::Stdout {
        line: json!({
            "id": "nq-2",
            "method": "item/tool/requestUserInput",
            "params": {
                "questions": [{
                    "id": "q2",
                    "question": "Pick again",
                    "options": [{ "label": "B" }]
                }]
            }
        })
        .to_string(),
    });
    let asked = norm.on_input(&AdapterInput::QuestionTicket {
        id: "nq-2".into(),
        ticket: Some(ApprovalTicket {
            request_id: "qt-2".into(),
            rememberable: false,
        }),
    });
    assert!(asked.iter().any(|e| matches!(
        e,
        AgentEvent::QuestionAsked { request_id, .. } if request_id == "qt-2"
    )));

    let ans = norm.on_input(&AdapterInput::Command {
        command: SessionCommand::AnswerQuestion {
            request_id: "qt-2".into(),
            answers: vec![QuestionAnswer {
                question_id: "q2".into(),
                selected: vec!["0".into()],
                text: None,
            }],
        },
    });
    assert!(ans.iter().any(|e| matches!(
        e,
        AgentEvent::RequestResolved {
            request_id,
            outcome: RequestOutcome::Answered,
        } if request_id == "qt-2"
    )));
}

#[test]
fn decline_question_resolves_cancelled_and_unknown_emits_non_fatal_error() {
    let mut norm = CodexNormalizer::new();
    init_seq(&mut norm);
    let _ = norm.on_input(&AdapterInput::Command {
        command: SessionCommand::SendUserMessage { text: "ask".into() },
    });
    let _ = norm.on_input(&AdapterInput::Stdout {
        line: json!({ "method": "turn/started", "params": { "turn": { "id": "t1" } } }).to_string(),
    });
    let _ = norm.on_input(&AdapterInput::Stdout {
        line: json!({
            "id": "nq-decl",
            "method": "item/tool/requestUserInput",
            "params": {
                "questions": [{
                    "id": "q-decl",
                    "question": "Pick",
                    "options": [{ "label": "A" }]
                }]
            }
        })
        .to_string(),
    });
    let asked = norm.on_input(&AdapterInput::QuestionTicket {
        id: "nq-decl".into(),
        ticket: Some(ApprovalTicket {
            request_id: "qt-decl".into(),
            rememberable: false,
        }),
    });
    assert!(asked.iter().any(|e| matches!(
        e,
        AgentEvent::QuestionAsked { request_id, .. } if request_id == "qt-decl"
    )));

    let declined = norm.on_input(&AdapterInput::Command {
        command: SessionCommand::DeclineQuestion {
            request_id: "qt-decl".into(),
        },
    });
    assert_eq!(
        declined,
        vec![AgentEvent::RequestResolved {
            request_id: "qt-decl".into(),
            outcome: RequestOutcome::Cancelled,
        }]
    );

    let unknown = norm.on_input(&AdapterInput::Command {
        command: SessionCommand::DeclineQuestion {
            request_id: "qt-decl".into(),
        },
    });
    assert!(matches!(
        unknown.as_slice(),
        [AgentEvent::Error { fatal: false, message, .. }] if message.contains("unknown request")
    ));
}

#[test]
fn plan_rate_limit_and_tool_subjects_map_cleanly() {
    let mut norm = CodexNormalizer::new();
    init_seq(&mut norm);
    let _ = norm.on_input(&AdapterInput::Command {
        command: SessionCommand::SendUserMessage { text: "go".into() },
    });
    let _ = norm.on_input(&AdapterInput::Stdout {
        line: json!({ "method": "turn/started", "params": { "turn": { "id": "t1" } } }).to_string(),
    });
    let plan_evs = norm.on_input(&AdapterInput::Stdout {
        line: json!({
            "method": "turn/plan/updated",
            "params": {
                "turnId": "t1",
                "plan": [
                    { "step": "Read files", "status": "completed" },
                    { "step": "Edit", "status": "inProgress" },
                    { "step": "Verify", "status": "pending" }
                ]
            }
        })
        .to_string(),
    });
    assert!(matches!(
        plan_evs.as_slice(),
        [AgentEvent::PlanUpdated { entries }]
            if entries.len() == 3
                && entries[0].status == PlanStatus::Completed
                && entries[1].status == PlanStatus::InProgress
                && entries[2].status == PlanStatus::Pending
    ));

    let err_evs = norm.on_input(&AdapterInput::Stdout {
        line: json!({
            "method": "error",
            "params": {
                "willRetry": false,
                "error": { "message": "too many requests", "codexErrorInfo": "usageLimitExceeded" }
            }
        })
        .to_string(),
    });
    assert!(
        err_evs
            .iter()
            .any(|e| matches!(e, AgentEvent::RateLimited { .. }))
    );

    let enforcement: BTreeMap<ToolClass, Coverage> = coverage()
        .into_iter()
        .map(|c| (c.class, c.coverage))
        .collect();
    assert_eq!(
        enforcement.get(&ToolClass::Read).copied(),
        Some(Coverage::ProviderGated)
    );
    assert_eq!(
        enforcement.get(&ToolClass::Execute).copied(),
        Some(Coverage::ProviderGated)
    );
    assert_eq!(
        enforcement.get(&ToolClass::Fetch).copied(),
        Some(Coverage::NotIntercepted)
    );
    let mcp_check: ToolCheck = tools::tool_check(
        Some("m1".into()),
        "mcpToolCall".into(),
        &json!({ "server": "docs", "tool": "search" }),
        "/scratch",
        1,
    );
    assert_eq!(mcp_check.kind, ToolKind::Mcp);
    assert_eq!(
        mcp_check.subject,
        ToolSubject::Mcp {
            server: "docs".into(),
            tool: "search".into(),
        }
    );
    let desc = EnforcementDescriptor::evaluate(
        Provider::Codex,
        Some("0.163.0".into()),
        true,
        coverage(),
        Vec::new(),
        Vec::new(),
    );
    assert_eq!(desc.level, EnforcementLevel::Partial);
}
