// @vitest-environment happy-dom
// Contract §12, rule by rule, with hostile inputs: what the parser produces and what the DOM
// ends up holding. The DOM checks walk every node the renderer made.

import { describe, expect, it, vi } from "vitest";
import { classifyImage, classifyLink } from "./links";
import { MAX_INLINE_IMAGE_BYTES } from "./limits";
import { type Block, type Inline, parseInline, parseMarkdown } from "./markdown";
import { type LinkActions, renderBlocks } from "./render";

const actions = (): LinkActions & { opened: string[]; revealed: string[] } => {
  const opened: string[] = [];
  const revealed: string[] = [];
  return { opened, revealed, open: (u) => opened.push(u), reveal: (p) => revealed.push(p) };
};

function render(md: string, a: LinkActions = actions()): HTMLDivElement {
  const div = document.createElement("div");
  div.append(renderBlocks(document, parseMarkdown(md), a));
  return div;
}

const ALLOWED_TAGS = new Set(["DIV", "P", "H1", "H2", "H3", "H4", "H5", "H6", "PRE", "CODE", "BLOCKQUOTE", "UL", "OL", "LI", "HR", "TABLE", "THEAD", "TBODY", "TR", "TH", "TD", "EM", "STRONG", "DEL", "BR", "A", "SPAN", "IMG", "BUTTON"]);
const ALLOWED_ATTRS = new Set(["class", "href", "title", "rel", "alt", "src", "decoding", "start", "tabindex", "role", "type", "data-lang", "referrerpolicy", "crossorigin"]);

/** Every element is one the renderer makes, with attributes it sets, and nothing active. */
function assertInert(root: Element): void {
  for (const node of [root, ...root.querySelectorAll("*")]) {
    expect(ALLOWED_TAGS.has(node.tagName), `tag ${node.tagName}`).toBe(true);
    for (const attr of node.getAttributeNames()) {
      expect(ALLOWED_ATTRS.has(attr), `attribute ${attr} on ${node.tagName}`).toBe(true);
      expect(attr.startsWith("on")).toBe(false);
    }
    if (node.tagName === "A" && node.hasAttribute("href")) expect(node.getAttribute("href")).toMatch(/^(https?:\/\/|mailto:)/);
    if (node.tagName === "IMG") expect(node.getAttribute("src")).toMatch(/^data:image\/(png|jpeg|gif|webp);base64,/);
  }
}

const find = (blocks: Block[], t: Block["t"]) => blocks.filter((b) => b.t === t);
const inl = (s: string) => parseInline(s);
const PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

describe("raw HTML is text (§12)", () => {
  it("a script element in markdown is shown as its text", () => {
    const div = render("Hello <script>alert(1)</script> there");
    expect(div.querySelector("script")).toBeNull();
    expect(div.textContent).toBe("Hello <script>alert(1)</script> there");
    assertInert(div);
  });

  it("an HTML block is a paragraph of text", () => {
    const div = render('<div onclick="x()">\n<iframe src="https://evil.example"></iframe>\n</div>');
    expect(div.querySelector("iframe,div div")).toBeNull();
    expect(div.textContent).toContain("<iframe");
    assertInert(div);
  });

  it("img onerror stays text", () => {
    const div = render('<img src=x onerror="alert(1)">');
    expect(div.querySelector("img")).toBeNull();
    expect(div.textContent).toBe('<img src=x onerror="alert(1)">');
    assertInert(div);
  });

  it("inline SVG markup stays text", () => {
    const div = render('<svg onload="alert(1)"><script>alert(2)</script></svg>');
    expect(div.querySelector("svg,script")).toBeNull();
    assertInert(div);
  });

  it("entities are not decoded into markup", () => {
    const div = render("&lt;b&gt;bold&lt;/b&gt;");
    expect(div.querySelector("b")).toBeNull();
    expect(div.textContent).toBe("&lt;b&gt;bold&lt;/b&gt;");
  });
});

