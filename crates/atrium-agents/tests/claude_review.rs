//! Tests written from the cross-provider review of M1-07 (fix round 1): each
//! one follows a sequence the review gave, written before the fix.
//! Normalizer and tool-subject findings live here; the adapter-level ones
//! (gate races, terminal tail, dropped `start`) are in `claude_adapter.rs`.

use atrium_agents::claude::{ClaudeNormalizer, tool_check, tool_kind};
use atrium_agents::{AdapterInput, ApprovalTicket, GateDecision, Normalizer};
use atrium_protocol::agent::{
    AgentEvent, ApprovalDecision, MessageBody, MessageKind, Provider, SessionCommand, ToolKind,
    ToolStatus,
};
use atrium_protocol::limits::MAX_PARTIAL_ARGS_BYTES;
use atrium_protocol::policy::{EnforcementDescriptor, EnforcementReason, ReasonCode, ToolSubject};
use serde_json::{Value, json};

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

fn result() -> AdapterInput {
    out(
        json!({"type":"result","subtype":"success","is_error":false,"terminal_reason":"completed","total_cost_usd":0.5,"usage":{"input_tokens":10,"output_tokens":5}}),
    )
}

fn run(n: &mut ClaudeNormalizer, inputs: Vec<AdapterInput>) -> Vec<AgentEvent> {
    inputs.iter().flat_map(|i| n.on_input(i)).collect()
}

fn started() -> ClaudeNormalizer {
    let mut n = ClaudeNormalizer::new();
    let evs = run(&mut n, vec![user_line("go"), init(), message_start()]);
    assert!(matches!(evs[0], AgentEvent::SessionStarted { .. }));
    n
}

fn ticket(id: &str) -> ApprovalTicket {
    ApprovalTicket {
        request_id: id.into(),
        rememberable: true,
    }
}

fn ask(id: &str) -> GateDecision {
    GateDecision::Ask { ticket: ticket(id) }
}

fn can_use(rid: &str, id: Option<&str>, name: &str, input: Value) -> AdapterInput {
    let mut req = json!({"subtype":"can_use_tool","tool_name":name,"input":input});
    if let Some(id) = id {
        req["tool_use_id"] = json!(id);
    }
    out(json!({"type":"control_request","request_id":rid,"request":req}))
}

fn gate(id: Option<&str>, name: &str, input: Value, d: GateDecision) -> AdapterInput {
    AdapterInput::Gate {
        check: tool_check(id, name, &input, 1),
        decision: d,
    }
}

fn is_started(e: &AgentEvent) -> bool {
    matches!(e, AgentEvent::SessionStarted { .. })
}

/// The first event is `SessionStarted`, a turn is open before anything that
/// needs one, and nothing precedes them (agent.rs, "Established session").
fn assert_well_opened(evs: &[AgentEvent]) {
    assert!(is_started(&evs[0]), "{evs:?}");
    let turn = evs
        .iter()
        .position(|e| matches!(e, AgentEvent::TurnStarted { .. }))
        .unwrap_or_else(|| panic!("no TurnStarted: {evs:?}"));
    for (i, e) in evs.iter().enumerate() {
        if matches!(
            e,
            AgentEvent::MessageDelta { .. }
                | AgentEvent::ToolCallStarted { .. }
                | AgentEvent::ApprovalRequested { .. }
                | AgentEvent::QuestionAsked { .. }
                | AgentEvent::Usage { .. }
        ) {
            assert!(i > turn, "{e:?} before TurnStarted: {evs:?}");
        }
    }
}

// ---- finding 3: oversize `can_use_tool` ----

fn big_input() -> Value {
    json!({"file_path":"/etc/hosts","old_string":"a","new_string":"n".repeat(MAX_PARTIAL_ARGS_BYTES + 512)})
}

fn small_events(evs: &[AgentEvent]) {
    for e in evs {
        let len = serde_json::to_string(e).unwrap().len();
        assert!(
            len < 64 * 1024,
            "an event carries the oversize input: {len} bytes"
        );
    }
}

