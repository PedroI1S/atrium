// In-app measurement harness, run when the app starts with ATRIUM_BENCH=screencast.
// It drives the real pane (Tauri webview, IPC channel, canvas) and reports to the backend,
// which adds Chromium CPU and RSS and writes the JSON to ATRIUM_BENCH_OUT.
//
//   ATRIUM_BENCH=screencast ATRIUM_BENCH_OUT=/tmp/bench.json <app binary>
//
// Phases: idle (no screencast) -> static page with screencast -> animation streaming ->
// click-to-frame latency -> switching to the second context.

import { invoke } from "@tauri-apps/api/core";
import type { StartInfo, StartupInfo } from "./api";
import type { DrawnFrame, PaneController } from "./pane/controller";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function percentile(sorted: number[], p: number): number {
  if (sorted.length === 0) return NaN;
  return sorted[Math.round((sorted.length - 1) * p)]!;
}

function dist(values: number[]) {
  const v = [...values].sort((a, b) => a - b);
  return {
    n: v.length,
    min: v[0] ?? NaN,
    p50: percentile(v, 0.5),
    p95: percentile(v, 0.95),
    max: v[v.length - 1] ?? NaN,
    mean: v.reduce((a, b) => a + b, 0) / Math.max(1, v.length),
  };
}

async function sampled<T>(label: string, run: () => Promise<T>): Promise<{ result: T; cpuRss: unknown }> {
  await invoke("bench_sampler_start", { label });
  const result = await run();
  const cpuRss = await invoke("bench_sampler_stop", { label });
  return { result, cpuRss };
}

