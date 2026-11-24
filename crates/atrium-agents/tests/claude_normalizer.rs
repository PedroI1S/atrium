//! Hand-made inputs for the parts of the mapping the live fixtures can't show
//! (records Claude Code 2.1.308 doesn't produce in `-p` mode, failures, and
//! the M1 limits). These are `hand-made` in the sense of
//! docs/contracts/m0.md §3.3, on the rev-3 contract: tickets, `MessageBody`,
//! `Error::details`, and an interrupt the CLI doesn't honour ending the
//! session.

use atrium_agents::claude::{ClaudeNormalizer, questions_of, tool_check, tool_kind};
use atrium_agents::{AdapterInput, ApprovalTicket, GateDecision, Normalizer, ToolCheck};
use atrium_protocol::agent::{
    AgentEvent, ApprovalDecision, MessageBody, MessageKind, PlanStatus, Provider, QuestionAnswer,
    RequestOutcome, SessionCommand, ToolKind, ToolStatus, TurnOutcome, Usage,
};
use atrium_protocol::limits::{
    MAX_DIAGNOSTIC_LINE_BYTES, MAX_DIAGNOSTIC_LINES, MAX_INLINE_MESSAGE_BYTES,
    MAX_PARTIAL_ARGS_BYTES, MAX_PROVIDER_RECORD_BYTES, MAX_TOOL_OUTPUT_BYTES,
    MAX_UNMAPPED_RAW_BYTES,
};
use atrium_protocol::policy::{
    EnforcementDescriptor, EnforcementLevel, EnforcementReason, ReasonCode, ToolSubject,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

// ---- helpers ----

fn out(v: Value) -> AdapterInput {
    AdapterInput::Stdout {
        line: v.to_string(),
    }
}

fn stderr(line: &str) -> AdapterInput {
    AdapterInput::Stderr { line: line.into() }
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

fn sha(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn command(command: SessionCommand) -> AdapterInput {
    AdapterInput::Command { command }
}

fn exit(code: Option<i32>) -> AdapterInput {
    AdapterInput::Exit { code }
}

fn deadline(what: &str) -> AdapterInput {
    AdapterInput::Deadline { what: what.into() }
}

fn ticket(id: &str, rememberable: bool) -> ApprovalTicket {
    ApprovalTicket::new(id.into(), rememberable)
}

/// Session started, turn 1 open, the user's message emitted.
fn started() -> (ClaudeNormalizer, Vec<AgentEvent>) {
    let mut n = ClaudeNormalizer::new();
    let evs = run(&mut n, vec![user_line("go"), init(), message_start()]);
    assert!(
        matches!(&evs[..],
            [
                AgentEvent::SessionStarted { provider: Provider::Claude, native_session_id, .. },
                AgentEvent::TurnStarted { turn: 1 },
                AgentEvent::MessageCompleted { kind: MessageKind::User, body: MessageBody::Inline { text }, .. },
            ] if native_session_id == "sess-1" && text == "go"),
        "{evs:?}"
    );
    (n, evs)
}

fn no_unmapped_or_session_events(evs: &[AgentEvent]) {
    assert!(
        !evs.iter().any(|e| matches!(
            e,
            AgentEvent::Unmapped { .. } | AgentEvent::SessionStarted { .. }
        )),
        "{evs:?}"
    );
}

fn is_session_ended(e: &AgentEvent) -> bool {
    matches!(e, AgentEvent::SessionEnded { .. })
}

// ---- messages ----

#[test]
fn todo_write_is_a_tool_call_and_a_plan() {
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
    assert_eq!(evs.len(), 3, "{evs:?}");
    assert!(matches!(&evs[0], AgentEvent::ToolCallStarted { name, .. } if name == "TodoWrite"));
    let AgentEvent::PlanUpdated { entries } = &evs[1] else {
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
    assert!(matches!(
        &evs[2],
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Succeeded,
            ..
        }
    ));
}

#[test]
fn streamed_text_completes_with_chars_deltas_and_sha() {
    let (mut n, _) = started();
    let delta = |text: &str| {
        out(
            json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}},"api_message_id":"m1"}),
        )
    };
    let evs = run(
        &mut n,
        vec![
            delta("héllo "),
            delta("wörld"),
            assistant(json!([{"type":"text","text":"héllo wörld"}])),
        ],
    );
    assert!(matches!(
        &evs[0],
        AgentEvent::MessageDelta { kind: MessageKind::Assistant, text, message_id } if text == "héllo " && message_id == "m1:0"
    ));
    let AgentEvent::MessageCompleted {
        message_id,
        kind: MessageKind::Assistant,
        body:
            MessageBody::Streamed {
                chars,
                deltas,
                sha256,
            },
    } = evs.last().unwrap()
    else {
        panic!("{evs:?}")
    };
    assert_eq!(message_id, "m1:0");
    // Unicode scalar values, not bytes.
    assert_eq!(*chars, "héllo wörld".chars().count() as u64);
    assert_eq!(*deltas, 2);
    assert_eq!(sha256, &sha("héllo wörld"));
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
        AgentEvent::MessageCompleted { kind: MessageKind::Reasoning, message_id, body: MessageBody::Streamed { chars: 6, deltas: 2, sha256 } }
            if message_id == "m1:0" && *sha256 == sha("hmm ok")
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
fn a_completion_only_message_is_inline() {
    let (mut n, _) = started();
    let evs = run(
        &mut n,
        vec![assistant(json!([{"type":"text","text":"short answer"}]))],
    );
    assert!(
        matches!(&evs[..], [AgentEvent::MessageCompleted { kind: MessageKind::Assistant, body: MessageBody::Inline { text }, .. }] if text == "short answer"),
        "{evs:?}"
    );
}

#[test]
fn a_long_completion_only_message_is_split_into_deltas() {
    let (mut n, _) = started();
    // Two-byte characters, so the split lands on a char boundary the byte
    // limit would cut through if the splitter were naive.
    let text = "é".repeat(MAX_INLINE_MESSAGE_BYTES / 2 + 10);
    assert!(text.len() > MAX_INLINE_MESSAGE_BYTES);
    let evs = run(
        &mut n,
        vec![assistant(json!([{"type":"text","text":text}]))],
    );
    let mut rebuilt = String::new();
    let mut deltas = 0;
    for e in &evs {
        if let AgentEvent::MessageDelta {
            text: piece,
            message_id,
            kind: MessageKind::Assistant,
        } = e
        {
            assert!(piece.len() <= MAX_INLINE_MESSAGE_BYTES);
            assert_eq!(message_id, "m1:0");
            rebuilt.push_str(piece);
            deltas += 1;
        }
    }
    assert_eq!(rebuilt, text);
    assert!(deltas >= 2);
    assert!(matches!(
        evs.last().unwrap(),
        AgentEvent::MessageCompleted { body: MessageBody::Streamed { chars, deltas: d, sha256 }, .. }
            if *chars == text.chars().count() as u64 && *d == deltas && *sha256 == sha(&text)
    ));
}

