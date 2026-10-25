use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use atrium_agents::fixture::{Sanitizer, assert_golden};
use atrium_protocol::agent::{AgentEvent, Provider};
use spike_agent_agy::AgyNormalizer;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("fixtures")
        .join("agents")
        .join("agy")
}

const ALL_FIXTURES: &[&str] = &[
    "m0-00-smoke",
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

#[test]
fn golden_all_agy_fixtures() {
    let dir = fixtures_dir();
    for name in ALL_FIXTURES {
        let jsonl = dir.join(format!("{name}.jsonl"));
        assert!(jsonl.exists(), "missing fixture {}", jsonl.display());
        assert_golden(&jsonl, &mut AgyNormalizer::default());
    }
}

#[test]
fn all_fixture_files_are_sanitized() {
    let dir = fixtures_dir();
    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/pedro".to_string());
    let user = std::env::var("USER").unwrap_or_else(|_| "pedro".to_string());
    let sanitizer = Sanitizer::new(&home, &user, &["sk-test-secret"]);
    let entries = fs::read_dir(&dir).expect("read fixtures/agents/agy");
    let mut checked = 0usize;

    for entry in entries {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_file() {
            let text = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
            assert!(
                sanitizer.is_clean(&text),
                "unsanitized content in {}",
                path.display()
            );
            // Also verify no explicit home paths or tokens leaked
            assert!(
                !text.contains("/Users/pedro"),
                "leaked user path in {}",
                path.display()
            );
            checked += 1;
        }
    }

    assert!(
        checked >= ALL_FIXTURES.len() * 2,
        "expected at least {} files in fixtures/agents/agy, found {checked}",
        ALL_FIXTURES.len() * 2
    );
}

#[test]
fn event_stream_contract_invariants_hold_for_all_fixtures() {
    let dir = fixtures_dir();

    for name in ALL_FIXTURES {
        let events_path = dir.join(format!("{name}.events.json"));
        let raw = fs::read_to_string(&events_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", events_path.display()));
        let events: Vec<AgentEvent> = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("parse {}: {e}", events_path.display()));

        assert!(!events.is_empty(), "{name}: events list must not be empty");

        // 1. Exactly one SessionEnded at the very end
        let session_ended_positions: Vec<usize> = events
            .iter()
            .enumerate()
            .filter_map(|(i, ev)| matches!(ev, AgentEvent::SessionEnded { .. }).then_some(i))
            .collect();
        assert_eq!(
            session_ended_positions,
            vec![events.len() - 1],
            "{name}: SessionEnded must appear exactly once as the final event"
        );

        // 2. First event is either SessionStarted or fatal Error (for startup-error)
        if *name == "startup-error" {
            assert!(
                matches!(events.first(), Some(AgentEvent::Error { fatal: true, .. })),
                "{name}: startup-error must start with Error {{ fatal: true }}"
            );
        } else {
            assert!(
                matches!(
                    events.first(),
                    Some(AgentEvent::SessionStarted {
                        provider: Provider::Agy,
                        ..
                    })
                ),
                "{name}: established session must start with SessionStarted(provider=agy)"
            );
        }

        // 3. Turn lifecycle pairing & 1-based monotonic numbering
        let mut active_turn: Option<u32> = None;
        let mut last_turn: u32 = 0;
        let mut open_tools: BTreeSet<String> = BTreeSet::new();
        let mut open_requests: BTreeSet<String> = BTreeSet::new();
        let mut message_deltas: BTreeMap<String, String> = BTreeMap::new();

        for (idx, event) in events.iter().enumerate() {
            match event {
                AgentEvent::TurnStarted { turn } => {
                    assert!(
                        active_turn.is_none(),
                        "{name}[{idx}]: TurnStarted({turn}) while turn {:?} is still active",
                        active_turn
                    );
                    assert_eq!(
                        *turn,
                        last_turn + 1,
                        "{name}[{idx}]: TurnStarted({turn}) is not monotonic after {last_turn}"
                    );
                    active_turn = Some(*turn);
                }
                AgentEvent::TurnCompleted { turn, .. } => {
                    assert_eq!(
                        active_turn,
                        Some(*turn),
                        "{name}[{idx}]: TurnCompleted({turn}) does not match active_turn {:?}",
                        active_turn
                    );
                    assert!(
                        open_tools.is_empty(),
                        "{name}[{idx}]: TurnCompleted({turn}) while tool calls still open: {:?}",
                        open_tools
                    );
                    last_turn = *turn;
                    active_turn = None;
                }
                AgentEvent::ToolCallStarted { call_id, .. } => {
                    assert!(
                        active_turn.is_some(),
                        "{name}[{idx}]: ToolCallStarted({call_id}) outside an active turn"
                    );
                    assert!(
                        open_tools.insert(call_id.clone()),
                        "{name}[{idx}]: duplicate ToolCallStarted({call_id})"
                    );
                }
                AgentEvent::ToolCallFinished { call_id, .. } => {
                    assert!(
                        open_tools.remove(call_id),
                        "{name}[{idx}]: ToolCallFinished({call_id}) without matching ToolCallStarted"
                    );
                }
                AgentEvent::ApprovalRequested { request_id, .. }
                | AgentEvent::QuestionAsked { request_id, .. } => {
                    assert!(
                        open_requests.insert(request_id.clone()),
                        "{name}[{idx}]: duplicate request_id {request_id}"
                    );
                }
                AgentEvent::RequestResolved { request_id, .. } => {
                    assert!(
                        open_requests.remove(request_id),
                        "{name}[{idx}]: RequestResolved({request_id}) without prior request"
                    );
                }
                AgentEvent::MessageDelta {
                    message_id, text, ..
                } => {
                    message_deltas
                        .entry(message_id.clone())
                        .or_default()
                        .push_str(text);
                }
                AgentEvent::MessageCompleted {
                    message_id, text, ..
                } => {
                    if let Some(accumulated) = message_deltas.remove(message_id) {
                        assert_eq!(
                            &accumulated, text,
                            "{name}[{idx}]: MessageCompleted({message_id}) text does not match accumulated deltas"
                        );
                    }
                }
                _ => {}
            }
        }

        assert!(
            active_turn.is_none(),
            "{name}: turn {:?} left open at SessionEnded",
            active_turn
        );
        assert!(
            open_tools.is_empty(),
            "{name}: tools {:?} left open at SessionEnded",
            open_tools
        );
        assert!(
            open_requests.is_empty(),
            "{name}: requests {:?} left open at SessionEnded",
            open_requests
        );
        assert!(
            message_deltas.is_empty(),
            "{name}: message deltas {:?} never completed",
            message_deltas.keys().collect::<Vec<_>>()
        );
    }
}
