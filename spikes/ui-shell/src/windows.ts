// Glue between the UI and the Rust single writer (shell.rs): the window model signal,
// optimistic view edits, settings patches, appearance painting and pop-out lifecycle.
// The ordering rules live in sync.ts (pure, tested); this file wires them to Tauri.

import { createSignal } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { emitTo, listen } from "@tauri-apps/api/event";
import { inTauri, setWindowTheme, type StartupInfo } from "./platform";
import { applyTheme } from "./theme/runtime";
import { calmTheme } from "./theme/calm";
import { WriteQueue, bringBackFromMain, closeOwnedView, coalescing, debounced, latestWins } from "./sync";
import { DEFAULT_SETTINGS, appearanceFor, initialModel, type Appearance, type AppearanceSettings, type ViewId, type ViewState, type WindowModel } from "./window-model";

export const viewId = new URLSearchParams(location.search).get("view") as ViewId | null;
export const windowLabel = viewId ? `view-${viewId}` : "main";
export const [model, setModel] = createSignal(initialModel());
export const [nativeSupported, setNativeSupported] = createSignal(false);
export const [accessibilityOpaque, setAccessibilityOpaque] = createSignal(false);
export const [nativeError, setNativeError] = createSignal("");
/** The view being moved to its own window: read-only until the move finishes (review #8). */
export const [moving, setMoving] = createSignal<ViewId | null>(null);

/** How long main waits for a pop-out to hand its view back before forcing it. */
const BRING_BACK_TIMEOUT_MS = 1500;
const BRING_BACK_POLL_MS = 50;
/** Scroll position is state too, but only needs to land before a move, not every frame. */
const SCROLL_PERSIST_MS = 150;

const writes = new WriteQueue((e) => setNativeError(String(e)));
const pending: { view: ViewId; patch: Partial<ViewState> }[] = [];
/** Settings sent to Rust but not yet confirmed; overlaid so a slider never jumps back. */
const [settingsOverlay, setSettingsOverlay] = createSignal<Partial<AppearanceSettings> | null>(null);

/** What the native window currently shows, so paint only calls native APIs on a change. */
const native = { vibrancy: false, alpha: 1, appearance: "" as "" | WindowAppearance };
let paintedTheme = "";
let lastAppearance: Appearance | null = null;

const systemDark = () => matchMedia("(prefers-color-scheme: dark)").matches;
const mediaReduce = () => matchMedia("(prefers-reduced-transparency: reduce)").matches || matchMedia("(prefers-contrast: more)").matches;
let osReduce = false;

export const effectiveSettings = (): AppearanceSettings => ({ ...model().settings, ...(settingsOverlay() ?? {}) });

function currentAppearance(): Appearance {
  return appearanceFor({ settings: effectiveSettings(), supported: nativeSupported(), reduce: osReduce || mediaReduce(), popout: viewId !== null, systemDark: systemDark() });
}

/** The native window follows the OS for "system", so prefers-color-scheme keeps tracking it. */
type WindowAppearance = "light" | "dark" | "system";
const windowAppearance = (a: Appearance): WindowAppearance => (effectiveSettings().theme === "system" ? "system" : a.native.appearance);

/** CSS half of a paint: synchronous, cheap, safe to run on every slider event. */
function paintTokens(a: Appearance): void {
  if (a.theme !== paintedTheme) {
    applyTheme(calmTheme(a.theme), a.native.vibrancy);
    paintedTheme = a.theme;
  }
  const root = document.documentElement.style;
  for (const [k, v] of Object.entries(a.tokens)) root.setProperty(k, v);
  setAccessibilityOpaque(osReduce || mediaReduce());
  lastAppearance = a;
}

