// What the bridge hands to a window. `Delivery`, `ConnectionState` and `Page` mirror
// `crates/atrium-client/src/delivery.rs`; `src-tauri/tests/delivery_shapes.rs` keeps the
// examples in `delivery-shapes.json` in step with the Rust types, and reduce.test.ts runs every
// one of them through the reducer. The protocol's own types are generated.

import type { ErrorCode, EventId, InboxItem, StoredEvent, ThreadId, ThreadSummary } from "../../../crates/atrium-protocol/bindings/protocol";

export type { AgentEvent, AgentLaunch, DaemonCommand, ErrorCode, EventId, InboxItem, InboxReason, StoredEvent, ThreadId, ThreadState, ThreadSummary } from "../../../crates/atrium-protocol/bindings/protocol";

export type ConnectionState =
  | { type: "connecting"; attempt: number }
  | { type: "connected"; daemon_version: string; daemon_pid: number }
  /** The daemon refused this app: another protocol version (contract §10), an unverified app, too many clients. */
  | { type: "incompatible"; protocol_version: number; message: string; daemon_version: string; daemon_pid: number; running_turns: number }
  | { type: "failed"; message: string }
  | { type: "closed" };

export type Delivery =
  | { type: "connection"; state: ConnectionState }
  /** Every thread and the whole inbox, replacing what the window has. */
  | { type: "inbox"; threads: ThreadSummary[]; inbox: InboxItem[] }
  | { type: "thread_updated"; summary: ThreadSummary }
  | { type: "inbox_updated"; items: InboxItem[] }
  | { type: "thread_removed"; thread_id: ThreadId }
  /** A view of a thread, replacing what the window has for it. */
  | { type: "snapshot"; thread_id: ThreadId; events: StoredEvent[]; has_older: boolean; truncated: boolean; last: EventId | null }
  | { type: "event"; event: StoredEvent }
  | { type: "thread_failed"; thread_id: ThreadId; code: ErrorCode; message: string };

/** A delivery about one thread's history. The thread views (M1-09b) consume these. */
export type ThreadDelivery = Extract<Delivery, { type: "snapshot" | "event" | "thread_failed" }>;

export interface Frame { seq: number; delivery: Delivery }

export interface Page { events: StoredEvent[]; has_older: boolean; truncated: boolean }

/** Why a command has no value (`bridge::CommandFailure`). */
export type CommandFailure =
  | { kind: "not_connected"; message: string }
  | { kind: "too_large"; message: string }
  /** The command may or may not have run: read the state again before repeating it. */
  | { kind: "unknown_outcome"; message: string }
  | { kind: "rejected"; code: ErrorCode; message: string };

export function isCommandFailure(e: unknown): e is CommandFailure {
  return typeof e === "object" && e !== null && "kind" in e && "message" in e;
}
