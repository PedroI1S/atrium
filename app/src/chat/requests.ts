// What an approval or question card shows and offers (contract §5, D-054, D-062). Pure; tested
// in requests.test.ts. The daemon's ledger is the only authority: the card never decides a
// request is allowed, it only shows what the events and the daemon's answers say.

import type { ApprovalDecision, ErrorCode, RememberScope, RequestOutcome, Value } from "../daemon/types";
import type { ApprovalItem, RequestItem } from "./transcript";

/** What this window did with a request. */
export type Local =
  | { status: "sending"; action: Action }
  /** The daemon accepted it; the resolution event follows. */
  | { status: "accepted"; action: Action }
  | { status: "refused"; action: Action; code: ErrorCode | null; message: string; unknownOutcome: boolean };

export type Action = "allow" | "deny" | "answer" | "cancel";

export type CardState =
  /** Pending: the buttons work. */
  | { kind: "fresh" }
  | { kind: "sending"; action: Action }
  /** Accepted by the daemon, waiting for the resolution event. */
  | { kind: "sent"; action: Action }
  /** It can't be answered any more; nothing was allowed. */
  | { kind: "stale"; text: string }
  | { kind: "resolved"; here: boolean; outcome: RequestOutcome; text: string }
  /** The daemon refused for another reason, or the outcome is unknown: it may be tried again. */
  | { kind: "failed"; text: string };

const RESOLVED_HERE: Record<RequestOutcome, string> = {
  approved: "Allowed",
  denied: "Denied",
  answered: "Answered",
  cancelled: "Withdrawn: the turn ended first",
  expired: "Expired: the agent stopped waiting",
};
const RESOLVED_ELSEWHERE: Record<RequestOutcome, string> = {
  approved: "Allowed in another window",
  denied: "Denied in another window",
  answered: "Answered in another window",
  cancelled: "Withdrawn: the turn ended first",
  expired: "Expired: the agent stopped waiting",
};

export const STALE_TEXT = "This request expired: the turn moved on, and nothing was allowed.";

export function cardState(request: RequestItem, local: Local | undefined): CardState {
  if (request.outcome !== null) {
    const here = local !== undefined && local.status !== "refused";
    return { kind: "resolved", here, outcome: request.outcome, text: (here ? RESOLVED_HERE : RESOLVED_ELSEWHERE)[request.outcome] };
  }
  if (local?.status === "refused" && local.code === "expired") return { kind: "stale", text: STALE_TEXT };
  if (request.stale) return { kind: "stale", text: STALE_TEXT };
  if (local?.status === "sending") return { kind: "sending", action: local.action };
  if (local?.status === "accepted") return { kind: "sent", action: local.action };
  if (local?.status === "refused") {
    return { kind: "failed", text: local.unknownOutcome ? `${local.message}. It may have been sent; check the chat before trying again.` : local.message };
  }
  return { kind: "fresh" };
}

/** Whether the card's buttons act. */
export const canAct = (state: CardState): boolean => state.kind === "fresh" || state.kind === "failed";

/** "Always…" choices (contract §5, PLAN §3.12), worded per D-062: remembered rules are global. */
export interface RememberChoice { scope: RememberScope; label: string; hint: string }

export function rememberChoices(request: ApprovalItem): RememberChoice[] {
  if (!request.rememberable) return [];
  const what = request.toolKind === "execute" ? "this exact command" : "this exact path";
  return [
    { scope: "thread", label: "Always in this chat", hint: `Allows ${what} again in this chat, in any folder` },
    { scope: "home", label: "Always in Home", hint: `Allows ${what} again in every Home chat, in any folder` },
    { scope: "global", label: "Always, everywhere", hint: `Allows ${what} again in any chat, in any folder` },
  ];
}

export function decisionOf(action: "allow" | "deny", message: string | null = null): ApprovalDecision {
  return action === "allow" ? { type: "allow" } : { type: "deny", message };
}

/** The exact command or paths an approval is about (D-054: shown exactly, never summarised). */
export function subjectOf(request: ApprovalItem): { label: string; text: string } {
  const input = request.input;
  const obj = input !== null && typeof input === "object" && !Array.isArray(input) ? (input as Record<string, Value>) : null;
  const str = (v: Value | undefined) => (typeof v === "string" ? v : null);
  if (request.toolKind === "execute") {
    const cmd = obj ? (str(obj.command) ?? str(obj.cmd) ?? (Array.isArray(obj.command) ? obj.command.map(String).join(" ") : null)) : str(input);
    if (cmd !== null) return { label: "Command", text: cmd };
  }
  if (obj) {
    const paths = [obj.file_path, obj.path, obj.notebook_path, obj.filePath].map(str).filter((p): p is string => p !== null);
    const list = Array.isArray(obj.paths) ? obj.paths.map(String) : [];
    const all = [...paths, ...list];
    if (all.length) return { label: all.length === 1 ? "Path" : "Paths", text: all.join("\n") };
    const url = str(obj.url);
    if (url) return { label: "Address", text: url };
  }
  if (request.title) return { label: request.name, text: request.title };
  return { label: request.name, text: JSON.stringify(input, null, 2) };
}

/** The card's question, by tool kind ("Run this command?"). */
export function askOf(request: ApprovalItem): string {
  switch (request.toolKind) {
    case "execute": return "Run this command?";
    case "edit": return "Change this file?";
    case "delete": return "Delete this?";
    case "move": return "Move this?";
    case "fetch": return "Fetch this address?";
    case "read": return "Read this?";
    case "search": return "Search here?";
    case "mcp": return `Use ${request.name}?`;
    default: return `Allow ${request.name}?`;
  }
}

/** Answers for `AnswerQuestion`: every question needs a choice or text. */
export function answersComplete(questions: { id: string; options?: unknown[]; allow_text?: boolean }[], picks: Record<string, { selected: string[]; text: string }>): boolean {
  return questions.every((q) => {
    const p = picks[q.id];
    if (!p) return false;
    return p.selected.length > 0 || p.text.trim() !== "";
  });
}
