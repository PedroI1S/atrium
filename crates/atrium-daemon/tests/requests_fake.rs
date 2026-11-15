use atrium_protocol::agent::{
    AgentEvent, ApprovalDecision, MessageKind, QuestionAnswer, RequestOutcome, SessionCommand,
    ToolKind, ToolStatus, TurnOutcome,
};
use atrium_protocol::policy::{
    RememberScope, Rule, RuleDecision, RuleDraft, RuleMatcher, RuleScope,
};
use atrium_protocol::thread::{ThreadId, ThreadState};
use atrium_protocol::wire::{
    AgentLaunch, ClientMessage, DaemonCommand, ErrorCode, ServerMessage, StoredEvent,
};

mod common;

use common::{Rig, Wire, kinds, soon};

fn fake_with(
    duration_ms: u64,
    approval_at_ms: Option<u64>,
    question_at_ms: Option<u64>,
) -> AgentLaunch {
    AgentLaunch::Fake {
        tokens_per_second: 100,
        chunk_chars: 8,
        duration_ms,
        tool_call_every_ms: 0,
        approval_at_ms,
        question_at_ms,
    }
}

fn say(thread_id: &ThreadId, text: &str) -> DaemonCommand {
    DaemonCommand::ThreadCommand {
        thread_id: thread_id.clone(),
        command: SessionCommand::SendUserMessage { text: text.into() },
    }
}

fn interrupt(thread_id: &ThreadId) -> DaemonCommand {
    DaemonCommand::ThreadCommand {
        thread_id: thread_id.clone(),
        command: SessionCommand::Interrupt,
    }
}

fn latest_approval_id(history: &[StoredEvent]) -> String {
    history
        .iter()
        .rev()
        .find_map(|e| match &e.envelope.event {
            AgentEvent::ApprovalRequested {
                request_id,
                rememberable,
                ..
            } => {
                assert!(
                    *rememberable,
                    "synthetic bash approval should be rememberable"
                );
                Some(request_id.clone())
            }
            _ => None,
        })
        .expect("approval_requested in history")
}

fn latest_question(history: &[StoredEvent]) -> (String, String) {
    history
        .iter()
        .rev()
        .find_map(|e| match &e.envelope.event {
            AgentEvent::QuestionAsked {
                request_id,
                questions,
            } => Some((request_id.clone(), questions[0].id.clone())),
            _ => None,
        })
        .expect("question_asked in history")
}

async fn wait_turn_completed(wire: &mut Wire, thread: &ThreadId, expected_turn: u32) {
    let _ = wire.subscribed(thread, None).await;
    soon(async {
        loop {
            if let ServerMessage::Event { event } = wire.next().await
                && event.thread_id == *thread
                && matches!(
                    event.envelope.event,
                    AgentEvent::TurnCompleted { turn, .. } if turn == expected_turn
                )
            {
                break;
            }
        }
    })
    .await;
    wire.send(&ClientMessage::Unsubscribe {
        thread_id: thread.clone(),
    })
    .await;
}

#[tokio::test]
async fn fake_approval_allow_and_deny() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;

    let summary = wire
        .start_thread(fake_with(60, Some(10), None), Some("turn one"))
        .await;
    let thread = summary.thread_id;

    let waiting = wire
        .thread_when(&thread, |s| s.state == ThreadState::NeedsApproval)
        .await;
    assert_eq!(waiting.pending_approvals, 1);

    let req1 = latest_approval_id(&wire.history(&thread).await);
    wire.ok(DaemonCommand::ResolveApproval {
        thread_id: thread.clone(),
        incarnation: 1,
        request_id: req1.clone(),
        decision: ApprovalDecision::Allow,
        remember: None,
    })
    .await;

    let idle = wire
        .thread_when(&thread, |s| s.state == ThreadState::Idle)
        .await;
    assert_eq!(idle.pending_approvals, 0);

    let history = wire.history(&thread).await;
    assert!(history.iter().any(|e| matches!(
        &e.envelope.event,
        AgentEvent::RequestResolved {
            request_id,
            outcome: RequestOutcome::Approved,
        } if request_id == &req1
    )));
    assert!(history.iter().any(|e| matches!(
        &e.envelope.event,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Succeeded,
            ..
        }
    )));

    // Without remembering, turn two asks again and Deny fails the tool call.
    wire.ok(say(&thread, "turn two")).await;
    wire.thread_when(&thread, |s| s.state == ThreadState::NeedsApproval)
        .await;
    let req2 = latest_approval_id(&wire.history(&thread).await);
    assert_ne!(req2, req1);

    wire.ok(DaemonCommand::ResolveApproval {
        thread_id: thread.clone(),
        incarnation: 1,
        request_id: req2.clone(),
        decision: ApprovalDecision::Deny {
            message: Some("nope".into()),
        },
        remember: None,
    })
    .await;

    wire.thread_when(&thread, |s| s.state == ThreadState::Idle)
        .await;
    let history = wire.history(&thread).await;
    assert!(history.iter().any(|e| matches!(
        &e.envelope.event,
        AgentEvent::RequestResolved {
            request_id,
            outcome: RequestOutcome::Denied,
        } if request_id == &req2
    )));
    assert!(history.iter().any(|e| matches!(
        &e.envelope.event,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Denied,
            error: Some(msg),
            ..
        } if msg == "nope"
    )));

    rig.stop(true).await;
}