#[test]
fn f3_an_oversize_can_use_tool_before_the_tool_use_is_never_offered() {
    let mut n = started();
    let mut evs = run(
        &mut n,
        vec![can_use("r1", Some("toolu_big"), "Edit", big_input())],
    );
    // The adapter denies it without asking; even if a gate answered Ask, the
    // approval must not carry the input.
    evs.extend(run(
        &mut n,
        vec![gate(Some("toolu_big"), "Edit", big_input(), ask("T1"))],
    ));
    // The tool_use record arrives next, with the same input.
    evs.extend(run(
        &mut n,
        vec![assistant(
            json!([{"type":"tool_use","id":"toolu_big","name":"Edit","input":big_input()}]),
        )],
    ));
    small_events(&evs);
    assert!(
        !evs.iter()
            .any(|e| matches!(e, AgentEvent::ApprovalRequested { .. })),
        "{} events",
        evs.len()
    );
    // The call is shown once and failed.
    let started = evs
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCallStarted { .. }))
        .count();
    let failed = evs
        .iter()
        .filter(|e| {
            matches!(
                e,
                AgentEvent::ToolCallFinished {
                    status: ToolStatus::Failed,
                    ..
                }
            )
        })
        .count();
    assert_eq!((started, failed), (1, 1), "{} events", evs.len());
}

#[test]
fn f3_an_oversize_can_use_tool_after_the_tool_use_is_never_offered() {
    let mut n = started();
    let mut evs = run(
        &mut n,
        vec![assistant(
            json!([{"type":"tool_use","id":"toolu_big","name":"Edit","input":big_input()}]),
        )],
    );
    evs.extend(run(
        &mut n,
        vec![
            can_use("r1", Some("toolu_big"), "Edit", big_input()),
            gate(Some("toolu_big"), "Edit", big_input(), ask("T1")),
        ],
    ));
    small_events(&evs);
    assert!(
        !evs.iter()
            .any(|e| matches!(e, AgentEvent::ApprovalRequested { .. })),
        "{} events",
        evs.len()
    );
}

// ---- finding 4: nothing before SessionStarted, nothing outside a turn ----

fn enforcement_and_message() -> Vec<AdapterInput> {
    vec![
        AdapterInput::Enforcement {
            descriptor: descriptor(vec![]),
        },
        AdapterInput::Command {
            command: SessionCommand::SendUserMessage { text: "hi".into() },
        },
        user_line("hi"),
        init(),
    ]
}

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
fn f4_an_approval_before_message_start_opens_the_session_and_the_turn_first() {
    let mut n = ClaudeNormalizer::new();
    let mut evs = run(&mut n, enforcement_and_message());
    assert!(evs.is_empty(), "{evs:?}");
    evs.extend(run(
        &mut n,
        vec![
            can_use("r1", Some("t1"), "Bash", json!({"command":"ls"})),
            gate(Some("t1"), "Bash", json!({"command":"ls"}), ask("T1")),
        ],
    ));
    assert_well_opened(&evs);
    assert!(
        evs.iter()
            .any(|e| matches!(e, AgentEvent::ApprovalRequested { .. })),
        "{evs:?}"
    );
}

#[test]
fn f4_a_question_before_message_start_opens_the_session_and_the_turn_first() {
    let mut n = ClaudeNormalizer::new();
    run(&mut n, enforcement_and_message());
    let evs = run(
        &mut n,
        vec![
            can_use(
                "r1",
                Some("q1"),
                "AskUserQuestion",
                json!({"questions":[{"question":"Q?","options":[{"label":"A"}]}]}),
            ),
            AdapterInput::QuestionTicket {
                id: "r1".into(),
                ticket: Some(ticket("Q1")),
            },
        ],
    );
    assert_well_opened(&evs);
    assert!(
        evs.iter()
            .any(|e| matches!(e, AgentEvent::QuestionAsked { .. })),
        "{evs:?}"
    );
}