#[test]
fn the_user_message_follows_turn_started_and_long_ones_are_split() {
    let mut n = ClaudeNormalizer::new();
    let text = "ü".repeat(MAX_INLINE_MESSAGE_BYTES / 2 + 5);
    let evs = run(&mut n, vec![user_line(&text), init(), message_start()]);
    assert!(matches!(evs[0], AgentEvent::SessionStarted { .. }));
    assert!(matches!(evs[1], AgentEvent::TurnStarted { turn: 1 }));
    assert!(
        matches!(
            &evs[2],
            AgentEvent::MessageDelta {
                kind: MessageKind::User,
                ..
            }
        ),
        "{evs:?}"
    );
    assert!(matches!(
        evs.last().unwrap(),
        AgentEvent::MessageCompleted { kind: MessageKind::User, body: MessageBody::Streamed { sha256, .. }, message_id }
            if *sha256 == sha(&text) && message_id == "user:1"
    ));
}

#[test]
fn the_second_turn_gets_its_own_user_message() {
    let (mut n, _) = started();
    run(&mut n, vec![result(json!({}))]);
    let evs = run(&mut n, vec![user_line("again"), message_start()]);
    assert!(
        matches!(
            &evs[..],
            [
                AgentEvent::TurnStarted { turn: 2 },
                AgentEvent::MessageCompleted { kind: MessageKind::User, body: MessageBody::Inline { text }, message_id }
            ] if text == "again" && message_id == "user:2"
        ),
        "{evs:?}"
    );
    assert_eq!(n.turn_number(), 2);
}

// ---- quota and limits ----

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
            AgentEvent::MessageCompleted {
                kind: MessageKind::User,
                ..
            } => "user",
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
    assert_eq!(
        kinds,
        ["started", "turn", "user", "limited", "usage", "failed"]
    );
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

// ---- usage and context ----

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

fn result_models(total: f64, input: u64, output: u64, thinking: u64) -> AdapterInput {
    result(json!({
        "total_cost_usd": total,
        "usage": {"input_tokens": 1, "output_tokens": 1},
        "modelUsage": {"m": {"inputTokens": input, "outputTokens": output, "cacheReadInputTokens": 0,
            "cacheCreationInputTokens": 0, "thinkingTokens": thinking}}
    }))
}

fn turn_usage(evs: &[AgentEvent]) -> Usage {
    evs.iter()
        .find_map(|e| match e {
            AgentEvent::TurnCompleted { usage: Some(u), .. } => Some(u.clone()),
            _ => None,
        })
        .unwrap()
}

#[test]
fn turn_usage_is_the_model_usage_delta_and_survives_a_new_process() {
    let (mut n, _) = started();
    // Subagent tokens are in modelUsage but not in result.usage.
    let u1 = turn_usage(&run(&mut n, vec![result_models(1.0, 100, 50, 10)]));
    assert_eq!(
        (u1.input_tokens, u1.output_tokens, u1.reasoning_tokens),
        (Some(100), Some(50), Some(10))
    );
    assert_eq!(u1.cost_usd, Some(1.0));
    let evs = run(
        &mut n,
        vec![
            user_line("2"),
            message_start(),
            result_models(1.5, 130, 70, 15),
        ],
    );
    let u2 = turn_usage(&evs);
    assert_eq!(
        (u2.input_tokens, u2.output_tokens, u2.reasoning_tokens),
        (Some(30), Some(20), Some(5))
    );
    assert_eq!(u2.cost_usd, Some(0.5));
    // A resumed process counts from zero again: totals went down.
    let evs = run(
        &mut n,
        vec![
            user_line("3"),
            message_start(),
            result_models(0.25, 10, 5, 1),
        ],
    );
    let u3 = turn_usage(&evs);
    assert_eq!((u3.input_tokens, u3.output_tokens), (Some(10), Some(5)));
    assert_eq!(u3.cost_usd, Some(0.25));
}

fn message_delta(id: &str, parent: Value, usage: Value) -> AdapterInput {
    out(
        json!({"type":"stream_event","api_message_id":id,"parent_tool_use_id":parent,
        "event":{"type":"message_delta","usage":usage}}),
    )
}

#[test]
fn running_usage_counts_reasoning_and_subagent_messages() {
    let (mut n, _) = started();
    let delta = |id: &str, parent: Value, out_tokens: u64, think: u64| {
        message_delta(
            id,
            parent,
            json!({"input_tokens":1,"output_tokens":out_tokens,
            "output_tokens_details":{"thinking_tokens":think}}),
        )
    };
    let evs = run(
        &mut n,
        vec![
            delta("a", Value::Null, 10, 4),
            delta("b", json!("toolu_x"), 5, 2),
        ],
    );
    let AgentEvent::Usage { usage } = evs.last().unwrap() else {
        panic!("{evs:?}")
    };
    assert_eq!(usage.output_tokens, Some(15));
    assert_eq!(usage.reasoning_tokens, Some(6));
}

#[test]
fn context_is_reported_from_the_main_thread_and_never_as_usage() {
    let (mut n, _) = started();
    run(
        &mut n,
        vec![
            message_delta(
                "a",
                Value::Null,
                json!({"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":100,"cache_creation_input_tokens":20}),
            ),
            // A subagent's messages fill its own context, not the thread's.
            message_delta(
                "b",
                json!("toolu_x"),
                json!({"input_tokens":9000,"output_tokens":9000}),
            ),
        ],
    );
    let evs = run(
        &mut n,
        vec![result(json!({"modelUsage":{"claude-x":{
            "inputTokens":10,"outputTokens":5,"cacheReadInputTokens":100,
            "cacheCreationInputTokens":20,"contextWindow":200000}}}))],
    );
    let kinds: Vec<&str> = evs
        .iter()
        .map(|e| match e {
            AgentEvent::ContextUpdated { .. } => "context",
            AgentEvent::Usage { .. } => "usage",
            AgentEvent::TurnCompleted { .. } => "turn",
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(kinds, ["context", "usage", "turn"]);
    assert!(matches!(
        &evs[0],
        AgentEvent::ContextUpdated {
            used_tokens: 135,
            window_tokens: Some(200_000)
        }
    ));
    // A turn with no model call has no context figure.
    let evs = run(
        &mut n,
        vec![user_line("again"), message_start(), result(json!({}))],
    );
    assert!(
        !evs.iter()
            .any(|e| matches!(e, AgentEvent::ContextUpdated { .. })),
        "{evs:?}"
    );
}

#[test]
fn context_without_a_window_figure_has_none() {
    let (mut n, _) = started();
    run(
        &mut n,
        vec![message_delta(
            "a",
            Value::Null,
            json!({"input_tokens":7,"output_tokens":3}),
        )],
    );
    let evs = run(&mut n, vec![result(json!({}))]);
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::ContextUpdated {
            used_tokens: 10,
            window_tokens: None
        }
    )));
}

// ---- approvals ----

fn can_use(rid: &str, tool_use_id: &str) -> AdapterInput {
    out(json!({"type":"control_request","request_id":rid,"request":{
        "subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"},
        "tool_use_id":tool_use_id,"decision_reason":"","description":"List files"}}))
}

fn bash_call(id: &str) -> AdapterInput {
    assistant(json!([{"type":"tool_use","id":id,"name":"Bash","input":{"command":"ls"}}]))
}

fn check(id: &str) -> ToolCheck {
    tool_check(Some(id), "Bash", &json!({"command":"ls"}), 1)
}

fn gate(id: &str, d: GateDecision) -> AdapterInput {
    AdapterInput::Gate {
        check: check(id),
        decision: d,
    }
}

fn ask(ticket_id: &str) -> GateDecision {
    GateDecision::Ask {
        ticket: ticket(ticket_id, true),
    }
}

