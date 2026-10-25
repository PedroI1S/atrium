import { setThemeId } from "./state";
import { createSignal } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { emitTo, listen } from "@tauri-apps/api/event";
import { inTauri, setVibrancy, setWindowTheme } from "./platform";
import { applyTheme } from "./theme/runtime";
import { loadAllThemes } from "./theme/registry";
import { initialModel, moveView, opacityTokens, type AppearanceSettings, type ViewId, type ViewState, type WindowModel } from "./window-model";

export const viewId = new URLSearchParams(location.search).get("view") as ViewId | null;
export const windowLabel = viewId ? `view-${viewId}` : "main";
export const [model, setModel] = createSignal(initialModel());
export const [nativeSupported, setNativeSupported] = createSignal(false);
export const [accessibilityOpaque, setAccessibilityOpaque] = createSignal(false);
export const [nativeError, setNativeError] = createSignal("");
const entries = loadAllThemes().entries;
let queue = Promise.resolve();
let unlisten: (() => void) | undefined;
let appliedSettings = "";

function accept(next: WindowModel) {
  if (next.revision < model().revision) return;
  const views = { ...next.views };
  for (const item of pending) views[item.view] = { ...views[item.view], ...item.patch };
  setModel({ ...next, views });
  const key = JSON.stringify(next.settings);
  if (key !== appliedSettings) {
    appliedSettings = key;
    queue = queue.catch(() => {}).then(() => paint(next.settings)).catch(e => { setNativeError(String(e)); });
  }
}
async function paint(s: AppearanceSettings) {
  setNativeError("");
  const entry = entries.find(e => e.id === s.theme)!;
  if (!entry) throw new Error(`Unknown theme ${s.theme}`);
  applyTheme(entry.theme, false);
  setThemeId(s.theme);
  await setWindowTheme(entry.theme.meta.appearance);
  const nativeReduce = inTauri && await invoke<boolean>("opaque_accessibility");
  const reduce = nativeReduce || matchMedia("(prefers-reduced-transparency: reduce)").matches || matchMedia("(prefers-contrast: more)").matches;
  setAccessibilityOpaque(reduce);
  const tokens = opacityTokens(s, nativeSupported(), reduce);
  for (const [key, value] of Object.entries(tokens)) document.documentElement.style.setProperty(key, value);
  const glass = await setVibrancy(s.mode === "background" && s.alpha < 1 && s.blur && !reduce, entry.theme.meta.appearance === "dark");
  document.documentElement.dataset.vibrancy = String(glass);
  if (nativeSupported() && s.mode === "background" && s.alpha < 1 && s.blur && !reduce && !glass) {
    for (const [key, value] of Object.entries(opacityTokens(s, false))) document.documentElement.style.setProperty(key, value);
    throw new Error("Native blur failed; drawing opaque surfaces.");
  }
  if (inTauri) {
    const applied = await invoke<number>("set_window_alpha", { alpha: Number(tokens["--window-opacity"]) });
    document.documentElement.dataset.nativeAlpha = String(applied);
    if (applied !== Number(tokens["--window-opacity"])) throw new Error("Native opacity was not applied");
  }
}
export async function initWindows(supported: boolean) {
  setNativeSupported(supported);
  if (inTauri) {
    // Subscribe before snapshot; revision guards ignore an older response.
    unlisten = await listen<WindowModel>("shell-state", e => accept(e.payload));
    accept(await invoke<WindowModel>("shell_snapshot"));
    if (viewId) await listen("return-view", () => { void bringBack(viewId); });
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().onCloseRequested(async event => {
      event.preventDefault();
      await writes;
      if (viewId) await bringBack(viewId); else await invoke("quit");
    });
  } else {
    try { const s = localStorage.getItem("atrium.calm.settings"); if (s) model().settings = { ...model().settings, ...JSON.parse(s) }; } catch { /* defaults */ }
    accept(model());
  }
  const media = [matchMedia("(prefers-reduced-transparency: reduce)"), matchMedia("(prefers-contrast: more)")];
  const refresh = () => { appliedSettings = ""; accept(model()); };
  media.forEach(m => m.addEventListener("change", refresh));
  window.addEventListener("focus", refresh);
  window.addEventListener("pagehide", () => { unlisten?.(); window.removeEventListener("focus", refresh); media.forEach(m => m.removeEventListener("change", refresh)); }, { once: true });
  await queue;
  if (nativeError()) throw new Error(nativeError());
}
export async function patchSettings(patch: Partial<AppearanceSettings>) {
  if (inTauri) accept(await invoke<WindowModel>("shell_patch", { view: null, patch }));
  else {
    const next = { ...model(), revision: model().revision + 1, settings: { ...model().settings, ...patch } };
    localStorage.setItem("atrium.calm.settings", JSON.stringify(next.settings)); accept(next);
  }
  await queue;
}
let writes = Promise.resolve();
let writeFailure: unknown = null;
const pending: { view: ViewId; patch: Partial<ViewState> }[] = [];
export async function patchView(view: ViewId, patch: Partial<ViewState>) {
  if (inTauri) {
    const item = { view, patch };
    pending.push(item);
    setModel(current => ({ ...current, views: { ...current.views, [view]: { ...current.views[view], ...patch } } }));
    writes = writes.then(async () => {
      try {
        const next = await invoke<WindowModel>("shell_patch", { view, patch });
        pending.splice(pending.indexOf(item), 1);
        accept(next);
        writeFailure = null;
      } catch (e) {
        pending.splice(pending.indexOf(item), 1);
        writeFailure = e;
        setNativeError(String(e));
      }
    });
    await writes;
  }
  else accept({ ...model(), revision: model().revision + 1, views: { ...model().views, [view]: { ...model().views[view], ...patch } } });
}
export async function popOut(view: ViewId) {
  await writes;
  if (writeFailure) throw writeFailure;
  if (!inTauri) { setNativeError("Pop-out windows require the native app."); return; }
  accept(await invoke<WindowModel>("pop_out", { view }));
}
export async function bringBack(view: ViewId) {
  await writes;
  if (writeFailure) throw writeFailure;
  if (inTauri) {
    const owner = model().views[view].owner;
    if (owner !== windowLabel && owner !== "main") { await emitTo(owner, "return-view"); return; }
    accept(await invoke<WindowModel>("bring_back", { view }));
  }
  else accept(moveView(model(), view, "main"));
}
export const settingsSettled = () => queue;
