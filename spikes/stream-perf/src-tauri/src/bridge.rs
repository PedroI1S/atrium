//! The bridge: M0-05 `ClientHandle`/`EventStream` on one side, one `tauri::ipc::Channel` on the other.
//!
//! - One task drains the `EventStream` continuously (M0-05 requires this: a slow consumer
//!   backpressures the socket) and groups events into batches. A batch leaves on a timer
//!   (`batchMs`), or at once when `batchMs` is 0, or when it reaches `MAX_BATCH_EVENTS`.
//! - Flow control: the webview acknowledges `(batch seq, per-thread highest rendered id)` after it
//!   has applied and painted. Only then does the bridge call `ClientHandle::applied`. While more than
//!   `maxInFlightBytes` of batches are unacknowledged the task stops reading the stream, so a slow
//!   webview pushes back on the socket and the daemon instead of growing a queue in this process.

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use spike_daemon_socket::{
    Client, ClientHandle, EventStream,
    agent::AgentEvent,
    wire::{AgentLaunch, CommandResult, DaemonCommand, EventId, ServerMessage, ThreadId},
};
use tauri::{
    State,
    ipc::{Channel, InvokeResponseBody},
};
use tokio::sync::Notify;

use crate::{
    corpus::Corpus,
    encode::{Encoding, Item, encode},
};

const MAX_BATCH_EVENTS: usize = 4096;
const HIST_MS: usize = 2000;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeConfig {
    pub threads: u32,
    pub tokens_per_second: u32,
    pub chunk_chars: u32,
    pub duration_ms: u64,
    #[serde(default)]
    pub tool_call_every_ms: u64,
    pub encoding: String,
    /// 0 = one message per event (no batching).
    pub batch_ms: u64,
    #[serde(default = "default_in_flight")]
    pub max_in_flight_bytes: usize,
    /// Replace the fake agent's `x` text by markdown of the same length (see `corpus.rs`).
    #[serde(default = "yes")]
    pub corpus: bool,
    pub dir: Option<String>,
    pub daemon_exe: Option<String>,
}

fn default_in_flight() -> usize {
    8 * 1024 * 1024
}
fn yes() -> bool {
    true
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartInfo {
    pub thread_ids: Vec<String>,
    pub encoding: String,
    pub start_epoch_ms: f64,
}

pub fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

#[derive(Default)]
struct Flow {
    inflight: Mutex<VecDeque<(u64, usize)>>,
    bytes: AtomicUsize,
    notify: Notify,
    /// Shutdown mode: keep reading the event stream (so command replies are never stuck behind a full
    /// event queue) but drop what is read, and stop waiting for acknowledgements.
    draining: AtomicBool,
}

struct StatsInner {
    events: u64,
    deltas: u64,
    chars: u64,
    batches: u64,
    bytes: u64,
    encode_us: Vec<u32>,
    batch_events: Vec<u32>,
    /// `recv - at_ms` per event: agent timestamp to arrival in this process (ms buckets).
    hist_to_bridge: Vec<u32>,
    /// `sent - recv` per event: time spent waiting in the batch.
    hist_batching: Vec<u32>,
    backpressure_waits: u64,
    backpressure_ms: f64,
    max_in_flight_bytes: usize,
    first_ms: f64,
    last_ms: f64,
    acks: u64,
    duplicates_dropped: u64,
}

impl StatsInner {
    fn new() -> Self {
        Self {
            events: 0,
            deltas: 0,
            chars: 0,
            batches: 0,
            bytes: 0,
            encode_us: Vec::new(),
            batch_events: Vec::new(),
            hist_to_bridge: vec![0; HIST_MS],
            hist_batching: vec![0; HIST_MS],
            backpressure_waits: 0,
            backpressure_ms: 0.0,
            max_in_flight_bytes: 0,
            first_ms: 0.0,
            last_ms: 0.0,
            acks: 0,
            duplicates_dropped: 0,
        }
    }
}

fn hist_add(h: &mut [u32], ms: f64) {
    let i = (ms.max(0.0) as usize).min(h.len() - 1);
    h[i] += 1;
}

fn hist_pct(h: &[u32], p: f64) -> f64 {
    let total: u64 = h.iter().map(|&c| u64::from(c)).sum();
    if total == 0 {
        return 0.0;
    }
    let want = (total as f64 * p).ceil().max(1.0) as u64;
    let mut seen = 0;
    for (i, &c) in h.iter().enumerate() {
        seen += u64::from(c);
        if seen >= want {
            return i as f64;
        }
    }
    (h.len() - 1) as f64
}

fn hist_max(h: &[u32]) -> f64 {
    h.iter().rposition(|&c| c > 0).map_or(0.0, |i| i as f64)
}

fn vec_pct(v: &mut [u32], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_unstable();
    f64::from(v[((v.len() - 1) as f64 * p) as usize])
}

pub struct Active {
    handle: ClientHandle,
    threads: Vec<ThreadId>,
    flow: Arc<Flow>,
    stats: Arc<Mutex<StatsInner>>,
    pump: tauri::async_runtime::JoinHandle<()>,
}

#[derive(Default)]
pub struct Bridge(tokio::sync::Mutex<Option<Active>>);

fn default_daemon_exe() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../../target/release/spike-daemon").to_string()
}

