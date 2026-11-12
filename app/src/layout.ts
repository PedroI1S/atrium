// Pure layout rules for layout B (ui-direction §2): what occupies the right column for each
// main card, and which panel tabs it offers. Tested in layout.test.ts.

/** What the main card shows. M1 has chats (all Home threads), the inbox and settings. */
export type Screen = "chat" | "inbox" | "settings";
/** M1's panel has one tab; the others arrive with workspaces (M2) and the browser (M3). */
export type PanelTab = "details";

/** What the right column shows. Widths are token names, so a theme can change them. */
export type RightColumn =
  | { kind: "panel"; width: "--panel-width" }
  | { kind: "strip"; width: "--panel-strip-width" }
  | { kind: "advanced"; width: "--advanced-width" }
  | { kind: "none" };

/**
 * Settings never shows the context panel: the Advanced card takes its place at its own width,
 * whether or not the panel was minimized.
 */
export function rightColumn(screen: Screen, panelOpen: boolean, advancedOpen: boolean): RightColumn {
  if (screen === "settings") return advancedOpen ? { kind: "advanced", width: "--advanced-width" } : { kind: "none" };
  if (!panelOpen) return { kind: "strip", width: "--panel-strip-width" };
  return { kind: "panel", width: "--panel-width" };
}

/** Panel tabs per main card (ui-direction §2.2) and the default tab. */
export function panelTabs(_screen: Screen): { tabs: PanelTab[]; initial: PanelTab } {
  return { tabs: ["details"], initial: "details" };
}

/** Width of a column dragged by the user, kept inside its token bounds. */
export const clampWidth = (px: number, min: number, max: number): number => Math.round(Math.min(max, Math.max(min, px)));