fn resolve(rid: &str, d: ApprovalDecision) -> AdapterInput {
    command(SessionCommand::ResolveApproval {
        request_id: rid.into(),
        decision: d,
    })
}

#[test]
fn can_use_tool_waits_for_the_gate_and_only_ask_reaches_the_client() {
    // Not preceded by a hook ask: the gate decides afterwards.
    let (mut n, _) = started();
    let evs = run(&mut n, vec![bash_call("t1"), can_use("r1", "t1")]);
    assert_eq!(evs.len(), 1);
    assert!(matches!(evs[0], AgentEvent::ToolCallStarted { .. }));
    let evs = run(&mut n, vec![gate("t1", ask("T1"))]);
    // The ticket's id is the request id, never Claude's own `r1`.
    assert!(
        matches!(
            &evs[0],
            AgentEvent::ApprovalRequested { request_id, call_id: Some(c), kind: ToolKind::Execute, rememberable: true, .. }
                if request_id == "T1" && c == "t1"
        ),
        "{evs:?}"
    );

    // The ticket's `rememberable` is copied as it is.
    let (mut n, _) = started();
    run(&mut n, vec![bash_call("t1"), can_use("r1", "t1")]);
    let evs = run(
        &mut n,
        vec![gate(
            "t1",
            GateDecision::Ask {
                ticket: ticket("T9", false),
            },
        )],
    );
    assert!(matches!(
        &evs[0],
        AgentEvent::ApprovalRequested {
            rememberable: false,
            ..
        }
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
        vec![bash_call("t1"), gate("t1", ask("T1")), can_use("r1", "t1")],
    );
    assert!(matches!(
        evs.last().unwrap(),
        AgentEvent::ApprovalRequested { request_id, .. } if request_id == "T1"
    ));
}

#[test]
fn resolving_twice_or_unknown_is_a_non_fatal_error() {
    let (mut n, _) = started();
    run(
        &mut n,
        vec![bash_call("t1"), gate("t1", ask("T1")), can_use("r1", "t1")],
    );
    // Claude's own request id is not a handle the client can use.
    let evs = run(&mut n, vec![resolve("r1", ApprovalDecision::Allow)]);
    assert!(
        matches!(&evs[..], [AgentEvent::Error { fatal: false, .. }]),
        "{evs:?}"
    );
    let evs = run(&mut n, vec![resolve("T1", ApprovalDecision::Allow)]);
    assert!(matches!(
        &evs[..],
        [AgentEvent::RequestResolved {
            request_id,
            outcome: RequestOutcome::Approved,
        }] if request_id == "T1"
    ));
    for rid in ["T1", "nope"] {
        let evs = run(&mut n, vec![resolve(rid, ApprovalDecision::Allow)]);
        assert!(
            matches!(&evs[..], [AgentEvent::Error { fatal: false, details, .. }] if details.is_empty()),
            "{evs:?}"
        );
    }
    // Answering an approval as if it were a question is rejected too.
    let (mut n, _) = started();
    run(
        &mut n,
        vec![bash_call("t1"), gate("t1", ask("T1")), can_use("r1", "t1")],
    );
    let evs = run(
        &mut n,
        vec![command(SessionCommand::AnswerQuestion {
            request_id: "T1".into(),
            answers: vec![],
        })],
    );
    assert!(matches!(&evs[..], [AgentEvent::Error { fatal: false, .. }]));
    // The approval is still open after the wrong command.
    let evs = run(&mut n, vec![resolve("T1", ApprovalDecision::Allow)]);
    assert!(matches!(
        &evs[..],
        [AgentEvent::RequestResolved {
            outcome: RequestOutcome::Approved,
            ..
        }]
    ));
}

#[test]
fn denying_an_approval_denies_the_call_and_swallows_the_provider_result() {
    let (mut n, _) = started();
    run(
        &mut n,
        vec![bash_call("t1"), gate("t1", ask("T1")), can_use("r1", "t1")],
    );
    let evs = run(
        &mut n,
        vec![resolve(
            "T1",
            ApprovalDecision::Deny {
                message: Some("no".into()),
            },
        )],
    );
    assert!(matches!(
        &evs[..],
        [
            AgentEvent::RequestResolved { outcome: RequestOutcome::Denied, request_id },
            AgentEvent::ToolCallFinished { status: ToolStatus::Denied, error: Some(e), .. },
        ] if e == "no" && request_id == "T1"
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
fn a_redeem_the_gate_refuses_denies_the_call() {
    let (mut n, _) = started();
    run(
        &mut n,
        vec![bash_call("t1"), gate("t1", ask("T1")), can_use("r1", "t1")],
    );
    run(&mut n, vec![resolve("T1", ApprovalDecision::Allow)]);
    // The user allowed it, the gate refuses at the moment of the effect.
    let evs = run(
        &mut n,
        vec![AdapterInput::Redeem {
            ticket: ticket("T1", true),
            check: check("t1"),
            decision: GateDecision::Deny {
                reason: "the input changed".into(),
            },
        }],
    );
    assert!(
        matches!(
            &evs[..],
            [AgentEvent::ToolCallFinished { status: ToolStatus::Denied, error: Some(e), .. }]
                if e == "the input changed"
        ),
        "{evs:?}"
    );

    // A granted redeem changes nothing for the client.
    let (mut n, _) = started();
    run(
        &mut n,
        vec![bash_call("t1"), gate("t1", ask("T1")), can_use("r1", "t1")],
    );
    run(&mut n, vec![resolve("T1", ApprovalDecision::Allow)]);
    let evs = run(
        &mut n,
        vec![AdapterInput::Redeem {
            ticket: ticket("T1", true),
            check: check("t1"),
            decision: GateDecision::Allow,
        }],
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
fn provider_cancelled_request_is_cancelled_under_the_ticket_id() {
    let (mut n, _) = started();
    run(
        &mut n,
        vec![bash_call("t1"), gate("t1", ask("T1")), can_use("r1", "t1")],
    );
    let evs = run(
        &mut n,
        vec![out(
            json!({"type":"control_cancel_request","request_id":"r1"}),
        )],
    );
    assert!(
        matches!(
            &evs[..],
            [AgentEvent::RequestResolved {
                request_id,
                outcome: RequestOutcome::Cancelled,
            }] if request_id == "T1"
        ),
        "{evs:?}"
    );
    // Resolved by the provider: the client's late answer is an unknown request.
    let evs = run(&mut n, vec![resolve("T1", ApprovalDecision::Allow)]);
    assert!(matches!(&evs[..], [AgentEvent::Error { fatal: false, .. }]));
}

fn can_use_anon(rid: &str, name: &str, input: Value) -> AdapterInput {
    out(json!({"type":"control_request","request_id":rid,"request":{
        "subtype":"can_use_tool","tool_name":name,"input":input}}))
}

fn gate_anon(name: &str, input: Value, d: GateDecision) -> AdapterInput {
    AdapterInput::Gate {
        check: tool_check(None, name, &input, 1),
        decision: d,
    }
}

#[test]
fn can_use_tool_without_an_id_is_matched_by_tool_and_input() {
    let (mut n, _) = started();
    let input = json!({"command":"ls"});
    let evs = run(&mut n, vec![can_use_anon("r1", "Bash", input.clone())]);
    assert!(evs.is_empty(), "{evs:?}");
    let evs = run(&mut n, vec![gate_anon("Bash", input, ask("T1"))]);
    assert!(matches!(
        &evs[..],
        [AgentEvent::ApprovalRequested { request_id, call_id: None, .. }] if request_id == "T1"
    ));
    // The other order (the gate answered first) works too.
    let (mut n, _) = started();
    let input = json!({"command":"pwd"});
    run(&mut n, vec![gate_anon("Bash", input.clone(), ask("T2"))]);
    let evs = run(&mut n, vec![can_use_anon("r2", "Bash", input)]);
    assert!(
        matches!(&evs[..], [AgentEvent::ApprovalRequested { request_id, .. }] if request_id == "T2")
    );
}

#[test]
fn an_approval_opens_the_call_it_refers_to() {
    // A subagent's call can ask before its tool_use record has arrived.
    let (mut n, _) = started();
    let evs = run(&mut n, vec![gate("sub1", ask("T1")), can_use("r1", "sub1")]);
    assert!(matches!(
        &evs[..],
        [
            AgentEvent::ToolCallStarted { call_id, .. },
            AgentEvent::ApprovalRequested { .. }
        ] if call_id == "sub1"
    ));
    // The late tool_use record does not start it twice.
    let evs = run(&mut n, vec![bash_call("sub1")]);
    assert!(evs.is_empty(), "{evs:?}");
}

#[test]
fn the_turn_end_cancels_requests_still_open() {
    let (mut n, _) = started();
    run(
        &mut n,
        vec![bash_call("t1"), gate("t1", ask("T1")), can_use("r1", "t1")],
    );
    let evs = run(&mut n, vec![result(json!({}))]);
    let kinds: Vec<&str> = evs
        .iter()
        .map(|e| match e {
            AgentEvent::ToolCallFinished {
                status: ToolStatus::Cancelled,
                ..
            } => "call",
            AgentEvent::RequestResolved {
                request_id,
                outcome: RequestOutcome::Cancelled,
            } if request_id == "T1" => "request",
            AgentEvent::Usage { .. } => "usage",
            AgentEvent::TurnCompleted { .. } => "turn",
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(kinds, ["call", "request", "usage", "turn"]);
    // Resolving it afterwards is an unknown request.
    let evs = run(&mut n, vec![resolve("T1", ApprovalDecision::Allow)]);
    assert!(matches!(&evs[..], [AgentEvent::Error { fatal: false, .. }]));
}

// ---- questions ----

fn ask_question(rid: &str, tool_use_id: &str) -> AdapterInput {
    out(json!({"type":"control_request","request_id":rid,"request":{
        "subtype":"can_use_tool","tool_name":"AskUserQuestion","tool_use_id":tool_use_id,
        "input":{"questions":[{"question":"Q?","options":[{"label":"A"}]}]}}}))
}

fn question_ticket(native: &str, t: Option<ApprovalTicket>) -> AdapterInput {
    AdapterInput::QuestionTicket {
        id: native.into(),
        ticket: t,
    }
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
fn a_question_is_shown_only_with_the_gates_ticket() {
    let (mut n, _) = started();
    // Nothing is shown until the gate has opened a ledger entry for it.
    let evs = run(&mut n, vec![ask_question("r1", "q1")]);
    assert!(evs.is_empty(), "{evs:?}");
    let evs = run(
        &mut n,
        vec![question_ticket("r1", Some(ticket("Q1", false)))],
    );
    assert!(
        matches!(&evs[..], [AgentEvent::QuestionAsked { request_id, questions }]
            if request_id == "Q1" && questions[0].id == "q0"),
        "{evs:?}"
    );
    // The answer is addressed to the ticket, and resolves it once.
    let answer = |rid: &str| {
        command(SessionCommand::AnswerQuestion {
            request_id: rid.into(),
            answers: vec![],
        })
    };
    let evs = run(&mut n, vec![answer("r1")]);
    assert!(matches!(&evs[..], [AgentEvent::Error { fatal: false, .. }]));
    let evs = run(&mut n, vec![answer("Q1")]);
    assert!(matches!(
        &evs[..],
        [AgentEvent::RequestResolved { request_id, outcome: RequestOutcome::Answered }] if request_id == "Q1"
    ));
    let evs = run(&mut n, vec![answer("Q1")]);
    assert!(matches!(&evs[..], [AgentEvent::Error { fatal: false, .. }]));
}

#[test]
fn a_question_can_be_declined_without_ending_the_turn() {
    let (mut n, _) = started();
    run(&mut n, vec![ask_question("r1", "q1")]);
    let evs = run(
        &mut n,
        vec![question_ticket("r1", Some(ticket("Q1", false)))],
    );
    assert!(
        matches!(&evs[..], [AgentEvent::QuestionAsked { request_id, .. }] if request_id == "Q1"),
        "{evs:?}"
    );
    let evs = run(
        &mut n,
        vec![command(SessionCommand::DeclineQuestion {
            request_id: "Q1".into(),
        })],
    );
    assert!(
        matches!(
            &evs[..],
            [AgentEvent::RequestResolved {
                request_id,
                outcome: RequestOutcome::Cancelled,
            }] if request_id == "Q1"
        ),
        "{evs:?}"
    );
    let evs = run(
        &mut n,
        vec![command(SessionCommand::DeclineQuestion {
            request_id: "Q1".into(),
        })],
    );
    assert!(matches!(&evs[..], [AgentEvent::Error { fatal: false, .. }]));
}

#[test]
fn a_declined_question_is_never_shown() {
    let (mut n, _) = started();
    run(&mut n, vec![ask_question("r1", "q1")]);
    let evs = run(&mut n, vec![question_ticket("r1", None)]);
    assert!(evs.is_empty(), "{evs:?}");
    // Nothing is open for it.
    let evs = run(
        &mut n,
        vec![command(SessionCommand::AnswerQuestion {
            request_id: "Q1".into(),
            answers: vec![],
        })],
    );
    assert!(matches!(&evs[..], [AgentEvent::Error { fatal: false, .. }]));
}

#[test]
fn a_question_is_not_a_tool_call_and_a_gate_input_does_not_show_it() {
    let (mut n, _) = started();
    let evs = run(
        &mut n,
        vec![
            assistant(
                json!([{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{"questions":[]}}]),
            ),
            ask_question("r1", "q1"),
            // A question can't be approved: a stray `Ask` is not shown.
            AdapterInput::Gate {
                check: tool_check(Some("q1"), "AskUserQuestion", &json!({}), 1),
                decision: ask("T1"),
            },
        ],
    );
    assert!(evs.is_empty(), "{evs:?}");
    // The provider's result for it is not a tool call either.
    let evs = run(
        &mut n,
        vec![out(
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"q1","content":"answered"}]}}),
        )],
    );
    assert!(evs.is_empty(), "{evs:?}");
}

#[test]
fn the_turn_end_cancels_a_question_still_open() {
    let (mut n, _) = started();
    run(
        &mut n,
        vec![
            ask_question("r1", "q1"),
            question_ticket("r1", Some(ticket("Q1", false))),
        ],
    );
    let evs = run(&mut n, vec![result(json!({}))]);
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::RequestResolved { request_id, outcome: RequestOutcome::Cancelled } if request_id == "Q1"
    )));
}

// ---- unmapped records ----

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
fn unmapped_raw_is_cut_to_the_limit() {
    let (mut n, _) = started();
    let evs = run(
        &mut n,
        vec![out(
            json!({"type":"brand_new_thing","pad":"é".repeat(MAX_UNMAPPED_RAW_BYTES)}),
        )],
    );
    let [AgentEvent::Unmapped { raw, .. }] = &evs[..] else {
        panic!("{evs:?}")
    };
    // Replaced by a JSON string of the truncated text.
    let Value::String(text) = raw else {
        panic!("{raw:?}")
    };
    assert!(text.len() <= MAX_UNMAPPED_RAW_BYTES);
    assert!(text.len() > MAX_UNMAPPED_RAW_BYTES - 4);
    // A small record is kept as it is.
    let evs = run(&mut n, vec![out(json!({"type":"brand_new_thing","x":1}))]);
    assert!(matches!(&evs[..], [AgentEvent::Unmapped { raw, .. }] if raw["x"] == 1));
}

#[test]
fn notes_about_unmapped_records_never_precede_session_started() {
    let mut n = ClaudeNormalizer::new();
    let evs = run(
        &mut n,
        vec![
            user_line("go"),
            out(json!({"type":"system","subtype":"hook_started"})),
            init(),
            message_start(),
        ],
    );
    assert!(
        matches!(evs[0], AgentEvent::SessionStarted { .. }),
        "{evs:?}"
    );
    assert!(matches!(evs[1], AgentEvent::TurnStarted { .. }));
    assert!(matches!(evs[2], AgentEvent::MessageCompleted { .. }));
    assert!(
        matches!(&evs[3], AgentEvent::Unmapped { record_type, .. } if record_type == "system/hook_started"),
        "{evs:?}"
    );
}

// ---- pre-session failures: details, no Unmapped ----

#[test]
fn a_failed_start_puts_its_diagnostics_in_error_details() {
    let mut n = ClaudeNormalizer::new();
    // Everything before the exit is silent: no Unmapped, no SessionStarted.
    let evs = run(
        &mut n,
        vec![
            user_line("go"),
            stderr("claude: warming up"),
            out(json!({"type":"system","subtype":"hook_started","hook":"x"})),
            out(json!({"type":"brand_new_thing","x":1})),
            AdapterInput::Stdout {
                line: "not json".into(),
            },
            stderr("fatal: no such model"),
            out(
                json!({"type":"assistant","error":"invalid_request","message":{"model":"<synthetic>","id":"x","content":[{"type":"text","text":"There's an issue with the selected model."}]}}),
            ),
            result(
                json!({"is_error":true,"terminal_reason":"api_error","result":"There's an issue with the selected model."}),
            ),
        ],
    );
    assert!(evs.is_empty(), "{evs:?}");
    assert!(n.pre_session_failed());
    assert!(!n.is_established());
    let evs = run(&mut n, vec![exit(Some(1))]);
    let [
        AgentEvent::Error {
            message,
            fatal: true,
            details,
        },
        AgentEvent::SessionEnded { exit_code, reason },
    ] = &evs[..]
    else {
        panic!("{evs:?}")
    };
    assert_eq!(message, "There's an issue with the selected model.");
    assert_eq!(*exit_code, Some(1));
    assert_eq!(reason.as_deref(), Some("provider error"));
    assert!(details.iter().any(|l| l == "claude: warming up"));
    assert!(details.iter().any(|l| l == "fatal: no such model"));
    // The records Claude sent that we don't map are diagnostics too.
    for want in ["system/hook_started", "brand_new_thing", "unparseable"] {
        assert!(
            details.iter().any(|l| l.contains(want)),
            "{want}: {details:?}"
        );
    }
    assert!(details.iter().any(|l| l.contains("not json")));
    no_unmapped_or_session_events(&evs);
}

#[test]
fn a_start_that_fails_with_a_result_error_uses_its_message() {
    let mut n = ClaudeNormalizer::new();
    let evs = run(
        &mut n,
        vec![
            user_line("go"),
            init(),
            stderr("some stderr"),
            result(
                json!({"is_error":true,"terminal_reason":"api_error","result":"Invalid API key"}),
            ),
            exit(Some(1)),
        ],
    );
    let [
        AgentEvent::Error {
            message,
            fatal: true,
            details,
        },
        end,
    ] = &evs[..]
    else {
        panic!("{evs:?}")
    };
    assert_eq!(message, "Invalid API key");
    assert_eq!(details, &["some stderr".to_owned()]);
    assert!(is_session_ended(end));
}

#[test]
fn a_process_that_exits_before_answering_is_a_pre_session_failure() {
    let mut n = ClaudeNormalizer::new();
    let evs = run(&mut n, vec![user_line("go"), stderr("boom"), exit(Some(2))]);
    let [
        AgentEvent::Error {
            message,
            fatal: true,
            details,
        },
        AgentEvent::SessionEnded {
            exit_code: Some(2), ..
        },
    ] = &evs[..]
    else {
        panic!("{evs:?}")
    };
    assert!(message.contains("code 2"), "{message}");
    assert_eq!(details, &["boom".to_owned()]);
}

#[test]
fn diagnostics_are_bounded_in_lines_and_bytes_and_clean_of_control_characters() {
    let mut n = ClaudeNormalizer::new();
    let mut inputs = vec![user_line("go")];
    inputs.push(stderr(&"é".repeat(MAX_DIAGNOSTIC_LINE_BYTES))); // 2x the limit
    inputs.push(stderr("bell\u{7}and\u{1b}[31mred"));
    for i in 0..(MAX_DIAGNOSTIC_LINES + 10) {
        inputs.push(stderr(&format!("line {i}")));
    }
    inputs.push(exit(Some(1)));
    let evs = run(&mut n, inputs);
    let [AgentEvent::Error { details, .. }, _] = &evs[..] else {
        panic!("{evs:?}")
    };
    assert_eq!(details.len(), MAX_DIAGNOSTIC_LINES);
    assert_eq!(
        details.last().unwrap(),
        &format!("line {}", MAX_DIAGNOSTIC_LINES + 9)
    );
    // The two long/odd lines were pushed out by the newer ones; test them on
    // their own.
    let mut n = ClaudeNormalizer::new();
    let evs = run(
        &mut n,
        vec![
            user_line("go"),
            stderr(&"é".repeat(MAX_DIAGNOSTIC_LINE_BYTES)),
            stderr("bell\u{7}and\u{1b}[31mred"),
            exit(Some(1)),
        ],
    );
    let [AgentEvent::Error { details, .. }, _] = &evs[..] else {
        panic!("{evs:?}")
    };
    assert_eq!(details.len(), 2);
    assert!(details[0].len() <= MAX_DIAGNOSTIC_LINE_BYTES);
    assert!(details[0].len() > MAX_DIAGNOSTIC_LINE_BYTES - 2);
    assert!(details[0].chars().all(|c| c == 'é'));
    assert!(
        !details[1].chars().any(|c| c.is_control()),
        "{:?}",
        details[1]
    );
    assert!(details[1].contains("bell"));
}

#[test]
fn details_never_exceed_the_line_limit_with_unmapped_records_mixed_in() {
    let mut n = ClaudeNormalizer::new();
    let mut inputs = vec![user_line("go")];
    for i in 0..(MAX_DIAGNOSTIC_LINES * 2) {
        inputs.push(out(
            json!({"type":format!("weird_{i}"),"pad":"y".repeat(5000)}),
        ));
    }
    inputs.push(exit(Some(1)));
    let evs = run(&mut n, inputs);
    let [AgentEvent::Error { details, .. }, _] = &evs[..] else {
        panic!("{evs:?}")
    };
    assert_eq!(details.len(), MAX_DIAGNOSTIC_LINES);
    assert!(details.iter().all(|l| l.len() <= MAX_DIAGNOSTIC_LINE_BYTES));
}

#[test]
fn a_mid_turn_exit_carries_the_stderr_tail_in_the_fatal_error() {
    let (mut n, _) = started();
    let evs = run(&mut n, vec![stderr("segfault"), exit(Some(139))]);
    let Some(AgentEvent::Error {
        fatal: true,
        details,
        message,
    }) = evs.iter().find(|e| matches!(e, AgentEvent::Error { .. }))
    else {
        panic!("{evs:?}")
    };
    assert!(message.contains("during a turn"));
    assert_eq!(details, &["segfault".to_owned()]);
}

// ---- tools: kinds and subjects ----

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
    let c: ToolCheck = tool_check(Some("t"), "Bash", &json!({"command":"ls -la"}), 3);
    assert_eq!(c.title.as_deref(), Some("ls -la"));
    assert_eq!(c.kind, ToolKind::Execute);
    assert_eq!(c.turn, 3);
    assert_eq!(c.call_id.as_deref(), Some("t"));
}

