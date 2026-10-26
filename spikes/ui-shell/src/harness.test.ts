// Review 2 #4: measured runs must schedule like normal launches. The harness may activate the
// app (as a Dock or Finder launch does) but must not hold a process activity such as
// NSActivityLatencyCritical, which normal launches never have.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";

it("the app holds no scheduling activity in harness mode", () => {
  const rust = readFileSync(join(import.meta.dirname, "../src-tauri/src/lib.rs"), "utf8").replace(/\/\/.*$/gm, "");
  expect(rust).not.toMatch(/beginActivity|NSActivityOptions|LatencyCritical|disableAppNap/);
});