#[tauri::command]
pub async fn bridge_start(
    bridge: State<'_, Bridge>,
    config: BridgeConfig,
    on_batch: Channel<InvokeResponseBody>,
) -> Result<StartInfo, String> {
    let mut slot = bridge.0.lock().await;
    if slot.is_some() {
        return Err("bridge already running".into());
    }
    let (active, info) = start(config, on_batch).await?;
    *slot = Some(active);
    Ok(info)
}

/// Connects (spawning the daemon if needed), starts the fake threads and subscribes to each.
pub async fn start(
    config: BridgeConfig,
    on_batch: Channel<InvokeResponseBody>,
) -> Result<(Active, StartInfo), String> {
    let enc = Encoding::parse(&config.encoding).ok_or("unknown encoding")?;
    let dir = config.dir.clone().unwrap_or_else(|| {
        std::env::temp_dir()
            .join(format!("atrium-stream-perf-{}", std::process::id()))
            .to_string_lossy()
            .into_owned()
    });
    let exe = config.daemon_exe.clone().unwrap_or_else(default_daemon_exe);
    let client = Client::connect_or_spawn(&dir, &exe, "stream-perf")
        .await
        .map_err(|e| format!("connect_or_spawn({exe}): {e}"))?;
    let (handle, events) = client.into_parts();

    let flow = Arc::new(Flow::default());
    let stats = Arc::new(Mutex::new(StatsInner::new()));
    let index: Arc<Mutex<HashMap<ThreadId, u16>>> = Arc::default();

    // The pump drains the stream from here on, so StartThread/Subscribe replies never wait behind events.
    let pump = tauri::async_runtime::spawn(pump(
        events,
        config.clone(),
        enc,
        on_batch,
        flow.clone(),
        stats.clone(),
        index.clone(),
    ));

    let start_epoch_ms = now_ms();
    let mut threads = Vec::new();
    for n in 0..config.threads {
        let launch = DaemonCommand::StartThread {
            agent: AgentLaunch::Fake {
                tokens_per_second: config.tokens_per_second,
                chunk_chars: config.chunk_chars,
                duration_ms: config.duration_ms,
                tool_call_every_ms: config.tool_call_every_ms,
            },
            cwd: dir.clone(),
        };
        let thread = match handle.command(launch).await.map_err(|e| e.to_string())? {
            CommandResult::Ok { value } => ThreadId(
                value["thread_id"]
                    .as_str()
                    .ok_or("StartThread: no thread_id")?
                    .to_string(),
            ),
            CommandResult::Err { message } => return Err(format!("StartThread {n}: {message}")),
        };
        index.lock().unwrap().insert(thread.clone(), n as u16);
        threads.push(thread.clone());
        match handle
            .subscribe(thread, None)
            .await
            .map_err(|e| e.to_string())?
        {
            CommandResult::Ok { .. } => {}
            CommandResult::Err { message } => return Err(format!("Subscribe {n}: {message}")),
        }
    }
    let info = StartInfo {
        thread_ids: threads.iter().map(|t| t.0.clone()).collect(),
        encoding: enc.name().into(),
        start_epoch_ms,
    };
    Ok((
        Active {
            handle,
            threads,
            flow,
            stats,
            pump,
        },
        info,
    ))
}