#[test]
fn every_check_carries_the_extracted_subject() {
    let subject = |name: &str, input: Value| tool_check(Some("t"), name, &input, 1).subject;
    let paths = |p: &str, glob: Option<&str>, recursive| ToolSubject::Paths {
        paths: vec![p.into()],
        glob: glob.map(str::to_owned),
        recursive,
    };
    assert_eq!(
        subject("Bash", json!({"command":"rm -rf x"})),
        ToolSubject::Command {
            line: "rm -rf x".into(),
            cwd: None
        }
    );
    assert_eq!(
        subject("Read", json!({"file_path":"/a/b"})),
        paths("/a/b", None, false)
    );
    assert_eq!(
        subject("Edit", json!({"file_path":"/a/b","old_string":"x"})),
        paths("/a/b", None, false)
    );
    assert_eq!(
        subject("Write", json!({"file_path":"/a/c"})),
        paths("/a/c", None, false)
    );
    assert_eq!(
        subject("NotebookEdit", json!({"notebook_path":"/a/n.ipynb"})),
        paths("/a/n.ipynb", None, false)
    );
    assert_eq!(
        subject("Grep", json!({"pattern":"foo","path":"src","glob":"*.rs"})),
        paths("src", Some("*.rs"), true)
    );
    assert_eq!(
        subject("Grep", json!({"pattern":"foo"})),
        paths(".", None, true)
    );
    // A pattern with a `/` is also joined to the base, `.` components
    // dropped (escalation, D-071).
    assert_eq!(
        subject("Glob", json!({"pattern":"**/*.rs"})),
        ToolSubject::Paths {
            paths: vec![".".into(), "**/*.rs".into()],
            glob: Some("**/*.rs".into()),
            recursive: true,
        }
    );
    assert_eq!(
        subject("WebFetch", json!({"url":"https://example.com/x"})),
        ToolSubject::Fetch {
            url: "https://example.com/x".into()
        }
    );
    assert_eq!(
        subject("mcp__echo__say__twice", json!({})),
        ToolSubject::Mcp {
            server: "echo".into(),
            tool: "say__twice".into()
        }
    );
    // Anything it can't extract is Unknown, which the policy asks about.
    assert_eq!(
        subject("Agent", json!({"description":"d"})),
        ToolSubject::Unknown
    );
    assert_eq!(subject("Bash", json!({})), ToolSubject::Unknown);
    assert_eq!(
        subject("Read", json!({"file_path":""})),
        ToolSubject::Unknown
    );
    assert_eq!(subject("mcp__", json!({})), ToolSubject::Unknown);
    assert_eq!(subject("WebFetch", json!({"url":5})), ToolSubject::Unknown);
}

