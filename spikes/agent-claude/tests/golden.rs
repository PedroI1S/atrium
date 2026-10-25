//! Golden tests over the live-recorded fixtures in `fixtures/agents/claude`,
//! plus the scenario-specific facts each one is there to prove.
//!
//! Re-record with `cargo run -p spike-agent-claude --example record -- <name>`.
//! After a normalizer change, `UPDATE_GOLDEN=1 cargo test -p
//! spike-agent-claude --test golden` rewrites the expected events; review that
//! diff like code.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use atrium_agents::fixture::{self, FixtureLine};
use atrium_protocol::agent::{AgentEvent, RequestOutcome, ToolKind, ToolStatus, TurnOutcome};
use spike_agent_claude::ClaudeNormalizer;

const REQUIRED: &[&str] = &[
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

const EXTRA: &[&str] = &[
    "gate-deny-exec",
    "gate-deny-edit",
    "gate-deny-delete",
    "gate-deny-fetch",
    "gate-deny-fullaccess",
    "gate-deny-mcp",
    "gate-deny-subagent",
    "mcp-allow",
    "interrupt-resume",
    "rate-limited",
];

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/agents/claude")
}

fn lines(name: &str) -> Vec<FixtureLine> {
    fixture::read(&dir().join(format!("{name}.jsonl"))).unwrap()
}

fn events(name: &str) -> Vec<AgentEvent> {
    fixture::normalize(&lines(name), &mut ClaudeNormalizer::new())
}

fn all_names() -> impl Iterator<Item = &'static str> {
    REQUIRED.iter().chain(EXTRA.iter()).copied()
}

#[test]
fn every_scenario_matches_its_golden_file() {
    for name in all_names() {
        let path = dir().join(format!("{name}.jsonl"));
        assert!(path.exists(), "missing fixture {name}");
        assert!(
            dir().join(format!("{name}.md")).exists(),
            "missing {name}.md"
        );
        fixture::assert_golden(&path, &mut ClaudeNormalizer::new());
    }
}

#[test]
fn fixtures_are_sanitized() {
    // The recorder runs the structured Sanitizer; this is the independent
    // check that nothing machine-specific slipped through.
    for name in all_names() {
        let text = fs::read_to_string(dir().join(format!("{name}.jsonl"))).unwrap();
        for bad in ["/Users/", "/home/", "@icloud.com", "ATRIUM-7Q2X-CANARY"] {
            assert!(!text.contains(bad), "{name}.jsonl contains {bad}");
        }
        let events = fs::read_to_string(dir().join(format!("{name}.events.json"))).unwrap();
        for bad in ["/Users/", "/home/", "@icloud.com", "ATRIUM-7Q2X-CANARY"] {
            assert!(!events.contains(bad), "{name}.events.json contains {bad}");
        }
        if let (Ok(home), Ok(user)) = (std::env::var("HOME"), std::env::var("USER")) {
            let sanitizer = atrium_agents::fixture::Sanitizer::new(&home, &user, &[]);
            assert!(sanitizer.is_clean(&text), "{name}.jsonl is not clean");
        }
    }
}

