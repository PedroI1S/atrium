// Thin wrapper over Tauri so the same frontend also runs in a plain browser (vite dev, for
// quick layout checks). Everything native degrades to a no-op there.

import { invoke } from "@tauri-apps/api/core";
import { MAIN, type WindowModel } from "./window-model";

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export interface StartupInfo {
  mainEpochMs: number;
  label: string;
  /** The thread a pop-out shows; null in the main window. */
  popoutThread: string | null;
  platform: "macos" | "linux";
  /** A development build: the dev commands are offered. */
  debug: boolean;
  /** Harness and screenshot pins. */
  benchMode: string | null;
  screen: string | null;
  panel: string | null;
  advanced: boolean;
  openPalette: boolean;
  windows: number;
  /** The platform can draw a translucent window (macOS). */
  translucency: boolean;
  accessibilityOpaque: boolean;
  /** What Rust already applied natively to this window before the webview loaded. */
  native: { vibrancy: boolean; dark: boolean; alpha: number };
  hasTray: boolean;
  /** The first state snapshot (null in a browser). */
  snapshot: WindowModel | null;
}

export async function startupInfo(): Promise<StartupInfo> {
  if (inTauri) return invoke<StartupInfo>("startup_info");
  const q = new URLSearchParams(location.search);
  return {
    mainEpochMs: 0, label: MAIN, popoutThread: null, platform: "linux", debug: true, benchMode: q.get("bench"),
    screen: q.get("screen"), panel: q.get("panel"), advanced: q.has("advanced"), openPalette: q.has("palette"), windows: 1,
    translucency: false, accessibilityOpaque: false, native: { vibrancy: false, dark: false, alpha: 1 }, hasTray: false, snapshot: null,
  };
}

/** Native window appearance; "system" clears the override so the window follows the OS. */
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

/** Opens a link in the system browser. Rust accepts http, https and mailto only (contract §12). */
export async function openLink(url: string): Promise<void> {
  if (inTauri) await invoke("open_link", { url });
}
