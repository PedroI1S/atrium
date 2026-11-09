//! M1-02: store (contract §7).

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use atrium_core::attention::{AttentionInput, Pending};
use atrium_core::store::{Migration, SqliteStore, StoreOptions};
use atrium_core::{Binding, NewThread, RequestKey, Signal, StorageStatus, Store, StoreError};
use atrium_protocol::agent::*;
use atrium_protocol::limits;
use atrium_protocol::policy::*;
use atrium_protocol::thread::{Cleanup, Location, ThreadId, ThreadState, ThreadSummary};
use atrium_protocol::wire::EventId;
use serde_json::json;
use sha2::{Digest, Sha256};

const MIN_FREE: u64 = 1000;

/// A small stand-in for `attention::next_state` (M1-04, not merged yet).
fn transition(cur: &ThreadState, p: Pending, input: AttentionInput<'_>) -> ThreadState {
    use ThreadState::*;
    let sticky = matches!(cur, Failed { .. } | Ended | Interrupted { .. });
    match input {
        AttentionInput::Signal(s) => match s {
            Signal::Starting => Starting,
            Signal::Stopped => Ended,
            Signal::Exited { code } if !sticky => {
                if *code == Some(0) {
                    Ended
                } else {
                    Interrupted {
                        cleanup: Cleanup::Verified,
                    }
                }
            }
            Signal::Exited { .. } => cur.clone(),
            Signal::Recovered { cleanup } => Interrupted { cleanup: *cleanup },
            Signal::StorageReadOnly => Interrupted {
                cleanup: Cleanup::Verified,
            },
        },
        AttentionInput::Event(e) => {
            if let AgentEvent::Error {
                message,
                fatal: true,
                ..
            } = e
            {
                return Failed {
                    message: message.clone(),
                };
            }
            if sticky {
                return cur.clone();
            }
            if let AgentEvent::SessionEnded { .. } = e {
                return Ended;
            }
            if p.approvals > 0 {
                return NeedsApproval;
            }
            if p.questions > 0 {
                return NeedsInput;
            }
            match e {
                AgentEvent::TurnStarted { .. } => Running,
                AgentEvent::TurnCompleted { .. } | AgentEvent::SessionStarted { .. } => Idle,
                _ if matches!(cur, NeedsApproval | NeedsInput) => Running,
                _ => cur.clone(),
            }
        }
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    free: Arc<AtomicU64>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            free: Arc::new(AtomicU64::new(u64::MAX / 2)),
        }
    }
    fn path(&self) -> std::path::PathBuf {
        self.dir.path().join("atrium.db")
    }
    fn options(&self) -> StoreOptions {
        let free = self.free.clone();
        StoreOptions {
            transition,
            free_space: Arc::new(move || free.load(Ordering::SeqCst)),
            min_free_disk_bytes: MIN_FREE,
        }
    }
    async fn open(&self) -> SqliteStore {
        SqliteStore::open(&self.path(), self.options())
            .await
            .unwrap()
    }
}

fn new_thread() -> NewThread {
    NewThread {
        provider: Provider::Claude,
        model: Some("sonnet".into()),
        effort: None,
        preset: Preset::Home,
        location: Location::Home,
        title: "New thread".into(),
    }
}

fn env(seq: u64, event: AgentEvent) -> AgentEventEnvelope {
    AgentEventEnvelope {
        incarnation: 0,
        seq,
        at_ms: 1_000 + seq,
        event,
    }
}

fn started() -> AgentEvent {
    AgentEvent::SessionStarted {
        provider: Provider::Claude,
        native_session_id: "native-1".into(),
        model: None,
        tools: vec![],
        enforcement: EnforcementDescriptor::evaluate(
            Provider::Claude,
            Some("1.0".into()),
            true,
            vec![],
            vec![],
            vec![],
        ),
    }
}

fn delta(id: &str, kind: MessageKind, text: &str) -> AgentEvent {
    AgentEvent::MessageDelta {
        message_id: id.into(),
        kind,
        text: text.into(),
    }
}

fn completed(id: &str, kind: MessageKind, text: &str, deltas: u64) -> AgentEvent {
    AgentEvent::MessageCompleted {
        message_id: id.into(),
        kind,
        body: MessageBody::Streamed {
            chars: text.chars().count() as u64,
            deltas,
            sha256: sha_hex(text),
        },
    }
}

fn sha_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A created thread with incarnation 1 begun.
async fn started_thread(store: &SqliteStore) -> (ThreadId, u32) {
    let s = store.create_thread(new_thread()).await.unwrap();
    let inc = store.begin_incarnation(&s.thread_id).await.unwrap();
    store.signal(&s.thread_id, Signal::Starting).await.unwrap();
    (s.thread_id, inc)
}

fn key(thread: &ThreadId, inc: u32, id: &str) -> RequestKey {
    RequestKey {
        thread: thread.clone(),
        incarnation: inc,
        request_id: id.into(),
    }
}

fn binding(title: &str) -> Binding {
    Binding {
        turn: 1,
        tool_name: "Bash".into(),
        tool_kind: ToolKind::Execute,
        input_sha256: "ab".repeat(32),
        context: PolicyContext {
            rules_revision: 0,
            preset: Preset::Home,
        },
        title: Some(title.into()),
        remember_matcher: None,
    }
}

fn approval(id: &str, title: &str) -> AgentEvent {
    AgentEvent::ApprovalRequested {
        request_id: id.into(),
        call_id: None,
        name: "Bash".into(),
        kind: ToolKind::Execute,
        title: Some(title.into()),
        input: json!({}),
        reason: None,
        rememberable: false,
    }
}

fn question(id: &str, prompt: &str) -> AgentEvent {
    AgentEvent::QuestionAsked {
        request_id: id.into(),
        questions: vec![Question {
            id: "q1".into(),
            prompt: prompt.into(),
            options: vec![],
            multi_select: false,
            allow_text: true,
        }],
    }
}

fn resolved(id: &str, outcome: RequestOutcome) -> AgentEvent {
    AgentEvent::RequestResolved {
        request_id: id.into(),
        outcome,
    }
}

fn rule(id: &str) -> Rule {
    Rule {
        id: RuleId(id.into()),
        scope: RuleScope::Home,
        tool_kind: ToolKind::Execute,
        tool_name: None,
        matcher: RuleMatcher::Any,
        path_scope: None,
        decision: RuleDecision::Allow,
        origin: RuleOrigin::Settings,
        created_at_ms: 5,
    }
}

async fn summary(store: &SqliteStore, t: &ThreadId) -> ThreadSummary {
    store.thread(t).await.unwrap().unwrap()
}

