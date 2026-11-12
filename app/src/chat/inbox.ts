// The inbox's order, groups and filters (contract §8, ui-direction §3 Inbox) and where ⌘J goes.
// Pure; tested in inbox.test.ts. The daemon ranks the inbox already; the view sorts again with
// the same rule so a list that arrives out of order (or is merged from updates) still reads
// right, and so the rule is written down where the view uses it.

import type { InboxItem, InboxReason } from "../daemon/types";

/** Contract §8: storage full > approval > question > failed > unverified cleanup > unread reply. */
export const REASON_RANK: Record<InboxReason, number> = { storage_full: 0, approval: 1, question: 2, failed: 3, cleanup_unverified: 4, reply: 5 };

/** Ranked by reason, then oldest first (`since_ms`), then by thread id for a stable order. */
export function rankInbox(items: readonly InboxItem[]): InboxItem[] {
  return [...items].sort((a, b) => REASON_RANK[a.reason] - REASON_RANK[b.reason] || a.since_ms - b.since_ms || (a.thread_id ?? "").localeCompare(b.thread_id ?? ""));
}

export type InboxFilter = "all" | "asks" | "replies";
export type InboxGroup = "needs-you" | "failed" | "replies";

export const GROUP_OF: Record<InboxReason, InboxGroup> = { storage_full: "needs-you", approval: "needs-you", question: "needs-you", failed: "failed", cleanup_unverified: "failed", reply: "replies" };
export const GROUP_LABEL: Record<InboxGroup, string> = { "needs-you": "Approvals and questions", failed: "Failed", replies: "Ready for you" };

export function filterInbox(items: readonly InboxItem[], filter: InboxFilter): InboxItem[] {
  if (filter === "all") return [...items];
  if (filter === "asks") return items.filter((i) => i.reason === "approval" || i.reason === "question" || i.reason === "storage_full");
  return items.filter((i) => i.reason === "reply");
}

/** The ranked items in their groups, groups in rank order, empty groups left out. */
export function groupInbox(items: readonly InboxItem[]): { group: InboxGroup; items: InboxItem[] }[] {
  const out: { group: InboxGroup; items: InboxItem[] }[] = [];
  for (const item of rankInbox(items)) {
    const group = GROUP_OF[item.reason];
    const last = out[out.length - 1];
    if (last?.group === group) last.items.push(item);
    else out.push({ group, items: [item] });
  }
  return out;
}

/** A stable identity for an inbox item (a thread has at most one item, storage full has none). */
export const itemKey = (item: InboxItem): string => item.thread_id ?? `@${item.reason}`;

export type Jump = { kind: "thread"; threadId: string } | { kind: "inbox" } | { kind: "none" };

/**
 * ⌘J: the next item that needs the user, in inbox order, after the one being looked at. From a
 * thread that isn't in the inbox it is the first item; from the last it wraps to the first. An
 * item with no thread (storage full) opens the inbox itself.
 */
export function nextJump(items: readonly InboxItem[], current: string | null): Jump {
  const ranked = rankInbox(items);
  if (!ranked.length) return { kind: "none" };
  const at = current === null ? -1 : ranked.findIndex((i) => i.thread_id === current);
  // Skip the thread being looked at (a thread appears once, but be safe about repeats).
  for (let step = 1; step <= ranked.length; step++) {
    const item = ranked[(at + step + ranked.length) % ranked.length]!;
    if (item.thread_id === null) return { kind: "inbox" };
    if (item.thread_id !== current) return { kind: "thread", threadId: item.thread_id };
  }
  return { kind: "none" };
}

/** J and K in the inbox: the next or previous item, clamped. */
export function moveSelection(keys: readonly string[], current: string | null, by: 1 | -1): string | null {
  if (!keys.length) return null;
  const at = current === null ? -1 : keys.indexOf(current);
  if (at < 0) return by > 0 ? keys[0]! : keys[keys.length - 1]!;
  return keys[Math.min(keys.length - 1, Math.max(0, at + by))]!;
}