#[test]
fn f4_deltas_and_usage_before_message_start_open_the_session_and_the_turn_first() {
    let delta = out(
        json!({"type":"stream_event","api_message_id":"m1","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}}),
    );
    let usage = out(
        json!({"type":"stream_event","api_message_id":"m1","event":{"type":"message_delta","usage":{"input_tokens":1,"output_tokens":1}}}),
    );
    for record in [delta, usage] {
        let mut n = ClaudeNormalizer::new();
        run(&mut n, enforcement_and_message());
        let evs = run(&mut n, vec![record]);
        assert_well_opened(&evs);
    }
}

#[test]
fn f4_a_command_error_before_the_session_starts_waits_for_it() {
    let mut n = ClaudeNormalizer::new();
    run(&mut n, enforcement_and_message());
    let evs = run(
        &mut n,
        vec![AdapterInput::Command {
            command: SessionCommand::ResolveApproval {
                request_id: "unknown".into(),
                decision: ApprovalDecision::Deny { message: None },
            },
        }],
    );
    assert!(evs.is_empty(), "{evs:?}");
    // It is reported once the session exists.
    let evs = run(&mut n, vec![message_start()]);
    assert!(is_started(&evs[0]), "{evs:?}");
    assert!(
        evs.iter()
            .any(|e| matches!(e, AgentEvent::Error { fatal: false, .. })),
        "{evs:?}"
    );
}

// ---- finding 7: tickets follow the call they were made for ----

#[test]
fn f7_a_hook_ticket_is_not_reused_when_can_use_tool_changes_the_input() {
    let mut n = started();
    // The hook was asked about `echo safe` and the gate said Ask (T1).
    let evs = run(
        &mut n,
        vec![
            assistant(
                json!([{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"echo safe"}}]),
            ),
            gate(
                Some("toolu_1"),
                "Bash",
                json!({"command":"echo safe"}),
                ask("T1"),
            ),
        ],
    );
    assert!(
        !evs.iter()
            .any(|e| matches!(e, AgentEvent::ApprovalRequested { .. }))
    );
    // The CLI then asks about the same call with another command.
    let evs = run(
        &mut n,
        vec![can_use(
            "r_can",
            Some("toolu_1"),
            "Bash",
            json!({"command":"rm -rf /tmp/x"}),
        )],
    );
    assert!(
        !evs.iter()
            .any(|e| matches!(e, AgentEvent::ApprovalRequested { .. })),
        "T1 was made for another input: {evs:?}"
    );
    // Only the gate's answer for the new input shows it.
    let evs = run(
        &mut n,
        vec![gate(
            Some("toolu_1"),
            "Bash",
            json!({"command":"rm -rf /tmp/x"}),
            ask("T2"),
        )],
    );
    assert!(
        matches!(
            &evs[..],
            [AgentEvent::ApprovalRequested { request_id, input, .. }]
                if request_id == "T2" && input["command"] == "rm -rf /tmp/x"
        ),
        "{evs:?}"
    );
}

#[test]
fn f7_two_anonymous_calls_keep_their_own_tickets_in_order() {
    let mut n = started();
    let input = json!({"command":"ls"});
    run(
        &mut n,
        vec![
            gate(None, "Bash", input.clone(), ask("T1")),
            gate(None, "Bash", input.clone(), ask("T2")),
        ],
    );
    let first = run(&mut n, vec![can_use("r1", None, "Bash", input.clone())]);
    let second = run(&mut n, vec![can_use("r2", None, "Bash", input)]);
    assert!(
        matches!(&first[..], [AgentEvent::ApprovalRequested { request_id, .. }] if request_id == "T1"),
        "{first:?}"
    );
    assert!(
        matches!(&second[..], [AgentEvent::ApprovalRequested { request_id, .. }] if request_id == "T2"),
        "{second:?}"
    );
}

// ---- finding 8: a message completes once ----

#[test]
fn f8_a_late_delta_after_the_assistant_record_does_not_complete_the_message_twice() {
    let mut n = started();
    let delta = |text: &str| {
        out(
            json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}},"api_message_id":"m1"}),
        )
    };
    let mut evs = run(
        &mut n,
        vec![
            delta("Hi"),
            assistant(json!([{"type":"text","text":"Hi"}])),
            delta("!"),
            result(),
        ],
    );
    evs.retain(|e| {
        matches!(
            e,
            AgentEvent::MessageCompleted {
                kind: MessageKind::Assistant,
                ..
            }
        )
    });
    assert_eq!(evs.len(), 1, "{evs:?}");
    assert!(matches!(
        &evs[0],
        AgentEvent::MessageCompleted {
            body: MessageBody::Streamed { deltas: 1, .. },
            ..
        }
    ));
}

