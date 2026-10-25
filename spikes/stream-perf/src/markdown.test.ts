import { describe, expect, it } from "vitest";
import { renderBlock, renderOpen } from "./markdown";

describe("markdown", () => {
  it("renders inline syntax", () => {
    expect(renderBlock("**bold** and `code`")).toContain("<strong>bold</strong>");
    expect(renderBlock("**bold** and `code`")).toContain("<code>code</code>");
  });
  it("never injects raw HTML", () => {
    const html = renderBlock("<img src=x onerror=alert(1)>\n\n<script>alert(1)</script>");
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<script");
    expect(html).toContain("&lt;img");
  });
  it("shows an unfinished fence as escaped text without its opening line", () => {
    const html = renderOpen("```ts\nif (a < b) {", true);
    expect(html).toBe("<pre><code>if (a &lt; b) {</code></pre>");
  });
  it("renders an unfinished bold run without throwing", () => {
    expect(() => renderOpen("a **bo", false)).not.toThrow();
  });
});
