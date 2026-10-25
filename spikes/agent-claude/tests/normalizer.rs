//! Hand-made inputs for the parts of the mapping the live fixtures can't show
//! (records Claude Code 2.1.308 doesn't produce in `-p` mode, or failures).
//! These are `hand-made` in the sense of docs/contracts/m0.md §3.3.

use atrium_agents::{AdapterInput, GateDecision, Normalizer, ToolCheck};
use atrium_protocol::agent::{
    AgentEvent, ApprovalDecision, MessageKind, PlanStatus, QuestionAnswer, RequestOutcome,
    SessionCommand, ToolKind, ToolStatus, TurnOutcome,
};
use serde_json::{Value, json};
use spike_agent_claude::{ClaudeNormalizer, questions_of, tool_check, tool_kind};

fn out(v: Value) -> AdapterInput {
    AdapterInput::Stdout {
        line: v.to_string(),
    }
}

fn user_line(text: &str) -> AdapterInput {
    AdapterInput::Stdin {
        line: json!({"type":"user","message":{"role":"user","content":text}}).to_string(),
    }
}

fn init() -> AdapterInput {
    out(
        json!({"type":"system","subtype":"init","session_id":"sess-1","model":"claude-x","tools":["Read","Bash"]}),
    )
}

fn message_start() -> AdapterInput {
    out(
        json!({"type":"stream_event","event":{"type":"message_start","message":{"model":"claude-x","id":"m1"}},"api_message_id":"m1"}),
    )
}

fn assistant(blocks: Value) -> AdapterInput {
    out(json!({"type":"assistant","message":{"model":"claude-x","id":"m1","content":blocks}}))
}

fn result(extra: Value) -> AdapterInput {
    let mut v = json!({"type":"result","subtype":"success","is_error":false,"terminal_reason":"completed","total_cost_usd":0.5,"usage":{"input_tokens":10,"output_tokens":5}});
    for (k, val) in extra.as_object().unwrap() {
        v[k] = val.clone();
    }
    out(v)
}

fn run(n: &mut ClaudeNormalizer, inputs: Vec<AdapterInput>) -> Vec<AgentEvent> {
    inputs.iter().flat_map(|i| n.on_input(i)).collect()
}

/// Session started, turn 1 open.
fn started() -> (ClaudeNormalizer, Vec<AgentEvent>) {
    let mut n = ClaudeNormalizer::new();
    let evs = run(&mut n, vec![user_line("go"), init(), message_start()]);
    assert!(matches!(evs[0], AgentEvent::SessionStarted { .. }));
    assert!(matches!(evs[1], AgentEvent::TurnStarted { turn: 1 }));
    (n, evs)
}

#[test]
fn todo_write_becomes_plan_updated_without_a_tool_call() {
    let (mut n, _) = started();
    let evs = run(
        &mut n,
        vec![
            assistant(
                json!([{"type":"tool_use","id":"t1","name":"TodoWrite","input":{"todos":[
                {"content":"a","status":"completed","activeForm":"A"},
                {"content":"b","status":"in_progress","activeForm":"B"},
                {"content":"c","status":"pending","activeForm":"C"}]}}]),
            ),
            out(
                json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"Todos updated"}]}}),
            ),
        ],
    );
    assert_eq!(evs.len(), 1, "{evs:?}");
    let AgentEvent::PlanUpdated { entries } = &evs[0] else {
        panic!("{evs:?}")
    };
    let st: Vec<_> = entries.iter().map(|e| e.status).collect();
    assert_eq!(
        st,
        [
            PlanStatus::Completed,
            PlanStatus::InProgress,
            PlanStatus::Pending
        ]
    );
}

#[test]
fn thinking_text_is_reasoning() {
    let (mut n, _) = started();
    let evs = run(
        &mut n,
        vec![
            out(
                json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm "}},"api_message_id":"m1"}),
            ),
            out(
                json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"ok"}},"api_message_id":"m1"}),
            ),
            assistant(json!([{"type":"thinking","thinking":"hmm ok","signature":"s"}])),
        ],
    );
    assert!(matches!(
        &evs[0],
        AgentEvent::MessageDelta { kind: MessageKind::Reasoning, text, .. } if text == "hmm "
    ));
    assert!(matches!(
        evs.last().unwrap(),
        AgentEvent::MessageCompleted { kind: MessageKind::Reasoning, text, message_id } if text == "hmm ok" && message_id == "m1:0"
    ));
}

