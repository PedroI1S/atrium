// How a remembered rule reads in Settings → Approvals (contract §5 and §9; D-054: the exact
// command, never a summary; D-062: rules apply in any folder). Pure; tested in rules.test.ts.

import type { Rule, ToolKind } from "../daemon/types";

const KIND: Record<ToolKind, string> = {
  execute: "command",
  edit: "file edit",
  read: "file read",
  delete: "deletion",
  move: "move",
  search: "search",
  fetch: "web request",
  think: "thinking step",
  mcp: "MCP tool call",
  other: "tool call",
};

/** One argument as a shell would need it written, so the argv reads back exactly. */
export function quoteArg(arg: string): string {
  if (arg !== "" && /^[\w@%+=:,./-]+$/.test(arg)) return arg;
  return `'${arg.replaceAll("'", "'\\''")}'`;
}

export const argvLine = (argv: readonly string[]): string => argv.map(quoteArg).join(" ");

export interface RuleText {
  /** "Allow" or "Deny". */
  decision: string;
  /** What it matches, in words. */
  what: string;
  /** The exact command or glob, shown in mono; null when the rule has none. */
  code: string | null;
  /** Where it applies. */
  where: string;
  /** How it came to be. */
  origin: string;
}

export function ruleText(rule: Rule, titleOf: (threadId: string) => string | undefined): RuleText {
  // The tool in brackets: "by Edit of" read badly when the tool is named after its kind.
  const tool = rule.tool_name ? ` (${rule.tool_name})` : "";
  let what: string;
  let code: string | null = null;
  switch (rule.matcher.type) {
    case "any":
      what = `every ${KIND[rule.tool_kind]}${tool}`;
      break;
    case "command_exact":
      what = "this exact command";
      code = argvLine(rule.matcher.argv);
      break;
    case "command_prefix":
      what = "commands that start with";
      code = `${argvLine(rule.matcher.argv)} …`;
      break;
    case "path_glob":
      what = `every ${KIND[rule.tool_kind]}${tool} of`;
      code = rule.matcher.glob;
      break;
  }
  let scope: string;
  switch (rule.scope.type) {
    case "thread": {
      const title = titleOf(rule.scope.thread_id);
      scope = title === undefined ? "In one chat that is no longer listed" : `In the chat “${title || "Untitled chat"}”`;
      break;
    }
    case "home":
      scope = "In every Home chat";
      break;
    case "global":
      scope = "In every chat";
      break;
  }
  const where = rule.path_scope ? `${scope}, only for paths inside ${rule.path_scope}` : `${scope}, in any folder`;
  const when = new Date(rule.created_at_ms).toLocaleDateString();
  const origin = rule.origin.type === "approval" ? `Remembered from an approval on ${when}` : `Added in settings on ${when}`;
  return { decision: rule.decision === "allow" ? "Allow" : "Deny", what, code, where, origin };
}