/// Per-bridge preparation of events for the wire.
///
/// After a reconnect the client replays events that were read but not yet acknowledged, so the same
/// id can arrive twice. A duplicate must be dropped **before** anything stateful happens: the corpus
/// offsets below advance per delta, and a replayed delta would shift all later replacement text.
/// (The webview also ignores `id <= lastId`; both ends are guarded.)
pub struct Prep {
    corpus: Option<Corpus>,
    offsets: HashMap<(u16, String), usize>,
    forwarded: HashMap<u16, u64>,
    pub duplicates: u64,
}

impl Prep {
    pub fn new(corpus: bool) -> Self {
        Self {
            corpus: corpus.then(Corpus::new),
            offsets: HashMap::new(),
            forwarded: HashMap::new(),
            duplicates: 0,
        }
    }

    /// `None` for a duplicate (already forwarded); otherwise the number of delta text bytes.
    pub fn prepare(&mut self, item: &mut Item) -> Option<usize> {
        let last = self.forwarded.entry(item.thread).or_insert(0);
        if item.event.id.0 <= *last {
            self.duplicates += 1;
            return None;
        }
        *last = item.event.id.0;
        let corpus = self.corpus.as_ref();
        Some(match &mut item.event.envelope.event {
            AgentEvent::MessageDelta {
                message_id, text, ..
            } => {
                let n = text.len();
                if let Some(c) = corpus {
                    let off = self
                        .offsets
                        .entry((item.thread, message_id.clone()))
                        .or_insert(0);
                    *text = c.slice(*off, n);
                    *off += n;
                }
                n
            }
            AgentEvent::MessageCompleted { text, .. } => {
                if let Some(c) = corpus {
                    *text = c.slice(0, text.len());
                }
                0
            }
            _ => 0,
        })
    }
}