#[test]
fn empty_thinking_is_dropped_on_purpose() {
    // Claude Code 2.1.308 sends thinking blocks with an empty text and only a
    // signature; there is nothing to show.
    let (mut n, _) = started();
    let evs = run(
        &mut n,
        vec![assistant(
            json!([{"type":"thinking","thinking":"","signature":"s"}]),
        )],
    );
    assert!(evs.is_empty(), "{evs:?}");
}

#[test]
fn quota_windows_and_rejection() {
    let (mut n, _) = started();
    let evs = run(
        &mut n,
        vec![out(json!({"type":"rate_limit_event","rate_limit_info":{
            "status":"rejected","resetsAt":1700000000,"rateLimitType":"five_hour",
            "unifiedWindows":{"five_hour":{"utilization":1.0,"resetsAt":1700000000},"seven_day":{"utilization":0.25,"resetsAt":1700600000}}}}))],
    );
    let AgentEvent::QuotaUpdated { windows } = &evs[0] else {
        panic!("{evs:?}")
    };
    assert_eq!(windows[0].name, "5h");
    assert_eq!(windows[0].used_percent, Some(100.0));
    assert_eq!(windows[0].resets_at_ms, Some(1_700_000_000_000));
    assert_eq!(windows[1].name, "weekly");
    assert_eq!(windows[1].used_percent, Some(25.0));
    assert_eq!(evs.len(), 1, "{evs:?}");
    // The refusal itself is reported once, when the turn ends without a
    // refusal message (with a message it comes with the message).
    let end = run(
        &mut n,
        vec![result(
            json!({"is_error":true,"terminal_reason":"api_error"}),
        )],
    );
    assert!(
        matches!(
            &end[0],
            AgentEvent::RateLimited {
                resets_at_ms: Some(1_700_000_000_000),
                message: None
            }
        ),
        "{end:?}"
    );
    // The same status again changes nothing.
    let again = run(
        &mut n,
        vec![out(json!({"type":"rate_limit_event","rate_limit_info":{
            "status":"allowed","resetsAt":1700000000,
            "unifiedWindows":{"five_hour":{"utilization":1.0,"resetsAt":1700000000},"seven_day":{"utilization":0.25,"resetsAt":1700600000}}}}))],
    );
    assert!(again.is_empty(), "{again:?}");
}

#[test]
fn synthetic_rate_limit_message_is_a_refusal() {
    let mut n = ClaudeNormalizer::new();
    let evs = run(
        &mut n,
        vec![
            user_line("go"),
            init(),
            out(
                json!({"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1700000000}}),
            ),
            out(
                json!({"type":"assistant","error":"rate_limit","message":{"model":"<synthetic>","id":"x","content":[{"type":"text","text":"You've hit your limit"}]}}),
            ),
            result(
                json!({"is_error":true,"terminal_reason":"api_error","result":"You've hit your limit"}),
            ),
        ],
    );
    // A limit is the provider talking, not a startup failure.
    let kinds: Vec<&str> = evs
        .iter()
        .map(|e| match e {
            AgentEvent::SessionStarted { .. } => "started",
            AgentEvent::TurnStarted { .. } => "turn",
            AgentEvent::QuotaUpdated { .. } => "quota",
            AgentEvent::Usage { .. } => "usage",
            AgentEvent::RateLimited { .. } => "limited",
            AgentEvent::TurnCompleted {
                outcome: TurnOutcome::Failed,
                ..
            } => "failed",
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(kinds, ["started", "turn", "limited", "usage", "failed"]);
}

#[test]
fn rate_limit_mid_session_ends_the_turn_failed_with_rate_limited() {
    let (mut n, _) = started();
    let evs = run(
        &mut n,
        vec![
            out(
                json!({"type":"assistant","error":"rate_limit","message":{"model":"<synthetic>","id":"x","content":[{"type":"text","text":"You've hit your limit"}]}}),
            ),
            result(
                json!({"is_error":true,"terminal_reason":"api_error","result":"You've hit your limit"}),
            ),
        ],
    );
    assert!(
        matches!(&evs[0], AgentEvent::RateLimited { message: Some(m), .. } if m.contains("limit"))
    );
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Failed,
            ..
        }
    )));
    // No duplicate Error for the same refusal.
    assert!(!evs.iter().any(|e| matches!(e, AgentEvent::Error { .. })));
}

