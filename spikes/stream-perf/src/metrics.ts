// Measurement primitives. Nothing here allocates per event: latencies go into fixed histograms.

/** Wall clock in ms with sub-millisecond resolution, comparable with Rust's SystemTime and the agent's `at_ms`. */
export const epochNow = (): number => performance.timeOrigin + performance.now();

/** 1 ms buckets. WebKit rounds `performance.now()` to 1 ms, so finer buckets would be fiction. */
export class Hist {
  private b: Uint32Array;
  n = 0;
  max = 0;
  sum = 0;
  constructor(readonly size = 20000) {
    this.b = new Uint32Array(size + 1);
  }
  add(ms: number, weight = 1): void {
    const v = ms < 0 ? 0 : ms;
    this.b[Math.min(this.size, Math.floor(v))]! += weight;
    this.n += weight;
    this.sum += v * weight;
    if (v > this.max) this.max = v;
  }
  pct(p: number): number {
    if (this.n === 0) return 0;
    const want = Math.max(1, Math.ceil(this.n * p));
    let seen = 0;
    for (let i = 0; i <= this.size; i++) {
      seen += this.b[i]!;
      if (seen >= want) return i;
    }
    return this.size;
  }
  summary(): LatencySummary {
    return { n: this.n, mean: this.n ? this.sum / this.n : 0, p50: this.pct(0.5), p95: this.pct(0.95), p99: this.pct(0.99), max: this.max };
  }
}

export interface LatencySummary {
  n: number;
  mean: number;
  p50: number;
  p95: number;
  p99: number;
  max: number;
}

export function pctOf(sorted: number[], p: number): number {
  return sorted[Math.min(sorted.length - 1, Math.floor((sorted.length - 1) * p))] ?? 0;
}

export function summarizeValues(xs: number[]): LatencySummary {
  const s = [...xs].sort((a, b) => a - b);
  return { n: xs.length, mean: xs.reduce((a, b) => a + b, 0) / (xs.length || 1), p50: pctOf(s, 0.5), p95: pctOf(s, 0.95), p99: pctOf(s, 0.99), max: s[s.length - 1] ?? 0 };
}

export interface FrameSummary extends LatencySummary {
  /** Frames sampled. */
  frames: number;
  seconds: number;
  fps: number;
  /** The display interval the drop counts are relative to (median of the idle baseline, ms). */
  refreshMs: number;
  /** Frames whose interval exceeded 1.5x the refresh interval. */
  slowFrames: number;
  /** Sum over frames of max(0, round(interval / refresh) - 1): the display updates that were skipped. */
  droppedFrames: number;
  /** Frames longer than 25 ms (M0-06's threshold). */
  over25ms: number;
  worst: number[];
}

export function summarizeFrames(dts: number[], refreshMs: number): FrameSummary {
  const base = summarizeValues(dts);
  const total = dts.reduce((a, b) => a + b, 0);
  let slow = 0;
  let dropped = 0;
  for (const dt of dts) {
    if (dt > refreshMs * 1.5) {
      slow++;
      dropped += Math.max(0, Math.round(dt / refreshMs) - 1);
    }
  }
  return {
    ...base,
    frames: dts.length,
    seconds: total / 1000,
    fps: dts.length / (total / 1000 || 1),
    refreshMs,
    slowFrames: slow,
    droppedFrames: dropped,
    over25ms: dts.filter((d) => d > 25).length,
    worst: [...dts].sort((a, b) => b - a).slice(0, 8),
  };
}

/**
 * Frame sampler: requestAnimationFrame intervals, plus the main-thread time from the start of each
 * rAF callback to the next task (style, layout, paint recording, ResizeObserver callbacks).
 */
export class FrameSampler {
  dts: number[] = [];
  work: number[] = [];
  private last = 0;
  private running = false;
  private channel = new MessageChannel();
  private rafStart = 0;
  /** Subscribers run at the start of every sampled frame (used by the UI's fps readout). */
  constructor() {
    this.channel.port1.onmessage = () => {
      this.work.push(performance.now() - this.rafStart);
    };
  }
  start(): void {
    this.running = true;
    this.last = 0;
    requestAnimationFrame(this.tick);
  }
  stop(): void {
    this.running = false;
  }
  private tick = (ts: number): void => {
    if (!this.running) return;
    this.rafStart = performance.now();
    if (this.last !== 0) this.dts.push(ts - this.last);
    this.last = ts;
    this.channel.port2.postMessage(0);
    requestAnimationFrame(this.tick);
  };
}
