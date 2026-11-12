// Startup timings for the measurement harness (contract §14). Marks are wall-clock ms since the
// Unix epoch so they compare with Rust's `mainEpochMs`.
//
// TTI is the later of the first paint (the second animation frame after mount, when the frame
// holding the mounted DOM has been produced) and the first handled input.

import { type StartupInfo, quitApp, writeReport } from "./platform";

declare global {
  interface Window { __atriumJsStart?: number }
}

export const epochNow = (): number => performance.timeOrigin + performance.now();

export type Mark = "mounted" | "firstPaint" | "firstInput";
const marks: Partial<Record<Mark, number>> = {};
let stage = "boot";

export function setStage(next: string): void {
  stage = next;
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
