// Pure layout rules for layout B (ui-direction §2): what occupies the right column for each
// main card, and which panel tabs it offers. Tested in layout.test.ts.

export type Screen = "chat" | "inbox" | "editor" | "database" | "settings";
export type PanelTab = "changes" | "checks" | "files" | "terminal" | "chat" | "preview";

/** What the right column shows. Widths are token names, so a theme can change them. */
export type RightColumn =
  | { kind: "panel"; width: "--panel-width" | "--panel-width-editor" }
  | { kind: "strip"; width: "--panel-strip-width" }
  | { kind: "advanced"; width: "--advanced-width" }
  | { kind: "none" };

/**
 * Settings never shows the context panel: the Advanced card takes its place at its own width,
 * whether or not the panel was minimized (review #9: it used to open in the 46 px strip).
 */
export function rightColumn(screen: Screen, panelOpen: boolean, advancedOpen: boolean): RightColumn {
  if (screen === "settings") return advancedOpen ? { kind: "advanced", width: "--advanced-width" } : { kind: "none" };
  if (!panelOpen) return { kind: "strip", width: "--panel-strip-width" };
  return { kind: "panel", width: screen === "editor" ? "--panel-width-editor" : "--panel-width" };
}

/** Panel tabs per main card (ui-direction §2.2) and the default tab. */
export function panelTabs(screen: Screen): { tabs: PanelTab[]; initial: PanelTab } {
  switch (screen) {
    case "editor": return { tabs: ["chat", "files", "changes"], initial: "chat" };
    case "inbox": return { tabs: ["preview"], initial: "preview" };
    case "settings": return { tabs: [], initial: "changes" };
    default: return { tabs: ["changes", "checks", "files", "terminal"], initial: "changes" };
  }
}

/** Width of a column dragged by the user, kept inside its token bounds. */
export const clampWidth = (px: number, min: number, max: number): number => Math.round(Math.min(max, Math.max(min, px)));
