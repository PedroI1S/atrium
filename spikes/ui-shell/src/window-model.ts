// Serializable window state and the pure opacity rules. Rust (src-tauri/src/shell.rs) is the
// single writer in the native app; this file only describes the shape and derives tokens.

export type ViewId = "chat" | "editor" | "database";
export const VIEW_IDS: readonly ViewId[] = ["chat", "editor", "database"];
export type OpacityMode = "background" | "whole";
export type ThemeChoice = "atrium-calm-light" | "atrium-calm-dark" | "system";
export type CalmTheme = Exclude<ThemeChoice, "system">;
export type PopoutOpacity = "same" | "opaque";

export interface AppearanceSettings {
  theme: ThemeChoice;
  alpha: number;
  mode: OpacityMode;
  solid: boolean;
  blur: boolean;
  /** Popped-out windows follow the main window ("same") or stay opaque. */
  popouts: PopoutOpacity;
}
export interface ViewState { threadId: string; owner: string; draft: string; messages: string[]; scroll: number; content: string }
export interface WindowModel { revision: number; settings: AppearanceSettings; views: Record<ViewId, ViewState> }

export const DEFAULT_SETTINGS: AppearanceSettings = { theme: "atrium-calm-dark", alpha: 1, mode: "background", solid: false, blur: true, popouts: "same" };

export const initialModel = (): WindowModel => ({
  revision: 0,
  settings: { ...DEFAULT_SETTINGS },
  views: Object.fromEntries(VIEW_IDS.map((id) => [id, {
    threadId: "t-auth", owner: "main", draft: "", messages: [], scroll: -1,
    content: id === "editor" ? "// session.rs\n" : "select * from sessions;",
  }])) as unknown as WindowModel["views"],
});

/** Opacity slider range (ui-direction §5): the lower bound keeps text readable. */
export const MIN_ALPHA = 0.3;
/** Background mode: cards = a + (1 − a) × READING_AREA_LIFT, so reading areas stay more solid. */
export const READING_AREA_LIFT = 0.35;
/** Token values are rounded so the emitted CSS and the harness checks compare exactly. */
const DECIMALS = 4;
const round = (x: number) => Number(x.toFixed(DECIMALS));

export interface AppearanceInput {
  settings: AppearanceSettings;
  /** The platform can draw a translucent window at all (macOS here; Linux falls back to opaque). */
  supported: boolean;
  /** Reduce transparency / Increase contrast (OS) forces opaque surfaces. */
  reduce: boolean;
  /** This window is a pop-out (it may be pinned opaque by `popouts`). */
  popout: boolean;
  systemDark: boolean;
}

export interface Appearance {
  theme: CalmTheme;
  /** CR-02 runtime tokens, written to :root. */
  tokens: Record<"--window-alpha-chrome" | "--window-alpha-content" | "--window-opacity" | "--window-blur", string>;
  /** What the native window must show: vibrancy on/off and NSWindow alpha. */
  native: { vibrancy: boolean; alpha: number; appearance: "light" | "dark" };
  /** Effective alpha after platform, accessibility and pop-out rules. */
  alpha: number;
}

export function resolveTheme(choice: ThemeChoice, systemDark: boolean): CalmTheme {
  if (choice === "system") return systemDark ? "atrium-calm-dark" : "atrium-calm-light";
  return choice;
}

/** The single source of the opacity rules (Rust mirrors the native half in `native_target`). */
export function appearanceFor(input: AppearanceInput): Appearance {
  const s = input.settings;
  const opaque = !input.supported || input.reduce || (input.popout && s.popouts === "opaque");
  const raw = Number.isFinite(s.alpha) ? s.alpha : 1;
  const a = opaque ? 1 : Math.min(1, Math.max(MIN_ALPHA, raw));
  const background = s.mode === "background";
  const theme = resolveTheme(s.theme, input.systemDark);
  return {
    theme,
    alpha: a,
    tokens: {
      "--window-alpha-chrome": String(background ? round(a) : 1),
      "--window-alpha-content": String(background && !s.solid ? round(a + (1 - a) * READING_AREA_LIFT) : 1),
      // Passed to NSWindow.alphaValue; never used as CSS opacity (see the report, CR-02 feedback).
      "--window-opacity": String(background ? 1 : round(a)),
      // Only CSS backdrop-filter overlays read this; the desktop blur is native vibrancy.
      "--window-blur": s.blur && a < 1 ? "var(--glass-blur)" : "0",
    },
    native: { vibrancy: background && a < 1 && s.blur, alpha: background ? 1 : round(a), appearance: theme.endsWith("dark") ? "dark" : "light" },
  };
}

export function moveView(model: WindowModel, id: ViewId, owner: string): WindowModel {
  return { ...model, revision: model.revision + 1, views: { ...model.views, [id]: { ...model.views[id], owner } } };
}
