import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { contrastRatio, parseColor } from "./color";
import { REQUIRED_SEMANTIC, ThemeError, loadTheme, parseThemeToml, resolveLayers } from "./loader";
import { DERIVED_TOKENS, SEMANTIC_MAP, SYNTAX_MAP, findTokenColor, importVscodeTheme } from "./vscode";

const THEMES_DIR = join(import.meta.dirname, "../../themes");
const shipped = readdirSync(THEMES_DIR).filter((f) => f.endsWith(".toml"));

/** A tiny valid theme to mutate in tests. */
function minimal(overrides: { semantic?: Record<string, string | null>; primitive?: Record<string, string>; component?: Record<string, string>; appearance?: string } = {}): string {
  const semantic: Record<string, string> = {
    "surface-0": "{gray-950}", "surface-1": "{gray-900}", "surface-2": "{gray-800}", "surface-overlay": "#202020",
    "surface-glass": "rgba(10, 10, 10, 0.5)", "text": "{gray-100}", "text-muted": "{gray-300}", "text-faint": "{gray-500}",
    "text-on-accent": "#000000", "border": "#333333", "border-strong": "#444444", "focus-ring": "{accent}",
    "accent": "#7c8cff", "accent-hover": "#9aa6ff", "accent-muted": "rgba(124,140,255,0.2)",
    "danger": "#ff6b6b", "warning": "#ffb454", "success": "#5fd49a", "info": "#6cb6ff",
    "code-bg": "#101010", "diff-add": "#103010", "diff-del": "#301010", "diff-add-text": "#8f8", "diff-del-text": "#f88", "ghost-text": "#777777",
    "state-running": "{info}", "state-needs-you": "{warning}", "state-failed": "{danger}", "state-ready": "{success}",
    "space-unit": "4px", "radius": "8px", "font-ui": "system-ui", "font-code": "monospace",
    "text-ui": "13px", "text-code": "12px", "motion-fast": "90ms", "motion-normal": "180ms",
  };
  if (overrides.appearance === "light") Object.assign(semantic, { info: "#4d6684", warning: "#94540f", danger: "#ac3c33", success: "#33704a" });
  for (const [k, v] of Object.entries(overrides.semantic ?? {})) {
    if (v === null) delete semantic[k];
    else semantic[k] = v;
  }
  const primitive = { "gray-100": "#eeeeee", "gray-300": "#bbbbbb", "gray-500": "#888888", "gray-800": "#222222", "gray-900": "#181818", "gray-950": "#101010", ...overrides.primitive };
  const toml = (t: Record<string, string>) => Object.entries(t).map(([k, v]) => `${k} = ${JSON.stringify(v)}`).join("\n");
  return `[meta]\nname = "T"\nappearance = "${overrides.appearance ?? "dark"}"\n\n[primitive]\n${toml(primitive)}\n\n[semantic]\n${toml(semantic)}\n\n[component]\n${toml(overrides.component ?? {})}\n`;
}

describe("colour maths", () => {
  it("parses hex and rgb forms", () => {
    expect(parseColor("#fff")).toEqual({ r: 255, g: 255, b: 255, a: 1 });
    expect(parseColor("#00000080")?.a).toBeCloseTo(0.5, 2);
    expect(parseColor("rgba(10, 20, 30, 0.5)")).toEqual({ r: 10, g: 20, b: 30, a: 0.5 });
    expect(parseColor("not-a-colour")).toBeNull();
  });
  it("accepts both valid rgb() syntaxes, with numbers and percentages", () => {
    expect(parseColor("rgb(255, 0, 0)")).toEqual({ r: 255, g: 0, b: 0, a: 1 });
    expect(parseColor("rgb(255 0 0)")).toEqual({ r: 255, g: 0, b: 0, a: 1 });
    expect(parseColor("rgb(10 20 30 / 50%)")).toEqual({ r: 10, g: 20, b: 30, a: 0.5 });
    expect(parseColor("rgba(100%, 0%, 0%, .25)")).toEqual({ r: 255, g: 0, b: 0, a: 0.25 });
    expect(parseColor("  RGB( 1 , 2 , 3 )  ")).toEqual({ r: 1, g: 2, b: 3, a: 1 });
  });
  it("rejects malformed rgb() the way CSS does (no parseFloat leniency)", () => {
    for (const bad of [
      "rgb(255junk,255junk,255junk)", // junk suffixes
      "rgb(255px, 0, 0)", // units
      "rgb(255, 255 255)", // mixed separators
      "rgb(255 255, 255)",
      "rgb(255, 255, 255 / 0.5)", // slash in the legacy syntax
      "rgb(255 255 255, 0.5)", // comma alpha in the modern syntax
      "rgb(255, 255)",
      "rgb(255, 255, 255, 0.5, 1)",
      "rgb(255, 255, 255,)",
      "rgb(,255, 255, 255)",
      "rgb(1e2, 0, 0)",
      "rgb(0x10, 0, 0)",
      "rgb(255, 255, 255) foo",
      "rgb()",
      "rgba(0, 0, 0, abc)",
    ])
      expect(parseColor(bad), bad).toBeNull();
  });
  it("rejects a malformed rgb() in a theme instead of certifying its contrast", () => {
    expect(() => loadTheme(minimal({ semantic: { text: "rgb(255junk,255junk,255junk)" } }), "junk")).toThrow(/unparseable-color/);
  });
  it("matches the WCAG reference ratios", () => {
    const w = parseColor("#ffffff")!;
    const b = parseColor("#000000")!;
    expect(contrastRatio(b, w)).toBeCloseTo(21, 5);
    expect(contrastRatio(parseColor("#767676")!, w)).toBeCloseTo(4.54, 1); // the classic AA grey
  });
});

