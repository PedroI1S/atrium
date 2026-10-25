// App state: plain Solid signals. The shell has no persistence beyond the chosen theme.

import { batch, createSignal } from "solid-js";
import { ALL_THREADS, INITIAL_TABS, type TabSpec, threadById } from "./data/dummy";
import { setVibrancy, setWindowTheme, vibrancySupported } from "./platform";
import type { ThemeEntry } from "./theme/registry";
import { applyTheme } from "./theme/runtime";

export type PresetId = "default" | "review" | "focus";
export type RightTabId = "review" | "diff" | "browser" | "terminal" | "database";
export const RIGHT_TABS: { id: RightTabId; label: string }[] = [
  { id: "review", label: "Review" },
  { id: "diff", label: "Diff" },
  { id: "browser", label: "Browser" },
  { id: "terminal", label: "Terminal" },
  { id: "database", label: "Database" },
];

/**
 * Panel sizes per preset. `outer` = [sidebar, main, right panel]; `chatShare` = how much of the
 * main area the chat (first pane) takes when a file preview / second thread is next to it.
 */
export const PRESETS: Record<PresetId, { label: string; outer: [number, number, number]; chatShare: number; splitShare: number; right: RightTabId | null }> = {
  default: { label: "Default", outer: [0.17, 0.5, 0.33], chatShare: 0.58, splitShare: 0.5, right: "review" },
  review: { label: "Review", outer: [0, 0.3, 0.7], chatShare: 1, splitShare: 1, right: "diff" },
  focus: { label: "Focus", outer: [0, 1, 0], chatShare: 1, splitShare: 1, right: null },
};

export const [themes, setThemes] = createSignal<ThemeEntry[]>([]);
export const [themeId, setThemeId] = createSignal("glass-dark");
export const [preset, setPresetId] = createSignal<PresetId>("default");
export const [tabs, setTabs] = createSignal<TabSpec[]>(INITIAL_TABS);
export const [activeTabId, setActiveTabId] = createSignal(INITIAL_TABS[0]!.id);
export const [rightTab, setRightTab] = createSignal<RightTabId>("review");
export const [paletteOpen, setPaletteOpen] = createSignal(false);
export const [outerSizes, setOuterSizes] = createSignal<number[]>([...PRESETS.default.outer]);
export const [innerSizes, setInnerSizes] = createSignal<number[]>([PRESETS.default.chatShare, 1 - PRESETS.default.chatShare]);
/** True for a moment after a preset switch so panel sizes animate; never during a drag. */
export const [animateLayout, setAnimateLayout] = createSignal(false);
export const [toast, setToast] = createSignal<{ id: number; text: string } | null>(null);
export const [benchOverlay, setBenchOverlay] = createSignal<string | null>(null);

export const activeTab = () => tabs().find((t) => t.id === activeTabId()) ?? tabs()[0]!;

let toastSeq = 0;
let toastTimer: number | undefined;
export function showToast(text: string): void {
  const id = ++toastSeq;
  setToast({ id, text });
  window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => setToast((t) => (t?.id === id ? null : t)), 2600);
}

// -- themes -----------------------------------------------------------------

export function activateTheme(id: string, persist = true): void {
  const entry = themes().find((t) => t.id === id);
  if (!entry) {
    showToast(`Unknown theme ${id}`);
    return;
  }
  const requested = entry.theme.meta.vibrancy;
  applyTheme(entry.theme, requested && vibrancySupported());
  setThemeId(id);
  void setWindowTheme(entry.theme.meta.appearance);
  void setVibrancy(requested, entry.theme.meta.appearance === "dark").then((applied) => {
    // Requested but not applied (unsupported platform, or the native call failed): fall back to opaque.
    // Ignore a stale answer if the theme changed meanwhile.
    if (requested && !applied && themeId() === id) document.documentElement.setAttribute("data-vibrancy", "false");
  });
  if (!persist) return;
  try {
    localStorage.setItem("atrium.theme", id);
  } catch {
    /* storage unavailable: fine, the choice just isn't remembered */
  }
}

