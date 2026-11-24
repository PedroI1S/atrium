//! CR-18 / P2: `SessionGate::withdraw` / `ToolGate::withdraw` resolves an open
//! ticket as cancelled, frees its ledger slot against `MAX_PENDING_REQUESTS`,
//! prevents redemption, and is idempotent.

use std::sync::Arc;

use atrium_agents::{ApprovalTicket, GateDecision, ToolCheck, ToolGate};
use atrium_core::policy::{DefaultPolicy, Ledger, SessionGate, SessionGateConfig, StoreRules};
use atrium_core::store::{SqliteStore, StoreOptions};
use atrium_core::{ApprovalLedger, LedgerError, NewThread, RequestKey, Signal, Store};
use atrium_protocol::agent::{ApprovalDecision, Provider, ToolKind};
use atrium_protocol::limits::MAX_PENDING_REQUESTS;
use atrium_protocol::policy::{Preset, ToolSubject};
use atrium_protocol::thread::{Location, ThreadId};
use serde_json::json;

struct Harness {
    _dir: tempfile::TempDir,
    home: std::path::PathBuf,
    store: Arc<SqliteStore>,
    ledger: Arc<Ledger>,
}

impl Harness {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let home = std::fs::canonicalize(home).unwrap();
        let store = Arc::new(
            SqliteStore::open(&dir.path().join("atrium.db"), StoreOptions::default())
                .await
                .unwrap(),
        );
        let ledger = Arc::new(Ledger::new(store.requests()));
        Self {
            _dir: dir,
            home,
            store,
            ledger,
        }
    }

    async fn thread(&self) -> (ThreadId, u32) {
        let s = self
            .store
            .create_thread(NewThread {
                provider: Provider::Claude,
                model: None,
                effort: None,
                preset: Preset::Home,
                location: Location::Home,
                title: "withdraw-test".into(),
            })
            .await
            .unwrap();
        let inc = self.store.begin_incarnation(&s.thread_id).await.unwrap();
        self.store
            .signal(&s.thread_id, Signal::Starting)
            .await
            .unwrap();
        (s.thread_id, inc)
    }

    fn gate(&self, thread: &ThreadId, incarnation: u32) -> Arc<SessionGate> {
        Arc::new(SessionGate::new(
            SessionGateConfig {
                thread: thread.clone(),
                incarnation,
                cwd: self.home.clone(),
                home: self.home.clone(),
                atrium_data_paths: vec![self.home.join(".local/share/atrium")],
                atrium_protected_paths: vec![self.home.join(".config/atrium")],
                preset: Preset::Home,
                provider: Provider::Claude,
            },
            Arc::new(DefaultPolicy::new()),
            self.ledger.clone(),
            Arc::new(StoreRules(self.store.clone())),
        ))
    }
}

fn cmd_check(line: &str) -> ToolCheck {
    ToolCheck {
        call_id: None,
        name: "Bash".into(),
        kind: ToolKind::Execute,
        title: None,
        input: json!({ "command": line }),
        subject: ToolSubject::Command {
            line: line.into(),
            cwd: None,
        },
        turn: 1,
    }
}

fn key(thread: &ThreadId, incarnation: u32, ticket: &ApprovalTicket) -> RequestKey {
    RequestKey {
        thread: thread.clone(),
        incarnation,
        request_id: ticket.request_id.clone(),
    }
}

#[tokio::test]
async fn withdrawn_tickets_free_slots_against_max_pending_requests() {
    let h = Harness::new().await;
    let (thread, inc) = h.thread().await;
    let gate: Arc<dyn ToolGate> = h.gate(&thread, inc);

    // Fill the incarnation up to MAX_PENDING_REQUESTS (mixing approvals and questions).
    let mut approval_tickets = Vec::new();
    for i in 0..(MAX_PENDING_REQUESTS - 1) {
        let check = cmd_check(&format!("touch file-{i}.txt"));
        match gate.check(&check).await {
            GateDecision::Ask { ticket } => approval_tickets.push(ticket),
            other => panic!("expected Ask at slot {i}, got {other:?}"),
        }
    }
    let q_ticket = gate
        .question_ticket(Some(1))
        .await
        .expect("question slot should open");

    // Next check and question are refused while all slots are pending.
    let overflow_check = cmd_check("touch overflow.txt");
    assert!(
        matches!(gate.check(&overflow_check).await, GateDecision::Deny { .. }),
        "opening past MAX_PENDING_REQUESTS must deny"
    );
    assert!(
        gate.question_ticket(Some(1)).await.is_none(),
        "question past MAX_PENDING_REQUESTS must return None"
    );

    // Withdraw one approval ticket and the question ticket via ToolGate::withdraw.
    gate.withdraw(&approval_tickets[0]).await;
    gate.withdraw(&q_ticket).await;

    let requests = h.store.requests();
    assert_eq!(
        requests.open_count(&thread, inc).await.unwrap(),
        (MAX_PENDING_REQUESTS - 2) as u64
    );

    // Two slots are now free: one approval and one question can open again.
    let reopened = match gate.check(&overflow_check).await {
        GateDecision::Ask { ticket } => ticket,
        other => panic!("expected Ask after withdraw, got {other:?}"),
    };
    let reopened_q = gate
        .question_ticket(Some(1))
        .await
        .expect("question slot should reopen after withdraw");
    assert_ne!(reopened.request_id, approval_tickets[0].request_id);
    assert_ne!(reopened_q.request_id, q_ticket.request_id);

    // At the limit again: next one is refused.
    assert!(matches!(
        gate.check(&cmd_check("touch overflow-2.txt")).await,
        GateDecision::Deny { .. }
    ));
}

