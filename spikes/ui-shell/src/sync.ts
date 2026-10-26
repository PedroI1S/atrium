// Ordering and back-pressure helpers for talking to the Rust single writer. Pure (no Tauri,
// no DOM) so the races the review found are unit-tested in sync.test.ts.

/** Rust rejects an edit to a view another window owns with this prefix (shell.rs `PatchError`). */
export const isOwnershipRejection = (e: unknown): boolean => String(e).includes("not-owner");

/**
 * Serial queue of IPC writes. A failed write never blocks later ones and `flush()` never
 * rejects: an ownership rejection (the view moved mid-edit) is dropped and counted; any other
 * failure is kept in `lastError` for display only. Nothing latches, so closing a window can
 * always proceed (review #6).
 */
export class WriteQueue {
  private tail: Promise<void> = Promise.resolve();
  lastError: unknown = null;
  dropped = 0;
  constructor(private readonly onError: (e: unknown) => void = () => {}) {}

  enqueue(task: () => Promise<void>): Promise<void> {
    const run = this.tail.then(task).then(
      () => { this.lastError = null; },
      (e: unknown) => {
        if (isOwnershipRejection(e)) this.dropped++;
        else { this.lastError = e; this.onError(e); }
      },
    );
    this.tail = run;
    return run;
  }

  flush(): Promise<void> {
    return this.tail;
  }
}

/**
 * Runs `run` with the latest value only: while a run is in flight, further calls replace the
 * queued value instead of piling up (review #7: a slider drag queued one native paint per event).
 * The returned promise settles when the latest value has been run.
 */
export function latestWins<T>(run: (value: T) => Promise<void>): (value: T) => Promise<void> {
  let queued: { value: T } | null = null;
  let draining: Promise<void> | null = null;
  const drain = async () => {
    while (queued) {
      const { value } = queued;
      queued = null;
      try { await run(value); } catch { /* run reports its own errors */ }
    }
    draining = null;
  };
  return (value: T) => {
    queued = { value };
    draining ??= drain();
    return draining;
  };
}

/**
 * Coalesces partial patches: one send in flight at a time, and everything that arrives
 * meanwhile is merged into a single follow-up send. `idle` fires when nothing is pending.
 */
export function coalescing<P extends object>(send: (patch: P) => Promise<void>, idle: () => void = () => {}): (patch: P) => Promise<void> {
  let pending: P | null = null;
  let pumping: Promise<void> | null = null;
  const pump = async () => {
    while (pending) {
      const patch = pending;
      pending = null;
      try { await send(patch); } catch { /* send reports its own errors */ }
    }
    pumping = null;
    idle();
  };
  return (patch: P) => {
    pending = { ...(pending ?? {}), ...patch } as P;
    pumping ??= pump();
    return pumping;
  };
}

/** Trailing debounce with an explicit `flush` (scroll position before a pop-out or close). */
export function debounced<A>(fn: (arg: A) => void, ms: number) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let last: { arg: A } | null = null;
  const flush = () => {
    clearTimeout(timer);
    timer = undefined;
    if (!last) return;
    const { arg } = last;
    last = null;
    fn(arg);
  };
  const call = (arg: A) => {
    last = { arg };
    clearTimeout(timer);
    timer = setTimeout(flush, ms);
  };
  return Object.assign(call, { flush, pending: () => last !== null });
}

/**
 * Closing a pop-out: flush its edits, then give the view back. A failed flush or a failed
 * bring-back can never leave the window unclosable: the last resort destroys it, and Rust
 * returns the view on destruction anyway.
 */
export async function closeOwnedView(steps: { flush: () => Promise<void>; bringBack: () => Promise<unknown>; destroy: () => Promise<unknown> }): Promise<"returned" | "destroyed"> {
  try {
    await steps.flush();
  } catch {
    /* edits that failed to send are already reported; closing must still work */
  }
  try {
    await steps.bringBack();
    return "returned";
  } catch {
    await steps.destroy();
    return "destroyed";
  }
}

/**
 * Main window asks a pop-out to return its view. If the pop-out does not hand it back within
 * `timeoutMs` (hung or busy), main forces it (review #8).
 */
export async function bringBackFromMain(steps: { ask: () => Promise<unknown>; returned: () => boolean; force: () => Promise<unknown>; wait: (ms: number) => Promise<void> }, timeoutMs: number, pollMs: number): Promise<"returned" | "forced"> {
  try { await steps.ask(); } catch { /* the pop-out may already be gone */ }
  for (let waited = 0; waited < timeoutMs; waited += pollMs) {
    if (steps.returned()) return "returned";
    await steps.wait(pollMs);
  }
  if (steps.returned()) return "returned";
  await steps.force();
  return "forced";
}
