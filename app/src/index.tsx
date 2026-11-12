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
import "./styles/shell.css";
import { App } from "./App";
import { attach, daemon } from "./daemon/client";
import { epochNow, markOnce, reportFailure, runHarness, setStage } from "./harness";
import type { Screen } from "./layout";
import { type StartupInfo, inTauri, startupInfo } from "./platform";
import { setAdvancedOpen, setPaletteOpen, setPanel, setScreen } from "./state";
import { afterMount, appliedState, initFromStartup, isPopout, repaint, subscribe } from "./windows";

const SCREENS: Screen[] = ["chat", "inbox", "settings"];

/** Harness and screenshot pins (ATRIUM_SCREEN, ATRIUM_PANEL, ATRIUM_ADVANCED, ATRIUM_PALETTE). */
function applyPins(info: StartupInfo): void {
  if (isPopout()) return;
  if (info.screen && SCREENS.includes(info.screen as Screen)) setScreen(info.screen as Screen);
  if (info.panel === "min") setPanel({ open: false });
  if (info.advanced) setAdvancedOpen(true);
  if (info.openPalette) setPaletteOpen(true);
}

let info: StartupInfo | null = null;

async function boot() {
  // One subscription and one startup call (snapshot included), then paint tokens and render:
  // no theme loading or native call sits before the first render.
  setStage("subscribe");
  await subscribe();
  setStage("startup-info");
  info = await startupInfo();
  initFromStartup(info);
  applyPins(info);
  setStage("render");
  render(() => <App />, document.getElementById("root")!);
  markOnce("mounted");
  // First paint: the second animation frame after mount, when the frame holding the mounted
  // DOM has been produced.
  requestAnimationFrame(() => requestAnimationFrame(() => markOnce("firstPaint")));
  // The daemon's state arrives after the first render: the shell never waits for the socket.
  setStage("attach");
  await attach();
  if (inTauri && !isPopout()) {
    // The pop-outs of the last session open only once main is interactive (Rust holds the list).
    setStage("restore-windows");
    await invoke("restore_windows");
  }
  setStage("after-mount");
  await afterMount();
  // Native reconciliation runs after mount; Rust already applied the settings to the window.
  setStage("native-paint");
  await repaint();
  if (inTauri) {
    setStage("ready");
    await invoke("window_ready", { applied: { ...appliedState(), readyAt: epochNow() } });
  }
  if (info.benchMode && !isPopout()) {
    setStage("harness");
    await runHarness(info, () => ({ connection: daemon().connection.type, threads: daemon().order.length }));
  }
}

boot().catch((e) => reportFailure(e, info));
