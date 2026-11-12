// Glue between the UI and the Rust single writer (src-tauri/src/shell.rs): the shared model,
// optimistic view edits, settings patches, appearance painting and the pop-out lifecycle.
// The ordering rules live in sync.ts (pure, tested); this file wires them to Tauri.

import { createSignal } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { emitTo, listen } from "@tauri-apps/api/event";
import { inTauri, setWindowTheme, type StartupInfo } from "./platform";
import { applyTheme } from "./theme/runtime";
import { calmTheme } from "./theme/calm";
import { EditGate, WriteQueue, bringBackFromMain, coalescing, debounced, latestWins, makeCloser, prepareMove } from "./sync";
import { DEFAULT_SETTINGS, EMPTY_VIEW, MAIN, appearanceFor, initialModel, nativeCalls, ownerOf, type Appearance, type AppearanceSettings, type NativeShown, type ViewState, type WindowAppearance, type WindowModel } from "./window-model";

/** This window's label and, for a pop-out, the thread it shows. Set once from the startup info. */
export const [windowLabel, setWindowLabel] = createSignal(MAIN);
export const [popoutThread, setPopoutThread] = createSignal<string | null>(null);
export const isPopout = () => windowLabel() !== MAIN;
export const [platform, setPlatform] = createSignal<"macos" | "linux">("linux");
export const [debugBuild, setDebugBuild] = createSignal(false);

export const [model, setModel] = createSignal(initialModel());
export const [nativeSupported, setNativeSupported] = createSignal(false);
export const [accessibilityOpaque, setAccessibilityOpaque] = createSignal(false);
export const [nativeError, setNativeError] = createSignal("");
/** The view being moved to another window: read-only until the move finishes. */
export const [moving, setMoving] = createSignal<string | null>(null);

/** How long main waits for a pop-out to hand its view back before forcing it. */
const BRING_BACK_TIMEOUT_MS = 1500;
const BRING_BACK_POLL_MS = 50;
/** Scroll position is state too, but only needs to land before a move, not every frame. */
const SCROLL_PERSIST_MS = 150;

const writes = new WriteQueue((e) => setNativeError(String(e)));
const pending: { key: string; patch: Partial<ViewState> }[] = [];
/** Settings sent to Rust but not yet confirmed; overlaid so a slider never jumps back. */
const [settingsOverlay, setSettingsOverlay] = createSignal<Partial<AppearanceSettings> | null>(null);

/** What the native window currently shows, so paint only calls native APIs on a change. */
const native: NativeShown = { vibrancy: false, dark: false, alpha: 1, appearance: "" };
let paintedTheme = "";
let lastAppearance: Appearance | null = null;

const systemDark = () => matchMedia("(prefers-color-scheme: dark)").matches;
const mediaReduce = () => matchMedia("(prefers-reduced-transparency: reduce)").matches || matchMedia("(prefers-contrast: more)").matches;
let osReduce = false;

export const effectiveSettings = (): AppearanceSettings => ({ ...model().settings, ...(settingsOverlay() ?? {}) });

function currentAppearance(): Appearance {
  return appearanceFor({ settings: effectiveSettings(), supported: nativeSupported(), reduce: osReduce || mediaReduce(), popout: isPopout(), systemDark: systemDark() });
}