/** Native half of a paint: only the calls whose inputs changed, in parallel. */
async function paintNative(a: Appearance): Promise<void> {
  if (!inTauri) return;
  const calls: Promise<void>[] = [];
  const appearance = windowAppearance(a);
  if (appearance !== native.appearance) {
    calls.push(setWindowTheme(appearance).then(() => { native.appearance = appearance; }));
  }
  if (a.native.vibrancy !== native.vibrancy) {
    calls.push(invoke<boolean>("set_vibrancy", { enabled: a.native.vibrancy, dark: a.native.appearance === "dark" }).then((v) => { native.vibrancy = v; }));
  }
  if (a.native.alpha !== native.alpha) {
    calls.push(invoke<number>("set_window_alpha", { alpha: a.native.alpha }).then((v) => { native.alpha = v; }));
  }
  const results = await Promise.allSettled(calls);
  const failed = results.find((r): r is PromiseRejectedResult => r.status === "rejected");
  document.documentElement.dataset.vibrancy = String(native.vibrancy);
  document.documentElement.dataset.nativeAlpha = String(native.alpha);
  if (failed) throw failed.reason;
  if (a.native.vibrancy && !native.vibrancy) {
    // Blur was asked for and the platform refused: draw opaque surfaces instead of see-through ones.
    paintTokens(appearanceFor({ settings: effectiveSettings(), supported: false, reduce: true, popout: viewId !== null, systemDark: systemDark() }));
    throw new Error("Native blur failed; drawing opaque surfaces.");
  }
}

/** Latest-wins: a slider drag never queues more than one pending native paint. */
const paintLatest = latestWins<void>(async () => {
  try {
    setNativeError("");
    await paintNative(currentAppearance());
  } catch (e) {
    setNativeError(String(e));
  }
});

/** Repaint now (tokens) and reconcile the native window (async, coalesced). */
export function repaint(): Promise<void> {
  paintTokens(currentAppearance());
  return paintLatest();
}

function accept(next: WindowModel) {
  if (next.revision < model().revision) return;
  const views = { ...next.views };
  for (const item of pending) views[item.view] = { ...views[item.view], ...item.patch };
  const before = JSON.stringify(effectiveSettings());
  setModel({ ...next, views });
  if (JSON.stringify(effectiveSettings()) !== before) void repaint();
}

/**
 * First paint, synchronously from the startup snapshot (no IPC between here and render).
 * `info.native` is what Rust already applied before the webview loaded.
 */
export function initFromStartup(info: StartupInfo): void {
  setNativeSupported(info.vibrancySupported);
  osReduce = info.accessibilityOpaque;
  native.vibrancy = info.native.vibrancy;
  native.alpha = info.native.alpha;
  if (info.snapshot) setModel(info.snapshot);
  else {
    try {
      const saved = localStorage.getItem("atrium.calm.settings");
      if (saved) setModel({ ...model(), settings: { ...DEFAULT_SETTINGS, ...JSON.parse(saved) } });
    } catch { /* defaults */ }
  }
  // Rust set the window theme in setup (main) or in the builder (pop-outs).
  native.appearance = windowAppearance(currentAppearance());
  paintTokens(currentAppearance());
}

/** Subscribes before the snapshot is read (the revision guard drops anything older). */
export async function subscribe(): Promise<void> {
  if (!inTauri) return;
  await listen<WindowModel>("shell-state", (e) => accept(e.payload));
  await listen<boolean>("accessibility-changed", (e) => { osReduce = e.payload; void repaint(); });
  if (viewId) await listen("return-view", () => { void closeThisPopout(); });
}

/** Wires the OS-preference listeners and the close handler. Runs after the first render. */
export async function afterMount(): Promise<void> {
  for (const q of ["(prefers-reduced-transparency: reduce)", "(prefers-contrast: more)", "(prefers-color-scheme: dark)"]) {
    matchMedia(q).addEventListener("change", () => { void repaint(); });
  }
  if (!inTauri) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().onCloseRequested((event) => {
    event.preventDefault();
    if (viewId) void closeThisPopout();
    else void invoke("quit");
  });
}

/** Appearance as this window applied it; the harness checks every window's entry. */
export function appliedState() {
  const css = getComputedStyle(document.documentElement);
  return {
    label: windowLabel,
    revision: model().revision,
    settings: effectiveSettings(),
    theme: paintedTheme,
    chromeAlpha: Number(css.getPropertyValue("--window-alpha-chrome")),
    contentAlpha: Number(css.getPropertyValue("--window-alpha-content")),
    nativeAlpha: native.alpha,
    vibrancy: native.vibrancy,
    expected: lastAppearance?.native ?? null,
    accessibilityOpaque: accessibilityOpaque(),
    error: nativeError(),
  };
}