#[tokio::test]
async fn append_updates_events_and_projections_atomically() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc) = started_thread(&store).await;
    let reqs = store.requests();
    reqs.open_approval(&key(&t, inc, "a1"), &binding("Run ls"))
        .await
        .unwrap();

    let stored = store
        .append(
            &t,
            inc,
            vec![
                env(0, started()),
                env(1, AgentEvent::TurnStarted { turn: 1 }),
                env(2, delta("m1", MessageKind::Assistant, "héllo")),
                env(3, delta("m1", MessageKind::Assistant, " wörld")),
                env(
                    4,
                    AgentEvent::ToolCallStarted {
                        call_id: "c1".into(),
                        name: "Bash".into(),
                        kind: ToolKind::Execute,
                        title: None,
                        input: json!({}),
                    },
                ),
                env(5, approval("a1", "Run ls")),
            ],
        )
        .await
        .unwrap();
    assert_eq!(stored.len(), 6);
    assert!(stored.windows(2).all(|w| w[0].id.0 < w[1].id.0));
    assert!(stored.iter().all(|e| e.envelope.incarnation == inc));

    let s = summary(&store, &t).await;
    assert_eq!(s.state, ThreadState::NeedsApproval);
    assert_eq!(s.pending_approvals, 1);
    assert_eq!(s.last_event, Some(stored[5].id));
    assert_eq!(s.native_session_id.as_deref(), Some("native-1"));
    assert!(s.enforcement.is_some());
    assert!(s.unread);

    let open = store.open_items(&t, inc).await.unwrap();
    assert_eq!(open.turn, Some(1));
    assert_eq!(open.tool_calls, vec!["c1"]);
    assert_eq!(open.requests, vec!["a1"]);
    assert_eq!(open.messages.len(), 1);
    assert_eq!(open.messages[0].chars, 11);
    assert_eq!(open.messages[0].deltas, 2);
    assert_eq!(open.messages[0].sha256, sha_hex("héllo wörld"));

    // Closing it all clears the projections.
    store
        .append(
            &t,
            inc,
            vec![
                env(6, completed("m1", MessageKind::Assistant, "héllo wörld", 2)),
                env(
                    7,
                    AgentEvent::ToolCallFinished {
                        call_id: "c1".into(),
                        status: ToolStatus::Cancelled,
                        output: None,
                        error: None,
                        truncated_bytes: 0,
                    },
                ),
                env(8, resolved("a1", RequestOutcome::Cancelled)),
                env(
                    9,
                    AgentEvent::TurnCompleted {
                        turn: 1,
                        outcome: TurnOutcome::Interrupted,
                        usage: None,
                    },
                ),
            ],
        )
        .await
        .unwrap();
    assert_eq!(store.integrity_errors(), 0);
    assert_eq!(store.open_items(&t, inc).await.unwrap(), Default::default());
    assert_eq!(summary(&store, &t).await.pending_approvals, 0);
}

#[tokio::test]
async fn failed_batch_stores_nothing() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc) = started_thread(&store).await;
    // Duplicate seq in one batch: the second insert fails, the first is rolled back.
    let err = store
        .append(
            &t,
            inc,
            vec![
                env(0, delta("m1", MessageKind::Assistant, "a")),
                env(0, delta("m1", MessageKind::Assistant, "b")),
            ],
        )
        .await;
    assert!(matches!(err, Err(StoreError::Sqlite(_))));
    assert!(store.events_after(&t, None, 10).await.unwrap().is_empty());
    assert_eq!(store.open_items(&t, inc).await.unwrap(), Default::default());
    assert!(store.last_event(&t).await.unwrap().is_none());
    // The in-memory hash was reset: a later message is still verified correctly.
    store
        .append(
            &t,
            inc,
            vec![env(0, delta("m1", MessageKind::Assistant, "x"))],
        )
        .await
        .unwrap();
    store
        .append(
            &t,
            inc,
            vec![env(1, completed("m1", MessageKind::Assistant, "x", 1))],
        )
        .await
        .unwrap();
    assert_eq!(store.integrity_errors(), 0);
}

#[tokio::test]
async fn closing_uses_projections_not_history() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc) = started_thread(&store).await;
    let mut batch = vec![
        env(0, started()),
        env(1, AgentEvent::TurnStarted { turn: 1 }),
    ];
    // Lots of finished history, plus one message and one call left open.
    for i in 0..500u64 {
        let id = format!("done{i}");
        batch.push(env(2 + i * 2, delta(&id, MessageKind::Assistant, "x")));
        batch.push(env(
            3 + i * 2,
            completed(&id, MessageKind::Assistant, "x", 1),
        ));
    }
    batch.push(env(1002, delta("open", MessageKind::Reasoning, "abc")));
    store.append(&t, inc, batch).await.unwrap();
    let open = store.open_items(&t, inc).await.unwrap();
    assert_eq!(open.messages.len(), 1);
    assert_eq!(open.messages[0].message_id, "open");
    assert_eq!(open.messages[0].kind, MessageKind::Reasoning);
    assert_eq!(open.messages[0].sha256, sha_hex("abc"));
    assert_eq!(open.turn, Some(1));

    // Survives a restart: the hash is recomputed from the stored deltas, and
    // appending to the message afterwards keeps it right.
    drop(store);
    let store = fx.open().await;
    let open = store.open_items(&t, inc).await.unwrap();
    assert_eq!(open.messages[0].sha256, sha_hex("abc"));
    assert_eq!(open.messages[0].chars, 3);
    drop(store);
    let store = fx.open().await;
    store
        .append(
            &t,
            inc,
            vec![env(1003, delta("open", MessageKind::Reasoning, "def"))],
        )
        .await
        .unwrap();
    store
        .append(
            &t,
            inc,
            vec![env(
                1004,
                completed("open", MessageKind::Reasoning, "abcdef", 2),
            )],
        )
        .await
        .unwrap();
    assert_eq!(store.integrity_errors(), 0);
    assert!(store.open_items(&t, inc).await.unwrap().messages.is_empty());
}

#[tokio::test]
async fn integrity_mismatch_is_stored_but_counted() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc) = started_thread(&store).await;
    store
        .append(
            &t,
            inc,
            vec![env(0, delta("m", MessageKind::Assistant, "abc"))],
        )
        .await
        .unwrap();
    let stored = store
        .append(
            &t,
            inc,
            vec![env(1, completed("m", MessageKind::Assistant, "abd", 1))],
        )
        .await
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(store.integrity_errors(), 1);
}

#[tokio::test]
async fn replay_pages_have_no_gaps_or_duplicates() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc) = started_thread(&store).await;
    let (other, oinc) = started_thread(&store).await;
    for chunk in 0..5u64 {
        let batch: Vec<_> = (0..20u64)
            .map(|i| env(chunk * 20 + i, delta("m", MessageKind::Assistant, "x")))
            .collect();
        store.append(&t, inc, batch).await.unwrap();
        // Interleaved events of another thread leave holes in the global ids.
        store
            .append(
                &other,
                oinc,
                vec![env(chunk, delta("o", MessageKind::Assistant, "y"))],
            )
            .await
            .unwrap();
    }
    let mut after: Option<EventId> = None;
    let mut seqs = Vec::new();
    loop {
        let page = store.events_after(&t, after, 7).await.unwrap();
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= 7);
        after = Some(page.last().unwrap().id);
        seqs.extend(page.iter().map(|e| e.envelope.seq));
    }
    assert_eq!(seqs, (0..100).collect::<Vec<_>>());
    assert_eq!(store.last_event(&t).await.unwrap(), after);
    // Resuming from a cursor mid-way.
    let mid = store
        .events_after(&t, None, 50)
        .await
        .unwrap()
        .last()
        .unwrap()
        .id;
    let rest = store.events_after(&t, Some(mid), 1000).await.unwrap();
    assert_eq!(rest.len(), 50);
    assert_eq!(rest[0].envelope.seq, 50);
}

