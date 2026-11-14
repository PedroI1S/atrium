import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import * as limits from "./limits";

const RUST = readFileSync(join(import.meta.dirname, "../../../crates/atrium-protocol/src/limits.rs"), "utf8");

/** Evaluates `pub const NAME: usize = 8 * 1024 * 1024;` (products of integers only). */
function rustConst(name: string): number {
  const found = new RegExp(`pub const ${name}: \\w+ = ([\\d_ *]+);`).exec(RUST);
  if (!found) throw new Error(`${name} is not in limits.rs`);
  return found[1]!.split("*").reduce((product, factor) => product * Number(factor.trim().replaceAll("_", "")), 1);
}

describe("limits mirrored from limits.rs", () => {
  for (const [name, value] of Object.entries(limits)) {
    it(`${name} matches the Rust constant`, () => expect(value).toBe(rustConst(name)));
  }
});