// -- settings ------------------------------------------------------------------------------

const sendSettings = coalescing<Partial<AppearanceSettings>>(
  async (patch) => {
    try {
      accept(await invoke<WindowModel>("shell_patch", { view: null, patch }));
    } catch (e) {
      setNativeError(String(e));
    }
  },
  () => {
    setSettingsOverlay(null);
    void repaint();
  },
);

/** Optimistic and coalesced: the UI follows the slider at once; Rust gets at most one patch in flight. */
export function patchSettings(patch: Partial<AppearanceSettings>): Promise<void> {
  if (!inTauri) {
    const settings = { ...model().settings, ...patch };
    try { localStorage.setItem("atrium.calm.settings", JSON.stringify(settings)); } catch { /* not remembered */ }
    setModel({ ...model(), revision: model().revision + 1, settings });
    return repaint();
  }
  setSettingsOverlay((o) => ({ ...(o ?? {}), ...patch }));
  void repaint();
  return sendSettings(patch);
}

// -- views ---------------------------------------------------------------------------------

export const ownsView = (view: ViewId) => model().views[view].owner === windowLabel;
export const canEdit = (view: ViewId) => ownsView(view) && moving() !== view;

export function patchView(view: ViewId, patch: Partial<ViewState>): Promise<void> {
  if (!canEdit(view)) return Promise.resolve();
  if (!inTauri) {
    setModel({ ...model(), revision: model().revision + 1, views: { ...model().views, [view]: { ...model().views[view], ...patch } } });
    return Promise.resolve();
  }
  const item = { view, patch };
  pending.push(item);
  setModel((m) => ({ ...m, views: { ...m.views, [view]: { ...m.views[view], ...patch } } }));
  return writes.enqueue(async () => {
    try {
      accept(await invoke<WindowModel>("shell_patch", { view, patch }));
    } finally {
      pending.splice(pending.indexOf(item), 1);
    }
  });
}

export const persistScroll = debounced((scroll: number) => { void patchView("chat", { scroll }); }, SCROLL_PERSIST_MS);

async function flushEdits(): Promise<void> {
  persistScroll.flush();
  await writes.flush();
}

export async function popOut(view: ViewId): Promise<void> {
  if (!inTauri) { setNativeError("Pop-out windows need the native app."); return; }
  setMoving(view);
  try {
    await flushEdits();
    accept(await invoke<WindowModel>("pop_out", { view }));
  } catch (e) {
    setNativeError(String(e));
  } finally {
    setMoving(null);
  }
}

/** Pop-out side: flush, hand the view back, close. Always closes (review #6). */
async function closeThisPopout(): Promise<void> {
  if (!viewId) return;
  const id = viewId;
  await closeOwnedView({
    flush: flushEdits,
    bringBack: () => invoke("bring_back", { view: id }),
    destroy: async () => {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow().destroy();
    },
  });
}

/** From either side: the pop-out closes itself; main asks politely, then forces after a timeout. */
export async function bringBack(view: ViewId): Promise<void> {
  if (!inTauri) return;
  if (viewId === view) return closeThisPopout();
  const owner = model().views[view].owner;
  if (owner === "main") return;
  await bringBackFromMain({
    ask: () => emitTo(owner, "return-view"),
    returned: () => model().views[view].owner === "main",
    force: async () => accept(await invoke<WindowModel>("force_bring_back", { view })),
    wait: (ms) => new Promise((r) => setTimeout(r, ms)),
  }, BRING_BACK_TIMEOUT_MS, BRING_BACK_POLL_MS);
}

export async function focusPopout(view: ViewId): Promise<void> {
  if (!inTauri) return;
  const { Window } = await import("@tauri-apps/api/window");
  const w = await Window.getByLabel(`view-${view}`);
  await w?.setFocus();
}

export async function setKeepOnTop(on: boolean): Promise<void> {
  if (!inTauri) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().setAlwaysOnTop(on);
}
