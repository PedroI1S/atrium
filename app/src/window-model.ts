// The state every window shares, as Rust publishes it (src-tauri/src/shell.rs is the single
// writer), and the pure opacity rules. This file only describes the shape and derives tokens.

export type ThemeChoice = "light" | "dark" | "system";
export type CalmTheme = "atrium-calm-light" | "atrium-calm-dark";
export type OpacityMode = "background" | "whole";
export type PopoutOpacity = "same" | "opaque";
export type TextSize = "small" | "default" | "large";

export interface AppearanceSettings {
  theme: ThemeChoice;
  alpha: number;
  mode: OpacityMode;
  /** Keep reading areas solid: only the frame fades. */
  solid: boolean;
  blur: boolean;
  /** Blur of in-window overlays, in px; null uses the theme's `glass-blur`. */
  blurStrength: number | null;
  /** Popped-out windows follow the main window ("same") or stay opaque. */
  popouts: PopoutOpacity;
  textSize: TextSize;
}

export interface Bounds { x: number; y: number; width: number; height: number }
export interface Popout { threadId: string; bounds: Bounds | null }
export interface ViewState { draft: string; scroll: number | null; touched?: number }
export interface WindowModel {
  revision: number;
  settings: AppearanceSettings;
  /** By window label. */
  popouts: Record<string, Popout>;
  /** By thread id, or NEW_THREAD. */
  views: Record<string, ViewState>;
  mainThread: string | null;
}

export const MAIN = "main";
/** The view key of the composer for a thread that doesn't exist yet. */
export const NEW_THREAD = "@new";
export const DEFAULT_SETTINGS: AppearanceSettings = { theme: "system", alpha: 1, mode: "background", solid: false, blur: true, blurStrength: null, popouts: "same", textSize: "default" };
export const EMPTY_VIEW: ViewState = { draft: "", scroll: null };

export const initialModel = (): WindowModel => ({ revision: 0, settings: { ...DEFAULT_SETTINGS }, popouts: {}, views: {}, mainThread: null });

/** The label of the window that shows `key` (mirrors `ShellState::owner`). */
export function ownerOf(model: WindowModel, key: string): string {
  for (const [label, popout] of Object.entries(model.popouts)) if (popout.threadId === key) return label;
  return MAIN;
}

/** Opacity slider range (ui-direction §5): the lower bound keeps text readable. */
export const MIN_ALPHA = 0.3;
export const MAX_BLUR_STRENGTH = 48;
/** Background mode: cards = a + (1 − a) × READING_AREA_LIFT, so reading areas stay more solid. */
export const READING_AREA_LIFT = 0.35;
/** The whole interface scales with the text size setting. */
export const TEXT_SCALE: Record<TextSize, number> = { small: 0.92, default: 1, large: 1.1 };
/** Token values are rounded so the emitted CSS and the harness checks compare exactly. */
const DECIMALS = 4;
const round = (x: number) => Number(x.toFixed(DECIMALS));

export interface AppearanceInput {
  settings: AppearanceSettings;
  /** The platform can draw a translucent window at all (macOS; Linux windows are opaque). */
  supported: boolean;
  /** Reduce transparency / Increase contrast (OS) forces opaque surfaces. */
  reduce: boolean;
  /** This window is a pop-out (it may be pinned opaque by `popouts`). */
  popout: boolean;
  systemDark: boolean;
}

export type RuntimeToken = "--window-alpha-chrome" | "--window-alpha-content" | "--window-opacity" | "--window-blur" | "--ui-scale";

export interface Appearance {
  theme: CalmTheme;
  /** Runtime tokens (contract §11.1), written to :root. Never set by a theme. */
  tokens: Record<RuntimeToken, string>;
  /** What the native window must show: vibrancy on/off and the window alpha. */
  native: { vibrancy: boolean; alpha: number; appearance: "light" | "dark" };
  /** Effective alpha after platform, accessibility and pop-out rules. */
  alpha: number;
}

export function resolveTheme(choice: ThemeChoice, systemDark: boolean): CalmTheme {
  const dark = choice === "system" ? systemDark : choice === "dark";
  return dark ? "atrium-calm-dark" : "atrium-calm-light";
}

/** The single source of the opacity rules (Rust mirrors the native half in `native_target`). */
export function appearanceFor(input: AppearanceInput): Appearance {
  const s = input.settings;
  const opaque = !input.supported || input.reduce || (input.popout && s.popouts === "opaque");
  const raw = Number.isFinite(s.alpha) ? s.alpha : 1;
  const a = opaque ? 1 : Math.min(1, Math.max(MIN_ALPHA, raw));
  const background = s.mode === "background";
  const theme = resolveTheme(s.theme, input.systemDark);
  const strength = s.blurStrength === null ? null : Math.min(MAX_BLUR_STRENGTH, Math.max(0, Math.round(s.blurStrength)));
  return {
    theme,
    alpha: a,
    tokens: {
      "--window-alpha-chrome": String(background ? round(a) : 1),
      "--window-alpha-content": String(background && !s.solid ? round(a + (1 - a) * READING_AREA_LIFT) : 1),
      // Passed to the native window alpha; never used as CSS opacity (contract §11.1).
      "--window-opacity": String(background ? 1 : round(a)),
      // In-window overlays only (palette, popovers). The blur behind the window is native.
      "--window-blur": strength === null ? "var(--glass-blur)" : `${strength}px`,
      "--ui-scale": String(TEXT_SCALE[s.textSize] ?? 1),
    },
    native: { vibrancy: background && a < 1 && s.blur, alpha: background ? 1 : round(a), appearance: theme.endsWith("dark") ? "dark" : "light" },
  };
}

/** The native window's appearance; "system" clears the override so it follows the OS. */
export type WindowAppearance = "light" | "dark" | "system";

/** What a window's native side currently shows. `dark` is the vibrancy material's input. */
export interface NativeShown { vibrancy: boolean; dark: boolean; alpha: number; appearance: "" | WindowAppearance }
export interface NativeTarget { vibrancy: boolean; dark: boolean; alpha: number; appearance: WindowAppearance }

/**
 * The native calls needed to go from what is shown to the target. Vibrancy is re-applied when it
 * turns on or off, and also when it stays on but the theme flips (the material depends on
 * light/dark).
 */
export function nativeCalls(shown: NativeShown, target: NativeTarget): { appearance?: WindowAppearance; vibrancy?: { enabled: boolean; dark: boolean }; alpha?: number } {
  const calls: ReturnType<typeof nativeCalls> = {};
  if (target.appearance !== shown.appearance) calls.appearance = target.appearance;
  if (target.vibrancy !== shown.vibrancy || (target.vibrancy && target.dark !== shown.dark)) calls.vibrancy = { enabled: target.vibrancy, dark: target.dark };
  if (target.alpha !== shown.alpha) calls.alpha = target.alpha;
  return calls;
}
