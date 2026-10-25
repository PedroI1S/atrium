// Minimal colour maths for the token pipeline: parsing, WCAG contrast, mixing.
// Only what the validator and the VS Code importer need; no dependencies.

export interface Rgba {
  r: number; // 0..255
  g: number;
  b: number;
  a: number; // 0..1
}

const clamp = (n: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, n));

// One CSS <number> or <percentage>. Anything else (units, junk suffixes, exponents) is rejected.
const N = "[+-]?(?:\\d+(?:\\.\\d+)?|\\.\\d+)%?";
// Legacy syntax: commas between all components. Modern syntax: spaces, `/` before alpha. Never mixed.
const RGB_LEGACY = new RegExp(`^rgba?\\(\\s*(${N})\\s*,\\s*(${N})\\s*,\\s*(${N})\\s*(?:,\\s*(${N})\\s*)?\\)$`);
const RGB_MODERN = new RegExp(`^rgba?\\(\\s*(${N})\\s+(${N})\\s+(${N})\\s*(?:/\\s*(${N})\\s*)?\\)$`);

/** Strict `rgb()`/`rgba()`: returns [r, g, b, a] (channels 0..255, alpha 0..1) or null if it is not valid CSS. */
function parseRgbFunction(s: string): [number, number, number, number] | null {
  const m = RGB_LEGACY.exec(s) ?? RGB_MODERN.exec(s);
  if (!m) return null;
  const channel = (p: string) => (p.endsWith("%") ? (Number.parseFloat(p) / 100) * 255 : Number.parseFloat(p));
  const alpha = (p: string | undefined) => (p === undefined ? 1 : p.endsWith("%") ? Number.parseFloat(p) / 100 : Number.parseFloat(p));
  return [channel(m[1] as string), channel(m[2] as string), channel(m[3] as string), alpha(m[4])];
}

/** Parses `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()`/`rgba()`. Returns null otherwise. */
export function parseColor(input: string): Rgba | null {
  const s = input.trim().toLowerCase();
  if (s === "transparent") return { r: 0, g: 0, b: 0, a: 0 };
  if (s.startsWith("#")) {
    let h = s.slice(1);
    if (!/^[0-9a-f]+$/.test(h)) return null;
    if (h.length === 3 || h.length === 4) h = [...h].map((c) => c + c).join("");
    if (h.length !== 6 && h.length !== 8) return null;
    const n = (i: number) => Number.parseInt(h.slice(i, i + 2), 16);
    return { r: n(0), g: n(2), b: n(4), a: h.length === 8 ? n(6) / 255 : 1 };
  }
  const parts = parseRgbFunction(s);
  if (parts) {
    const [r, g, b, a] = parts;
    return { r: clamp(r, 0, 255), g: clamp(g, 0, 255), b: clamp(b, 0, 255), a: clamp(a, 0, 1) };
  }
  return null;
}

const hex2 = (n: number) => Math.round(clamp(n, 0, 255)).toString(16).padStart(2, "0");

export function toHex({ r, g, b, a }: Rgba): string {
  return `#${hex2(r)}${hex2(g)}${hex2(b)}${a < 1 ? hex2(a * 255) : ""}`;
}

/** Source-over compositing of `fg` onto an opaque `bg`. */
export function composite(fg: Rgba, bg: Rgba): Rgba {
  const a = fg.a;
  return {
    r: fg.r * a + bg.r * (1 - a),
    g: fg.g * a + bg.g * (1 - a),
    b: fg.b * a + bg.b * (1 - a),
    a: 1,
  };
}

/** WCAG 2.x relative luminance of an opaque colour. */
export function luminance({ r, g, b }: Rgba): number {
  const lin = (v: number) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

/** WCAG contrast ratio (1..21) of `fg` on `bg`; alpha is composited (fg over bg, bg over `base`). */
export function contrastRatio(fg: Rgba, bg: Rgba, base: Rgba = { r: 0, g: 0, b: 0, a: 1 }): number {
  const opaqueBg = composite(bg, base);
  const opaqueFg = composite(fg, opaqueBg);
  const l1 = luminance(opaqueFg);
  const l2 = luminance(opaqueBg);
  const [hi, lo] = l1 >= l2 ? [l1, l2] : [l2, l1];
  return (hi + 0.05) / (lo + 0.05);
}

/** Linear sRGB-space-agnostic mix: `t` of `b` into `a` (t = 0 gives a, 1 gives b). */
export function mix(a: Rgba, b: Rgba, t: number): Rgba {
  return {
    r: a.r + (b.r - a.r) * t,
    g: a.g + (b.g - a.g) * t,
    b: a.b + (b.b - a.b) * t,
    a: a.a + (b.a - a.a) * t,
  };
}

export function withAlpha(c: Rgba, a: number): Rgba {
  return { ...c, a };
}
