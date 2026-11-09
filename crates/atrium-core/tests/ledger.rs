//! M1-03: approval ledger (contract §5, D-046). M1-13 adds adversarial cases.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use atrium_agents::{BoxFuture, GateDecision, ToolCheck, ToolGate};
use atrium_core::policy::{
    DefaultPolicy, Ledger, RulesSource, SessionGate, SessionGateConfig, input_sha256,
};
use atrium_core::{
    ApprovalLedger, Binding, LedgerError, RequestKey, RequestStore, StoreError, StoreResult,
};
use atrium_protocol::agent::{ApprovalDecision, Provider, ToolKind};
use atrium_protocol::limits::MAX_PENDING_REQUESTS;
use atrium_protocol::policy::{
    PolicyContext, Preset, RememberScope, Rule, RuleDecision, RuleId, RuleMatcher, RuleOrigin,
    RuleScope, ToolSubject,
};
use atrium_protocol::thread::ThreadId;
use serde_json::json;

// ---------------------------------------------------------------------------
// In-memory RequestStore with the semantics contract §5 gives the SQL store
// ---------------------------------------------------------------------------

type Row = (String, Option<Binding>, Option<u32>);

#[derive(Default)]
struct Inner {
    rows: HashMap<RequestKey, Row>,
    rules: Vec<Rule>,
    rev: u64,
    current: HashMap<ThreadId, u32>,
}

#[derive(Default)]
struct MemStore(Mutex<Inner>);

impl MemStore {
    fn status(&self, key: &RequestKey) -> Option<String> {
        self.0.lock().unwrap().rows.get(key).map(|r| r.0.clone())
    }
    fn set_current(&self, thread: &ThreadId, inc: u32) {
        self.0.lock().unwrap().current.insert(thread.clone(), inc);
    }
}

impl RequestStore for MemStore {
    fn current_incarnation(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<Option<u32>>> {
        let thread = thread.clone();
        Box::pin(async move { Ok(self.0.lock().unwrap().current.get(&thread).copied()) })
    }
    fn open_approval(&self, key: &RequestKey, binding: &Binding) -> BoxFuture<'_, StoreResult<()>> {
        let (key, binding) = (key.clone(), binding.clone());
        Box::pin(async move {
            let mut g = self.0.lock().unwrap();
            if g.rows.contains_key(&key) {
                return Err(StoreError::Sqlite("duplicate request id".into()));
            }
            g.rows.insert(
                key,
                ("pending".into(), Some(binding.clone()), Some(binding.turn)),
            );
            Ok(())
        })
    }
    fn open_question(&self, key: &RequestKey, turn: Option<u32>) -> BoxFuture<'_, StoreResult<()>> {
        let key = key.clone();
        Box::pin(async move {
            let mut g = self.0.lock().unwrap();
            if g.rows.contains_key(&key) {
                return Err(StoreError::Sqlite("duplicate request id".into()));
            }
            g.rows.insert(key, ("pending".into(), None, turn));
            Ok(())
        })
    }
    fn transition(
        &self,
        key: &RequestKey,
        from: &[&str],
        to: &str,
    ) -> BoxFuture<'_, StoreResult<Option<String>>> {
        let key = key.clone();
        let from: Vec<String> = from.iter().map(|s| s.to_string()).collect();
        let to = to.to_string();
        Box::pin(async move {
            tokio::task::yield_now().await;
            let mut g = self.0.lock().unwrap();
            Ok(g.rows.get_mut(&key).map(|r| {
                let found = r.0.clone();
                if from.contains(&found) {
                    r.0 = to;
                }
                found
            }))
        })
    }
    fn open_count(&self, thread: &ThreadId, incarnation: u32) -> BoxFuture<'_, StoreResult<u64>> {
        let thread = thread.clone();
        Box::pin(async move {
            let g = self.0.lock().unwrap();
            Ok(g.rows
                .iter()
                .filter(|(k, v)| {
                    k.thread == thread
                        && k.incarnation == incarnation
                        && matches!(v.0.as_str(), "pending" | "approved")
                })
                .count() as u64)
        })
    }
    fn approve_with_rule(
        &self,
        key: &RequestKey,
        rule: Option<Rule>,
    ) -> BoxFuture<'_, StoreResult<Option<String>>> {
        let key = key.clone();
        Box::pin(async move {
            tokio::task::yield_now().await;
            let mut g = self.0.lock().unwrap();
            let Some(r) = g.rows.get_mut(&key) else {
                return Ok(None);
            };
            let found = r.0.clone();
            if found == "pending" {
                r.0 = "approved".into();
                if let Some(rule) = rule {
                    g.rules.push(rule);
                    g.rev += 1;
                }
            }
            Ok(Some(found))
        })
    }
    fn binding(&self, key: &RequestKey) -> BoxFuture<'_, StoreResult<Option<(String, Binding)>>> {
        let key = key.clone();
        Box::pin(async move {
            let g = self.0.lock().unwrap();
            Ok(g.rows
                .get(&key)
                .and_then(|r| r.1.clone().map(|b| (r.0.clone(), b))))
        })
    }
    fn expire(
        &self,
        thread: Option<&ThreadId>,
        incarnation: Option<u32>,
        turn: Option<u32>,
    ) -> BoxFuture<'_, StoreResult<u64>> {
        let thread = thread.cloned();
        Box::pin(async move {
            let mut g = self.0.lock().unwrap();
            let mut n = 0;
            for (k, v) in g.rows.iter_mut() {
                if thread.as_ref().is_some_and(|t| *t != k.thread)
                    || incarnation.is_some_and(|i| i != k.incarnation)
                    || turn.is_some_and(|t| v.2 != Some(t))
                {
                    continue;
                }
                if matches!(v.0.as_str(), "pending" | "approved") {
                    v.0 = "expired".into();
                    n += 1;
                }
            }
            Ok(n)
        })
    }
}

