// Static conformance check for "components use only semantic and component tokens"
// (PLAN 3.20, docs/contracts/m0.md section 4). Fails on hard-coded colours, absolute sizes and
// durations in component CSS and TSX. Literal values belong in token definitions only: the
// `:root { ... }` block of base.css (component-token defaults) and the theme TOML files.

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

export interface Violation {
  line: number;
  text: string;
  rule: string;
}

/** Removes comments and (for base.css) the `:root { ... }` token-definition blocks. */
function stripCss(css: string, dropRootBlocks: boolean): string {
  let out = css.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, " "));
  if (dropRootBlocks) {
    out = out.replace(/:root\s*\{[^}]*\}/g, (m) => m.replace(/[^\n]/g, " "));
  }
  return out;
}

export function checkCss(css: string, dropRootBlocks = false): Violation[] {
  const v: Violation[] = [];
  stripCss(css, dropRootBlocks)
    .split("\n")
    .forEach((text, i) => {
      // selectors such as `.syntax-keyword`, `.right-tab` and attribute values are not declarations
      const decl = text.includes(":") ? text.slice(text.indexOf(":")) : "";
      if (!decl) return;
      const check = (re: RegExp, rule: string) => re.test(decl) && v.push({ line: i + 1, text: text.trim(), rule });
      check(HEX, "hex colour");
      check(COLOUR_FN, "colour function");
      check(NAMED_COLOURS, "named colour");
      check(ABS_LENGTH, "absolute length");
      check(DURATION, "duration");
    });
  return v;
}

export function checkTsx(src: string): Violation[] {
  const v: Violation[] = [];
  src.split("\n").forEach((text, i) => {
    const code = text.replace(/\/\/.*$/, "");
    const check = (re: RegExp, rule: string) => re.test(code) && v.push({ line: i + 1, text: text.trim(), rule });
    check(HEX, "hex colour");
    check(COLOUR_FN, "colour function");
    check(/\bsize=\{\d+\}/, "numeric icon size (use the icon-xs/sm/md classes)");
    check(/\d\s*(px|rem|ms)\b/, "absolute size or duration in a literal");
    check(/\bMath\.(min|max)\([^)]*\b\d{2,}\b/, "numeric literal in a size clamp");
    check(/\b(width|height|maxHeight|minHeight)\s*:\s*\d/, "numeric style dimension");
  });
  return v;
}

const fmt = (file: string, vs: Violation[]) => vs.map((x) => `${file}:${x.line} [${x.rule}] ${x.text}`).join("\n");

describe("token-only components (static conformance)", () => {
  it("components.css has no hard-coded colours, absolute sizes or durations", () => {
    const vs = checkCss(read("styles/components.css"));
    expect(fmt("components.css", vs)).toBe("");
  });

  it("base.css has literals only inside :root token definitions", () => {
    const vs = checkCss(read("styles/base.css"), true);
    expect(fmt("base.css", vs)).toBe("");
  });

  const tsx = [
    ...readdirSync(join(SRC, "ui")).filter((f) => f.endsWith(".tsx")).map((f) => `ui/${f}`),
    "App.tsx",
  ];
  for (const f of tsx) {
    it(`${f} has no hard-coded colours, icon sizes, sizes or durations`, () => {
      expect(fmt(f, checkTsx(read(f)))).toBe("");
    });
  }

  it("the checker itself catches each kind of violation (so a pass means something)", () => {
    expect(checkCss(".a { color: #fff; }")).toHaveLength(1);
    expect(checkCss(".a { color: rgba(0, 0, 0, 0.5); }")).toHaveLength(1);
    expect(checkCss(".a { color: white; }")).toHaveLength(1);
    expect(checkCss(".a { width: 12px; }")).toHaveLength(1);
    expect(checkCss(".a { margin: 0 -4px; }")).toHaveLength(1);
    expect(checkCss(".a { transition: opacity 120ms ease; }").length).toBeGreaterThan(0);
    expect(checkCss(".a { animation: x 1.6s infinite; }").length).toBeGreaterThan(0);
    expect(checkCss(".a { padding: calc(var(--space-unit) * 2); border: var(--hairline) solid var(--border); }")).toHaveLength(0);
    expect(checkCss(":root { --a: 12px; --b: #fff; }\n.a { width: 1px; }", true)).toHaveLength(1);
    expect(checkTsx('<X size={12} />')).toHaveLength(1);
    expect(checkTsx("el.style.height = `${Math.min(h, 200)}px`;")).toHaveLength(1);
    expect(checkTsx("style={{ height: 40 }}")).toHaveLength(1);
    expect(checkTsx('<div style={{ background: e.theme.semantic.accent }} />')).toHaveLength(0);
  });
});
