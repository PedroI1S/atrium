// The streaming run of contract §14 (`ATRIUM_BENCH=stream`, run by app/scripts/stream_bench.py
// against a real `atriumd` with the synthetic agent): 10 threads at 1,000 tokens/s each for
// 30 s, one token per event (4 characters, the hardest case, as in M0-07), a tool call every
// 4 s. The main window shows one thread and holds the models of all ten, so every event of
// every thread is delivered to this window and applied (M0-07's "multi" scenario).
//
// Frame times are `requestAnimationFrame` timestamp deltas: WebKitGTK gives a page no
// presentation timestamps. They don't see compositor or GPU drops.

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
const START_TIMEOUT_MS = 30_000;
const END_TIMEOUT_MS = 90_000;
/** Contract §14's gate values. */
const GATE_FRAME_MS = 16.7;
const GATE_P99_MS = 33;
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

const quantile = (sorted: number[], q: number): number => sorted[Math.min(sorted.length - 1, Math.round(q * (sorted.length - 1)))] ?? Number.NaN;
const TENTHS = 10;
const round1 = (x: number) => Math.round(x * TENTHS) / TENTHS;

export async function runStreamBench(): Promise<void> {
  await waitFor("the daemon", () => daemon().connection.type === "connected" && daemon().loaded, START_TIMEOUT_MS);

  const frames: number[] = [];
  let sampling = false;
  let previous = 0;
  let hiddenDuring = 0;
  document.addEventListener("visibilitychange", () => { if (sampling && document.hidden) hiddenDuring++; });
  const loop = (t: number) => {
    if (sampling) {
      if (previous) frames.push(t - previous);
      previous = t;
    }
    requestAnimationFrame(loop);
  };
  requestAnimationFrame(loop);

  const started = await Promise.all(Array.from({ length: THREADS }, (_, k) =>
    command<ThreadSummary>({ type: "start_thread", agent: { type: "fake", tokens_per_second: TOKENS_PER_SECOND, chunk_chars: CHARS_PER_TOKEN, duration_ms: SECONDS * 1_000, tool_call_every_ms: TOOL_CALL_EVERY_MS }, message: `Streaming run, thread ${k + 1}` })));
  const ids = started.map((s) => s.thread_id);
  const held = ids.map((id) => holdThread(id).model);
  openThread(ids[0]!);

  const charsOf = (k: number) => held[k]!.transcript.items.reduce((n, item) => n + (item.kind === "message" && item.role === "assistant" ? item.chars : 0), 0);
  const total = () => held.reduce((n, _m, k) => n + charsOf(k), 0);
  await waitFor("the first tokens", () => charsOf(0) > 0, START_TIMEOUT_MS);
  sampling = true;
  const t0 = performance.now();
  const chars0 = total();
  const wasHidden = document.hidden;
  // Every turn has ended and every model has applied its thread's last event.
  await waitFor("the streams to end", () => ids.every((id, k) => {
    const summary = daemon().threads[id];
    return summary?.state.type === "idle" && held[k]!.transcript.turn === null && held[k]!.transcript.last !== null && held[k]!.transcript.last! >= (summary.last_event ?? 0);
  }), END_TIMEOUT_MS);
  sampling = false;
  const seconds = (performance.now() - t0) / 1_000;
  const chars = total() - chars0;

  const sorted = [...frames].sort((x, y) => x - y);
  const refresh = quantile(sorted, 1 / 2);
  const over = frames.filter((f) => f > GATE_FRAME_MS).length;
  const long = frames.filter((f) => f > refresh * DROPPED_FACTOR);
  const dropped = long.reduce((n, f) => n + Math.round(f / refresh) - 1, 0);
  const p99 = quantile(sorted, 99 / PERCENT);
  const scroller = document.querySelector<HTMLElement>('[data-atrium-part="transcript"]');
  await writeReport({
    kind: "stream",
    threads: THREADS, tokensPerSecondEach: TOKENS_PER_SECOND, charsPerEvent: CHARS_PER_TOKEN, secondsAsked: SECONDS,
    seconds: round1(seconds),
    tokens: Math.round(chars / CHARS_PER_TOKEN),
    tokensPerSecond: Math.round(chars / CHARS_PER_TOKEN / seconds),
    timing: "requestAnimationFrame",
    frames: frames.length,
    fps: round1(frames.length / seconds),
    refreshMs: round1(refresh),
    frameMs: { p50: round1(refresh), p95: round1(quantile(sorted, 95 / PERCENT)), p99: round1(p99), max: round1(sorted[sorted.length - 1] ?? Number.NaN) },
    overGateFramePct: round1((over / Math.max(1, frames.length)) * PERCENT),
    longFrames: long.length,
    longFramesPct: round1((long.length / Math.max(1, frames.length)) * PERCENT),
    droppedFrames: dropped,
    gate: { frameMs: GATE_FRAME_MS, p99Ms: GATE_P99_MS, p99Within: p99 <= GATE_P99_MS },
    hidden: wasHidden || document.hidden, hiddenDuring,
    rowsRendered: document.querySelectorAll(".thread-row").length,
    tailGapPx: scroller ? Math.round(scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight) : null,
    loadedBytesVisibleThread: held[0]!.transcript.bytes,
  });
}
