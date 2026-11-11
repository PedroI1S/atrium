//! Sessions, approvals, the inbox and the commands, against a real daemon
//! with a scripted adapter.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::Ordering;
use std::time::Duration;

use atrium_agents::GateDecision;
use atrium_daemon::{KEEP_RUNNING_SETTING, NotifyReason};
use atrium_protocol::agent::{
    AgentEvent, ApprovalDecision, MessageBody, MessageKind, Question, QuestionAnswer,
    RequestOutcome, SessionCommand, ToolKind, ToolStatus, TurnOutcome,
};
use atrium_protocol::policy::{
    RememberScope, Rule, RuleDecision, RuleDraft, RuleMatcher, RuleOrigin, RuleScope,
};
use atrium_protocol::thread::{Cleanup, InboxItem, InboxReason, ThreadId, ThreadState};
use atrium_protocol::wire::{
    AgentLaunch, DaemonCommand, ErrorCode, ExportFormat, ProviderProbe, ServerMessage,
};
use common::{Rig, Script, Start, Wire, kinds, native, sha256_hex, soon};
use serde_json::{Value, json};

fn resolve(
    thread: &ThreadId,
    incarnation: u32,
    request_id: &str,
    decision: ApprovalDecision,
) -> DaemonCommand {
    DaemonCommand::ResolveApproval {
        thread_id: thread.clone(),
        incarnation,
        request_id: request_id.into(),
        decision,
        remember: None,
    }
}

fn say(thread: &ThreadId, text: &str) -> DaemonCommand {
    DaemonCommand::ThreadCommand {
        thread_id: thread.clone(),
        command: SessionCommand::SendUserMessage { text: text.into() },
    }
}

fn shutdown(thread: &ThreadId) -> DaemonCommand {
    DaemonCommand::ThreadCommand {
        thread_id: thread.clone(),
        command: SessionCommand::Shutdown,
    }
}

async fn inbox(wire: &mut Wire) -> Vec<InboxItem> {
    serde_json::from_value(wire.ok(DaemonCommand::ListInbox).await).expect("an inbox")
}

/// A thread with a session that has started and is in its first turn.
async fn running(rig: &mut Rig, wire: &mut Wire) -> (ThreadId, Script) {
    let thread = wire.start_thread(native(), Some("go")).await.thread_id;
    let mut script = rig.script().await;
    // The message the thread was started with is the session's first.
    assert_eq!(
        script.command().await,
        SessionCommand::SendUserMessage { text: "go".into() }
    );
    script.started("native-1").await;
    script.turn(1).await;
    wire.thread_when(&thread, |summary| summary.state == ThreadState::Running)
        .await;
    (thread, script)
}

