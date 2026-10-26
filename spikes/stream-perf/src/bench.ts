// The measurement harness. Launched by scripts/bench.py through `ATRIUM_BENCH=<scenario json>`; the
// app then runs one scenario in its real UI, writes a JSON report and exits (or holds, for memory
// sampling by the script). Method and commands: docs/spikes/M0-07-stream-perf.md.

import { setActivePipeline } from "./App";
import { FrameSampler, epochNow, summarizeFrames, summarizeValues } from "./metrics";
import { bridgeStats, bridgeStop, quitApp, type StartupInfo, writeProgress, writeReport } from "./platform";
import { type RunConfig, DEFAULT_RUN, startSession } from "./session";
import { typing } from "./typing";

export interface Scenario extends RunConfig {
  name: string;
  /** `synthetic`: the page dispatches key + input events; `external`: the script injects OS key events. */
  typing: "none" | "synthetic" | "external";
  typingIntervalMs: number;
  /** Milliseconds after the stream starts before typing begins (typing also runs idle before it). */
  typingStartMs: number;
  /** Idle typing window before the stream starts (ms); 0 skips. */
  idleTypingMs: number;
  baselineMs: number;
  /** `ended`: wait for every session to end; `time`: stop `durationMs + 3 s` after the start (the daemon folds a very long history when a session closes, which takes minutes). */
  endMode: "ended" | "time";
  /** Keep the app open after the report so the script can sample memory. */
  hold: boolean;
}

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
const raf2 = () => new Promise<void>((r) => requestAnimationFrame(() => requestAnimationFrame(() => r())));
const r2 = (x: number) => Math.round(x * 100) / 100;

function round<T>(o: T): T {
  if (typeof o === "number") return r2(o) as T;
  if (Array.isArray(o)) return o.map(round) as T;
  if (o && typeof o === "object") return Object.fromEntries(Object.entries(o).map(([k, v]) => [k, round(v)])) as T;
  return o;
}

function medianRefresh(dts: number[]): number {
  const m = summarizeValues(dts).p50;
  // Snap to the common display intervals so a noisy baseline cannot move the drop threshold.
  const known = [8.33, 11.11, 16.67, 33.33];
  return known.reduce((best, k) => (Math.abs(k - m) < Math.abs(best - m) ? k : best), known[0]!);
}

/**
 * Types into the composer on an absolute, jittery schedule until `stop()` is called. Each keystroke has a
 * deadline (previous deadline + interval); the event is created when its timer fires, which a busy main
 * thread delays. Latencies are measured from the deadline, and the lateness is recorded, so a stalled
 * main thread cannot hide itself by delaying the event's creation. A late timer fires the following
 * keystrokes back to back (they are never skipped), so the count of keystrokes sent depends only on
 * elapsed time. These are still synthetic events: they bypass OS and WebKit input routing.
 */
function startTypist(intervalMs: number) {
  const ta = document.querySelector("textarea") as HTMLTextAreaElement;
  ta.focus();
  const text = "the quick brown fox jumps over the lazy dog while the agent streams tokens ";
  let n = 0;
  let seed = 12345;
  let alive = true;
  const jitter = () => {
    seed = (seed * 1664525 + 1013904223) >>> 0;
    return intervalMs * (0.6 + 0.8 * (seed / 0xffffffff));
  };
  let deadline = performance.now() + jitter();
  const step = () => {
    if (!alive) return;
    const now = performance.now();
    typing.lateness.push(Math.max(0, now - deadline));
    const ch = text[n % text.length]!;
    n++;
    typing.nextOrigin = deadline;
    ta.dispatchEvent(new KeyboardEvent("keydown", { key: ch, bubbles: true }));
    ta.value += ch;
    ta.dispatchEvent(new InputEvent("input", { data: ch, inputType: "insertText", bubbles: true }));
    ta.dispatchEvent(new KeyboardEvent("keyup", { key: ch, bubbles: true }));
    deadline += jitter();
    setTimeout(step, Math.max(0, deadline - performance.now()));
  };
  setTimeout(step, Math.max(0, deadline - performance.now()));
  return {
    stop: () => {
      alive = false;
    },
    typed: () => n,
    textarea: ta,
  };
}