// ---- finding 9: SessionStarted agrees with the descriptor `start` returned ----

#[test]
fn f9_a_descriptor_that_changes_before_the_session_starts_is_reported_as_a_change() {
    let d1 = descriptor(vec![]);
    let d2 = descriptor(vec![EnforcementReason {
        code: ReasonCode::ExternalHooks,
        message: "a hook appeared".into(),
    }]);
    let mut n = ClaudeNormalizer::new();
    let evs = run(
        &mut n,
        vec![
            AdapterInput::Enforcement {
                descriptor: d1.clone(),
            },
            // dispatch_next refreshed it before the first message.
            AdapterInput::Enforcement {
                descriptor: d2.clone(),
            },
            user_line("hi"),
            init(),
            message_start(),
        ],
    );
    // What `start()` returned is what SessionStarted says; the change follows.
    assert!(
        matches!(&evs[0], AgentEvent::SessionStarted { enforcement, .. } if *enforcement == d1),
        "{evs:?}"
    );
    assert!(
        evs.iter().any(
            |e| matches!(e, AgentEvent::EnforcementChanged { enforcement } if *enforcement == d2)
        ),
        "{evs:?}"
    );
    // And the stream ends up at d2 only once.
    let again = run(
        &mut n,
        vec![AdapterInput::Enforcement {
            descriptor: d2.clone(),
        }],
    );
    assert!(again.is_empty(), "{again:?}");
}

// ---- finding 2 and care for the other tools: subjects ----

fn subject(name: &str, input: Value) -> ToolSubject {
    tool_check(Some("t"), name, &input, 1).subject
}

fn paths_of(s: &ToolSubject) -> Vec<String> {
    match s {
        ToolSubject::Paths { paths, .. } => paths.clone(),
        other => panic!("not Paths: {other:?}"),
    }
}

#[test]
fn f2_a_glob_pattern_that_leaves_the_base_names_its_target() {
    // Absolute: the target is in `paths`, with the base kept.
    let s = subject(
        "Glob",
        json!({"path":"/home/ana/work","pattern":"/home/ana/.ssh/id_ed25519"}),
    );
    let paths = paths_of(&s);
    assert!(
        paths.contains(&"/home/ana/.ssh/id_ed25519".to_owned()),
        "{paths:?}"
    );
    assert!(paths.contains(&"/home/ana/work".to_owned()), "{paths:?}");

    // No path given: the base is ".".
    let paths = paths_of(&subject("Glob", json!({"pattern":"/etc/passwd"})));
    assert!(paths.contains(&"/etc/passwd".to_owned()) && paths.contains(&".".to_owned()));

    // A wildcard in the last component: its directory is the target.
    let paths = paths_of(&subject(
        "Glob",
        json!({"path":"sub","pattern":"/etc/pass*"}),
    ));
    assert!(paths.contains(&"/etc/".to_owned()), "{paths:?}");

    // Tilde.
    let paths = paths_of(&subject("Glob", json!({"pattern":"~/.ssh/*"})));
    assert!(paths.contains(&"~/.ssh/".to_owned()), "{paths:?}");
    let paths = paths_of(&subject("Glob", json!({"pattern":"~*"})));
    assert!(paths.contains(&"~".to_owned()), "{paths:?}");

    // Climbing out of the base: joined to it, for the policy to resolve.
    let paths = paths_of(&subject(
        "Glob",
        json!({"path":"/home/ana/work","pattern":"../../ana/.ssh/**"}),
    ));
    assert!(
        paths.contains(&"/home/ana/work/../../ana/.ssh/".to_owned()),
        "{paths:?}"
    );

    // A pattern inside the base adds nothing.
    let s = subject("Glob", json!({"path":"src","pattern":"**/*.{rs,toml}"}));
    assert_eq!(
        s,
        ToolSubject::Paths {
            paths: vec!["src".into()],
            glob: Some("**/*.{rs,toml}".into()),
            recursive: true
        }
    );
}

#[test]
fn f2_a_glob_pattern_that_cannot_be_bounded_is_unknown() {
    for pattern in ["**/../../etc/passwd", "src/*/../../../x", "a\\b", "*/.."] {
        assert_eq!(
            subject("Glob", json!({"path":"/work","pattern":pattern})),
            ToolSubject::Unknown,
            "{pattern}"
        );
    }
}