/** The native window follows the OS for "system", so prefers-color-scheme keeps tracking it. */
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
  const need = nativeCalls(native, { vibrancy: a.native.vibrancy, dark: a.native.appearance === "dark", alpha: a.native.alpha, appearance: windowAppearance(a) });
  if (need.appearance) {
    const appearance = need.appearance;
    calls.push(setWindowTheme(appearance).then(() => { native.appearance = appearance; }));
  }
  if (need.vibrancy) {
    const { enabled, dark } = need.vibrancy;
    calls.push(invoke<boolean>("set_vibrancy", { enabled, dark }).then((v) => { native.vibrancy = v; native.dark = dark; }));
  }
  if (need.alpha !== undefined) {
    calls.push(invoke<number>("set_window_alpha", { alpha: need.alpha }).then((v) => { native.alpha = v; }));
  }
  const results = await Promise.allSettled(calls);
  const failed = results.find((r): r is PromiseRejectedResult => r.status === "rejected");
  document.documentElement.dataset.vibrancy = String(native.vibrancy);
  document.documentElement.dataset.nativeAlpha = String(native.alpha);
  if (failed) throw failed.reason;
  if (a.native.vibrancy && !native.vibrancy) {
    // Blur was asked for and the platform refused: draw opaque surfaces instead of see-through ones.
    paintTokens(appearanceFor({ settings: effectiveSettings(), supported: false, reduce: true, popout: isPopout(), systemDark: systemDark() }));
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
  for (const item of pending) views[item.key] = { ...(views[item.key] ?? EMPTY_VIEW), ...item.patch };
  const before = JSON.stringify(effectiveSettings());
  setModel({ ...next, views });
  if (JSON.stringify(effectiveSettings()) !== before) void repaint();
}

/**
 * First paint, synchronously from the startup snapshot (no IPC between here and render).
 * `info.native` is what Rust already applied before the webview loaded.
 */
export function initFromStartup(info: StartupInfo): void {
  setWindowLabel(info.label);
  setPopoutThread(info.popoutThread);
  setPlatform(info.platform);
  setDebugBuild(info.debug);
  setNativeSupported(info.translucency);
  osReduce = info.accessibilityOpaque;
  native.vibrancy = info.native.vibrancy;
  native.dark = info.native.dark;
  native.alpha = info.native.alpha;
  document.documentElement.dataset.platform = info.platform;
  if (info.snapshot) setModel(info.snapshot);
  else {
    try {
      const saved = localStorage.getItem("atrium.settings");
      if (saved) setModel({ ...model(), settings: { ...DEFAULT_SETTINGS, ...JSON.parse(saved) } });
    } catch { /* defaults */ }
  }
  // Rust set the window theme when it built the window.
  native.appearance = windowAppearance(currentAppearance());
  paintTokens(currentAppearance());
}

/** Subscribes before the snapshot is read (the revision guard drops anything older). */
export async function subscribe(): Promise<void> {
  if (!inTauri) return;
  await listen<WindowModel>("shell-state", (e) => accept(e.payload));
  await listen<boolean>("accessibility-changed", (e) => { osReduce = e.payload; void repaint(); });
  await listen("return-view", () => { if (isPopout() && answerReturnRequests) void closeThisPopout(); });
}

/** Wires the OS-preference listeners and the close handler. Runs after the first render. */
export async function afterMount(): Promise<void> {
  for (const q of ["(prefers-reduced-transparency: reduce)", "(prefers-contrast: more)", "(prefers-color-scheme: dark)"]) {
    matchMedia(q).addEventListener("change", () => { void repaint(); });
  }
  if (!inTauri || !isPopout()) return;
  // A pop-out sends its last edits and hands its view back before it closes. The main
  // window's close is Rust's to decide (hide to the tray, or quit).
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().onCloseRequested((event) => {
    event.preventDefault();
    void closeThisPopout();
  });
}

/** Appearance as this window applied it; the harness checks every window's entry. */
export function appliedState() {
  const css = getComputedStyle(document.documentElement);
  return {
    label: windowLabel(),
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
      accept(await invoke<WindowModel>("shell_patch_settings", { patch }));
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
    try { localStorage.setItem("atrium.settings", JSON.stringify(settings)); } catch { /* not remembered */ }
    setModel({ ...model(), revision: model().revision + 1, settings });
    return repaint();
  }
  setSettingsOverlay((o) => ({ ...(o ?? {}), ...patch }));
  void repaint();
  return sendSettings(patch);
}

// -- views ---------------------------------------------------------------------------------

export const viewOf = (key: string): ViewState => model().views[key] ?? EMPTY_VIEW;
export const ownsView = (key: string) => ownerOf(model(), key) === windowLabel();
/** Refuses new user edits to a view while it moves to another window (synchronous). */
const gate = new EditGate<string>();
export const canEdit = (key: string) => ownsView(key) && moving() !== key;