describe("links (§12)", () => {
  it("http, https and mailto become links that open through the app after one click", () => {
    const a = actions();
    const div = render("[docs](https://example.com/a?b=c) and <mailto:me@example.com> and http://example.org/x.", a);
    const links = [...div.querySelectorAll("a")];
    expect(links.map((l) => l.getAttribute("href"))).toEqual(["https://example.com/a?b=c", "mailto:me@example.com", "http://example.org/x"]);
    // The full URL is shown on hover.
    expect(links.map((l) => l.title)).toEqual(["https://example.com/a?b=c", "mailto:me@example.com", "http://example.org/x"]);
    const click = new MouseEvent("click", { bubbles: true, cancelable: true });
    links[0]!.dispatchEvent(click);
    expect(click.defaultPrevented).toBe(true);
    expect(a.opened).toEqual(["https://example.com/a?b=c"]);
    assertInert(div);
  });

  it("javascript:, data:, vbscript:, tauri: and other schemes are plain text", () => {
    for (const url of ["javascript:alert(1)", "JaVaScRiPt:alert(1)", "java\tscript:alert(1)", "data:text/html,<script>alert(1)</script>", "vbscript:msgbox", "tauri://localhost/index.html", "ipc://localhost/x", "file://evil.example/share", "ftp://example.com/"]) {
      const div = render(`[click me](${url})`);
      expect(div.querySelector("a"), url).toBeNull();
      expect(div.textContent).toContain("click me");
      assertInert(div);
    }
  });

  it("a javascript: autolink is not a link", () => {
    const div = render("<javascript:alert(1)>");
    expect(div.querySelector("a")).toBeNull();
    assertInert(div);
  });

  it("a file: link is revealed through the app, never navigated to", () => {
    const a = actions();
    const div = render("[notes](file:///Users/me/notes%20today.md)", a);
    const link = div.querySelector("a")!;
    expect(link.hasAttribute("href")).toBe(false);
    expect(link.title).toContain("/Users/me/notes today.md");
    link.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    expect(a.revealed).toEqual(["/Users/me/notes today.md"]);
    expect(a.opened).toEqual([]);
  });

  it("classifies links", () => {
    expect(classifyLink("https://x.example/")).toEqual({ kind: "web", url: "https://x.example/" });
    expect(classifyLink("file://localhost/etc/hosts")).toEqual({ kind: "file", path: "/etc/hosts" });
    expect(classifyLink("file:relative").kind).toBe("none");
    expect(classifyLink("https://").kind).toBe("none");
    expect(classifyLink(`https://x.example/${"a".repeat(9000)}`).kind).toBe("none");
    expect(classifyLink("https://x.example/a\u2028b").kind).toBe("none");
  });

  it("a link inside a link is not a link", () => {
    const tree = inl("[a [b](https://b.example) c](https://a.example)");
    const nested = (items: Inline[], inLink: boolean): boolean =>
      items.some((i) => (i.t === "link" ? inLink || nested(i.c, true) : "c" in i ? nested(i.c, inLink) : false));
    expect(nested(tree, false)).toBe(false);
    // The inner one is the link; the outer brackets are text.
    expect(tree[1]).toMatchObject({ t: "link", target: { kind: "web", url: "https://b.example/" } });
  });
});

describe("images (§12)", () => {
  it("a small data: PNG renders", () => {
    const div = render(`![dot](data:image/png;base64,${PNG})`);
    const img = div.querySelector("img")!;
    expect(img.getAttribute("src")).toBe(`data:image/png;base64,${PNG}`);
    expect(img.alt).toBe("dot");
    assertInert(div);
  });

  it("SVG never renders inline, as data or remote", () => {
    for (const src of ["data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=", "<data:image/svg+xml,%3Csvg onload=alert(1)%3E>", "https://x.example/a.svg"]) {
      const div = render(`![x](${src})`);
      expect(div.querySelector("img,svg"), src).toBeNull();
      expect(div.textContent).toContain("SVG images are not shown");
      assertInert(div);
    }
    // With a space in it, it isn't even an image: text.
    const div = render("![x](data:image/svg+xml,<svg onload=alert(1)>)");
    expect(div.querySelector("img,svg")).toBeNull();
    assertInert(div);
  });

  it("a data image above the limit is refused, not decoded", () => {
    const huge = "A".repeat(Math.ceil((MAX_INLINE_IMAGE_BYTES + 3) / 3) * 4);
    const target = classifyImage(`data:image/png;base64,${huge}`);
    expect(target.kind).toBe("refused");
    const div = render(`![big](data:image/png;base64,${huge})`);
    expect(div.querySelector("img")).toBeNull();
    expect(div.textContent).toContain("too large");
    // Exactly at the limit is fine.
    const exact = "A".repeat((MAX_INLINE_IMAGE_BYTES / 3) * 4 - 4) + "AAA=";
    expect(classifyImage(`data:image/png;base64,${exact}`).kind).toBe("data");
  });

  it("other data types and non-base64 data are refused", () => {
    expect(classifyImage("data:text/html;base64,PHNjcmlwdD4=").kind).toBe("refused");
    expect(classifyImage("data:image/png,rawbytes").kind).toBe("refused");
    expect(classifyImage("data:image/png;base64,<script>").kind).toBe("refused");
  });

  it("remote images wait for a click, then load with no referrer and no cookies", () => {
    const div = render("![chart](https://img.example/c.png)");
    expect(div.querySelector("img")).toBeNull();
    const button = div.querySelector("button")!;
    expect(button.textContent).toContain("img.example");
    button.click();
    const img = div.querySelector("img")!;
    expect(img.getAttribute("referrerpolicy")).toBe("no-referrer");
    expect(img.getAttribute("crossorigin")).toBe("anonymous");
    expect(img.getAttribute("src")).toBe("https://img.example/c.png");
  });

  it("http (not https) images are refused", () => {
    expect(classifyImage("http://img.example/c.png").kind).toBe("refused");
    expect(classifyImage("javascript:alert(1)").kind).toBe("refused");
  });
});