// ---- lifecycle ----

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
            gate("t1", ask("T1")),
            can_use("r1", "t1"),
        ],
    );
    let evs = run(&mut n, vec![exit(Some(137))]);
    let kinds: Vec<&str> = evs
        .iter()
        .map(|e| match e {
            AgentEvent::MessageCompleted {
                body:
                    MessageBody::Streamed {
                        chars: 3,
                        deltas: 1,
                        sha256,
                    },
                ..
            } if *sha256 == sha("par") => "message",
            AgentEvent::ToolCallFinished {
                status: ToolStatus::Cancelled,
                ..
            } => "call",
            AgentEvent::RequestResolved {
                request_id,
                outcome: RequestOutcome::Cancelled,
            } if request_id == "T1" => "request",
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
    // The failure is reported first; then the contract's closing order
    // (messages, calls, requests, the turn) and `SessionEnded` last.
    assert_eq!(
        kinds,
        ["error", "message", "call", "request", "turn", "end"]
    );
}

#[test]
fn closing_a_session_that_never_had_a_turn_is_just_session_ended() {
    let mut n = ClaudeNormalizer::new();
    let evs = run(
        &mut n,
        vec![command(SessionCommand::Shutdown), exit(Some(0))],
    );
    assert!(matches!(&evs[..], [AgentEvent::SessionEnded { .. }]));
    assert!(n.has_ended());
    // Nothing more comes out once it has ended.
    assert!(run(&mut n, vec![exit(Some(0))]).is_empty());
}

#[test]
fn a_shutdown_mid_turn_ends_interrupted_with_the_reason_shutdown() {
    let (mut n, _) = started();
    let evs = run(
        &mut n,
        vec![command(SessionCommand::Shutdown), exit(Some(0))],
    );
    assert!(
        matches!(
            &evs[..],
            [
                AgentEvent::TurnCompleted {
                    outcome: TurnOutcome::Interrupted,
                    ..
                },
                AgentEvent::SessionEnded { reason: Some(r), .. }
            ] if r == "shutdown"
        ),
        "{evs:?}"
    );
}

#[test]
fn interrupt_that_the_cli_answers_with_a_result_is_interrupted() {
    let (mut n, _) = started();
    let evs = run(
        &mut n,
        vec![
            command(SessionCommand::Interrupt),
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
    // The session stays usable.
    assert!(!n.has_ended());
    assert!(!n.wants_end());
}

#[test]
fn an_interrupt_outside_a_turn_does_nothing() {
    let (mut n, _) = started();
    run(&mut n, vec![result(json!({}))]);
    assert!(run(&mut n, vec![command(SessionCommand::Interrupt)]).is_empty());
    // A later exit is not "after an interrupt".
    let evs = run(&mut n, vec![exit(Some(0))]);
    assert!(
        matches!(&evs[..], [AgentEvent::SessionEnded { reason: Some(r), .. }] if !r.contains("interrupt")),
        "{evs:?}"
    );
}

#[test]
fn a_cli_that_leaves_after_an_interrupt_ends_the_session() {
    // The adapter never respawns inside a session (rev 3): the process is
    // gone, so the session ends and the daemon resumes it in a new
    // incarnation.
    let (mut n, _) = started();
    let evs = run(&mut n, vec![command(SessionCommand::Interrupt), exit(None)]);
    assert!(
        matches!(
            &evs[..],
            [
                AgentEvent::TurnCompleted {
                    outcome: TurnOutcome::Interrupted,
                    ..
                },
                AgentEvent::SessionEnded {
                    reason: Some(_),
                    ..
                }
            ]
        ),
        "{evs:?}"
    );
    assert!(!evs.iter().any(|e| matches!(e, AgentEvent::Error { .. })));
    assert!(n.has_ended());
}

#[test]
fn an_interrupt_the_cli_never_honours_ends_the_session() {
    let (mut n, _) = started();
    run(
        &mut n,
        vec![
            bash_call("t1"),
            gate("t1", ask("T1")),
            can_use("r1", "t1"),
            command(SessionCommand::Interrupt),
        ],
    );
    assert!(!n.wants_end());
    let evs = run(&mut n, vec![deadline("interrupt")]);
    assert!(evs.is_empty(), "{evs:?}");
    assert!(n.wants_end(), "the adapter must end the session");
    // The adapter kills the process group; the exit closes everything.
    let evs = run(&mut n, vec![exit(Some(137))]);
    let kinds: Vec<&str> = evs
        .iter()
        .map(|e| match e {
            AgentEvent::ToolCallFinished {
                status: ToolStatus::Cancelled,
                ..
            } => "call",
            AgentEvent::RequestResolved {
                outcome: RequestOutcome::Cancelled,
                ..
            } => "request",
            AgentEvent::TurnCompleted {
                outcome: TurnOutcome::Interrupted,
                ..
            } => "turn",
            AgentEvent::SessionEnded {
                exit_code: Some(137),
                reason: Some(r),
            } if r.contains("interrupt") => "end",
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(kinds, ["call", "request", "turn", "end"]);
    assert!(!n.wants_end(), "ended");
}

#[test]
fn unknown_deadlines_change_nothing() {
    let (mut n, _) = started();
    for what in ["startup", "shutdown_grace", "hook", "something-new", ""] {
        let evs = run(&mut n, vec![deadline(what)]);
        assert!(evs.is_empty(), "{what}: {evs:?}");
        assert!(!n.wants_end(), "{what}");
    }
    // The turn carries on.
    let evs = run(&mut n, vec![result(json!({}))]);
    assert!(matches!(
        evs.last().unwrap(),
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Completed,
            ..
        }
    ));
}

// ---- limits ----

#[test]
fn a_record_over_the_limit_ends_an_established_session_with_a_fatal_error() {
    let (mut n, _) = started();
    let evs = run(
        &mut n,
        vec![AdapterInput::Stdout {
            line: "x".repeat(MAX_PROVIDER_RECORD_BYTES + 1),
        }],
    );
    assert!(
        matches!(&evs[..], [AgentEvent::Error { fatal: true, message, .. }] if message.contains("exceeds")),
        "{evs:?}"
    );
    assert!(n.wants_end());
    // A record at the limit is parsed as any other (this one isn't JSON).
    let (mut n, _) = started();
    let evs = run(
        &mut n,
        vec![AdapterInput::Stdout {
            line: "x".repeat(MAX_PROVIDER_RECORD_BYTES),
        }],
    );
    assert!(
        matches!(&evs[..], [AgentEvent::Unmapped { record_type, raw: Value::String(s) }]
            if record_type == "unparseable" && s.len() <= MAX_UNMAPPED_RAW_BYTES),
        "{}",
        evs.len()
    );
    assert!(!n.wants_end());
}

#[test]
fn the_adapters_oversize_deadline_does_the_same_once() {
    let (mut n, _) = started();
    let evs = run(&mut n, vec![deadline("record_too_large")]);
    assert!(
        matches!(&evs[..], [AgentEvent::Error { fatal: true, .. }]),
        "{evs:?}"
    );
    assert!(n.wants_end());
    // Both the line and the deadline can arrive: one Error, not two.
    let again = run(
        &mut n,
        vec![
            deadline("record_too_large"),
            AdapterInput::Stdout {
                line: "x".repeat(MAX_PROVIDER_RECORD_BYTES + 1),
            },
        ],
    );
    assert!(again.is_empty(), "{again:?}");
    let evs = run(&mut n, vec![exit(Some(137))]);
    // The exit was requested by the adapter: no second Error.
    assert!(
        !evs.iter().any(|e| matches!(e, AgentEvent::Error { .. })),
        "{evs:?}"
    );
    let Some(AgentEvent::SessionEnded {
        reason: Some(reason),
        ..
    }) = evs.last()
    else {
        panic!("{evs:?}")
    };
    assert!(reason.contains("exceeds"), "{reason}");
    // The turn was closed before the end.
    assert!(matches!(
        evs[evs.len() - 2],
        AgentEvent::TurnCompleted { .. }
    ));
}

#[test]
fn a_record_over_the_limit_before_the_session_starts_is_a_pre_session_failure() {
    let mut n = ClaudeNormalizer::new();
    let evs = run(
        &mut n,
        vec![
            user_line("go"),
            stderr("noisy"),
            AdapterInput::Stdout {
                line: "x".repeat(MAX_PROVIDER_RECORD_BYTES + 1),
            },
        ],
    );
    assert!(evs.is_empty(), "{evs:?}");
    assert!(n.wants_end());
    assert!(n.pre_session_failed());
    let evs = run(&mut n, vec![exit(Some(137))]);
    let [
        AgentEvent::Error {
            fatal: true,
            message,
            details,
        },
        AgentEvent::SessionEnded { .. },
    ] = &evs[..]
    else {
        panic!("{evs:?}")
    };
    assert!(message.contains("exceeds"));
    assert_eq!(details, &["noisy".to_owned()]);
}

fn big_bash(id: &str, size: usize) -> AdapterInput {
    assistant(
        json!([{"type":"tool_use","id":id,"name":"Bash","input":{"command":"x".repeat(size)}}]),
    )
}

#[test]
fn tool_arguments_over_the_cap_fail_the_call_and_are_never_shown_to_the_gate() {
    let (mut n, _) = started();
    // The record itself is under MAX_PROVIDER_RECORD_BYTES.
    let evs = run(&mut n, vec![big_bash("t1", MAX_PARTIAL_ARGS_BYTES)]);
    let [
        AgentEvent::ToolCallStarted {
            call_id,
            name,
            input,
            ..
        },
        AgentEvent::ToolCallFinished {
            call_id: done,
            status: ToolStatus::Failed,
            error: Some(error),
            output: None,
            ..
        },
    ] = &evs[..]
    else {
        panic!("{}", evs.len())
    };
    assert_eq!(
        (call_id.as_str(), done.as_str(), name.as_str()),
        ("t1", "t1", "Bash")
    );
    // The oversize input is not echoed back.
    assert_eq!(input, &json!({}));
    assert!(error.contains("exceed"), "{error}");
    assert!(!n.wants_end(), "only the call fails, not the session");

    // The provider's own result for it adds nothing, and an approval for it is
    // never offered.
    let evs = run(
        &mut n,
        vec![
            out(
                json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"denied","is_error":true}]}}),
            ),
            can_use("r1", "t1"),
        ],
    );
    assert!(evs.is_empty(), "{evs:?}");

    // Just under the cap is an ordinary call.
    let (mut n, _) = started();
    let evs = run(&mut n, vec![big_bash("t2", MAX_PARTIAL_ARGS_BYTES / 2)]);
    assert!(
        matches!(&evs[..], [AgentEvent::ToolCallStarted { .. }]),
        "{}",
        evs.len()
    );
}

