// Static conformance check for "components use only semantic and component tokens"
// (PLAN 3.20, docs/contracts/m0.md section 4). Fails on hard-coded colours, absolute sizes,
// durations and bare fractions (opacity, alpha) in CSS, TSX and the TS modules that paint or
// size anything. Literal values belong in token definitions only: the `:root { ... }` blocks
// of base.css, the theme TOML files, and named UPPER_CASE constants in TS.

import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const SRC = join(import.meta.dirname, "..");
const read = (p: string) => readFileSync(join(SRC, p), "utf8");

const NAMED_COLOURS = /\b(white|black|red|green|blue|yellow|orange|purple|pink|gray|grey|silver|navy|teal|maroon|olive|lime|aqua|fuchsia)\b/i;
const COLOUR_FN = /\b(rgba?|hsla?|hwb|lab|lch|oklab|oklch|color-mix)\(/i;
const HEX = /#[0-9a-f]{3,8}\b/i;
const ABS_LENGTH = /(?<![\w-])-?\d*\.?\d+(px|pt|pc|cm|mm|in|rem)\b/i;
const DURATION = /(?<![\w-])\d*\.?\d+(ms|s)\b/i;
/** A unitless fraction such as 0.35 or .7 that is not a multiplier inside calc() (`* 1.5`, `/ 2.5`). */
function hasBareFraction(value: string): boolean {
  for (const m of value.matchAll(/(?<![\w.-])\d*\.\d+(?![\w.%])/g)) {
    if (!/[*/]$/.test(value.slice(0, m.index).trimEnd())) return true;
  }
  return false;
}
const CSS_FRACTION = { test: hasBareFraction } as unknown as RegExp;
const TS_FRACTION = /(?<![\w.$])\d*\.\d+(?![\w.])/;
/** `const NAME = 0.35` (optionally exported/typed) is how TS names a visual constant. */
const NAMED_CONSTANT = /^\s*(export\s+)?const\s+[A-Z][A-Z0-9_]*\b/;

export interface Violation {
  line: number;
  text: string;
  rule: string;
}

/** Removes comments and (for base.css) the `:root { ... }` token-definition blocks. */
function stripCss(css: string, dropRootBlocks: boolean): string {
  let out = css.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, " "));
  if (dropRootBlocks) {
    out = out.replace(/:root(\[[^\]]*\])?\s*\{[^}]*\}/g, (m) => m.replace(/[^\n]/g, " "));
  }
  return out;
}

export function checkCss(css: string, dropRootBlocks = false): Violation[] {
  const v: Violation[] = [];
  stripCss(css, dropRootBlocks)
    .split("\n")
    .forEach((text, i) => {
      // check declarations only: selectors such as `.syntax-keyword` and `[data-state]` are not values
      const decls = text.match(/(?<=[{;]\s*|^\s*)[\w-]+\s*:[^;{}]*/g) ?? [];
      for (const decl of decls) {
        const value = decl.slice(decl.indexOf(":") + 1);
        const check = (re: RegExp, rule: string) => re.test(value) && v.push({ line: i + 1, text: text.trim(), rule });
        check(HEX, "hex colour");
        check(COLOUR_FN, "colour function");
        check(NAMED_COLOURS, "named colour");
        check(ABS_LENGTH, "absolute length");
        check(DURATION, "duration");
        check(CSS_FRACTION, "bare fraction");
      }
    });
  return v;
}

/** Rules for any TS/TSX source line (comments stripped). */
/** Blanks block comments (keeping line numbers) and line comments. */
const stripTsComments = (src: string) => src.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, " "));
const lineCode = (text: string) => text.replace(/(^|[^:])\/\/.*$/, "$1");

export function checkTs(src: string): Violation[] {
  const v: Violation[] = [];
  const original = src.split("\n");
  stripTsComments(src).split("\n").forEach((line, i) => {
    const text = original[i] ?? line;
    const code = lineCode(line);
    const check = (re: RegExp, rule: string) => re.test(code) && v.push({ line: i + 1, text: text.trim(), rule });
    check(HEX, "hex colour");
    check(COLOUR_FN, "colour function");
    check(/(?<!\w)\d*\.?\d+\s*(px|rem|ms)\b/, "absolute size or duration in a literal");
    // Fractions in quoted prose ("Sonnet 5.5") are text, not values.
    const unquoted = code.replace(/"[^"]*"|'[^']*'/g, '""');
    if (!NAMED_CONSTANT.test(code) && TS_FRACTION.test(unquoted)) v.push({ line: i + 1, text: text.trim(), rule: "bare fraction (name it as an UPPER_CASE constant)" });
  });
  return v;
}

