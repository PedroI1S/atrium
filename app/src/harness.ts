// Startup timings for the measurement harness (contract §14). Marks are wall-clock ms since the
// Unix epoch so they compare with Rust's `mainEpochMs`.
//
// TTI is the later of the first paint (the second animation frame after mount, when the frame
// holding the mounted DOM has been produced) and the first handled input.
//
// The report also carries the startup profile (M1-14b, L3): Rust's native marks, the webview's
// navigation timing, the time boot.js and the bundle started, and the time each boot stage
// began (`setStage`). They cost a clock read each.

import { type StartupInfo, quitApp, writeReport } from "./platform";

declare global {
  interface Window { __atriumJsStart?: number; __atriumModuleStart?: number }
}

export const epochNow = (): number => performance.timeOrigin + performance.now();

export type Mark = "mounted" | "firstPaint" | "firstInput";
const marks: Partial<Record<Mark, number>> = {};
let stage = "boot";
/** When each boot stage began (epoch ms), in order. */
const stages: [string, number][] = [];

export function setStage(next: string): void {
  stage = next;
  stages.push([next, epochNow()]);
}

export function markOnce(name: Mark): void {
  marks[name] ??= epochNow();
}

/** A key the app doesn't bind: the probe dispatches it and the app's key handler marks it. */
export const PROBE_KEY = "F24";

/** Dispatches the probe key; the handler in App.tsx calls `markOnce("firstInput")`. */
export function probeInput(): void {
  window.dispatchEvent(new KeyboardEvent("keydown", { key: PROBE_KEY }));
}

const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));

/** The webview's own timing of the page load, as epoch ms (navigation and each resource). */
function loadTiming(): Record<string, unknown> {
  const origin = performance.timeOrigin;
  const at = (ms: number) => (ms > 0 ? origin + ms : null);
  const nav = performance.getEntriesByType("navigation")[0] as PerformanceNavigationTiming | undefined;
  const resources = (performance.getEntriesByType("resource") as PerformanceResourceTiming[]).map((r) => ({
    name: r.name.replace(/^.*\//, ""), start: at(r.startTime), end: at(r.responseEnd),
  }));
  return {
    timeOrigin: origin,
    navigation: nav && {
      fetchStart: at(nav.fetchStart), responseStart: at(nav.responseStart), responseEnd: at(nav.responseEnd),
      domInteractive: at(nav.domInteractive), domContentLoaded: at(nav.domContentLoadedEventStart), loadEnd: at(nav.loadEventEnd),
    },
    resources,
    paint: performance.getEntriesByType("paint").map((p) => ({ name: p.name, at: at(p.startTime) })),
  };
}

/** `ATRIUM_BENCH=startup`: report and stay open (memory sampling). `startup-exit`: report and quit. */
export async function runHarness(info: StartupInfo, extra: () => Record<string, unknown>): Promise<void> {
  while (marks.firstPaint === undefined) await frame();
  probeInput();
  const firstPaint = marks.firstPaint;
  const firstInput = marks.firstInput ?? Number.NaN;
  await writeReport({
    kind: "startup",
    label: info.label,
    platform: info.platform,
    mainEpochMs: info.mainEpochMs,
    jsStart: window.__atriumJsStart ?? null,
    mounted: marks.mounted ?? null,
    firstPaint,
    firstInput,
    tti: Math.max(firstPaint, firstInput) - info.mainEpochMs,
    profile: {
      native: info.startupMarks,
      moduleStart: window.__atriumModuleStart ?? null,
      stages,
      ...loadTiming(),
    },
    ...extra(),
  });
  if (info.benchMode === "startup-exit") await quitApp();
}

export async function reportFailure(error: unknown, info: StartupInfo | null): Promise<void> {
  console.error(error);
  if (!info?.benchMode) return;
  try {
    await writeReport({ kind: "failure", stage, error: String(error) });
  } finally {
    await quitApp();
  }
}