fn tool_result(id: &str, content: &str, is_error: bool) -> AdapterInput {
    out(
        json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":id,"content":content,"is_error":is_error}]}}),
    )
}

#[test]
fn tool_output_over_the_cap_is_cut_and_counted() {
    let (mut n, _) = started();
    run(&mut n, vec![bash_call("t1")]);
    // Three-byte characters: the cap falls inside one.
    let text = "€".repeat(MAX_TOOL_OUTPUT_BYTES / 3 + 100);
    assert!(text.len() > MAX_TOOL_OUTPUT_BYTES);
    let evs = run(&mut n, vec![tool_result("t1", &text, false)]);
    let [
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Succeeded,
            output: Some(kept),
            truncated_bytes,
            ..
        },
    ] = &evs[..]
    else {
        panic!("{}", evs.len())
    };
    assert!(kept.len() <= MAX_TOOL_OUTPUT_BYTES);
    assert!(kept.len() > MAX_TOOL_OUTPUT_BYTES - 3);
    assert!(text.starts_with(kept.as_str()));
    assert_eq!(*truncated_bytes, (text.len() - kept.len()) as u64);
}

#[test]
fn tool_output_at_the_cap_is_kept_whole() {
    let (mut n, _) = started();
    run(&mut n, vec![bash_call("t1")]);
    let text = "y".repeat(MAX_TOOL_OUTPUT_BYTES);
    let evs = run(&mut n, vec![tool_result("t1", &text, false)]);
    assert!(
        matches!(&evs[..], [AgentEvent::ToolCallFinished { output: Some(o), truncated_bytes: 0, .. }] if o.len() == MAX_TOOL_OUTPUT_BYTES),
        "{}",
        evs.len()
    );
}