#[tokio::test]
async fn low_disk_makes_store_read_only_and_recovers() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc) = started_thread(&store).await;
    assert!(matches!(store.storage_status(), StorageStatus::Ok { .. }));

    fx.free.store(MIN_FREE - 1, Ordering::SeqCst);
    assert_eq!(
        store.storage_status(),
        StorageStatus::ReadOnly {
            free_bytes: MIN_FREE - 1
        }
    );
    let e = store
        .append(
            &t,
            inc,
            vec![env(0, delta("m", MessageKind::Assistant, "x"))],
        )
        .await;
    assert!(matches!(e, Err(StoreError::ReadOnly { free_bytes }) if free_bytes == MIN_FREE - 1));
    assert!(matches!(
        store.create_thread(new_thread()).await,
        Err(StoreError::ReadOnly { .. })
    ));
    assert!(matches!(
        store.begin_incarnation(&t).await,
        Err(StoreError::ReadOnly { .. })
    ));
    assert!(matches!(
        store.rename(&t, "x").await,
        Err(StoreError::ReadOnly { .. })
    ));
    assert!(matches!(
        store.set_setting("k", json!(1)).await,
        Err(StoreError::ReadOnly { .. })
    ));
    assert!(matches!(
        store.create_rule(rule("r")).await,
        Err(StoreError::ReadOnly { .. })
    ));
    assert!(matches!(
        store
            .requests()
            .open_question(&key(&t, inc, "q"), None)
            .await,
        Err(StoreError::ReadOnly { .. })
    ));
    assert!(store.events_after(&t, None, 10).await.unwrap().is_empty());

    // Recovers by itself.
    fx.free.store(MIN_FREE, Ordering::SeqCst);
    assert!(matches!(store.storage_status(), StorageStatus::Ok { .. }));
    store
        .append(
            &t,
            inc,
            vec![env(0, delta("m", MessageKind::Assistant, "x"))],
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn read_only_store_still_accepts_closing_batches_and_signals() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc) = started_thread(&store).await;
    let reqs = store.requests();
    reqs.open_approval(&key(&t, inc, "a1"), &binding("x"))
        .await
        .unwrap();
    reqs.open_approval(&key(&t, inc, "a2"), &binding("y"))
        .await
        .unwrap();
    store
        .append(
            &t,
            inc,
            vec![
                env(0, AgentEvent::TurnStarted { turn: 1 }),
                env(1, delta("m", MessageKind::Assistant, "ab")),
                env(2, approval("a1", "x")),
            ],
        )
        .await
        .unwrap();
    store
        .set_process(
            &t,
            inc,
            Some(atrium_host::supervise::ProcessIdentity {
                pid: 1,
                pgid: 1,
                start_time: 2,
                executable: "/bin/x".into(),
            }),
        )
        .await
        .unwrap();

    fx.free.store(0, Ordering::SeqCst);
    // A mixed batch (closing + ordinary) is refused whole.
    let mixed = store
        .append(
            &t,
            inc,
            vec![
                env(3, completed("m", MessageKind::Assistant, "ab", 1)),
                env(4, delta("n", MessageKind::Assistant, "z")),
            ],
        )
        .await;
    assert!(matches!(mixed, Err(StoreError::ReadOnly { .. })));
    // A closing-only batch works.
    store
        .append(
            &t,
            inc,
            vec![
                env(3, completed("m", MessageKind::Assistant, "ab", 1)),
                env(4, resolved("a1", RequestOutcome::Cancelled)),
                env(
                    5,
                    AgentEvent::TurnCompleted {
                        turn: 1,
                        outcome: TurnOutcome::Interrupted,
                        usage: None,
                    },
                ),
                env(
                    6,
                    AgentEvent::Error {
                        message: "disk".into(),
                        fatal: true,
                        details: vec![],
                    },
                ),
                env(
                    6 + 1,
                    AgentEvent::SessionEnded {
                        exit_code: Some(1),
                        reason: None,
                    },
                ),
            ],
        )
        .await
        .unwrap();
    assert_eq!(store.open_items(&t, inc).await.unwrap(), Default::default());
    // Signals, clearing a process and request cleanup work.
    store.signal(&t, Signal::StorageReadOnly).await.unwrap();
    store
        .signal(&t, Signal::Exited { code: Some(1) })
        .await
        .unwrap();
    store.set_process(&t, inc, None).await.unwrap();
    assert!(store.live_processes().await.unwrap().is_empty());
    assert_eq!(
        reqs.transition(&key(&t, inc, "a2"), &["pending"], "denied")
            .await
            .unwrap()
            .as_deref(),
        Some("pending")
    );
    reqs.open_approval(&key(&t, inc, "a3"), &binding("z"))
        .await
        .unwrap_err();
    assert_eq!(reqs.expire(None, None, None).await.unwrap(), 1);
}

#[tokio::test]
async fn failed_migration_rolls_back_and_refuses_to_open() {
    // A file that already has a conflicting `threads` table: migration 1 fails
    // after creating `schema_migrations`.
    let fx = Fixture::new();
    {
        let c = rusqlite::Connection::open(fx.path()).unwrap();
        c.execute_batch("CREATE TABLE threads (x); INSERT INTO threads VALUES (1);")
            .unwrap();
    }
    let before = std::fs::read(fx.path()).unwrap();
    let err = SqliteStore::open(&fx.path(), fx.options())
        .await
        .unwrap_err();
    assert!(
        matches!(err, StoreError::Migration { version: 1, .. }),
        "{err:?}"
    );
    assert_eq!(std::fs::read(fx.path()).unwrap(), before);

    // A later migration failing halfway leaves a good store exactly as it was.
    let fx = Fixture::new();
    drop(fx.open().await);
    let before = std::fs::read(fx.path()).unwrap();
    static BAD: &[Migration] = &[
        Migration {
            version: 1,
            sql: include_str!("../schema/0001_init.sql"),
        },
        Migration {
            version: 2,
            sql: "CREATE TABLE extra (a); INSERT INTO extra VALUES (1); INSERT INTO nope VALUES (1);",
        },
    ];
    let err = SqliteStore::open_with(&fx.path(), fx.options(), BAD)
        .await
        .unwrap_err();
    assert!(
        matches!(err, StoreError::Migration { version: 2, .. }),
        "{err:?}"
    );
    assert_eq!(std::fs::read(fx.path()).unwrap(), before);
    // ... and it still opens with the right migrations.
    let store = fx.open().await;
    store.create_thread(new_thread()).await.unwrap();

    // A database from a newer build is refused too.
    drop(store);
    static NEWER: &[Migration] = &[Migration {
        version: 0,
        sql: "SELECT 1;",
    }];
    let err = SqliteStore::open_with(&fx.path(), fx.options(), NEWER)
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::Migration { .. }));
}

