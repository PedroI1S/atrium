import { describe, expect, it } from "vitest";
import type { InboxItem, InboxReason } from "../daemon/types";
import { filterInbox, groupInbox, itemKey, moveSelection, nextJump, rankInbox } from "./inbox";
import { askOf, rememberChoices, subjectOf } from "./requests";
import type { ApprovalItem } from "./transcript";

const item = (thread_id: string | null, reason: InboxReason, since_ms: number): InboxItem => ({ thread_id, reason, since_ms, title: thread_id ?? "", detail: null });

describe("inbox order (contract §8)", () => {
  const items = [
    item("reply-old", "reply", 1),
    item("question", "question", 50),
    item("approval-new", "approval", 90),
    item("failed", "failed", 10),
    item("cleanup", "cleanup_unverified", 5),
    item(null, "storage_full", 100),
    item("approval-old", "approval", 20),
    item("reply-new", "reply", 70),
  ];

  it("ranks by reason, then oldest first", () => {
    expect(rankInbox(items).map(itemKey)).toEqual(["@storage_full", "approval-old", "approval-new", "question", "failed", "cleanup", "reply-old", "reply-new"]);
  });

  it("is stable for equal times and doesn't change its input", () => {
    const copy = [...items];
    rankInbox([item("b", "approval", 1), item("a", "approval", 1)]);
    expect(rankInbox([item("b", "approval", 1), item("a", "approval", 1)]).map(itemKey)).toEqual(["a", "b"]);
    expect(items).toEqual(copy);
  });

  it("groups as approvals and questions, failed, ready for you", () => {
    expect(groupInbox(items).map((g) => [g.group, g.items.length])).toEqual([["needs-you", 4], ["failed", 2], ["replies", 2]]);
  });

  it("filters", () => {
    expect(filterInbox(items, "asks").map(itemKey).sort()).toEqual(["@storage_full", "approval-new", "approval-old", "question"]);
    expect(filterInbox(items, "replies").map(itemKey).sort()).toEqual(["reply-new", "reply-old"]);
    expect(filterInbox(items, "all")).toHaveLength(items.length);
  });
});

describe("⌘J order", () => {
  const items = [item("r", "reply", 1), item("a2", "approval", 30), item("q", "question", 5), item("a1", "approval", 10)];

  it("from a thread that isn't in the inbox, goes to the first item", () => {
    expect(nextJump(items, null)).toEqual({ kind: "thread", threadId: "a1" });
    expect(nextJump(items, "elsewhere")).toEqual({ kind: "thread", threadId: "a1" });
  });

  it("from an item, goes to the next one in inbox order, wrapping at the end", () => {
    expect(nextJump(items, "a1")).toEqual({ kind: "thread", threadId: "a2" });
    expect(nextJump(items, "a2")).toEqual({ kind: "thread", threadId: "q" });
    expect(nextJump(items, "q")).toEqual({ kind: "thread", threadId: "r" });
    expect(nextJump(items, "r")).toEqual({ kind: "thread", threadId: "a1" });
  });

  it("storage full opens the inbox; nothing to do says so", () => {
    expect(nextJump([item(null, "storage_full", 1), ...items], null)).toEqual({ kind: "inbox" });
    expect(nextJump([], "x")).toEqual({ kind: "none" });
    expect(nextJump([item("only", "approval", 1)], "only")).toEqual({ kind: "none" });
  });

  it("J and K move the selection and stop at the ends", () => {
    const keys = ["a", "b", "c"];
    expect(moveSelection(keys, null, 1)).toBe("a");
    expect(moveSelection(keys, null, -1)).toBe("c");
    expect(moveSelection(keys, "a", 1)).toBe("b");
    expect(moveSelection(keys, "c", 1)).toBe("c");
    expect(moveSelection(keys, "a", -1)).toBe("a");
    expect(moveSelection([], "a", 1)).toBeNull();
  });
});

describe("what an approval card says (D-054, D-062)", () => {
  const approval = (over: Partial<ApprovalItem>): ApprovalItem => ({
    kind: "approval", key: "k", first: 1, last: 1, incarnation: 1, v: 0, bytes: 0, requestId: "r", turn: 1, outcome: null, stale: false, at: 0,
    callId: null, name: "Bash", toolKind: "execute", title: "rm", input: { command: "rm -rf build" }, reason: null, rememberable: true, ...over,
  });

  it("shows the exact command or paths", () => {
    expect(subjectOf(approval({}))).toEqual({ label: "Command", text: "rm -rf build" });
    expect(subjectOf(approval({ toolKind: "edit", name: "Write", input: { file_path: "/Users/me/a.md", content: "x" } }))).toEqual({ label: "Path", text: "/Users/me/a.md" });
    expect(subjectOf(approval({ toolKind: "other", name: "Weird", title: null, input: { a: 1 } })).text).toContain('"a": 1');
    expect(askOf(approval({}))).toBe("Run this command?");
  });

  it("offers always… only when rememberable, with the three scopes, in any folder", () => {
    expect(rememberChoices(approval({ rememberable: false }))).toEqual([]);
    const choices = rememberChoices(approval({}));
    expect(choices.map((c) => c.scope)).toEqual(["thread", "home", "global"]);
    for (const c of choices) expect(c.hint).toContain("in any folder");
    expect(choices[0]!.hint).toContain("this exact command");
  });
});
