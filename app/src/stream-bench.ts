// The streaming run of contract §14 (`ATRIUM_BENCH=stream`, run by app/scripts/stream_bench.py
// against a real `atriumd` with the synthetic agent): 10 threads at 1,000 tokens/s each for
// 30 s, one token per event (4 characters, the hardest case, as in M0-07), a tool call every
// 4 s. The main window shows one thread and holds the models of all ten, so every event of
// every thread is delivered to this window and applied (M0-07's "multi" scenario).
//
// Frame times are `requestAnimationFrame` timestamp deltas: WebKitGTK gives a page no
// presentation timestamps. They don't see compositor or GPU drops.

import { perf, resetPerf } from "./chat/perf";
import { holdThread } from "./chat/threads";
import { command, daemon } from "./daemon/client";
import type { ThreadSummary } from "./daemon/types";
import { writeReport } from "./platform";
import { sleep } from "./roundtrip";
import { openThread } from "./ui/Sidebar";

const THREADS = 10;
const TOKENS_PER_SECOND = 1_000;
const CHARS_PER_TOKEN = 4;
const SECONDS = 30;
const TOOL_CALL_EVERY_MS = 4_000;
const POLL_MS = 50;
/** Frames sampled with nothing streaming, before the run. */
const IDLE_SAMPLE_MS = 5_000;
const START_TIMEOUT_MS = 30_000;
const END_TIMEOUT_MS = 90_000;
/** Contract §14's gate values. */
const GATE_FRAME_MS = 16.7;
const GATE_P99_MS = 33;
/** rAF timestamps in WebKitGTK are whole milliseconds. */
const TIMESTAMP_RESOLUTION_MS = 1;
/** A frame this many refresh intervals long skipped at least one display update (M0-07's "dropped"). */
const DROPPED_FACTOR = 1.5;
const PERCENT = 100;

async function waitFor(what: string, test: () => boolean, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!test()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await sleep(POLL_MS);
  }
}

interface Between { flushMs: number; deliveries: number; structure: number; summaries: number; markRead: number; renders: number; measures: number }
interface FrameWork {
  /** From the frame's timestamp to its first callback (ms). */
  delay: number;
  /** From the frame's first callback to a task posted from it (its rendering done), ms. */
  busy: number;
  /** What happened from this frame's callbacks to the next frame's. */
  between: Between | null;
}

/** The shown thread's transcript, once the run has started. */
let shown: { structure: number } | null = null;

function snapshot(): Between {
  let deliveries = 0;
  let summaries = 0;
  for (const [k, n] of Object.entries(perf.counts)) {
    if (k === "deliver:event") deliveries += n;
    else if (k.startsWith("deliver:")) summaries += n;
  }
  return { flushMs: perf.ms.flush ?? 0, deliveries, summaries, structure: shown?.structure ?? 0, markRead: perf.counts.markRead ?? 0, renders: perf.counts.renderBlock ?? 0, measures: perf.counts.measure ?? 0 };
}

const quantile =(sorted: number[], q: number): number => sorted[Math.min(sorted.length - 1, Math.round(q * (sorted.length - 1)))] ?? Number.NaN;
const TENTHS = 10;
const round1 = (x: number) => Math.round(x * TENTHS) / TENTHS;