#[tokio::test]
async fn snapshot_is_private_and_pruned() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc) = started_thread(&store).await;
    store
        .append(
            &t,
            inc,
            vec![env(0, delta("m", MessageKind::Assistant, "x"))],
        )
        .await
        .unwrap();
    let dir = fx.dir.path().join("snapshots");
    let mut last = None;
    for _ in 0..limits::SNAPSHOTS_KEPT + 3 {
        let p = store.snapshot(&dir).await.unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        last = Some(p);
    }
    let files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), limits::SNAPSHOTS_KEPT);
    assert!(files.contains(last.as_ref().unwrap()));
    for f in &files {
        assert_eq!(
            std::fs::metadata(f).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    // The newest snapshot is a valid copy of the data.
    let c = rusqlite::Connection::open(last.unwrap()).unwrap();
    let n: i64 = c
        .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
    // The database file itself is private too.
    assert_eq!(
        std::fs::metadata(fx.path()).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[tokio::test]
async fn incarnations_scope_sequences_and_requests() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc1) = started_thread(&store).await;
    assert_eq!(inc1, 1);
    let reqs = store.requests();
    assert_eq!(reqs.current_incarnation(&t).await.unwrap(), Some(1));
    assert_eq!(
        reqs.current_incarnation(&ThreadId("nope".into()))
            .await
            .unwrap(),
        None
    );
    reqs.open_approval(&key(&t, 1, "r1"), &binding("a"))
        .await
        .unwrap();
    store
        .append(
            &t,
            1,
            vec![
                env(0, started()),
                env(1, AgentEvent::TurnStarted { turn: 1 }),
                env(2, approval("r1", "a")),
            ],
        )
        .await
        .unwrap();
    store
        .append(
            &t,
            1,
            vec![env(
                3,
                AgentEvent::SessionEnded {
                    exit_code: Some(0),
                    reason: None,
                },
            )],
        )
        .await
        .unwrap();

    let inc2 = store.begin_incarnation(&t).await.unwrap();
    assert_eq!(inc2, 2);
    assert_eq!(reqs.current_incarnation(&t).await.unwrap(), Some(2));
    // Incarnation 1 never closed r1, but the new incarnation starts clean.
    let s = summary(&store, &t).await;
    assert_eq!((s.pending_approvals, s.pending_questions), (0, 0));
    assert_eq!((s.attention_since_ms, s.attention_detail), (None, None));
    assert_eq!(
        store.signal(&t, Signal::Starting).await.unwrap().state,
        ThreadState::Starting
    );
    // seq, turn numbers and request ids restart.
    reqs.open_approval(&key(&t, 2, "r1"), &binding("b"))
        .await
        .unwrap();
    let e = store
        .append(
            &t,
            2,
            vec![
                env(
                    0,
                    AgentEvent::SessionStarted {
                        provider: Provider::Claude,
                        native_session_id: "native-2".into(),
                        model: None,
                        tools: vec![],
                        enforcement: EnforcementDescriptor::evaluate(
                            Provider::Claude,
                            None,
                            false,
                            vec![],
                            vec![],
                            vec![],
                        ),
                    },
                ),
                env(1, AgentEvent::TurnStarted { turn: 1 }),
                env(2, approval("r1", "b")),
            ],
        )
        .await
        .unwrap();
    assert!(e.iter().all(|e| e.envelope.incarnation == 2));
    // Each incarnation has its own open items.
    assert_eq!(store.open_items(&t, 1).await.unwrap().requests, vec!["r1"]);
    assert_eq!(store.open_items(&t, 2).await.unwrap().requests, vec!["r1"]);
    assert_eq!(store.open_items(&t, 2).await.unwrap().turn, Some(1));
    assert_eq!(reqs.open_count(&t, 1).await.unwrap(), 1);
    assert_eq!(reqs.open_count(&t, 2).await.unwrap(), 1);
    // Closing in incarnation 2 leaves incarnation 1's row alone.
    store
        .append(
            &t,
            2,
            vec![env(3, resolved("r1", RequestOutcome::Cancelled))],
        )
        .await
        .unwrap();
    assert!(store.open_items(&t, 2).await.unwrap().requests.is_empty());
    assert_eq!(store.open_items(&t, 1).await.unwrap().requests, vec!["r1"]);
    let s = summary(&store, &t).await;
    assert_eq!(s.pending_approvals, 0);
    assert_eq!(s.state, ThreadState::Running);
    assert_eq!((s.attention_since_ms, s.attention_detail), (None, None));
    // The summary and resume id follow the latest incarnation.
    let s = summary(&store, &t).await;
    assert_eq!(
        (s.incarnation, s.native_session_id.as_deref()),
        (2, Some("native-2"))
    );
    assert_eq!(
        store.resume_id(&t).await.unwrap().as_deref(),
        Some("native-2")
    );
    // Expiring is per incarnation.
    assert_eq!(reqs.expire(Some(&t), Some(1), None).await.unwrap(), 1);
    assert_eq!(reqs.open_count(&t, 1).await.unwrap(), 0);
    assert_eq!(reqs.open_count(&t, 2).await.unwrap(), 1);
    // The whole history replays in order.
    let all = store.events_after(&t, None, 100).await.unwrap();
    assert_eq!(
        all.iter()
            .map(|e| (e.envelope.incarnation, e.envelope.seq))
            .collect::<Vec<_>>(),
        vec![
            (1, 0),
            (1, 1),
            (1, 2),
            (1, 3),
            (2, 0),
            (2, 1),
            (2, 2),
            (2, 3)
        ]
    );
}

#[tokio::test]
async fn resume_id_survives_an_incarnation_without_one() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc) = started_thread(&store).await;
    assert_eq!(store.resume_id(&t).await.unwrap(), None);
    store
        .append(&t, inc, vec![env(0, started())])
        .await
        .unwrap();
    store.begin_incarnation(&t).await.unwrap();
    assert_eq!(
        store.resume_id(&t).await.unwrap().as_deref(),
        Some("native-1")
    );
}

#[tokio::test]
async fn stale_incarnation_batch_is_rejected() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc) = started_thread(&store).await;
    store.begin_incarnation(&t).await.unwrap();
    let e = store
        .append(
            &t,
            inc,
            vec![env(0, delta("m", MessageKind::Assistant, "x"))],
        )
        .await;
    assert!(matches!(
        e,
        Err(StoreError::StaleIncarnation { got: 1, current: 2 })
    ));
    let e = store
        .append(&t, 3, vec![env(0, delta("m", MessageKind::Assistant, "x"))])
        .await;
    assert!(matches!(
        e,
        Err(StoreError::StaleIncarnation { got: 3, current: 2 })
    ));
    assert!(store.events_after(&t, None, 10).await.unwrap().is_empty());
    assert!(!summary(&store, &t).await.unread);
    let e = store
        .append(&ThreadId("nope".into()), 1, vec![env(0, started())])
        .await;
    assert!(matches!(e, Err(StoreError::UnknownThread(_))));
}

