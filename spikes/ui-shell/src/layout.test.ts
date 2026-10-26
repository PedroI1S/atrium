import { describe, expect, it } from "vitest";
import { clampWidth, panelTabs, rightColumn } from "./layout";
import { MAX_ESSENTIALS, registerSection, searchSettings } from "./settings-registry";

describe("right column (ui-direction §2.2, §3 Settings)", () => {
  it("Advanced opens at its own width even when the panel is minimized (review #9)", () => {
    expect(rightColumn("settings", false, true)).toEqual({ kind: "advanced", width: "--advanced-width" });
    expect(rightColumn("settings", true, true)).toEqual({ kind: "advanced", width: "--advanced-width" });
  });
  it("settings never shows the context panel", () => {
    expect(rightColumn("settings", true, false)).toEqual({ kind: "none" });
  });
  it("minimized panel is a strip; the editor's panel is narrower", () => {
    expect(rightColumn("chat", false, false).kind).toBe("strip");
    expect(rightColumn("chat", true, false)).toEqual({ kind: "panel", width: "--panel-width" });
    expect(rightColumn("editor", true, false)).toEqual({ kind: "panel", width: "--panel-width-editor" });
  });
  it("panel tabs depend on the main card", () => {
    expect(panelTabs("chat")).toEqual({ tabs: ["changes", "checks", "files", "terminal"], initial: "changes" });
    expect(panelTabs("editor").initial).toBe("chat");
    expect(panelTabs("inbox").tabs).toEqual(["preview"]);
  });
  it("clamps dragged widths", () => {
    expect(clampWidth(10, 200, 360)).toBe(200);
    expect(clampWidth(999, 200, 360)).toBe(360);
    expect(clampWidth(250.4, 200, 360)).toBe(250);
  });
});

describe("settings registry", () => {
  const row = (id: string) => ({ id, label: `Label ${id}`, control: null });
  it("moves essentials beyond the limit to Advanced", () => {
    const s = registerSection({ id: "x", label: "X", essentials: ["a", "b", "c", "d", "e", "f", "g"].map(row), advanced: [row("z")] });
    expect(s.essentials).toHaveLength(MAX_ESSENTIALS);
    expect(s.advanced.map((r) => r.id)).toEqual(["f", "g", "z"]);
  });
  it("search covers Advanced and reports where a match lives", () => {
    const sections = [
      { id: "appearance", label: "Appearance", essentials: [{ id: "opacity", label: "Window opacity", control: null }], advanced: [{ id: "popouts", label: "Popped-out windows", hint: "opacity of pop-outs", control: null }] },
      { id: "general", label: "General", essentials: [{ id: "login", label: "Open at login", control: null }], advanced: [] },
    ];
    expect(searchSettings(sections, "opacity").map((m) => [m.row, m.advanced])).toEqual([["opacity", false], ["popouts", true]]);
    expect(searchSettings(sections, "pop up")).toEqual([]);
    expect(searchSettings(sections, "popped windows")[0]).toMatchObject({ section: "appearance", advanced: true });
    expect(searchSettings(sections, "  ")).toEqual([]);
  });
});
