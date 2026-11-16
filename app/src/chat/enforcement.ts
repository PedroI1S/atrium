// How an `EnforcementDescriptor` reads in the "details" view (PLAN §3.12, contract §3): the
// level, coverage per tool class, the configuration the session loads, and why it is Partial.
// Coverage, policy and isolation stay three separate things. Pure; tested in enforcement.test.ts.

import type { Tone } from "../daemon/reduce";
import type { ConfigSource, ConfigSourceKind, Coverage, EnforcementDescriptor, ToolClass } from "../daemon/types";

/** `ToolClass::ALL`, in the order the details list them. */
export const TOOL_CLASSES: readonly ToolClass[] = ["read", "search", "edit", "move", "delete", "execute", "fetch", "mcp", "subagent", "provider_specific"];

export const CLASS_LABEL: Record<ToolClass, string> = {
  read: "File reads",
  search: "Searches",
  edit: "File edits",
  move: "Moves and renames",
  delete: "Deletions",
  execute: "Commands",
  fetch: "Web requests",
  mcp: "MCP tools",
  subagent: "Subagents",
  provider_specific: "Provider-specific tools",
};

export const COVERAGE: Record<Coverage, { label: string; tone: Tone }> = {
  intercepted: { label: "Checked before it runs", tone: "ready" },
  provider_gated: { label: "Checked only when the agent asks", tone: "needs-you" },
  not_intercepted: { label: "Runs without Atrium seeing it first", tone: "failed" },
  unknown: { label: "Not verified", tone: "needs-you" },
  not_applicable: { label: "This agent has none", tone: "idle" },
};

export const SOURCE_LABEL: Record<ConfigSourceKind, string> = {
  user_settings: "User settings",
  project_settings: "Project settings",
  local_settings: "Local settings",
  managed_policy: "Managed policy",
  explicit_settings: "Settings passed to the agent",
  plugin: "Plugin",
  hooks: "Hooks",
  other: "Other",
};

/** Weakest first: a class listed more than once counts as its weakest entry (contract §3). */
const STRENGTH: Record<Coverage, number> = { not_intercepted: 0, unknown: 1, provider_gated: 2, intercepted: 3, not_applicable: 4 };

export interface CoverageRow {
  cls: ToolClass;
  label: string;
  coverage: Coverage;
  text: string;
  tone: Tone;
  evidence: string | null;
}

/**
 * One row per tool class, in `TOOL_CLASSES` order. A class with several entries shows its
 * weakest; a class the descriptor doesn't mention is "Not verified" (unknown is Partial).
 */
export function coverageRows(d: EnforcementDescriptor): CoverageRow[] {
  return TOOL_CLASSES.map((cls) => {
    const entries = d.coverage.filter((c) => c.class === cls);
    const weakest = entries.reduce<(typeof entries)[number] | null>((w, c) => (w === null || STRENGTH[c.coverage] < STRENGTH[w.coverage] ? c : w), null);
    const coverage: Coverage = weakest?.coverage ?? "unknown";
    return { cls, label: CLASS_LABEL[cls], coverage, text: COVERAGE[coverage].label, tone: COVERAGE[coverage].tone, evidence: weakest?.evidence ?? null };
  });
}

/** The one-line explanation next to the level (PLAN §3.12). */
export function levelLine(d: EnforcementDescriptor): string {
  return d.level === "full"
    ? "Full: every tool call passes Atrium's rules before it runs."
    : "Partial: Atrium sees only some of what this agent does. Some reads or commands run unchecked.";
}

export function versionLine(d: EnforcementDescriptor): string | null {
  if (!d.cli_version) return "CLI version unknown";
  return d.version_tested ? `CLI version ${d.cli_version}, tested with Atrium` : `CLI version ${d.cli_version}, not tested with Atrium`;
}

export interface SourceRow {
  kind: string;
  path: string;
  inspected: string;
  hash: string | null;
}

const HASH_SHOWN = 12;

export function sourceRows(sources: readonly ConfigSource[]): SourceRow[] {
  return sources.map((s) => ({
    kind: SOURCE_LABEL[s.kind],
    path: s.path,
    inspected: s.inspected ? "Inspected by Atrium" : "Not inspected: what it does isn't known",
    hash: s.sha256 ? s.sha256.slice(0, HASH_SHOWN) : null,
  }));
}