/** `onlyShown` (diagnosis, not the §14 run): only the shown thread is held, so only its events reach the window. */
export async function runStreamBench(onlyShown = false): Promise<void> {
  await waitFor("the daemon", () => daemon().connection.type === "connected" && daemon().loaded, START_TIMEOUT_MS);

  const frames: number[] = [];
  // What happened in each frame (index i: the frame that `frames[i]` ends), to tell where long
  // frames come from: main-thread time from this frame's callbacks to after its rendering,
  // the batches applied in it, and what changed in the shown thread before it.
  const work: FrameWork[] = [];
  let sampling = false;
  let previous = 0;
  let hiddenDuring = 0;
  let marks = snapshot();
  document.addEventListener("visibilitychange", () => { if (sampling && document.hidden) hiddenDuring++; });
  const after = new MessageChannel();
  let pending: FrameWork | null = null;
  let frameStart = 0;
  after.port1.onmessage = () => { if (pending) pending.busy = performance.now() - frameStart; };
  // The same window with nothing streaming, first: how much of the frame statistics is the
  // platform's own timing (rAF timestamps of an idle page).
  const idle: number[] = [];
  let idling = false;
  let idlePrevious = 0;
  const loop = (t: number) => {
    if (idling) {
      if (idlePrevious) idle.push(t - idlePrevious);
      idlePrevious = t;
    }
    if (sampling) {
      if (previous) frames.push(t - previous);
      previous = t;
      const now = snapshot();
      // Work done since the last frame's callbacks began: this record is for the frame that
      // starts now and ends at the next timestamp.
      if (pending) {
        pending.between = { flushMs: now.flushMs - marks.flushMs, deliveries: now.deliveries - marks.deliveries, structure: now.structure - marks.structure, summaries: now.summaries - marks.summaries, markRead: now.markRead - marks.markRead, renders: now.renders - marks.renders, measures: now.measures - marks.measures };
        work.push(pending);
      }
      marks = now;
      frameStart = performance.now();
      pending = { delay: frameStart - t, busy: Number.NaN, between: null };
      after.port2.postMessage(0);
    }
    requestAnimationFrame(loop);
  };
  requestAnimationFrame(loop);
  idling = true;
  await sleep(IDLE_SAMPLE_MS);
  idling = false;

  const started = await Promise.all(Array.from({ length: THREADS }, (_, k) =>
    command<ThreadSummary>({ type: "start_thread", agent: { type: "fake", tokens_per_second: TOKENS_PER_SECOND, chunk_chars: CHARS_PER_TOKEN, duration_ms: SECONDS * 1_000, tool_call_every_ms: TOOL_CALL_EVERY_MS }, message: `Streaming run, thread ${k + 1}` })));
  const ids = started.map((s) => s.thread_id);
  const held = (onlyShown ? ids.slice(0, 1) : ids).map((id) => holdThread(id).model);
  openThread(ids[0]!);
  shown = held[0]!.transcript;

  const charsOf = (k: number) => held[k]!.transcript.items.reduce((n, item) => n + (item.kind === "message" && item.role === "assistant" ? item.chars : 0), 0);
  const total = () => held.reduce((n, _m, k) => n + charsOf(k), 0);
  await waitFor("the first tokens", () => charsOf(0) > 0, START_TIMEOUT_MS);
  resetPerf();
  marks = snapshot();
  sampling = true;
  const t0 = performance.now();
  const chars0 = total();
  const wasHidden = document.hidden;
  // Every turn has ended and every model has applied its thread's last event.
  await waitFor("the streams to end", () => ids.every((id, k) => k >= held.length ? daemon().threads[id]?.state.type === "idle" : (() => {
    const summary = daemon().threads[id];
    return summary?.state.type === "idle" && held[k]!.transcript.turn === null && held[k]!.transcript.last !== null && held[k]!.transcript.last! >= (summary.last_event ?? 0);
  })()), END_TIMEOUT_MS);
  sampling = false;
  const seconds = (performance.now() - t0) / 1_000;
  const chars = total() - chars0;

  const sorted = [...frames].sort((x, y) => x - y);
  const refresh = quantile(sorted, 1 / 2);
  const over = frames.filter((f) => f > GATE_FRAME_MS).length;
  // WebKitGTK rounds rAF timestamps to whole ms, so an on-time 60 Hz frame reads 16 or 17: this
  // counts frames over 16.7 ms by more than that resolution.
  const overBeyondResolution = frames.filter((f) => f > GATE_FRAME_MS + TIMESTAMP_RESOLUTION_MS).length;
  const long = frames.filter((f) => f > refresh * DROPPED_FACTOR);
  const dropped = long.reduce((n, f) => n + Math.round(f / refresh) - 1, 0);
  const p99 = quantile(sorted, 99 / PERCENT);
  // Long frames beside the others: what the frame did, and what changed before it.
  const isLong = (k: number) => frames[k]! > refresh * DROPPED_FACTOR;
  const mean = (xs: number[]) => (xs.length ? round1(xs.reduce((a, b) => a + b, 0) / xs.length) : null);
  const describe = (ks: number[]) => ({
    frames: ks.length,
    busyMs: mean(ks.map((k) => work[k]?.busy ?? Number.NaN).filter((x) => !Number.isNaN(x))),
    flushMs: mean(ks.map((k) => work[k]?.between?.flushMs ?? 0)),
    withRowChangePct: round1((ks.filter((k) => (work[k]?.between?.structure ?? 0) > 0).length / Math.max(1, ks.length)) * PERCENT),
    withSummaryPct: round1((ks.filter((k) => (work[k]?.between?.summaries ?? 0) > 0).length / Math.max(1, ks.length)) * PERCENT),
    withMarkReadPct: round1((ks.filter((k) => (work[k]?.between?.markRead ?? 0) > 0).length / Math.max(1, ks.length)) * PERCENT),
  });
  const all = frames.map((_f, k) => k);
  const longKs = all.filter(isLong);
  const frameWork = {
    long: describe(longKs),
    // The frame before a long one: rendering it may be what overran.
    beforeLong: describe(longKs.map((k) => k - 1).filter((k) => k >= 0)),
    others: describe(all.filter((k) => !isLong(k))),
    withRowChange: describe(all.filter((k) => (work[k]?.between?.structure ?? 0) > 0)),
    longest: [...longKs].sort((a, b) => frames[b]! - frames[a]!).slice(0, 12).map((k) => ({
      k, ms: round1(frames[k]!), delay: round1(work[k]?.delay ?? Number.NaN), busy: round1(work[k]?.busy ?? Number.NaN),
      ...Object.fromEntries(Object.entries(work[k]?.between ?? {}).map(([n, x]) => [n, round1(x)])),
      prevBusy: round1(work[k - 1]?.busy ?? Number.NaN),
    })),
  };
  const scroller = document.querySelector<HTMLElement>('[data-atrium-part="transcript"]');
  await writeReport({
    kind: "stream",
    threads: THREADS, threadsHeld: held.length, tokensPerSecondEach: TOKENS_PER_SECOND, charsPerEvent: CHARS_PER_TOKEN, secondsAsked: SECONDS,
    seconds: round1(seconds),
    tokens: Math.round(chars / CHARS_PER_TOKEN),
    tokensPerSecond: Math.round(chars / CHARS_PER_TOKEN / seconds),
    timing: "requestAnimationFrame",
    frames: frames.length,
    fps: round1(frames.length / seconds),
    refreshMs: round1(refresh),
    frameMs: { p50: round1(refresh), p95: round1(quantile(sorted, 95 / PERCENT)), p99: round1(p99), max: round1(sorted[sorted.length - 1] ?? Number.NaN) },
    overGateFramePct: round1((over / Math.max(1, frames.length)) * PERCENT),
    overGateBeyondResolutionPct: round1((overBeyondResolution / Math.max(1, frames.length)) * PERCENT),
    longFrames: long.length,
    longFramesPct: round1((long.length / Math.max(1, frames.length)) * PERCENT),
    droppedFrames: dropped,
    gate: { frameMs: GATE_FRAME_MS, p99Ms: GATE_P99_MS, p99Within: p99 <= GATE_P99_MS },
    hidden: wasHidden || document.hidden, hiddenDuring,
    rowsRendered: document.querySelectorAll(".thread-row").length,
    tailGapPx: scroller ? Math.round(scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight) : null,
    loadedBytesVisibleThread: held[0]!.transcript.bytes,
    idleFrames: (() => {
      const s = [...idle].sort((x, y) => x - y);
      const r = quantile(s, 1 / 2);
      return { frames: idle.length, p50: round1(r), p95: round1(quantile(s, 95 / PERCENT)), p99: round1(quantile(s, 99 / PERCENT)), max: round1(s[s.length - 1] ?? Number.NaN), overGateFramePct: round1((idle.filter((f) => f > GATE_FRAME_MS).length / Math.max(1, idle.length)) * PERCENT), overGateBeyondResolutionPct: round1((idle.filter((f) => f > GATE_FRAME_MS + TIMESTAMP_RESOLUTION_MS).length / Math.max(1, idle.length)) * PERCENT), longFramesPct: round1((idle.filter((f) => f > r * DROPPED_FACTOR).length / Math.max(1, idle.length)) * PERCENT) };
    })(),
    frameWork,
    // Main-thread work while sampling: how many times, and total ms, per kind.
    work: Object.fromEntries(Object.keys(perf.counts).sort().map((k) => [k, { n: perf.counts[k], ms: Math.round(perf.ms[k] ?? 0) }])),
  });
}
