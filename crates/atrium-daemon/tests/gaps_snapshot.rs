#[path = "common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use atrium_agents::BoxFuture;
use atrium_core::{NewThread, OpenItems, RequestStore, Signal, StorageStatus, Store, StoreResult};
use atrium_host::supervise::ProcessIdentity;
use atrium_protocol::agent::{AgentEvent, AgentEventEnvelope, SessionCommand};
use atrium_protocol::policy::{EnforcementDescriptor, Rule, RuleId};
use atrium_protocol::thread::{ThreadId, ThreadState, ThreadSummary};
use atrium_protocol::wire::{AgentLaunch, DaemonCommand, EventId, StoredEvent};
use common::{Rig, kinds, soon};
use serde_json::Value;
use tokio::sync::Notify;

struct SnapshotGate {
    hold: AtomicBool,
    snapshot_calls: AtomicUsize,
    storage_status_calls: AtomicUsize,
    started: Notify,
    release: Notify,
    storage_checked: Notify,
    turn_started: Notify,
}

impl SnapshotGate {
    fn new(hold: bool) -> Arc<Self> {
        Arc::new(Self {
            hold: AtomicBool::new(hold),
            snapshot_calls: AtomicUsize::new(0),
            storage_status_calls: AtomicUsize::new(0),
            started: Notify::new(),
            release: Notify::new(),
            storage_checked: Notify::new(),
            turn_started: Notify::new(),
        })
    }
}

struct GatedStore {
    inner: Arc<dyn Store>,
    gate: Arc<SnapshotGate>,
}

impl Store for GatedStore {
    fn create_thread(&self, new: NewThread) -> BoxFuture<'_, StoreResult<ThreadSummary>> {
        self.inner.create_thread(new)
    }

    fn begin_incarnation(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<u32>> {
        self.inner.begin_incarnation(thread)
    }

    fn set_process(
        &self,
        thread: &ThreadId,
        incarnation: u32,
        process: Option<ProcessIdentity>,
    ) -> BoxFuture<'_, StoreResult<()>> {
        self.inner.set_process(thread, incarnation, process)
    }

    fn append(
        &self,
        thread: &ThreadId,
        incarnation: u32,
        events: Vec<AgentEventEnvelope>,
    ) -> BoxFuture<'_, StoreResult<Vec<StoredEvent>>> {
        let has_turn_started = events
            .iter()
            .any(|env| matches!(env.event, AgentEvent::TurnStarted { .. }));
        let fut = self.inner.append(thread, incarnation, events);
        let gate = self.gate.clone();
        Box::pin(async move {
            let stored = fut.await?;
            if has_turn_started {
                gate.turn_started.notify_one();
            }
            Ok(stored)
        })
    }

    fn signal(
        &self,
        thread: &ThreadId,
        signal: Signal,
    ) -> BoxFuture<'_, StoreResult<ThreadSummary>> {
        self.inner.signal(thread, signal)
    }

    fn events_after(
        &self,
        thread: &ThreadId,
        after: Option<EventId>,
        limit: usize,
    ) -> BoxFuture<'_, StoreResult<Vec<StoredEvent>>> {
        self.inner.events_after(thread, after, limit)
    }

    fn last_event(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<Option<EventId>>> {
        self.inner.last_event(thread)
    }

    fn thread(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<Option<ThreadSummary>>> {
        self.inner.thread(thread)
    }

    fn threads(&self) -> BoxFuture<'_, StoreResult<Vec<ThreadSummary>>> {
        self.inner.threads()
    }

    fn archive(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<()>> {
        self.inner.archive(thread)
    }

    fn rename(&self, thread: &ThreadId, title: &str) -> BoxFuture<'_, StoreResult<ThreadSummary>> {
        self.inner.rename(thread, title)
    }

    fn mark_read(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<()>> {
        self.inner.mark_read(thread)
    }

    fn set_enforcement(
        &self,
        thread: &ThreadId,
        incarnation: u32,
        descriptor: EnforcementDescriptor,
    ) -> BoxFuture<'_, StoreResult<()>> {
        self.inner.set_enforcement(thread, incarnation, descriptor)
    }

    fn resume_id(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<Option<String>>> {
        self.inner.resume_id(thread)
    }

    fn open_items(
        &self,
        thread: &ThreadId,
        incarnation: u32,
    ) -> BoxFuture<'_, StoreResult<OpenItems>> {
        self.inner.open_items(thread, incarnation)
    }

    fn live_processes(&self) -> BoxFuture<'_, StoreResult<Vec<(ThreadId, u32, ProcessIdentity)>>> {
        self.inner.live_processes()
    }

    fn rules(&self) -> BoxFuture<'_, StoreResult<(u64, Vec<Rule>)>> {
        self.inner.rules()
    }

    fn create_rule(&self, rule: Rule) -> BoxFuture<'_, StoreResult<Rule>> {
        self.inner.create_rule(rule)
    }

    fn delete_rule(&self, rule: &RuleId) -> BoxFuture<'_, StoreResult<()>> {
        self.inner.delete_rule(rule)
    }

    fn settings(&self) -> BoxFuture<'_, StoreResult<Vec<(String, Value)>>> {
        self.inner.settings()
    }

    fn set_setting(&self, key: &str, value: Value) -> BoxFuture<'_, StoreResult<()>> {
        self.inner.set_setting(key, value)
    }

    fn storage_status(&self) -> StorageStatus {
        self.gate
            .storage_status_calls
            .fetch_add(1, Ordering::SeqCst);
        self.gate.storage_checked.notify_waiters();
        self.inner.storage_status()
    }

    fn snapshot(&self, dir: &Path) -> BoxFuture<'_, StoreResult<PathBuf>> {
        let dir = dir.to_path_buf();
        Box::pin(async move {
            self.gate.snapshot_calls.fetch_add(1, Ordering::SeqCst);
            self.gate.started.notify_one();
            if self.gate.hold.load(Ordering::SeqCst) {
                self.gate.release.notified().await;
            }
            self.inner.snapshot(&dir).await
        })
    }

    fn requests(&self) -> Arc<dyn RequestStore> {
        self.inner.requests()
    }
}