export function checkTsx(src: string): Violation[] {
  const v = checkTs(src);
  const original = src.split("\n");
  stripTsComments(src).split("\n").forEach((line, i) => {
    const text = original[i] ?? line;
    const code = lineCode(line);
    const check = (re: RegExp, rule: string) => re.test(code) && v.push({ line: i + 1, text: text.trim(), rule });
    check(/\bsize=\{\d+\}/, "numeric icon size (use the .i / .i.sm classes)");
    check(/\bMath\.(min|max)\([^)]*\b\d{2,}\b/, "numeric literal in a size clamp");
    check(/\b(width|height|maxHeight|minHeight)\s*:\s*\d/, "numeric style dimension");
  });
  return v.sort((a, b) => a.line - b.line);
}

const fmt = (file: string, vs: Violation[]) => vs.map((x) => `${file}:${x.line} [${x.rule}] ${x.text}`).join("\n");

/** Every module that paints, sizes or animates anything (review #11 widened this list). */
const TS_FILES = ["windows.ts", "window-model.ts", "state.ts", "sync.ts", "layout.ts", "bench.ts", "platform.ts", "settings-registry.ts", "theme/runtime.ts", "theme/calm.ts"];

describe("token-only components (static conformance)", () => {
  for (const css of readdirSync(join(SRC, "styles")).filter((f) => f.endsWith(".css") && f !== "base.css")) {
    it(`${css} has no hard-coded colours, sizes, durations or fractions`, () => {
      expect(fmt(css, checkCss(read(`styles/${css}`)))).toBe("");
    });
  }

  it("base.css has literals only inside :root token definitions", () => {
    expect(fmt("base.css", checkCss(read("styles/base.css"), true))).toBe("");
  });

  it("base.css does not repeat values the Calm themes own", () => {
    const owned = [...read("../themes/atrium-calm-light.toml").matchAll(/^([\w-]+)\s*=/gm)].map((m) => m[1]);
    const defined = [...read("styles/base.css").matchAll(/^\s*--([\w-]+)\s*:/gm)].map((m) => m[1]);
    expect(defined.filter((d) => owned.includes(d))).toEqual([]);
  });

  const tsx = [...readdirSync(join(SRC, "ui")).filter((f) => f.endsWith(".tsx")).map((f) => `ui/${f}`), "App.tsx", "index.tsx"];
  for (const f of tsx) {
    it(`${f} has no hard-coded colours, icon sizes, sizes, durations or fractions`, () => {
      expect(fmt(f, checkTsx(read(f)))).toBe("");
    });
  }
  for (const f of TS_FILES) {
    it(`${f} has no hard-coded colours, sizes, durations or unnamed fractions`, () => {
      expect(fmt(f, checkTs(read(f)))).toBe("");
    });
  }

  it("the checker itself catches each kind of violation (so a pass means something)", () => {
    expect(checkCss(".a { color: #fff; }")).toHaveLength(1);
    expect(checkCss(".a { color: rgba(0, 0, 0, 0.5); }").length).toBeGreaterThan(0);
    expect(checkCss(".a { color: white; }")).toHaveLength(1);
    expect(checkCss(".a { width: 12px; }")).toHaveLength(1);
    expect(checkCss(".a { margin: 0 -4px; }")).toHaveLength(1);
    expect(checkCss(".a { transition: opacity 120ms ease; }").length).toBeGreaterThan(0);
    expect(checkCss(".a { animation: x 1.6s infinite; }").length).toBeGreaterThan(0);
    expect(checkCss(".a { opacity: 0.7; }")).toHaveLength(1);
    expect(checkCss(".a { opacity: .7; }")).toHaveLength(1);
    expect(checkCss(".a { padding: calc(var(--space-unit) * 2.5); border: var(--hairline) solid var(--border); }")).toHaveLength(0);
    expect(checkCss(".a { font-weight: 400; width: 50%; }")).toHaveLength(0);
    expect(checkCss(":root { --a: 12px; --b: #fff; --c: 0.5; }\n.a { width: 1px; }", true)).toHaveLength(1);
    expect(checkCss(":root[data-x] { --a: 12px; }", true)).toHaveLength(0);
    expect(checkTsx("<X size={12} />")).toHaveLength(1);
    expect(checkTsx("el.style.height = `${Math.min(h, 200)}px`;")).toHaveLength(1);
    expect(checkTsx("style={{ height: 40 }}")).toHaveLength(1);
    expect(checkTsx("<div style={{ background: e.theme.semantic.accent }} />")).toHaveLength(0);
    expect(checkTs('"--window-blur": "24px",')).toHaveLength(1);
    expect(checkTs("const content = a + (1 - a) * 0.35;")).toHaveLength(1);
    expect(checkTs("export const READING_AREA_LIFT = 0.35;")).toHaveLength(0);
    expect(checkTs("setTimeout(f, 150); // 150ms")).toHaveLength(0);
    expect(checkTs("x.toFixed(2); arr.at(-1); `${v}%`")).toHaveLength(0);
    expect(checkTs('label: "Claude · Sonnet 5.5"')).toHaveLength(0);
    expect(checkTs("patchSettings({ alpha: 0.6 })")).toHaveLength(1);
  });
});