#[test]
fn every_scenario_obeys_the_lifecycle_rules() {
    for name in all_names() {
        let evs = events(name);
        let desc = format!("{name}: {:#?}", evs.iter().map(kind).collect::<Vec<_>>());
        // Established session, or pre-session failure.
        match evs.first().unwrap() {
            AgentEvent::SessionStarted { .. } => {}
            AgentEvent::Error { fatal: true, .. } => {
                assert!(
                    !evs.iter()
                        .any(|e| matches!(e, AgentEvent::SessionStarted { .. })),
                    "{desc}"
                );
            }
            other => panic!("{name}: first event is {other:?}"),
        }
        assert!(
            matches!(evs.last().unwrap(), AgentEvent::SessionEnded { .. }),
            "{desc}"
        );
        assert_eq!(
            evs.iter()
                .filter(|e| matches!(e, AgentEvent::SessionEnded { .. }))
                .count(),
            1,
            "{desc}"
        );

        // Every call and request is closed exactly once.
        let mut open_calls = HashSet::new();
        let mut open_requests = HashSet::new();
        let mut turn_open = false;
        let mut open_msgs: HashSet<String> = HashSet::new();
        let mut streamed: std::collections::HashMap<String, String> = Default::default();
        for e in &evs {
            match e {
                AgentEvent::ToolCallStarted { call_id, .. } => {
                    assert!(
                        open_calls.insert(call_id.clone()),
                        "{desc}: {call_id} twice"
                    );
                }
                AgentEvent::ToolCallUpdated { call_id, .. } => {
                    assert!(
                        open_calls.contains(call_id),
                        "{desc}: update of closed {call_id}"
                    );
                }
                AgentEvent::ToolCallFinished { call_id, .. } => {
                    assert!(
                        open_calls.remove(call_id),
                        "{desc}: {call_id} finished twice"
                    );
                }
                AgentEvent::ApprovalRequested { request_id, .. }
                | AgentEvent::QuestionAsked { request_id, .. } => {
                    assert!(open_requests.insert(request_id.clone()), "{desc}");
                }
                AgentEvent::RequestResolved { request_id, .. } => {
                    assert!(open_requests.remove(request_id), "{desc}: {request_id}");
                }
                AgentEvent::TurnStarted { .. } => {
                    assert!(!turn_open, "{desc}");
                    turn_open = true;
                }
                AgentEvent::TurnCompleted { .. } => {
                    assert!(turn_open, "{desc}");
                    turn_open = false;
                }
                AgentEvent::MessageDelta {
                    message_id, text, ..
                } => {
                    open_msgs.insert(message_id.clone());
                    streamed
                        .entry(message_id.clone())
                        .or_default()
                        .push_str(text);
                }
                AgentEvent::MessageCompleted {
                    message_id, text, ..
                } => {
                    if let Some(deltas) = streamed.get(message_id) {
                        assert_eq!(deltas, text, "{desc}: {message_id}");
                    }
                    open_msgs.remove(message_id);
                }
                AgentEvent::SessionEnded { .. } => {
                    assert!(open_calls.is_empty(), "{desc}: open calls {open_calls:?}");
                    assert!(open_requests.is_empty(), "{desc}");
                    assert!(open_msgs.is_empty(), "{desc}");
                    assert!(!turn_open, "{desc}");
                }
                _ => {}
            }
        }
    }
}

#[test]
fn nothing_unmapped_except_subagent_bookkeeping() {
    for name in all_names() {
        for e in events(name) {
            if let AgentEvent::Unmapped { record_type, .. } = e {
                assert!(
                    matches!(
                        record_type.as_str(),
                        "system/task_started" | "system/task_updated" | "system/task_notification"
                    ),
                    "{name}: unmapped {record_type}"
                );
            }
        }
    }
}

fn kind(e: &AgentEvent) -> String {
    serde_json::to_value(e).unwrap()["type"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn finished(evs: &[AgentEvent], status: ToolStatus) -> Vec<&AgentEvent> {
    evs.iter()
        .filter(|e| matches!(e, AgentEvent::ToolCallFinished { status: s, .. } if *s == status))
        .collect()
}

fn text_of(evs: &[AgentEvent]) -> String {
    evs.iter()
        .filter_map(|e| match e {
            AgentEvent::MessageCompleted { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn started_kinds(evs: &[AgentEvent]) -> Vec<(String, ToolKind)> {
    evs.iter()
        .filter_map(|e| match e {
            AgentEvent::ToolCallStarted { name, kind, .. } => Some((name.clone(), *kind)),
            _ => None,
        })
        .collect()
}

#[test]
fn simple_answers_without_tools() {
    let evs = events("simple");
    assert!(started_kinds(&evs).is_empty());
    assert!(text_of(&evs).to_lowercase().contains("pong"));
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Completed,
            usage: Some(u),
            ..
        } if u.cost_usd.is_some() && u.output_tokens.is_some()
    )));
    assert!(
        evs.iter()
            .any(|e| matches!(e, AgentEvent::QuotaUpdated { .. }))
    );
}

#[test]
fn tool_read_runs_after_the_gate_allows() {
    let evs = events("tool-read");
    assert!(
        started_kinds(&evs)
            .iter()
            .any(|(n, k)| n == "Read" && *k == ToolKind::Read)
    );
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished { status: ToolStatus::Succeeded, output: Some(o), .. } if o.contains("MARIGOLD")
    )));
    assert!(text_of(&evs).contains("MARIGOLD"));
}