#[tokio::test]
async fn oversized_event_is_rejected_and_nothing_is_stored() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc) = started_thread(&store).await;
    let big = "x".repeat(limits::MAX_EVENT_BYTES + 1);
    let e = store
        .append(
            &t,
            inc,
            vec![
                env(0, delta("m", MessageKind::Assistant, "ok")),
                env(1, delta("m", MessageKind::Assistant, &big)),
            ],
        )
        .await;
    assert!(
        matches!(e, Err(StoreError::EventTooLarge { bytes }) if bytes > limits::MAX_EVENT_BYTES)
    );
    assert!(store.events_after(&t, None, 10).await.unwrap().is_empty());
    // Just under the limit is fine.
    let ok = "x".repeat(limits::MAX_EVENT_BYTES - 1024);
    store
        .append(
            &t,
            inc,
            vec![env(0, delta("m", MessageKind::Assistant, &ok))],
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn store_never_changes_request_status() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc) = started_thread(&store).await;
    let reqs = store.requests();
    let k = key(&t, inc, "a1");
    reqs.open_approval(&k, &binding("x")).await.unwrap();
    // A binding without a title round-trips even after the event is stored.
    let mut untitled = binding("x");
    untitled.title = None;
    let k0 = key(&t, inc, "a0");
    reqs.open_approval(&k0, &untitled).await.unwrap();
    store
        .append(&t, inc, vec![env(100, approval("a0", "from the event"))])
        .await
        .unwrap();
    assert_eq!(reqs.binding(&k0).await.unwrap().unwrap().1, untitled);
    assert_eq!(
        summary(&store, &t).await.attention_detail.as_deref(),
        Some("Bash")
    );
    store
        .append(
            &t,
            inc,
            vec![env(101, resolved("a0", RequestOutcome::Denied))],
        )
        .await
        .unwrap();
    reqs.transition(&k0, &["pending"], "denied").await.unwrap();
    assert_eq!(reqs.open_count(&t, inc).await.unwrap(), 1);
    store
        .append(&t, inc, vec![env(0, approval("a1", "x"))])
        .await
        .unwrap();
    store
        .append(
            &t,
            inc,
            vec![env(1, resolved("a1", RequestOutcome::Approved))],
        )
        .await
        .unwrap();
    // The event closed it for crash recovery, but the ledger's status is untouched.
    assert!(store.open_items(&t, inc).await.unwrap().requests.is_empty());
    let (status, b) = reqs.binding(&k).await.unwrap().unwrap();
    assert_eq!(status, "pending");
    assert_eq!(b, binding("x"));
    assert_eq!(reqs.open_count(&t, inc).await.unwrap(), 1);

    // Compare-and-set semantics.
    assert_eq!(
        reqs.transition(&k, &["approved"], "redeemed")
            .await
            .unwrap()
            .as_deref(),
        Some("pending")
    );
    assert_eq!(reqs.binding(&k).await.unwrap().unwrap().0, "pending");
    assert_eq!(
        reqs.transition(&k, &["pending", "approved"], "approved")
            .await
            .unwrap()
            .as_deref(),
        Some("pending")
    );
    assert_eq!(reqs.binding(&k).await.unwrap().unwrap().0, "approved");
    assert_eq!(
        reqs.transition(&k, &["approved"], "redeemed")
            .await
            .unwrap()
            .as_deref(),
        Some("approved")
    );
    assert_eq!(
        reqs.transition(&k, &["approved"], "redeemed")
            .await
            .unwrap()
            .as_deref(),
        Some("redeemed")
    );
    assert_eq!(reqs.open_count(&t, inc).await.unwrap(), 0);
    assert_eq!(
        reqs.transition(&key(&t, inc, "zzz"), &["pending"], "denied")
            .await
            .unwrap(),
        None
    );
    // `expire` never touches a redeemed row.
    assert_eq!(reqs.expire(None, None, None).await.unwrap(), 0);
    assert_eq!(reqs.binding(&k).await.unwrap().unwrap().0, "redeemed");
}

#[tokio::test]
async fn approve_with_rule_is_one_compare_and_set() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc) = started_thread(&store).await;
    let reqs = store.requests();
    let k = key(&t, inc, "a1");
    reqs.open_approval(&k, &binding("x")).await.unwrap();
    assert_eq!(store.rules().await.unwrap().0, 0);
    assert_eq!(
        reqs.approve_with_rule(&k, Some(rule("r1")))
            .await
            .unwrap()
            .as_deref(),
        Some("pending")
    );
    let (rev, rules) = store.rules().await.unwrap();
    assert_eq!((rev, rules.len()), (1, 1));
    // Second time: no longer pending, so no rule and no revision bump.
    assert_eq!(
        reqs.approve_with_rule(&k, Some(rule("r2")))
            .await
            .unwrap()
            .as_deref(),
        Some("approved")
    );
    assert_eq!(store.rules().await.unwrap().0, 1);
    // Without a rule: approves without touching rules.
    let k2 = key(&t, inc, "a2");
    reqs.open_approval(&k2, &binding("y")).await.unwrap();
    assert_eq!(
        reqs.approve_with_rule(&k2, None).await.unwrap().as_deref(),
        Some("pending")
    );
    assert_eq!(store.rules().await.unwrap().0, 1);
    // An expired request doesn't create its rule.
    let k3 = key(&t, inc, "a3");
    reqs.open_approval(&k3, &binding("z")).await.unwrap();
    reqs.expire(Some(&t), Some(inc), None).await.unwrap();
    assert_eq!(
        reqs.approve_with_rule(&k3, Some(rule("r3")))
            .await
            .unwrap()
            .as_deref(),
        Some("expired")
    );
    assert_eq!(store.rules().await.unwrap().1.len(), 1);
    // Rules CRUD bumps the revision.
    store.create_rule(rule("r9")).await.unwrap();
    assert_eq!(store.rules().await.unwrap().0, 2);
    store.delete_rule(&RuleId("r9".into())).await.unwrap();
    store.delete_rule(&RuleId("r9".into())).await.unwrap();
    assert_eq!(store.rules().await.unwrap().0, 3);
}

#[tokio::test]
async fn expire_filters_by_thread_incarnation_and_turn() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc) = started_thread(&store).await;
    let (t2, inc2) = started_thread(&store).await;
    let reqs = store.requests();
    let mut b2 = binding("x");
    b2.turn = 2;
    reqs.open_approval(&key(&t, inc, "a"), &binding("x"))
        .await
        .unwrap();
    reqs.open_approval(&key(&t, inc, "b"), &b2).await.unwrap();
    reqs.open_question(&key(&t, inc, "q"), None).await.unwrap();
    reqs.open_approval(&key(&t2, inc2, "a"), &binding("x"))
        .await
        .unwrap();
    assert_eq!(reqs.expire(Some(&t), Some(inc), Some(2)).await.unwrap(), 1);
    assert_eq!(reqs.expire(Some(&t), Some(inc), None).await.unwrap(), 2);
    assert_eq!(reqs.open_count(&t2, inc2).await.unwrap(), 1);
    assert_eq!(reqs.expire(None, None, None).await.unwrap(), 1);
}

