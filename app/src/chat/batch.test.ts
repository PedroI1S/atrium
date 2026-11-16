import { describe, expect, it } from "vitest";
import { type BatchTarget, allState, batchKey, outcomeOf, pruned, selectAll, summarize, toggled } from "./batch";
import type { ApprovalItem } from "./transcript";

const target = (key: string, chat = "Chat"): BatchTarget => ({ key, threadId: key, request: {} as ApprovalItem, chat, subject: `cmd ${key}` });

describe("batch selection", () => {
  it("keys a request by thread, incarnation and request id", () => {
    expect(batchKey("t1", { incarnation: 2, requestId: "r9" })).toBe("t1:2:r9");
  });
  it("toggles one key without touching the others", () => {
    const a = toggled(new Set(), "a");
    expect([...a]).toEqual(["a"]);
    expect([...toggled(new Set(["a", "b"]), "a")]).toEqual(["b"]);
  });
  it("select all takes everything offered, and clears when all are selected", () => {
    const offered = ["a", "b", "c"];
    expect(allState(new Set(), offered)).toBe("none");
    expect(allState(new Set(["b"]), offered)).toBe("some");
    expect([...selectAll(new Set(["b"]), offered)].sort()).toEqual(offered);
    expect(allState(new Set(offered), offered)).toBe("all");
    expect(selectAll(new Set(offered), offered).size).toBe(0);
    expect(allState(new Set(), [])).toBe("none");
  });
  it("drops approvals that are no longer offered (answered elsewhere, expired)", () => {
    expect([...pruned(new Set(["a", "gone", "c"]), ["a", "b", "c"])]).toEqual(["a", "c"]);
  });
});

describe("batch outcomes", () => {
  it("counts accepted and in-flight requests as done", () => {
    expect(outcomeOf({ status: "accepted", action: "allow" })).toEqual({ ok: true });
    expect(outcomeOf({ status: "sending", action: "deny" })).toEqual({ ok: true });
  });
  it("names why one failed: expired, unknown outcome, or the daemon's message", () => {
    expect(outcomeOf({ status: "refused", action: "allow", code: "expired", message: "x", unknownOutcome: false })).toEqual({ ok: false, reason: "expired: the turn moved on, nothing was allowed" });
    expect(outcomeOf({ status: "refused", action: "allow", code: null, message: "connection lost", unknownOutcome: true })).toEqual({ ok: false, reason: "connection lost; it may have been sent, check the chat" });
    expect(outcomeOf({ status: "refused", action: "deny", code: "not_running", message: "no session", unknownOutcome: false })).toEqual({ ok: false, reason: "no session" });
    expect(outcomeOf(undefined)).toEqual({ ok: false, reason: "not sent" });
  });
  it("summarizes what went through and lists each failure", () => {
    const s = summarize("allow", [
      { target: target("a"), outcome: { ok: true } },
      { target: target("b", "Build"), outcome: { ok: false, reason: "expired" } },
      { target: target("c"), outcome: { ok: true } },
    ]);
    expect(s.text).toBe("Allowed 2. 1 wasn't allowed:");
    expect(s.failures).toEqual([{ key: "b", chat: "Build", subject: "cmd b", reason: "expired" }]);
    expect(summarize("deny", [{ target: target("a"), outcome: { ok: true } }]).text).toBe("Denied 1.");
    expect(summarize("deny", [{ target: target("a"), outcome: { ok: false, reason: "x" } }, { target: target("b"), outcome: { ok: false, reason: "y" } }]).text).toBe("2 weren't denied:");
    expect(summarize("allow", []).text).toBe("Nothing was selected.");
  });
});