struct MemRules(Arc<MemStore>);

impl RulesSource for MemRules {
    fn snapshot(&self) -> BoxFuture<'_, Option<(u64, Vec<Rule>)>> {
        Box::pin(async move {
            let g = self.0.0.lock().unwrap();
            Some((g.rev, g.rules.clone()))
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn thread() -> ThreadId {
    ThreadId("t1".into())
}

fn setup() -> (Arc<MemStore>, Arc<Ledger>) {
    let store = Arc::new(MemStore::default());
    store.set_current(&thread(), 1);
    let ledger = Arc::new(Ledger::new(store.clone()));
    (store, ledger)
}

fn check(turn: u32, line: &str) -> ToolCheck {
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
        turn,
    }
}

fn binding(c: &ToolCheck, matcher: Option<RuleMatcher>) -> Binding {
    Binding {
        turn: c.turn,
        tool_name: c.name.clone(),
        tool_kind: c.kind,
        input_sha256: input_sha256(c),
        context: PolicyContext {
            rules_revision: 0,
            preset: Preset::Home,
        },
        title: None,
        remember_matcher: matcher,
    }
}

fn key(t: &atrium_agents::ApprovalTicket) -> RequestKey {
    RequestKey {
        thread: thread(),
        incarnation: 1,
        request_id: t.request_id.clone(),
    }
}

async fn open(l: &Ledger, c: &ToolCheck) -> RequestKey {
    key(&l
        .open_approval(&thread(), 1, binding(c, None))
        .await
        .unwrap())
}

async fn open_and_allow(l: &Ledger, c: &ToolCheck) -> RequestKey {
    let k = open(l, c).await;
    l.resolve(&k, &ApprovalDecision::Allow, None).await.unwrap();
    k
}

fn prefix(argv: &[&str]) -> RuleMatcher {
    RuleMatcher::CommandPrefix {
        argv: argv.iter().map(|s| s.to_string()).collect(),
    }
}

// ---------------------------------------------------------------------------
// The ledger
// ---------------------------------------------------------------------------

#[tokio::test]
async fn allow_once_redeems_exactly_once() {
    let (store, l) = setup();
    let c = check(1, "touch x");
    let k = open_and_allow(&l, &c).await;
    assert_eq!(store.status(&k).as_deref(), Some("approved"));
    assert_eq!(l.redeem(&k, &c).await, Ok(()));
    assert_eq!(store.status(&k).as_deref(), Some("redeemed"));
    assert_eq!(l.redeem(&k, &c).await, Err(LedgerError::AlreadyResolved));
    // nothing leaves `redeemed`
    assert_eq!(l.withdraw(&k).await, Err(LedgerError::AlreadyResolved));
    assert_eq!(l.expire_incarnation(&thread(), 1).await, 0);
    assert_eq!(l.expire_all().await, 0);
    assert_eq!(store.status(&k).as_deref(), Some("redeemed"));
}

#[tokio::test]
async fn redeem_rejects_changed_input() {
    let (_s, l) = setup();
    let c = check(1, "touch x");
    let k = open_and_allow(&l, &c).await;
    assert_eq!(
        l.redeem(&k, &check(1, "touch y")).await,
        Err(LedgerError::Mismatch)
    );
    assert_eq!(
        l.redeem(&k, &check(1, "touch x ")).await,
        Err(LedgerError::Mismatch)
    );
    let mut other_name = c.clone();
    other_name.name = "Run".into();
    assert_eq!(l.redeem(&k, &other_name).await, Err(LedgerError::Mismatch));
    let mut other_kind = c.clone();
    other_kind.kind = ToolKind::Edit;
    assert_eq!(l.redeem(&k, &other_kind).await, Err(LedgerError::Mismatch));
    // a different spelling of the same JSON is the same call
    let mut respelled = c.clone();
    respelled.input = serde_json::from_str("{ \"command\" :  \"touch x\" }").unwrap();
    assert_eq!(l.redeem(&k, &respelled).await, Ok(()));
}

#[tokio::test]
async fn resolve_after_turn_end_is_expired_and_never_redeemable() {
    let (store, l) = setup();
    let c = check(3, "touch x");
    let k = open(&l, &c).await;
    let approved = open_and_allow(&l, &c).await;
    assert_eq!(l.expire_turn(&thread(), 1, 3).await, 2);
    assert_eq!(
        l.resolve(&k, &ApprovalDecision::Allow, None).await,
        Err(LedgerError::Expired)
    );
    assert_eq!(
        l.resolve(&k, &ApprovalDecision::Allow, Some(RememberScope::Global))
            .await,
        Err(LedgerError::Expired)
    );
    assert_eq!(l.redeem(&k, &c).await, Err(LedgerError::Expired));
    assert_eq!(l.redeem(&approved, &c).await, Err(LedgerError::Expired));
    assert_eq!(store.status(&k).as_deref(), Some("expired"));
    // another turn is untouched
    let other = check(4, "touch y");
    let k4 = open(&l, &other).await;
    assert_eq!(l.expire_turn(&thread(), 1, 3).await, 0);
    assert_eq!(store.status(&k4).as_deref(), Some("pending"));
    // the whole incarnation
    assert_eq!(l.expire_incarnation(&thread(), 1).await, 1);
    assert_eq!(
        l.resolve(&k4, &ApprovalDecision::Allow, None).await,
        Err(LedgerError::Expired)
    );
}

#[tokio::test]
async fn resolve_for_older_incarnation_is_stale() {
    let (store, l) = setup();
    let c = check(1, "touch x");
    let k = open(&l, &c).await;
    store.set_current(&thread(), 2);
    assert_eq!(
        l.resolve(&k, &ApprovalDecision::Allow, None).await,
        Err(LedgerError::StaleIncarnation)
    );
    assert_eq!(
        l.resolve(&k, &ApprovalDecision::Deny { message: None }, None)
            .await,
        Err(LedgerError::StaleIncarnation)
    );
    assert_eq!(l.redeem(&k, &c).await, Err(LedgerError::StaleIncarnation));
    assert_eq!(l.answer(&k).await, Err(LedgerError::StaleIncarnation));
    // opening for the old incarnation is refused too
    assert!(matches!(
        l.open_approval(&thread(), 1, binding(&c, None)).await,
        Err(LedgerError::StaleIncarnation)
    ));
    assert!(matches!(
        l.open_question(&thread(), 1, Some(1)).await,
        Err(LedgerError::StaleIncarnation)
    ));
    // an unknown thread
    let ghost = RequestKey {
        thread: ThreadId("ghost".into()),
        incarnation: 1,
        request_id: "x".into(),
    };
    assert_eq!(
        l.resolve(&ghost, &ApprovalDecision::Allow, None).await,
        Err(LedgerError::Unknown)
    );
    // an unknown request
    let unknown = RequestKey {
        thread: thread(),
        incarnation: 2,
        request_id: "nope".into(),
    };
    assert_eq!(
        l.resolve(&unknown, &ApprovalDecision::Allow, None).await,
        Err(LedgerError::Unknown)
    );
    assert_eq!(l.redeem(&unknown, &c).await, Err(LedgerError::Unknown));
}

#[tokio::test]
async fn resolve_on_older_incarnation_is_stale() {
    // the same contract from the other side: the new incarnation's own
    // requests resolve normally and the old key stays refused
    let (store, l) = setup();
    let c = check(1, "touch x");
    let old = open(&l, &c).await;
    store.set_current(&thread(), 2);
    let t2 = l
        .open_approval(&thread(), 2, binding(&c, None))
        .await
        .unwrap();
    let k2 = RequestKey {
        thread: thread(),
        incarnation: 2,
        request_id: t2.request_id,
    };
    assert_eq!(
        l.resolve(&k2, &ApprovalDecision::Allow, None).await,
        Ok(None)
    );
    assert_eq!(
        l.resolve(&old, &ApprovalDecision::Allow, None).await,
        Err(LedgerError::StaleIncarnation)
    );
}

#[tokio::test]
async fn second_resolve_from_another_window_is_rejected() {
    let (store, l) = setup();
    let c = check(1, "touch x");
    let k = open(&l, &c).await;
    assert_eq!(
        l.resolve(&k, &ApprovalDecision::Allow, None).await,
        Ok(None)
    );
    assert_eq!(
        l.resolve(&k, &ApprovalDecision::Allow, None).await,
        Err(LedgerError::AlreadyResolved)
    );
    assert_eq!(
        l.resolve(&k, &ApprovalDecision::Deny { message: None }, None)
            .await,
        Err(LedgerError::AlreadyResolved)
    );
    assert_eq!(
        store.status(&k).as_deref(),
        Some("approved"),
        "a late deny doesn't undo the allow"
    );
    // and a deny first stays denied
    let k = open(&l, &c).await;
    assert_eq!(
        l.resolve(&k, &ApprovalDecision::Deny { message: None }, None)
            .await,
        Ok(None)
    );
    assert_eq!(
        l.resolve(&k, &ApprovalDecision::Allow, None).await,
        Err(LedgerError::AlreadyResolved)
    );
    assert_eq!(l.redeem(&k, &c).await, Err(LedgerError::AlreadyResolved));
}

#[tokio::test]
async fn deny_or_cancel_never_creates_a_grant() {
    let (store, l) = setup();
    let c = check(1, "touch x");
    // denied
    let k = open(&l, &c).await;
    l.resolve(
        &k,
        &ApprovalDecision::Deny {
            message: Some("no".into()),
        },
        Some(RememberScope::Global),
    )
    .await
    .unwrap();
    assert_eq!(l.redeem(&k, &c).await, Err(LedgerError::AlreadyResolved));
    assert!(
        store.0.lock().unwrap().rules.is_empty(),
        "a denial never creates a rule"
    );
    // withdrawn while pending
    let k = open(&l, &c).await;
    assert_eq!(l.withdraw(&k).await, Ok(()));
    assert_eq!(store.status(&k).as_deref(), Some("cancelled"));
    assert_eq!(l.redeem(&k, &c).await, Err(LedgerError::Expired));
    assert_eq!(
        l.resolve(&k, &ApprovalDecision::Allow, None).await,
        Err(LedgerError::Expired)
    );
    // a question answered can't be redeemed or resolved as an approval
    let q = l.open_question(&thread(), 1, Some(1)).await.unwrap();
    assert!(!q.rememberable);
    let qk = key(&q);
    assert_eq!(l.answer(&qk).await, Ok(()));
    assert_eq!(l.answer(&qk).await, Err(LedgerError::AlreadyResolved));
    assert_eq!(
        l.resolve(&qk, &ApprovalDecision::Allow, None).await,
        Err(LedgerError::AlreadyResolved)
    );
    assert_eq!(l.withdraw(&qk).await, Err(LedgerError::AlreadyResolved));
}

#[tokio::test]
async fn daemon_startup_expires_everything_pending_or_approved() {
    let (store, l) = setup();
    let c = check(1, "touch x");
    let pending = open(&l, &c).await;
    let approved = open_and_allow(&l, &c).await;
    let redeemed = open_and_allow(&l, &c).await;
    l.redeem(&redeemed, &c).await.unwrap();
    let denied = open(&l, &c).await;
    l.resolve(&denied, &ApprovalDecision::Deny { message: None }, None)
        .await
        .unwrap();
    let question = key(&l.open_question(&thread(), 1, None).await.unwrap());
    // another thread too
    let other = ThreadId("t2".into());
    store.set_current(&other, 1);
    let ot = l.open_approval(&other, 1, binding(&c, None)).await.unwrap();
    let ok = RequestKey {
        thread: other,
        incarnation: 1,
        request_id: ot.request_id,
    };
    assert_eq!(l.expire_all().await, 4);
    for k in [&pending, &approved, &question, &ok] {
        assert_eq!(store.status(k).as_deref(), Some("expired"));
    }
    assert_eq!(store.status(&redeemed).as_deref(), Some("redeemed"));
    assert_eq!(store.status(&denied).as_deref(), Some("denied"));
    assert_eq!(l.redeem(&approved, &c).await, Err(LedgerError::Expired));
    assert_eq!(
        l.resolve(&pending, &ApprovalDecision::Allow, None).await,
        Err(LedgerError::Expired)
    );
    assert_eq!(l.expire_all().await, 0);
}

#[tokio::test]
async fn remember_creates_rule_atomically_and_never_revives_an_expired_request() {
    let (store, l) = setup();
    let c = check(1, "touch x");
    let b = binding(&c, Some(prefix(&["touch", "x"])));
    let t = l.open_approval(&thread(), 1, b.clone()).await.unwrap();
    assert!(t.rememberable);
    let k = key(&t);
    let id = l
        .resolve(&k, &ApprovalDecision::Allow, Some(RememberScope::Home))
        .await
        .unwrap()
        .expect("a rule");
    {
        let g = store.0.lock().unwrap();
        assert_eq!(g.rev, 1);
        assert_eq!(g.rules.len(), 1);
        let r = &g.rules[0];
        assert_eq!(r.id, id);
        assert_eq!(r.scope, RuleScope::Home);
        assert_eq!(r.decision, RuleDecision::Allow);
        assert_eq!(r.matcher, prefix(&["touch", "x"]));
        assert_eq!(r.tool_kind, ToolKind::Execute);
        assert_eq!(r.tool_name.as_deref(), Some("Bash"));
        assert_eq!(
            r.origin,
            RuleOrigin::Approval {
                request_id: t.request_id.clone()
            }
        );
    }
    // scopes
    for (scope, want) in [
        (
            RememberScope::Thread,
            RuleScope::Thread {
                thread_id: thread(),
            },
        ),
        (RememberScope::Global, RuleScope::Global),
    ] {
        let t = l.open_approval(&thread(), 1, b.clone()).await.unwrap();
        l.resolve(&key(&t), &ApprovalDecision::Allow, Some(scope))
            .await
            .unwrap();
        assert_eq!(store.0.lock().unwrap().rules.last().unwrap().scope, want);
    }
    assert_eq!(store.0.lock().unwrap().rules.len(), 3);
    // allow without a matcher still approves once and returns no rule
    let t = l
        .open_approval(&thread(), 1, binding(&c, None))
        .await
        .unwrap();
    assert!(!t.rememberable);
    assert_eq!(
        l.resolve(
            &key(&t),
            &ApprovalDecision::Allow,
            Some(RememberScope::Global)
        )
        .await,
        Ok(None)
    );
    assert_eq!(store.status(&key(&t)).as_deref(), Some("approved"));
    assert_eq!(store.0.lock().unwrap().rules.len(), 3);
    // an expired request is never revived and creates no rule
    let t = l.open_approval(&thread(), 1, b).await.unwrap();
    l.expire_turn(&thread(), 1, 1).await;
    assert_eq!(
        l.resolve(
            &key(&t),
            &ApprovalDecision::Allow,
            Some(RememberScope::Global)
        )
        .await,
        Err(LedgerError::Expired)
    );
    assert_eq!(store.status(&key(&t)).as_deref(), Some("expired"));
    let g = store.0.lock().unwrap();
    assert_eq!((g.rules.len(), g.rev), (3, 3));
}

#[tokio::test]
async fn approved_then_withdrawn_is_cancelled_and_never_redeemable() {
    let (store, l) = setup();
    let c = check(1, "touch x");
    let k = open_and_allow(&l, &c).await;
    assert_eq!(l.withdraw(&k).await, Ok(()));
    assert_eq!(store.status(&k).as_deref(), Some("cancelled"));
    assert_eq!(l.redeem(&k, &c).await, Err(LedgerError::Expired));
    assert_eq!(l.withdraw(&k).await, Err(LedgerError::Expired));
    assert_eq!(l.expire_incarnation(&thread(), 1).await, 0);
    assert_eq!(
        l.withdraw(&RequestKey {
            thread: thread(),
            incarnation: 1,
            request_id: "no".into()
        })
        .await,
        Err(LedgerError::Unknown)
    );
}

#[tokio::test]
async fn opening_past_max_pending_is_refused() {
    let (store, l) = setup();
    let c = check(1, "touch x");
    let mut keys = vec![];
    for i in 0..MAX_PENDING_REQUESTS {
        // a mix of pending and approved both count
        let k = open(&l, &c).await;
        if i % 2 == 0 {
            l.resolve(&k, &ApprovalDecision::Allow, None).await.unwrap();
        }
        keys.push(k);
    }
    assert!(matches!(
        l.open_approval(&thread(), 1, binding(&c, None)).await,
        Err(LedgerError::TooMany)
    ));
    assert!(matches!(
        l.open_question(&thread(), 1, None).await,
        Err(LedgerError::TooMany)
    ));
    assert_eq!(
        store.0.lock().unwrap().rows.len(),
        MAX_PENDING_REQUESTS,
        "no hidden rows left behind"
    );
    // redeemed and denied ones don't count
    l.redeem(&keys[0], &c).await.unwrap();
    assert!(
        l.open_approval(&thread(), 1, binding(&c, None))
            .await
            .is_ok()
    );
    assert!(matches!(
        l.open_approval(&thread(), 1, binding(&c, None)).await,
        Err(LedgerError::TooMany)
    ));
    // the limit is per incarnation
    store.set_current(&thread(), 2);
    assert!(
        l.open_approval(&thread(), 2, binding(&c, None))
            .await
            .is_ok()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_opens_cannot_pass_the_limit() {
    let (store, l) = setup();
    let c = check(1, "touch x");
    let mut tasks = vec![];
    for _ in 0..(MAX_PENDING_REQUESTS + 20) {
        let (l, c) = (l.clone(), c.clone());
        tasks.push(tokio::spawn(async move {
            l.open_approval(&thread(), 1, binding(&c, None))
                .await
                .is_ok()
        }));
    }
    let mut ok = 0;
    for t in tasks {
        ok += usize::from(t.await.unwrap());
    }
    assert_eq!(ok, MAX_PENDING_REQUESTS);
    assert_eq!(store.0.lock().unwrap().rows.len(), MAX_PENDING_REQUESTS);
}

#[tokio::test]
async fn redeem_in_a_later_turn_is_refused() {
    let (_s, l) = setup();
    let c = check(2, "touch x");
    let k = open_and_allow(&l, &c).await;
    assert_eq!(
        l.redeem(&k, &check(3, "touch x")).await,
        Err(LedgerError::Mismatch)
    );
    assert_eq!(
        l.redeem(&k, &check(1, "touch x")).await,
        Err(LedgerError::Mismatch)
    );
    assert_eq!(l.redeem(&k, &c).await, Ok(()));
}

fn gate(store: &Arc<MemStore>, ledger: Arc<Ledger>) -> SessionGate {
    SessionGate::new(
        SessionGateConfig {
            thread: thread(),
            incarnation: 1,
            cwd: PathBuf::from("/nonexistent-h/u"),
            home: PathBuf::from("/nonexistent-h/u"),
            atrium_data_paths: vec![],
            atrium_protected_paths: vec![],
            preset: Preset::Home,
            provider: Provider::Claude,
        },
        Arc::new(DefaultPolicy::new()),
        ledger,
        Arc::new(MemRules(store.clone())),
    )
}

fn deny_rule(id: &str, argv: &[&str]) -> Rule {
    Rule {
        id: RuleId(id.into()),
        scope: RuleScope::Global,
        tool_kind: ToolKind::Execute,
        tool_name: None,
        matcher: prefix(argv),
        path_scope: None,
        decision: RuleDecision::Deny,
        origin: RuleOrigin::Settings,
        created_at_ms: 0,
    }
}

#[tokio::test]
async fn rule_change_to_deny_before_redeem_wins() {
    let (store, l) = setup();
    let g = gate(&store, l.clone());
    let c = check(1, "touch x");
    let GateDecision::Ask { ticket } = g.check(&c).await else {
        panic!()
    };
    l.resolve(&key(&ticket), &ApprovalDecision::Allow, None)
        .await
        .unwrap();
    // the user adds a deny rule before the effect
    {
        let mut s = store.0.lock().unwrap();
        s.rules.push(deny_rule("d", &["touch"]));
        s.rev += 1;
    }
    let d = g.redeem(&ticket, &c).await;
    assert!(matches!(d, GateDecision::Deny { .. }), "{d:?}");
    // the approval is spent: it can't be redeemed once the rule is gone
    store.0.lock().unwrap().rules.clear();
    assert!(matches!(
        g.redeem(&ticket, &c).await,
        GateDecision::Deny { .. }
    ));
    assert_eq!(store.status(&key(&ticket)).as_deref(), Some("cancelled"));
}

#[tokio::test]
async fn own_remember_rule_does_not_block_redeem() {
    let (store, l) = setup();
    let g = gate(&store, l.clone());
    let c = check(1, "touch x");
    let GateDecision::Ask { ticket } = g.check(&c).await else {
        panic!()
    };
    assert!(ticket.rememberable);
    // "always": the rules revision moves because of this very approval
    let id = l
        .resolve(
            &key(&ticket),
            &ApprovalDecision::Allow,
            Some(RememberScope::Thread),
        )
        .await
        .unwrap();
    assert!(id.is_some());
    assert_eq!(store.0.lock().unwrap().rev, 1);
    assert_eq!(g.redeem(&ticket, &c).await, GateDecision::Allow);
    // and the rule applies from the next check on
    assert_eq!(g.check(&c).await, GateDecision::Allow);
    // a second ticket for another call isn't affected by the first's rule
    let c2 = check(1, "touch y");
    let GateDecision::Ask { ticket: t2 } = g.check(&c2).await else {
        panic!()
    };
    l.resolve(&key(&t2), &ApprovalDecision::Allow, None)
        .await
        .unwrap();
    assert_eq!(g.redeem(&t2, &c2).await, GateDecision::Allow);
}

#[tokio::test]
async fn approve_with_rule_creates_no_rule_when_not_pending() {
    let (store, l) = setup();
    let c = check(1, "touch x");
    let b = binding(&c, Some(prefix(&["touch", "x"])));
    let t = l.open_approval(&thread(), 1, b).await.unwrap();
    let k = key(&t);
    assert!(
        l.resolve(&k, &ApprovalDecision::Allow, Some(RememberScope::Global))
            .await
            .unwrap()
            .is_some()
    );
    // a second "always" from another window: rejected, no second rule
    assert_eq!(
        l.resolve(&k, &ApprovalDecision::Allow, Some(RememberScope::Global))
            .await,
        Err(LedgerError::AlreadyResolved)
    );
    // after a withdrawal, an old window's "always" creates nothing
    l.withdraw(&k).await.unwrap();
    assert_eq!(
        l.resolve(&k, &ApprovalDecision::Allow, Some(RememberScope::Global))
            .await,
        Err(LedgerError::Expired)
    );
    let g = store.0.lock().unwrap();
    assert_eq!((g.rules.len(), g.rev), (1, 1));
}

// ---------------------------------------------------------------------------
// Concurrency
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_resolve_and_redeem_have_exactly_one_winner() {
    for _ in 0..50 {
        let (store, l) = setup();
        let c = check(1, "touch x");
        let k = open(&l, &c).await;
        // two windows resolve at once
        let mut resolves = vec![];
        for _ in 0..2 {
            let (l, k) = (l.clone(), k.clone());
            resolves.push(tokio::spawn(async move {
                l.resolve(&k, &ApprovalDecision::Allow, Some(RememberScope::Global))
                    .await
            }));
        }
        let mut won = 0;
        for r in resolves {
            match r.await.unwrap() {
                Ok(_) => won += 1,
                Err(e) => assert_eq!(e, LedgerError::AlreadyResolved),
            }
        }
        assert_eq!(won, 1);
        assert!(store.0.lock().unwrap().rules.len() <= 1);
        // many redeemers at once, racing a withdrawal and an expiry
        let mut redeems = vec![];
        for _ in 0..8 {
            let (l, k, c) = (l.clone(), k.clone(), c.clone());
            redeems.push(tokio::spawn(async move { l.redeem(&k, &c).await }));
        }
        let mut ok = 0;
        for r in redeems {
            if r.await.unwrap().is_ok() {
                ok += 1;
            }
        }
        assert_eq!(ok, 1, "exactly one redeem succeeds");
        assert_eq!(store.status(&k).as_deref(), Some("redeemed"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn withdraw_racing_redeem_never_allows_a_cancelled_request() {
    for _ in 0..50 {
        let (store, l) = setup();
        let c = check(1, "touch x");
        let k = open_and_allow(&l, &c).await;
        let (l1, k1, c1) = (l.clone(), k.clone(), c.clone());
        let (l2, k2) = (l.clone(), k.clone());
        let redeem = tokio::spawn(async move { l1.redeem(&k1, &c1).await });
        let withdraw = tokio::spawn(async move { l2.withdraw(&k2).await });
        let (r, w) = (redeem.await.unwrap(), withdraw.await.unwrap());
        // exactly one of them won the compare-and-set
        assert_ne!(r.is_ok(), w.is_ok(), "{r:?} {w:?}");
        let status = store.status(&k).unwrap();
        assert_eq!(status, if r.is_ok() { "redeemed" } else { "cancelled" });
    }
}
