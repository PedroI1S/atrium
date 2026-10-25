// VS Code theme import: `colors` map onto Atrium semantic tokens, `tokenColors` onto the
// `syntax-*` component tokens. The mapping tables below are the single source of truth;
// the spike report prints them (docs/spikes/M0-06-ui-shell.md).

import { type Rgba, contrastRatio, composite, mix, parseColor, toHex, withAlpha } from "./color";
import { AA_NORMAL, type Appearance, type ThemeFile, themeFileFromObject } from "./loader";

export interface VscodeTokenRule {
  scope?: string | string[];
  settings?: { foreground?: string; fontStyle?: string };
}
export interface VscodeTheme {
  name?: string;
  displayName?: string;
  type?: string;
  colors?: Record<string, string>;
  tokenColors?: VscodeTokenRule[];
}

/** Semantic token to ordered candidate VS Code `colors` keys (first one present wins). */
export const SEMANTIC_MAP: Readonly<Record<string, readonly string[]>> = {
  "surface-0": ["titleBar.activeBackground", "activityBar.background", "sideBar.background", "editor.background"],
  "surface-1": ["editor.background"],
  "surface-2": ["editorWidget.background", "dropdown.background", "input.background", "list.hoverBackground"],
  "surface-overlay": ["quickInput.background", "editorSuggestWidget.background", "editorWidget.background"],
  "text": ["foreground", "editor.foreground"],
  "text-muted": ["descriptionForeground", "sideBarTitle.foreground", "tab.inactiveForeground"],
  "text-faint": ["disabledForeground", "editorLineNumber.foreground", "tab.inactiveForeground"],
  "text-on-accent": ["button.foreground"],
  "border": ["panel.border", "editorGroup.border", "contrastBorder"],
  "border-strong": ["input.border", "dropdown.border", "focusBorder"],
  "focus-ring": ["focusBorder"],
  "accent": ["button.background", "focusBorder", "activityBarBadge.background"],
  "accent-hover": ["button.hoverBackground"],
  "accent-muted": ["list.activeSelectionBackground", "editor.selectionBackground"],
  "danger": ["errorForeground", "editorError.foreground", "terminal.ansiRed"],
  "warning": ["editorWarning.foreground", "list.warningForeground", "terminal.ansiYellow"],
  "success": ["gitDecoration.addedResourceForeground", "terminal.ansiGreen", "charts.green"],
  "info": ["editorInfo.foreground", "terminal.ansiBlue", "charts.blue"],
  "code-bg": ["textCodeBlock.background", "editor.background"],
  "diff-add": ["diffEditor.insertedLineBackground", "diffEditor.insertedTextBackground"],
  "diff-del": ["diffEditor.removedLineBackground", "diffEditor.removedTextBackground"],
  "diff-add-text": ["gitDecoration.addedResourceForeground", "terminal.ansiGreen"],
  "diff-del-text": ["gitDecoration.deletedResourceForeground", "terminal.ansiRed"],
  "ghost-text": ["editorGhostText.foreground", "editorLineNumber.foreground"],
};

/** Derived tokens (no direct VS Code counterpart): how each is computed. */
export const DERIVED_TOKENS: Readonly<Record<string, string>> = {
  "surface-glass": "surface-0 at 72% alpha",
  "state-running": "= info",
  "state-needs-you": "= warning",
  "state-failed": "= danger",
  "state-ready": "= success",
  "space-unit, radius, font-ui, font-code, text-ui, text-code, motion-*": "Atrium defaults (a VS Code theme carries no density or motion)",
};

/** `syntax-*` component token to ordered candidate TextMate scopes (first scope found wins). */
export const SYNTAX_MAP: Readonly<Record<string, readonly string[]>> = {
  "syntax-keyword": ["keyword", "storage.type", "storage"],
  "syntax-string": ["string"],
  "syntax-number": ["constant.numeric", "constant"],
  "syntax-comment": ["comment"],
  "syntax-function": ["entity.name.function", "support.function", "meta.function-call"],
  "syntax-type": ["entity.name.type", "support.type", "entity.name.class"],
  "syntax-variable": ["variable", "source"],
  "syntax-property": ["variable.other.property", "support.type.property-name", "meta.object-literal.key"],
  "syntax-punctuation": ["punctuation", "meta.brace"],
};