#[tokio::test]
async fn a_stale_approval_is_expired_and_never_reaches_the_session() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;
    let (thread, mut script) = running(&mut rig, &mut wire).await;

    // An approval the ledger accepts reaches the session, once.
    let (ticket, check) = script.ask_ticket(1, "a.txt").await;
    let first = ticket.request_id.clone();
    let summary = wire
        .thread_when(&thread, |summary| summary.pending_approvals == 1)
        .await;
    assert_eq!(summary.state, ThreadState::NeedsApproval);
    assert_eq!(summary.incarnation, 1);
    let answer = wire
        .ok(resolve(&thread, 1, &first, ApprovalDecision::Allow))
        .await;
    assert_eq!(answer, json!({ "rule_id": null }));
    assert_eq!(
        script.command().await,
        SessionCommand::ResolveApproval {
            request_id: first.clone(),
            decision: ApprovalDecision::Allow
        }
    );
    // The grant is the ledger's: redeemed once by the session's gate.
    assert_eq!(
        script.config.gate.redeem(&ticket, &check).await,
        GateDecision::Allow
    );
    assert!(matches!(
        script.config.gate.redeem(&ticket, &check).await,
        GateDecision::Deny { .. }
    ));

    // Resolved twice, either way.
    for decision in [
        ApprovalDecision::Allow,
        ApprovalDecision::Deny { message: None },
    ] {
        assert_eq!(
            wire.refused(resolve(&thread, 1, &first, decision)).await,
            ErrorCode::Expired
        );
    }
    // Unknown request, unknown incarnation, unknown thread.
    assert_eq!(
        wire.refused(resolve(&thread, 1, "req_nope", ApprovalDecision::Allow))
            .await,
        ErrorCode::Expired
    );
    assert_eq!(
        wire.refused(resolve(&thread, 9, &first, ApprovalDecision::Allow))
            .await,
        ErrorCode::Expired
    );
    assert_eq!(
        wire.refused(resolve(
            &ThreadId("th-nobody".into()),
            1,
            &first,
            ApprovalDecision::Allow
        ))
        .await,
        ErrorCode::Expired
    );
    script.no_command().await;
    script
        .emit(AgentEvent::RequestResolved {
            request_id: first.clone(),
            outcome: RequestOutcome::Approved,
        })
        .await;

    // The provider withdrew it.
    let withdrawn = script.ask(1, "b.txt").await;
    script
        .emit(AgentEvent::RequestResolved {
            request_id: withdrawn.clone(),
            outcome: RequestOutcome::Cancelled,
        })
        .await;
    wire.thread_when(&thread, |summary| summary.state == ThreadState::Running)
        .await;
    assert_eq!(
        wire.refused(resolve(&thread, 1, &withdrawn, ApprovalDecision::Allow))
            .await,
        ErrorCode::Expired
    );

    // The turn ended under it.
    let late = script.ask(1, "c.txt").await;
    wire.thread_when(&thread, |summary| summary.pending_approvals == 1)
        .await;
    let before = wire.thread(&thread).await.last_event;
    script.turn_done(1).await;
    wire.thread_when(&thread, |summary| summary.last_event > before)
        .await;
    assert_eq!(
        wire.refused(resolve(&thread, 1, &late, ApprovalDecision::Allow))
            .await,
        ErrorCode::Expired
    );
    script.no_command().await;

    // An older incarnation's request, after the thread was resumed.
    script.turn(2).await;
    let old = script.ask(2, "d.txt").await;
    wire.thread_when(&thread, |summary| summary.pending_approvals >= 1)
        .await;
    script.ended(Some(0)).await;
    wire.thread_when(&thread, |summary| summary.state == ThreadState::Ended)
        .await;
    let resumed = wire
        .ok(DaemonCommand::ResumeThread {
            thread_id: thread.clone(),
        })
        .await;
    assert_eq!(resumed["incarnation"], 2);
    let mut second = rig.script().await;
    assert_eq!(second.config.resume.as_deref(), Some("native-1"));
    second.started("native-1").await;
    second.turn(1).await;
    wire.thread_when(&thread, |summary| summary.state == ThreadState::Running)
        .await;
    for incarnation in [1, 2] {
        assert_eq!(
            wire.refused(resolve(&thread, incarnation, &old, ApprovalDecision::Allow))
                .await,
            ErrorCode::Expired
        );
    }
    second.no_command().await;

    // The new incarnation's own request works, and a denial reaches it.
    let fresh = second.ask(1, "e.txt").await;
    wire.thread_when(&thread, |summary| summary.pending_approvals == 1)
        .await;
    let deny = ApprovalDecision::Deny {
        message: Some("not now".into()),
    };
    // Not with the old incarnation's number.
    assert_eq!(
        wire.refused(resolve(&thread, 1, &fresh, deny.clone()))
            .await,
        ErrorCode::Expired
    );
    wire.ok(resolve(&thread, 2, &fresh, deny.clone())).await;
    assert_eq!(
        second.command().await,
        SessionCommand::ResolveApproval {
            request_id: fresh,
            decision: deny
        }
    );
}

#[tokio::test]
async fn always_creates_a_rule_with_the_approval() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;
    let (thread, mut script) = running(&mut rig, &mut wire).await;
    let request = script.ask(1, "notes.txt").await;
    wire.thread_when(&thread, |summary| summary.pending_approvals == 1)
        .await;
    let answer = wire
        .ok(DaemonCommand::ResolveApproval {
            thread_id: thread.clone(),
            incarnation: 1,
            request_id: request.clone(),
            decision: ApprovalDecision::Allow,
            remember: Some(RememberScope::Thread),
        })
        .await;
    let rule_id = answer["rule_id"].as_str().expect("a rule").to_owned();
    let rules: Vec<Rule> =
        serde_json::from_value(wire.ok(DaemonCommand::ListRules).await).expect("rules");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].id.0, rule_id);
    assert_eq!(
        rules[0].origin,
        RuleOrigin::Approval {
            request_id: request
        }
    );
    assert_eq!(
        rules[0].matcher,
        RuleMatcher::CommandExact {
            argv: vec!["touch".into(), "notes.txt".into()]
        }
    );
    // The session's gate sees it from the next check on: the same command
    // is allowed, another still asks.
    assert_eq!(
        script
            .config
            .gate
            .check(&Script::write_check(1, "notes.txt"))
            .await,
        GateDecision::Allow
    );
    assert!(matches!(
        script
            .config
            .gate
            .check(&Script::write_check(1, "other.txt"))
            .await,
        GateDecision::Ask { .. }
    ));
}

