import { describe, expect, it } from "vitest";
import type { AgentEvent, StoredEvent } from "../daemon/types";
import recorded from "./fixtures/approvals.json";
import { SOFT_BLOCK_CHARS } from "./blocks";
import { type ApprovalItem, type MessageItem, type QuestionItem, type ToolItem, Transcript } from "./transcript";
import { cardState } from "./requests";

let nextId = 1;
const seqs = new Map<number, number>();
function ev(event: AgentEvent, incarnation = 1, id = nextId++): StoredEvent {
  const seq = seqs.get(incarnation) ?? 0;
  seqs.set(incarnation, seq + 1);
  return { id, thread_id: "t1", envelope: { incarnation, seq, at_ms: 1_000 + id, event } };
}
function reset() {
  nextId = 1;
  seqs.clear();
}
const delta = (message_id: string, text: string, kind: "assistant" | "user" | "reasoning" = "assistant"): AgentEvent => ({ type: "message_delta", message_id, kind, text });
const done = (message_id: string, kind: "assistant" | "user" = "assistant"): AgentEvent => ({ type: "message_completed", message_id, kind, body: { type: "streamed", chars: 0, deltas: 0, sha256: "" } });
const textOf = (m: MessageItem) => m.splitter.blocks.map((b) => b.text).join("|");
const messages = (t: Transcript) => t.items.filter((i): i is MessageItem => i.kind === "message");

describe("messages", () => {
  it("deltas render as they arrive, and folded deltas give the same transcript", () => {
    reset();
    const live = new Transcript();
    live.apply([ev({ type: "turn_started", turn: 1 }), ev(delta("a1", "Hello "))]);
    expect(textOf(messages(live)[0]!)).toBe("Hello ");
    live.apply([ev(delta("a1", "world\n\nSecond")), ev(delta("a1", " para")), ev(done("a1"))]);
    expect(textOf(messages(live)[0]!)).toBe("Hello world\n|Second para");
    expect(messages(live)[0]!.open).toBe(false);

    reset();
    const folded = new Transcript();
    folded.load([ev({ type: "turn_started", turn: 1 }), ev(delta("a1", "Hello world\n\nSecond para")), ev(done("a1"))], false, false);
    expect(textOf(messages(folded)[0]!)).toBe(textOf(messages(live)[0]!));
    expect(folded.rows().map((r) => r.type)).toEqual(live.rows().map((r) => r.type));
  });

  it("an inline user message is one row; the next delta of an assistant message grows its open block", () => {
    reset();
    const t = new Transcript();
    t.apply([ev({ type: "message_completed", message_id: "u1", kind: "user", body: { type: "inline", text: "hi <b>there</b>" } }), ev(delta("a1", "par"))]);
    const rows = t.rows();
    expect(rows.map((r) => r.type)).toEqual(["user", "block"]);
    const block = rows[1]!.type === "block" ? rows[1]!.block : null;
    const v = block!.v;
    t.apply([ev(delta("a1", "tial"))]);
    expect(t.rows()).toBe(rows);
    expect(block!.text).toBe("partial");
    expect(block!.v).toBeGreaterThan(v);
  });

  it("a replayed event (at or before the cursor) changes nothing", () => {
    reset();
    const t = new Transcript();
    const e = ev(delta("a1", "once"));
    t.apply([e]);
    expect(t.apply([e])).toBe(false);
    expect(textOf(messages(t)[0]!)).toBe("once");
  });

  it("a long paragraph is cut into rows instead of one growing row", () => {
    reset();
    const t = new Transcript();
    const word = "synthetic tokens ";
    for (let i = 0; i < 2000; i++) t.apply([ev(delta("a1", word))]);
    const blocks = messages(t)[0]!.splitter.blocks;
    expect(blocks.length).toBeGreaterThan(5);
    expect(blocks.every((b) => b.text.length <= SOFT_BLOCK_CHARS + word.length)).toBe(true);
    expect(blocks.slice(1).every((b) => b.cont)).toBe(true);
    expect(blocks.map((b) => b.text).join(" ").replace(/\s+/g, " ").trim()).toBe(word.repeat(2000).trim());
  });

  it("a long code block continues as a second code block", () => {
    reset();
    const t = new Transcript();
    t.apply([ev(delta("a1", "```js\n"))]);
    for (let i = 0; i < 450; i++) t.apply([ev(delta("a1", `line ${i}\n`))]);
    const blocks = messages(t)[0]!.splitter.blocks;
    expect(blocks.length).toBe(3);
    expect(blocks.every((b) => b.text.startsWith("```js\n"))).toBe(true);
    expect(blocks[0]!.closed && blocks[1]!.closed && !blocks[2]!.closed).toBe(true);
  });
});

