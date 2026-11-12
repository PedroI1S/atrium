import { describe, expect, it } from "vitest";
import shapes from "./delivery-shapes.json";
import { ageLabel, initialDaemonState, isThreadDelivery, reduce, runningCount, threadOf, toneOf, type DaemonState } from "./reduce";
import type { Delivery, ThreadSummary } from "./types";

// The same examples `src-tauri/tests/delivery_shapes.rs` checks against the Rust types.
const deliveries = shapes as unknown as Delivery[];
const of = <T extends Delivery["type"]>(type: T) => deliveries.filter((d): d is Extract<Delivery, { type: T }> => d.type === type);
const run = (state: DaemonState, ...ds: Delivery[]) => ds.reduce(reduce, state);
const inbox = of("inbox")[0]!;
const summary = (over: Partial<ThreadSummary>): ThreadSummary => ({ ...inbox.threads[0]!, ...over });

describe("the delivery reducer", () => {
  it("handles every kind of delivery the client sends", () => {
    const kinds = new Set(deliveries.map((d) => d.type));
    expect([...kinds].sort()).toEqual(["connection", "event", "inbox", "inbox_updated", "snapshot", "thread_failed", "thread_removed", "thread_updated"]);
    let state = initialDaemonState();
    for (const d of deliveries) state = reduce(state, d);
    expect(state.connection.type).toBe("closed");
  });

  it("starts not connected, with the list unknown", () => {
    const state = initialDaemonState();
    expect(state.connection).toEqual({ type: "connecting", attempt: 1 });
    expect(state.loaded).toBe(false);
  });

  it("connection replaces the connection state and keeps the list (for the return)", () => {
    const loaded = run(initialDaemonState(), inbox);
    for (const d of of("connection")) {
      const next = reduce(loaded, d);
      expect(next.connection).toEqual(d.state);
      expect(next.order).toEqual(loaded.order);
    }
  });

  it("inbox replaces every thread and the whole inbox, newest thread first", () => {
    const stale = run(initialDaemonState(), { type: "thread_updated", summary: summary({ thread_id: "stale" }) });
    const state = reduce(stale, inbox);
    expect(state.loaded).toBe(true);
    expect(state.order).toEqual(["t-new", "t-old"]);
    expect(Object.keys(state.threads).sort()).toEqual(["t-new", "t-old"]);
    expect(state.inbox.map((i) => i.reason)).toEqual(["approval", "reply"]);
  });

  it("thread_updated replaces one summary, adds an unknown thread, and reorders when it moved", () => {
    const [updated, added] = of("thread_updated");
    const state = run(initialDaemonState(), inbox, updated!);
    expect(state.threads["t-old"]!.state).toEqual({ type: "interrupted", cleanup: "unverified" });
    expect(state.order).toEqual(["t-old", "t-new"]);
    const more = reduce(state, added!);
    expect(more.order).toEqual(["t-third", "t-old", "t-new"]);
  });

  it("keeps the order's identity when a thread changed without moving", () => {
    const state = run(initialDaemonState(), inbox);
    const next = reduce(state, { type: "thread_updated", summary: summary({ title: "Renamed" }) });
    expect(next.order).toBe(state.order);
    expect(next.threads["t-old"]!.title).toBe("Renamed");
  });

  it("thread_removed drops the thread; an unknown one changes nothing", () => {
    const state = run(initialDaemonState(), inbox);
    const next = reduce(state, of("thread_removed")[0]!);
    expect(next.order).toEqual(["t-old"]);
    expect(next.threads["t-new"]).toBeUndefined();
    expect(reduce(next, of("thread_removed")[0]!)).toBe(next);
  });

  it("inbox_updated replaces the inbox", () => {
    const state = run(initialDaemonState(), inbox, of("inbox_updated")[0]!);
    expect(state.inbox).toHaveLength(1);
    expect(state.inbox[0]!.thread_id).toBeNull();
  });

  it("history deliveries leave this state alone and name their thread", () => {
    const state = run(initialDaemonState(), inbox);
    for (const d of [...of("snapshot"), ...of("event"), ...of("thread_failed")]) {
      expect(reduce(state, d)).toBe(state);
      expect(isThreadDelivery(d)).toBe(true);
    }
    expect(threadOf(of("snapshot")[0]!)).toBe("t-old");
    expect(threadOf(of("event")[0]!)).toBe("t-old");
    expect(threadOf(of("thread_failed")[0]!)).toBe("t-gone");
    expect(isThreadDelivery(inbox)).toBe(false);
  });
});

describe("how a thread reads", () => {
  it("every state has a tone and a label", () => {
    const states: ThreadSummary["state"][] = [
      { type: "starting" }, { type: "running" }, { type: "needs_approval" }, { type: "needs_input" }, { type: "idle" },
      { type: "paused", resets_at_ms: null }, { type: "failed", message: "x" }, { type: "interrupted", cleanup: "verified" },
      { type: "interrupted", cleanup: "unverified" }, { type: "ended" },
    ];
    expect(states.map((state) => toneOf(summary({ state, unread: false })).tone)).toEqual(["running", "running", "needs-you", "needs-you", "idle", "needs-you", "failed", "idle", "failed", "idle"]);
    for (const state of states) expect(toneOf(summary({ state })).label).not.toBe("");
  });
  it("an unverified cleanup is never shown as simply stopped (contract §10)", () => {
    expect(toneOf(summary({ state: { type: "interrupted", cleanup: "unverified" } })).label).toBe("Cleanup unverified");
  });
  it("an idle thread with a new reply says so", () => {
    expect(toneOf(summary({ state: { type: "idle" }, unread: true }))).toEqual({ tone: "ready", label: "New reply" });
  });
  it("counts starting and running threads as running", () => {
    const state = run(initialDaemonState(), { type: "inbox", inbox: [], threads: [summary({ thread_id: "a", state: { type: "running" } }), summary({ thread_id: "b", state: { type: "starting" } }), summary({ thread_id: "c", state: { type: "idle" } })] });
    expect(runningCount(state)).toBe(2);
  });
});

describe("ages", () => {
  it("reads as now, minutes, hours and days", () => {
    const t = 1_700_000_000_000;
    expect(ageLabel(t, t + 59_000)).toBe("now");
    expect(ageLabel(t, t + 5 * 60_000)).toBe("5m");
    expect(ageLabel(t, t + 3 * 3_600_000)).toBe("3h");
    expect(ageLabel(t, t + 49 * 3_600_000)).toBe("2d");
    expect(ageLabel(t, t - 1000)).toBe("now");
  });
});
