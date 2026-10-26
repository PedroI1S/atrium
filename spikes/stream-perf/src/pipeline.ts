// The frame pipeline: batches arrive from the bridge at any time; once per animation frame they are
// applied to the model and published to Solid in one `batch`; in the following frame, once the
// result has been committed, the webview acknowledges what it has rendered so the backend may call
// `ClientHandle::applied`.

import { batch, createSignal } from "solid-js";
import { Hist, epochNow } from "./metrics";
import { Transcript } from "./model";
import { ack } from "./platform";
import { type Batch, type Encoding, decodeBatch } from "./wire";

export interface PipelineConfig {
  encoding: Encoding;
  /** Minimum gap between acknowledgements (ms). */
  ackIntervalMs: number;
}

export class Pipeline {
  threads: Transcript[] = [];
  threadIds: string[] = [];
  private idIndex = new Map<string, number>();
  private queue: Batch[] = [];
  private pendingRaw: { raw: unknown; at: number }[] = [];
  private ready = false;
  private raf = 0;
  /** What the previous frame applied and is waiting to settle. */
  private applied: { start: number; ats: number[]; seq: number; ids: number[] } | null = null;
  /** Per-thread highest id from frames that have been committed (safe to acknowledge). */
  private settledIds: number[] = [];
  private lastAckAt = 0;
  private ackTimer = 0;
  private ackedIds: number[] = [];
  private ackSeq = -1;

  readonly visible = createSignal(0);
  /** Bumped inside the same `batch` as the row updates; views use it to follow the tail. */
  readonly tick = createSignal(0);

  // Measurements.
  readonly latency = new Hist(); // agent timestamp -> committed frame, visible-thread deltas
  readonly ipc = new Hist(); // Rust encode time -> JS handler, per event
  readonly queueWait = new Hist(); // JS handler -> start of the applying frame, per event
  readonly commit = new Hist(); // start of the applying frame -> start of the next one, per frame
  applyMs: number[] = []; // synchronous time of each applying frame (JS + Solid DOM updates)
  decodeMs = 0;
  decodeBatches = 0;
  batchesReceived = 0;
  eventsReceived = 0;
  eventsApplied = 0;
  duplicatesDropped = 0;
  framesWithWork = 0;
  maxQueue = 0;
  lastSeq = -1;
  seqGaps = 0;

  constructor(readonly cfg: PipelineConfig) {}

  setThreads(ids: string[]): void {
    this.threadIds = ids;
    ids.forEach((id, i) => this.idIndex.set(id, i));
    this.threads = ids.map(() => new Transcript());
    this.ackedIds = ids.map(() => 0);
    this.ready = true;
    for (const p of this.pendingRaw.splice(0)) this.receive(p.raw, p.at);
  }

  /** Called by the Tauri channel for every batch. */
  onMessage = (raw: unknown): void => {
    const at = performance.now();
    if (!this.ready) {
      this.pendingRaw.push({ raw, at });
      return;
    }
    this.receive(raw, at);
  };

  private receive(raw: unknown, t0: number): void {
    const recvAt = performance.timeOrigin + t0;
    const b = decodeBatch(this.cfg.encoding, raw, (id) => this.idIndex.get(id) ?? 0);
    this.decodeMs += performance.now() - t0;
    this.decodeBatches++;
    if (this.lastSeq >= 0 && b.seq !== this.lastSeq + 1) this.seqGaps++;
    this.lastSeq = b.seq;
    this.batchesReceived++;
    this.eventsReceived += b.events.length;
    this.ipc.add(recvAt - b.sent, b.events.length);
    (b as Batch & { recvAt: number }).recvAt = recvAt;
    this.queue.push(b);
    if (this.queue.length > this.maxQueue) this.maxQueue = this.queue.length;
    if (!this.raf) this.raf = requestAnimationFrame(this.frame);
  }

  private frame = (): void => {
    this.raf = 0;
    const start = performance.now();
    const startEpoch = performance.timeOrigin + start;

    // 1. Settle what the previous frame applied: it has been committed by now.
    if (this.applied) {
      const a = this.applied;
      this.applied = null;
      for (let i = 0; i < a.ats.length; i++) this.latency.add(startEpoch - a.ats[i]!);
      this.commit.add(start - a.start);
      this.settledIds = a.ids;
      this.maybeAck(a.seq);
    }

    // 2. Apply everything that has arrived.
    const batches = this.queue;
    if (batches.length > 0) {
      this.queue = [];
      const visible = this.visible[0]();
      const ats: number[] = [];
      let seq = -1;
      batch(() => {
        for (const b of batches) {
          this.queueWait.add(startEpoch - (b as Batch & { recvAt: number }).recvAt, b.events.length);
          seq = b.seq;
          for (const ev of b.events) {
            const t = this.threads[ev.thread];
            if (!t) continue;
            if (!t.apply(ev)) {
              this.duplicatesDropped++;
              continue;
            }
            this.eventsApplied++;
            if (ev.delta && ev.thread === visible) ats.push(ev.at);
          }
        }
        for (const t of this.threads) t.flush();
        this.tick[1]((n) => n + 1);
      });
      this.applyMs.push(performance.now() - start);
      this.framesWithWork++;
      this.applied = { start, ats, seq, ids: this.threads.map((t) => t.lastId) };
    }

    if (this.applied || this.queue.length > 0) this.raf = requestAnimationFrame(this.frame);
  };

  private maybeAck(seq: number): void {
    this.ackSeq = Math.max(this.ackSeq, seq);
    const now = performance.now();
    if (now - this.lastAckAt >= this.cfg.ackIntervalMs) {
      this.sendAck();
    } else if (!this.ackTimer) {
      this.ackTimer = window.setTimeout(() => this.sendAck(), this.cfg.ackIntervalMs - (now - this.lastAckAt));
    }
  }

  /** Reports the highest event id each thread has applied and had committed to a frame. */
  sendAck = (): void => {
    if (this.ackTimer) {
      clearTimeout(this.ackTimer);
      this.ackTimer = 0;
    }
    this.lastAckAt = performance.now();
    const cursors: [number, number][] = [];
    this.settledIds.forEach((id, i) => {
      if (id > this.ackedIds[i]!) {
        cursors.push([i, id]);
        this.ackedIds[i] = id;
      }
    });
    if (cursors.length > 0 || this.ackSeq >= 0) void ack(this.ackSeq, cursors);
  };

  /** True once nothing is queued or waiting to settle. */
  idle(): boolean {
    return this.queue.length === 0 && this.applied === null && this.pendingRaw.length === 0;
  }
}

export { epochNow };
