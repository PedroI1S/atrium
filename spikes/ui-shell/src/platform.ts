// Thin wrapper over Tauri so the same frontend also runs in a plain browser (vite dev, for
// quick layout checks). Everything native degrades to a no-op there.

import { invoke } from "@tauri-apps/api/core";
import type { WindowModel } from "./window-model";

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export interface StartupInfo {
  mainEpochMs: number;
  label: string;
  benchMode: string | null;
  openPalette: boolean;
  /** Harness/screenshot pins: starting screen, panel tab, Advanced open, panel minimized. */
  screen: string | null;
  rightTab: string | null;
  advanced: boolean;
  panel: string | null;
  vibrancySupported: boolean;
  windows: number;
  opacity: string;
  accessibilityOpaque: boolean;
  /** What Rust already applied natively to this window before the webview loaded. */
  native: { vibrancy: boolean; alpha: number };
  /** The first state snapshot (null in a browser). */
  snapshot: WindowModel | null;
}

export async function startupInfo(): Promise<StartupInfo> {
  if (inTauri) return invoke<StartupInfo>("startup_info");
  const q = new URLSearchParams(location.search);
  return {
    mainEpochMs: 0, label: q.get("view") ? `view-${q.get("view")}` : "main", benchMode: q.get("bench"), openPalette: q.has("palette"),
    screen: q.get("screen"), rightTab: q.get("rightTab"), advanced: q.has("advanced"), panel: q.get("panel"),
    vibrancySupported: false, windows: 1, opacity: "off", accessibilityOpaque: false, native: { vibrancy: false, alpha: 1 }, snapshot: null,
  };
}

/** Native window appearance; "system" clears the override so the window follows macOS. */
export async function setWindowTheme(appearance: "dark" | "light" | "system"): Promise<void> {
  if (!inTauri) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().setTheme(appearance === "system" ? null : appearance);
}

export async function writeReport(payload: unknown): Promise<void> {
  const json = JSON.stringify(payload);
  if (!inTauri) {
    console.log("ATRIUM_REPORT", json);
    return;
  }
  await invoke("write_report", { json });
}

export async function quitApp(): Promise<void> {
  if (inTauri) await invoke("quit");
}
