import { afterEach, describe, expect, it, vi } from "vitest";
import { EditGate, WriteQueue, bringBackFromMain, coalescing, debounced, latestWins, makeCloser, prepareMove } from "./sync";

const tick = () => new Promise((r) => setTimeout(r, 0));
function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((r) => { resolve = r; });
  return { promise, resolve };
}

describe("WriteQueue (review #6: a rejected patch latched and made the pop-out unclosable)", () => {
  it("drops ownership rejections without latching an error", async () => {
    const q = new WriteQueue();
    await q.enqueue(() => Promise.reject("not-owner: the view is owned by another window"));
    expect(q.lastError).toBeNull();
    expect(q.dropped).toBe(1);
    await expect(q.flush()).resolves.toBeUndefined();
  });
  it("keeps other errors for display, still runs later writes, flush never rejects", async () => {
    const seen: unknown[] = [];
    const q = new WriteQueue((e) => seen.push(e));
    const order: number[] = [];
    void q.enqueue(async () => { throw new Error("disk full"); });
    void q.enqueue(async () => { order.push(2); });
    await expect(q.flush()).resolves.toBeUndefined();
    expect(order).toEqual([2]);
    expect(seen).toHaveLength(1);
    expect(q.lastError).toBeNull(); // cleared by the next successful write
  });
  it("runs writes in order", async () => {
    const q = new WriteQueue();
    const order: number[] = [];
    const slow = deferred();
    void q.enqueue(async () => { await slow.promise; order.push(1); });
    void q.enqueue(async () => { order.push(2); });
    slow.resolve();
    await q.flush();
    expect(order).toEqual([1, 2]);
  });
});

describe("makeCloser", () => {
  it("brings the view back even when the drain fails", async () => {
    const bringBack = vi.fn(async () => {});
    const destroy = vi.fn(async () => {});
    expect(await makeCloser({ prepare: () => Promise.reject(new Error("x")), bringBack, destroy })()).toBe("returned");
    expect(bringBack).toHaveBeenCalledOnce();
    expect(destroy).not.toHaveBeenCalled();
  });
  it("destroys the window when bring-back is refused", async () => {
    const destroy = vi.fn(async () => {});
    expect(await makeCloser({ prepare: async () => {}, bringBack: () => Promise.reject(new Error("refused")), destroy })()).toBe("destroyed");
    expect(destroy).toHaveBeenCalledOnce();
  });
});

/** A miniature of windows.ts: user edits pass the gate, then go through the write queue. */
function viewModel() {
  const gate = new EditGate<"chat">();
  const queue = new WriteQueue();
  const sent: unknown[] = [];
  const stalls = new Map<unknown, Promise<void>>();
  const edit = (patch: unknown, buffered = false): boolean => {
    if (!gate.allows("chat", buffered)) return false;
    void queue.enqueue(async () => {
      await stalls.get(patch);
      sent.push(patch);
    });
    return true;
  };
  return { gate, queue, sent, stalls, edit };
}

describe("closing with a stalled write (review 2 #1)", () => {
  it("freezes edits synchronously, drains every accepted edit before bring-back, and closes once", async () => {
    const m = viewModel();
    const stall = deferred();
    const a = { draft: "a" };
    m.stalls.set(a, stall.promise);
    expect(m.edit(a)).toBe(true); // in flight, stalled
    expect(m.edit({ draft: "ab" })).toBe(true); // accepted, queued behind the stalled write
    const bringBack = vi.fn(async () => { m.sent.push("bring_back"); });
    const close = makeCloser({ prepare: () => prepareMove(m.gate, "chat", () => {}, m.queue), bringBack, destroy: async () => {} });
    const first = close();
    expect(m.edit({ draft: "abc" })).toBe(false); // typed after close started: refused at once
    expect(close()).toBe(first); // idempotent
    await tick();
    expect(bringBack).not.toHaveBeenCalled(); // still draining the stalled write
    stall.resolve();
    expect(await first).toBe("returned");
    expect(m.sent).toEqual([a, { draft: "ab" }, "bring_back"]);
    expect(bringBack).toHaveBeenCalledOnce();
  });
  it("drain also waits for writes enqueued while it waits", async () => {
    const q = new WriteQueue();
    const order: string[] = [];
    void q.enqueue(async () => { await tick(); order.push("a"); void q.enqueue(async () => { await tick(); order.push("b"); }); });
    await q.drain();
    expect(order).toEqual(["a", "b"]);
  });
});

