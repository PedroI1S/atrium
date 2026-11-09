//! M1-04: thread state machine, inbox ranking, notifications (contract §8).

use std::time::Duration;

use atrium_core::Signal;
use atrium_core::attention::{AttentionInput, Pending, next_state, rank_inbox, should_notify};
use atrium_protocol::agent::{
    AgentEvent, MessageBody, MessageKind, Provider, RequestOutcome, ToolKind, ToolStatus,
    TurnOutcome,
};
use atrium_protocol::limits::NOTIFY_MIN_INTERVAL;
use atrium_protocol::policy::EnforcementDescriptor;
use atrium_protocol::thread::{
    Cleanup, InboxReason, Location, ThreadId, ThreadState, ThreadSummary,
};
use serde_json::json;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn enforcement() -> EnforcementDescriptor {
    EnforcementDescriptor::evaluate(Provider::Claude, None, false, vec![], vec![], vec![])
}

fn failed() -> ThreadState {
    ThreadState::Failed {
        message: "boom".into(),
    }
}

fn paused() -> ThreadState {
    ThreadState::Paused {
        resets_at_ms: Some(5),
    }
}

/// Column order of the table below: one code per state.
const STATES: &str = "SRAQIPFXUE";

fn state_of(code: char) -> ThreadState {
    match code {
        'S' => ThreadState::Starting,
        'R' => ThreadState::Running,
        'A' => ThreadState::NeedsApproval,
        'Q' => ThreadState::NeedsInput,
        'I' => ThreadState::Idle,
        'P' => paused(),
        'F' => failed(),
        'X' => ThreadState::Interrupted {
            cleanup: Cleanup::Verified,
        },
        'U' => ThreadState::Interrupted {
            cleanup: Cleanup::Unverified,
        },
        'E' => ThreadState::Ended,
        // Row-specific results.
        'p' => ThreadState::Paused {
            resets_at_ms: Some(9),
        },
        'f' => ThreadState::Failed {
            message: "fatal".into(),
        },
        _ => panic!("bad code {code}"),
    }
}

/// What is pending in a state, for inputs that don't change requests.
fn pending_of(code: char) -> Pending {
    match code {
        'A' => Pending {
            approvals: 1,
            questions: 0,
        },
        'Q' => Pending {
            approvals: 0,
            questions: 1,
        },
        _ => Pending::default(),
    }
}

enum Input {
    Event(AgentEvent),
    Signal(Signal),
}

struct Row {
    name: &'static str,
    input: Input,
    /// `None`: whatever the starting state has pending.
    pending: Option<Pending>,
    /// One code per state in `STATES` order; `=` means unchanged.
    expect: &'static str,
}

fn ev(name: &'static str, e: AgentEvent, pending: Option<Pending>, expect: &'static str) -> Row {
    Row {
        name,
        input: Input::Event(e),
        pending,
        expect,
    }
}

fn sig(name: &'static str, s: Signal, expect: &'static str) -> Row {
    Row {
        name,
        input: Input::Signal(s),
        pending: None,
        expect,
    }
}

fn pend(approvals: u32, questions: u32) -> Option<Pending> {
    Some(Pending {
        approvals,
        questions,
    })
}

