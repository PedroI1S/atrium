#[path = "common/mod.rs"]
mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use atrium_protocol::agent::{
    AgentEvent, MessageBody, MessageKind, SessionCommand, ToolKind, ToolStatus, TurnOutcome,
};
use atrium_protocol::thread::{Cleanup, InboxItem, InboxReason, ThreadId, ThreadState};
use atrium_protocol::wire::{DaemonCommand, StoredEvent};
use common::{Rig, Script, Wire, kinds, native, sha256_hex, soon};
use serde_json::json;

async fn inbox(wire: &mut Wire) -> Vec<InboxItem> {
    serde_json::from_value(wire.ok(DaemonCommand::ListInbox).await).expect("an inbox")
}

async fn start_running(rig: &mut Rig, wire: &mut Wire, native_id: &str) -> (ThreadId, Script) {
    let thread = wire.start_thread(native(), Some("go")).await.thread_id;
    let mut script = rig.script().await;
    assert_eq!(
        script.command().await,
        SessionCommand::SendUserMessage { text: "go".into() }
    );
    script.started(native_id).await;
    script.turn(1).await;
    wire.thread_when(&thread, |summary| summary.state == ThreadState::Running)
        .await;
    (thread, script)
}

async fn wait_for_kind(wire: &mut Wire, thread: &ThreadId, expected: &str) {
    soon(async {
        while !kinds(&wire.history(thread).await).contains(&expected) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
}

async fn wait_for_ended(wire: &mut Wire, thread: &ThreadId) -> Vec<StoredEvent> {
    soon(async {
        loop {
            let history = wire.history(thread).await;
            if kinds(&history).last() == Some(&"session_ended") {
                return history;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
}

#[tokio::test]
async fn full_storage_closes_open_message_from_store_not_refused_batch() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;

    let (msg_thread, mut msg_script) = start_running(&mut rig, &mut wire, "native-msg").await;
    msg_script.delta("m", "hello").await;
    wait_for_kind(&mut wire, &msg_thread, "message_delta").await;

    rig.free_bytes.store(1024, Ordering::Relaxed);
    let summary = wire
        .thread_when(&msg_thread, |summary| {
            matches!(summary.state, ThreadState::Interrupted { .. })
        })
        .await;
    assert_eq!(
        summary.state,
        ThreadState::Interrupted {
            cleanup: Cleanup::Verified
        }
    );
    assert_eq!(msg_script.command().await, SessionCommand::Shutdown);

    // Send a batch holding another delta, MessageCompleted, and TurnCompleted
    // (which the full store refuses), followed later by SessionEnded(Some(0)).
    // Before a27d432, the refused batch was dropped and SessionEnded alone was
    // kept, leaving the message and turn open without "storage is full".
    msg_script.delta("m", "more").await;
    msg_script.completed("m", &["hello", "more"]).await;
    msg_script.turn_done(1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    msg_script.ended(Some(0)).await;

    let history = wait_for_ended(&mut wire, &msg_thread).await;
    assert_eq!(
        kinds(&history),
        [
            "session_started",
            "turn_started",
            "message_delta",
            "message_completed",
            "turn_completed",
            "session_ended",
        ]
    );
    for (at, event) in history.iter().enumerate() {
        assert_eq!(event.envelope.seq, at as u64);
    }
    assert_eq!(
        history[3].envelope.event,
        AgentEvent::MessageCompleted {
            message_id: "m".into(),
            kind: MessageKind::Assistant,
            body: MessageBody::Streamed {
                chars: 5,
                deltas: 1,
                sha256: sha256_hex("hello"),
            },
        }
    );
    assert_eq!(
        history[4].envelope.event,
        AgentEvent::TurnCompleted {
            turn: 1,
            outcome: TurnOutcome::Interrupted,
            usage: None,
        }
    );
    assert_eq!(
        history[5].envelope.event,
        AgentEvent::SessionEnded {
            exit_code: None,
            reason: Some("storage is full".into()),
        }
    );
    assert!(
        inbox(&mut wire)
            .await
            .iter()
            .any(|item| item.reason == InboxReason::StorageFull)
    );

    // Restore space and verify the incarnation is cleanly closed and resumable.
    rig.free_bytes.store(u64::MAX, Ordering::Relaxed);
    soon(async {
        while inbox(&mut wire)
            .await
            .iter()
            .any(|item| item.reason == InboxReason::StorageFull)
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    wire.ok(DaemonCommand::ResumeThread {
        thread_id: msg_thread,
    })
    .await;
    let mut resumed_script = rig.script().await;
    resumed_script.started("native-msg-2").await;
    resumed_script.ended(Some(0)).await;

    rig.stop(false).await;
}

#[tokio::test]
async fn full_storage_closes_open_tool_call_from_store_not_refused_batch() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;

    let (tool_thread, mut tool_script) = start_running(&mut rig, &mut wire, "native-tool").await;
    tool_script
        .emit(AgentEvent::ToolCallStarted {
            call_id: "c1".into(),
            name: "Bash".into(),
            kind: ToolKind::Execute,
            title: Some("echo hi".into()),
            input: json!({ "command": "echo hi" }),
        })
        .await;
    wait_for_kind(&mut wire, &tool_thread, "tool_call_started").await;

    rig.free_bytes.store(1024, Ordering::Relaxed);
    let summary = wire
        .thread_when(&tool_thread, |summary| {
            matches!(summary.state, ThreadState::Interrupted { .. })
        })
        .await;
    assert_eq!(
        summary.state,
        ThreadState::Interrupted {
            cleanup: Cleanup::Verified
        }
    );
    assert_eq!(tool_script.command().await, SessionCommand::Shutdown);

    // Send a delta, ToolCallFinished(Succeeded), and TurnCompleted (refused),
    // followed by SessionEnded(Some(0)).
    tool_script.delta("m", "after").await;
    tool_script
        .emit(AgentEvent::ToolCallFinished {
            call_id: "c1".into(),
            status: ToolStatus::Succeeded,
            output: Some("hi\n".into()),
            error: None,
            truncated_bytes: 0,
        })
        .await;
    tool_script.turn_done(1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    tool_script.ended(Some(0)).await;

    let history = wait_for_ended(&mut wire, &tool_thread).await;
    assert_eq!(
        kinds(&history),
        [
            "session_started",
            "turn_started",
            "tool_call_started",
            "tool_call_finished",
            "turn_completed",
            "session_ended",
        ]
    );
    for (at, event) in history.iter().enumerate() {
        assert_eq!(event.envelope.seq, at as u64);
    }
    assert_eq!(
        history[3].envelope.event,
        AgentEvent::ToolCallFinished {
            call_id: "c1".into(),
            status: ToolStatus::Cancelled,
            output: None,
            error: None,
            truncated_bytes: 0,
        }
    );
    assert_eq!(
        history[4].envelope.event,
        AgentEvent::TurnCompleted {
            turn: 1,
            outcome: TurnOutcome::Interrupted,
            usage: None,
        }
    );
    assert_eq!(
        history[5].envelope.event,
        AgentEvent::SessionEnded {
            exit_code: None,
            reason: Some("storage is full".into()),
        }
    );
    assert!(
        inbox(&mut wire)
            .await
            .iter()
            .any(|item| item.reason == InboxReason::StorageFull)
    );

    rig.free_bytes.store(u64::MAX, Ordering::Relaxed);
    soon(async {
        while inbox(&mut wire)
            .await
            .iter()
            .any(|item| item.reason == InboxReason::StorageFull)
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    wire.ok(DaemonCommand::ResumeThread {
        thread_id: tool_thread,
    })
    .await;
    let mut resumed_tool = rig.script().await;
    resumed_tool.started("native-tool-2").await;
    resumed_tool.ended(Some(0)).await;

    rig.stop(false).await;
}
