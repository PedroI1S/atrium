// This window's end of the bridge (src-tauri/src/bridge.rs): deliveries arrive in order on a
// channel, are applied, then acknowledged. Commands go through `command`, never through the
// delivery path (contract §11).

import { Channel, invoke } from "@tauri-apps/api/core";
import { createSignal } from "solid-js";
import { inTauri } from "../platform";
import { initialDaemonState, isThreadDelivery, reduce, threadOf } from "./reduce";
import type { DaemonCommand, Delivery, Frame, Page, ThreadDelivery } from "./types";

export const [daemon, setDaemon] = createSignal(initialDaemonState());

type ThreadListener = (delivery: ThreadDelivery) => void;
const listeners = new Map<string, Set<ThreadListener>>();

/**
 * Acknowledgements are sent at most this often, with the latest number. A timer, not an
 * animation frame: frames stop in a hidden window, and a window that stops acknowledging is
 * cut off by the client after 16 MiB.
 */
const ACK_INTERVAL_MS = 16;
let applied = 0;
let acknowledged = 0;
let ackTimer: ReturnType<typeof setTimeout> | undefined;

function scheduleAck(): void {
  if (ackTimer !== undefined) return;
  ackTimer = setTimeout(() => {
    ackTimer = undefined;
    if (applied === acknowledged) return;
    acknowledged = applied;
    void invoke("bridge_ack", { seq: acknowledged });
  }, ACK_INTERVAL_MS);
}

/** Applies one delivery. Exported for tests of the routing. */
export function apply(delivery: Delivery): void {
  if (isThreadDelivery(delivery)) {
    for (const listener of listeners.get(threadOf(delivery)) ?? []) listener(delivery);
  } else {
    setDaemon((state) => reduce(state, delivery));
  }
}

/** Starts receiving. Called once, before the first render; a reloaded webview calls it again. */
export async function attach(): Promise<void> {
  if (!inTauri) return;
  const channel = new Channel<Frame>();
  channel.onmessage = (frame) => {
    try {
      apply(frame.delivery);
    } finally {
      // A delivery that failed to apply is still acknowledged: holding it back would stall
      // the window, and the next snapshot replaces the state anyway.
      applied = frame.seq;
      scheduleAck();
    }
  };
  applied = 0;
  acknowledged = 0;
  await invoke("bridge_attach", { channel });
  await invoke("bridge_show_inbox", { show: true });
}

/**
 * Views a thread: `listener` gets its snapshot, then its events. The thread is viewed in the
 * client while at least one listener is registered. Returns the function that stops it.
 */
export function viewThread(threadId: string, listener: ThreadListener): () => void {
  let set = listeners.get(threadId);
  if (!set) {
    set = new Set();
    listeners.set(threadId, set);
  }
  set.add(listener);
  if (inTauri) {
    // Viewing again asks for a fresh snapshot, which every listener of the thread receives.
    void invoke("bridge_view", { threadId });
  }
  return () => {
    const current = listeners.get(threadId);
    if (!current?.delete(listener)) return;
    if (current.size === 0) {
      listeners.delete(threadId);
      if (inTauri) void invoke("bridge_unview", { threadId });
    }
  };
}

/** Older history of a viewed thread, before the event `before`. Null when it isn't viewed. */
export function pageBefore(threadId: string, before: number, maxBytes: number): Promise<Page | null> {
  return inTauri ? invoke<Page | null>("bridge_page_before", { threadId, before, maxBytes }) : Promise.resolve(null);
}

/**
 * Sends a command and resolves with the daemon's value. Rejects with a `CommandFailure`
 * (`isCommandFailure`): a command is never sent twice, and "unknown_outcome" means it may have run.
 */
export function command<T = unknown>(cmd: DaemonCommand): Promise<T> {
  if (!inTauri) return Promise.reject({ kind: "not_connected", message: "not running in the app" });
  return invoke<T>("daemon_command", { command: cmd });
}

/** Tries to connect again now (after `incompatible` or `failed`, which are not retried). */
export function reconnect(): void {
  if (inTauri) void invoke("bridge_reconnect");
}

/** Asks the older daemon that refused this app to stop: when idle, or now (`force`). */
export function replaceDaemon(force: boolean): Promise<void> {
  return inTauri ? invoke("replace_daemon", { force }) : Promise.resolve();
}