describe("theme loader", () => {
  it("loads a minimal theme and resolves references into CSS variables", () => {
    const t = loadTheme(minimal(), "min");
    expect(t.vars["--surface-0"]).toBe("#101010");
    expect(t.vars["--p-gray-950"]).toBe("#101010");
    expect(t.vars["--state-needs-you"]).toBe("#ffb454"); // semantic -> semantic
    expect(t.contrast.every((c) => c.pass)).toBe(true);
  });

  it("resolves chains across all three layers (primitive -> semantic -> component)", () => {
    const t = loadTheme(minimal({ primitive: { "brand": "#8899ff", "brand-2": "{brand}" }, semantic: { accent: "{brand-2}" }, component: { "tab-active-bg": "{surface-2}", "tab-edge": "{tab-active-bg}" } }), "chain");
    expect(t.vars["--accent"]).toBe("#8899ff");
    expect(t.vars["--tab-active-bg"]).toBe("#222222");
    expect(t.vars["--tab-edge"]).toBe("#222222");
  });

  it("interpolates references inside larger values", () => {
    const t = loadTheme(minimal({ component: { "tab-shadow": "0 1px 0 {border}, 0 0 0 1px {accent}" } }), "interp");
    expect(t.vars["--tab-shadow"]).toBe("0 1px 0 #333333, 0 0 0 1px #7c8cff");
  });

  it("rejects a reference to a token that does not exist", () => {
    expect.assertions(3);
    try {
      loadTheme(minimal({ semantic: { accent: "{nope}" } }), "ref");
    } catch (e) {
      expect(e).toBeInstanceOf(ThemeError);
      expect((e as ThemeError).has("missing-reference")).toBe(true);
      expect((e as ThemeError).message).toContain("{nope}");
    }
  });

  it("rejects a reference cycle and names the chain", () => {
    expect.assertions(2);
    try {
      loadTheme(minimal({ semantic: { accent: "{danger}", danger: "{accent}" } }), "cycle");
    } catch (e) {
      expect((e as ThemeError).has("cycle")).toBe(true);
      expect((e as ThemeError).message).toMatch(/semantic\.accent -> semantic\.danger -> semantic\.accent|semantic\.danger -> semantic\.accent -> semantic\.danger/);
    }
  });

  it("rejects a self-reference as a cycle", () => {
    expect(() => loadTheme(minimal({ semantic: { "ghost-text": "{ghost-text}" } }), "self")).toThrow(/cycle/);
  });

  it("does not let a primitive reference a semantic token (layers only look downwards)", () => {
    const text = minimal({ primitive: { "gray-300": "{text}" } });
    expect(() => loadTheme(text, "up")).toThrow(/missing-reference|cycle/);
  });

  for (const token of REQUIRED_SEMANTIC) {
    it(`fails validation when --${token} is missing`, () => {
      expect.assertions(2);
      try {
        loadTheme(minimal({ semantic: { [token]: null } }), "missing");
      } catch (e) {
        expect((e as ThemeError).has("missing-token")).toBe(true);
        expect((e as ThemeError).issues.find((i) => i.code === "missing-token")?.token).toBe(token);
      }
    });
  }

  it("fails WCAG AA for low-contrast body text and reports the ratio", () => {
    expect.assertions(3);
    try {
      loadTheme(minimal({ semantic: { text: "#2a2a2a" } }), "lowtext");
    } catch (e) {
      const err = e as ThemeError;
      expect(err.has("contrast")).toBe(true);
      expect(err.issues.some((i) => i.token === "text/surface-0")).toBe(true);
      expect(err.message).toMatch(/needs 4\.5:1/);
    }
  });

  it("fails WCAG AA for muted text, and for text on the accent", () => {
    expect(() => loadTheme(minimal({ semantic: { "text-muted": "#3a3a3a" } }), "m")).toThrow(/text-muted/);
    expect(() => loadTheme(minimal({ semantic: { "text-on-accent": "#8a96ff" } }), "a")).toThrow(/text-on-accent/);
  });

  it("accepts text exactly at the boundary and rejects just below (#767676 on white is 4.54:1)", () => {
    const light = (fg: string) => minimal({ appearance: "light", semantic: { "surface-0": "#ffffff", "surface-1": "#ffffff", "surface-2": "#ffffff", text: "#000000", "text-muted": fg, "text-on-accent": "#ffffff", accent: "#000000" } });
    expect(() => loadTheme(light("#767676"), "ok")).not.toThrow();
    expect(() => loadTheme(light("#7a7a7a"), "bad")).toThrow(/contrast|text-muted/);
  });

  it("refuses colours the contrast check cannot read, instead of skipping the check", () => {
    expect(() => loadTheme(minimal({ semantic: { text: "color-mix(in srgb, white, black)" } }), "cm")).toThrow(/unparseable-color/);
  });

  it("reports every problem in one error, not just the first", () => {
    expect.assertions(2);
    try {
      loadTheme(minimal({ semantic: { "code-bg": null, "ghost-text": null, text: "#2a2a2a" } }), "many");
    } catch (e) {
      const codes = (e as ThemeError).issues.map((i) => i.code);
      expect(codes.filter((c) => c === "missing-token")).toHaveLength(2);
      expect(codes).toContain("contrast");
    }
  });

  it("rejects malformed files: bad TOML, bad meta, bad token names", () => {
    expect(() => parseThemeToml("[meta\nname=", "x")).toThrow(ThemeError);
    expect(() => parseThemeToml('[meta]\nname = "x"\nappearance = "sepia"\n', "x")).toThrow(/appearance/);
    expect(() => parseThemeToml('[meta]\nname = "x"\nappearance = "dark"\n[semantic]\nBad_Name = "#fff"\n', "x")).toThrow(/invalid-name/);
  });

  it("resolveLayers is deterministic and does not mutate its input", () => {
    const file = parseThemeToml(minimal(), "x");
    const before = JSON.stringify(file);
    expect(resolveLayers(file)).toEqual(resolveLayers(file));
    expect(JSON.stringify(file)).toBe(before);
  });
});