export async function runBench(
  pane: PaneController,
  cfg: StartupInfo,
  info: StartInfo,
  takeOver: (on: boolean) => Promise<void>,
): Promise<void> {
  const base = `http://127.0.0.1:${info.fixturePort}`;
  const report: Record<string, unknown> = {};
  try {
    await takeOver(true);
    await sleep(800); // let the first resize and frames settle: startup work, not the metric
    const rect = pane.canvas.getBoundingClientRect();
    report.environment = {
      userAgent: navigator.userAgent,
      devicePixelRatio: window.devicePixelRatio,
      pane: {
        cssWidth: Math.round(rect.width),
        cssHeight: Math.round(rect.height),
        canvasPixels: [pane.canvas.width, pane.canvas.height],
        deviceScaleFactor: pane.options.deviceScaleFactor,
      },
      browser: { name: info.browser.name, product: info.product, pid: info.pid },
      settings: {
        quality: cfg.quality,
        format: cfg.format,
        ack: cfg.eagerAck ? "backend, as soon as the frame is on the channel" : "frontend, after drawing",
        phaseSecs: cfg.phaseSecs,
        probes: cfg.probeIterations,
      },
    };

    // 1. Idle: a static page, no screencast running.
    await pane.navigate(`${base}/click.html`);
    await sleep(2000);
    await pane.detach();
    await sleep(300);
    report.idleNoScreencast = (await sampled("idle", () => sleep(cfg.phaseSecs * 1000))).cpuRss;

    // 2. Static page with the screencast on (damage-driven, so almost no frames).
    let staticFrames = 0;
    pane.onDrawn = () => staticFrames++;
    await pane.attach(0);
    await sleep(1500);
    staticFrames = 0;
    const staticRun = await sampled("static-screencast", () => sleep(5000));
    report.staticPageWithScreencast = { framesIn5s: staticFrames, cpuRss: staticRun.cpuRss };

    // 3. Streaming: the animation page, end to end into the canvas.
    await pane.navigate(`${base}/animation.html`);
    await sleep(1500);
    const drawn: DrawnFrame[] = [];
    pane.onDrawn = (f) => drawn.push({ ...f, bitmap: undefined as unknown as ImageBitmap });
    const dropped0 = pane.stats.framesDropped;
    const streamRun = await sampled("streaming", async () => {
      const t0 = performance.now();
      await sleep(cfg.phaseSecs * 1000);
      return [t0, performance.now()] as const;
    });
    pane.onDrawn = undefined;
    const [t0, t1] = streamRun.result;
    const inWindow = drawn.filter((f) => f.drawnAt >= t0 && f.drawnAt <= t1);
    const intervals = inWindow.slice(1).map((f, i) => f.drawnAt - inWindow[i]!.drawnAt);
    const transit = inWindow.map((f) => f.receivedEpochMs - f.header.sentEpochMs);
    report.streaming = {
      seconds: (t1 - t0) / 1000,
      frames: inWindow.length,
      fps: inWindow.length / ((t1 - t0) / 1000),
      drawIntervalMs: dist(intervals),
      intervalsOver33ms: intervals.filter((x) => x > 33.4).length,
      intervalsOver50ms: intervals.filter((x) => x > 50).length,
      frameKiB: dist(inWindow.map((f) => f.bytes / 1024)),
      mbitPerSecond: (inWindow.reduce((a, f) => a + f.bytes, 0) * 8) / ((t1 - t0) / 1000) / 1e6,
      channelTransitMs: dist(transit),
      droppedByPane: pane.stats.framesDropped - dropped0,
      cpuRss: streamRun.cpuRss,
    };

    // 4. Click-to-frame latency. A DOM pointerdown/pointerup is dispatched on the canvas, so the
    // whole path runs: Pane handler -> invoke -> Rust -> CDP pipe -> Chromium -> screencast
    // frame -> channel -> createImageBitmap -> drawImage. The clock stops when the first frame
    // whose probe pixel shows the new colour has been drawn to the canvas.
    await pane.navigate(`${base}/click.html`);
    await sleep(1500);
    const colours: [number, number, number][] = [[255, 0, 0], [0, 0, 255], [0, 255, 0], [255, 255, 255]];
    const probe = new OffscreenCanvas(1, 1).getContext("2d", { willReadFrequently: true })!;
    const cssRect = pane.canvas.getBoundingClientRect();
    const view = pane.geometry();
    const clientX = cssRect.left + (100 * cssRect.width) / view.viewWidth;
    const clientY = cssRect.top + (100 * cssRect.height) / view.viewHeight;
    const latency: number[] = [];
    const transitAtMatch: number[] = [];
    const decodeAtMatch: number[] = [];
    let misses = 0;
    for (let i = 0; i < cfg.probeIterations; i++) {
      await sleep(120);
      const want = colours[i % 4]!;
      const match = new Promise<DrawnFrame | null>((resolve) => {
        const timer = setTimeout(() => {
          pane.onDrawn = undefined;
          resolve(null);
        }, 500);
        pane.onDrawn = (f) => {
          probe.drawImage(f.bitmap, Math.round((100 * f.bitmap.width) / view.viewWidth), Math.round((100 * f.bitmap.height) / view.viewHeight), 1, 1, 0, 0, 1, 1);
          const px = probe.getImageData(0, 0, 1, 1).data;
          if ([0, 1, 2].every((c) => Math.abs(px[c]! - want[c]!) < 70)) {
            clearTimeout(timer);
            pane.onDrawn = undefined;
            resolve(f);
          }
        };
      });
      const start = performance.now();
      const ev = (type: string, buttons: number) =>
        new PointerEvent(type, { clientX, clientY, button: 0, buttons, detail: 1, bubbles: true, cancelable: true });
      pane.canvas.dispatchEvent(ev("pointerdown", 1));
      pane.canvas.dispatchEvent(ev("pointerup", 0));
      const hit = await match;
      if (hit) {
        latency.push(hit.drawnAt - start);
        transitAtMatch.push(hit.receivedEpochMs - hit.header.sentEpochMs);
        decodeAtMatch.push(pane.stats.lastDecodeMs);
      } else {
        misses++;
      }
    }
    report.inputToFrameMs = {
      definition: "synthetic DOM pointerdown on the canvas -> drawImage of the first frame showing the change",
      misses,
      dist: dist(latency),
      channelTransitMs: dist(transitAtMatch),
      decodeMs: dist(decodeAtMatch),
    };

    // 5. Second context: switch time (attach -> first frame) and isolation seen from the UI.
    let first: number | null = null;
    const switchStart = performance.now();
    pane.onDrawn = () => {
      if (first === null) first = performance.now() - switchStart;
    };
    await pane.attach(1);
    for (let i = 0; i < 50 && first === null; i++) await sleep(20);
    pane.onDrawn = undefined;
    report.contextSwitch = { toFirstFrameMs: first, shows: await invoke("bench_eval", { context: 1, expression: "location.href" }) };

    report.paneStats = pane.stats;
    report.ok = true;
  } catch (e) {
    report.ok = false;
    report.error = String(e);
  }
  await invoke("bench_finish", { report });
}
