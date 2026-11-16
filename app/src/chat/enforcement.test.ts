import { describe, expect, it } from "vitest";
import type { EnforcementDescriptor } from "../daemon/types";
import { TOOL_CLASSES, coverageRows, levelLine, sourceRows, versionLine } from "./enforcement";

const descriptor = (over: Partial<EnforcementDescriptor> = {}): EnforcementDescriptor => ({
  level: "partial",
  reasons: [{ code: "class_not_intercepted", message: "MCP calls run unchecked" }],
  provider: "claude",
  cli_version: "2.1.0",
  version_tested: true,
  coverage: [],
  config_sources: [],
  ...over,
});

describe("enforcement details: coverage", () => {
  it("lists every tool class once, in a fixed order", () => {
    const rows = coverageRows(descriptor({ coverage: [{ class: "execute", coverage: "intercepted", evidence: null }] }));
    expect(rows.map((r) => r.cls)).toEqual([...TOOL_CLASSES]);
  });
  it("shows a class the descriptor doesn't mention as not verified", () => {
    const rows = coverageRows(descriptor());
    expect(rows.every((r) => r.coverage === "unknown" && r.text === "Not verified")).toBe(true);
  });
  it("shows the weakest entry when a class has several, with its evidence", () => {
    const rows = coverageRows(descriptor({
      coverage: [
        { class: "mcp", coverage: "intercepted", evidence: "fixtures/a" },
        { class: "mcp", coverage: "not_intercepted", evidence: "fixtures/b" },
        { class: "mcp", coverage: "provider_gated", evidence: null },
        { class: "read", coverage: "not_applicable", evidence: null },
        { class: "read", coverage: "intercepted", evidence: "fixtures/r" },
      ],
    }));
    const mcp = rows.find((r) => r.cls === "mcp")!;
    expect(mcp).toMatchObject({ coverage: "not_intercepted", evidence: "fixtures/b", tone: "failed" });
    expect(rows.find((r) => r.cls === "read")).toMatchObject({ coverage: "intercepted", tone: "ready" });
  });
});

describe("enforcement details: level, version and sources", () => {
  it("explains each level in one line", () => {
    expect(levelLine(descriptor({ level: "full", reasons: [] }))).toMatch(/^Full: every tool call/);
    expect(levelLine(descriptor())).toMatch(/^Partial:/);
  });
  it("says whether the CLI version was tested", () => {
    expect(versionLine(descriptor())).toBe("CLI version 2.1.0, tested with Atrium");
    expect(versionLine(descriptor({ version_tested: false }))).toBe("CLI version 2.1.0, not tested with Atrium");
    expect(versionLine(descriptor({ cli_version: null }))).toBe("CLI version unknown");
  });
  it("names each configuration source, whether it was inspected, and a short hash", () => {
    const rows = sourceRows([
      { kind: "user_settings", path: "~/.claude/settings.json", sha256: "a".repeat(64), inspected: true },
      { kind: "plugin", path: "~/.claude/plugins/x", sha256: null, inspected: false },
    ]);
    expect(rows[0]).toEqual({ kind: "User settings", path: "~/.claude/settings.json", inspected: "Inspected by Atrium", hash: "aaaaaaaaaaaa" });
    expect(rows[1]).toMatchObject({ kind: "Plugin", hash: null });
    expect(rows[1]!.inspected).toMatch(/^Not inspected/);
  });
});