#[tokio::test]
async fn attention_fields_follow_events() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc) = started_thread(&store).await;
    let reqs = store.requests();
    assert_eq!(summary(&store, &t).await.attention_since_ms, None);

    reqs.open_approval(&key(&t, inc, "a1"), &binding("Delete build/"))
        .await
        .unwrap();
    reqs.open_question(&key(&t, inc, "q1"), Some(1))
        .await
        .unwrap();
    store
        .append(
            &t,
            inc,
            vec![
                env(0, AgentEvent::TurnStarted { turn: 1 }),
                env(1, question("q1", "Which branch?")),
            ],
        )
        .await
        .unwrap();
    let s = summary(&store, &t).await;
    assert_eq!(s.state, ThreadState::NeedsInput);
    assert_eq!((s.pending_approvals, s.pending_questions), (0, 1));
    assert_eq!(s.attention_detail.as_deref(), Some("Which branch?"));
    assert!(s.attention_since_ms.is_some());

    // An approval outranks the question.
    store
        .append(&t, inc, vec![env(2, approval("a1", "Delete build/"))])
        .await
        .unwrap();
    let s = summary(&store, &t).await;
    assert_eq!(s.state, ThreadState::NeedsApproval);
    assert_eq!(s.attention_detail.as_deref(), Some("Delete build/"));

    store
        .append(
            &t,
            inc,
            vec![env(3, resolved("a1", RequestOutcome::Approved))],
        )
        .await
        .unwrap();
    let s = summary(&store, &t).await;
    assert_eq!(s.attention_detail.as_deref(), Some("Which branch?"));
    store
        .append(
            &t,
            inc,
            vec![env(4, resolved("q1", RequestOutcome::Answered))],
        )
        .await
        .unwrap();
    let s = summary(&store, &t).await;
    assert_eq!((s.pending_approvals, s.pending_questions), (0, 0));
    assert_eq!((s.attention_since_ms, s.attention_detail), (None, None));
    assert_eq!(s.state, ThreadState::Running);

    // A fatal error: its message, then cleared by the next incarnation.
    store
        .append(
            &t,
            inc,
            vec![env(
                5,
                AgentEvent::Error {
                    message: "model not found".into(),
                    fatal: true,
                    details: vec![],
                },
            )],
        )
        .await
        .unwrap();
    let s = summary(&store, &t).await;
    assert!(matches!(s.state, ThreadState::Failed { .. }));
    assert_eq!(s.attention_detail.as_deref(), Some("model not found"));
    let since = s.attention_since_ms;
    assert!(since.is_some());
    store
        .append(
            &t,
            inc,
            vec![env(
                6,
                AgentEvent::SessionEnded {
                    exit_code: Some(1),
                    reason: None,
                },
            )],
        )
        .await
        .unwrap();
    let s = summary(&store, &t).await;
    assert_eq!(s.attention_since_ms, since);
    assert_eq!(s.attention_detail.as_deref(), Some("model not found"));
    store.begin_incarnation(&t).await.unwrap();
    let s = store.signal(&t, Signal::Starting).await.unwrap();
    assert_eq!(s.state, ThreadState::Starting);
    assert_eq!((s.attention_since_ms, s.attention_detail), (None, None));

    // Unverified cleanup needs the user too.
    let s = store
        .signal(
            &t,
            Signal::Recovered {
                cleanup: Cleanup::Unverified,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        s.state,
        ThreadState::Interrupted {
            cleanup: Cleanup::Unverified
        }
    );
    assert!(s.attention_since_ms.is_some());
}

#[tokio::test]
async fn request_row_without_opening_event_is_not_open() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc) = started_thread(&store).await;
    let reqs = store.requests();
    reqs.open_approval(&key(&t, inc, "ghost"), &binding("x"))
        .await
        .unwrap();
    reqs.open_approval(&key(&t, inc, "real"), &binding("y"))
        .await
        .unwrap();
    store
        .append(&t, inc, vec![env(0, approval("real", "y"))])
        .await
        .unwrap();
    assert_eq!(
        store.open_items(&t, inc).await.unwrap().requests,
        vec!["real"]
    );
    // A question's title becomes its first prompt; the ghost is still expired.
    assert_eq!(reqs.expire(None, None, None).await.unwrap(), 2);
    assert_eq!(summary(&store, &t).await.pending_approvals, 1);
    // A closing event for a request that was never opened changes nothing.
    store
        .append(
            &t,
            inc,
            vec![env(1, resolved("ghost", RequestOutcome::Cancelled))],
        )
        .await
        .unwrap();
    assert_eq!(summary(&store, &t).await.pending_approvals, 1);
}

#[tokio::test]
async fn rename_updates_title_and_summary() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, _) = started_thread(&store).await;
    assert_eq!(summary(&store, &t).await.title, "New thread");
    let s = store.rename(&t, "Fix the build").await.unwrap();
    assert_eq!(s.title, "Fix the build");
    assert_eq!(summary(&store, &t).await.title, "Fix the build");
    assert_eq!(store.threads().await.unwrap()[0].title, "Fix the build");
    assert!(matches!(
        store.rename(&ThreadId("nope".into()), "x").await,
        Err(StoreError::UnknownThread(_))
    ));
}

#[tokio::test]
async fn user_messages_never_set_unread() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc) = started_thread(&store).await;
    store
        .append(
            &t,
            inc,
            vec![
                env(0, AgentEvent::TurnStarted { turn: 1 }),
                env(1, delta("u", MessageKind::User, "hi")),
                env(2, completed("u", MessageKind::User, "hi", 1)),
                env(
                    3,
                    AgentEvent::MessageCompleted {
                        message_id: "u2".into(),
                        kind: MessageKind::User,
                        body: MessageBody::Inline {
                            text: "inline".into(),
                        },
                    },
                ),
            ],
        )
        .await
        .unwrap();
    assert_eq!(store.integrity_errors(), 0);
    let s = summary(&store, &t).await;
    assert!(!s.unread);
    assert_eq!(s.attention_since_ms, None);
    // An assistant reply does, and mark_read clears it.
    store
        .append(
            &t,
            inc,
            vec![env(4, delta("a", MessageKind::Assistant, "yo"))],
        )
        .await
        .unwrap();
    let s = summary(&store, &t).await;
    assert!(s.unread);
    assert!(s.attention_since_ms.is_some());
    store.mark_read(&t).await.unwrap();
    let s = summary(&store, &t).await;
    assert!(!s.unread);
    assert_eq!(s.attention_since_ms, None);
    // Reasoning counts as output.
    store
        .append(
            &t,
            inc,
            vec![env(5, delta("r", MessageKind::Reasoning, "hm"))],
        )
        .await
        .unwrap();
    assert!(summary(&store, &t).await.unread);
}

#[tokio::test]
async fn threads_settings_and_live_processes() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (a, ai) = started_thread(&store).await;
    let (b, _) = started_thread(&store).await;
    let list = store.threads().await.unwrap();
    assert_eq!(
        list.iter().map(|s| s.thread_id.clone()).collect::<Vec<_>>(),
        vec![b.clone(), a.clone()]
    );
    store.archive(&a).await.unwrap();
    assert_eq!(store.threads().await.unwrap().len(), 1);
    assert!(store.thread(&a).await.unwrap().is_some());

    store.set_setting("theme", json!("dark")).await.unwrap();
    store.set_setting("theme", json!({"x": 1})).await.unwrap();
    store.set_setting("a", json!(true)).await.unwrap();
    assert_eq!(
        store.settings().await.unwrap(),
        vec![("a".into(), json!(true)), ("theme".into(), json!({"x": 1}))]
    );

    let id = atrium_host::supervise::ProcessIdentity {
        pid: 7,
        pgid: 7,
        start_time: 9,
        executable: "/bin/claude".into(),
    };
    store.set_process(&a, ai, Some(id.clone())).await.unwrap();
    assert_eq!(
        store.live_processes().await.unwrap(),
        vec![(a.clone(), ai, id)]
    );
    store
        .append(
            &a,
            ai,
            vec![env(
                0,
                AgentEvent::SessionEnded {
                    exit_code: Some(0),
                    reason: None,
                },
            )],
        )
        .await
        .unwrap();
    assert!(store.live_processes().await.unwrap().is_empty());

    // Everything persists across a restart.
    drop(store);
    let store = fx.open().await;
    assert_eq!(store.threads().await.unwrap().len(), 1);
    assert_eq!(store.settings().await.unwrap().len(), 2);
}