async fn pump(
    mut events: EventStream,
    cfg: BridgeConfig,
    enc: Encoding,
    channel: Channel<InvokeResponseBody>,
    flow: Arc<Flow>,
    stats: Arc<Mutex<StatsInner>>,
    index: Arc<Mutex<HashMap<ThreadId, u16>>>,
) {
    let mut prep = Prep::new(cfg.corpus);
    let mut batch: Vec<Item> = Vec::new();
    let mut recv_times: Vec<f64> = Vec::new();
    let mut seq: u64 = 0;
    let mut tick = tokio::time::interval(Duration::from_millis(cfg.batch_ms.max(1)));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let flush = |batch: &mut Vec<Item>, recv_times: &mut Vec<f64>, seq: &mut u64| {
        if batch.is_empty() {
            return;
        }
        let t0 = Instant::now();
        let sent = now_ms();
        let body = encode(enc, *seq, sent, batch);
        let size = match &body {
            InvokeResponseBody::Json(s) => s.len(),
            InvokeResponseBody::Raw(b) => b.len(),
        };
        let enc_us = t0.elapsed().as_micros() as u32;
        {
            let mut inflight = flow.inflight.lock().unwrap();
            inflight.push_back((*seq, size));
        }
        let now_bytes = flow.bytes.fetch_add(size, Ordering::SeqCst) + size;
        {
            let mut s = stats.lock().unwrap();
            s.batches += 1;
            s.bytes += size as u64;
            s.encode_us.push(enc_us);
            s.batch_events.push(batch.len() as u32);
            s.max_in_flight_bytes = s.max_in_flight_bytes.max(now_bytes);
            for r in recv_times.iter() {
                hist_add(&mut s.hist_batching, sent - r);
            }
        }
        if let Err(e) = channel.send(body) {
            eprintln!("channel send failed: {e}");
        }
        *seq += 1;
        batch.clear();
        recv_times.clear();
    };

    loop {
        // Backpressure: stop reading while too much is unacknowledged.
        let over = || {
            flow.bytes.load(Ordering::SeqCst) > cfg.max_in_flight_bytes
                && !flow.draining.load(Ordering::SeqCst)
        };
        if over() {
            flush(&mut batch, &mut recv_times, &mut seq);
            let t = Instant::now();
            while over() {
                flow.notify.notified().await;
            }
            let mut s = stats.lock().unwrap();
            s.backpressure_waits += 1;
            s.backpressure_ms += t.elapsed().as_secs_f64() * 1000.0;
        }
        tokio::select! {
            msg = events.next() => {
                let Ok(msg) = msg else { break };
                let ServerMessage::Event { event } = msg else { continue };
                if flow.draining.load(Ordering::SeqCst) { continue; }
                let recv = now_ms();
                let Some(&thread) = index.lock().unwrap().get(&event.thread_id) else { continue };
                let at = event.envelope.at_ms as f64;
                let mut item = Item { thread, event };
                let Some(chars) = prep.prepare(&mut item) else {
                    stats.lock().unwrap().duplicates_dropped += 1;
                    continue;
                };
                {
                    let mut s = stats.lock().unwrap();
                    s.events += 1;
                    if chars > 0 {
                        s.deltas += 1;
                        s.chars += chars as u64;
                    }
                    if s.first_ms == 0.0 { s.first_ms = recv; }
                    s.last_ms = recv;
                    hist_add(&mut s.hist_to_bridge, recv - at);
                }
                recv_times.push(recv);
                batch.push(item);
                if cfg.batch_ms == 0 || batch.len() >= MAX_BATCH_EVENTS {
                    flush(&mut batch, &mut recv_times, &mut seq);
                }
            }
            _ = tick.tick(), if cfg.batch_ms > 0 && !batch.is_empty() => {
                flush(&mut batch, &mut recv_times, &mut seq);
            }
        }
    }
    flush(&mut batch, &mut recv_times, &mut seq);
}

/// The webview reports what it has applied and painted. `seq` is the newest batch included.
#[tauri::command]
pub async fn bridge_ack(
    bridge: State<'_, Bridge>,
    seq: u64,
    cursors: Vec<(u16, u64)>,
) -> Result<(), String> {
    let slot = bridge.0.lock().await;
    let Some(active) = slot.as_ref() else {
        return Ok(());
    };
    for (thread, id) in cursors {
        if let Some(t) = active.threads.get(thread as usize) {
            active.handle.applied(t, EventId(id));
        }
    }
    let mut freed = 0;
    {
        let mut inflight = active.flow.inflight.lock().unwrap();
        while inflight.front().is_some_and(|&(s, _)| s <= seq) {
            freed += inflight.pop_front().map_or(0, |(_, b)| b);
        }
    }
    active.flow.bytes.fetch_sub(freed, Ordering::SeqCst);
    active.flow.notify.notify_one();
    active.stats.lock().unwrap().acks += 1;
    Ok(())
}