#[tokio::test]
async fn fake_approval_allow_redeems_through_gate() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;

    let thread = wire
        .start_thread(fake_with(60, Some(10), None), Some("redeem check"))
        .await
        .thread_id;

    wire.thread_when(&thread, |s| s.state == ThreadState::NeedsApproval)
        .await;
    let req = latest_approval_id(&wire.history(&thread).await);

    // Adding a matching deny rule while the approval is pending means Allow
    // passes the ledger, but gate.redeem re-evaluates policy and refuses.
    wire.ok(DaemonCommand::CreateRule {
        rule: RuleDraft {
            scope: RuleScope::Home,
            tool_kind: ToolKind::Execute,
            tool_name: Some("Bash".into()),
            matcher: RuleMatcher::CommandExact {
                argv: vec!["echo".into(), "synthetic".into()],
            },
            path_scope: None,
            decision: RuleDecision::Deny,
        },
    })
    .await;

    wire.ok(DaemonCommand::ResolveApproval {
        thread_id: thread.clone(),
        incarnation: 1,
        request_id: req.clone(),
        decision: ApprovalDecision::Allow,
        remember: None,
    })
    .await;

    wire.thread_when(&thread, |s| s.state == ThreadState::Idle)
        .await;
    let history = wire.history(&thread).await;
    assert!(history.iter().any(|e| matches!(
        &e.envelope.event,
        AgentEvent::RequestResolved {
            request_id,
            outcome: RequestOutcome::Approved,
        } if request_id == &req
    )));
    assert!(history.iter().any(|e| matches!(
        &e.envelope.event,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Denied,
            error: Some(_),
            ..
        }
    )));
    assert!(!history.iter().any(|e| matches!(
        &e.envelope.event,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Succeeded,
            ..
        }
    )));

    rig.stop(true).await;
}

#[tokio::test]
async fn fake_approval_always_creates_rule_and_skips_next_turn() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;

    let thread = wire
        .start_thread(fake_with(50, Some(10), None), Some("turn one"))
        .await
        .thread_id;

    wire.thread_when(&thread, |s| s.state == ThreadState::NeedsApproval)
        .await;
    let req1 = latest_approval_id(&wire.history(&thread).await);

    wire.ok(DaemonCommand::ResolveApproval {
        thread_id: thread.clone(),
        incarnation: 1,
        request_id: req1.clone(),
        decision: ApprovalDecision::Allow,
        remember: Some(RememberScope::Home),
    })
    .await;

    wire.thread_when(&thread, |s| s.state == ThreadState::Idle)
        .await;

    let rules_val = wire.ok(DaemonCommand::ListRules).await;
    let rules: Vec<Rule> = serde_json::from_value(rules_val).unwrap();
    assert_eq!(rules.len(), 1);

    // Turn two runs the same Bash command and succeeds without asking.
    wire.ok(say(&thread, "turn two")).await;
    wait_turn_completed(&mut wire, &thread, 2).await;
    let idle = wire
        .thread_when(&thread, |s| s.state == ThreadState::Idle)
        .await;
    assert_eq!(idle.pending_approvals, 0);

    let history = wire.history(&thread).await;
    let approvals = kinds(&history)
        .into_iter()
        .filter(|k| *k == "approval_requested")
        .count();
    let tools = kinds(&history)
        .into_iter()
        .filter(|k| *k == "tool_call_finished")
        .count();
    assert_eq!(approvals, 1, "only the first turn should have asked");
    assert_eq!(tools, 2, "both turns ran the synthetic tool call");

    rig.stop(true).await;
}

