// Measurement hooks. Everything here is inert unless the harness (or the palette) calls it.
// Method and commands are documented in docs/spikes/M0-06-ui-shell.md.

import { accessibilityOpaque, model, nativeError, settingsSettled } from "./windows";
import { batch } from "solid-js";
import { type StartupInfo, quitApp, writeReport } from "./platform";
import { floatingDefaultSizes, setBenchOverlay, setOuterSizes, setPaletteOpen, themeId } from "./state";

/** Wall clock in ms with sub-millisecond resolution, comparable with the Rust side and the shell script. */
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

/** Main-thread cost of the work the benchmark triggers each frame (JS + forced style/layout), in ms. */
export interface WorkStats {
  samples: number;
  meanMs: number;
  p95Ms: number;
  maxMs: number;
}
const work = (xs: number[]): WorkStats => {
  const sorted = [...xs].sort((a, b) => a - b);
  return { samples: xs.length, meanMs: xs.reduce((s, v) => s + v, 0) / (xs.length || 1), p95Ms: pct(sorted, 0.95), maxMs: sorted[sorted.length - 1] ?? 0 };
};

export interface FrameStats {
  /**
   * Main-thread time from the start of each rAF callback to the next task: the rendering update
   * (style, layout, paint recording, ResizeObserver callbacks such as the virtualizer's row
   * measurements) plus the callback itself. Compositing and GPU work are off-thread and not included.
   */
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

const pct = (sorted: number[], p: number): number => sorted[Math.min(sorted.length - 1, Math.floor((sorted.length - 1) * p))] ?? 0;

export function summarize(dts: number[], render: number[] = []): FrameStats {
  const sorted = [...dts].sort((a, b) => a - b);
  const total = dts.reduce((s, v) => s + v, 0);
  return {
    renderWork: work(render),
    frames: dts.length,
    seconds: total / 1000,
    fps: dts.length / (total / 1000),
    meanMs: total / dts.length,
    p50Ms: pct(sorted, 0.5),
    p95Ms: pct(sorted, 0.95),
    p99Ms: pct(sorted, 0.99),
    maxMs: sorted[sorted.length - 1] ?? 0,
    over25ms: dts.filter((d) => d > 25).length,
    over16_7ms: dts.filter((d) => d > 16.7).length,
  };
}

/** Samples requestAnimationFrame deltas for `ms`, calling `step(elapsedMs, dtMs)` every frame. */
export function sampleFrames(ms: number, step?: (elapsed: number, dt: number) => void): Promise<FrameStats> {
  return new Promise((resolve) => {
    const dts: number[] = [];
    const render: number[] = [];
    let last = 0;
    let start = 0;
    let t0 = 0;
    // A task posted from the rAF callback runs after that frame's rendering update, so its delay
    // is the main-thread cost of the update.
    const ch = new MessageChannel();
    ch.port1.onmessage = () => render.push(performance.now() - t0);
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
      else
        setTimeout(() => {
          ch.port1.close();
          resolve(summarize(dts, render));
        }, 50);
    };
    requestAnimationFrame(tick);
  });
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * Scrolls the 5,000-message transcript upwards at a constant speed (px/s). Besides frame deltas it
 * times the scroll handler (virtualizer + Solid DOM updates) plus a forced style/layout, which is
 * the main-thread cost per frame and shows headroom even when rAF is pinned at 60 Hz.
 */
export async function benchScroll(pxPerSecond: number, ms = 3000): Promise<FrameStats & { pxPerSecond: number; distancePx: number; handler: WorkStats }> {
  if (!scroller) throw new Error("no transcript registered");
  const el = scroller;
  el.scrollTop = el.scrollHeight;
  await sleep(200);
  const startTop = el.scrollTop;
  const costs: number[] = [];
  let t0 = 0;
  const begin = () => {
    t0 = performance.now();
  };
  const end = () => {
    void el.offsetHeight; // force style + layout for the rows the handler just rendered
    costs.push(performance.now() - t0);
  };
  window.addEventListener("scroll", begin, true);
  el.addEventListener("scroll", end);
  const stats = await sampleFrames(ms, (elapsed) => {
    el.scrollTop = Math.max(0, startTop - (pxPerSecond * elapsed) / 1000);
  });
  window.removeEventListener("scroll", begin, true);
  el.removeEventListener("scroll", end);
  return { ...stats, pxPerSecond, distancePx: Math.round(startTop - el.scrollTop), handler: work(costs) };
}

/** Drives the panel sizes every frame, the same state path a pointer drag takes. */
export async function benchResize(ms = 3000): Promise<FrameStats & { step: WorkStats }> {
  const base = floatingDefaultSizes();
  const costs: number[] = [];
  const stats = await sampleFrames(ms, (elapsed) => {
    const t0 = performance.now();
    const k = (Math.sin((elapsed / 1000) * Math.PI * 2 * 0.8) + 1) / 2; // 0.8 Hz sweep
    const sidebar = 0.14 + 0.14 * k;
    const right = 0.2 + 0.2 * (1 - k);
    batch(() => setOuterSizes([sidebar, 1 - sidebar - right, right]));
    void document.body.offsetHeight; // force style + layout of every panel
    costs.push(performance.now() - t0);
  });
  setOuterSizes([...base]);
  return { ...stats, step: work(costs) };
}

const fmt = (s: FrameStats) => `${s.fps.toFixed(1)} fps · p50 ${s.p50Ms.toFixed(1)} · p95 ${s.p95Ms.toFixed(1)} · p99 ${s.p99Ms.toFixed(1)} · max ${s.maxMs.toFixed(1)} ms · >16.7 ms: ${s.over16_7ms}/${s.frames}`;

/** Palette action: run both frame benchmarks and show the numbers on screen. */
export async function runFrameBenchesInteractive(): Promise<void> {
  setBenchOverlay("Running frame benchmarks (hands off the window)...");
  const reading = await benchScroll(1500);
  const fling = await benchScroll(20000);
  const resize = await benchResize();
  setBenchOverlay(`scroll 1.5k px/s: ${fmt(reading)}\nscroll 20k px/s: ${fmt(fling)}\npane resize:     ${fmt(resize)}`);
  window.setTimeout(() => setBenchOverlay(null), 12000);
}

/** Cold-start probe plus (in `frames` mode) the frame benchmarks. Reports through Rust and exits. */
export async function runHarness(info: StartupInfo): Promise<void> {
  const raf2 = () => new Promise<void>((r) => requestAnimationFrame(() => requestAnimationFrame(() => r())));
  await raf2();
  markOnce("firstPaintRaf");
  const fcp = performance.getEntriesByName("first-contentful-paint")[0];
  if (fcp) marks.firstContentfulPaint = performance.timeOrigin + fcp.startTime;

  // Probe 1: a keystroke in the composer. Synthetic (dispatched from script), so it measures the
  // app's readiness to handle input, not the OS delivery path.
  const composer = document.getElementById("composer-primary") as HTMLTextAreaElement | null;
  if (!composer) throw new Error("TTI probe has no primary composer");
  composer.focus();
  const t0 = epochNow();
  composer?.dispatchEvent(new KeyboardEvent("keydown", { key: "a", bubbles: true }));
  marks.keystrokeDispatched = t0;
  // Probe 2: open the palette with Cmd+K and wait until it is in the DOM and has painted.
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", metaKey: true, bubbles: true }));
  const t1 = epochNow();
  for (let i = 0; i < 200 && !document.querySelector(".palette"); i++) await sleep(5);
  if (!document.querySelector(".palette")) throw new Error("Palette input was not handled");
  await raf2();
  marks.paletteReady = epochNow();
  marks.paletteDispatched = t1;
  setPaletteOpen(false);

  await settingsSettled();
  const report: Record<string, unknown> = {
    kind: "startup",
    configuration: { windows: info.windows, opacity: info.opacity, settings: model().settings, readyWindows: info.windows,
      applied: { nativeAlpha: Number(document.documentElement.dataset.nativeAlpha ?? 1),
        chromeAlpha: Number(getComputedStyle(document.documentElement).getPropertyValue("--window-alpha-chrome")),
        contentAlpha: Number(getComputedStyle(document.documentElement).getPropertyValue("--window-alpha-content")),
        vibrancy: document.documentElement.dataset.vibrancy, accessibilityOpaque: accessibilityOpaque(), error: nativeError() } },
    theme: themeId(),
    marks,
    mainEpochMs: info.mainEpochMs,
    userAgent: navigator.userAgent,
    dpr: window.devicePixelRatio,
    viewport: [window.innerWidth, window.innerHeight],
  };
  if (info.benchMode !== "frames") {
    await writeReport(report);
    return;
  }
  await sleep(500);
  const idle = await sampleFrames(1500);
  report.idleFrames = idle;
  report.scrollReading = await benchScroll(1500);
  report.scrollFling = await benchScroll(20000);
  report.resize = await benchResize();
  report.themeSwapToNextFrameMs = []; // Pinned configuration: do not change modes mid-run.
  report.kind = "frames";
  await writeReport(report);
  await quitApp();
}
