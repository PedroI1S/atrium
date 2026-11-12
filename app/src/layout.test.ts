import { describe, expect, it } from "vitest";
import { clampWidth, panelTabs, rightColumn } from "./layout";

describe("right column", () => {
  it("shows the panel, or its strip when minimized", () => {
    expect(rightColumn("chat", true, false)).toEqual({ kind: "panel", width: "--panel-width" });
    expect(rightColumn("inbox", false, false)).toEqual({ kind: "strip", width: "--panel-strip-width" });
  });
  it("settings shows the Advanced card or nothing, never the panel or its strip", () => {
    expect(rightColumn("settings", true, false)).toEqual({ kind: "none" });
    expect(rightColumn("settings", false, true)).toEqual({ kind: "advanced", width: "--advanced-width" });
  });
  it("every screen has a default tab among its tabs", () => {
    for (const screen of ["chat", "inbox", "settings"] as const) expect(panelTabs(screen).tabs).toContain(panelTabs(screen).initial);
  });
  it("dragged widths stay inside their bounds", () => {
    expect(clampWidth(100, 200, 360)).toBe(200);
    expect(clampWidth(500.4, 200, 360)).toBe(360);
    expect(clampWidth(244.6, 200, 360)).toBe(245);
  });
});