fn table() -> Vec<Row> {
    let msg_body = MessageBody::Inline { text: "hi".into() };
    //                                                          S R A Q I P F X U E
    let mut rows = vec![
        sig("Signal::Starting", Signal::Starting, "SSSSSSSSSS"),
        sig("Signal::Stopped", Signal::Stopped, "EEEEEEEEEE"),
        sig(
            "Signal::Exited(0)",
            Signal::Exited { code: Some(0) },
            "EEEEE=====",
        ),
        sig(
            "Signal::Exited(1)",
            Signal::Exited { code: Some(1) },
            "XXXXX=====",
        ),
        sig(
            "Signal::Exited(None)",
            Signal::Exited { code: None },
            "XXXXX=====",
        ),
        sig(
            "Signal::Recovered(Unverified)",
            Signal::Recovered {
                cleanup: Cleanup::Unverified,
            },
            "UUUUUU====",
        ),
        sig(
            "Signal::Recovered(Verified)",
            Signal::Recovered {
                cleanup: Cleanup::Verified,
            },
            "XXXXXX====",
        ),
        sig(
            "Signal::StorageReadOnly",
            Signal::StorageReadOnly,
            "XXXXXX====",
        ),
        ev(
            "SessionStarted",
            AgentEvent::SessionStarted {
                provider: Provider::Claude,
                native_session_id: "n".into(),
                model: None,
                tools: vec![],
                enforcement: enforcement(),
            },
            None,
            "I=========",
        ),
        ev(
            "EnforcementChanged",
            AgentEvent::EnforcementChanged {
                enforcement: enforcement(),
            },
            None,
            "==========",
        ),
        ev(
            "TurnStarted",
            AgentEvent::TurnStarted { turn: 1 },
            None,
            "RRAQRRFXUE",
        ),
        ev(
            "TurnCompleted(Completed)",
            AgentEvent::TurnCompleted {
                turn: 1,
                outcome: TurnOutcome::Completed,
                usage: None,
            },
            None,
            "SIAQIPFXUE",
        ),
        ev(
            "ApprovalRequested",
            AgentEvent::ApprovalRequested {
                request_id: "r".into(),
                call_id: None,
                name: "Bash".into(),
                kind: ToolKind::Execute,
                title: None,
                input: json!({}),
                reason: None,
                rememberable: false,
            },
            pend(1, 0),
            "AAAAAAFXUE",
        ),
        ev(
            "QuestionAsked",
            AgentEvent::QuestionAsked {
                request_id: "q".into(),
                questions: vec![],
            },
            pend(0, 1),
            "QQQQQQFXUE",
        ),
        ev(
            "QuestionAsked (approval also pending)",
            AgentEvent::QuestionAsked {
                request_id: "q".into(),
                questions: vec![],
            },
            pend(1, 1),
            "AAAAAAFXUE",
        ),
        ev(
            "RequestResolved (nothing left)",
            AgentEvent::RequestResolved {
                request_id: "r".into(),
                outcome: RequestOutcome::Approved,
            },
            pend(0, 0),
            "SRRRIPFXUE",
        ),
        ev(
            "RequestResolved (question left)",
            AgentEvent::RequestResolved {
                request_id: "r".into(),
                outcome: RequestOutcome::Approved,
            },
            pend(0, 1),
            "QQQQQQFXUE",
        ),
        ev(
            "RateLimited",
            AgentEvent::RateLimited {
                resets_at_ms: Some(9),
                message: None,
            },
            None,
            "pppppp====",
        ),
        ev(
            "Error(fatal)",
            AgentEvent::Error {
                message: "fatal".into(),
                fatal: true,
                details: vec![],
            },
            None,
            "ffffff====",
        ),
        ev(
            "Error(non-fatal)",
            AgentEvent::Error {
                message: "meh".into(),
                fatal: false,
                details: vec![],
            },
            None,
            "==========",
        ),
        ev(
            "SessionEnded(0)",
            AgentEvent::SessionEnded {
                exit_code: Some(0),
                reason: None,
            },
            None,
            "EEEEE=====",
        ),
        ev(
            "SessionEnded(2)",
            AgentEvent::SessionEnded {
                exit_code: Some(2),
                reason: None,
            },
            None,
            "XXXXX=====",
        ),
        ev(
            "SessionEnded(None)",
            AgentEvent::SessionEnded {
                exit_code: None,
                reason: None,
            },
            None,
            "XXXXX=====",
        ),
    ];

    // Events that never change the state.
    let neutral = vec![
        (
            "MessageDelta",
            AgentEvent::MessageDelta {
                message_id: "m".into(),
                kind: MessageKind::Assistant,
                text: "x".into(),
            },
        ),
        (
            "MessageCompleted",
            AgentEvent::MessageCompleted {
                message_id: "m".into(),
                kind: MessageKind::Assistant,
                body: msg_body,
            },
        ),
        (
            "ToolCallStarted",
            AgentEvent::ToolCallStarted {
                call_id: "c".into(),
                name: "Bash".into(),
                kind: ToolKind::Execute,
                title: None,
                input: json!({}),
            },
        ),
        (
            "ToolCallUpdated",
            AgentEvent::ToolCallUpdated {
                call_id: "c".into(),
                title: None,
                kind: None,
                input: None,
                output_delta: None,
                output: None,
            },
        ),
        (
            "ToolCallFinished",
            AgentEvent::ToolCallFinished {
                call_id: "c".into(),
                status: ToolStatus::Succeeded,
                output: None,
                error: None,
                truncated_bytes: 0,
            },
        ),
        ("PlanUpdated", AgentEvent::PlanUpdated { entries: vec![] }),
        (
            "ContextUpdated",
            AgentEvent::ContextUpdated {
                used_tokens: 1,
                window_tokens: None,
            },
        ),
        ("QuotaUpdated", AgentEvent::QuotaUpdated { windows: vec![] }),
        (
            "Unmapped",
            AgentEvent::Unmapped {
                record_type: "x".into(),
                raw: json!(null),
            },
        ),
    ];
    for (name, e) in neutral {
        rows.push(ev(name, e, None, "=========="));
    }
    rows
}

