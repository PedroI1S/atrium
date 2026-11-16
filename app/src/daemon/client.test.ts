import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { perf, resetPerf } from "../chat/perf";
import type { Delivery, ThreadDelivery, ThreadSummary } from "./types";

const invokeMock = vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>();
let activeChannel: { onmessage?: (frame: unknown) => void } | undefined;

vi.mock("@tauri-apps/api/core", () => ({
  Channel: class MockChannel {
    onmessage?: (frame: unknown) => void;
    constructor() {
      activeChannel = this;
    }
  },
  invoke: (cmd: string, args?: Record<string, unknown>) => invokeMock(cmd, args),
}));

vi.mock("../platform", () => ({
  inTauri: true,
}));

import { attach, daemon, viewThread } from "./client";

const baseSummary: ThreadSummary = {
  thread_id: "t-old",
  title: "Initial",
  provider: "claude",
  model: "sonnet",
  location: { type: "home" },
  state: { type: "idle" },
  enforcement: null,
  incarnation: 1,
  native_session_id: "s-1",
  last_event: 40,
  unread: false,
  pending_approvals: 0,
  pending_questions: 0,
  attention_since_ms: null,
  attention_detail: null,
  created_at_ms: 1_700_000_000_000,
  updated_at_ms: 1_700_000_000_000,
};

const eventDelivery = (id: number, text: string): Delivery => ({
  type: "event",
  event: {
    id,
    thread_id: "t-old",
    envelope: {
      incarnation: 1,
      seq: id,
      at_ms: 1_700_000_000_000 + id,
      event: { type: "message_delta", message_id: "m-1", kind: "assistant", text },
    },
  },
});

const ackCalls = () =>
  invokeMock.mock.calls
    .filter(([cmd]) => cmd === "bridge_ack")
    .map(([, args]) => args?.seq);

describe("bridge client batch receive path", () => {
  beforeEach(async () => {
    vi.useFakeTimers();
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(undefined);
    resetPerf();
    activeChannel = undefined;
    await attach();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("applies a batch in order, records per-delivery timings, and acknowledges the last seq", () => {
    const seen: ThreadDelivery[] = [];
    const stop = viewThread("t-old", (d) => seen.push(d));

    activeChannel!.onmessage!({
      batch: [
        { seq: 1, delivery: { type: "connection", state: { type: "connected", daemon_version: "0.0.0", daemon_pid: 1 } } },
        { seq: 2, delivery: { type: "inbox", threads: [baseSummary], inbox: [] } },
        { seq: 3, delivery: eventDelivery(41, "one ") },
        { seq: 4, delivery: eventDelivery(42, "two") },
        { seq: 5, delivery: { type: "thread_updated", summary: { ...baseSummary, title: "Updated" } } },
      ],
    });

    expect(daemon().connection.type).toBe("connected");
    expect(daemon().loaded).toBe(true);
    expect(daemon().threads["t-old"]?.title).toBe("Updated");
    expect(seen.map((d) => (d.type === "event" ? d.event.id : -1))).toEqual([41, 42]);
    expect(perf.counts["deliver:connection"]).toBe(1);
    expect(perf.counts["deliver:inbox"]).toBe(1);
    expect(perf.counts["deliver:event"]).toBe(2);
    expect(perf.counts["deliver:thread_updated"]).toBe(1);

    expect(ackCalls()).toEqual([]);
    vi.advanceTimersByTime(16);
    expect(ackCalls()).toEqual([5]);

    stop();
  });

  it("handles a mix of batch and single frames in order", () => {
    const seen: number[] = [];
    const stop = viewThread("t-old", (d) => {
      if (d.type === "event") seen.push(d.event.id);
    });

    activeChannel!.onmessage!({ seq: 6, delivery: eventDelivery(43, "a") });
    activeChannel!.onmessage!({
      batch: [
        { seq: 7, delivery: eventDelivery(44, "b") },
        { seq: 8, delivery: eventDelivery(45, "c") },
      ],
    });
    activeChannel!.onmessage!({
      seq: 9,
      delivery: { type: "thread_updated", summary: { ...baseSummary, title: "Final" } },
    });
    activeChannel!.onmessage!({ batch: [] });

    expect(seen).toEqual([43, 44, 45]);
    expect(daemon().threads["t-old"]?.title).toBe("Final");

    vi.advanceTimersByTime(16);
    expect(ackCalls()).toEqual([9]);

    vi.advanceTimersByTime(32);
    expect(ackCalls()).toEqual([9]);

    stop();
  });

  it("continues applying and acknowledges the last seq when one item in a batch throws", () => {
    const stop = viewThread("t-old", (d) => {
      if (d.type === "event" && d.event.id === 50) throw new Error("boom");
    });

    expect(() =>
      activeChannel!.onmessage!({
        batch: [
          { seq: 10, delivery: eventDelivery(50, "bad") },
          { seq: 11, delivery: { type: "thread_updated", summary: { ...baseSummary, title: "AfterError" } } },
        ],
      }),
    ).toThrow("boom");

    expect(daemon().threads["t-old"]?.title).toBe("AfterError");
    vi.advanceTimersByTime(16);
    expect(ackCalls()).toEqual([11]);

    stop();
  });
});