#[tokio::test]
async fn fake_stale_approval_is_refused_with_expired() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;

    let thread = wire
        .start_thread(fake_with(200, Some(10), None), Some("wait"))
        .await
        .thread_id;

    wire.thread_when(&thread, |s| s.state == ThreadState::NeedsApproval)
        .await;
    let req = latest_approval_id(&wire.history(&thread).await);

    // Wrong incarnation is refused before touching the request.
    assert_eq!(
        wire.refused(DaemonCommand::ResolveApproval {
            thread_id: thread.clone(),
            incarnation: 99,
            request_id: req.clone(),
            decision: ApprovalDecision::Allow,
            remember: None,
        })
        .await,
        ErrorCode::Expired
    );

    // Interrupt withdraws the pending approval; resolving afterwards returns Expired.
    wire.ok(interrupt(&thread)).await;
    wire.thread_when(&thread, |s| s.state == ThreadState::Idle)
        .await;
    assert_eq!(
        wire.refused(DaemonCommand::ResolveApproval {
            thread_id: thread.clone(),
            incarnation: 1,
            request_id: req,
            decision: ApprovalDecision::Allow,
            remember: None,
        })
        .await,
        ErrorCode::Expired
    );

    rig.stop(true).await;
}

#[tokio::test]
async fn fake_question_answered_declined_and_checked_by_ledger() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;

    let thread = wire
        .start_thread(fake_with(60, None, Some(10)), Some("ask once"))
        .await
        .thread_id;

    let waiting = wire
        .thread_when(&thread, |s| s.state == ThreadState::NeedsInput)
        .await;
    assert_eq!(waiting.pending_questions, 1);

    let history = wire.history(&thread).await;
    let (req1, qid) = latest_question(&history);
    assert!(history.iter().any(|e| matches!(
        &e.envelope.event,
        AgentEvent::QuestionAsked { request_id, questions }
        if request_id == &req1
            && questions.len() == 1
            && questions[0].options.len() == 2
            && questions[0].allow_text
    )));

    // Wrong incarnation and direct ThreadCommand door are refused.
    assert_eq!(
        wire.refused(DaemonCommand::DeclineQuestion {
            thread_id: thread.clone(),
            incarnation: 2,
            request_id: req1.clone(),
        })
        .await,
        ErrorCode::Expired
    );
    assert_eq!(
        wire.refused(DaemonCommand::ThreadCommand {
            thread_id: thread.clone(),
            command: SessionCommand::DeclineQuestion {
                request_id: req1.clone(),
            },
        })
        .await,
        ErrorCode::Invalid
    );
    // Unknown request id returns the same ErrorCode for AnswerQuestion and DeclineQuestion.
    let answer_unknown = wire
        .refused(DaemonCommand::AnswerQuestion {
            thread_id: thread.clone(),
            incarnation: 1,
            request_id: "unknown-request".into(),
            answers: vec![],
        })
        .await;
    let decline_unknown = wire
        .refused(DaemonCommand::DeclineQuestion {
            thread_id: thread.clone(),
            incarnation: 1,
            request_id: "unknown-request".into(),
        })
        .await;
    assert_eq!(decline_unknown, answer_unknown);
    assert_eq!(decline_unknown, ErrorCode::Expired);

    // Answer turn 1.
    wire.ok(DaemonCommand::AnswerQuestion {
        thread_id: thread.clone(),
        incarnation: 1,
        request_id: req1.clone(),
        answers: vec![QuestionAnswer {
            question_id: qid,
            selected: vec!["fast".into()],
            text: None,
        }],
    })
    .await;

    let idle = wire
        .thread_when(&thread, |s| s.state == ThreadState::Idle)
        .await;
    assert_eq!(idle.pending_questions, 0);
    assert_eq!(
        wire.refused(DaemonCommand::DeclineQuestion {
            thread_id: thread.clone(),
            incarnation: 1,
            request_id: req1.clone(),
        })
        .await,
        ErrorCode::Expired
    );

    // Streaming continues after the answer until turn 1 completes.
    let history = wire.history(&thread).await;
    let resolved_at = history
        .iter()
        .position(|e| {
            matches!(
                &e.envelope.event,
                AgentEvent::RequestResolved {
                    request_id,
                    outcome: RequestOutcome::Answered,
                } if request_id == &req1
            )
        })
        .expect("RequestResolved(Answered) in history");
    let after_answer = &history[resolved_at + 1..];
    assert!(after_answer.iter().any(|e| matches!(
        &e.envelope.event,
        AgentEvent::MessageDelta {
            kind: MessageKind::Assistant,
            ..
        }
    )));
    assert!(after_answer.iter().any(|e| matches!(
        &e.envelope.event,
        AgentEvent::MessageCompleted {
            kind: MessageKind::Assistant,
            ..
        }
    )));
    assert!(matches!(
        after_answer.last().map(|e| &e.envelope.event),
        Some(AgentEvent::TurnCompleted {
            turn: 1,
            outcome: TurnOutcome::Completed,
            ..
        })
    ));

    // Turn 2 asks again; DeclineQuestion resolves it as Cancelled and the turn still completes.
    wire.ok(say(&thread, "ask twice")).await;
    wire.thread_when(&thread, |s| s.state == ThreadState::NeedsInput)
        .await;
    let (req2, _) = latest_question(&wire.history(&thread).await);
    assert_ne!(req2, req1);

    wire.ok(DaemonCommand::DeclineQuestion {
        thread_id: thread.clone(),
        incarnation: 1,
        request_id: req2.clone(),
    })
    .await;

    let idle = wire
        .thread_when(&thread, |s| s.state == ThreadState::Idle)
        .await;
    assert_eq!(idle.pending_questions, 0);

    let history = wire.history(&thread).await;
    assert!(history.iter().any(|e| matches!(
        &e.envelope.event,
        AgentEvent::RequestResolved {
            request_id,
            outcome: RequestOutcome::Cancelled,
        } if request_id == &req2
    )));
    assert!(matches!(
        history.last().map(|e| &e.envelope.event),
        Some(AgentEvent::TurnCompleted {
            turn: 2,
            outcome: TurnOutcome::Completed,
            ..
        })
    ));

    // Declining an approval through DeclineQuestion is refused with Expired.
    let approval_thread = wire
        .start_thread(fake_with(200, Some(10), None), Some("not a question"))
        .await
        .thread_id;
    wire.thread_when(&approval_thread, |s| s.state == ThreadState::NeedsApproval)
        .await;
    let approval_req = latest_approval_id(&wire.history(&approval_thread).await);
    assert_eq!(
        wire.refused(DaemonCommand::DeclineQuestion {
            thread_id: approval_thread,
            incarnation: 1,
            request_id: approval_req,
        })
        .await,
        ErrorCode::Expired
    );

    rig.stop(true).await;
}

