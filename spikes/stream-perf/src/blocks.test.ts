import { describe, expect, it } from "vitest";
import { BlockSplitter } from "./blocks";

function run(chunks: string[]) {
  const s = new BlockSplitter();
  const closed: string[] = [];
  for (const c of chunks) s.feed(c, (b) => closed.push(b));
  return { s, closed };
}

const DOC = "# Title\n\nFirst paragraph\nstill first.\n\n- a\n- b\n\n```rust\nfn main() {\n\n    println!();\n}\n```\n\nLast para";

describe("BlockSplitter", () => {
  it("splits on blank lines and keeps fences whole", () => {
    const { s, closed } = run([DOC]);
    expect(closed).toEqual(["# Title\n", "First paragraph\nstill first.\n", "- a\n- b\n", "```rust\nfn main() {\n\n    println!();\n}\n```\n"]);
    expect(s.openText()).toBe("Last para");
  });

  it("gives the same blocks however the text is chunked", () => {
    const whole = run([DOC]);
    for (const size of [1, 2, 3, 4, 7, 50]) {
      const chunks: string[] = [];
      for (let i = 0; i < DOC.length; i += size) chunks.push(DOC.slice(i, i + size));
      const r = run(chunks);
      expect(r.closed).toEqual(whole.closed);
      expect(r.s.openText()).toBe(whole.s.openText());
    }
  });

  it("reports an unfinished fence as open and renders its tail as the open text", () => {
    const { s, closed } = run(["text\n\n```ts\nconst a = 1;\n\nconst b"]);
    expect(closed).toEqual(["text\n"]);
    expect(s.isFenceOpen()).toBe(true);
    expect(s.openText()).toBe("```ts\nconst a = 1;\n\nconst b");
  });

  it("seal closes whatever is open and resets", () => {
    const { s, closed } = run(["abc\ndef"]);
    s.seal((b) => closed.push(b));
    expect(closed).toEqual(["abc\ndef"]);
    expect(s.openText()).toBe("");
  });

  it("a closing fence needs the same character and at least the same length", () => {
    const { s, closed } = run(["````\ncode\n```\nstill code\n````\n\nafter"]);
    expect(closed).toEqual(["````\ncode\n```\nstill code\n````\n"]);
    expect(s.openText()).toBe("after");
  });
});