/// Enforcement matrix evidence: every denied class never produced output.
fn assert_denied(name: &str, kind_wanted: ToolKind) {
    let evs = events(name);
    let denied = finished(&evs, ToolStatus::Denied);
    assert!(!denied.is_empty(), "{name}: nothing was denied");
    let started = started_kinds(&evs);
    assert!(
        started.iter().any(|(_, k)| *k == kind_wanted),
        "{name}: no {kind_wanted:?} call started: {started:?}"
    );
    // No call of the denied kind succeeded.
    let mut kinds = std::collections::HashMap::new();
    for e in &evs {
        match e {
            AgentEvent::ToolCallStarted { call_id, kind, .. } => {
                kinds.insert(call_id.clone(), *kind);
            }
            AgentEvent::ToolCallFinished {
                call_id,
                status: ToolStatus::Succeeded,
                ..
            } => {
                assert_ne!(
                    kinds.get(call_id),
                    Some(&kind_wanted),
                    "{name}: a {kind_wanted:?} call succeeded"
                );
            }
            _ => {}
        }
    }
    for e in denied {
        if let AgentEvent::ToolCallFinished { output, .. } = e {
            assert!(output.is_none());
        }
    }
}

#[test]
fn gate_deny_blocks_reads_and_the_contents_never_appear() {
    assert_denied("gate-deny", ToolKind::Read);
    let raw = fs::read_to_string(dir().join("gate-deny.jsonl")).unwrap();
    assert!(!raw.contains("ATRIUM-7Q2X-CANARY"));
    let json = fs::read_to_string(dir().join("gate-deny.events.json")).unwrap();
    assert!(!json.contains("ATRIUM-7Q2X-CANARY"));
}

#[test]
fn gate_deny_covers_execute_mcp_subagent_and_full_access() {
    assert_denied("gate-deny-exec", ToolKind::Execute);
    assert_denied("gate-deny-edit", ToolKind::Edit);
    assert_denied("gate-deny-delete", ToolKind::Execute);
    assert_denied("gate-deny-fetch", ToolKind::Fetch);
    assert_denied("gate-deny-fullaccess", ToolKind::Execute);
    assert_denied("gate-deny-mcp", ToolKind::Mcp);
    assert_denied("gate-deny-subagent", ToolKind::Read);
    // The subagent's call is nested: its Read was denied while the Agent call
    // itself passed.
    let evs = events("gate-deny-subagent");
    assert!(
        started_kinds(&evs)
            .iter()
            .any(|(n, _)| n == "Agent" || n == "Task")
    );
    // The control: the same MCP call succeeds when the gate allows it.
    let evs = events("mcp-allow");
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished { status: ToolStatus::Succeeded, output: Some(o), .. } if o.contains("echo: ping")
    )));
}

