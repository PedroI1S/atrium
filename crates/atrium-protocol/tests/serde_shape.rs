//! Pins the JSON shape of the contract types. The TypeScript side and the
//! fixtures depend on these exact forms.

use atrium_protocol::agent::*;
use atrium_protocol::wire::*;
use serde_json::json;

fn round_trip<T>(value: &T, expected: serde_json::Value)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    assert_eq!(serde_json::to_value(value).unwrap(), expected);
    assert_eq!(&serde_json::from_value::<T>(expected).unwrap(), value);
}

#[test]
fn agent_events_are_internally_tagged() {
    round_trip(
        &AgentEvent::ToolCallStarted {
            call_id: "c1".into(),
            name: "Bash".into(),
            kind: ToolKind::Execute,
            title: Some("cargo test".into()),
            input: json!({"command": "cargo test"}),
        },
        json!({
            "type": "tool_call_started", "call_id": "c1", "name": "Bash",
            "kind": "execute", "title": "cargo test", "input": {"command": "cargo test"}
        }),
    );
    round_trip(
        &AgentEvent::TurnCompleted {
            turn: 1,
            outcome: TurnOutcome::Completed,
            usage: Some(Usage {
                input_tokens: Some(10),
                output_tokens: Some(2),
                ..Usage::default()
            }),
        },
        json!({
            "type": "turn_completed", "turn": 1, "outcome": "completed",
            "usage": {
                "input_tokens": 10, "output_tokens": 2, "cache_read_tokens": null,
                "cache_write_tokens": null, "reasoning_tokens": null, "cost_usd": null
            }
        }),
    );
}

#[test]
fn session_commands_are_internally_tagged() {
    round_trip(
        &SessionCommand::ResolveApproval {
            request_id: "r1".into(),
            decision: ApprovalDecision::Deny {
                message: Some("not in this worktree".into()),
            },
        },
        json!({
            "type": "resolve_approval", "request_id": "r1",
            "decision": {"type": "deny", "message": "not in this worktree"}
        }),
    );
}

#[test]
fn questions_have_structured_answers() {
    round_trip(
        &SessionCommand::AnswerQuestion {
            request_id: "q1".into(),
            answers: vec![QuestionAnswer {
                question_id: "scope".into(),
                selected: vec!["a".into(), "c".into()],
                text: None,
            }],
        },
        json!({
            "type": "answer_question", "request_id": "q1",
            "answers": [{"question_id": "scope", "selected": ["a", "c"], "text": null}]
        }),
    );
    round_trip(
        &AgentEvent::RequestResolved {
            request_id: "q1".into(),
            outcome: RequestOutcome::Cancelled,
        },
        json!({"type": "request_resolved", "request_id": "q1", "outcome": "cancelled"}),
    );
}

#[test]
fn wire_messages() {
    round_trip(
        &ClientMessage::Subscribe {
            request_id: 3,
            thread_id: ThreadId("t1".into()),
            after: Some(EventId(41)),
        },
        json!({"type": "subscribe", "request_id": 3, "thread_id": "t1", "after": 41}),
    );
    round_trip(
        &ServerMessage::Response {
            request_id: 7,
            result: CommandResult::Err {
                code: ErrorCode::UnknownThread,
                message: "no such thread".into(),
            },
        },
        json!({
            "type": "response", "request_id": 7,
            "result": {"status": "err", "code": "unknown_thread", "message": "no such thread"}
        }),
    );
    assert_eq!(FrameKind::try_from(1), Ok(FrameKind::Stream));
    assert_eq!(FrameKind::try_from(9), Err(9));
}
