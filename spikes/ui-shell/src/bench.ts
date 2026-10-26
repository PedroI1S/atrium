// Measurement hooks. Everything here is inert unless the harness (or the palette) calls it.
// Method and commands: docs/spikes/M0-06b-ui-shell-layout-b.md ("Measurements").

import { invoke } from "@tauri-apps/api/core";
import { batch } from "solid-js";
import { type StartupInfo, inTauri, quitApp, writeReport } from "./platform";
import { setBenchOverlay, setPaletteOpen, setPanelWidth, setSidebarWidth } from "./state";

/** Wall clock in ms with sub-millisecond resolution, comparable with the Rust side and the harness. */
export const epochNow = (): number => performance.timeOrigin + performance.now();

declare global { interface Window { __atriumJsStart?: number } }
export const marks: Record<string, number> = { jsStart: window.__atriumJsStart ?? epochNow() };
export const markOnce = (name: string): void => {
  if (marks[name] === undefined) marks[name] = epochNow();
};
export const markFirstInput = (_kind: string): void => markOnce("firstInputHandled");

let scroller: HTMLElement | null = null;
export const registerScroller = (el: HTMLElement): void => {
  scroller = el;
};

// -- failure reporting (review #2): every failure becomes a report with a stage ------------

let stage = "load";
export const setStage = (s: string): void => { stage = s; };

/** Writes an error report (the harness fails fast on it) and, in a harness run, exits. */
export async function reportFailure(e: unknown, info?: StartupInfo | null): Promise<void> {
  const message = e instanceof Error ? `${e.name}: ${e.message}` : String(e);
  console.error(`[atrium] failed at ${stage}:`, e);
  let windows: unknown = null;
  try { if (inTauri) windows = await invoke("ready_states"); } catch { /* best effort */ }
  try {
    await writeReport({ kind: "error", stage, message, stack: e instanceof Error ? e.stack : undefined, windows, marks });
  } catch (w) {
    console.error("[atrium] could not write the error report", w);
  }
  if (info?.benchMode) await quitApp();
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
/** rAF stops in occluded or background windows; a stuck wait names itself instead of hanging. */
const RAF_TIMEOUT_MS = 3000;
function raf2(what: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error(`requestAnimationFrame did not fire within ${RAF_TIMEOUT_MS} ms (${what}); is the window occluded or in the background?`)), RAF_TIMEOUT_MS);
    requestAnimationFrame(() => requestAnimationFrame(() => { clearTimeout(t); resolve(); }));
  });
}

// -- frame statistics --------------------------------------------------------------------

export interface WorkStats { samples: number; meanMs: number; p95Ms: number; maxMs: number }
const pct = (sorted: number[], p: number): number => sorted[Math.min(sorted.length - 1, Math.floor((sorted.length - 1) * p))] ?? 0;
const P50 = 0.5;
const P95 = 0.95;
const P99 = 0.99;
const work = (xs: number[]): WorkStats => {
  const sorted = [...xs].sort((a, b) => a - b);
  return { samples: xs.length, meanMs: xs.reduce((s, v) => s + v, 0) / (xs.length || 1), p95Ms: pct(sorted, P95), maxMs: sorted[sorted.length - 1] ?? 0 };
};

export interface FrameStats {
  /** Main-thread time from each rAF callback to the next task: the rendering update plus the callback. */
  renderWork: WorkStats;
  frames: number;
  seconds: number;
  fps: number;
  meanMs: number;
  p50Ms: number;
  p95Ms: number;
  p99Ms: number;
  maxMs: number;
  /** Frames longer than 1.5x the 60 Hz budget (25 ms) and than 2x the 120 Hz budget (16.7 ms). */
  over25ms: number;
  over16_7ms: number;
}
const LONG_60HZ_MS = 25;
const LONG_120HZ_MS = 16.7;
const MS_PER_S = 1000;

export function summarize(dts: number[], render: number[] = []): FrameStats {
  const sorted = [...dts].sort((a, b) => a - b);
  const total = dts.reduce((s, v) => s + v, 0);
  return {
    renderWork: work(render),
    frames: dts.length,
    seconds: total / MS_PER_S,
    fps: dts.length / (total / MS_PER_S),
    meanMs: total / dts.length,
    p50Ms: pct(sorted, P50),
    p95Ms: pct(sorted, P95),
    p99Ms: pct(sorted, P99),
    maxMs: sorted[sorted.length - 1] ?? 0,
    over25ms: dts.filter((d) => d > LONG_60HZ_MS).length,
    over16_7ms: dts.filter((d) => d > LONG_120HZ_MS).length,
  };
}

