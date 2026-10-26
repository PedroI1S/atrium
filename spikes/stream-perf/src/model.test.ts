import { describe, expect, it } from "vitest";
import { Transcript } from "./model";
import type { Ev } from "./wire";

let id = 0;
const delta = (text: string, message = "1"): Ev => ({ delta: true, id: ++id, thread: 0, at: 0, message, mkind: 0, text });
const other = (event: Record<string, unknown>): Ev => ({ delta: false, id: ++id, thread: 0, at: 0, event: event as never });

describe("Transcript", () => {
  it("streams one message into closed rows plus one open row", () => {
    const t = new Transcript();
    for (const c of ["Hel", "lo\n", "\nsecond ", "para"]) t.apply(delta(c));
    t.flush();
    expect(t.rows.map((r) => (r.kind === "md" ? [r.text, r.open] : null))).toEqual([
      ["Hello\n", false],
      ["second para", true],
    ]);
    expect(t.count()).toBe(2);
  });

  it("closing the open row on completion keeps the same text", () => {
    const t = new Transcript();
    t.apply(delta("one\n\ntwo"));
    t.flush();
    t.apply(other({ type: "message_completed", message_id: "1", kind: "assistant", text: "one\n\ntwo" }));
    t.flush();
    expect(t.rows.length).toBe(2);
    expect(t.rows.every((r) => r.kind !== "md" || !r.open)).toBe(true);
    expect(t.completedMismatch).toBe(0);
  });

  it("counts a completed message whose length differs from the deltas", () => {
    const t = new Transcript();
    t.apply(delta("abc"));
    t.apply(other({ type: "message_completed", message_id: "1", kind: "assistant", text: "abcdef" }));
    expect(t.completedMismatch).toBe(1);
  });

  it("a tool call ends the open text and gets its own row", () => {
    const t = new Transcript();
    t.apply(delta("before"));
    t.apply(other({ type: "tool_call_started", call_id: "c", name: "read", kind: "read", title: "a.rs", input: {} }));
    t.apply(delta(" after"));
    t.apply(other({ type: "tool_call_finished", call_id: "c", status: "succeeded", output: "ok", error: null }));
    t.flush();
    expect(t.rows.map((r) => r.kind)).toEqual(["md", "tool", "md"]);
    const tool = t.rows[1]!;
    expect(tool.kind === "tool" && tool.status).toBe("succeeded");
  });

  it("tracks the highest applied id and the end of the session", () => {
    const t = new Transcript();
    t.apply(delta("x"));
    const last = id;
    t.apply(other({ type: "session_ended", exit_code: 0, reason: null }));
    expect(t.lastId).toBe(last + 1);
    expect(t.ended).toBe(true);
  });
});

describe("replay after a reconnect", () => {
  const feed = (t: Transcript, from: number, to: number) => {
    for (let i = from; i <= to; i++) t.apply({ delta: true, id: i, thread: 0, at: 0, message: "1", mkind: 0, text: `w${i} ` });
  };
  const textOf = (t: Transcript) => t.rows.map((r) => (r.kind === "md" ? r.text : "")).join("|");

  it("ignores events replayed before their delayed acknowledgement", () => {
    const once = new Transcript();
    feed(once, 1, 12);
    once.flush();

    const t = new Transcript();
    feed(t, 1, 10); // applied, acknowledgement delayed
    // reconnect: the client resubscribes from the last acknowledged id (5) and replays 6..=10, then new ones
    const results: boolean[] = [];
    for (let i = 6; i <= 12; i++) results.push(t.apply({ delta: true, id: i, thread: 0, at: 0, message: "1", mkind: 0, text: `w${i} ` }));
    t.flush();
    expect(results).toEqual([false, false, false, false, false, true, true]);
    expect(t.duplicates).toBe(5);
    expect(t.chars).toBe(once.chars);
    expect(t.events).toBe(12);
    expect(textOf(t)).toBe(textOf(once));
  });

  it("does not duplicate lifecycle rows", () => {
    const t = new Transcript();
    t.apply({ delta: false, id: 1, thread: 0, at: 0, event: { type: "turn_started", turn: 1 } });
    t.apply({ delta: false, id: 2, thread: 0, at: 0, event: { type: "tool_call_started", call_id: "c", name: "read", kind: "read", title: null, input: {} } });
    t.apply({ delta: false, id: 1, thread: 0, at: 0, event: { type: "turn_started", turn: 1 } });
    t.apply({ delta: false, id: 2, thread: 0, at: 0, event: { type: "tool_call_started", call_id: "c", name: "read", kind: "read", title: null, input: {} } });
    t.flush();
    expect(t.rows.map((r) => r.kind)).toEqual(["note", "tool"]);
  });

  it("a replay after everything was acknowledged changes nothing", () => {
    const t = new Transcript();
    feed(t, 1, 10);
    feed(t, 11, 12); // the client resumes at 11
    t.flush();
    expect(t.duplicates).toBe(0);
    expect(t.events).toBe(12);
  });
});