#[tokio::test]
async fn an_answer_goes_through_the_ledger_too() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;
    let (thread, mut script) = running(&mut rig, &mut wire).await;
    let ticket = script
        .config
        .gate
        .question_ticket(Some(1))
        .await
        .expect("a ticket");
    script
        .emit(AgentEvent::QuestionAsked {
            request_id: ticket.request_id.clone(),
            questions: vec![Question {
                id: "q1".into(),
                prompt: "Which folder?".into(),
                options: Vec::new(),
                multi_select: false,
                allow_text: true,
            }],
        })
        .await;
    let summary = wire
        .thread_when(&thread, |summary| summary.pending_questions == 1)
        .await;
    assert_eq!(summary.state, ThreadState::NeedsInput);
    assert_eq!(summary.attention_detail.as_deref(), Some("Which folder?"));
    let answers = vec![QuestionAnswer {
        question_id: "q1".into(),
        selected: Vec::new(),
        text: Some("~/notes".into()),
    }];
    let answer = |incarnation| DaemonCommand::AnswerQuestion {
        thread_id: thread.clone(),
        incarnation,
        request_id: ticket.request_id.clone(),
        answers: answers.clone(),
    };
    assert_eq!(wire.refused(answer(2)).await, ErrorCode::Expired);
    wire.ok(answer(1)).await;
    assert_eq!(
        script.command().await,
        SessionCommand::AnswerQuestion {
            request_id: ticket.request_id.clone(),
            answers: answers.clone()
        }
    );
    assert_eq!(wire.refused(answer(1)).await, ErrorCode::Expired);
    // An approval can't be answered through the question's door, and the
    // session commands' own door is closed to both.
    assert_eq!(
        wire.refused(DaemonCommand::ThreadCommand {
            thread_id: thread.clone(),
            command: SessionCommand::AnswerQuestion {
                request_id: ticket.request_id.clone(),
                answers,
            },
        })
        .await,
        ErrorCode::Invalid
    );
    script.no_command().await;
}

#[tokio::test]
async fn a_session_that_dies_is_closed_in_order() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;
    let (thread, mut script) = running(&mut rig, &mut wire).await;
    script.delta("m1", "hel").await;
    script.delta("m1", "lo").await;
    script
        .emit(AgentEvent::ToolCallStarted {
            call_id: "c1".into(),
            name: "Bash".into(),
            kind: ToolKind::Execute,
            title: None,
            input: json!({ "command": "sleep 100" }),
        })
        .await;
    let request = script.ask(1, "x.txt").await;
    wire.thread_when(&thread, |summary| summary.pending_approvals == 1)
        .await;

    // The provider is gone: no closing events, no `SessionEnded`.
    script.die();
    let summary = wire
        .thread_when(&thread, |summary| {
            matches!(summary.state, ThreadState::Interrupted { .. })
        })
        .await;
    assert_eq!(
        summary.state,
        ThreadState::Interrupted {
            cleanup: Cleanup::Verified
        }
    );
    assert_eq!(summary.pending_approvals, 0);

    let history = wire.history(&thread).await;
    assert_eq!(
        kinds(&history),
        [
            "session_started",
            "turn_started",
            "message_delta",
            "message_delta",
            "tool_call_started",
            "approval_requested",
            "message_completed",
            "tool_call_finished",
            "request_resolved",
            "turn_completed",
            "session_ended",
        ]
    );
    let closing: Vec<&AgentEvent> = history[6..].iter().map(|e| &e.envelope.event).collect();
    assert_eq!(
        *closing[0],
        AgentEvent::MessageCompleted {
            message_id: "m1".into(),
            kind: MessageKind::Assistant,
            body: MessageBody::Streamed {
                chars: 5,
                deltas: 2,
                sha256: sha256_hex("hello"),
            },
        }
    );
    assert!(matches!(
        closing[1],
        AgentEvent::ToolCallFinished { call_id, status: ToolStatus::Cancelled, .. } if call_id == "c1"
    ));
    assert_eq!(
        *closing[2],
        AgentEvent::RequestResolved {
            request_id: request.clone(),
            outcome: RequestOutcome::Cancelled
        }
    );
    assert!(matches!(
        closing[3],
        AgentEvent::TurnCompleted {
            turn: 1,
            outcome: TurnOutcome::Interrupted,
            ..
        }
    ));
    // One incarnation, sequence numbers without a gap.
    for (at, event) in history.iter().enumerate() {
        assert_eq!(event.envelope.incarnation, 1);
        assert_eq!(event.envelope.seq, at as u64);
    }

    // Its request is over, and so is the session.
    assert_eq!(
        wire.refused(resolve(&thread, 1, &request, ApprovalDecision::Allow))
            .await,
        ErrorCode::Expired
    );
    assert_eq!(
        wire.refused(say(&thread, "still there?")).await,
        ErrorCode::NotRunning
    );
}