#[test]
fn a_failed_calls_error_text_is_capped_too() {
    let (mut n, _) = started();
    run(&mut n, vec![bash_call("t1")]);
    let text = "z".repeat(MAX_TOOL_OUTPUT_BYTES + 777);
    let evs = run(&mut n, vec![tool_result("t1", &text, true)]);
    let [
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Failed,
            error: Some(e),
            truncated_bytes: 777,
            ..
        },
    ] = &evs[..]
    else {
        panic!("{}", evs.len())
    };
    assert_eq!(e.len(), MAX_TOOL_OUTPUT_BYTES);
}

// ---- enforcement descriptor ----

fn descriptor(extra: Vec<EnforcementReason>) -> EnforcementDescriptor {
    EnforcementDescriptor::evaluate(
        Provider::Claude,
        Some("2.1.308".into()),
        true,
        vec![],
        vec![],
        extra,
    )
}

#[test]
fn the_recorded_descriptor_is_what_session_started_carries() {
    let d = descriptor(vec![]);
    let mut n = ClaudeNormalizer::new();
    let none = run(
        &mut n,
        vec![AdapterInput::Enforcement {
            descriptor: d.clone(),
        }],
    );
    assert!(none.is_empty(), "{none:?}");
    let evs = run(&mut n, vec![user_line("go"), init(), message_start()]);
    assert!(
        matches!(&evs[0], AgentEvent::SessionStarted { enforcement, .. } if *enforcement == d),
        "{evs:?}"
    );
    // The same descriptor again is not a change.
    let same = run(
        &mut n,
        vec![AdapterInput::Enforcement {
            descriptor: d.clone(),
        }],
    );
    assert!(same.is_empty(), "{same:?}");
    // A different one is.
    let d2 = descriptor(vec![EnforcementReason {
        code: ReasonCode::ExternalHooks,
        message: "a hook appeared".into(),
    }]);
    let evs = run(
        &mut n,
        vec![AdapterInput::Enforcement {
            descriptor: d2.clone(),
        }],
    );
    assert!(
        matches!(&evs[..], [AgentEvent::EnforcementChanged { enforcement }] if *enforcement == d2),
        "{evs:?}"
    );
}

