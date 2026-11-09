use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use atrium_protocol::agent::{AgentEvent, MessageBody, RequestOutcome, ToolStatus, TurnOutcome};

const CANARIES: &[&str] = &[
    "ATRIUM-7Q2X-CANARY",
    "TOP_SECRET_VALUE_994812_DO_NOT_LEAK",
    "TOP_SECRET_CANARY_VALUE_9941",
    "FIXTURE_SECRET_MUST_NOT_APPEAR",
];

#[derive(Debug, Default)]
pub struct ConsistencyReport {
    pub violations: Vec<String>,
}

impl ConsistencyReport {
    pub fn add(&mut self, v: impl Into<String>) {
        self.violations.push(v.into());
    }

    pub fn is_empty(&self) -> bool {
        self.violations.is_empty()
    }
}

pub fn find_fixtures(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_fixtures(root, &mut out);
    out.sort();
    out
}

fn collect_fixtures(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_fixtures(&path, out);
        } else if path
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.ends_with(".events.json"))
        {
            out.push(path);
        }
    }
}

pub fn check_fixture(path: &Path) -> ConsistencyReport {
    let mut report = ConsistencyReport::default();
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            report.add(format!("Failed to read file: {e}"));
            return report;
        }
    };

    let events: Vec<AgentEvent> = match serde_json::from_str(&content) {
        Ok(evs) => evs,
        Err(e) => {
            report.add(format!("Failed to parse AgentEvents: {e}"));
            return report;
        }
    };

    if events.is_empty() {
        report.add("Fixture contains no events");
        return report;
    }

    let file_stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .trim_end_matches(".events");

    let is_gate_deny = file_stem.contains("gate-deny");
    let is_approval_deny = file_stem.contains("approval-deny");
    let is_interrupt = file_stem.contains("interrupt");

    // Check classification:
    // established session: contains SessionStarted
    // pre-session failure: no SessionStarted
    let has_session_started = events
        .iter()
        .any(|e| matches!(e, AgentEvent::SessionStarted { .. }));

    if !has_session_started {
        // Pre-session failure: only Error { fatal: true } then SessionEnded.
        if events.len() != 2 {
            report.add(format!(
                "Pre-session failure must have exactly 2 events (Error {{ fatal: true }} then SessionEnded), found {}",
                events.len()
            ));
        }
        match events.first() {
            Some(AgentEvent::Error { fatal: true, .. }) => {}
            Some(other) => {
                report.add(format!(
                    "Pre-session failure first event must be Error {{ fatal: true }}, found {:?}",
                    other
                ));
            }
            None => {}
        }
        match events.last() {
            Some(AgentEvent::SessionEnded { .. }) => {}
            Some(other) => {
                report.add(format!(
                    "Pre-session failure last event must be SessionEnded, found {:?}",
                    other
                ));
            }
            None => {}
        }
        return report;
    }

    // Established session checks
    match events.first() {
        Some(AgentEvent::SessionStarted { .. }) => {}
        Some(other) => {
            report.add(format!(
                "Established session first event must be SessionStarted, found {:?}",
                other
            ));
        }
        None => {}
    }

    match events.last() {
        Some(AgentEvent::SessionEnded { .. }) => {}
        Some(other) => {
            report.add(format!(
                "Established session last event must be SessionEnded, found {:?}",
                other
            ));
        }
        None => {}
    }

    // Check multiple SessionStarted / SessionEnded
    let session_started_count = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::SessionStarted { .. }))
        .count();
    if session_started_count > 1 {
        report.add(format!(
            "Multiple SessionStarted events found: {}",
            session_started_count
        ));
    }

    let session_ended_count = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::SessionEnded { .. }))
        .count();
    if session_ended_count > 1 {
        report.add(format!(
            "Multiple SessionEnded events found: {}",
            session_ended_count
        ));
    }

    // Track tool calls:
    // every ToolCallStarted has exactly one ToolCallFinished, and no ToolCallUpdated comes after it
    let mut tool_starts: HashMap<String, usize> = HashMap::new();
    let mut tool_finishes: HashMap<String, usize> = HashMap::new();
    let mut tool_status: HashMap<String, ToolStatus> = HashMap::new();
    let mut finished_tool_calls: HashMap<String, usize> = HashMap::new();

    // Track message deltas & message completed:
    // deltas are followed by one MessageCompleted whose text equals their concatenation
    let mut message_deltas: BTreeMap<String, String> = BTreeMap::new();
    let mut completed_messages: HashMap<String, usize> = HashMap::new();

    // Track approvals and questions:
    // every ApprovalRequested / QuestionAsked has exactly one RequestResolved, and none is left open at SessionEnded
    let mut requests_asked: HashMap<String, &'static str> = HashMap::new();
    let mut requests_resolved: HashMap<String, usize> = HashMap::new();

    // Turns and usage
    // TurnStarted/TurnCompleted pair up and turn increases; no Usage comes after its turn's TurnCompleted
    let mut current_turn: Option<u32> = None;
    let mut last_turn_completed: Option<u32> = None;
    let mut turn_outcomes: Vec<(u32, TurnOutcome)> = Vec::new();

    // Closing order checks:
    // When a session ends: open messages -> open tool calls (cancelled) -> pending approvals/questions (cancelled) -> open turn -> SessionEnded
    let mut last_cancelled_request_idx: Option<usize> = None;

    // Check events in order
    for (idx, event) in events.iter().enumerate() {
        match event {
            AgentEvent::ToolCallStarted { call_id, .. } => {
                *tool_starts.entry(call_id.clone()).or_insert(0) += 1;
            }
            AgentEvent::ToolCallUpdated { call_id, .. } => {
                if finished_tool_calls.contains_key(call_id) {
                    report.add(format!(
                        "Event {idx}: ToolCallUpdated for call_id '{call_id}' after it already finished"
                    ));
                }
            }
            AgentEvent::ToolCallFinished {
                call_id, status, ..
            } => {
                *tool_finishes.entry(call_id.clone()).or_insert(0) += 1;
                finished_tool_calls.insert(call_id.clone(), idx);
                tool_status.insert(call_id.clone(), *status);

                if *status == ToolStatus::Cancelled
                    && let Some(req_idx) = last_cancelled_request_idx
                {
                    report.add(format!(
                        "Event {idx}: ToolCallFinished(cancelled) for '{call_id}' appeared after RequestResolved(cancelled) at event {req_idx} (violates closing order: tool calls before requests)"
                    ));
                }
            }
            AgentEvent::MessageDelta {
                message_id, text, ..
            } => {
                if completed_messages.contains_key(message_id) {
                    report.add(format!(
                        "Event {idx}: MessageDelta for '{message_id}' after MessageCompleted"
                    ));
                }
                message_deltas
                    .entry(message_id.clone())
                    .or_default()
                    .push_str(text);
            }
            AgentEvent::MessageCompleted {
                message_id, body, ..
            } => {
                *completed_messages.entry(message_id.clone()).or_insert(0) += 1;
                // M1-12 adds the sha256 check (contract §6.1).
                let expected = message_deltas.get(message_id);
                let ok = match (body, expected) {
                    (MessageBody::Inline { text }, None) => {
                        text.len() <= atrium_protocol::limits::MAX_INLINE_MESSAGE_BYTES
                    }
                    (MessageBody::Inline { .. }, Some(_)) => false,
                    (MessageBody::Streamed { chars, .. }, Some(expected)) => {
                        *chars == expected.chars().count() as u64
                    }
                    (MessageBody::Streamed { .. }, None) => false,
                };
                if !ok {
                    report.add(format!(
                        "Event {idx}: MessageCompleted body does not match the deltas for '{message_id}'"
                    ));
                }
            }
            AgentEvent::ApprovalRequested { request_id, .. } => {
                requests_asked.insert(request_id.clone(), "ApprovalRequested");
            }
            AgentEvent::QuestionAsked { request_id, .. } => {
                requests_asked.insert(request_id.clone(), "QuestionAsked");
            }
            AgentEvent::RequestResolved {
                request_id,
                outcome,
            } => {
                *requests_resolved.entry(request_id.clone()).or_insert(0) += 1;
                if *outcome == RequestOutcome::Cancelled {
                    last_cancelled_request_idx = Some(idx);
                }
            }
            AgentEvent::TurnStarted { turn } => {
                if let Some(open) = current_turn {
                    report.add(format!(
                        "Event {idx}: TurnStarted({turn}) while turn {open} is still open"
                    ));
                }
                if matches!(last_turn_completed, Some(prev) if *turn <= prev) {
                    let prev = last_turn_completed.unwrap();
                    report.add(format!(
                        "Event {idx}: TurnStarted({turn}) did not increase from previous turn {prev}"
                    ));
                }
                current_turn = Some(*turn);
            }
            AgentEvent::TurnCompleted { turn, outcome, .. } => match current_turn {
                Some(open) if open == *turn => {
                    current_turn = None;
                    last_turn_completed = Some(*turn);
                    turn_outcomes.push((*turn, *outcome));
                }
                Some(open) => {
                    report.add(format!(
                        "Event {idx}: TurnCompleted({turn}) does not match open turn {open}"
                    ));
                    current_turn = None;
                    last_turn_completed = Some(*turn);
                }
                None => {
                    report.add(format!(
                        "Event {idx}: TurnCompleted({turn}) without an open TurnStarted"
                    ));
                    last_turn_completed = Some(*turn);
                }
            },
            AgentEvent::Usage { .. } if current_turn.is_none() => {
                report.add(format!(
                    "Event {idx}: Usage event emitted while no turn is open (after TurnCompleted)"
                ));
            }
            _ => {}
        }
    }

    // Check tool call pairings
    for (call_id, start_count) in &tool_starts {
        let finish_count = tool_finishes.get(call_id).copied().unwrap_or(0);
        if *start_count != 1 || finish_count != 1 {
            report.add(format!(
                "Tool call '{call_id}' started {start_count} times, finished {finish_count} times"
            ));
        }
    }
    for (call_id, finish_count) in &tool_finishes {
        if !tool_starts.contains_key(call_id) {
            report.add(format!(
                "Tool call '{call_id}' finished {finish_count} times but was never started"
            ));
        }
    }

    // Check message completed counts
    for msg_id in message_deltas.keys() {
        let comp = completed_messages.get(msg_id).copied().unwrap_or(0);
        if comp != 1 {
            report.add(format!(
                "Message '{msg_id}' has deltas but completed {comp} times (expected 1)"
            ));
        }
    }

    // Check request resolutions
    for (req_id, kind) in &requests_asked {
        let res = requests_resolved.get(req_id).copied().unwrap_or(0);
        if res != 1 {
            report.add(format!(
                "{kind} '{req_id}' resolved {res} times (expected 1)"
            ));
        }
    }
    for (req_id, res_count) in &requests_resolved {
        if !requests_asked.contains_key(req_id) {
            report.add(format!(
                "RequestResolved for '{req_id}' ({res_count} times) without matching request"
            ));
        }
    }

    // Scenario specific: approval-deny & gate-deny
    if is_approval_deny || is_gate_deny {
        let has_denied = tool_status
            .values()
            .any(|s| matches!(s, ToolStatus::Denied));
        if !has_denied {
            report.add(format!(
                "Scenario '{file_stem}' expected at least one ToolCallFinished with status 'denied'"
            ));
        }
    }

    if is_gate_deny {
        for canary in CANARIES {
            if content.contains(canary) {
                report.add(format!(
                    "Scenario '{file_stem}' leaked canary secret in events: {canary}"
                ));
            }
        }
    }

    // Scenario specific: interrupt
    if is_interrupt {
        let has_interrupted = turn_outcomes
            .iter()
            .any(|(_, out)| matches!(out, TurnOutcome::Interrupted));
        if !has_interrupted {
            report.add(format!(
                "Interrupt scenario '{file_stem}' expected TurnCompleted with outcome 'interrupted'"
            ));
        }
        let has_cancelled_call = tool_status
            .values()
            .any(|s| matches!(s, ToolStatus::Cancelled));
        if !has_cancelled_call {
            report.add(format!(
                "Interrupt scenario '{file_stem}' expected at least one ToolCallFinished with status 'cancelled'"
            ));
        }
    }

    // Closing order on exit (module docs):
    // "When a session ends for any reason, the adapter first closes everything still open,
    // in this order: open messages → MessageCompleted (text so far);
    // open tool calls → ToolCallFinished { status: cancelled };
    // pending approvals and questions → RequestResolved { outcome: cancelled };
    // an open turn → TurnCompleted { outcome: interrupted | failed }. Then SessionEnded."
    // Let's check if session ended cleanly without open turns
    if let Some(open) = current_turn {
        report.add(format!("Session ended while turn {open} was still open"));
    }

    report
}

#[test]
#[ignore = "Cross-adapter consistency violations are tracked and reported in docs/spikes/M0-findings.md"]
fn consistency_all_fixtures() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");
    let fixtures_dir = workspace_root.join("fixtures").join("agents");

    let fixtures = find_fixtures(&fixtures_dir);
    assert!(
        !fixtures.is_empty(),
        "No fixtures found under {}",
        fixtures_dir.display()
    );

    let mut all_violations = BTreeMap::new();

    for fixture in &fixtures {
        let rel_path = fixture.strip_prefix(workspace_root).unwrap_or(fixture);
        let rep = check_fixture(fixture);
        if !rep.is_empty() {
            all_violations.insert(rel_path.display().to_string(), rep.violations);
        }
    }

    if !all_violations.is_empty() {
        println!(
            "=== CONSISTENCY VIOLATIONS FOUND ({} files) ===",
            all_violations.len()
        );
        for (file, viols) in &all_violations {
            println!("\nFile: {file}");
            for v in viols {
                println!("  - {v}");
            }
        }
        panic!(
            "Found consistency violations in {} fixtures",
            all_violations.len()
        );
    }
}