#[tokio::test]
async fn append_ten_thousand_events_in_one_batch() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc) = started_thread(&store).await;
    let mut batch = vec![env(0, AgentEvent::TurnStarted { turn: 1 })];
    for i in 1..=10_000u64 {
        batch.push(env(
            i,
            delta("m", MessageKind::Assistant, &format!("tok{i} ")),
        ));
    }
    let stored = store.append(&t, inc, batch).await.unwrap();
    assert_eq!(stored.len(), 10_001);
    let open = store.open_items(&t, inc).await.unwrap();
    assert_eq!(open.messages[0].deltas, 10_000);
    let text: String = (1..=10_000).map(|i| format!("tok{i} ")).collect();
    assert_eq!(open.messages[0].sha256, sha_hex(&text));
    assert_eq!(open.messages[0].chars, text.chars().count() as u64);
    assert_eq!(
        store.events_after(&t, None, 100_000).await.unwrap().len(),
        10_001
    );
}

fn rss_kib(pid: u32) -> u64 {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// macOS `phys_footprint`, via `footprint(1)`, in KiB.
fn footprint_kib(pid: u32) -> u64 {
    let out = std::process::Command::new("footprint")
        .args(["-p", &pid.to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    let Some(line) = out.lines().find(|l| l.contains("Footprint:")) else {
        return 0;
    };
    let mut it = line
        .split("Footprint:")
        .nth(1)
        .unwrap_or("")
        .split_whitespace();
    let n: f64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    match it.next() {
        Some("GB") => (n * 1024.0 * 1024.0) as u64,
        Some("MB") => (n * 1024.0) as u64,
        Some("KB") => n as u64,
        Some("bytes") => (n / 1024.0) as u64,
        _ => 0,
    }
}

/// M0-07's stream: 3M `MessageDelta`s (10k tok/s for 5 min), appended in
/// batches as fast as the store takes them, then closed from the projections.
#[tokio::test]
async fn three_million_token_stream_appends_and_closes() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc) = started_thread(&store).await;
    let pid = std::process::id();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let sampler = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let (mut rss, mut fp) = (0, 0);
            while !stop.load(Ordering::Relaxed) {
                rss = rss.max(rss_kib(pid));
                fp = fp.max(footprint_kib(pid));
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            (rss, fp)
        })
    };
    let before = rss_kib(pid);
    let started = std::time::Instant::now();
    const TOTAL: u64 = 3_000_000;
    const BATCH: u64 = 10_000;
    let mut hasher = Sha256::new();
    let mut seq = 0u64;
    store
        .append(&t, inc, vec![env(seq, AgentEvent::TurnStarted { turn: 1 })])
        .await
        .unwrap();
    seq += 1;
    let mut chars = 0u64;
    while seq - 1 < TOTAL {
        let batch: Vec<_> = (0..BATCH)
            .map(|i| {
                let text = format!("t{} ", (seq + i) % 997);
                hasher.update(text.as_bytes());
                chars += text.chars().count() as u64;
                env(seq + i, delta("big", MessageKind::Assistant, &text))
            })
            .collect();
        seq += BATCH;
        let stored = store.append(&t, inc, batch).await.unwrap();
        assert_eq!(stored.len() as u64, BATCH);
    }
    let appended = started.elapsed();
    let open = store.open_items(&t, inc).await.unwrap();
    let m = &open.messages[0];
    assert_eq!((m.deltas, m.chars), (TOTAL, chars));
    let sha: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(m.sha256, sha);
    store
        .append(
            &t,
            inc,
            vec![
                env(
                    seq,
                    AgentEvent::MessageCompleted {
                        message_id: "big".into(),
                        kind: MessageKind::Assistant,
                        body: MessageBody::Streamed {
                            chars: m.chars,
                            deltas: m.deltas,
                            sha256: m.sha256.clone(),
                        },
                    },
                ),
                env(
                    seq + 1,
                    AgentEvent::TurnCompleted {
                        turn: 1,
                        outcome: TurnOutcome::Completed,
                        usage: None,
                    },
                ),
            ],
        )
        .await
        .unwrap();
    assert_eq!(store.integrity_errors(), 0);
    assert_eq!(store.open_items(&t, inc).await.unwrap(), Default::default());
    let closed = started.elapsed();
    stop.store(true, Ordering::Relaxed);
    let (peak_rss, peak_fp) = sampler.join().unwrap();
    eprintln!(
        "3M-stream: appended in {appended:.1?}, closed at {closed:.1?}; RSS before {} MiB, \
         sampled peak RSS {} MiB, sampled peak phys_footprint {} MiB, after {} MiB",
        before / 1024,
        peak_rss / 1024,
        peak_fp / 1024,
        rss_kib(pid) / 1024
    );
    // The closing step is O(open items): the open-items query is instant even now.
    let t0 = std::time::Instant::now();
    store.open_items(&t, inc).await.unwrap();
    assert!(t0.elapsed() < std::time::Duration::from_millis(200));
    let _: &Path = fx.dir.path();
}

