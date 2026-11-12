import { describe, expect, it } from "vitest";
import { DEFAULT_SETTINGS, MAIN, MIN_ALPHA, appearanceFor, initialModel, nativeCalls, ownerOf, resolveTheme } from "./window-model";

const input = (settings: Partial<typeof DEFAULT_SETTINGS> = {}, rest: Partial<Parameters<typeof appearanceFor>[0]> = {}) =>
  ({ settings: { ...DEFAULT_SETTINGS, ...settings }, supported: true, reduce: false, popout: false, systemDark: false, ...rest });

describe("opacity rules (ui-direction §5)", () => {
  it("is off at 100 %", () => {
    const a = appearanceFor(input());
    expect(a.tokens["--window-alpha-chrome"]).toBe("1");
    expect(a.tokens["--window-alpha-content"]).toBe("1");
    expect(a.native).toEqual({ vibrancy: false, alpha: 1, appearance: "light" });
  });
  it("background mode fades the frame more than the cards and keeps the window alpha at 1", () => {
    const a = appearanceFor(input({ alpha: 0.6 }));
    expect(a.tokens["--window-alpha-chrome"]).toBe("0.6");
    expect(a.tokens["--window-alpha-content"]).toBe("0.74");
    expect(a.tokens["--window-opacity"]).toBe("1");
    expect(a.native.vibrancy).toBe(true);
    expect(a.native.alpha).toBe(1);
  });
  it("keep reading areas solid leaves cards at 1", () => {
    expect(appearanceFor(input({ alpha: 0.6, solid: true })).tokens["--window-alpha-content"]).toBe("1");
  });
  it("whole-window mode uses the native alpha and opaque surfaces", () => {
    const a = appearanceFor(input({ alpha: 0.5, mode: "whole" }));
    expect(a.tokens["--window-alpha-chrome"]).toBe("1");
    expect(a.tokens["--window-opacity"]).toBe("0.5");
    expect(a.native).toMatchObject({ vibrancy: false, alpha: 0.5 });
  });
  it("clamps to the slider's range", () => {
    expect(appearanceFor(input({ alpha: 0.01 })).alpha).toBe(MIN_ALPHA);
    expect(appearanceFor(input({ alpha: Number.NaN })).alpha).toBe(1);
  });
  it("is opaque on a platform without translucent windows, under the accessibility options, and in opaque pop-outs", () => {
    for (const rest of [{ supported: false }, { reduce: true }, { popout: true }]) {
      const a = appearanceFor(input({ alpha: 0.5, popouts: "opaque" }, rest));
      expect(a.alpha, JSON.stringify(rest)).toBe(1);
      expect(a.native.vibrancy).toBe(false);
    }
    expect(appearanceFor(input({ alpha: 0.5, popouts: "same" }, { popout: true })).alpha).toBe(0.5);
  });
  it("no blur behind the window when blur is off", () => {
    expect(appearanceFor(input({ alpha: 0.6, blur: false })).native.vibrancy).toBe(false);
  });
});

describe("overlay blur and text size", () => {
  it("uses the theme's blur until the user sets a strength", () => {
    expect(appearanceFor(input()).tokens["--window-blur"]).toBe("var(--glass-blur)");
    expect(appearanceFor(input({ blurStrength: 12 })).tokens["--window-blur"]).toBe("12px");
    expect(appearanceFor(input({ blurStrength: 0 })).tokens["--window-blur"]).toBe("0px");
    expect(appearanceFor(input({ blurStrength: 900 })).tokens["--window-blur"]).toBe("48px");
  });
  it("scales the interface with the text size", () => {
    expect(appearanceFor(input({ textSize: "small" })).tokens["--ui-scale"]).toBe("0.92");
    expect(appearanceFor(input({ textSize: "default" })).tokens["--ui-scale"]).toBe("1");
    expect(appearanceFor(input({ textSize: "large" })).tokens["--ui-scale"]).toBe("1.1");
  });
});

describe("theme choice", () => {
  it("follows the system when asked to", () => {
    expect(resolveTheme("system", true)).toBe("atrium-calm-dark");
    expect(resolveTheme("system", false)).toBe("atrium-calm-light");
    expect(resolveTheme("light", true)).toBe("atrium-calm-light");
    expect(resolveTheme("dark", false)).toBe("atrium-calm-dark");
  });
});

describe("native calls", () => {
  const shown = { vibrancy: false, dark: false, alpha: 1, appearance: "light" as const };
  it("calls nothing when nothing changed", () => {
    expect(nativeCalls(shown, { ...shown })).toEqual({});
  });
  it("re-applies vibrancy when the theme flips under it", () => {
    const on = { ...shown, vibrancy: true };
    expect(nativeCalls(on, { ...on, dark: true })).toEqual({ vibrancy: { enabled: true, dark: true } });
    expect(nativeCalls(shown, { ...shown, dark: true })).toEqual({});
  });
  it("sets only what differs", () => {
    expect(nativeCalls(shown, { ...shown, alpha: 0.5, appearance: "system" })).toEqual({ alpha: 0.5, appearance: "system" });
  });
});

describe("who shows a view", () => {
  it("is the pop-out that has the thread, else the main window", () => {
    const model = { ...initialModel(), popouts: { "view-3": { threadId: "t1", bounds: null } } };
    expect(ownerOf(model, "t1")).toBe("view-3");
    expect(ownerOf(model, "t2")).toBe(MAIN);
  });
});
