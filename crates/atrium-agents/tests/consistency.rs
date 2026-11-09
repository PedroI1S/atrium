use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use atrium_protocol::agent::{
    AgentEvent, MessageBody, MessageKind, Provider, RequestOutcome, ToolKind, ToolStatus,
    TurnOutcome,
};
use atrium_protocol::policy::EnforcementDescriptor;
use sha2::{Digest, Sha256};

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

/// Restricts fixture discovery to `fixtures/agents/{claude,codex}` (M1-12).
pub fn find_fixtures(agents_root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_fixtures(&agents_root.join("claude"), &mut out);
    collect_fixtures(&agents_root.join("codex"), &mut out);
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

    let file_stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .trim_end_matches(".events");

    let is_gate_deny = file_stem.contains("gate-deny");
    if is_gate_deny {
        for canary in CANARIES {
            if content.contains(canary) {
                report.add(format!(
                    "Scenario '{file_stem}' leaked canary secret in events: {canary}"
                ));
            }
        }
    }

    let mut event_report = check_events(&events, file_stem);
    report.violations.append(&mut event_report.violations);
    report
}

#[derive(Default)]
struct MessageDeltasTracker {
    text: String,
    delta_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ClosureStage {
    MessagesCompleted = 0,
    ToolCallsCancelled = 1,
    RequestsCancelled = 2,
    TurnCompleted = 3,
}

fn is_closing_candidate(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::MessageCompleted { .. }
            | AgentEvent::ToolCallFinished {
                status: ToolStatus::Cancelled,
                ..
            }
            | AgentEvent::RequestResolved {
                outcome: RequestOutcome::Cancelled,
                ..
            }
            | AgentEvent::TurnCompleted { .. }
    )
}

