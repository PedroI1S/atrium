// Where the window's main thread spends its time while streaming: counts and total
// milliseconds per named piece of work. Two additions and a clock read per call; the streaming
// run (stream-bench.ts) reports them, so a regression says where it is.

export const perf: { counts: Record<string, number>; ms: Record<string, number> } = { counts: {}, ms: {} };

export function count(name: string, by = 1): void {
  perf.counts[name] = (perf.counts[name] ?? 0) + by;
}

export function timed<T>(name: string, work: () => T): T {
  const start = performance.now();
  try {
    return work();
  } finally {
    perf.ms[name] = (perf.ms[name] ?? 0) + (performance.now() - start);
    perf.counts[name] = (perf.counts[name] ?? 0) + 1;
  }
}

export function resetPerf(): void {
  perf.counts = {};
  perf.ms = {};
}