#[test]
fn f2_missing_empty_or_non_string_fields_are_unknown() {
    for input in [
        json!({}),
        json!({"pattern":""}),
        json!({"pattern":5}),
        json!({"path":"","pattern":"*.rs"}),
        json!({"path":123,"pattern":"*.rs"}),
        json!({"path":null,"pattern":"*.rs"}),
    ] {
        assert_eq!(
            subject("Glob", input.clone()),
            ToolSubject::Unknown,
            "{input}"
        );
    }
    for input in [
        json!({}),
        json!({"pattern":""}),
        json!({"pattern":"foo","path":""}),
        json!({"pattern":"foo","path":123}),
        json!({"pattern":"foo","glob":""}),
        json!({"pattern":"foo","glob":7}),
        json!({"pattern":7,"path":"src"}),
    ] {
        assert_eq!(
            subject("Grep", input.clone()),
            ToolSubject::Unknown,
            "{input}"
        );
    }
    // Still fine when well-formed.
    assert_eq!(
        subject("Grep", json!({"pattern":"foo"})),
        ToolSubject::Paths {
            paths: vec![".".into()],
            glob: None,
            recursive: true
        }
    );
}

#[test]
fn f2_a_grep_glob_that_leaves_the_path_names_its_target_too() {
    let paths = paths_of(&subject(
        "Grep",
        json!({"pattern":"key","path":"/home/ana/work","glob":"/home/ana/.ssh/*"}),
    ));
    assert!(paths.contains(&"/home/ana/.ssh/".to_owned()), "{paths:?}");
    assert!(paths.contains(&"/home/ana/work".to_owned()), "{paths:?}");
    assert_eq!(
        subject("Grep", json!({"pattern":"k","glob":"../../x/**"})),
        ToolSubject::Paths {
            paths: vec![".".into(), "./../../x/".into(), "./../../x/**".into()],
            glob: Some("../../x/**".into()),
            recursive: true
        }
    );
}

/// What every tool the CLI is known to have gets today (round-1 item 8).
/// `Unknown` is fine (the policy asks); a subject that hides what a tool
/// acts on is not.
#[test]
fn item8_subjects_of_the_tools_without_a_dedicated_one_are_pinned() {
    let cases: [(&str, Value, ToolKind); 12] = [
        (
            "WebSearch",
            json!({"query":"how to exfiltrate"}),
            ToolKind::Fetch,
        ),
        (
            "Task",
            json!({"description":"d","prompt":"read /etc/shadow","subagent_type":"general-purpose"}),
            ToolKind::Other,
        ),
        (
            "Agent",
            json!({"description":"d","prompt":"p"}),
            ToolKind::Other,
        ),
        ("Skill", json!({"skill":"deploy"}), ToolKind::Other),
        ("BashOutput", json!({"bash_id":"1"}), ToolKind::Execute),
        ("KillShell", json!({"shell_id":"1"}), ToolKind::Execute),
        ("KillBash", json!({"shell_id":"1"}), ToolKind::Execute),
        ("ExitPlanMode", json!({"plan":"do things"}), ToolKind::Other),
        ("TodoWrite", json!({"todos":[]}), ToolKind::Other),
        ("ToolSearch", json!({"query":"select:x"}), ToolKind::Search),
        ("SomethingNew", json!({"x":1}), ToolKind::Other),
        ("mcp__srv", json!({}), ToolKind::Mcp),
    ];
    for (name, input, kind) in cases {
        assert_eq!(tool_kind(name), kind, "{name}");
        assert_eq!(subject(name, input), ToolSubject::Unknown, "{name}");
    }
    // The ones with a subject keep it.
    assert!(matches!(
        subject("MultiEdit", json!({"file_path":"/a/b","edits":[]})),
        ToolSubject::Paths { .. }
    ));
    assert!(matches!(
        subject("PowerShell", json!({"command":"dir"})),
        ToolSubject::Command { .. }
    ));
}