describe("name collisions across layers (validated variables cannot be overwritten)", () => {
  it("rejects a [component] token named like a semantic token (the --text bypass)", () => {
    expect.assertions(3);
    try {
      loadTheme(minimal({ component: { text: "{surface-1}" } }), "bypass");
    } catch (e) {
      expect((e as ThemeError).has("name-collision")).toBe(true);
      expect((e as ThemeError).message).toContain("--text");
      expect((e as ThemeError).issues.find((i) => i.code === "name-collision")?.token).toBe("text");
    }
  });
  it("rejects collisions with every required semantic name", () => {
    for (const token of REQUIRED_SEMANTIC) {
      expect(() => loadTheme(minimal({ component: { [token]: "#ffffff" } }), "c"), token).toThrow(/name-collision/);
    }
  });
  it("rejects a component or semantic token that would emit an existing --p-* primitive variable", () => {
    expect(() => loadTheme(minimal({ component: { "p-gray-100": "#000000" } }), "p")).toThrow(/name-collision/);
    expect(() => loadTheme(minimal({ semantic: { "p-gray-900": "#000000" } }), "p")).toThrow(/name-collision/);
  });
  it("still reports the collision when the colliding value would also fail to resolve", () => {
    expect(() => loadTheme(minimal({ component: { text: "{nope}" } }), "both")).toThrow(/name-collision/);
  });
  it("allows component tokens with distinct names, and the same name in different layers' emitted spaces", () => {
    // primitive `gray-100` emits --p-gray-100; a component `gray-100` emits --gray-100: no clash.
    expect(() => loadTheme(minimal({ component: { "gray-100": "#ffffff", "tab-active-bg": "{surface-2}" } }), "ok")).not.toThrow();
  });
  it("leaves the validated --text untouched in the output of a valid theme", () => {
    const t = loadTheme(minimal(), "ok");
    expect(t.vars["--text"]).toBe(t.semantic.text);
  });
});