/** Samples requestAnimationFrame deltas for `ms`, calling `step(elapsedMs, dtMs)` every frame. */
export function sampleFrames(ms: number, step?: (elapsed: number, dt: number) => void): Promise<FrameStats> {
  return new Promise((resolve, reject) => {
    const dts: number[] = [];
    const render: number[] = [];
    let last = 0;
    let start = 0;
    let t0 = 0;
    // A task posted from the rAF callback runs after that frame's rendering update, so its
    // delay is the main-thread cost of the update.
    const ch = new MessageChannel();
    ch.port1.onmessage = () => render.push(performance.now() - t0);
    let watchdog = setTimeout(() => reject(new Error("rAF stopped during a frame benchmark")), ms + RAF_TIMEOUT_MS);
    const tick = (now: number) => {
      t0 = performance.now();
      ch.port2.postMessage(0);
      if (!start) {
        start = now;
        last = now;
      } else {
        const dt = now - last;
        last = now;
        dts.push(dt);
        step?.(now - start, dt);
      }
      if (now - start < ms) requestAnimationFrame(tick);
      else {
        clearTimeout(watchdog);
        watchdog = setTimeout(() => {
          ch.port1.close();
          resolve(summarize(dts, render));
        }, 50);
      }
    };
    requestAnimationFrame(tick);
  });
}

const BENCH_MS = 3000;
const SETTLE_MS = 200;

/**
 * Scrolls the 5,000-message transcript upwards at a constant speed (px/s). Besides frame deltas
 * it times the scroll handler (virtualizer + Solid DOM updates) plus a forced style/layout.
 */
export async function benchScroll(pxPerSecond: number, ms = BENCH_MS): Promise<FrameStats & { pxPerSecond: number; distancePx: number; handler: WorkStats }> {
  if (!scroller) throw new Error("no transcript registered");
  const el = scroller;
  el.scrollTop = el.scrollHeight;
  await sleep(SETTLE_MS);
  const startTop = el.scrollTop;
  const costs: number[] = [];
  let t0 = 0;
  const begin = () => { t0 = performance.now(); };
  const end = () => {
    void el.offsetHeight; // force style + layout for the rows the handler just rendered
    costs.push(performance.now() - t0);
  };
  window.addEventListener("scroll", begin, true);
  el.addEventListener("scroll", end);
  try {
    const stats = await sampleFrames(ms, (elapsed) => {
      el.scrollTop = Math.max(0, startTop - (pxPerSecond * elapsed) / MS_PER_S);
    });
    return { ...stats, pxPerSecond, distancePx: Math.round(startTop - el.scrollTop), handler: work(costs) };
  } finally {
    window.removeEventListener("scroll", begin, true);
    el.removeEventListener("scroll", end);
  }
}

const SWEEP_HZ = 0.8;
const tokenPx = (name: string) => Number.parseFloat(getComputedStyle(document.documentElement).getPropertyValue(name));

/** Sweeps the sidebar and panel widths every frame (the state path a pointer drag takes). */
export async function benchResize(ms = BENCH_MS): Promise<FrameStats & { step: WorkStats }> {
  const [sMin, sMax] = [tokenPx("--sidebar-min-width"), tokenPx("--sidebar-width")];
  const [pMin, pMax] = [tokenPx("--panel-min-width"), tokenPx("--panel-width")];
  const costs: number[] = [];
  try {
    return { ...(await sampleFrames(ms, (elapsed) => {
      const t0 = performance.now();
      const k = (Math.sin((elapsed / MS_PER_S) * Math.PI * 2 * SWEEP_HZ) + 1) / 2;
      batch(() => {
        setSidebarWidth(Math.round(sMin + (sMax - sMin) * k));
        setPanelWidth(Math.round(pMin + (pMax - pMin) * (1 - k)));
      });
      void document.body.offsetHeight; // force style + layout of every column
      costs.push(performance.now() - t0);
    })), step: work(costs) };
  } finally {
    batch(() => { setSidebarWidth(null); setPanelWidth(null); });
  }
}

const SCROLL_READING = 1500;
const SCROLL_FLING = 20000;
const OVERLAY_MS = 12000;
const fmt = (s: FrameStats) => `${s.fps.toFixed(1)} fps · p50 ${s.p50Ms.toFixed(1)} · p95 ${s.p95Ms.toFixed(1)} · p99 ${s.p99Ms.toFixed(1)} · max ${s.maxMs.toFixed(1)} ms · over ${LONG_120HZ_MS} ms: ${s.over16_7ms}/${s.frames}`;