/// Braces that hide a root or a tilde are expanded, and the target named
/// (round 2: they used to be `Unknown`).
#[test]
fn f2_braces_that_hide_a_root_are_expanded_and_the_target_named() {
    for (pattern, target) in [
        ("{/etc/passwd,x}", "/etc/passwd"),
        ("{a,/etc/passwd}", "/etc/passwd"),
        ("{~/x,y}", "~/x"),
    ] {
        let paths = paths_of(&subject("Glob", json!({"path":"/work","pattern":pattern})));
        assert!(paths.contains(&target.to_owned()), "{pattern}: {paths:?}");
    }
}

// ---- round 2, item 3: the key of a call's input is canonical ----

#[test]
fn r2_a_ticket_survives_the_input_being_reserialized_with_other_key_order() {
    let mut n = started();
    let hook_input =
        json!({"path":"/etc/hosts","pattern":"root","extra":{"b":1,"a":[{"y":2,"x":1}]}});
    let can_use_input =
        json!({"extra":{"a":[{"x":1,"y":2}],"b":1},"pattern":"root","path":"/etc/hosts"});
    // The serialized forms really differ.
    assert_ne!(hook_input.to_string(), can_use_input.to_string());
    run(
        &mut n,
        vec![
            assistant(
                json!([{"type":"tool_use","id":"toolu_1","name":"Grep","input":hook_input.clone()}]),
            ),
            gate(Some("toolu_1"), "Grep", hook_input, ask("T1")),
        ],
    );
    let evs = run(
        &mut n,
        vec![can_use("r_can", Some("toolu_1"), "Grep", can_use_input)],
    );
    assert!(
        matches!(&evs[..], [AgentEvent::ApprovalRequested { request_id, .. }] if request_id == "T1"),
        "{evs:?}"
    );
    // Arrays keep their order: a reordered array is another input.
    let mut n = started();
    run(
        &mut n,
        vec![gate(Some("t"), "Grep", json!({"a":[1,2]}), ask("T1"))],
    );
    let evs = run(
        &mut n,
        vec![can_use("r", Some("t"), "Grep", json!({"a":[2,1]}))],
    );
    assert!(evs.is_empty(), "{evs:?}");
}