export async function runScenario(sc: Partial<Scenario>, info: StartupInfo): Promise<void> {
  const full: Scenario = { ...DEFAULT_RUN, name: "scenario", typing: "none", typingIntervalMs: 120, typingStartMs: 2000, idleTypingMs: 0, baselineMs: 1500, endMode: "ended", hold: false, ...sc };
  // Evidence for "the window was hidden or occluded": rAF stops then, which is an environment
  // artifact, not a result. The script discards and repeats such runs and keeps the evidence.
  const visibility: { t: number; state: string; focus: boolean }[] = [];
  const t0 = performance.now();
  document.addEventListener("visibilitychange", () => visibility.push({ t: performance.now() - t0, state: document.visibilityState, focus: document.hasFocus() }));
  writeProgress({ stage: "boot", hidden: document.hidden });
  try {
    await raf2();
    await sleep(300);

    // 1. Idle frame baseline (and idle typing, so streaming typing has a reference).
    const base = new FrameSampler();
    base.start();
    let idleTyping: ReturnType<typeof typing.summary> | null = null;
    if (full.typing === "synthetic" && full.idleTypingMs > 0) {
      const t = startTypist(full.typingIntervalMs);
      await sleep(full.idleTypingMs);
      t.stop();
      await sleep(300);
      idleTyping = typing.summary();
      t.textarea.value = "";
      t.textarea.dispatchEvent(new InputEvent("input", { bubbles: true }));
      typing.reset();
    } else {
      await sleep(full.baselineMs);
    }
    base.stop();
    const refreshMs = medianRefresh(base.dts);

    // 2. The run.
    const sampler = new FrameSampler();
    const streamStartEpoch = epochNow();
    const { pipeline } = await startSession(full, info);
    setActivePipeline(pipeline);
    await raf2();
    sampler.start();
    const startedAt = performance.now();
    const streamStartRel = startedAt - t0;
    let typist: ReturnType<typeof startTypist> | null = null;
    if (full.typing === "synthetic") setTimeout(() => (typist = startTypist(full.typingIntervalMs)), full.typingStartMs);

    const deadline = startedAt + full.durationMs + 90_000;
    let lastProgress = 0;
    while (performance.now() < deadline) {
      if (performance.now() - lastProgress > 10_000) {
        lastProgress = performance.now();
        writeProgress({ t: (lastProgress - startedAt) / 1000, applied: pipeline.eventsApplied, received: pipeline.eventsReceived, queued: pipeline.maxQueue, frames: sampler.dts.length, hidden: document.hidden, rows: pipeline.threads[0]?.rows.length ?? 0 });
      }
      if (pipeline.threads.length > 0 && pipeline.threads.every((t) => t.ended)) break;
      if (full.endMode === "time" && performance.now() >= startedAt + full.durationMs + 3000) break;
      await sleep(100);
    }
    const timedOut = full.endMode === "ended" && !pipeline.threads.every((t) => t.ended);
    // Let the last frames settle and the acknowledgement go out.
    while (!pipeline.idle()) await sleep(20);
    await raf2();
    await raf2();
    pipeline.sendAck();
    const streamEndEpoch = epochNow();
    sampler.stop();
    (typist as ReturnType<typeof startTypist> | null)?.stop();
    await sleep(300);

    // 3. Report.
    const bridge = await bridgeStats();
    const scroller = document.querySelector(".transcript") as HTMLElement | null;
    const ty = typist as ReturnType<typeof startTypist> | null;
    const visible = pipeline.visible[0]();
    const report = round({
      scenario: full,
      timedOut,
      host: { userAgent: navigator.userAgent, dpr: devicePixelRatio, viewport: [innerWidth, innerHeight] },
      refreshMs,
      visibility: { streamStartRelMs: streamStartRel, streamEndRelMs: performance.now() - t0, changes: visibility, hiddenAtEnd: document.hidden, focusAtEnd: document.hasFocus() },
      baseline: summarizeFrames(base.dts, refreshMs),
      frames: summarizeFrames(sampler.dts, refreshMs),
      renderWork: summarizeValues(sampler.work),
      applyMs: summarizeValues(pipeline.applyMs),
      latency: pipeline.latency.summary(),
      ipc: pipeline.ipc.summary(),
      queueWait: pipeline.queueWait.summary(),
      commit: pipeline.commit.summary(),
      decode: { totalMs: pipeline.decodeMs, perBatchMs: pipeline.decodeMs / Math.max(1, pipeline.decodeBatches), batches: pipeline.decodeBatches },
      pipeline: {
        batchesReceived: pipeline.batchesReceived,
        eventsReceived: pipeline.eventsReceived,
        eventsApplied: pipeline.eventsApplied,
        duplicatesDropped: pipeline.duplicatesDropped,
        framesWithWork: pipeline.framesWithWork,
        maxQueue: pipeline.maxQueue,
        seqGaps: pipeline.seqGaps,
        streamSeconds: (streamEndEpoch - streamStartEpoch) / 1000,
      },
      transcript: {
        visibleThread: visible,
        visibleRows: pipeline.threads[visible]?.rows.length ?? 0,
        visibleChars: pipeline.threads[visible]?.chars ?? 0,
        totalChars: pipeline.threads.reduce((s, t) => s + t.chars, 0),
        totalTokensApprox: pipeline.threads.reduce((s, t) => s + t.chars, 0) / 4,
        events: pipeline.threads.map((t) => t.events),
        completedMismatch: pipeline.threads.reduce((s, t) => s + t.completedMismatch, 0),
        domNodes: document.querySelectorAll("*").length,
        followingTail: scroller ? scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight : null,
      },
      typing: {
        idle: idleTyping,
        streaming: typing.summary(),
        typed: ty?.typed() ?? 0,
        textareaLength: ty?.textarea.value.length ?? null,
      },
      bridge,
      timeline: { streamStartEpochMs: streamStartEpoch, streamEndEpochMs: streamEndEpoch, reportEpochMs: epochNow() },
    });
    await writeReport(report);
    await bridgeStop();
  } catch (e) {
    await writeReport({ error: String(e), stack: (e as Error)?.stack ?? null });
  }
  if (!full.hold) await quitApp();
}