#[test]
fn transition_table_covers_every_state_and_input() {
    let mut mismatches = Vec::new();
    let mut cells = 0;
    for row in table() {
        assert_eq!(row.expect.len(), STATES.len(), "row {}", row.name);
        for (from_code, exp_code) in STATES.chars().zip(row.expect.chars()) {
            let from = state_of(from_code);
            let expected = if exp_code == '=' {
                from.clone()
            } else {
                state_of(exp_code)
            };
            let pending = row.pending.unwrap_or_else(|| pending_of(from_code));
            let input = match &row.input {
                Input::Event(e) => AttentionInput::Event(e),
                Input::Signal(s) => AttentionInput::Signal(s),
            };
            let got = next_state(&from, pending, input);
            cells += 1;
            if got != expected {
                mismatches.push(format!(
                    "{} from {:?}: expected {:?}, got {:?}",
                    row.name, from, expected, got
                ));
            }
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    // Every state is a column of every row.
    assert_eq!(cells, table().len() * STATES.len());
}

// ---------------------------------------------------------------------------
// Targeted rules
// ---------------------------------------------------------------------------

fn run(from: ThreadState, pending: Pending, e: AgentEvent) -> ThreadState {
    next_state(&from, pending, AttentionInput::Event(&e))
}

fn exit(from: ThreadState, code: Option<i32>) -> ThreadState {
    next_state(
        &from,
        Pending::default(),
        AttentionInput::Signal(&Signal::Exited { code }),
    )
}

fn thread(id: &str, state: ThreadState, unread: bool, since: Option<u64>) -> ThreadSummary {
    ThreadSummary {
        thread_id: ThreadId(id.into()),
        title: format!("title {id}"),
        provider: Provider::Claude,
        model: None,
        location: Location::Home,
        state,
        enforcement: None,
        incarnation: 1,
        native_session_id: None,
        last_event: None,
        unread,
        pending_approvals: 0,
        pending_questions: 0,
        attention_since_ms: since,
        attention_detail: Some(format!("detail {id}")),
        created_at_ms: 1,
        updated_at_ms: 2,
    }
}

#[test]
fn approval_outranks_question_outranks_failed() {
    // State: both pending is NeedsApproval; clearing the approval leaves the
    // question; clearing both during a turn returns to Running.
    let both = Pending {
        approvals: 1,
        questions: 1,
    };
    let q = AgentEvent::QuestionAsked {
        request_id: "q".into(),
        questions: vec![],
    };
    assert_eq!(
        run(ThreadState::Running, both, q),
        ThreadState::NeedsApproval
    );
    let resolved = AgentEvent::RequestResolved {
        request_id: "r".into(),
        outcome: RequestOutcome::Denied,
    };
    let only_q = Pending {
        approvals: 0,
        questions: 1,
    };
    assert_eq!(
        run(ThreadState::NeedsApproval, only_q, resolved.clone()),
        ThreadState::NeedsInput
    );
    assert_eq!(
        run(ThreadState::NeedsInput, Pending::default(), resolved),
        ThreadState::Running
    );

    // Inbox: storage full > approval > question > failed > cleanup > reply,
    // whatever the age or input order.
    let threads = vec![
        thread("reply", ThreadState::Idle, true, Some(1)),
        thread(
            "cleanup",
            ThreadState::Interrupted {
                cleanup: Cleanup::Unverified,
            },
            false,
            Some(2),
        ),
        thread("failed", failed(), false, Some(3)),
        thread("question", ThreadState::NeedsInput, false, Some(900)),
        thread("approval", ThreadState::NeedsApproval, false, Some(1000)),
    ];
    let items = rank_inbox(&threads, Some(5000));
    let reasons: Vec<_> = items.iter().map(|i| i.reason).collect();
    assert_eq!(
        reasons,
        vec![
            InboxReason::StorageFull,
            InboxReason::Approval,
            InboxReason::Question,
            InboxReason::Failed,
            InboxReason::CleanupUnverified,
            InboxReason::Reply,
        ]
    );
    assert_eq!(items[0].thread_id, None);
    assert_eq!(items[0].since_ms, 5000);
    assert_eq!(items[1].detail.as_deref(), Some("detail approval"));
    assert_eq!(items[1].title, "title approval");

    // Same reason: oldest first. Threads that don't need the user are left
    // out, as is a read reply.
    let threads = vec![
        thread("new", ThreadState::NeedsApproval, false, Some(20)),
        thread("old", ThreadState::NeedsApproval, false, Some(10)),
        thread("idle", ThreadState::Idle, false, None),
        thread("running", ThreadState::Running, false, None),
        thread("ended", ThreadState::Ended, false, None),
        thread(
            "verified",
            ThreadState::Interrupted {
                cleanup: Cleanup::Verified,
            },
            false,
            None,
        ),
    ];
    let items = rank_inbox(&threads, None);
    let ids: Vec<_> = items
        .iter()
        .map(|i| i.thread_id.as_ref().unwrap().0.as_str())
        .collect();
    assert_eq!(ids, vec!["old", "new"]);
    assert!(rank_inbox(&[], None).is_empty());
}

#[test]
fn unverified_cleanup_is_never_shown_as_stopped() {
    let unverified = ThreadState::Interrupted {
        cleanup: Cleanup::Unverified,
    };
    let recovered = next_state(
        &ThreadState::Running,
        Pending::default(),
        AttentionInput::Signal(&Signal::Recovered {
            cleanup: Cleanup::Unverified,
        }),
    );
    assert_eq!(recovered, unverified);
    // The closing events and exit that follow don't turn it into Ended, even
    // with exit code 0.
    assert_eq!(exit(recovered.clone(), Some(0)), unverified);
    let ended = AgentEvent::SessionEnded {
        exit_code: Some(0),
        reason: None,
    };
    assert_eq!(
        run(recovered.clone(), Pending::default(), ended),
        unverified
    );

    // And the inbox keeps asking about it.
    let items = rank_inbox(&[thread("t", recovered, false, Some(7))], None);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].reason, InboxReason::CleanupUnverified);
}

