import { describe, expect, it } from "vitest";
import { normalizeUrl } from "./url";

const BASE = "http://127.0.0.1:5555";

describe("normalizeUrl", () => {
  it("keeps full URLs and special schemes", () => {
    expect(normalizeUrl("http://127.0.0.1:5555/a.html", BASE)).toBe("http://127.0.0.1:5555/a.html");
    expect(normalizeUrl("about:blank", BASE)).toBe("about:blank");
    expect(normalizeUrl("", BASE)).toBe("about:blank");
  });
  it("resolves paths against the fixture server", () => {
    expect(normalizeUrl("/click.html", BASE)).toBe("http://127.0.0.1:5555/click.html");
    expect(normalizeUrl("form.html", BASE)).toBe("http://127.0.0.1:5555/form.html");
  });
  it("uses http for local hosts and https for other hosts", () => {
    expect(normalizeUrl("localhost:3000/x", BASE)).toBe("http://localhost:3000/x");
    expect(normalizeUrl("127.0.0.1:8080", BASE)).toBe("http://127.0.0.1:8080");
    expect(normalizeUrl("example.com/a", BASE)).toBe("https://example.com/a");
  });
});