function firstColor(colors: Record<string, string>, keys: readonly string[]): { value: Rgba; key: string } | null {
  for (const k of keys) {
    const raw = colors[k];
    const c = raw ? parseColor(raw) : null;
    // A fully transparent colour (e.g. "#00000000" borders) carries no information.
    if (c && c.a > 0) return { value: c, key: k };
  }
  return null;
}

/** Picks the most specific rule whose scope selector is a prefix of `target`. */
export function findTokenColor(rules: VscodeTokenRule[], target: string): string | null {
  let best: { len: number; color: string } | null = null;
  for (const rule of rules) {
    const fg = rule.settings?.foreground;
    if (!fg) continue;
    const scopes = Array.isArray(rule.scope) ? rule.scope : (rule.scope ?? "").split(",");
    for (const raw of scopes) {
      const s = raw.trim();
      if (!s) continue;
      if (target === s || target.startsWith(`${s}.`)) {
        if (!best || s.length > best.len) best = { len: s.length, color: fg };
      }
    }
  }
  return best?.color ?? null;
}

export interface ImportResult {
  file: ThemeFile;
  /** Semantic tokens taken straight from a VS Code key: token to key used. */
  mapped: Record<string, string>;
  /** Tokens that fell back to a derived value because no candidate key existed. */
  derived: string[];
  /** Colours nudged to reach WCAG AA, with before and after. */
  adjustments: string[];
}

const BASE_PRIMITIVES: Record<string, string> = {
  "space-1": "4px", "space-2": "8px", "space-3": "12px", "space-4": "16px",
  "radius-1": "4px", "radius-2": "8px", "radius-3": "12px",
  "font-ui": '-apple-system, BlinkMacSystemFont, "SF Pro Text", system-ui, sans-serif',
  "font-code": 'ui-monospace, "SF Mono", Menlo, Consolas, monospace',
  "text-sm": "12px", "text-md": "13px", "text-lg": "15px",
  "duration-fast": "90ms", "duration-normal": "180ms",
};
const BASE_SEMANTIC: Record<string, string> = {
  "space-unit": "{space-1}", "radius": "{radius-2}", "font-ui": "{font-ui}", "font-code": "{font-code}",
  "text-ui": "{text-md}", "text-code": "12.5px", "motion-fast": "{duration-fast}", "motion-normal": "{duration-normal}",
};

/** Nudges `fg` towards white or black until it clears AA on every background. */
function ensureContrast(fg: Rgba, bgs: Rgba[], base: Rgba, name: string, notes: string[]): Rgba {
  const worst = (c: Rgba) => Math.min(...bgs.map((b) => contrastRatio(c, b, base)));
  if (worst(fg) >= AA_NORMAL) return fg;
  const avgLum = bgs.reduce((s, b) => s + (composite(b, base).r + composite(b, base).g + composite(b, base).b) / 765, 0) / bgs.length;
  const target: Rgba = avgLum < 0.5 ? { r: 255, g: 255, b: 255, a: 1 } : { r: 0, g: 0, b: 0, a: 1 };
  let out = fg;
  for (let t = 0.05; t <= 1.0001; t += 0.05) {
    out = mix(fg, target, t);
    if (worst(out) >= AA_NORMAL + 0.05) break;
  }
  notes.push(`${name}: ${toHex(fg)} -> ${toHex(out)} (was ${worst(fg).toFixed(2)}:1, now ${worst(out).toFixed(2)}:1)`);
  return out;
}