#[tokio::test]
async fn symlinks_are_refused() {
    use std::os::unix::fs::symlink;
    let fx = Fixture::new();
    // The database path itself.
    let real = fx.dir.path().join("real.db");
    std::fs::write(&real, b"").unwrap();
    let link = fx.dir.path().join("link.db");
    symlink(&real, &link).unwrap();
    let e = SqliteStore::open(&link, fx.options()).await.unwrap_err();
    assert!(matches!(e, StoreError::Io(_)), "{e:?}");
    assert_eq!(std::fs::metadata(&real).unwrap().len(), 0);
    // A dangling one too.
    let dangling = fx.dir.path().join("dangling.db");
    symlink(fx.dir.path().join("nowhere"), &dangling).unwrap();
    assert!(SqliteStore::open(&dangling, fx.options()).await.is_err());
    assert!(!fx.dir.path().join("nowhere").exists());
    // A symlinked sidecar.
    let other = fx.dir.path().join("other.db");
    symlink(
        fx.dir.path().join("elsewhere"),
        fx.dir.path().join("other.db-wal"),
    )
    .unwrap();
    let e = SqliteStore::open(&other, fx.options()).await.unwrap_err();
    assert!(matches!(e, StoreError::Io(_)), "{e:?}");
    // A loose existing file is tightened.
    let loose = fx.dir.path().join("loose.db");
    std::fs::write(&loose, b"").unwrap();
    std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o644)).unwrap();
    drop(SqliteStore::open(&loose, fx.options()).await.unwrap());
    assert_eq!(
        std::fs::metadata(&loose).unwrap().permissions().mode() & 0o777,
        0o600
    );

    // A symlinked snapshot folder is refused and its target left alone.
    let store = fx.open().await;
    let target = fx.dir.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
    let snaps = fx.dir.path().join("snaps");
    symlink(&target, &snaps).unwrap();
    assert!(store.snapshot(&snaps).await.is_err());
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
    // A loose real folder is tightened.
    let loose_dir = fx.dir.path().join("loose-snaps");
    std::fs::create_dir(&loose_dir).unwrap();
    std::fs::set_permissions(&loose_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    store.snapshot(&loose_dir).await.unwrap();
    assert_eq!(
        std::fs::metadata(&loose_dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

fn panicking(cur: &ThreadState, p: Pending, input: AttentionInput<'_>) -> ThreadState {
    if let AttentionInput::Event(AgentEvent::TurnStarted { turn: 99 }) = input {
        panic!("boom");
    }
    transition(cur, p, input)
}

#[tokio::test]
async fn writer_survives_a_panic_and_rebuilds_hashes() {
    let fx = Fixture::new();
    let mut options = fx.options();
    options.transition = panicking;
    let store = SqliteStore::open(&fx.path(), options).await.unwrap();
    let (t, inc) = started_thread(&store).await;
    store
        .append(
            &t,
            inc,
            vec![env(0, delta("m", MessageKind::Assistant, "ab"))],
        )
        .await
        .unwrap();
    let e = store
        .append(
            &t,
            inc,
            vec![
                env(1, delta("m", MessageKind::Assistant, "cd")),
                env(2, AgentEvent::TurnStarted { turn: 99 }),
            ],
        )
        .await;
    assert!(e.is_err());
    // Nothing of the failed batch is stored and the hash is back to "ab".
    assert_eq!(store.events_after(&t, None, 10).await.unwrap().len(), 1);
    let open = store.open_items(&t, inc).await.unwrap();
    assert_eq!(open.messages[0].sha256, sha_hex("ab"));
    store
        .append(
            &t,
            inc,
            vec![env(1, delta("m", MessageKind::Assistant, "ef"))],
        )
        .await
        .unwrap();
    store
        .append(
            &t,
            inc,
            vec![env(2, completed("m", MessageKind::Assistant, "abef", 2))],
        )
        .await
        .unwrap();
    assert_eq!(store.integrity_errors(), 0);
}

#[tokio::test]
async fn usage_enforcement_and_two_approvals() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc) = started_thread(&store).await;
    let usage = |n: u64| Usage {
        input_tokens: Some(n),
        ..Default::default()
    };
    store
        .append(
            &t,
            inc,
            vec![
                env(0, AgentEvent::TurnStarted { turn: 1 }),
                env(1, AgentEvent::Usage { usage: usage(5) }),
                env(2, AgentEvent::Usage { usage: usage(7) }),
            ],
        )
        .await
        .unwrap();
    let turn_usage = |turn: u32| -> (Option<String>, Option<String>) {
        let c = rusqlite::Connection::open(fx.path()).unwrap();
        c.query_row(
            "SELECT usage, outcome FROM turns WHERE turn = ?1",
            [turn],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    };
    let (u, o) = turn_usage(1);
    assert_eq!(
        serde_json::from_str::<Usage>(&u.unwrap()).unwrap(),
        usage(7)
    );
    assert_eq!(o, None);
    store
        .append(
            &t,
            inc,
            vec![env(
                3,
                AgentEvent::TurnCompleted {
                    turn: 1,
                    outcome: TurnOutcome::Completed,
                    usage: Some(usage(9)),
                },
            )],
        )
        .await
        .unwrap();
    let (u, o) = turn_usage(1);
    assert_eq!(
        serde_json::from_str::<Usage>(&u.unwrap()).unwrap(),
        usage(9)
    );
    assert_eq!(o.as_deref(), Some("completed"));

    // Enforcement: the latest descriptor wins, from events and from the setter.
    let desc = |v: &str| {
        EnforcementDescriptor::evaluate(
            Provider::Claude,
            Some(v.into()),
            false,
            vec![],
            vec![],
            vec![],
        )
    };
    store
        .append(&t, inc, vec![env(4, started())])
        .await
        .unwrap();
    store
        .append(
            &t,
            inc,
            vec![env(
                5,
                AgentEvent::EnforcementChanged {
                    enforcement: desc("2.0"),
                },
            )],
        )
        .await
        .unwrap();
    assert_eq!(
        summary(&store, &t)
            .await
            .enforcement
            .unwrap()
            .cli_version
            .as_deref(),
        Some("2.0")
    );
    store.set_enforcement(&t, inc, desc("3.0")).await.unwrap();
    assert_eq!(
        summary(&store, &t)
            .await
            .enforcement
            .unwrap()
            .cli_version
            .as_deref(),
        Some("3.0")
    );
    assert!(matches!(
        store.set_enforcement(&t, 9, desc("x")).await,
        Err(StoreError::UnknownThread(_))
    ));

    // Two open approvals: the oldest decides the attention detail.
    let reqs = store.requests();
    reqs.open_approval(&key(&t, inc, "first"), &binding("First ask"))
        .await
        .unwrap();
    reqs.open_approval(&key(&t, inc, "second"), &binding("Second ask"))
        .await
        .unwrap();
    store
        .append(
            &t,
            inc,
            vec![
                env(6, approval("second", "Second ask")),
                env(7, approval("first", "First ask")),
            ],
        )
        .await
        .unwrap();
    let s = summary(&store, &t).await;
    assert_eq!(s.pending_approvals, 2);
    assert_eq!(s.attention_detail.as_deref(), Some("First ask"));
    store
        .append(
            &t,
            inc,
            vec![env(8, resolved("first", RequestOutcome::Denied))],
        )
        .await
        .unwrap();
    let s = summary(&store, &t).await;
    assert_eq!(s.pending_approvals, 1);
    assert_eq!(s.attention_detail.as_deref(), Some("Second ask"));

    // approve_with_rule and snapshots are not cleanup: refused while read-only.
    fx.free.store(0, Ordering::SeqCst);
    assert!(matches!(
        reqs.approve_with_rule(&key(&t, inc, "second"), Some(rule("r")))
            .await,
        Err(StoreError::ReadOnly { .. })
    ));
    assert!(matches!(
        store.snapshot(&fx.dir.path().join("s")).await,
        Err(StoreError::ReadOnly { .. })
    ));
    assert_eq!(
        reqs.transition(&key(&t, inc, "second"), &["pending"], "denied")
            .await
            .unwrap()
            .as_deref(),
        Some("pending")
    );
}

#[tokio::test]
async fn events_after_handles_extreme_arguments() {
    let fx = Fixture::new();
    let store = fx.open().await;
    let (t, inc) = started_thread(&store).await;
    store
        .append(
            &t,
            inc,
            vec![env(0, delta("m", MessageKind::Assistant, "x"))],
        )
        .await
        .unwrap();
    assert!(store.events_after(&t, None, 0).await.unwrap().is_empty());
    assert!(
        store
            .events_after(&t, Some(EventId(u64::MAX)), 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .events_after(&t, None, usize::MAX)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn concurrent_reads_and_writes_complete() {
    let fx = Fixture::new();
    let store = Arc::new(fx.open().await);
    let (t, inc) = started_thread(&store).await;
    let mut tasks = Vec::new();
    for i in 0..200u64 {
        let (store, t) = (store.clone(), t.clone());
        tasks.push(tokio::spawn(async move {
            store
                .append(
                    &t,
                    inc,
                    vec![env(i, delta("m", MessageKind::Assistant, "x"))],
                )
                .await
                .unwrap();
            store.events_after(&t, None, 5).await.unwrap();
            store.rules().await.unwrap();
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(store.events_after(&t, None, 1000).await.unwrap().len(), 200);
}