#[test]
fn notifications_are_rate_limited_and_suppressed_when_focused() {
    let interval = NOTIFY_MIN_INTERVAL.as_millis() as u64;
    let running = ThreadState::Running;
    let now = 1_000_000;

    // Transitions into the three states notify.
    for next in [
        ThreadState::NeedsApproval,
        ThreadState::NeedsInput,
        failed(),
    ] {
        assert!(should_notify(&running, &next, false, None, now), "{next:?}");
    }
    // Other transitions don't.
    for next in [
        ThreadState::Running,
        ThreadState::Idle,
        ThreadState::Ended,
        paused(),
        ThreadState::Starting,
        ThreadState::Interrupted {
            cleanup: Cleanup::Unverified,
        },
    ] {
        assert!(
            !should_notify(&ThreadState::Idle, &next, false, None, now),
            "{next:?}"
        );
    }
    // Staying in the state is not a transition.
    assert!(!should_notify(
        &ThreadState::NeedsApproval,
        &ThreadState::NeedsApproval,
        false,
        None,
        now
    ));
    // Approval to question is a transition into NeedsInput.
    assert!(should_notify(
        &ThreadState::NeedsApproval,
        &ThreadState::NeedsInput,
        false,
        None,
        now
    ));
    // Focused: never.
    assert!(!should_notify(
        &running,
        &ThreadState::NeedsApproval,
        true,
        None,
        now
    ));
    // Rate limit per thread.
    let next = ThreadState::NeedsApproval;
    assert!(!should_notify(&running, &next, false, Some(now - 1), now));
    assert!(!should_notify(
        &running,
        &next,
        false,
        Some(now - interval + 1),
        now
    ));
    assert!(should_notify(
        &running,
        &next,
        false,
        Some(now - interval),
        now
    ));
    // A clock that went backwards doesn't notify.
    assert!(!should_notify(&running, &next, false, Some(now + 5), now));
    assert_eq!(NOTIFY_MIN_INTERVAL, Duration::from_secs(30));
}