export function counterpartTheme(id: string): string | null {
  if (id.endsWith("-dark")) return id.replace(/-dark$/, "-light");
  if (id.endsWith("-light")) return id.replace(/-light$/, "-dark");
  return null;
}

export function toggleAppearance(): void {
  const other = counterpartTheme(themeId());
  if (other && themes().some((t) => t.id === other)) activateTheme(other);
  else showToast("This theme has no light/dark counterpart");
}

// -- layout -----------------------------------------------------------------

/** Layout animation lasts `--motion-normal` (plus a frame of slack); read from the active theme. */
function layoutAnimationMs(): number {
  const raw = getComputedStyle(document.documentElement).getPropertyValue("--motion-normal").trim();
  const n = Number.parseFloat(raw);
  const ms = raw.endsWith("ms") ? n : raw.endsWith("s") ? n * 1000 : 220;
  return (Number.isFinite(ms) ? ms : 220) + 40;
}

let animTimer: number | undefined;
export function applyPreset(id: PresetId, silent = false): void {
  const p = PRESETS[id];
  const split = activeTab().kind === "split";
  const share = split ? p.splitShare : p.chatShare;
  setAnimateLayout(true);
  window.clearTimeout(animTimer);
  animTimer = window.setTimeout(() => setAnimateLayout(false), layoutAnimationMs());
  batch(() => {
    setPresetId(id);
    setOuterSizes([...p.outer]);
    setInnerSizes([share, 1 - share]);
    if (p.right) setRightTab(p.right);
  });
  if (!silent) showToast(`Layout: ${p.label}`);
}

export function toggleSidebar(): void {
  const [s, m, r] = outerSizes() as [number, number, number];
  setAnimateLayout(true);
  window.clearTimeout(animTimer);
  animTimer = window.setTimeout(() => setAnimateLayout(false), layoutAnimationMs());
  if (s > 0.01) setOuterSizes([0, m + s, r]);
  else setOuterSizes([0.17, Math.max(0.3, m - 0.17), r]);
}

export function toggleRightPanel(): void {
  const [s, m, r] = outerSizes() as [number, number, number];
  setAnimateLayout(true);
  window.clearTimeout(animTimer);
  animTimer = window.setTimeout(() => setAnimateLayout(false), layoutAnimationMs());
  if (r > 0.01) setOuterSizes([s, m + r, 0]);
  else setOuterSizes([s, Math.max(0.3, m - 0.33), 0.33]);
}

// -- tabs and attention -----------------------------------------------------

export function openThread(threadId: string): void {
  const existing = tabs().find((t) => t.threads.includes(threadId) && t.kind !== "home");
  if (existing) {
    setActiveTabId(existing.id);
    return;
  }
  const th = threadById(threadId);
  const tab: TabSpec = { id: `tab-${threadId}`, title: th.title, kind: "thread", threads: [threadId], file: "session.rs" };
  batch(() => {
    setTabs((ts) => [...ts, tab]);
    setActiveTabId(tab.id);
  });
}

/** Activates the Home tab, recreating it if it was closed. */
export function openHome(): void {
  const existing = tabs().find((t) => t.kind === "home");
  if (existing) {
    setActiveTabId(existing.id);
    return;
  }
  const home = INITIAL_TABS.find((t) => t.kind === "home")!;
  batch(() => {
    setTabs((ts) => [...ts, home]);
    setActiveTabId(home.id);
  });
}

export const attentionThreads = () => ALL_THREADS.filter((t) => t.state === "needs-you" || t.state === "failed" || t.state === "ready");
export const inboxCount = () => attentionThreads().length;

let attentionCursor = -1;
/** Cmd+J: jump to the next thread that needs a human. Dummy ordering: needs-you, failed, ready. */
export function nextAttention(): void {
  const rank = { "needs-you": 0, failed: 1, ready: 2, running: 3, idle: 4 } as const;
  const queue = [...attentionThreads()].sort((a, b) => rank[a.state] - rank[b.state]);
  if (!queue.length) {
    showToast("Nothing needs you");
    return;
  }
  attentionCursor = (attentionCursor + 1) % queue.length;
  const t = queue[attentionCursor]!;
  openThread(t.id);
  showToast(`Next: ${t.title} (${t.state.replace("-", " ")})`);
}
