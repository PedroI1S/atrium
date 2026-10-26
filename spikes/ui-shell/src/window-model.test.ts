import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { loadTheme } from "./theme/loader";
import { DEFAULT_SETTINGS, MIN_ALPHA, appearanceFor, moveView, initialModel, resolveTheme, type AppearanceInput } from "./window-model";

const calm = readFileSync(new URL("../themes/atrium-calm-dark.toml", import.meta.url), "utf8");
const input = (over: Partial<AppearanceInput> & { s?: Partial<typeof DEFAULT_SETTINGS> } = {}): AppearanceInput => ({
  settings: { ...DEFAULT_SETTINGS, alpha: 0.6, ...over.s },
  supported: over.supported ?? true,
  reduce: over.reduce ?? false,
  popout: over.popout ?? false,
  systemDark: over.systemDark ?? true,
});

describe("CR-02 theme rules", () => {
  it("reserves runtime names in primitives and validates state contrast for arbitrary theme ids", () => {
    expect(() => loadTheme(calm.replace("[primitive]", '[primitive]\nwindow-opacity = "0.5"'), "custom")).toThrow(/reserved/);
    expect(() => loadTheme(calm.replace('state-ready = "{success}"', 'state-ready = "#222222"'), "custom")).toThrow(/state-ready/);
  });
  it("emits channels separately from user opacity; rejects theme overrides", () => {
    const theme = loadTheme(calm, "atrium-calm-dark");
    expect(theme.vars["--surface-0-rgb"]).toBe("22 22 21");
    expect(theme.vars["--surface-overlay-rgb"]).toBe("47 47 45");
    for (const key of ["window-alpha-chrome", "window-alpha-content", "window-opacity", "window-blur", "surface-0-rgb"])
      expect(() => loadTheme(`${calm}\n${key} = "0.5"`, "bad")).toThrow(/reserved/);
    expect(() => loadTheme(calm.replace('surface-0 = "{gray-0}"', 'surface-0 = "rgba(22, 22, 21, 0.8)"'), "bad")).toThrow(/opaque/);
  });
});

describe("appearanceFor (opacity rules, ui-direction §5)", () => {
  it("background mode fades surfaces only; cards stay more solid; vibrancy on", () => {
    const a = appearanceFor(input());
    expect(a.tokens).toEqual({ "--window-alpha-chrome": "0.6", "--window-alpha-content": "0.74", "--window-opacity": "1", "--window-blur": "var(--glass-blur)" });
    expect(a.native).toEqual({ vibrancy: true, alpha: 1, appearance: "dark" });
  });
  it("keep reading areas solid holds cards at 1", () => {
    expect(appearanceFor(input({ s: { solid: true } })).tokens["--window-alpha-content"]).toBe("1");
  });
  it("whole window keeps surfaces opaque and uses native alpha, no vibrancy", () => {
    const a = appearanceFor(input({ s: { mode: "whole" } }));
    expect(a.tokens["--window-alpha-chrome"]).toBe("1");
    expect(a.tokens["--window-alpha-content"]).toBe("1");
    expect(a.native).toEqual({ vibrancy: false, alpha: 0.6, appearance: "dark" });
  });
  it("blur off: translucent without vibrancy", () => {
    const a = appearanceFor(input({ s: { blur: false } }));
    expect(a.native.vibrancy).toBe(false);
    expect(a.tokens["--window-blur"]).toBe("0");
  });
  it("unsupported platform and accessibility force opaque", () => {
    for (const a of [appearanceFor(input({ supported: false })), appearanceFor(input({ reduce: true })), appearanceFor(input({ reduce: true, s: { mode: "whole" } }))]) {
      expect(a.tokens["--window-alpha-chrome"]).toBe("1");
      expect(a.native).toMatchObject({ vibrancy: false, alpha: 1 });
    }
  });
  it("pop-outs follow main unless Advanced pins them opaque", () => {
    expect(appearanceFor(input({ popout: true })).alpha).toBe(0.6);
    const pinned = appearanceFor(input({ popout: true, s: { popouts: "opaque", mode: "whole" } }));
    expect(pinned.native.alpha).toBe(1);
    expect(appearanceFor(input({ popout: false, s: { popouts: "opaque" } })).alpha).toBe(0.6);
  });
  it("clamps to the slider range and ignores NaN", () => {
    expect(appearanceFor(input({ s: { alpha: -10 } })).alpha).toBe(MIN_ALPHA);
    expect(appearanceFor(input({ s: { alpha: Number.NaN } })).alpha).toBe(1);
  });
  it("system theme follows the OS", () => {
    expect(resolveTheme("system", false)).toBe("atrium-calm-light");
    expect(appearanceFor(input({ s: { theme: "system" }, systemDark: false })).native.appearance).toBe("light");
  });
});

it("moveView changes only the owner and bumps the revision", () => {
  const state = initialModel();
  const moved = moveView(state, "editor", "view-editor");
  expect(moved.views.editor).toEqual({ ...state.views.editor, owner: "view-editor" });
  expect(moved.views.chat).toBe(state.views.chat);
  expect(moved.revision).toBe(state.revision + 1);
});