describe("tool calls", () => {
  it("output deltas append, output replaces, and a folded update does both in order", () => {
    reset();
    const t = new Transcript();
    t.apply([ev({ type: "tool_call_started", call_id: "c1", name: "Bash", kind: "execute", title: "ls", input: { command: "ls" } })]);
    const tool = t.items[0] as ToolItem;
    const upd = (o: { output_delta?: string; output?: string; title?: string }): AgentEvent => ({ type: "tool_call_updated", call_id: "c1", title: o.title ?? null, kind: null, input: null, output_delta: o.output_delta ?? null, output: o.output ?? null });
    t.apply([ev(upd({ output_delta: "a\n" })), ev(upd({ output_delta: "b\n" }))]);
    expect(tool.output).toBe("a\nb\n");
    t.apply([ev(upd({ output: "replaced\n" }))]);
    expect(tool.output).toBe("replaced\n");
    t.apply([ev(upd({ output: "whole\n", output_delta: "more\n", title: "ls -la" }))]);
    expect(tool.output).toBe("whole\nmore\n");
    expect(tool.title).toBe("ls -la");
    expect(tool.status).toBe("running");
    t.apply([ev({ type: "tool_call_finished", call_id: "c1", status: "succeeded", output: null, error: null, truncated_bytes: 4096 })]);
    expect(tool.status).toBe("succeeded");
    expect(tool.output).toBe("whole\nmore\n");
    expect(tool.truncatedBytes).toBe(4096);
    expect(t.bytes).toBe(tool.bytes);
  });

  it("finished output replaces what streamed", () => {
    reset();
    const t = new Transcript();
    t.apply([
      ev({ type: "tool_call_started", call_id: "c1", name: "Read", kind: "read", title: null, input: {} }),
      ev({ type: "tool_call_updated", call_id: "c1", title: null, kind: null, input: null, output_delta: "partial", output: null }),
      ev({ type: "tool_call_finished", call_id: "c1", status: "failed", output: "final", error: "no such file" }),
    ]);
    const tool = t.items[0] as ToolItem;
    expect([tool.output, tool.status, tool.error]).toEqual(["final", "failed", "no such file"]);
  });
});

describe("incarnations", () => {
  it("a new incarnation cuts open messages, interrupts running calls and makes requests stale", () => {
    reset();
    const t = new Transcript();
    t.apply([
      ev({ type: "turn_started", turn: 1 }),
      ev(delta("a1", "half a sent")),
      ev({ type: "tool_call_started", call_id: "c1", name: "Bash", kind: "execute", title: "make", input: {} }),
      ev({ type: "approval_requested", request_id: "r1", call_id: "c1", name: "Bash", kind: "execute", title: "make", input: { command: "make" }, reason: null, rememberable: true }),
    ]);
    expect(t.pending()).toHaveLength(1);
    // The daemon restarted: incarnation 2 reuses ids, which must not touch the old items.
    t.apply([ev({ type: "turn_started", turn: 1 }, 2), ev(delta("a1", "fresh"), 2)]);
    const [old, fresh] = messages(t);
    expect(old!.cut).toBe(true);
    expect(old!.open).toBe(false);
    expect(textOf(old!)).toBe("half a sent");
    expect(textOf(fresh!)).toBe("fresh");
    expect((t.items.find((i) => i.kind === "tool") as ToolItem).status).toBe("interrupted");
    const req = t.items.find((i) => i.kind === "approval") as ApprovalItem;
    expect(req.stale).toBe(true);
    expect(t.pending()).toHaveLength(0);
    expect(t.incarnation).toBe(2);
  });

  it("a turn's end makes its open requests stale; a later turn's are untouched", () => {
    reset();
    const t = new Transcript();
    t.apply([
      ev({ type: "turn_started", turn: 1 }),
      ev({ type: "question_asked", request_id: "q1", questions: [{ id: "a", prompt: "?", options: [], allow_text: true }] }),
      ev({ type: "turn_completed", turn: 1, outcome: "completed", usage: null }),
      ev({ type: "turn_started", turn: 2 }),
      ev({ type: "question_asked", request_id: "q2", questions: [{ id: "a", prompt: "?", options: [], allow_text: true }] }),
    ]);
    const [q1, q2] = t.items.filter((i): i is QuestionItem => i.kind === "question");
    expect(q1!.stale).toBe(true);
    expect(q2!.stale).toBe(false);
    expect(t.pending().map((r) => r.requestId)).toEqual(["q2"]);
  });
});