#[tokio::test]
async fn a_session_that_cannot_start_fails_or_is_stopped() {
    let rig = Rig::start().await;
    let mut wire = rig.dev().await;

    *rig.adapter.start.lock().expect("lock") = Start::Fail;
    let failed = wire.start_thread(native(), Some("go")).await;
    assert_eq!(failed.state, ThreadState::Starting);
    let summary = wire
        .thread_when(&failed.thread_id, |summary| {
            matches!(summary.state, ThreadState::Failed { .. })
        })
        .await;
    let ThreadState::Failed { message } = summary.state else {
        unreachable!()
    };
    assert!(message.contains("no such model"), "{message}");
    // A pre-session failure is exactly an error and the end.
    let history = wire.history(&failed.thread_id).await;
    assert_eq!(kinds(&history), ["error", "session_ended"]);
    assert_eq!(
        inbox(&mut wire).await[0].reason,
        InboxReason::Failed,
        "it shows in the inbox"
    );
    let notes = rig.notes();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].reason, NotifyReason::Failed);
    assert_eq!(notes[0].thread_id, failed.thread_id);

    // Stopped while the adapter is still starting: no error, just the end.
    *rig.adapter.start.lock().expect("lock") = Start::Hang;
    let hung = wire.start_thread(native(), Some("go")).await.thread_id;
    wire.ok(shutdown(&hung)).await;
    wire.thread_when(&hung, |summary| {
        summary.state == ThreadState::Ended && summary.last_event.is_some()
    })
    .await;
    assert_eq!(kinds(&wire.history(&hung).await), ["session_ended"]);

    // No adapter for the provider, and resuming what never had a session.
    assert_eq!(
        wire.refused(DaemonCommand::StartThread {
            agent: AgentLaunch::Native {
                provider: atrium_protocol::agent::Provider::Codex,
                model: None,
                effort: None,
                preset: atrium_protocol::policy::Preset::Home,
            },
            message: None,
        })
        .await,
        ErrorCode::Invalid
    );
    assert_eq!(
        wire.refused(DaemonCommand::ResumeThread { thread_id: hung })
            .await,
        ErrorCode::Invalid
    );
}