describe("shipped themes (the CI check on the token files)", () => {
  it("ships three directions, each with dark and light", () => {
    for (const d of ["glass", "flat", "paper", "atrium-calm"]) {
      expect(shipped).toContain(`${d}-dark.toml`);
      expect(shipped).toContain(`${d}-light.toml`);
    }
  });
  for (const file of shipped) {
    it(`${file} loads, defines the full semantic set and passes WCAG AA`, () => {
      const t = loadTheme(readFileSync(join(THEMES_DIR, file), "utf8"), file);
      for (const req of REQUIRED_SEMANTIC) expect(t.vars[`--${req}`], `--${req}`).toBeTruthy();
      expect(t.contrast.length).toBeGreaterThanOrEqual(7);
      expect(t.contrast.every((c) => c.pass)).toBe(true);
      expect(t.meta.appearance).toBe(file.includes("light") ? "light" : "dark");
    });
  }
  it("light and dark variants differ in appearance, not in structure (same token names)", () => {
    for (const d of ["glass", "flat", "paper", "atrium-calm"]) {
      const a = loadTheme(readFileSync(join(THEMES_DIR, `${d}-dark.toml`), "utf8"), "a");
      const b = loadTheme(readFileSync(join(THEMES_DIR, `${d}-light.toml`), "utf8"), "b");
      const sem = (t: typeof a) => Object.keys(t.semantic).filter((k) => !k.startsWith("syntax")).sort();
      expect(sem(a)).toEqual(sem(b));
    }
  });
});

describe("VS Code import", () => {
  const mocha = JSON.parse(readFileSync(join(THEMES_DIR, "vscode/catppuccin-mocha.json"), "utf8"));

  it("maps a real VS Code theme onto a valid, AA-passing Atrium theme", () => {
    const r = importVscodeTheme(mocha);
    const t = loadTheme(tomlOf(r.file), "mocha");
    expect(t.meta.appearance).toBe("dark");
    expect(t.vars["--accent"]).toBe("#cba6f7");
    expect(t.vars["--surface-1"]).toBe("#1e1e2e");
    expect(t.vars["--surface-0"]).toBe("#11111b");
    expect(t.vars["--syntax-string"]).toBe("#a6e3a1");
    expect(t.contrast.every((c) => c.pass)).toBe(true);
  });

  it("covers every required semantic token (mapped or derived)", () => {
    const r = importVscodeTheme(mocha);
    for (const req of REQUIRED_SEMANTIC) expect(r.file.semantic[req], req).toBeTruthy();
    const accounted = new Set([...Object.keys(SEMANTIC_MAP), ...Object.keys(DERIVED_TOKENS)]);
    expect(accounted.has("accent")).toBe(true);
  });

  it("derives tokens and nudges contrast when the VS Code theme lacks or fails them", () => {
    const r = importVscodeTheme({ type: "dark", colors: { "editor.background": "#202020", foreground: "#444444", descriptionForeground: "#505050" } });
    expect(r.derived).toContain("accent");
    expect(r.adjustments.some((a) => a.startsWith("text:"))).toBe(true);
    expect(() => loadTheme(tomlOf(r.file), "sparse")).not.toThrow();
  });

  it("picks the most specific TextMate scope", () => {
    const rules = [
      { scope: "keyword", settings: { foreground: "#111111" } },
      { scope: ["keyword.control"], settings: { foreground: "#222222" } },
    ];
    expect(findTokenColor(rules, "keyword.control.flow")).toBe("#222222");
    expect(findTokenColor(rules, "keyword.operator")).toBe("#111111");
    expect(findTokenColor(rules, "string")).toBeNull();
    expect(Object.keys(SYNTAX_MAP)).toContain("syntax-keyword");
  });
});

/** Serialises a ThemeFile back to TOML so the import goes through the text loader too. */
function tomlOf(f: import("./loader").ThemeFile): string {
  const sec = (name: string, t: Record<string, string>) => `[${name}]\n${Object.entries(t).map(([k, v]) => `${k} = ${JSON.stringify(v)}`).join("\n")}\n`;
  return `[meta]\nname = ${JSON.stringify(f.meta.name)}\nappearance = "${f.meta.appearance}"\nvibrancy = ${f.meta.vibrancy}\n${sec("primitive", f.primitive)}${sec("semantic", f.semantic)}${sec("component", f.component)}`;
}