#[test]
fn failed_and_paused_are_sticky_through_exit() {
    // Failed: the closing events keep it, whatever the exit code, until the
    // next start.
    let closing = [
        AgentEvent::TurnCompleted {
            turn: 1,
            outcome: TurnOutcome::Failed,
            usage: None,
        },
        AgentEvent::SessionEnded {
            exit_code: Some(0),
            reason: None,
        },
        AgentEvent::SessionEnded {
            exit_code: Some(1),
            reason: None,
        },
    ];
    for state in [failed(), paused()] {
        for e in &closing {
            assert_eq!(run(state.clone(), Pending::default(), e.clone()), state);
        }
        for code in [Some(0), Some(1), None] {
            assert_eq!(exit(state.clone(), code), state);
        }
    }
    // A fatal error closes a session as Failed; the rest of the close keeps it.
    let state = run(
        ThreadState::Running,
        Pending::default(),
        AgentEvent::Error {
            message: "dead".into(),
            fatal: true,
            details: vec![],
        },
    );
    assert_eq!(
        state,
        ThreadState::Failed {
            message: "dead".into()
        }
    );
    assert_eq!(exit(state.clone(), Some(1)), state);
    // Only a start leaves it.
    assert_eq!(
        next_state(
            &state,
            Pending::default(),
            AttentionInput::Signal(&Signal::Starting)
        ),
        ThreadState::Starting
    );
    // A pause ends when the next turn starts.
    assert_eq!(
        run(
            paused(),
            Pending::default(),
            AgentEvent::TurnStarted { turn: 2 }
        ),
        ThreadState::Running
    );
}

#[test]
fn user_stop_ends_as_ended_whatever_the_exit_code() {
    for from in STATES.chars().map(state_of) {
        let stopped = next_state(
            &from,
            Pending::default(),
            AttentionInput::Signal(&Signal::Stopped),
        );
        assert_eq!(stopped, ThreadState::Ended, "from {from:?}");
    }
    for code in [Some(0), Some(1), Some(137), None] {
        assert_eq!(exit(ThreadState::Ended, code), ThreadState::Ended);
    }
}

#[test]
fn stopped_is_ended_and_sticky_through_exit() {
    let stopped = next_state(
        &ThreadState::NeedsApproval,
        Pending::default(),
        AttentionInput::Signal(&Signal::Stopped),
    );
    assert_eq!(stopped, ThreadState::Ended);
    let closing = [
        AgentEvent::TurnCompleted {
            turn: 1,
            outcome: TurnOutcome::Interrupted,
            usage: None,
        },
        AgentEvent::SessionEnded {
            exit_code: Some(143),
            reason: Some("killed".into()),
        },
    ];
    for e in closing {
        assert_eq!(run(ThreadState::Ended, Pending::default(), e), stopped);
    }
    assert_eq!(exit(stopped.clone(), Some(143)), stopped);
    assert_eq!(exit(stopped.clone(), None), stopped);
}

#[test]
fn exit_code_decides_ended_or_interrupted() {
    let interrupted = ThreadState::Interrupted {
        cleanup: Cleanup::Verified,
    };
    for from in [
        ThreadState::Starting,
        ThreadState::Running,
        ThreadState::NeedsApproval,
        ThreadState::NeedsInput,
        ThreadState::Idle,
    ] {
        assert_eq!(exit(from.clone(), Some(0)), ThreadState::Ended);
        assert_eq!(exit(from.clone(), Some(1)), interrupted);
        assert_eq!(exit(from.clone(), Some(-9)), interrupted);
        assert_eq!(exit(from.clone(), None), interrupted);
        // The same through SessionEnded, whichever comes first.
        let ended = |code| AgentEvent::SessionEnded {
            exit_code: code,
            reason: None,
        };
        assert_eq!(
            run(from.clone(), Pending::default(), ended(Some(0))),
            ThreadState::Ended
        );
        assert_eq!(
            run(from.clone(), Pending::default(), ended(None)),
            interrupted
        );
        // Whichever comes first, the second keeps the result.
        let first = exit(from, Some(1));
        assert_eq!(
            run(first.clone(), Pending::default(), ended(Some(0))),
            first
        );
    }
}
