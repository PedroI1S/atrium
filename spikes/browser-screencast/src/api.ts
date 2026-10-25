import { invoke } from "@tauri-apps/api/core";

export interface BrowserInfo {
  id: string;
  name: string;
  appPath: string | null;
  binary: string;
}

export interface StartInfo {
  browser: BrowserInfo;
  product: string;
  pid: number;
  fixturePort: number;
  userDataDir: string;
  contexts: string[];
}

export interface StartupInfo {
  bench: string | null;
  quality: number;
  format: "jpeg" | "png";
  deviceScaleFactor: number | null;
  eagerAck: boolean;
  clicks: number;
  phaseSecs: number;
  probeIterations: number;
}

export const startupInfo = () => invoke<StartupInfo>("startup_info");
export const browserStart = () => invoke<StartInfo>("browser_start", { defaultBrowser: null });
export const setUserControl = (on: boolean) => invoke<void>("set_user_control", { on });
export const agentAction = (context: number, kind: "click" | "navigate", extra: { x?: number; y?: number; url?: string } = {}) =>
  invoke<string>("agent_action", { context, kind, ...extra });