function unfreeze(key: string): void {
  gate.unblock(key);
  setMoving(null);
}

/**
 * A view edit (its draft, its scroll position). User edits are refused while the view is
 * moving; `buffered` marks state that was captured before the move started (the debounced
 * scroll position), which must still land.
 */
export function patchView(key: string, patch: Partial<Pick<ViewState, "draft" | "scroll">>, buffered = false): Promise<void> {
  if (!ownsView(key) || !gate.allows(key, buffered)) return Promise.resolve();
  const item = { key, patch };
  const applyLocal = () => setModel((m) => ({ ...m, views: { ...m.views, [key]: { ...(m.views[key] ?? EMPTY_VIEW), ...patch } } }));
  if (!inTauri) {
    applyLocal();
    return Promise.resolve();
  }
  pending.push(item);
  applyLocal();
  return writes.enqueue(async () => {
    try {
      const revision = await invoke<number>("shell_patch_view", { key, patch });
      setModel((m) => (revision > m.revision ? { ...m, revision } : m));
    } finally {
      pending.splice(pending.indexOf(item), 1);
    }
  });
}

/** The scroll position of the view this window is showing, written a little after scrolling stops. */
export const persistScroll = debounced(({ key, scroll }: { key: string; scroll: number | null }) => { void patchView(key, { scroll }, true); }, SCROLL_PERSIST_MS);

/** The thread the main window shows (null: the new-chat view). */
export function setMainThread(threadId: string | null): void {
  if (isPopout() || model().mainThread === threadId) return;
  setModel((m) => ({ ...m, mainThread: threadId }));
  if (!inTauri) return;
  void writes.enqueue(async () => {
    const revision = await invoke<number>("shell_set_main_thread", { threadId });
    setModel((m) => (revision > m.revision ? { ...m, revision } : m));
  });
}

/** Freeze user edits, flush the buffered scroll position, drain every accepted write. */
function prepare(key: string): Promise<void> {
  setMoving(key);
  return prepareMove(gate, key, () => persistScroll.flush(), writes);
}

/** Moves a thread's view into its own window. */
export async function popOut(threadId: string): Promise<void> {
  if (!inTauri) { setNativeError("Pop-out windows need the native app."); return; }
  try {
    await prepare(threadId);
    accept(await invoke<WindowModel>("pop_out", { threadId }));
  } catch (e) {
    setNativeError(String(e));
  } finally {
    unfreeze(threadId);
  }
}

/** Pop-out side: freeze, drain, hand the view back, close. Idempotent; always closes. */
const closeThisPopout = makeCloser({
  prepare: () => { const key = popoutThread(); return key ? prepare(key) : Promise.resolve(); },
  // Rust returns the view and destroys this window.
  bringBack: () => invoke("bring_back"),
  destroy: async () => {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().destroy();
  },
});

/** Pop-out side: a "return-view" request from main. Suppressible for the forced-path check. */
let answerReturnRequests = true;
export function ignoreReturnRequests(): void {
  answerReturnRequests = false;
}

/** From either side: the pop-out closes itself; main asks politely, then forces after a timeout. */
export async function bringBack(threadId: string): Promise<"returned" | "forced" | "destroyed" | "none"> {
  if (!inTauri) return "none";
  if (popoutThread() === threadId) return closeThisPopout();
  const owner = ownerOf(model(), threadId);
  if (owner === MAIN) return "none";
  return bringBackFromMain({
    ask: () => emitTo(owner, "return-view"),
    returned: () => ownerOf(model(), threadId) === MAIN,
    force: () => invoke("force_bring_back", { label: owner }),
    wait: (ms) => new Promise((r) => setTimeout(r, ms)),
  }, BRING_BACK_TIMEOUT_MS, BRING_BACK_POLL_MS);
}

export async function focusPopout(threadId: string): Promise<void> {
  const owner = ownerOf(model(), threadId);
  if (inTauri && owner !== MAIN) await invoke("focus_popout", { label: owner });
}

export async function setKeepOnTop(on: boolean): Promise<void> {
  if (!inTauri) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().setAlwaysOnTop(on);
}