describe("the loaded-bytes bound and paging (contract §11)", () => {
  const MAX = 1000;
  function fill(t: Transcript, from: number, count: number, size: number) {
    for (let i = from; i < from + count; i++) t.apply([ev(delta(`m${i}`, "x".repeat(size))), ev(done(`m${i}`))]);
  }

  it("keeps at most the bound, dropping the oldest closed items", () => {
    reset();
    const t = new Transcript(MAX);
    fill(t, 0, 30, 100);
    expect(t.bytes).toBeLessThanOrEqual(MAX);
    expect(t.hasOlder).toBe(true);
    expect(messages(t).map((m) => m.messageId)[0]).toBe("m20");
    expect(t.bytes).toBe(t.items.reduce((sum, i) => sum + i.bytes, 0));
  });

  it("never drops an item that is still open", () => {
    reset();
    const t = new Transcript(MAX);
    t.apply([ev(delta("open", "y".repeat(50)))]);
    fill(t, 0, 30, 100);
    expect(messages(t)[0]!.messageId).toBe("open");
    // Over the bound, but the open message holds the cut: nothing before it can go either.
    expect(t.bytes).toBeGreaterThan(MAX);
    // Once it closed, the next item makes a cut point after it.
    t.apply([ev(done("open"))]);
    fill(t, 30, 1, 100);
    expect(t.bytes).toBeLessThanOrEqual(MAX);
    expect(messages(t).map((m) => m.messageId)).toEqual(["m30"]);
  });

  it("cuts only where the dropped events all come before what is kept", () => {
    reset();
    const t = new Transcript(MAX);
    // m0 opens, a call starts inside it, m0 completes after the call started: no cut between them.
    t.apply([ev(delta("m0", "a".repeat(600))), ev({ type: "tool_call_started", call_id: "c", name: "Bash", kind: "execute", title: null, input: null }), ev(done("m0")), ev({ type: "tool_call_finished", call_id: "c", status: "succeeded", output: "o".repeat(300), error: null })]);
    fill(t, 1, 3, 200);
    const first = t.items[0]!;
    // m0 went together with the call, or neither did.
    expect(first.kind === "message" ? (first as MessageItem).messageId !== "m0" || t.items[1]!.kind === "tool" : true).toBe(true);
    for (let k = 1; k < t.items.length; k++) expect(t.items[k]!.first).toBeGreaterThan(-1);
  });

  it("pages older history back in at the top, exactly where it was cut", () => {
    reset();
    const all: StoredEvent[] = [];
    const t = new Transcript(MAX);
    for (let i = 0; i < 30; i++) {
      const evs = [ev(delta(`m${i}`, "x".repeat(100))), ev(done(`m${i}`))];
      all.push(...evs);
      t.apply(evs);
    }
    const first = t.firstLoaded!;
    const page = all.filter((e) => e.id < first).slice(-6);
    const added = t.prepend(page, true, false);
    expect(added).toBe(3);
    expect(messages(t).map((m) => m.messageId).slice(0, 4)).toEqual(["m17", "m18", "m19", "m20"]);
    // Paging back past the bound drops the newest items and detaches from the live tail.
    expect(t.bytes).toBeLessThanOrEqual(MAX);
    expect(t.detached).toBe(true);
    // While detached, live events are counted, not shown; requests are still tracked.
    const before = t.items.length;
    t.apply([ev(delta("m99", "new")), ev({ type: "approval_requested", request_id: "r9", call_id: null, name: "Bash", kind: "execute", title: "x", input: { command: "x" }, reason: null, rememberable: false })]);
    expect(t.items.length).toBe(before);
    expect(t.missed).toBe(2);
    expect(t.pending().map((r) => r.requestId)).toEqual(["r9"]);
    // A fresh snapshot reattaches.
    t.load(all.slice(-4), true, false);
    expect(t.detached).toBe(false);
    expect(messages(t).map((m) => m.messageId)).toEqual(["m28", "m29"]);
  });

  it("a snapshot is bounded too", () => {
    reset();
    const t = new Transcript(MAX);
    const evs: StoredEvent[] = [];
    for (let i = 0; i < 20; i++) evs.push(ev(delta(`m${i}`, "z".repeat(100))), ev(done(`m${i}`)));
    t.load(evs, false, true);
    expect(t.bytes).toBeLessThanOrEqual(MAX);
    expect(t.hasOlder).toBe(true);
    expect(t.truncated).toBe(true);
  });
});

