use atrium_daemon::{
    NOTIFY_APPROVAL_SETTING, NOTIFY_FAILED_SETTING, NOTIFY_QUESTION_SETTING, NotifyReason,
};
use atrium_protocol::agent::SessionCommand;
use atrium_protocol::thread::{ThreadId, ThreadState};
use atrium_protocol::wire::{AgentLaunch, DaemonCommand, ErrorCode};
use serde_json::{Value, json};

#[allow(dead_code)]
mod common;

use common::{Rig, Start, native};

fn fake_approval() -> AgentLaunch {
    AgentLaunch::Fake {
        tokens_per_second: 100,
        chunk_chars: 8,
        duration_ms: 2_000,
        tool_call_every_ms: 0,
        approval_at_ms: Some(10),
        question_at_ms: None,
    }
}

fn fake_question() -> AgentLaunch {
    AgentLaunch::Fake {
        tokens_per_second: 100,
        chunk_chars: 8,
        duration_ms: 2_000,
        tool_call_every_ms: 0,
        approval_at_ms: None,
        question_at_ms: Some(10),
    }
}

fn interrupt(thread_id: &ThreadId) -> DaemonCommand {
    DaemonCommand::ThreadCommand {
        thread_id: thread_id.clone(),
        command: SessionCommand::Interrupt,
    }
}

async fn set_setting(wire: &mut common::Wire, key: &str, value: Value) {
    wire.ok(DaemonCommand::SetSetting {
        key: key.into(),
        value,
    })
    .await;
}

#[tokio::test]
async fn notify_settings_require_booleans() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;

    for key in [
        NOTIFY_APPROVAL_SETTING,
        NOTIFY_QUESTION_SETTING,
        NOTIFY_FAILED_SETTING,
    ] {
        for bad in [json!("yes"), json!(1), json!(null)] {
            assert_eq!(
                wire.refused(DaemonCommand::SetSetting {
                    key: key.into(),
                    value: bad,
                })
                .await,
                ErrorCode::Invalid
            );
        }
        set_setting(&mut wire, key, json!(false)).await;
    }

    let settings = wire.ok(DaemonCommand::GetSettings).await;
    assert_eq!(
        settings,
        json!({
            NOTIFY_APPROVAL_SETTING: false,
            NOTIFY_QUESTION_SETTING: false,
            NOTIFY_FAILED_SETTING: false,
        })
    );

    rig.stop(true).await;
}

#[tokio::test]
async fn notify_switches_gate_each_reason_and_persist_across_restart() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;

    // 1. Default (absent): all three notify.
    let t_a1 = wire
        .start_thread(fake_approval(), Some("a1"))
        .await
        .thread_id;
    wire.thread_when(&t_a1, |s| s.state == ThreadState::NeedsApproval)
        .await;
    wire.ok(interrupt(&t_a1)).await;
    wire.thread_when(&t_a1, |s| s.state == ThreadState::Idle)
        .await;

    let t_q1 = wire
        .start_thread(fake_question(), Some("q1"))
        .await
        .thread_id;
    wire.thread_when(&t_q1, |s| s.state == ThreadState::NeedsInput)
        .await;
    wire.ok(interrupt(&t_q1)).await;
    wire.thread_when(&t_q1, |s| s.state == ThreadState::Idle)
        .await;

    *rig.adapter.start.lock().expect("lock") = Start::Fail;
    let t_f1 = wire.start_thread(native(), Some("f1")).await.thread_id;
    wire.thread_when(&t_f1, |s| matches!(s.state, ThreadState::Failed { .. }))
        .await;
    *rig.adapter.start.lock().expect("lock") = Start::Script;

    let notes = rig.notes();
    assert_eq!(
        notes
            .iter()
            .map(|n| (&n.thread_id, n.reason))
            .collect::<Vec<_>>(),
        vec![
            (&t_a1, NotifyReason::Approval),
            (&t_q1, NotifyReason::Question),
            (&t_f1, NotifyReason::Failed),
        ]
    );

    // 2. Disable notify_approval: approval does not post, question and failed still post.
    set_setting(&mut wire, NOTIFY_APPROVAL_SETTING, json!(false)).await;
    let t_a2 = wire
        .start_thread(fake_approval(), Some("a2"))
        .await
        .thread_id;
    wire.thread_when(&t_a2, |s| s.state == ThreadState::NeedsApproval)
        .await;
    wire.ok(interrupt(&t_a2)).await;
    wire.thread_when(&t_a2, |s| s.state == ThreadState::Idle)
        .await;
    assert_eq!(rig.notes().len(), 3, "approval notification suppressed");

    let t_q2 = wire
        .start_thread(fake_question(), Some("q2"))
        .await
        .thread_id;
    wire.thread_when(&t_q2, |s| s.state == ThreadState::NeedsInput)
        .await;
    wire.ok(interrupt(&t_q2)).await;
    wire.thread_when(&t_q2, |s| s.state == ThreadState::Idle)
        .await;
    assert_eq!(rig.notes().len(), 4);
    assert_eq!(rig.notes()[3].reason, NotifyReason::Question);

    *rig.adapter.start.lock().expect("lock") = Start::Fail;
    let t_f2 = wire.start_thread(native(), Some("f2")).await.thread_id;
    wire.thread_when(&t_f2, |s| matches!(s.state, ThreadState::Failed { .. }))
        .await;
    *rig.adapter.start.lock().expect("lock") = Start::Script;
    assert_eq!(rig.notes().len(), 5);
    assert_eq!(rig.notes()[4].reason, NotifyReason::Failed);

    // 3. Re-enable approval, disable question & failed, and restart to verify startup loading.
    set_setting(&mut wire, NOTIFY_APPROVAL_SETTING, json!(true)).await;
    set_setting(&mut wire, NOTIFY_QUESTION_SETTING, json!(false)).await;
    set_setting(&mut wire, NOTIFY_FAILED_SETTING, json!(false)).await;
    drop(wire);
    rig.stop(true).await;
    rig.boot(|_| {}).await;
    let mut wire = rig.dev().await;

    let t_q3 = wire
        .start_thread(fake_question(), Some("q3"))
        .await
        .thread_id;
    wire.thread_when(&t_q3, |s| s.state == ThreadState::NeedsInput)
        .await;
    wire.ok(interrupt(&t_q3)).await;
    wire.thread_when(&t_q3, |s| s.state == ThreadState::Idle)
        .await;
    assert_eq!(
        rig.notes().len(),
        5,
        "question stayed suppressed after restart"
    );

    *rig.adapter.start.lock().expect("lock") = Start::Fail;
    let t_f3 = wire.start_thread(native(), Some("f3")).await.thread_id;
    wire.thread_when(&t_f3, |s| matches!(s.state, ThreadState::Failed { .. }))
        .await;
    *rig.adapter.start.lock().expect("lock") = Start::Script;
    assert_eq!(
        rig.notes().len(),
        5,
        "failed stayed suppressed after restart"
    );

    let t_a3 = wire
        .start_thread(fake_approval(), Some("a3"))
        .await
        .thread_id;
    wire.thread_when(&t_a3, |s| s.state == ThreadState::NeedsApproval)
        .await;
    wire.ok(interrupt(&t_a3)).await;
    wire.thread_when(&t_a3, |s| s.state == ThreadState::Idle)
        .await;
    assert_eq!(
        rig.notes().len(),
        6,
        "re-enabled approval notifies after restart"
    );
    assert_eq!(rig.notes()[5].reason, NotifyReason::Approval);
    assert_eq!(rig.notes()[5].thread_id, t_a3);

    rig.stop(true).await;
}
