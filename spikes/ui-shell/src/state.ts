// Window-local UI state (plain Solid signals): which screen the main card shows, the topic,
// the sidebar and panel, settings navigation. Shared, persisted state lives in windows.ts.

import { createSignal } from "solid-js";
import { type PanelTab, type Screen, panelTabs } from "./layout";

export const [screen, setScreenSignal] = createSignal<Screen>("chat");
export const [topicId, setTopicId] = createSignal("auth");
export const [sidebarOpen, setSidebarOpen] = createSignal(true);

/** Panel open/minimized and its tab are remembered per main-card kind (ui-direction §2.2). */
const [panelMemory, setPanelMemory] = createSignal<Partial<Record<Screen, { open: boolean; tab: PanelTab }>>>({});
const memo = (s: Screen) => panelMemory()[s] ?? { open: true, tab: panelTabs(s).initial };
export const panelOpen = () => memo(screen()).open;
export const panelTab = () => memo(screen()).tab;
export function setPanel(next: { open?: boolean; tab?: PanelTab }): void {
  const s = screen();
  setPanelMemory((m) => ({ ...m, [s]: { ...memo(s), ...next } }));
}

/** User-dragged widths in px; null means the theme's token width. */
export const [sidebarWidth, setSidebarWidth] = createSignal<number | null>(null);
export const [panelWidth, setPanelWidth] = createSignal<number | null>(null);

export type SettingsSection = "general" | "appearance" | "agents" | "approvals" | "notifications" | "more";
export const [settingsSection, setSettingsSectionSignal] = createSignal<SettingsSection>("appearance");
export const [advancedOpen, setAdvancedOpen] = createSignal(false);
export const [settingsQuery, setSettingsQuery] = createSignal<string | null>(null);
/** Row to briefly highlight after a settings search jump. */
export const [highlightRow, setHighlightRow] = createSignal<string | null>(null);

/** Leaving a section (or settings) closes its Advanced card (ui-direction §3 Settings). */
export function setSettingsSection(s: SettingsSection): void {
  if (s !== settingsSection()) setAdvancedOpen(false);
  setSettingsSectionSignal(s);
}
export function setScreen(s: Screen): void {
  if (s !== "settings") { setAdvancedOpen(false); setSettingsQuery(null); }
  setScreenSignal(s);
}

export const [inboxPick, setInboxPick] = createSignal("i-run");
export const [inboxFilter, setInboxFilter] = createSignal<"all" | "approvals" | "reviews">("all");

export const [paletteOpen, setPaletteOpen] = createSignal(false);
export const [keepOnTop, setKeepOnTopSignal] = createSignal(false);
export const [toast, setToast] = createSignal<{ id: number; text: string } | null>(null);
export const [benchOverlay, setBenchOverlay] = createSignal<string | null>(null);

/** How long a toast stays up. */
const TOAST_MS = 2600;
let toastSeq = 0;
let toastTimer: ReturnType<typeof setTimeout> | undefined;
export function showToast(text: string): void {
  const id = ++toastSeq;
  setToast({ id, text });
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => setToast((t) => (t?.id === id ? null : t)), TOAST_MS);
}