fn fake(duration_ms: u64) -> AgentLaunch {
    AgentLaunch::Fake {
        tokens_per_second: 2_000,
        chunk_chars: 16,
        duration_ms,
        tool_call_every_ms: 0,
    }
}

#[tokio::test]
async fn turn_started_during_snapshot_waits_for_snapshot_to_finish() {
    let gate = SnapshotGate::new(true);
    let wrapper_gate = gate.clone();
    let mut rig = Rig::start_with(|options| {
        options.snapshot_check_every = Duration::from_millis(50);
        options.store_wrapper = Some(Arc::new(move |inner| {
            Arc::new(GatedStore {
                inner,
                gate: wrapper_gate.clone(),
            })
        }));
    })
    .await;
    let mut wire = rig.dev().await;

    // Wait until snapshot_daily enters Store::snapshot() (holding `publish`).
    soon(gate.started.notified()).await;
    assert_eq!(gate.snapshot_calls.load(Ordering::SeqCst), 1);

    let thread = wire.start_thread(fake(50), Some("hello")).await.thread_id;

    // Bounded wait as a negative check: `TurnStarted` must not be stored while
    // `snapshot()` holds `publish`.
    let stored_during_snapshot =
        tokio::time::timeout(Duration::from_millis(200), gate.turn_started.notified()).await;
    assert!(
        stored_during_snapshot.is_err(),
        "TurnStarted was stored while snapshot was held"
    );

    gate.hold.store(false, Ordering::SeqCst);
    gate.release.notify_one();

    soon(gate.turn_started.notified()).await;
    wire.thread_when(&thread, |summary| summary.state == ThreadState::Idle)
        .await;
    let history = wire.history(&thread).await;
    assert!(kinds(&history).contains(&"turn_started"));

    let snapshots: Vec<_> = std::fs::read_dir(&rig.paths.snapshots_dir)
        .expect("snapshots_dir")
        .filter_map(|entry| entry.ok())
        .collect();
    assert!(
        !snapshots.is_empty(),
        "expected a snapshot file in {}",
        rig.paths.snapshots_dir.display()
    );

    rig.stop(false).await;
}

#[tokio::test]
async fn snapshot_is_not_called_while_a_turn_is_running() {
    let gate = SnapshotGate::new(false);
    let wrapper_gate = gate.clone();
    let mut rig = Rig::start_with(|options| {
        options.snapshot_check_every = Duration::from_millis(150);
        options.store_wrapper = Some(Arc::new(move |inner| {
            Arc::new(GatedStore {
                inner,
                gate: wrapper_gate.clone(),
            })
        }));
    })
    .await;
    let mut wire = rig.dev().await;

    let thread = wire
        .start_thread(fake(5_000), Some("stay open"))
        .await
        .thread_id;
    wire.thread_when(&thread, |summary| summary.state == ThreadState::Running)
        .await;

    // Wait for at least two storage_status() checks while the turn is running
    // (watch_storage only checks every 2s, so >= 2 checks proves
    // snapshot_daily ran its due/running_turns check at least once).
    let before = gate.storage_status_calls.load(Ordering::SeqCst);
    soon(async {
        while gate.storage_status_calls.load(Ordering::SeqCst) < before + 2 {
            gate.storage_checked.notified().await;
        }
    })
    .await;

    assert_eq!(
        gate.snapshot_calls.load(Ordering::SeqCst),
        0,
        "snapshot() must not be called while a turn is running"
    );

    wire.ok(DaemonCommand::ThreadCommand {
        thread_id: thread.clone(),
        command: SessionCommand::Interrupt,
    })
    .await;
    wire.thread_when(&thread, |summary| summary.state == ThreadState::Idle)
        .await;
    rig.stop(false).await;
}
