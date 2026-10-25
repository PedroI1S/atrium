/* @refresh reload */
import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "./platform";
import { render } from "solid-js/web";
import { initWindows, popOut, settingsSettled, viewId } from "./windows";
import { App } from "./App";
import "./styles/base.css";
import "./styles/components.css";
import { markOnce, runHarness } from "./bench";
import { startupInfo } from "./platform";
import { PRESETS, applyPreset, floatingDefaultSizes, setOuterSizes, setActiveTabId, setRightTab, setPaletteOpen, setThemes, tabs, type RightTabId } from "./state";
import { loadAllThemes } from "./theme/registry";

async function boot() {
  const info = await startupInfo();
  const { entries, errors } = loadAllThemes();
  setThemes(entries.filter(e => e.direction === "atrium"));
  for (const e of errors) console.error("theme failed to load:", e.message);

  if (info.preset && info.preset in PRESETS) applyPreset(info.preset as keyof typeof PRESETS, true);
  if (info.tab) {
    const t = tabs().find((x) => x.id === `tab-${info.tab}` || x.id === info.tab);
    if (t) setActiveTabId(t.id);
  }
  if (info.rightTab) setRightTab(info.rightTab as RightTabId);
  if (info.openPalette) setPaletteOpen(true);

  await initWindows(info.vibrancySupported);
  if (!info.preset || info.preset === "default") setOuterSizes(floatingDefaultSizes());
  if (!viewId && info.windows === 3) { await popOut("editor"); await popOut("database"); }
  await settingsSettled();
  render(() => <App />, document.getElementById("root")!);
  markOnce("mounted");
  if (inTauri) {
    await new Promise<void>(r => requestAnimationFrame(() => requestAnimationFrame(() => r())));
    markOnce("firstPaintRaf");
    await invoke("window_ready");
    if (!viewId && info.benchMode) {
      let count = 0;
      for (let i = 0; i < 500; i++) {
        count = await invoke<number>("ready_count");
        if (count === info.windows) break;
        await new Promise(r => setTimeout(r, 20));
      }
      if (count !== info.windows) throw new Error(`Expected ${info.windows} ready windows; got ${count}`);
    }
  }
  if (info.benchMode && !viewId) void runHarness(info);
}

void boot();
