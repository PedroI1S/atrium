// Batch approve and deny in the inbox's approval queue (PLAN §3.4): select several pending
// approvals, then Allow once or Deny them together. Each one still goes through the ledger
// alone (`ResolveApproval`), so one can fail (expired) while the others go through. There is no
// batch "Always…": rules are made one at a time. Pure; tested in batch.test.ts.

import type { Local } from "./requests";
import type { ApprovalItem } from "./transcript";

/** One approval the batch can act on. */
export interface BatchTarget {
  key: string;
  threadId: string;
  request: ApprovalItem;
  /** The chat's title, to name a failure. */
  chat: string;
  /** What it asks, in a few words (the command or path). */
  subject: string;
}

/** A request's identity across windows and incarnations (the same key `threads.ts` uses). */
export const batchKey = (threadId: string, r: { incarnation: number; requestId: string }): string => `${threadId}:${r.incarnation}:${r.requestId}`;

export function toggled(selection: ReadonlySet<string>, key: string): Set<string> {
  const next = new Set(selection);
  if (!next.delete(key)) next.add(key);
  return next;
}

/** Only keys still offered stay selected (an approval answered elsewhere leaves the batch). */
export function pruned(selection: ReadonlySet<string>, offered: readonly string[]): Set<string> {
  const keep = new Set(offered);
  return new Set([...selection].filter((k) => keep.has(k)));
}

export type AllState = "none" | "some" | "all";

export function allState(selection: ReadonlySet<string>, offered: readonly string[]): AllState {
  const n = offered.filter((k) => selection.has(k)).length;
  return n === 0 ? "none" : n === offered.length ? "all" : "some";
}

/** "Select all": everything offered, or nothing when everything already is. */
export function selectAll(selection: ReadonlySet<string>, offered: readonly string[]): Set<string> {
  return allState(selection, offered) === "all" ? new Set() : new Set(offered);
}

/** Selects every key of a group, or clears them all when they all already are; other keys stay. */
export function toggledGroup(selection: ReadonlySet<string>, keys: readonly string[]): Set<string> {
  const next = new Set(selection);
  if (keys.length && keys.every((k) => next.has(k))) for (const k of keys) next.delete(k);
  else for (const k of keys) next.add(k);
  return next;
}

/**
 * What the inbox's A, D and X act on (round 2 nit 9, round 3 blocker 2). With focus in an approval
 * card, that card, if it can still be answered, and nothing else: never another card or another
 * chat's. With no card focused, the picked chat's cards: the first for A and D (`group` false),
 * every one for X (`group` true). `focusedKey` is the focused card's key, or null when focus
 * isn't in an approval card.
 */
export function keyTargets<T extends Pick<BatchTarget, "key" | "threadId">>(targets: readonly T[], focusedKey: string | null, threadId: string | null, group: boolean): T[] {
  if (focusedKey !== null) return targets.filter((t) => t.key === focusedKey);
  if (threadId === null) return [];
  const mine = targets.filter((t) => t.threadId === threadId);
  return group ? mine : mine.slice(0, 1);
}

export type ItemOutcome = { ok: true } | { ok: false; reason: string };

/** How one request of the batch ended, from what this window recorded for it. */
export function outcomeOf(local: Local | undefined): ItemOutcome {
  if (!local) return { ok: false, reason: "not sent" };
  if (local.status !== "refused") return { ok: true };
  if (local.code === "expired") return { ok: false, reason: "expired: the turn moved on, nothing was allowed" };
  if (local.unknownOutcome) return { ok: false, reason: `${local.message}; it may have been sent, check the chat` };
  return { ok: false, reason: local.message };
}

export interface BatchFailure { key: string; chat: string; subject: string; reason: string }
export interface BatchSummary { text: string; failures: BatchFailure[] }

export function summarize(action: "allow" | "deny", results: readonly { target: BatchTarget; outcome: ItemOutcome }[]): BatchSummary {
  const done = results.filter((r) => r.outcome.ok).length;
  const failures = results.flatMap((r) => (r.outcome.ok ? [] : [{ key: r.target.key, chat: r.target.chat, subject: r.target.subject, reason: r.outcome.reason }]));
  const verb = action === "allow" ? "Allowed" : "Denied";
  const parts: string[] = [];
  if (done) parts.push(`${verb} ${done}.`);
  if (failures.length) parts.push(`${failures.length} ${failures.length === 1 ? "wasn't" : "weren't"} ${action === "allow" ? "allowed" : "denied"}:`);
  return { text: parts.join(" ") || "Nothing was selected.", failures };
}