#[tokio::test]
async fn thread_commands_reach_the_session() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;
    let nobody = ThreadId("th-nobody".into());
    assert_eq!(
        wire.refused(say(&nobody, "hi")).await,
        ErrorCode::UnknownThread
    );

    // Started without a message: untitled until the first one.
    let started = wire.start_thread(native(), None).await;
    let thread = started.thread_id.clone();
    assert_eq!(started.title, "New thread");
    let mut script = rig.script().await;
    script.started("native-1").await;
    wire.thread_when(&thread, |summary| summary.state == ThreadState::Idle)
        .await;
    assert_eq!(wire.refused(say(&thread, "  \n")).await, ErrorCode::Invalid);
    wire.ok(say(&thread, "Rename the photos\nby date")).await;
    assert_eq!(
        script.command().await,
        SessionCommand::SendUserMessage {
            text: "Rename the photos\nby date".into()
        }
    );
    assert_eq!(wire.thread(&thread).await.title, "Rename the photos");
    // A later message doesn't retitle it.
    wire.ok(say(&thread, "and the videos")).await;
    assert!(matches!(
        script.command().await,
        SessionCommand::SendUserMessage { .. }
    ));
    assert_eq!(wire.thread(&thread).await.title, "Rename the photos");

    wire.ok(DaemonCommand::ThreadCommand {
        thread_id: thread.clone(),
        command: SessionCommand::Interrupt,
    })
    .await;
    assert_eq!(script.command().await, SessionCommand::Interrupt);

    // A second session for the same thread is refused.
    assert_eq!(
        wire.refused(DaemonCommand::ResumeThread {
            thread_id: thread.clone()
        })
        .await,
        ErrorCode::Invalid
    );

    // The user's stop is `Ended`, whatever the provider exits with.
    wire.ok(shutdown(&thread)).await;
    assert_eq!(script.command().await, SessionCommand::Shutdown);
    assert_eq!(wire.thread(&thread).await.state, ThreadState::Ended);
    script.ended(Some(137)).await;
    soon(async {
        while wire.command(say(&thread, "hi")).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert_eq!(
        wire.refused(say(&thread, "hi")).await,
        ErrorCode::NotRunning
    );
    assert_eq!(wire.thread(&thread).await.state, ThreadState::Ended);
}

#[tokio::test]
async fn full_storage_interrupts_running_turns_and_refuses_new_ones() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;
    let (busy, mut busy_script) = running(&mut rig, &mut wire).await;
    let idle = wire.start_thread(native(), Some("later")).await.thread_id;
    let mut idle_script = rig.script().await;
    idle_script.command().await;
    idle_script.started("native-2").await;
    wire.thread_when(&idle, |summary| summary.state == ThreadState::Idle)
        .await;
    assert!(inbox(&mut wire).await.is_empty());

    rig.free_bytes.store(1024, Ordering::Relaxed);
    // The thread with a turn is interrupted and its session stopped.
    let summary = wire
        .thread_when(&busy, |summary| {
            matches!(summary.state, ThreadState::Interrupted { .. })
        })
        .await;
    assert_eq!(
        summary.state,
        ThreadState::Interrupted {
            cleanup: Cleanup::Verified
        }
    );
    assert_eq!(busy_script.command().await, SessionCommand::Shutdown);
    // Its closing events are still stored.
    busy_script.die();
    soon(async {
        while kinds(&wire.history(&busy).await).last() != Some(&"session_ended") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert_eq!(
        kinds(&wire.history(&busy).await)[2..],
        ["turn_completed", "session_ended"]
    );

    // The inbox says so, first.
    let items = inbox(&mut wire).await;
    assert_eq!(items[0].reason, InboxReason::StorageFull);
    assert_eq!(items[0].thread_id, None);

    // New turns are refused; the idle session is left alone.
    assert_eq!(
        wire.refused(DaemonCommand::StartThread {
            agent: native(),
            message: Some("more".into())
        })
        .await,
        ErrorCode::StorageReadOnly
    );
    assert_eq!(
        wire.refused(say(&idle, "more")).await,
        ErrorCode::StorageReadOnly
    );
    assert_eq!(
        wire.refused(DaemonCommand::ResumeThread {
            thread_id: busy.clone()
        })
        .await,
        ErrorCode::StorageReadOnly
    );
    idle_script.no_command().await;

    // It ends by itself when space returns.
    rig.free_bytes.store(u64::MAX, Ordering::Relaxed);
    soon(async {
        while !inbox(&mut wire).await.is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    wire.ok(say(&idle, "more")).await;
    assert!(matches!(
        idle_script.command().await,
        SessionCommand::SendUserMessage { .. }
    ));
    wire.ok(DaemonCommand::ResumeThread { thread_id: busy })
        .await;
}

/// A scripted provider that ends cleanly when told to.
fn obedient(mut script: Script) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while script.command().await != SessionCommand::Shutdown {}
        script.turn_done(1).await;
        script.ended(Some(0)).await;
    })
}

#[tokio::test]
async fn shutdown_waits_for_turns_unless_forced() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;
    let (thread, mut script) = running(&mut rig, &mut wire).await;

    // Not forced, with a turn running: refused, and nothing changes.
    assert_eq!(
        wire.refused(DaemonCommand::ShutdownDaemon { force: false })
            .await,
        ErrorCode::TurnsRunning
    );
    assert!(wire.ok(DaemonCommand::ListThreads).await.is_array());

    // `SIGTERM`: it keeps serving until the turn ends, without taking on
    // new work.
    rig.daemon.as_ref().expect("running").stop(false);
    assert_eq!(wire.thread(&thread).await.state, ThreadState::Running);
    assert_eq!(
        wire.refused(DaemonCommand::StartThread {
            agent: native(),
            message: None
        })
        .await,
        ErrorCode::Invalid
    );
    assert_eq!(wire.refused(say(&thread, "more")).await, ErrorCode::Invalid);
    script.turn_done(1).await;
    // The turn is over: connections close, the idle session is ended.
    assert!(wire.closed().await);
    assert_eq!(script.command().await, SessionCommand::Shutdown);
    script.ended(Some(0)).await;
    rig.stop(false).await;
    assert!(!rig.paths.socket.exists(), "the socket is removed");

    // The same home starts again, with the thread as it was left.
    rig.boot(|_| {}).await;
    let mut wire = rig.dev().await;
    assert_eq!(wire.thread(&thread).await.state, ThreadState::Ended);
    assert_eq!(
        kinds(&wire.history(&thread).await),
        [
            "session_started",
            "turn_started",
            "turn_completed",
            "session_ended"
        ]
    );

    // Forced: answered, then running turns are interrupted and it exits.
    let other = wire.start_thread(native(), Some("again")).await.thread_id;
    let mut script = rig.script().await;
    script.command().await;
    script.started("native-9").await;
    script.turn(1).await;
    wire.thread_when(&other, |summary| summary.state == ThreadState::Running)
        .await;
    let ending = obedient(script);
    wire.ok(DaemonCommand::ShutdownDaemon { force: true }).await;
    assert!(wire.closed().await);
    rig.stop(true).await;
    soon(ending).await.expect("the provider ended");

    rig.boot(|_| {}).await;
    let mut wire = rig.dev().await;
    assert_eq!(wire.thread(&other).await.state, ThreadState::Ended);
    assert_eq!(
        kinds(&wire.history(&other).await),
        [
            "session_started",
            "turn_started",
            "turn_completed",
            "session_ended"
        ]
    );
    // Not forced, with nothing running: it exits at once.
    wire.ok(DaemonCommand::ShutdownDaemon { force: false })
        .await;
    assert!(wire.closed().await);
    rig.stop(false).await;
}

