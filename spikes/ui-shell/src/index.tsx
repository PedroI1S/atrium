/* @refresh reload */
import { render } from "solid-js/web";
import { App } from "./App";
import "./styles/base.css";
import "./styles/components.css";
import { markOnce, runHarness } from "./bench";
import { startupInfo } from "./platform";
import { PRESETS, activateTheme, applyPreset, setActiveTabId, setRightTab, setPaletteOpen, setThemes, tabs, type RightTabId } from "./state";
import { loadAllThemes } from "./theme/registry";

async function boot() {
  const info = await startupInfo();
  const { entries, errors } = loadAllThemes();
  setThemes(entries);
  for (const e of errors) console.error("theme failed to load:", e.message);

  let saved: string | null = null;
  try {
    saved = localStorage.getItem("atrium.theme");
  } catch {
    /* ignore */
  }
  const wanted = info.theme ?? saved ?? "glass-dark";
  // A theme pinned by the harness (env) is not remembered, so it never leaks into later runs.
  activateTheme(entries.some((t) => t.id === wanted) ? wanted : "glass-dark", !info.theme);

  if (info.preset && info.preset in PRESETS) applyPreset(info.preset as keyof typeof PRESETS, true);
  if (info.tab) {
    const t = tabs().find((x) => x.id === `tab-${info.tab}` || x.id === info.tab);
    if (t) setActiveTabId(t.id);
  }
  if (info.rightTab) setRightTab(info.rightTab as RightTabId);
  if (info.openPalette) setPaletteOpen(true);

  render(() => <App />, document.getElementById("root")!);
  markOnce("mounted");
  if (info.benchMode) void runHarness(info);
}

void boot();
