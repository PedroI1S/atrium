/* @refresh reload */
import { invoke } from "@tauri-apps/api/core";
import { render } from "solid-js/web";
import "@fontsource/instrument-sans/latin-400.css";
import "@fontsource/instrument-sans/latin-500.css";
import "@fontsource/instrument-sans/latin-600.css";
import "@fontsource/geist-mono/latin-400.css";
import "@fontsource/geist-mono/latin-500.css";
import "./styles/base.css";
import "./styles/calm.css";
import { App } from "./App";
import { markOnce, reportFailure, runHarness, runProbe, setStage } from "./bench";
import { type PanelTab, type Screen } from "./layout";
import { type StartupInfo, inTauri, startupInfo } from "./platform";
import { setAdvancedOpen, setPaletteOpen, setPanel, setScreen } from "./state";
import { afterMount, appliedState, initFromStartup, repaint, subscribe, viewId } from "./windows";

const SCREENS: Screen[] = ["chat", "inbox", "editor", "database", "settings"];

/** Harness and screenshot pins (ATRIUM_SCREEN, ATRIUM_RIGHT_TAB, ATRIUM_ADVANCED, ATRIUM_PANEL). */
function applyPins(info: StartupInfo): void {
  if (viewId) return;
  if (info.screen && SCREENS.includes(info.screen as Screen)) setScreen(info.screen as Screen);
  if (info.rightTab) setPanel({ tab: info.rightTab as PanelTab });
  if (info.panel === "min") setPanel({ open: false });
  if (info.advanced) setAdvancedOpen(true);
  if (info.openPalette) setPaletteOpen(true);
}

let info: StartupInfo | null = null;

async function boot() {
  // One subscription and one startup call (snapshot included), then paint tokens and render:
  // no other IPC, theme loading or native call sits before the first render (review #5).
  setStage("subscribe");
  await subscribe();
  setStage("startup-info");
  info = await startupInfo();
  initFromStartup(info);
  applyPins(info);
  setStage("render");
  render(() => <App />, document.getElementById("root")!);
  markOnce("mounted");
  requestAnimationFrame(() => markOnce("firstPaintRaf"));
  const harnessMain = Boolean(info.benchMode) && !viewId;
  if (harnessMain) {
    setStage("probe");
    await runProbe();
  }
  setStage("after-mount");
  await afterMount();
  // Native reconciliation runs after mount; Rust already applied the pinned state in setup.
  setStage("native-paint");
  await repaint();
  if (inTauri) {
    // Ready = mounted and appearance applied. Not rAF, which stops in occluded windows (review #1).
    setStage("ready");
    await invoke("window_ready", { applied: appliedState() });
  }
  if (harnessMain) await runHarness(info);
}

boot().catch((e) => reportFailure(e, info));