/** Palette action: run the frame benchmarks and show the numbers on screen. */
export async function runFrameBenchesInteractive(): Promise<void> {
  setBenchOverlay("Running frame benchmarks (hands off the window)...");
  const reading = await benchScroll(SCROLL_READING);
  const fling = await benchScroll(SCROLL_FLING);
  const resize = await benchResize();
  setBenchOverlay(`scroll 1.5k px/s: ${fmt(reading)}\nscroll 20k px/s: ${fmt(fling)}\ncolumn resize:   ${fmt(resize)}`);
  setTimeout(() => setBenchOverlay(null), OVERLAY_MS);
}

const PALETTE_POLL_MS = 5;
const PALETTE_POLLS = 400;

/**
 * Time-to-interactive probe, run right after mount and before any wait on other windows or on
 * native calls (review #5). Probe 1: a keystroke in the composer (synthetic: it measures the
 * app's readiness to handle input, not the OS delivery path). Probe 2: ⌘K opens the palette and
 * it has painted.
 */
export async function runProbe(): Promise<void> {
  const composer = document.getElementById("composer-primary") as HTMLTextAreaElement | null;
  if (!composer) throw new Error("TTI probe has no primary composer");
  composer.focus();
  marks.keystrokeDispatched = epochNow();
  composer.dispatchEvent(new KeyboardEvent("keydown", { key: "a", bubbles: true }));
  marks.paletteDispatched = epochNow();
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", metaKey: true, bubbles: true }));
  for (let i = 0; i < PALETTE_POLLS && !document.querySelector(".palette"); i++) await sleep(PALETTE_POLL_MS);
  if (!document.querySelector(".palette")) throw new Error("Palette input was not handled");
  await raf2("palette paint");
  marks.paletteReady = epochNow();
  setPaletteOpen(false);
  const fcp = performance.getEntriesByName("first-contentful-paint")[0];
  if (fcp) marks.firstContentfulPaint = performance.timeOrigin + fcp.startTime;
}

const GATE_TIMEOUT_MS = 20000;
const RAF_REPORT_GRACE_MS = 300;
const GATE_POLL_MS = 25;

/** Waits until every expected window has mounted and reported its applied appearance. */
async function waitForWindows(expected: number): Promise<Record<string, unknown>> {
  let states: Record<string, unknown> = {};
  const until = performance.now() + GATE_TIMEOUT_MS;
  while (performance.now() < until) {
    states = await invoke<Record<string, unknown>>("ready_states");
    if (Object.keys(states).length >= expected) return states;
    await sleep(GATE_POLL_MS);
  }
  throw new Error(`Expected ${expected} ready windows within ${GATE_TIMEOUT_MS} ms; got ${Object.keys(states).length}: ${JSON.stringify(Object.keys(states))}`);
}

const IDLE_SAMPLE_MS = 1500;
const BEFORE_FRAMES_MS = 500;

/** Startup report (and, in `frames` mode, the frame benchmarks); reports through Rust. */
export async function runHarness(info: StartupInfo): Promise<void> {
  setStage("first paint");
  for (let waited = 0; marks.firstPaint === undefined; waited += GATE_POLL_MS) {
    if (waited > RAF_TIMEOUT_MS) throw new Error("the main window never painted (no rAF after mount)");
    await sleep(GATE_POLL_MS);
  }
  setStage("gate");
  await waitForWindows(info.windows);
  marks.allWindowsReady = epochNow();
  // Give every window a moment to report whether rAF fires for it (see index.tsx).
  await sleep(RAF_REPORT_GRACE_MS);
  const windows = await invoke<Record<string, unknown>>("ready_states");
  const report: Record<string, unknown> = {
    kind: "startup",
    configuration: { windows: info.windows, opacity: info.opacity, readyWindows: Object.keys(windows).length, perWindow: windows },
    marks,
    mainEpochMs: info.mainEpochMs,
    userAgent: navigator.userAgent,
    dpr: window.devicePixelRatio,
    viewport: [window.innerWidth, window.innerHeight],
  };
  if (info.benchMode !== "frames") {
    setStage("report");
    await writeReport(report);
    return;
  }
  await sleep(BEFORE_FRAMES_MS);
  setStage("frames: idle");
  report.idleFrames = await sampleFrames(IDLE_SAMPLE_MS);
  setStage("frames: scroll 1.5k");
  report.scrollReading = await benchScroll(SCROLL_READING);
  setStage("frames: scroll 20k");
  report.scrollFling = await benchScroll(SCROLL_FLING);
  setStage("frames: resize");
  report.resize = await benchResize();
  report.kind = "frames";
  setStage("report");
  await writeReport(report);
  await quitApp();
}