#[test]
fn without_a_recorded_descriptor_the_session_is_never_full() {
    let (_, evs) = started();
    let AgentEvent::SessionStarted { enforcement, .. } = &evs[0] else {
        panic!("{evs:?}")
    };
    assert_eq!(enforcement.level, EnforcementLevel::Partial);
    assert!(!enforcement.reasons.is_empty());
}

// ---- message queue ----

fn say(text: &str) -> AdapterInput {
    command(SessionCommand::SendUserMessage { text: text.into() })
}

#[test]
fn the_message_queue_limit_is_decided_by_the_normalizer() {
    let mut n = ClaudeNormalizer::new();
    // Idle: accepted, written, the turn starts.
    assert!(run(&mut n, vec![say("m0")]).is_empty());
    assert!(!n.take_rejected());
    let evs = run(&mut n, vec![user_line("m0"), init(), message_start()]);
    assert!(matches!(evs[1], AgentEvent::TurnStarted { .. }));
    // Eight wait while the turn runs; the ninth is refused.
    for i in 1..=8 {
        assert!(run(&mut n, vec![say(&format!("m{i}"))]).is_empty());
        assert!(!n.take_rejected());
    }
    let evs = run(&mut n, vec![say("m9")]);
    assert!(
        matches!(&evs[..], [AgentEvent::Error { fatal: false, message, .. }] if message.contains("rejected"))
    );
    assert!(n.take_rejected());
    assert!(!n.take_rejected(), "reading clears the flag");
    // The turn ends and one waiting message is written: room again.
    run(&mut n, vec![result(json!({})), user_line("m1")]);
    assert!(run(&mut n, vec![say("m10")]).is_empty());
    assert!(!n.take_rejected());
}

#[test]
fn unread_and_exhausted_tickets_propagate_to_approval_requested() {
    use atrium_protocol::agent::UnreadNotice;

    // Defence in depth: with_notice forces rememberable to false whenever unread is Some.
    let coerced = ApprovalTicket::with_notice("T-coerced".into(), true, Some(UnreadNotice::Unread));
    assert!(!coerced.rememberable);
    assert_eq!(coerced.unread, Some(UnreadNotice::Unread));

    // Serde backwards compatibility: old JSON without `unread` parses as None and omits it.
    let legacy: ApprovalTicket =
        serde_json::from_value(json!({"request_id": "legacy", "rememberable": true})).unwrap();
    assert_eq!(legacy, ApprovalTicket::new("legacy".into(), true));
    let encoded_legacy = serde_json::to_value(&legacy).unwrap();
    assert!(encoded_legacy.get("unread").is_none());

    for notice in [UnreadNotice::Unread, UnreadNotice::Exhausted] {
        let t = ApprovalTicket::with_unread("T-un".into(), notice);
        let roundtrip: ApprovalTicket =
            serde_json::from_value(serde_json::to_value(&t).unwrap()).unwrap();
        assert_eq!(roundtrip, t);

        let (mut n, _) = started();
        run(&mut n, vec![bash_call("t1"), can_use("r1", "t1")]);
        let evs = run(&mut n, vec![gate("t1", GateDecision::Ask { ticket: t })]);
        assert!(
            matches!(
                &evs[0],
                AgentEvent::ApprovalRequested {
                    request_id,
                    rememberable: false,
                    unread,
                    ..
                } if request_id == "T-un" && *unread == Some(notice)
            ),
            "expected unread notice {notice:?} on ApprovalRequested, got {evs:?}"
        );
    }
}