#[test]
fn max_turns_is_a_limit() {
    let (mut n, _) = started();
    let evs = run(
        &mut n,
        vec![result(
            json!({"subtype":"error_max_turns","is_error":true,"terminal_reason":"max_turns"}),
        )],
    );
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::LimitReached,
            ..
        }
    )));
}

#[test]
fn turn_cost_is_the_difference_of_the_session_total() {
    let (mut n, _) = started();
    let first = run(&mut n, vec![result(json!({"total_cost_usd":0.5}))]);
    let second = run(
        &mut n,
        vec![
            user_line("again"),
            message_start(),
            result(json!({"total_cost_usd":0.75})),
        ],
    );
    let cost = |evs: &[AgentEvent]| {
        evs.iter().find_map(|e| match e {
            AgentEvent::TurnCompleted { usage: Some(u), .. } => u.cost_usd,
            _ => None,
        })
    };
    assert_eq!(cost(&first), Some(0.5));
    assert_eq!(cost(&second), Some(0.25));
}

fn can_use(rid: &str, tool_use_id: &str) -> AdapterInput {
    out(json!({"type":"control_request","request_id":rid,"request":{
        "subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"},
        "tool_use_id":tool_use_id,"decision_reason":"","description":"List files"}}))
}

fn bash_call(id: &str) -> AdapterInput {
    assistant(json!([{"type":"tool_use","id":id,"name":"Bash","input":{"command":"ls"}}]))
}

fn gate(id: &str, d: GateDecision) -> AdapterInput {
    AdapterInput::Gate {
        check: tool_check(Some(id), "Bash", &json!({"command":"ls"})),
        decision: d,
    }
}

#[test]
fn can_use_tool_waits_for_the_gate_and_only_ask_reaches_the_client() {
    // Not preceded by a hook ask: the gate decides afterwards.
    let (mut n, _) = started();
    let evs = run(&mut n, vec![bash_call("t1"), can_use("r1", "t1")]);
    assert_eq!(evs.len(), 1);
    assert!(matches!(evs[0], AgentEvent::ToolCallStarted { .. }));
    let evs = run(&mut n, vec![gate("t1", GateDecision::Ask)]);
    assert!(matches!(
        &evs[0],
        AgentEvent::ApprovalRequested { request_id, call_id: Some(c), kind: ToolKind::Execute, .. }
            if request_id == "r1" && c == "t1"
    ));

    // The gate allows: nothing for the client.
    let (mut n, _) = started();
    let evs = run(
        &mut n,
        vec![
            bash_call("t1"),
            can_use("r1", "t1"),
            gate("t1", GateDecision::Allow),
        ],
    );
    assert_eq!(evs.len(), 1);

    // Preceded by a hook ask: the request is shown immediately.
    let (mut n, _) = started();
    let evs = run(
        &mut n,
        vec![
            bash_call("t1"),
            gate("t1", GateDecision::Ask),
            can_use("r1", "t1"),
        ],
    );
    assert!(matches!(
        evs.last().unwrap(),
        AgentEvent::ApprovalRequested { .. }
    ));
}