fn updates_of(
    messages: &[ServerMessage],
    thread: &ThreadId,
) -> Vec<atrium_protocol::thread::ThreadSummary> {
    messages
        .iter()
        .filter_map(|message| match message {
            ServerMessage::ThreadUpdated { summary } if summary.thread_id == *thread => {
                Some(summary.clone())
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn the_inbox_stream_follows_threads_without_following_every_token() {
    let mut rig = Rig::start().await;
    let mut watcher = rig.dev().await;
    assert_eq!(
        watcher.subscribe_inbox().await,
        json!({ "threads": [], "inbox": [] })
    );
    let mut wire = rig.dev().await;
    let thread = wire
        .start_thread(native(), Some("  Fix   the build\nplease"))
        .await
        .thread_id;
    let mut script = rig.script().await;
    script.command().await;
    script.started("native-1").await;
    script.turn(1).await;
    wire.thread_when(&thread, |summary| summary.state == ThreadState::Running)
        .await;
    // Everything the watcher was sent so far.
    watcher.ok(DaemonCommand::ListInbox).await;
    let seen: Vec<ServerMessage> = watcher.aside.drain(..).collect();
    let updates = updates_of(&seen, &thread);
    assert_eq!(updates[0].title, "Fix the build");
    assert_eq!(updates[0].state, ThreadState::Starting);
    assert_eq!(updates.last().expect("updates").state, ThreadState::Running);

    // Forty deltas, each stored by itself: the reply shows as unread once,
    // and no update follows the rest.
    for n in 0..40 {
        let before = wire.thread(&thread).await.last_event;
        script.delta("m", &format!("{n} ")).await;
        wire.thread_when(&thread, |summary| summary.last_event > before)
            .await;
    }
    watcher.ok(DaemonCommand::ListInbox).await;
    let seen: Vec<ServerMessage> = watcher.aside.drain(..).collect();
    let updates = updates_of(&seen, &thread);
    assert_eq!(updates.len(), 1, "{updates:?}");
    assert!(updates[0].unread);
    let reply = seen.iter().find_map(|message| match message {
        ServerMessage::InboxUpdated { items } => Some(items.clone()),
        _ => None,
    });
    let reply = reply.expect("the inbox changed");
    assert_eq!(reply.len(), 1);
    assert_eq!(reply[0].reason, InboxReason::Reply);
    assert_eq!(reply[0].thread_id.as_ref(), Some(&thread));

    // Reading and renaming it show too.
    wire.ok(DaemonCommand::MarkRead {
        thread_id: thread.clone(),
    })
    .await;
    wire.ok(DaemonCommand::RenameThread {
        thread_id: thread.clone(),
        title: "Build fix".into(),
    })
    .await;
    assert_eq!(
        wire.refused(DaemonCommand::RenameThread {
            thread_id: thread.clone(),
            title: "  ".into(),
        })
        .await,
        ErrorCode::Invalid
    );
    watcher.ok(DaemonCommand::ListInbox).await;
    let seen: Vec<ServerMessage> = watcher.aside.drain(..).collect();
    let last = updates_of(&seen, &thread).pop().expect("an update");
    assert!(!last.unread);
    assert_eq!(last.title, "Build fix");
    assert!(seen.contains(&ServerMessage::InboxUpdated { items: Vec::new() }));

    // An approval is in the inbox and posts a notification, once.
    script.ask(1, "a.txt").await;
    wire.thread_when(&thread, |summary| summary.pending_approvals == 1)
        .await;
    let items = inbox(&mut watcher).await;
    assert_eq!(items[0].reason, InboxReason::Approval);
    assert_eq!(items[0].detail.as_deref(), Some("touch a.txt"));
    let notes = rig.notes();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(notes[0].reason, NotifyReason::Approval);
    assert_eq!(notes[0].title, "Build fix");

    // None for a thread someone is looking at.
    let focused = wire.start_thread(native(), Some("second")).await.thread_id;
    let mut other = rig.script().await;
    other.started("native-2").await;
    other.turn(1).await;
    watcher
        .ok(DaemonCommand::SetFocus {
            thread_id: Some(focused.clone()),
        })
        .await;
    other.ask(1, "b.txt").await;
    wire.thread_when(&focused, |summary| summary.pending_approvals == 1)
        .await;
    assert_eq!(rig.notes().len(), 1);

    // Archived by one connection: it leaves the list for every watcher,
    // and its session is told to stop.
    watcher.aside.clear();
    wire.ok(DaemonCommand::ArchiveThread {
        thread_id: thread.clone(),
    })
    .await;
    assert_eq!(script.command().await, SessionCommand::Shutdown);
    let listed: Vec<Value> =
        serde_json::from_value(watcher.ok(DaemonCommand::ListThreads).await).expect("threads");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["thread_id"], focused.0);
    let seen: Vec<ServerMessage> = watcher.aside.drain(..).collect();
    assert!(seen.contains(&ServerMessage::ThreadRemoved {
        thread_id: thread.clone()
    }));
    // Whatever its session does afterwards, no update for it follows.
    script.die();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let items = inbox(&mut watcher).await;
    assert!(
        items
            .iter()
            .all(|item| item.thread_id.as_ref() != Some(&thread))
    );
    let seen: Vec<ServerMessage> = watcher.aside.drain(..).collect();
    assert!(updates_of(&seen, &thread).is_empty(), "{seen:?}");
    for command in [
        DaemonCommand::MarkRead {
            thread_id: thread.clone(),
        },
        DaemonCommand::ArchiveThread {
            thread_id: thread.clone(),
        },
        say(&thread, "hello?"),
    ] {
        assert_eq!(wire.refused(command).await, ErrorCode::UnknownThread);
    }
    // A new watcher starts from the current set.
    let mut late = rig.dev().await;
    let all = late.subscribe_inbox().await;
    assert_eq!(all["threads"].as_array().expect("threads").len(), 1);
    assert_eq!(all["inbox"][0]["reason"], "approval");
}

#[tokio::test]
async fn rules_settings_and_probes() {
    let rig = Rig::start().await;
    let mut wire = rig.dev().await;

    let draft = |argv: &[&str]| RuleDraft {
        scope: RuleScope::Home,
        tool_kind: ToolKind::Execute,
        tool_name: None,
        matcher: RuleMatcher::CommandPrefix {
            argv: argv.iter().map(|word| (*word).to_owned()).collect(),
        },
        path_scope: None,
        decision: RuleDecision::Deny,
    };
    let created: Rule = serde_json::from_value(
        wire.ok(DaemonCommand::CreateRule {
            rule: draft(&["rm"]),
        })
        .await,
    )
    .expect("a rule");
    assert_eq!(created.origin, RuleOrigin::Settings);
    assert!(created.created_at_ms > 0);
    let second: Rule = serde_json::from_value(
        wire.ok(DaemonCommand::CreateRule {
            rule: draft(&["mv"]),
        })
        .await,
    )
    .expect("a rule");
    assert_ne!(created.id, second.id);
    assert_eq!(
        wire.refused(DaemonCommand::CreateRule { rule: draft(&[]) })
            .await,
        ErrorCode::Invalid
    );
    let listed: Vec<Rule> =
        serde_json::from_value(wire.ok(DaemonCommand::ListRules).await).expect("rules");
    assert_eq!(listed.len(), 2);
    wire.ok(DaemonCommand::DeleteRule {
        rule_id: created.id.clone(),
    })
    .await;
    let listed: Vec<Rule> =
        serde_json::from_value(wire.ok(DaemonCommand::ListRules).await).expect("rules");
    assert_eq!(listed, [second]);

    assert_eq!(wire.ok(DaemonCommand::GetSettings).await, json!({}));
    wire.ok(DaemonCommand::SetSetting {
        key: "theme".into(),
        value: json!({ "name": "dark" }),
    })
    .await;
    // Tests never touch the user's login services (`manage_service`).
    wire.ok(DaemonCommand::SetSetting {
        key: KEEP_RUNNING_SETTING.into(),
        value: json!(true),
    })
    .await;
    assert_eq!(
        wire.ok(DaemonCommand::GetSettings).await,
        json!({ KEEP_RUNNING_SETTING: true, "theme": { "name": "dark" } })
    );
    for (key, value) in [
        (KEEP_RUNNING_SETTING.to_owned(), json!("yes")),
        (String::new(), json!(1)),
        ("k".repeat(200), json!(1)),
    ] {
        assert_eq!(
            wire.refused(DaemonCommand::SetSetting { key, value }).await,
            ErrorCode::Invalid
        );
    }

    let probes: Vec<ProviderProbe> = serde_json::from_value(
        wire.ok(DaemonCommand::ProbeProviders {
            preset: atrium_protocol::policy::Preset::Home,
        })
        .await,
    )
    .expect("probes");
    assert_eq!(probes.len(), 1);
    assert_eq!(probes[0].provider, atrium_protocol::agent::Provider::Claude);
    assert!(probes[0].enforcement.is_some());
    assert_eq!(
        wire.refused(DaemonCommand::GetThread {
            thread_id: ThreadId("th-nobody".into())
        })
        .await,
        ErrorCode::UnknownThread
    );
}

#[tokio::test]
async fn an_export_is_private_and_removed_when_it_fails() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;
    let (thread, mut script) = running(&mut rig, &mut wire).await;
    script.delta("m1", "The answer ").await;
    script.delta("m1", "is 42.").await;
    script.completed("m1", &["The answer ", "is 42."]).await;
    script.turn_done(1).await;
    wire.thread_when(&thread, |summary| summary.state == ThreadState::Idle)
        .await;

    let export = |thread: &ThreadId, format, path: &std::path::Path| DaemonCommand::ExportThread {
        thread_id: thread.clone(),
        format,
        path: path.to_string_lossy().into_owned(),
    };
    let markdown = rig.home.join("thread.md");
    let answer = wire
        .ok(export(&thread, ExportFormat::Markdown, &markdown))
        .await;
    let text = std::fs::read_to_string(&markdown).expect("the export");
    assert_eq!(answer["bytes"], text.len() as u64);
    assert!(text.contains("The answer is 42."), "{text}");
    let mode = std::fs::metadata(&markdown)
        .expect("stat")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);

    let jsonl = rig.home.join("thread.jsonl");
    let answer = wire.ok(export(&thread, ExportFormat::Jsonl, &jsonl)).await;
    let text = std::fs::read_to_string(&jsonl).expect("the export");
    assert_eq!(answer["bytes"], text.len() as u64);
    assert!(text.lines().count() >= 4);

    // Never over something that exists, never a relative path, never into
    // a folder that isn't there.
    for path in [
        markdown.clone(),
        std::path::PathBuf::from("thread.md"),
        rig.home.join("nowhere/thread.md"),
    ] {
        assert_eq!(
            wire.refused(export(&thread, ExportFormat::Markdown, &path))
                .await,
            ErrorCode::Invalid,
            "{path:?}"
        );
    }
    assert_eq!(
        wire.refused(export(
            &ThreadId("th-nobody".into()),
            ExportFormat::Markdown,
            &rig.home.join("nobody.md")
        ))
        .await,
        ErrorCode::UnknownThread
    );
    assert!(!rig.home.join("nobody.md").exists());

    // A message that doesn't match its completion fails the export, and
    // what was written before it goes with the file.
    let damaged = wire.start_thread(native(), Some("go")).await.thread_id;
    let mut script = rig.script().await;
    script.started("native-2").await;
    script.turn(1).await;
    script.delta("m1", "one ").await;
    script.completed("m1", &["one ", "two"]).await;
    script.turn_done(1).await;
    wire.thread_when(&damaged, |summary| summary.state == ThreadState::Idle)
        .await;
    let broken = rig.home.join("damaged.md");
    assert_eq!(
        wire.refused(export(&damaged, ExportFormat::Markdown, &broken))
            .await,
        ErrorCode::Internal
    );
    assert!(!broken.exists(), "the partial export was removed");
}

#[tokio::test]
async fn the_synthetic_agent_runs_turns_and_stops() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;
    let fake = |duration_ms| AgentLaunch::Fake {
        tokens_per_second: 2_000,
        chunk_chars: 16,
        duration_ms,
        tool_call_every_ms: 100,
    };
    let thread = wire.start_thread(fake(300), Some("hi")).await.thread_id;
    wire.thread_when(&thread, |summary| summary.state == ThreadState::Idle)
        .await;
    let history = wire.history(&thread).await;
    let shape = kinds(&history);
    assert_eq!(
        shape[..3],
        ["session_started", "turn_started", "message_completed"]
    );
    assert_eq!(
        shape[shape.len() - 2..],
        ["message_completed", "turn_completed"]
    );
    assert_eq!(
        shape
            .iter()
            .filter(|kind| **kind == "tool_call_started")
            .count(),
        3
    );
    assert_eq!(
        shape
            .iter()
            .filter(|kind| **kind == "tool_call_finished")
            .count(),
        3
    );
    // 300 ms at 2,000 tokens a second, four characters a token.
    let text: String = history
        .iter()
        .filter_map(|event| match &event.envelope.event {
            AgentEvent::MessageDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text.len(), 2_400);
    let completed = history
        .iter()
        .rev()
        .find_map(|event| match &event.envelope.event {
            AgentEvent::MessageCompleted {
                kind: MessageKind::Assistant,
                body,
                ..
            } => Some(body.clone()),
            _ => None,
        });
    assert_eq!(
        completed,
        Some(MessageBody::Streamed {
            chars: 2_400,
            deltas: 150,
            sha256: sha256_hex(&text),
        })
    );
    for (at, event) in history.iter().enumerate() {
        assert_eq!(event.envelope.seq, at as u64);
    }

    // A second turn, cut short.
    let long = wire.start_thread(fake(600_000), Some("hi")).await.thread_id;
    wire.thread_when(&long, |summary| summary.unread).await;
    wire.ok(DaemonCommand::ThreadCommand {
        thread_id: long.clone(),
        command: SessionCommand::Interrupt,
    })
    .await;
    wire.thread_when(&long, |summary| summary.state == ThreadState::Idle)
        .await;
    let history = wire.history(&long).await;
    assert!(matches!(
        history.last().expect("events").envelope.event,
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Interrupted,
            ..
        }
    ));
    wire.ok(say(&long, "again")).await;
    wire.thread_when(&long, |summary| summary.state == ThreadState::Running)
        .await;
    wire.ok(shutdown(&long)).await;
    soon(async {
        while kinds(&wire.history(&long).await).last() != Some(&"session_ended") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert_eq!(wire.thread(&long).await.state, ThreadState::Ended);
    // Nothing to resume: it has no provider session.
    assert_eq!(
        wire.refused(DaemonCommand::ResumeThread { thread_id: long })
            .await,
        ErrorCode::Invalid
    );
    rig.stop(true).await;
}

#[tokio::test]
async fn a_snapshot_is_made_once_a_day_when_no_turn_runs() {
    let mut rig = Rig::start_with(|options| {
        options.snapshot_check_every = Duration::from_millis(50);
    })
    .await;
    let dir = rig.paths.snapshots_dir.clone();
    let count = || std::fs::read_dir(&dir).map_or(0, |entries| entries.count());
    common::until(|| count() == 1).await;
    // Not again at the next look: today's exists.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(count(), 1);
    let file = std::fs::read_dir(&dir)
        .expect("the folder")
        .next()
        .expect("a file")
        .expect("an entry")
        .path();
    let mode = std::fs::metadata(&file).expect("stat").permissions().mode();
    assert_eq!(mode & 0o777, 0o600);

    // With none left, a new one is due, but not while a turn runs.
    let mut wire = rig.dev().await;
    let (thread, mut script) = running(&mut rig, &mut wire).await;
    std::fs::remove_file(&file).expect("removes");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(count(), 0);
    script.turn_done(1).await;
    wire.thread_when(&thread, |summary| summary.state == ThreadState::Idle)
        .await;
    common::until(|| count() == 1).await;
}