/** Time `fn` over enough iterations to span ~`targetMs`; returns microseconds per call. */
function timeIt(fn: () => void, targetMs = 400): number {
  for (let i = 0; i < 20; i++) fn();
  let n = 50;
  for (;;) {
    const t0 = performance.now();
    for (let i = 0; i < n; i++) fn();
    const dt = performance.now() - t0;
    if (dt >= targetMs) return (dt * 1000) / n;
    n = Math.ceil(n * Math.max(2, (targetMs * 1.2) / Math.max(dt, 1)));
  }
}

/** In-webview microbenchmarks: decoders and the markdown parser on JavaScriptCore. */
export async function runMicro(): Promise<void> {
  const { ENCODINGS, decodeBatch, messageFromBytes } = await import("./wire");
  const { encodeSample } = await import("./platform");
  const { BlockSplitter } = await import("./blocks");
  const { renderBlock, renderOpen } = await import("./markdown");
  const decode: unknown[] = [];
  for (const chars of [4, 40, 200]) {
    for (const events of [1, 16, 160]) {
      for (const enc of ENCODINGS) {
        const bytes = await encodeSample(enc, events, chars);
        const message = messageFromBytes(enc, bytes);
        const us = timeIt(() => {
          // JSON encodings arrive pre-parsed from Tauri: include the parse, from a fresh string each time.
          const m = enc === "json" || enc === "json-slim" ? messageFromBytes(enc, bytes) : message;
          decodeBatch(enc, m, () => 0);
        });
        decode.push({ chars, events, encoding: enc, bytes: bytes.byteLength, usPerBatch: us, nsPerEvent: (us * 1000) / events });
      }
    }
  }
  // Markdown: a closed block of ~600 characters, and the open-block re-parse at several sizes.
  const sample = (await import("./micro-sample")).MARKDOWN_SAMPLE;
  const closed = sample.slice(0, 600);
  const parse = {
    closedBlock600UsPerParse: timeIt(() => renderBlock(closed)),
    openBlockUsPerParse: [100, 300, 600, 1200, 2400, 4800].map((n) => ({ chars: n, us: timeIt(() => renderOpen(sample.slice(0, n), false)) })),
    openFenceUsPerRender: [1000, 10000, 100000].map((n) => ({ chars: n, us: timeIt(() => renderOpen("```\n" + "let x = 1;\n".repeat(n / 11), true)) })),
    splitterNsPerChar: (() => {
      const text = sample.repeat(50);
      const us = timeIt(() => {
        const s = new BlockSplitter();
        for (let i = 0; i < text.length; i += 4) s.feed(text.slice(i, i + 4), () => {});
      });
      return (us * 1000) / text.length;
    })(),
  };
  await writeReport(round({ mode: "micro", host: { userAgent: navigator.userAgent }, decode, parse }));
  await quitApp();
}
