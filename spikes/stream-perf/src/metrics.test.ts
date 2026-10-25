import { describe, expect, it } from "vitest";
import { Hist, summarizeFrames, summarizeValues } from "./metrics";

describe("Hist", () => {
  it("computes weighted percentiles on 1 ms buckets", () => {
    const h = new Hist(100);
    h.add(5, 90);
    h.add(50, 9);
    h.add(99, 1);
    expect(h.n).toBe(100);
    expect(h.pct(0.5)).toBe(5);
    expect(h.pct(0.95)).toBe(50);
    expect(h.pct(1)).toBe(99);
    expect(h.summary().max).toBe(99);
  });
  it("clamps values past the last bucket but keeps the true max", () => {
    const h = new Hist(10);
    h.add(5000);
    expect(h.pct(0.5)).toBe(10);
    expect(h.max).toBe(5000);
  });
});

describe("summarizeFrames", () => {
  it("counts skipped display updates, not slow frames", () => {
    const dts = [16.7, 16.7, 50, 16.7, 33.4, 16.7];
    const s = summarizeFrames(dts, 16.67);
    expect(s.slowFrames).toBe(2);
    expect(s.droppedFrames).toBe(2 + 1);
    expect(s.over25ms).toBe(2);
    expect(s.worst[0]).toBe(50);
  });
  it("percentiles of plain values", () => {
    const s = summarizeValues([1, 2, 3, 4, 100]);
    expect(s.p50).toBe(3);
    expect(s.max).toBe(100);
  });
});
