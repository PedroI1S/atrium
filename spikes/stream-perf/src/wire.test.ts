import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { ENCODINGS, type Batch, decodeBatch, messageFromBytes } from "./wire";

// Fixtures come from `cargo run --example stream-perf-encode-bench -- --fixtures ../test-fixtures`
// (Rust encoders), so these tests prove the TypeScript decoders agree with them.
const dir = new URL("../test-fixtures/", import.meta.url);
const index = (id: string) => (id === "t-b" ? 1 : 0);

function load(enc: (typeof ENCODINGS)[number]): Batch {
  const bytes = readFileSync(new URL(`batch.${enc}`, dir));
  const ab = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
  return decodeBatch(enc, messageFromBytes(enc, ab), index);
}

describe("decoders agree with the Rust encoders", () => {
  const reference = load("json");

  it("json reference has the expected shape", () => {
    expect(reference.seq).toBe(42);
    expect(reference.sent).toBe(1_760_000_000_500.25);
    expect(reference.events.map((e) => e.id)).toEqual([1, 2, 3, 4, 5, 6]);
    const d = reference.events[1]!;
    expect(d.delta && d.text).toBe('Hello, "wor\\ld"\n\n');
    const d2 = reference.events[2]!;
    expect(d2.delta && d2.text).toBe("café \u{1f600} ok");
    expect(reference.events[2]!.thread).toBe(1);
  });

  for (const enc of ENCODINGS) {
    it(`${enc} decodes to the same events`, () => {
      expect(load(enc)).toEqual(reference);
    });
  }
});
