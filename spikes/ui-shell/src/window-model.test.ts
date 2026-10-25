import { describe, expect, it } from "vitest";
import { initialModel, moveView, opacityTokens } from "./window-model";
import { loadTheme } from "./theme/loader";
import { readFileSync } from "node:fs";
const calm = readFileSync(new URL("../themes/atrium-calm-dark.toml", import.meta.url), "utf8");

describe("CR-02 opacity", () => {
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
  it("background preserves ink; whole mode leaves surfaces solid and uses native alpha", () => {
    const s = { ...initialModel().settings, alpha: 0.6 };
    expect(opacityTokens(s, true)).toEqual({ "--window-alpha-chrome": "0.6", "--window-alpha-content": "0.74", "--window-opacity": "1", "--window-blur": "24px" });
    expect(opacityTokens({ ...s, solid: true }, true)["--window-alpha-content"]).toBe("1");
    expect(opacityTokens({ ...s, mode: "whole" }, true)["--window-opacity"]).toBe("0.6");
    expect(opacityTokens({ ...s, mode: "whole" }, true)["--window-alpha-content"]).toBe("1");
  });
  it("uses opaque fallback, accessibility override and bounds", () => {
    const s = { ...initialModel().settings, alpha: 0.3 };
    for (const t of [opacityTokens(s, false), opacityTokens(s, true, true)]) {
      expect(t["--window-alpha-chrome"]).toBe("1"); expect(t["--window-opacity"]).toBe("1");
    }
    expect(opacityTokens({ ...s, alpha: -10 }, true)["--window-alpha-chrome"]).toBe("0.3");
    expect(opacityTokens({ ...s, alpha: NaN }, true)["--window-alpha-chrome"]).toBe("1");
  });
});
it("round-trips all view state through pop-out, serialization and close", () => {
  for (const id of ["chat", "editor", "database"] as const) {
    const state = initialModel();
    state.views[id] = { threadId: "t-flaky", owner: "main", draft: "unfinished", messages: ["hello"], scroll: 425, content: "edited" };
    const detached = moveView(state, id, `view-${id}`);
    expect(detached.views[id].owner).toBe(`view-${id}`);
    const restored = moveView(JSON.parse(JSON.stringify(detached)), id, "main");
    expect(restored.views).toEqual(state.views);
    expect(restored.revision).toBe(2);
    expect(state.views[id].owner).toBe("main");
  }
});