#[test]
fn resolving_twice_or_unknown_is_a_non_fatal_error() {
    let (mut n, _) = started();
    run(
        &mut n,
        vec![
            bash_call("t1"),
            gate("t1", GateDecision::Ask),
            can_use("r1", "t1"),
        ],
    );
    let resolve = |rid: &str, d| AdapterInput::Command {
        command: SessionCommand::ResolveApproval {
            request_id: rid.into(),
            decision: d,
        },
    };
    let evs = run(&mut n, vec![resolve("r1", ApprovalDecision::Allow)]);
    assert!(matches!(
        &evs[..],
        [AgentEvent::RequestResolved {
            outcome: RequestOutcome::Approved,
            ..
        }]
    ));
    for rid in ["r1", "nope"] {
        let evs = run(&mut n, vec![resolve(rid, ApprovalDecision::Allow)]);
        assert!(
            matches!(&evs[..], [AgentEvent::Error { fatal: false, .. }]),
            "{evs:?}"
        );
    }
    // Answering an approval as if it were a question is rejected too.
    let evs = run(
        &mut n,
        vec![AdapterInput::Command {
            command: SessionCommand::AnswerQuestion {
                request_id: "r1".into(),
                answers: vec![],
            },
        }],
    );
    assert!(matches!(&evs[..], [AgentEvent::Error { fatal: false, .. }]));
}

#[test]
fn denying_an_approval_denies_the_call_and_swallows_the_provider_result() {
    let (mut n, _) = started();
    run(
        &mut n,
        vec![
            bash_call("t1"),
            gate("t1", GateDecision::Ask),
            can_use("r1", "t1"),
        ],
    );
    let evs = run(
        &mut n,
        vec![AdapterInput::Command {
            command: SessionCommand::ResolveApproval {
                request_id: "r1".into(),
                decision: ApprovalDecision::Deny {
                    message: Some("no".into()),
                },
            },
        }],
    );
    assert!(matches!(
        &evs[..],
        [
            AgentEvent::RequestResolved { outcome: RequestOutcome::Denied, .. },
            AgentEvent::ToolCallFinished { status: ToolStatus::Denied, error: Some(e), .. },
        ] if e == "no"
    ));
    let evs = run(
        &mut n,
        vec![out(
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"no","is_error":true}]}}),
        )],
    );
    assert!(evs.is_empty(), "{evs:?}");
}

#[test]
fn a_gate_deny_that_arrives_before_the_call_is_applied_when_it_starts() {
    let (mut n, _) = started();
    let evs = run(
        &mut n,
        vec![
            gate("t9", GateDecision::Deny { reason: "x".into() }),
            bash_call("t9"),
        ],
    );
    assert!(matches!(&evs[0], AgentEvent::ToolCallStarted { .. }));
    assert!(matches!(
        &evs[1],
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Denied,
            ..
        }
    ));
}

#[test]
fn provider_cancelled_request_is_cancelled() {
    let (mut n, _) = started();
    run(
        &mut n,
        vec![
            bash_call("t1"),
            gate("t1", GateDecision::Ask),
            can_use("r1", "t1"),
        ],
    );
    let evs = run(
        &mut n,
        vec![out(
            json!({"type":"control_cancel_request","request_id":"r1"}),
        )],
    );
    assert!(matches!(
        &evs[..],
        [AgentEvent::RequestResolved {
            outcome: RequestOutcome::Cancelled,
            ..
        }]
    ));
}