#[tauri::command]
pub async fn bridge_stats(bridge: State<'_, Bridge>) -> Result<Value, String> {
    let slot = bridge.0.lock().await;
    let active = slot.as_ref().ok_or("bridge not running")?;
    let mut s = active.stats.lock().unwrap();
    let span_s = ((s.last_ms - s.first_ms) / 1000.0).max(0.001);
    let enc_p50 = vec_pct(&mut s.encode_us, 0.5);
    let enc_p99 = vec_pct(&mut s.encode_us, 0.99);
    let enc_max = s.encode_us.iter().copied().max().unwrap_or(0);
    let enc_total: u64 = s.encode_us.iter().map(|&x| u64::from(x)).sum();
    let be_p50 = vec_pct(&mut s.batch_events, 0.5);
    let be_max = s.batch_events.iter().copied().max().unwrap_or(0);
    Ok(json!({
        "events": s.events,
        "deltas": s.deltas,
        "chars": s.chars,
        "tokensApprox": s.chars / 4,
        "spanSeconds": span_s,
        "eventsPerSecond": s.events as f64 / span_s,
        "tokensPerSecondApprox": (s.chars / 4) as f64 / span_s,
        "batches": s.batches,
        "bytes": s.bytes,
        "bytesPerEvent": s.bytes as f64 / s.events.max(1) as f64,
        "batchEventsP50": be_p50,
        "batchEventsMax": be_max,
        "encodeUs": { "p50": enc_p50, "p99": enc_p99, "max": enc_max, "totalMs": enc_total as f64 / 1000.0 },
        "agentToBridgeMs": { "p50": hist_pct(&s.hist_to_bridge, 0.5), "p95": hist_pct(&s.hist_to_bridge, 0.95), "p99": hist_pct(&s.hist_to_bridge, 0.99), "max": hist_max(&s.hist_to_bridge) },
        "batchingMs": { "p50": hist_pct(&s.hist_batching, 0.5), "p95": hist_pct(&s.hist_batching, 0.95), "p99": hist_pct(&s.hist_batching, 0.99), "max": hist_max(&s.hist_batching) },
        "backpressureWaits": s.backpressure_waits,
        "backpressureMs": s.backpressure_ms,
        "maxInFlightBytes": s.max_in_flight_bytes,
        "acks": s.acks,
        "duplicatesDropped": s.duplicates_dropped,
    }))
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShutdownReport {
    pub stopped: usize,
    pub failed: usize,
    /// The deadline passed before every StopThread was answered; the connection was dropped instead.
    pub timed_out: bool,
}

/// Stops the fake threads and tears the bridge down.
///
/// A stalled or hidden webview stops acknowledging, which exhausts the flow-control credits and parks
/// the pump; once the client's bounded event queue is full it cannot read a command reply either, so
/// a plain "send StopThread and wait" never finishes. Shutdown therefore switches the pump to drain
/// mode first (read and drop events, ignore credits), bounds the whole thing by `deadline`, and falls
/// back to dropping the connection (which stops the client actor) when the deadline passes.
pub async fn shutdown(active: Active, deadline: Duration) -> ShutdownReport {
    active.flow.draining.store(true, Ordering::SeqCst);
    active.flow.notify.notify_one();
    let mut report = ShutdownReport::default();
    let end = tokio::time::Instant::now() + deadline;
    for t in &active.threads {
        let cmd = active.handle.command(DaemonCommand::StopThread {
            thread_id: t.clone(),
        });
        match tokio::time::timeout_at(end, cmd).await {
            Ok(Ok(CommandResult::Ok { .. })) => report.stopped += 1,
            Ok(_) => report.failed += 1,
            Err(_) => {
                report.timed_out = true;
                break;
            }
        }
    }
    active.pump.abort();
    report
}

#[tauri::command]
pub async fn bridge_stop(bridge: State<'_, Bridge>) -> Result<ShutdownReport, String> {
    let Some(active) = bridge.0.lock().await.take() else {
        return Ok(ShutdownReport::default());
    };
    Ok(shutdown(active, Duration::from_secs(5)).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use spike_daemon_socket::{agent::*, wire::StoredEvent};

    fn delta(id: u64, text: &str) -> Item {
        Item {
            thread: 0,
            event: StoredEvent {
                id: EventId(id),
                thread_id: ThreadId("t".into()),
                envelope: AgentEventEnvelope {
                    seq: id,
                    at_ms: 0,
                    event: AgentEvent::MessageDelta {
                        message_id: "1".into(),
                        kind: MessageKind::Assistant,
                        text: text.into(),
                    },
                },
            },
        }
    }

    fn text(item: &Item) -> String {
        match &item.event.envelope.event {
            AgentEvent::MessageDelta { text, .. } => text.clone(),
            _ => unreachable!(),
        }
    }

    /// Delivers `ids` in order and returns what the webview would receive.
    fn deliver(prep: &mut Prep, ids: &[u64]) -> Vec<(u64, String)> {
        ids.iter()
            .filter_map(|&id| {
                let mut item = delta(id, "xxxx");
                prep.prepare(&mut item)?;
                Some((id, text(&item)))
            })
            .collect()
    }

    #[test]
    fn replay_after_reconnect_does_not_shift_corpus_text() {
        // Reference: ids 1..=10 delivered once.
        let reference = deliver(&mut Prep::new(true), &(1..=10).collect::<Vec<_>>());
        // Reconnect before the acknowledgements of 6..=10: the client replays them, then goes on.
        let mut prep = Prep::new(true);
        let mut got = deliver(&mut prep, &(1..=10).collect::<Vec<_>>());
        let replayed = deliver(&mut prep, &(6..=12).collect::<Vec<_>>());
        assert_eq!(replayed.iter().map(|r| r.0).collect::<Vec<_>>(), vec![11, 12]);
        got.extend(replayed);
        // The first ten are untouched, and 11 and 12 continue the corpus where 10 stopped.
        assert_eq!(got[..10], reference[..]);
        let continued = deliver(&mut Prep::new(true), &(1..=12).collect::<Vec<_>>());
        assert_eq!(got, continued);
        assert_eq!(prep.duplicates, 5);
    }

    #[test]
    fn replay_after_the_acknowledgement_is_unaffected() {
        // Reconnect after everything was acknowledged: the client resumes at 11, nothing to drop.
        let mut prep = Prep::new(true);
        let mut got = deliver(&mut prep, &(1..=10).collect::<Vec<_>>());
        got.extend(deliver(&mut prep, &[11, 12]));
        assert_eq!(got, deliver(&mut Prep::new(true), &(1..=12).collect::<Vec<_>>()));
        assert_eq!(prep.duplicates, 0);
    }

    /// Stopping while the webview has stopped acknowledging. Needs the daemon binaries:
    /// `cargo build --release -p spike-daemon-socket --bins` at the repository root
    /// (or ATRIUM_DAEMON_EXE).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stop_with_exhausted_credits_completes_within_the_deadline() {
        let exe = std::env::var("ATRIUM_DAEMON_EXE").unwrap_or_else(|_| default_daemon_exe());
        assert!(
            std::path::Path::new(&exe).exists(),
            "daemon binary missing: {exe}; see the test's doc comment"
        );
        let dir = std::env::temp_dir()
            .join(format!("asp-stop-test-{}", std::process::id()))
            .to_string_lossy()
            .into_owned();
        let cfg = BridgeConfig {
            threads: 2,
            tokens_per_second: 20_000,
            chunk_chars: 4,
            duration_ms: 60_000,
            tool_call_every_ms: 0,
            encoding: "bin".into(),
            batch_ms: 2,
            max_in_flight_bytes: 1024,
            corpus: true,
            dir: Some(dir.clone()),
            daemon_exe: Some(exe),
        };
        // Nobody ever acknowledges: the credits run out almost at once.
        let (active, _) = start(cfg, Channel::new(|_| Ok(()))).await.unwrap();
        let flow = active.flow.clone();
        for _ in 0..100 {
            if flow.bytes.load(Ordering::SeqCst) > 1024 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(flow.bytes.load(Ordering::SeqCst) > 1024, "credits were never exhausted");
        // Let the daemon fill the client's bounded event queue behind the parked pump.
        tokio::time::sleep(Duration::from_millis(1500)).await;

        let t = Instant::now();
        let report = shutdown(active, Duration::from_secs(5)).await;
        let took = t.elapsed();
        let _ = std::process::Command::new("pkill").arg("-f").arg(&dir).status();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!report.timed_out, "shutdown hit its deadline: {report:?}");
        assert_eq!(report.stopped, 2, "{report:?}");
        assert!(took < Duration::from_secs(4), "took {took:?}");
    }
}