describe("blocks", () => {
  it("headings, paragraphs, lists, quotes, rules and tables", () => {
    const blocks = parseMarkdown("# Title\n\nSome *em* and **strong** and ~~gone~~ and `code`.\n\n- a\n- b\n  - nested\n\n3. three\n4. four\n\n> quoted\n\n---\n\n| a | b |\n|:--|--:|\n| 1 | 2 |\n");
    expect(blocks.map((b) => b.t)).toEqual(["h", "p", "list", "list", "quote", "hr", "table"]);
    const ordered = find(blocks, "list")[1] as Extract<Block, { t: "list" }>;
    expect(ordered.start).toBe(3);
    const table = find(blocks, "table")[0] as Extract<Block, { t: "table" }>;
    expect(table.align).toEqual(["left", "right"]);
    const div = render("3. three\n4. four");
    expect(div.querySelector("ol")!.getAttribute("start")).toBe("3");
  });

  it("nested fences: a longer fence holds a shorter one", () => {
    const blocks = parseMarkdown("````md\n```js\nalert(1)\n```\n````\nafter");
    expect(blocks[0]).toEqual({ t: "code", lang: "md", v: "```js\nalert(1)\n```", open: false });
    expect(blocks[1]!.t).toBe("p");
  });

  it("a tilde fence is not closed by backticks, and an unclosed fence stays open", () => {
    const blocks = parseMarkdown("~~~\n```\nstill code\n");
    expect(blocks).toEqual([{ t: "code", lang: "", v: "```\nstill code\n", open: true }]);
  });

  it("code in a fence is text, whatever it holds", () => {
    const div = render("```html\n<script>alert(1)</script>\n<img src=x onerror=alert(1)>\n```");
    expect(div.querySelector("script,img")).toBeNull();
    expect(div.querySelector("pre code")!.textContent).toBe("<script>alert(1)</script>\n<img src=x onerror=alert(1)>");
    assertInert(div);
  });

  it("deep nesting stops instead of recursing without bound", () => {
    const deep = `${"> ".repeat(500)}x`;
    expect(() => parseMarkdown(deep)).not.toThrow();
    const list = Array.from({ length: 300 }, (_, i) => `${"  ".repeat(i)}- x`).join("\n");
    expect(() => parseMarkdown(list)).not.toThrow();
  });

  it("pathological emphasis and brackets parse quickly", () => {
    const start = performance.now();
    parseInline("*a".repeat(20_000));
    parseInline("[".repeat(20_000));
    parseInline("**_".repeat(10_000));
    parseInline("`".repeat(20_000));
    expect(performance.now() - start).toBeLessThan(2000);
  });
});

describe("inlines", () => {
  const types = (items: Inline[]) => items.map((i) => i.t);
  it("emphasis, strong, strikethrough, code, escapes and breaks", () => {
    expect(types(inl("*a* **b** ~~c~~ `d` \\*e\\* f  \ng"))).toEqual(["em", "text", "strong", "text", "del", "text", "code", "text", "br", "text"]);
    expect(inl("snake_case_name")).toEqual([{ t: "text", v: "snake_case_name" }]);
    expect(inl("2 * 3 * 4")).toEqual([{ t: "text", v: "2 * 3 * 4" }]);
  });

  it("bare URLs drop trailing punctuation", () => {
    const tree = inl("see https://example.com/a_(b), then");
    const link = tree.find((i) => i.t === "link") as Extract<Inline, { t: "link" }>;
    expect(link.target).toEqual({ kind: "web", url: "https://example.com/a_(b)" });
  });

  it("a link's title is accepted and not shown", () => {
    const div = render('[x](https://example.com "a title")');
    expect(div.querySelector("a")!.title).toBe("https://example.com/");
  });
});

describe("the renderer never parses HTML", () => {
  it("does not use innerHTML or insertAdjacentHTML", async () => {
    const { readFileSync } = await import("node:fs");
    const { join } = await import("node:path");
    for (const file of ["render.ts", "markdown.ts", "../ui/Transcript.tsx", "../ui/RequestCard.tsx", "../ui/InboxCard.tsx"]) {
      const src = readFileSync(join(import.meta.dirname, file), "utf8");
      expect(src, file).not.toMatch(/innerHTML|outerHTML|insertAdjacentHTML|DOMParser|createContextualFragment|document\.write/);
    }
  });

  it("a hostile document of every kind stays inert", () => {
    const spy = vi.spyOn(Element.prototype, "setAttribute");
    const div = render([
      "<script>x</script>",
      "[a](javascript:alert(1)) [b](data:text/html,x) [c](https://ok.example)",
      "![s](data:image/svg+xml;base64,PHN2Zz4=) ![p](data:image/png;base64," + PNG + ")",
      "```\n<style>body{display:none}</style>\n```",
      "<a href=\"javascript:alert(1)\">x</a>",
      "| <b>x</b> | y |\n|---|---|\n| <i onmouseover=x> | z |",
    ].join("\n\n"));
    assertInert(div);
    for (const [name] of spy.mock.calls) expect(String(name).startsWith("on")).toBe(false);
    spy.mockRestore();
  });
});