#[tokio::test]
async fn fake_interrupt_withdraws_pending_approval_and_question() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;

    let t_approval = wire
        .start_thread(fake_with(5_000, Some(10), None), Some("approval"))
        .await
        .thread_id;
    wire.thread_when(&t_approval, |s| s.state == ThreadState::NeedsApproval)
        .await;
    let req_a = latest_approval_id(&wire.history(&t_approval).await);

    wire.ok(interrupt(&t_approval)).await;
    let idle_a = wire
        .thread_when(&t_approval, |s| s.state == ThreadState::Idle)
        .await;
    assert_eq!(idle_a.pending_approvals, 0);
    let hist_a = wire.history(&t_approval).await;
    assert!(hist_a.iter().any(|e| matches!(
        &e.envelope.event,
        AgentEvent::RequestResolved {
            request_id,
            outcome: RequestOutcome::Cancelled,
        } if request_id == &req_a
    )));
    assert!(hist_a.iter().any(|e| matches!(
        &e.envelope.event,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Cancelled,
            ..
        }
    )));
    assert!(matches!(
        hist_a.last().map(|e| &e.envelope.event),
        Some(AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Interrupted,
            ..
        })
    ));

    let t_question = wire
        .start_thread(fake_with(5_000, None, Some(10)), Some("question"))
        .await
        .thread_id;
    wire.thread_when(&t_question, |s| s.state == ThreadState::NeedsInput)
        .await;
    let (req_q, _) = latest_question(&wire.history(&t_question).await);

    wire.ok(interrupt(&t_question)).await;
    let idle_q = wire
        .thread_when(&t_question, |s| s.state == ThreadState::Idle)
        .await;
    assert_eq!(idle_q.pending_questions, 0);
    let hist_q = wire.history(&t_question).await;
    assert!(hist_q.iter().any(|e| matches!(
        &e.envelope.event,
        AgentEvent::RequestResolved {
            request_id,
            outcome: RequestOutcome::Cancelled,
        } if request_id == &req_q
    )));
    assert!(matches!(
        hist_q.last().map(|e| &e.envelope.event),
        Some(AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Interrupted,
            ..
        })
    ));

    rig.stop(true).await;
}