describe("scroll then pop-out (review 2 #2)", () => {
  it("the pending scroll position lands before the move; new user edits do not", async () => {
    const m = viewModel();
    const scroll = debounced((y: number) => { m.edit({ scroll: y }, true); }, 150);
    scroll(100);
    scroll(425); // the user pops out within 150 ms of scrolling
    const moved = prepareMove(m.gate, "chat", () => scroll.flush(), m.queue).then(() => { m.sent.push("pop_out"); });
    expect(m.edit({ draft: "late" })).toBe(false);
    await moved;
    expect(m.sent).toEqual([{ scroll: 425 }, "pop_out"]);
    expect(scroll.pending()).toBe(false);
  });
});

describe("latestWins (review #7: one queued native paint per slider event)", () => {
  it("runs the first value and then only the latest", async () => {
    const ran: number[] = [];
    const gate = deferred();
    const paint = latestWins<number>(async (v) => { ran.push(v); if (v === 0) await gate.promise; });
    const done = [0, 1, 2, 3, 4, 5].map((v) => paint(v));
    gate.resolve();
    await Promise.all(done);
    expect(ran).toEqual([0, 5]);
  });
  it("keeps working after a run throws", async () => {
    const ran: number[] = [];
    const paint = latestWins<number>(async (v) => { ran.push(v); if (v === 1) throw new Error("boom"); });
    await paint(1);
    await paint(2);
    expect(ran).toEqual([1, 2]);
  });
});

describe("coalescing (settings patches)", () => {
  it("sends one patch in flight and merges the rest into one follow-up", async () => {
    const sent: object[] = [];
    const gate = deferred();
    let idle = 0;
    const send = coalescing<{ alpha?: number; blur?: boolean }>(async (p) => { sent.push(p); if (sent.length === 1) await gate.promise; }, () => idle++);
    const all = [send({ alpha: 0.9 }), ...Array.from({ length: 50 }, (_, i) => send({ alpha: 0.5 + i / 1000 })), send({ blur: false })];
    gate.resolve();
    await Promise.all(all);
    expect(sent).toEqual([{ alpha: 0.9 }, { alpha: 0.549, blur: false }]);
    expect(idle).toBe(1);
  });
});

describe("debounced (scroll position)", () => {
  afterEach(() => vi.useRealTimers());
  it("calls once with the last value, and flush delivers it early", () => {
    vi.useFakeTimers();
    const fn = vi.fn();
    const d = debounced(fn, 150);
    for (let i = 0; i < 60; i++) d(i);
    expect(fn).not.toHaveBeenCalled();
    vi.advanceTimersByTime(150);
    expect(fn).toHaveBeenCalledOnce();
    expect(fn).toHaveBeenLastCalledWith(59);
    d(100);
    expect(d.pending()).toBe(true);
    d.flush();
    expect(fn).toHaveBeenLastCalledWith(100);
    vi.advanceTimersByTime(500);
    expect(fn).toHaveBeenCalledTimes(2);
  });
});

describe("bringBackFromMain (review #8: main could not recover a view from a hung pop-out)", () => {
  it("returns when the pop-out hands the view back", async () => {
    let owner = "view-chat";
    const force = vi.fn(async () => {});
    const result = await bringBackFromMain({ ask: async () => { setTimeout(() => { owner = "main"; }, 0); }, returned: () => owner === "main", force, wait: async () => { await tick(); } }, 1000, 10);
    expect(result).toBe("returned");
    expect(force).not.toHaveBeenCalled();
  });
  it("forces after the timeout when the pop-out never answers", async () => {
    const force = vi.fn(async () => {});
    let waited = 0;
    const result = await bringBackFromMain({ ask: () => Promise.reject(new Error("gone")), returned: () => false, force, wait: async (ms) => { waited += ms; } }, 1500, 50);
    expect(result).toBe("forced");
    expect(force).toHaveBeenCalledOnce();
    expect(waited).toBe(1500);
  });
});