#[test]
fn exit_mid_turn_closes_everything_in_order() {
    let (mut n, _) = started();
    run(
        &mut n,
        vec![
            out(
                json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"par"}},"api_message_id":"m1"}),
            ),
            out(
                json!({"type":"assistant","message":{"model":"claude-x","id":"m0","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}),
            ),
            gate("t1", GateDecision::Ask),
            can_use("r1", "t1"),
        ],
    );
    let evs = run(&mut n, vec![AdapterInput::Exit { code: Some(137) }]);
    let kinds: Vec<&str> = evs
        .iter()
        .map(|e| match e {
            AgentEvent::MessageCompleted { text, .. } if text == "par" => "message",
            AgentEvent::ToolCallFinished {
                status: ToolStatus::Cancelled,
                ..
            } => "call",
            AgentEvent::RequestResolved {
                outcome: RequestOutcome::Cancelled,
                ..
            } => "request",
            AgentEvent::Error { fatal: true, .. } => "error",
            AgentEvent::TurnCompleted {
                outcome: TurnOutcome::Failed,
                ..
            } => "turn",
            AgentEvent::SessionEnded {
                exit_code: Some(137),
                ..
            } => "end",
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        kinds,
        ["message", "call", "request", "error", "turn", "end"]
    );
}

#[test]
fn multi_select_question_ids_are_stable() {
    let input = json!({"questions":[{"question":"Pick","header":"H","multiSelect":true,
        "options":[{"label":"A","description":"a"},{"label":"B","description":""}]}]});
    let qs = questions_of(&input);
    assert_eq!(qs[0].id, "q0");
    assert!(qs[0].multi_select && qs[0].allow_text);
    assert_eq!(qs[0].options[1].id, "o1");
    assert_eq!(qs[0].options[1].description, None);
    let _ = QuestionAnswer {
        question_id: "q0".into(),
        selected: vec!["o0".into(), "o1".into()],
        text: None,
    };
}

#[test]
fn unknown_records_are_unmapped_not_dropped() {
    let (mut n, _) = started();
    let evs = run(
        &mut n,
        vec![
            out(json!({"type":"brand_new_thing","x":1})),
            out(json!({"type":"system","subtype":"hook_started"})),
            AdapterInput::Stdout {
                line: "not json".into(),
            },
        ],
    );
    let types: Vec<&str> = evs
        .iter()
        .map(|e| match e {
            AgentEvent::Unmapped { record_type, .. } => record_type.as_str(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        types,
        ["brand_new_thing", "system/hook_started", "unparseable"]
    );
}

#[test]
fn tool_names_map_to_kinds() {
    for (name, kind) in [
        ("Read", ToolKind::Read),
        ("Edit", ToolKind::Edit),
        ("Write", ToolKind::Edit),
        ("NotebookEdit", ToolKind::Edit),
        ("Grep", ToolKind::Search),
        ("Glob", ToolKind::Search),
        ("Bash", ToolKind::Execute),
        ("WebFetch", ToolKind::Fetch),
        ("WebSearch", ToolKind::Fetch),
        ("mcp__echo__echo", ToolKind::Mcp),
        ("Agent", ToolKind::Other),
    ] {
        assert_eq!(tool_kind(name), kind, "{name}");
    }
    let c: ToolCheck = tool_check(Some("t"), "Bash", &json!({"command":"ls -la"}));
    assert_eq!(c.title.as_deref(), Some("ls -la"));
    assert_eq!(c.kind, ToolKind::Execute);
}

#[test]
fn closing_a_session_that_never_had_a_turn_is_just_session_ended() {
    let mut n = ClaudeNormalizer::new();
    let evs = run(
        &mut n,
        vec![
            AdapterInput::Command {
                command: SessionCommand::Shutdown,
            },
            AdapterInput::Exit { code: Some(0) },
        ],
    );
    assert!(matches!(&evs[..], [AgentEvent::SessionEnded { .. }]));
}

#[test]
fn interrupt_that_the_cli_answers_with_a_result_is_interrupted() {
    let (mut n, _) = started();
    let evs = run(
        &mut n,
        vec![
            AdapterInput::Command {
                command: SessionCommand::Interrupt,
            },
            result(
                json!({"subtype":"error_during_execution","is_error":true,"terminal_reason":"aborted_streaming"}),
            ),
        ],
    );
    assert!(matches!(
        evs.last().unwrap(),
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Interrupted,
            ..
        }
    ));
    assert!(!evs.iter().any(|e| matches!(e, AgentEvent::Error { .. })));
}

#[test]
fn a_cli_that_leaves_after_an_interrupt_keeps_the_session_open() {
    let (mut n, _) = started();
    let evs = run(
        &mut n,
        vec![
            AdapterInput::Command {
                command: SessionCommand::Interrupt,
            },
            AdapterInput::Exit { code: None },
        ],
    );
    assert!(matches!(
        evs.last().unwrap(),
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Interrupted,
            ..
        }
    ));
    assert!(
        !evs.iter()
            .any(|e| matches!(e, AgentEvent::SessionEnded { .. }))
    );
    // A later shutdown ends it.
    let evs = run(
        &mut n,
        vec![
            AdapterInput::Command {
                command: SessionCommand::Shutdown,
            },
            AdapterInput::Exit { code: None },
        ],
    );
    assert!(matches!(&evs[..], [AgentEvent::SessionEnded { .. }]));
}