#[test]
fn approvals_resolve_both_ways() {
    for (name, outcome, tool_status) in [
        (
            "approval-allow",
            RequestOutcome::Approved,
            ToolStatus::Succeeded,
        ),
        ("approval-deny", RequestOutcome::Denied, ToolStatus::Denied),
    ] {
        let evs = events(name);
        let asked = evs
            .iter()
            .position(|e| {
                matches!(
                    e,
                    AgentEvent::ApprovalRequested {
                        kind: ToolKind::Execute,
                        ..
                    }
                )
            })
            .unwrap_or_else(|| panic!("{name}: no approval"));
        let resolved = evs
            .iter()
            .position(
                |e| matches!(e, AgentEvent::RequestResolved { outcome: o, .. } if *o == outcome),
            )
            .unwrap_or_else(|| panic!("{name}: not resolved as {outcome:?}"));
        assert!(asked < resolved);
        assert_eq!(finished(&evs, tool_status).len(), 1, "{name}");
    }
}

#[test]
fn question_carries_structured_options_and_is_answered() {
    let evs = events("question");
    let q = evs
        .iter()
        .find_map(|e| match e {
            AgentEvent::QuestionAsked { questions, .. } => Some(questions),
            _ => None,
        })
        .expect("a question");
    assert_eq!(q.len(), 1);
    assert!(q[0].options.len() >= 3);
    assert_eq!(q[0].options[0].id, "o0");
    assert!(!q[0].multi_select);
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::RequestResolved {
            outcome: RequestOutcome::Answered,
            ..
        }
    )));
    // The first option was chosen, and the agent saw it.
    assert!(
        text_of(&evs)
            .to_lowercase()
            .contains(&q[0].options[0].label.to_lowercase())
    );
}

#[test]
fn interrupt_ends_the_turn_interrupted_and_cancels_the_open_call() {
    for name in ["interrupt", "interrupt-resume"] {
        let evs = events(name);
        assert_eq!(finished(&evs, ToolStatus::Cancelled).len(), 1, "{name}");
        assert!(evs.iter().any(|e| matches!(
            e,
            AgentEvent::TurnCompleted {
                turn: 1,
                outcome: TurnOutcome::Interrupted,
                ..
            }
        )));
    }
    let evs = events("interrupt-resume");
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::TurnCompleted {
            turn: 2,
            outcome: TurnOutcome::Completed,
            ..
        }
    )));
}

#[test]
fn multi_turn_and_resume_keep_context() {
    let evs = events("multi-turn");
    let turns: Vec<_> = evs
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
        .collect();
    assert_eq!(turns.len(), 2);
    assert!(text_of(&evs).contains("42"));

    let evs = events("resume");
    assert!(text_of(&evs).to_uppercase().contains("PINEAPPLE"));
    // Same native id as the (unrecorded) first session: the init record says
    // so and the adapter passed --resume. The id is in SessionStarted.
    assert!(
        evs.iter()
            .any(|e| matches!(e, AgentEvent::SessionStarted { .. }))
    );
}

#[test]
fn startup_error_is_a_pre_session_failure() {
    let evs = events("startup-error");
    assert_eq!(evs.len(), 2, "{evs:#?}");
    assert!(matches!(
        &evs[0],
        AgentEvent::Error { fatal: true, message } if message.contains("nonexistent-model-xyz")
    ));
    assert!(matches!(&evs[1], AgentEvent::SessionEnded { .. }));
}

#[test]
fn a_real_refusal_is_one_rate_limited_event_inside_an_established_session() {
    let evs = events("rate-limited");
    assert!(matches!(evs[0], AgentEvent::SessionStarted { .. }));
    let limited: Vec<_> = evs
        .iter()
        .filter(|e| matches!(e, AgentEvent::RateLimited { .. }))
        .collect();
    assert_eq!(limited.len(), 1, "{evs:#?}");
    assert!(matches!(
        limited[0],
        AgentEvent::RateLimited { resets_at_ms: Some(_), message: Some(m) } if m.contains("limit")
    ));
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Failed,
            ..
        }
    )));
    assert!(!evs.iter().any(|e| matches!(e, AgentEvent::Error { .. })));
}
