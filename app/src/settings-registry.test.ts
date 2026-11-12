import { describe, expect, it } from "vitest";
import { MAX_ESSENTIALS, registerSection, searchSettings, type SettingsSectionSpec } from "./settings-registry";

const row = (id: string, label: string, hint?: string) => ({ id, label, hint, control: null });

describe("settings registry (ui-direction §3 Settings)", () => {
  it("keeps at most five essentials and moves the rest to Advanced, ahead of its own rows", () => {
    const spec = registerSection({
      id: "s", label: "Section",
      essentials: Array.from({ length: MAX_ESSENTIALS + 2 }, (_, i) => row(`e${i}`, `Essential ${i}`)),
      advanced: [row("a0", "Advanced 0")],
    });
    expect(spec.essentials.map((r) => r.id)).toEqual(["e0", "e1", "e2", "e3", "e4"]);
    expect(spec.advanced.map((r) => r.id)).toEqual(["e5", "e6", "a0"]);
    expect(spec.advanced[0]!.group).toBe("More");
  });
  it("leaves a small section as it is", () => {
    const spec = { id: "s", label: "S", essentials: [row("a", "A")], advanced: [] };
    expect(registerSection(spec)).toBe(spec);
  });
});

describe("settings search", () => {
  const sections: SettingsSectionSpec<null>[] = [
    { id: "appearance", label: "Appearance", essentials: [row("opacity", "Window opacity", "Lets the desktop show through")], advanced: [{ ...row("strength", "Blur strength"), hintNow: () => "Only affects overlays" }] },
    { id: "general", label: "General", essentials: [row("keep", "Keep agents running after quitting")], advanced: [] },
  ];
  it("searches labels, hints and section names, in Advanced too", () => {
    expect(searchSettings(sections, "opacity").map((m) => m.row)).toEqual(["opacity"]);
    expect(searchSettings(sections, "desktop").map((m) => m.row)).toEqual(["opacity"]);
    expect(searchSettings(sections, "appearance").map((m) => m.row)).toEqual(["opacity", "strength"]);
    expect(searchSettings(sections, "blur")[0]).toMatchObject({ section: "appearance", row: "strength", advanced: true });
  });
  it("searches a hint that depends on the platform", () => {
    expect(searchSettings(sections, "overlays").map((m) => m.row)).toEqual(["strength"]);
  });
  it("needs every word, ignores case, and returns nothing for an empty query", () => {
    expect(searchSettings(sections, "KEEP quitting").map((m) => m.row)).toEqual(["keep"]);
    expect(searchSettings(sections, "keep opacity")).toEqual([]);
    expect(searchSettings(sections, "   ")).toEqual([]);
  });
});