describe("recorded approvals and questions (the fake agent can't raise them)", () => {
  type Msg = { type: string; event?: StoredEvent };
  const events = (recorded as unknown as Msg[]).filter((m) => m.type === "event").map((m) => m.event!);
  const upTo = (id: number) => {
    const t = new Transcript();
    t.apply(events.filter((e) => e.id <= id));
    return t;
  };
  const req = (t: Transcript, id: string, inc = 1) => t.items.find((i) => (i.kind === "approval" || i.kind === "question") && i.requestId === id && i.incarnation === inc) as ApprovalItem | QuestionItem;

  it("a fresh approval is pending, and resolved here once allowed", () => {
    const t = upTo(6);
    const r1 = req(t, "r1");
    expect(cardState(r1, undefined)).toEqual({ kind: "fresh" });
    expect(cardState(r1, { status: "sending", action: "allow" })).toEqual({ kind: "sending", action: "allow" });
    expect(cardState(r1, { status: "accepted", action: "allow" })).toEqual({ kind: "sent", action: "allow" });
    t.apply(events.filter((e) => e.id === 7));
    expect(cardState(r1, { status: "accepted", action: "allow" })).toMatchObject({ kind: "resolved", here: true, text: "Allowed" });
  });

  it("an approval whose turn ended can't be allowed: stale, then refused by the daemon too", () => {
    const t = upTo(10);
    const r2 = req(t, "r2");
    expect(cardState(r2, undefined).kind).toBe("fresh");
    t.apply(events.filter((e) => e.id > 10 && e.id <= 13));
    expect(cardState(r2, undefined)).toMatchObject({ kind: "stale" });
    // A click that raced the turn's end: the ledger answers `Expired` (contract §5).
    const fresh = upTo(10);
    expect(cardState(req(fresh, "r2"), { status: "refused", action: "allow", code: "expired", message: "expired", unknownOutcome: false })).toMatchObject({ kind: "stale" });
  });

  it("a question answered in another window says so", () => {
    const t = upTo(15);
    const q1 = req(t, "q1");
    expect(cardState(q1, undefined).kind).toBe("fresh");
    expect((q1 as QuestionItem).questions[0]!.options).toHaveLength(2);
    t.apply(events.filter((e) => e.id === 16));
    expect(cardState(q1, undefined)).toMatchObject({ kind: "resolved", here: false, text: "Answered in another window" });
  });

  it("a new incarnation makes the old question stale and scopes reused request ids", () => {
    const t = upTo(20);
    expect(cardState(req(t, "q2"), undefined).kind).toBe("stale");
    const newer = req(t, "r1", 2) as ApprovalItem;
    expect(newer.toolKind).toBe("edit");
    expect(cardState(newer, undefined).kind).toBe("fresh");
    expect(req(t, "r1", 1).outcome).toBe("approved");
    expect(t.pending().map((r) => `${r.incarnation}:${r.requestId}`)).toEqual(["2:r1"]);
  });

  it("a refusal for another reason can be retried; an unknown outcome says it may have been sent", () => {
    const r1 = req(upTo(6), "r1");
    expect(cardState(r1, { status: "refused", action: "allow", code: "internal", message: "the store is unavailable", unknownOutcome: false })).toEqual({ kind: "failed", text: "the store is unavailable" });
    expect(cardState(r1, { status: "refused", action: "allow", code: null, message: "the connection dropped", unknownOutcome: true }).kind).toBe("failed");
  });
});