export function importVscodeTheme(vs: VscodeTheme, opts: { name?: string; vibrancy?: boolean } = {}): ImportResult {
  const colors = vs.colors ?? {};
  const rules = vs.tokenColors ?? [];
  const appearance: Appearance = vs.type === "light" || vs.type === "hc-light" ? "light" : "dark";
  const base: Rgba = appearance === "dark" ? { r: 0, g: 0, b: 0, a: 1 } : { r: 255, g: 255, b: 255, a: 1 };
  const mapped: Record<string, string> = {};
  const derived: string[] = [];
  const adjustments: string[] = [];

  const pick = (token: string): Rgba | null => {
    const hit = firstColor(colors, SEMANTIC_MAP[token] ?? []);
    if (hit) mapped[token] = hit.key;
    return hit?.value ?? null;
  };
  const orDerive = (token: string, make: () => Rgba): Rgba => {
    const v = pick(token);
    if (v) return v;
    derived.push(token);
    return make();
  };

  const fgDefault: Rgba = appearance === "dark" ? { r: 230, g: 230, b: 235, a: 1 } : { r: 30, g: 30, b: 36, a: 1 };
  const bgDefault: Rgba = appearance === "dark" ? { r: 24, g: 24, b: 28, a: 1 } : { r: 250, g: 250, b: 252, a: 1 };

  const s1 = orDerive("surface-1", () => bgDefault);
  const s0 = orDerive("surface-0", () => s1);
  const s2 = orDerive("surface-2", () => mix(s1, fgDefault, 0.06));
  const overlay = orDerive("surface-overlay", () => s2);
  const textRaw = orDerive("text", () => fgDefault);
  const bgs = [s0, s1, s2].map((c) => composite(c, base));
  const text = ensureContrast(textRaw, bgs, base, "text", adjustments);
  const mutedRaw = orDerive("text-muted", () => mix(text, s1, 0.35));
  const muted = ensureContrast(mutedRaw, bgs, base, "text-muted", adjustments);
  const faint = orDerive("text-faint", () => mix(text, s1, 0.55));
  const accent = orDerive("accent", () => mix(text, s1, 0.3));
  const accentHover = orDerive("accent-hover", () => mix(accent, text, 0.2));
  const accentMuted = orDerive("accent-muted", () => withAlpha(accent, 0.22));
  const onAccentRaw = pick("text-on-accent") ?? ((derived.push("text-on-accent"), s0));
  const onAccent = ensureContrast(onAccentRaw, [composite(accent, base)], base, "text-on-accent", adjustments);
  const border = orDerive("border", () => mix(s1, text, 0.14));
  const borderStrong = orDerive("border-strong", () => mix(s1, text, 0.28));
  const focus = orDerive("focus-ring", () => accent);
  const danger = orDerive("danger", () => ({ r: 235, g: 87, b: 87, a: 1 }));
  const warning = orDerive("warning", () => ({ r: 242, g: 169, b: 59, a: 1 }));
  const success = orDerive("success", () => ({ r: 76, g: 190, b: 120, a: 1 }));
  const info = orDerive("info", () => ({ r: 86, g: 156, b: 245, a: 1 }));
  const codeBg = orDerive("code-bg", () => s1);
  const diffAdd = orDerive("diff-add", () => withAlpha(success, 0.15));
  const diffDel = orDerive("diff-del", () => withAlpha(danger, 0.15));
  const diffAddText = orDerive("diff-add-text", () => success);
  const diffDelText = orDerive("diff-del-text", () => danger);
  const ghost = orDerive("ghost-text", () => faint);

  const sem: Record<string, string> = {
    ...BASE_SEMANTIC,
    "surface-0": toHex(s0), "surface-1": toHex(s1), "surface-2": toHex(s2), "surface-overlay": toHex(overlay),
    "surface-glass": toHex(withAlpha(s0, 0.72)),
    "text": toHex(text), "text-muted": toHex(muted), "text-faint": toHex(faint), "text-on-accent": toHex(onAccent),
    "border": toHex(border), "border-strong": toHex(borderStrong), "focus-ring": toHex(focus),
    "accent": toHex(accent), "accent-hover": toHex(accentHover), "accent-muted": toHex(accentMuted),
    "danger": toHex(danger), "warning": toHex(warning), "success": toHex(success), "info": toHex(info),
    "code-bg": toHex(codeBg), "diff-add": toHex(diffAdd), "diff-del": toHex(diffDel),
    "diff-add-text": toHex(diffAddText), "diff-del-text": toHex(diffDelText), "ghost-text": toHex(ghost),
    "state-running": "{info}", "state-needs-you": "{warning}", "state-failed": "{danger}", "state-ready": "{success}",
  };

  const component: Record<string, string> = {};
  for (const [token, scopes] of Object.entries(SYNTAX_MAP)) {
    for (const scope of scopes) {
      const c = findTokenColor(rules, scope);
      if (c) {
        component[token] = c;
        break;
      }
    }
  }

  const file = themeFileFromObject({
    meta: { name: opts.name ?? vs.displayName ?? vs.name ?? "Imported theme", appearance, vibrancy: opts.vibrancy ?? false },
    primitive: BASE_PRIMITIVES,
    semantic: sem,
    component,
  });
  return { file, mapped, derived, adjustments };
}