#[tokio::test]
async fn withdrawn_ticket_cannot_be_redeemed_and_never_becomes_approval() {
    let h = Harness::new().await;
    let (thread, inc) = h.thread().await;
    let gate = h.gate(&thread, inc);

    // Case 1: pending ticket withdrawn -> cannot be resolved or redeemed.
    let c1 = cmd_check("touch one.txt");
    let t1 = match gate.check(&c1).await {
        GateDecision::Ask { ticket } => ticket,
        other => panic!("expected Ask, got {other:?}"),
    };
    gate.withdraw(&t1).await;
    assert!(matches!(
        gate.redeem(&t1, &c1).await,
        GateDecision::Deny { .. }
    ));
    assert_eq!(
        h.ledger
            .resolve(&key(&thread, inc, &t1), &ApprovalDecision::Allow, None)
            .await,
        Err(LedgerError::Expired)
    );
    assert!(matches!(
        gate.redeem(&t1, &c1).await,
        GateDecision::Deny { .. }
    ));

    // Case 2: approved (not yet redeemed) ticket withdrawn -> cannot be redeemed.
    let c2 = cmd_check("touch two.txt");
    let t2 = match gate.check(&c2).await {
        GateDecision::Ask { ticket } => ticket,
        other => panic!("expected Ask, got {other:?}"),
    };
    h.ledger
        .resolve(&key(&thread, inc, &t2), &ApprovalDecision::Allow, None)
        .await
        .unwrap();
    gate.withdraw(&t2).await;
    assert!(matches!(
        gate.redeem(&t2, &c2).await,
        GateDecision::Deny { .. }
    ));
    let (status, _) = h
        .store
        .requests()
        .binding(&key(&thread, inc, &t2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status, "cancelled");
}

#[tokio::test]
async fn withdraw_is_idempotent_across_resolved_redeemed_and_unknown_tickets() {
    let h = Harness::new().await;
    let (thread, inc) = h.thread().await;
    let gate = h.gate(&thread, inc);

    // Pending -> withdraw twice (second call is a no-op).
    let c_pending = cmd_check("touch p.txt");
    let t_pending = match gate.check(&c_pending).await {
        GateDecision::Ask { ticket } => ticket,
        other => panic!("expected Ask, got {other:?}"),
    };
    gate.withdraw(&t_pending).await;
    gate.withdraw(&t_pending).await;

    // Redeemed -> withdraw is a no-op and keeps status = redeemed.
    let c_redeemed = cmd_check("touch r.txt");
    let t_redeemed = match gate.check(&c_redeemed).await {
        GateDecision::Ask { ticket } => ticket,
        other => panic!("expected Ask, got {other:?}"),
    };
    h.ledger
        .resolve(
            &key(&thread, inc, &t_redeemed),
            &ApprovalDecision::Allow,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        gate.redeem(&t_redeemed, &c_redeemed).await,
        GateDecision::Allow
    );
    gate.withdraw(&t_redeemed).await;
    let (status, _) = h
        .store
        .requests()
        .binding(&key(&thread, inc, &t_redeemed))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status, "redeemed");

    // Denied -> withdraw is a no-op and keeps status = denied.
    let c_denied = cmd_check("touch d.txt");
    let t_denied = match gate.check(&c_denied).await {
        GateDecision::Ask { ticket } => ticket,
        other => panic!("expected Ask, got {other:?}"),
    };
    h.ledger
        .resolve(
            &key(&thread, inc, &t_denied),
            &ApprovalDecision::Deny { message: None },
            None,
        )
        .await
        .unwrap();
    gate.withdraw(&t_denied).await;
    let (status, _) = h
        .store
        .requests()
        .binding(&key(&thread, inc, &t_denied))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status, "denied");

    // Answered question -> withdraw is a no-op.
    let q_answered = gate.question_ticket(Some(1)).await.unwrap();
    h.ledger
        .answer(&key(&thread, inc, &q_answered))
        .await
        .unwrap();
    gate.withdraw(&q_answered).await;

    // Unknown ticket -> withdraw is a no-op.
    gate.withdraw(&ApprovalTicket::new("unknown-ticket".into(), false))
        .await;

    // None of the withdrawn/final tickets count as open.
    assert_eq!(
        h.store.requests().open_count(&thread, inc).await.unwrap(),
        0
    );
}
