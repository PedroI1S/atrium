// Review 2 #9: every bundled font ships with its complete OFL notice as an app resource.
// scripts/check-bundle.sh verifies the notices are inside the built .app.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const ROOT = join(import.meta.dirname, "..");
const conf = JSON.parse(readFileSync(join(ROOT, "src-tauri/tauri.conf.json"), "utf8"));
const resources: Record<string, string> = conf.bundle.resources ?? {};
const fonts = [...new Set([...readFileSync(join(ROOT, "src/index.tsx"), "utf8").matchAll(/@fontsource\/([\w-]+)\//g)].map((m) => m[1]!))];

describe("font licence notices", () => {
  it("the app bundles at least the two fonts it imports", () => {
    expect(fonts.sort()).toEqual(["geist-mono", "instrument-sans"]);
  });
  for (const font of fonts) {
    it(`${font}: its OFL notice is a bundle resource under licenses/`, () => {
      const source = Object.keys(resources).find((k) => k.includes(`@fontsource/${font}/LICENSE`));
      expect(source, `no bundle.resources entry for ${font}`).toBeDefined();
      expect(resources[source!]).toMatch(/^licenses\/.+OFL-1\.1\.txt$/);
      const text = readFileSync(join(ROOT, "src-tauri", source!), "utf8");
      expect(text).toMatch(/Copyright \d{4}/);
      expect(text).toContain("SIL OPEN FONT LICENSE Version 1.1");
    });
  }
});
