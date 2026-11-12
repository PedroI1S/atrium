// What a window knows from the daemon besides thread histories: the connection, the thread
// list and the inbox. A pure reducer over deliveries, tested in reduce.test.ts.

import type { ConnectionState, Delivery, InboxItem, ThreadDelivery, ThreadSummary } from "./types";

export interface DaemonState {
  connection: ConnectionState;
  /** False until the first `inbox` delivery of this connection: the list isn't known yet. */
  loaded: boolean;
  threads: Readonly<Record<string, ThreadSummary>>;
  /** Thread ids, most recently updated first. */
  order: readonly string[];
  /** Ranked by the daemon. */
  inbox: readonly InboxItem[];
}

export const initialDaemonState = (): DaemonState => ({ connection: { type: "connecting", attempt: 1 }, loaded: false, threads: {}, order: [], inbox: [] });

function ordered(threads: Readonly<Record<string, ThreadSummary>>): string[] {
  return Object.values(threads)
    .sort((a, b) => b.updated_at_ms - a.updated_at_ms || b.created_at_ms - a.created_at_ms || (a.thread_id < b.thread_id ? -1 : 1))
    .map((t) => t.thread_id);
}

/** Applies a delivery. Deliveries about a thread's history don't change this state. */
export function reduce(state: DaemonState, delivery: Delivery): DaemonState {
  switch (delivery.type) {
    case "connection":
      return { ...state, connection: delivery.state };
    case "inbox": {
      const threads = Object.fromEntries(delivery.threads.map((t) => [t.thread_id, t]));
      return { ...state, loaded: true, threads, order: ordered(threads), inbox: delivery.inbox };
    }
    case "thread_updated": {
      const before = state.threads[delivery.summary.thread_id];
      const threads = { ...state.threads, [delivery.summary.thread_id]: delivery.summary };
      const moved = !before || before.updated_at_ms !== delivery.summary.updated_at_ms;
      return { ...state, threads, order: moved ? ordered(threads) : state.order };
    }
    case "thread_removed": {
      if (!(delivery.thread_id in state.threads)) return state;
      const { [delivery.thread_id]: _gone, ...threads } = state.threads;
      return { ...state, threads, order: state.order.filter((id) => id !== delivery.thread_id) };
    }
    case "inbox_updated":
      return { ...state, inbox: delivery.items };
    case "snapshot":
    case "event":
    case "thread_failed":
      return state;
  }
}

export function isThreadDelivery(delivery: Delivery): delivery is ThreadDelivery {
  return delivery.type === "snapshot" || delivery.type === "event" || delivery.type === "thread_failed";
}

/** The thread a history delivery is about. */
export function threadOf(delivery: ThreadDelivery): string {
  return delivery.type === "event" ? delivery.event.thread_id : delivery.thread_id;
}

/** How a thread's state reads in the interface (ui-direction §6: a dot plus a label, never colour alone). */
export type Tone = "needs-you" | "failed" | "ready" | "running" | "idle";

export function toneOf(thread: ThreadSummary): { tone: Tone; label: string } {
  switch (thread.state.type) {
    case "starting": return { tone: "running", label: "Starting" };
    case "running": return { tone: "running", label: "Running" };
    case "needs_approval": return { tone: "needs-you", label: "Needs approval" };
    case "needs_input": return { tone: "needs-you", label: "Has a question" };
    case "paused": return { tone: "needs-you", label: "Paused by a limit" };
    case "failed": return { tone: "failed", label: "Failed" };
    case "interrupted": return thread.state.cleanup === "unverified" ? { tone: "failed", label: "Cleanup unverified" } : { tone: "idle", label: "Interrupted" };
    case "ended": return { tone: "idle", label: "Ended" };
    case "idle": return thread.unread ? { tone: "ready", label: "New reply" } : { tone: "idle", label: "Idle" };
  }
}

/** Threads that are starting or have a turn running. */
export const runningCount = (state: DaemonState): number =>
  Object.values(state.threads).filter((t) => t.state.type === "starting" || t.state.type === "running").length;

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** "now", "5m", "3h", "2d": the age shown on a row. */
export function ageLabel(sinceMs: number, nowMs: number): string {
  const d = Math.max(0, nowMs - sinceMs);
  if (d < MINUTE) return "now";
  if (d < HOUR) return `${Math.floor(d / MINUTE)}m`;
  if (d < DAY) return `${Math.floor(d / HOUR)}h`;
  return `${Math.floor(d / DAY)}d`;
}
