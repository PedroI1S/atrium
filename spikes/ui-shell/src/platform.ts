// Thin wrapper over Tauri so the same frontend also runs in a plain browser (vite dev,
// used for quick functional checks). Everything degrades to a no-op there.

import { invoke } from "@tauri-apps/api/core";

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export interface StartupInfo {
  mainEpochMs: number;
  benchMode: string | null;
  theme: string | null;
  preset: string | null;
  openPalette: boolean;
  tab: string | null;
  rightTab: string | null;
  vibrancySupported: boolean;
}

const BROWSER_DEFAULTS: StartupInfo = { mainEpochMs: 0, benchMode: null, theme: null, preset: null, openPalette: false, tab: null, rightTab: null, vibrancySupported: false };

/** Whether this platform can show native vibrancy at all (Rust says macOS only; a browser says no). */
let vibrancyAvailable = false;
export const vibrancySupported = (): boolean => vibrancyAvailable;

export async function startupInfo(): Promise<StartupInfo> {
  const info = await readStartupInfo();
  vibrancyAvailable = info.vibrancySupported;
  return info;
}

async function readStartupInfo(): Promise<StartupInfo> {
  if (!inTauri) {
    const q = new URLSearchParams(location.search);
    return {
      ...BROWSER_DEFAULTS,
      benchMode: q.get("bench"),
      theme: q.get("theme"),
      preset: q.get("preset"),
      openPalette: q.has("palette"),
      tab: q.get("tab"),
      rightTab: q.get("rightTab"),
    };
  }
  return invoke<StartupInfo>("startup_info");
}

/** Returns whether native vibrancy is applied now (always false when it was not requested or failed). */
export async function setVibrancy(enabled: boolean, dark: boolean): Promise<boolean> {
  if (!inTauri) return false;
  try {
    const result = await invoke<boolean>("set_vibrancy", { enabled, dark });
    return enabled && result;
  } catch (e) {
    console.warn("set_vibrancy failed", e);
    return false;
  }
}

export async function setWindowTheme(appearance: "dark" | "light"): Promise<void> {
  if (!inTauri) return;
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().setTheme(appearance);
  } catch (e) {
    console.warn("setTheme failed", e);
  }
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
