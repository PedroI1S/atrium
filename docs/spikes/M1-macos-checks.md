# M1 macOS checks: startup, idle memory, streaming, font preload (2026-11-16)

Run by the orchestrator on Pedro's MacBook Air at Pedro's request ("run the macos benchmarks while it runs"). They ran on `dev` at `a688f5c` (M1-13, M1-15 and M1-16 merged), with the `perf` profile, under `/usr/bin/lockf -k /tmp/atrium-gui-measure.lock`.

**The machine was not quiet.** A Gemini 4 Hydrogen fix round (M1-18) was working in another worktree the whole time. The 1-minute load average went from 2.3 at the start to 3.6 after the startup bench, 4.8 after the streaming bench, and 12–14 in the last A/B round. Read every number as an upper bound on a busy machine.

The raw JSON is in the orchestrator's scratchpad (`bench/startup.json`, `bench/ab-*.json`) and is not kept in the repository.

## Startup (`startup_bench.py`, 30 warm launches) — pass

| Phase (ms) | median | p90 | min | max |
|---|---|---|---|---|
| main → first JS | 226.1 | 233.0 | 214.6 | 255.4 |
| first JS → mounted | 65.0 | 66.0 | 63.0 | 67.0 |
| **TTI (main → first paint and input)** | **330.9** | **341.8** | 318.4 | 353.4 |
| spawn → TTI | 338.9 | 351.4 | 325.7 | 411.4 |

Gate: median < 400 ms and p90 < 450 ms. Both pass (`ttiPass: true`). The 2026-11-12 run was median 340 / p90 346.

## Idle memory (one window, 9 launches) — pass

Per-launch median `phys_footprint` is 84.6–91.5 MiB, and the worst sample is 92.9 MiB. No sample went over 150 MB, the gate. The 143.7 MiB outlier from 2026-11-12 didn't come back.

## Streaming (`stream_bench.py --runs 3`; 10 threads, 1000 tokens/s each, 30 s)

| Run | tokens/s | fps | frame p50 / p95 / p99 / max (ms) | long frames | updates dropped |
|---|---|---|---|---|---|
| 1 | 9994 | 60 | 17 / 18 / 19 / 19 | 0 | 0 |
| 2 | 9995 | 60 | 17 / 18 / 19 / 19 | 0 | 0 |
| 3 | 9993 | 59.9 | 17 / 18 / 19 / 61 | 2 (0.1 %) | 4 |

- **About 60 % of frames read "over 16.7 ms" during streaming.** The same window sitting idle before each run reads the same (61–63 %), so this comes from the 1 ms timestamp resolution at 60 Hz, not from streaming. The share over 16.7 ms by more than the 1 ms resolution is 19–21 % while streaming and 12–16 % idle.
- **Long frames (more than 1.5 refresh intervals):** two in run 3, with 31 ms busy on the main thread. The load average was rising at the time. Runs 1 and 2 had none.

## Font preload A/B (D-041): no measurable gain

Interleaved runs: 3 rounds × 10 warm launches each, of the binaries `atrium-head` (no preload) and `atrium-preload`, built from the same tree on 2026-11-15.

| Round (load) | head TTI median / p90 | preload TTI median / p90 |
|---|---|---|
| 1 (4.7–5.4) | 336.5 / 339.7 | 342.0 / 349.8 |
| 2 (5.4–6.3) | 341.1 / 350.7 | 341.0 / 350.3 |
| 3 (13–14) | 355.6 / 447.4 | 334.3 / 343.9 |

- **Rounds 1 and 2 are within ±6 ms of each other.**
- **Round 3's gap comes from the load, not the preload.** Head was measured while the load average jumped to 14, and preload ran after the peak.
- **Conclusion:** on macOS the font preload makes no difference to TTI that this machine can measure. A quiet re-run would only firm this up. It's Pedro's decision whether to keep the preload (open item D-041 / font preload).