#[test]
fn r2_our_canonical_text_is_the_policys() {
    use atrium_protocol::agent::ToolKind;
    let v = json!({"c":null,"b":"é\n","a":[{"y":2,"x":1}]});
    let core = atrium_core::policy::canonical_input(ToolKind::Search, "Grep", &v);
    let core = String::from_utf8(core).unwrap();
    assert!(
        core.ends_with(r#"{"a":[{"x":1,"y":2}],"b":"é\n","c":null}"#),
        "{core}"
    );
}

// ---- round 2, item 2: nothing that needs an answer is ever held ----

#[test]
fn r2_records_before_any_init_establish_the_session_from_the_record() {
    let mut n = ClaudeNormalizer::new();
    let evs = run(
        &mut n,
        vec![
            user_line("go"),
            message_start(),
            assistant(json!([{"type":"text","text":"hi"}])),
            result(),
        ],
    );
    assert!(
        matches!(&evs[0], AgentEvent::SessionStarted { model: Some(m), .. } if m == "claude-x"),
        "{evs:?}"
    );
    assert_well_opened(&evs);
    assert!(
        matches!(evs.last(), Some(AgentEvent::TurnCompleted { turn: 1, .. })),
        "{evs:?}"
    );
}

#[test]
fn r2_a_failed_start_with_only_noise_is_still_two_events() {
    let mut n = ClaudeNormalizer::new();
    let evs = run(
        &mut n,
        vec![
            user_line("go"),
            out(json!({"type":"system","subtype":"hook_started"})),
            AdapterInput::Exit { code: Some(1) },
        ],
    );
    assert!(
        matches!(
            &evs[..],
            [
                AgentEvent::Error { fatal: true, .. },
                AgentEvent::SessionEnded { .. }
            ]
        ),
        "{evs:?}"
    );
}

/// Case A: an approval or a question before `init` is shown at once.
#[test]
fn r2_an_approval_or_question_before_init_is_shown_at_once() {
    let mut n = ClaudeNormalizer::new();
    let evs = run(
        &mut n,
        vec![
            user_line("go"),
            can_use("c1", Some("t1"), "Bash", json!({"command":"ls"})),
            gate(Some("t1"), "Bash", json!({"command":"ls"}), ask("T1")),
        ],
    );
    assert_well_opened(&evs);
    assert!(
        evs.iter().any(
            |e| matches!(e, AgentEvent::ApprovalRequested { request_id, .. } if request_id == "T1")
        ),
        "{evs:?}"
    );

    let mut n = ClaudeNormalizer::new();
    let evs = run(
        &mut n,
        vec![
            user_line("go"),
            can_use(
                "c1",
                Some("q1"),
                "AskUserQuestion",
                json!({"questions":[{"question":"Q?","options":[{"label":"A"}]}]}),
            ),
            AdapterInput::QuestionTicket {
                id: "c1".into(),
                ticket: Some(ticket("Q1")),
            },
        ],
    );
    assert_well_opened(&evs);
    assert!(
        evs.iter()
            .any(|e| matches!(e, AgentEvent::QuestionAsked { .. }))
    );
}

/// Case B: a `result` before `init` resolves what it shows, and turns stay
/// in order.
#[test]
fn r2_a_result_before_init_resolves_the_request_and_keeps_turns_in_order() {
    let mut n = ClaudeNormalizer::new();
    let evs = run(
        &mut n,
        vec![
            user_line("go"),
            can_use("r1", Some("t1"), "Bash", json!({"command":"ls"})),
            gate(Some("t1"), "Bash", json!({"command":"ls"}), ask("T1")),
            result(),
            init(),
            message_start(),
            AdapterInput::Exit { code: Some(0) },
        ],
    );
    assert_well_opened(&evs);
    let mut open: Option<u32> = None;
    let mut requested = vec![];
    let mut resolved = vec![];
    for e in &evs {
        match e {
            AgentEvent::TurnStarted { turn } => {
                assert!(
                    open.is_none(),
                    "turn {turn} started inside another: {evs:?}"
                );
                open = Some(*turn);
            }
            AgentEvent::TurnCompleted { turn, .. } => {
                assert_eq!(open.take(), Some(*turn), "{evs:?}");
            }
            AgentEvent::ApprovalRequested { request_id, .. } => requested.push(request_id.clone()),
            AgentEvent::RequestResolved { request_id, .. } => resolved.push(request_id.clone()),
            _ => {}
        }
    }
    assert!(open.is_none(), "a turn is left open: {evs:?}");
    assert_eq!(requested, ["T1"]);
    assert_eq!(resolved, ["T1"]);
    assert!(matches!(evs.last(), Some(AgentEvent::SessionEnded { .. })));
}

/// Case C: what waits for the session is bounded.
#[test]
fn r2_what_waits_for_the_session_is_bounded() {
    use atrium_protocol::limits::MAX_DIAGNOSTIC_LINES;
    let mut n = ClaudeNormalizer::new();
    let mut inputs = vec![user_line("go")];
    for i in 0..(MAX_DIAGNOSTIC_LINES * 10) {
        inputs.push(out(json!({"type":"system","subtype":format!("noise_{i}")})));
    }
    assert!(run(&mut n, inputs).is_empty());
    let evs = run(&mut n, vec![init(), message_start()]);
    let unmapped: Vec<&AgentEvent> = evs
        .iter()
        .filter(|e| matches!(e, AgentEvent::Unmapped { .. }))
        .collect();
    assert_eq!(unmapped.len(), MAX_DIAGNOSTIC_LINES, "{}", evs.len());
    // The newest are the ones kept.
    assert!(matches!(
        unmapped.last(),
        Some(AgentEvent::Unmapped { record_type, .. })
            if *record_type == format!("system/noise_{}", MAX_DIAGNOSTIC_LINES * 10 - 1)
    ));
}

// ---- round 2, item 5: a late assistant record after the turn closed the message ----

#[test]
fn r2_a_late_assistant_record_does_not_complete_a_closed_message_again() {
    let mut n = started();
    let mut evs = run(
        &mut n,
        vec![
            out(
                json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}},"api_message_id":"m1"}),
            ),
            result(),
            assistant(json!([{"type":"text","text":"Hi"}])),
        ],
    );
    evs.retain(|e| {
        matches!(
            e,
            AgentEvent::MessageCompleted {
                kind: MessageKind::Assistant,
                ..
            }
        )
    });
    assert_eq!(evs.len(), 1, "{evs:?}");
}