pub fn check_events(events: &[AgentEvent], file_stem: &str) -> ConsistencyReport {
    let mut report = ConsistencyReport::default();

    if events.is_empty() {
        report.add("Fixture contains no events");
        return report;
    }

    let is_gate_deny = file_stem.contains("gate-deny");
    let is_approval_deny = file_stem.contains("approval-deny");
    let is_interrupt = file_stem.contains("interrupt");

    // Classification:
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

    // Tool call tracking
    let mut tool_starts: HashMap<String, usize> = HashMap::new();
    let mut tool_finishes: HashMap<String, usize> = HashMap::new();
    let mut tool_status: HashMap<String, ToolStatus> = HashMap::new();
    let mut finished_tool_calls: HashMap<String, usize> = HashMap::new();

    // Message tracking
    let mut message_deltas: BTreeMap<String, MessageDeltasTracker> = BTreeMap::new();
    let mut completed_messages: HashMap<String, usize> = HashMap::new();

    // Requests tracking
    let mut requests_asked: HashMap<String, (&'static str, usize)> = HashMap::new();
    let mut requests_asked_counts: HashMap<String, usize> = HashMap::new();
    let mut requests_resolved: HashMap<String, usize> = HashMap::new();

    // Turns and usage
    let mut current_turn: Option<u32> = None;
    let mut last_turn_completed: Option<u32> = None;
    let mut turn_outcomes: Vec<(u32, TurnOutcome)> = Vec::new();

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
            }
            AgentEvent::MessageDelta {
                message_id, text, ..
            } => {
                if completed_messages.contains_key(message_id) {
                    report.add(format!(
                        "Event {idx}: MessageDelta for '{message_id}' after MessageCompleted"
                    ));
                }
                let tracker = message_deltas.entry(message_id.clone()).or_default();
                tracker.text.push_str(text);
                tracker.delta_count += 1;
            }
            AgentEvent::MessageCompleted {
                message_id, body, ..
            } => {
                *completed_messages.entry(message_id.clone()).or_insert(0) += 1;

                match body {
                    MessageBody::Inline { text } => {
                        if message_deltas.contains_key(message_id) {
                            report.add(format!(
                                "Event {idx}: MessageCompleted(inline) for '{message_id}' but deltas were emitted"
                            ));
                        } else if text.len() > atrium_protocol::limits::MAX_INLINE_MESSAGE_BYTES {
                            report.add(format!(
                                "Event {idx}: MessageCompleted(inline) for '{message_id}' exceeds MAX_INLINE_MESSAGE_BYTES ({})",
                                text.len()
                            ));
                        }
                    }
                    MessageBody::Streamed {
                        chars,
                        deltas,
                        sha256,
                    } => match message_deltas.get(message_id) {
                        None => {
                            report.add(format!(
                                "Event {idx}: MessageCompleted(streamed) for '{message_id}' without deltas"
                            ));
                        }
                        Some(tracker) => {
                            let actual_chars = tracker.text.chars().count() as u64;
                            if *chars != actual_chars {
                                report.add(format!(
                                    "Event {idx}: MessageCompleted chars mismatch for '{message_id}': expected {actual_chars}, found {chars}"
                                ));
                            }
                            if *deltas != tracker.delta_count {
                                report.add(format!(
                                    "Event {idx}: MessageCompleted deltas mismatch for '{message_id}': expected {}, found {deltas}",
                                    tracker.delta_count
                                ));
                            }
                            let mut hasher = Sha256::new();
                            hasher.update(tracker.text.as_bytes());
                            let actual_sha256 = format!("{:x}", hasher.finalize());
                            if sha256 != &actual_sha256 {
                                report.add(format!(
                                    "Event {idx}: MessageCompleted sha256 mismatch for '{message_id}': expected {actual_sha256}, found {sha256}"
                                ));
                            }
                        }
                    },
                }
            }
            AgentEvent::ApprovalRequested { request_id, .. } => {
                let count = requests_asked_counts.entry(request_id.clone()).or_insert(0);
                *count += 1;
                if *count > 1 {
                    report.add(format!(
                        "Event {idx}: Duplicate ApprovalRequested id '{request_id}'"
                    ));
                } else {
                    requests_asked.insert(request_id.clone(), ("ApprovalRequested", idx));
                }
            }
            AgentEvent::QuestionAsked { request_id, .. } => {
                let count = requests_asked_counts.entry(request_id.clone()).or_insert(0);
                *count += 1;
                if *count > 1 {
                    report.add(format!(
                        "Event {idx}: Duplicate QuestionAsked id '{request_id}'"
                    ));
                } else {
                    requests_asked.insert(request_id.clone(), ("QuestionAsked", idx));
                }
            }
            AgentEvent::RequestResolved {
                request_id,
                outcome: _,
            } => {
                let res_count = requests_resolved.entry(request_id.clone()).or_insert(0);
                *res_count += 1;
                if *res_count > 1 {
                    report.add(format!(
                        "Event {idx}: Duplicate RequestResolved for request_id '{request_id}'"
                    ));
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
    for (req_id, (kind, _)) in &requests_asked {
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

    // Check no unclosed turns at SessionEnded
    if let Some(open) = current_turn {
        report.add(format!("Session ended while turn {open} was still open"));
    }

    // Check closing order only for the closing run when a session ends (agent.rs lifecycle).
    // The closing run consists of events between the last non-closing event and SessionEnded:
    // messages completed -> cancelled tools -> cancelled requests -> turn completed -> session ended.
    if matches!(events.last(), Some(AgentEvent::SessionEnded { .. })) {
        let session_ended_idx = events.len() - 1;
        let last_non_closing_idx = events[..session_ended_idx]
            .iter()
            .rposition(|e| !is_closing_candidate(e));
        let closing_start = match last_non_closing_idx {
            Some(idx) => idx + 1,
            None => 0,
        };

        let mut max_stage: Option<(ClosureStage, usize, String)> = None;

        for (offset, event) in events[closing_start..session_ended_idx].iter().enumerate() {
            let idx = closing_start + offset;
            let (stage, desc) = match event {
                AgentEvent::MessageCompleted { message_id, .. } => (
                    ClosureStage::MessagesCompleted,
                    format!("MessageCompleted for '{message_id}'"),
                ),
                AgentEvent::ToolCallFinished {
                    call_id,
                    status: ToolStatus::Cancelled,
                    ..
                } => (
                    ClosureStage::ToolCallsCancelled,
                    format!("ToolCallFinished(cancelled) for '{call_id}'"),
                ),
                AgentEvent::RequestResolved {
                    request_id,
                    outcome: RequestOutcome::Cancelled,
                    ..
                } => (
                    ClosureStage::RequestsCancelled,
                    format!("RequestResolved(cancelled) for '{request_id}'"),
                ),
                AgentEvent::TurnCompleted { turn, .. } => (
                    ClosureStage::TurnCompleted,
                    format!("TurnCompleted({turn})"),
                ),
                _ => unreachable!("closing run only contains closing candidates"),
            };

            if let Some((prev_stage, prev_idx, prev_name)) = &max_stage
                && stage < *prev_stage
            {
                let reason = match (*prev_stage, stage) {
                    (ClosureStage::RequestsCancelled, ClosureStage::ToolCallsCancelled) => {
                        ": tool calls before requests"
                    }
                    (ClosureStage::TurnCompleted, ClosureStage::RequestsCancelled) => {
                        ": requests before turn completion"
                    }
                    (ClosureStage::TurnCompleted, ClosureStage::ToolCallsCancelled) => {
                        ": tool calls before turn completion"
                    }
                    (_, ClosureStage::MessagesCompleted) => ": messages before cancellations",
                    _ => "",
                };
                report.add(format!(
                    "Event {idx}: {desc} appeared after {prev_name} at event {prev_idx} in closing run (violates closing order{reason})"
                ));
            }

            // Never lower the stage: use max
            if max_stage.as_ref().is_none_or(|(s, _, _)| stage >= *s) {
                max_stage = Some((stage, idx, desc));
            }
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_descriptor() -> EnforcementDescriptor {
        EnforcementDescriptor {
            level: atrium_protocol::policy::EnforcementLevel::Full,
            reasons: vec![],
            provider: Provider::Claude,
            cli_version: Some("1.0.0".into()),
            version_tested: true,
            coverage: vec![],
            config_sources: vec![],
        }
    }

    #[test]
    fn test_streamed_message_sha256_mismatch() {
        let events = vec![
            AgentEvent::SessionStarted {
                provider: Provider::Claude,
                native_session_id: "s1".into(),
                model: None,
                tools: vec![],
                enforcement: dummy_descriptor(),
            },
            AgentEvent::TurnStarted { turn: 1 },
            AgentEvent::MessageDelta {
                message_id: "m1".into(),
                kind: MessageKind::Assistant,
                text: "hello world".into(),
            },
            AgentEvent::MessageCompleted {
                message_id: "m1".into(),
                kind: MessageKind::Assistant,
                body: MessageBody::Streamed {
                    chars: 11,
                    deltas: 1,
                    sha256: "0000000000000000000000000000000000000000000000000000000000000000"
                        .into(),
                },
            },
            AgentEvent::TurnCompleted {
                turn: 1,
                outcome: TurnOutcome::Completed,
                usage: None,
            },
            AgentEvent::SessionEnded {
                exit_code: Some(0),
                reason: None,
            },
        ];
        let report = check_events(&events, "simple");
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.contains("sha256 mismatch")),
            "Expected sha256 mismatch violation, got: {:?}",
            report.violations
        );
    }

    #[test]
    fn test_streamed_message_deltas_count_mismatch() {
        let events = vec![
            AgentEvent::SessionStarted {
                provider: Provider::Claude,
                native_session_id: "s1".into(),
                model: None,
                tools: vec![],
                enforcement: dummy_descriptor(),
            },
            AgentEvent::TurnStarted { turn: 1 },
            AgentEvent::MessageDelta {
                message_id: "m1".into(),
                kind: MessageKind::Assistant,
                text: "hello".into(),
            },
            AgentEvent::MessageDelta {
                message_id: "m1".into(),
                kind: MessageKind::Assistant,
                text: " world".into(),
            },
            AgentEvent::MessageCompleted {
                message_id: "m1".into(),
                kind: MessageKind::Assistant,
                body: MessageBody::Streamed {
                    chars: 11,
                    deltas: 1, // Actually 2 deltas
                    sha256: "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
                        .into(),
                },
            },
            AgentEvent::TurnCompleted {
                turn: 1,
                outcome: TurnOutcome::Completed,
                usage: None,
            },
            AgentEvent::SessionEnded {
                exit_code: Some(0),
                reason: None,
            },
        ];
        let report = check_events(&events, "simple");
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.contains("deltas mismatch")),
            "Expected deltas mismatch violation, got: {:?}",
            report.violations
        );
    }

    #[test]
    fn test_streamed_message_chars_mismatch() {
        let events = vec![
            AgentEvent::SessionStarted {
                provider: Provider::Claude,
                native_session_id: "s1".into(),
                model: None,
                tools: vec![],
                enforcement: dummy_descriptor(),
            },
            AgentEvent::TurnStarted { turn: 1 },
            AgentEvent::MessageDelta {
                message_id: "m1".into(),
                kind: MessageKind::Assistant,
                text: "hello".into(),
            },
            AgentEvent::MessageCompleted {
                message_id: "m1".into(),
                kind: MessageKind::Assistant,
                body: MessageBody::Streamed {
                    chars: 99, // Actually 5 chars
                    deltas: 1,
                    sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
                        .into(),
                },
            },
            AgentEvent::TurnCompleted {
                turn: 1,
                outcome: TurnOutcome::Completed,
                usage: None,
            },
            AgentEvent::SessionEnded {
                exit_code: Some(0),
                reason: None,
            },
        ];
        let report = check_events(&events, "simple");
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.contains("chars mismatch")),
            "Expected chars mismatch violation, got: {:?}",
            report.violations
        );
    }

    #[test]
    fn test_duplicate_approval_requested_id() {
        let events = vec![
            AgentEvent::SessionStarted {
                provider: Provider::Claude,
                native_session_id: "s1".into(),
                model: None,
                tools: vec![],
                enforcement: dummy_descriptor(),
            },
            AgentEvent::TurnStarted { turn: 1 },
            AgentEvent::ApprovalRequested {
                request_id: "dup_req".into(),
                call_id: None,
                name: "Bash".into(),
                kind: ToolKind::Execute,
                title: None,
                input: serde_json::json!({}),
                reason: None,
                rememberable: false,
            },
            AgentEvent::ApprovalRequested {
                request_id: "dup_req".into(),
                call_id: None,
                name: "Bash".into(),
                kind: ToolKind::Execute,
                title: None,
                input: serde_json::json!({}),
                reason: None,
                rememberable: false,
            },
            AgentEvent::RequestResolved {
                request_id: "dup_req".into(),
                outcome: RequestOutcome::Approved,
            },
            AgentEvent::TurnCompleted {
                turn: 1,
                outcome: TurnOutcome::Completed,
                usage: None,
            },
            AgentEvent::SessionEnded {
                exit_code: Some(0),
                reason: None,
            },
        ];
        let report = check_events(&events, "simple");
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.contains("Duplicate ApprovalRequested id 'dup_req'")),
            "Expected duplicate approval id violation, got: {:?}",
            report.violations
        );
    }

    #[test]
    fn test_duplicate_question_asked_id() {
        let events = vec![
            AgentEvent::SessionStarted {
                provider: Provider::Claude,
                native_session_id: "s1".into(),
                model: None,
                tools: vec![],
                enforcement: dummy_descriptor(),
            },
            AgentEvent::TurnStarted { turn: 1 },
            AgentEvent::QuestionAsked {
                request_id: "dup_q".into(),
                questions: vec![],
            },
            AgentEvent::QuestionAsked {
                request_id: "dup_q".into(),
                questions: vec![],
            },
            AgentEvent::RequestResolved {
                request_id: "dup_q".into(),
                outcome: RequestOutcome::Answered,
            },
            AgentEvent::TurnCompleted {
                turn: 1,
                outcome: TurnOutcome::Completed,
                usage: None,
            },
            AgentEvent::SessionEnded {
                exit_code: Some(0),
                reason: None,
            },
        ];
        let report = check_events(&events, "simple");
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.contains("Duplicate QuestionAsked id 'dup_q'")),
            "Expected duplicate question id violation, got: {:?}",
            report.violations
        );
    }

    #[test]
    fn test_duplicate_request_resolved() {
        let events = vec![
            AgentEvent::SessionStarted {
                provider: Provider::Claude,
                native_session_id: "s1".into(),
                model: None,
                tools: vec![],
                enforcement: dummy_descriptor(),
            },
            AgentEvent::TurnStarted { turn: 1 },
            AgentEvent::ApprovalRequested {
                request_id: "req1".into(),
                call_id: None,
                name: "Bash".into(),
                kind: ToolKind::Execute,
                title: None,
                input: serde_json::json!({}),
                reason: None,
                rememberable: false,
            },
            AgentEvent::RequestResolved {
                request_id: "req1".into(),
                outcome: RequestOutcome::Approved,
            },
            AgentEvent::RequestResolved {
                request_id: "req1".into(),
                outcome: RequestOutcome::Approved,
            },
            AgentEvent::TurnCompleted {
                turn: 1,
                outcome: TurnOutcome::Completed,
                usage: None,
            },
            AgentEvent::SessionEnded {
                exit_code: Some(0),
                reason: None,
            },
        ];
        let report = check_events(&events, "simple");
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.contains("Duplicate RequestResolved for request_id 'req1'")),
            "Expected duplicate resolution violation, got: {:?}",
            report.violations
        );
    }

    #[test]
    fn test_closing_order_tool_cancelled_after_request_cancelled() {
        let events = vec![
            AgentEvent::SessionStarted {
                provider: Provider::Claude,
                native_session_id: "s1".into(),
                model: None,
                tools: vec![],
                enforcement: dummy_descriptor(),
            },
            AgentEvent::TurnStarted { turn: 1 },
            AgentEvent::ToolCallStarted {
                call_id: "c1".into(),
                name: "Bash".into(),
                kind: ToolKind::Execute,
                title: None,
                input: serde_json::json!({}),
            },
            AgentEvent::ApprovalRequested {
                request_id: "req1".into(),
                call_id: Some("c1".into()),
                name: "Bash".into(),
                kind: ToolKind::Execute,
                title: None,
                input: serde_json::json!({}),
                reason: None,
                rememberable: false,
            },
            // BAD ORDER: RequestResolved(cancelled) before ToolCallFinished(cancelled)
            AgentEvent::RequestResolved {
                request_id: "req1".into(),
                outcome: RequestOutcome::Cancelled,
            },
            AgentEvent::ToolCallFinished {
                call_id: "c1".into(),
                status: ToolStatus::Cancelled,
                output: None,
                error: None,
                truncated_bytes: 0,
            },
            AgentEvent::TurnCompleted {
                turn: 1,
                outcome: TurnOutcome::Interrupted,
                usage: None,
            },
            AgentEvent::SessionEnded {
                exit_code: Some(1),
                reason: Some("interrupted".into()),
            },
        ];
        let report = check_events(&events, "interrupt");
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.contains("violates closing order: tool calls before requests")),
            "Expected closing order violation, got: {:?}",
            report.violations
        );
    }

    #[test]
    fn test_valid_mid_turn_cancel_followed_by_more_output_passes() {
        let events = vec![
            AgentEvent::SessionStarted {
                provider: Provider::Claude,
                native_session_id: "s1".into(),
                model: None,
                tools: vec![],
                enforcement: dummy_descriptor(),
            },
            AgentEvent::TurnStarted { turn: 1 },
            AgentEvent::ToolCallStarted {
                call_id: "c1".into(),
                name: "Bash".into(),
                kind: ToolKind::Execute,
                title: None,
                input: serde_json::json!({}),
            },
            // Mid-turn cancel of tool call c1
            AgentEvent::ToolCallFinished {
                call_id: "c1".into(),
                status: ToolStatus::Cancelled,
                output: None,
                error: None,
                truncated_bytes: 0,
            },
            // More output follows in the same turn: message delta and completion
            AgentEvent::MessageDelta {
                message_id: "m1".into(),
                kind: MessageKind::Assistant,
                text: "Tool cancelled, trying alternative approach.".into(),
            },
            AgentEvent::MessageCompleted {
                message_id: "m1".into(),
                kind: MessageKind::Assistant,
                body: MessageBody::Streamed {
                    chars: 44,
                    deltas: 1,
                    sha256: "8805f635676755ec50eeb557348eebf995804368a4d70da67285a225674c3e80"
                        .into(),
                },
            },
            AgentEvent::TurnCompleted {
                turn: 1,
                outcome: TurnOutcome::Completed,
                usage: None,
            },
            AgentEvent::SessionEnded {
                exit_code: Some(0),
                reason: None,
            },
        ];
        let report = check_events(&events, "simple");
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.contains("violates closing order")),
            "Expected NO closing order violation for valid mid-turn cancel followed by output, got: {:?}",
            report.violations
        );
    }

    #[test]
    fn test_bad_closing_order_before_session_ended_fails() {
        let events = vec![
            AgentEvent::SessionStarted {
                provider: Provider::Claude,
                native_session_id: "s1".into(),
                model: None,
                tools: vec![],
                enforcement: dummy_descriptor(),
            },
            AgentEvent::TurnStarted { turn: 1 },
            AgentEvent::ToolCallStarted {
                call_id: "c1".into(),
                name: "Bash".into(),
                kind: ToolKind::Execute,
                title: None,
                input: serde_json::json!({}),
            },
            // Closing run before SessionEnded: ToolCallFinished(cancelled) at event 3
            AgentEvent::ToolCallFinished {
                call_id: "c1".into(),
                status: ToolStatus::Cancelled,
                output: None,
                error: None,
                truncated_bytes: 0,
            },
            // BAD ORDER: MessageCompleted at event 4 appeared after ToolCallFinished(cancelled) at event 3
            AgentEvent::MessageCompleted {
                message_id: "m1".into(),
                kind: MessageKind::Assistant,
                body: MessageBody::Inline {
                    text: "Closing message".into(),
                },
            },
            AgentEvent::TurnCompleted {
                turn: 1,
                outcome: TurnOutcome::Interrupted,
                usage: None,
            },
            AgentEvent::SessionEnded {
                exit_code: Some(1),
                reason: Some("interrupted".into()),
            },
        ];
        let report = check_events(&events, "interrupt");
        let violation = report
            .violations
            .iter()
            .find(|v| v.contains("violates closing order: messages before cancellations"))
            .expect("Expected closing order violation for MessageCompleted after ToolCallFinished(cancelled)");
        assert!(
            violation.contains("Event 4: MessageCompleted for 'm1' appeared after ToolCallFinished(cancelled) for 'c1' at event 3"),
            "Expected exact indices in violation message, got: {violation}"
        );
    }

    #[test]
    fn test_closing_order_request_resolved_cancelled_after_turn_completed() {
        let events = vec![
            AgentEvent::SessionStarted {
                provider: Provider::Claude,
                native_session_id: "s1".into(),
                model: None,
                tools: vec![],
                enforcement: dummy_descriptor(),
            },
            AgentEvent::TurnStarted { turn: 1 },
            AgentEvent::ApprovalRequested {
                request_id: "req1".into(),
                call_id: None,
                name: "Bash".into(),
                kind: ToolKind::Execute,
                title: None,
                input: serde_json::json!({}),
                reason: None,
                rememberable: false,
            },
            AgentEvent::ApprovalRequested {
                request_id: "req2".into(),
                call_id: None,
                name: "Bash".into(),
                kind: ToolKind::Execute,
                title: None,
                input: serde_json::json!({}),
                reason: None,
                rememberable: false,
            },
            // TurnCompleted occurs at event index 4
            AgentEvent::TurnCompleted {
                turn: 1,
                outcome: TurnOutcome::Interrupted,
                usage: None,
            },
            // BAD ORDER: RequestResolved(cancelled) at event index 5 after TurnCompleted at index 4
            AgentEvent::RequestResolved {
                request_id: "req1".into(),
                outcome: RequestOutcome::Cancelled,
            },
            // Another RequestResolved(cancelled) at event index 6: stage must not lower,
            // so this also flags against TurnCompleted at event index 4!
            AgentEvent::RequestResolved {
                request_id: "req2".into(),
                outcome: RequestOutcome::Cancelled,
            },
            AgentEvent::SessionEnded {
                exit_code: Some(1),
                reason: Some("interrupted".into()),
            },
        ];
        let report = check_events(&events, "interrupt");
        let v1 = report
            .violations
            .iter()
            .find(|v| v.contains("Event 5: RequestResolved(cancelled) for 'req1' appeared after TurnCompleted(1) at event 4"))
            .expect("Expected first violation referencing TurnCompleted at event 4");
        assert!(
            v1.contains("violates closing order: requests before turn completion"),
            "Expected requests before turn completion reason, got: {v1}"
        );

        let v2 = report
            .violations
            .iter()
            .find(|v| v.contains("Event 6: RequestResolved(cancelled) for 'req2' appeared after TurnCompleted(1) at event 4"))
            .expect("Expected second violation still referencing TurnCompleted at event 4 (stage was not lowered)");
        assert!(
            v2.contains("violates closing order: requests before turn completion"),
            "Expected requests before turn completion reason, got: {v2}"
        );
    }

    #[test]
    fn test_closing_order_per_closure_episode_resets_on_new_turn() {
        let events = vec![
            AgentEvent::SessionStarted {
                provider: Provider::Claude,
                native_session_id: "s1".into(),
                model: None,
                tools: vec![],
                enforcement: dummy_descriptor(),
            },
            // Turn 1: has a cancelled request during turn 1 closure
            AgentEvent::TurnStarted { turn: 1 },
            AgentEvent::ApprovalRequested {
                request_id: "req1".into(),
                call_id: None,
                name: "Bash".into(),
                kind: ToolKind::Execute,
                title: None,
                input: serde_json::json!({}),
                reason: None,
                rememberable: false,
            },
            AgentEvent::RequestResolved {
                request_id: "req1".into(),
                outcome: RequestOutcome::Cancelled,
            },
            AgentEvent::TurnCompleted {
                turn: 1,
                outcome: TurnOutcome::Interrupted,
                usage: None,
            },
            // Turn 2: starts fresh. Tool call is started and cancelled.
            // This should NOT trigger a violation from Turn 1's cancelled request!
            AgentEvent::TurnStarted { turn: 2 },
            AgentEvent::ToolCallStarted {
                call_id: "c2".into(),
                name: "Bash".into(),
                kind: ToolKind::Execute,
                title: None,
                input: serde_json::json!({}),
            },
            AgentEvent::ToolCallFinished {
                call_id: "c2".into(),
                status: ToolStatus::Cancelled,
                output: None,
                error: None,
                truncated_bytes: 0,
            },
            AgentEvent::TurnCompleted {
                turn: 2,
                outcome: TurnOutcome::Interrupted,
                usage: None,
            },
            AgentEvent::SessionEnded {
                exit_code: Some(0),
                reason: None,
            },
        ];
        let report = check_events(&events, "interrupt");
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.contains("violates closing order")),
            "Expected NO closing order violation across separate turns, got: {:?}",
            report.violations
        );
    }
}

#[test]
#[ignore = "M1-07, M1-08: fixtures regenerate for rev 3"]
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
