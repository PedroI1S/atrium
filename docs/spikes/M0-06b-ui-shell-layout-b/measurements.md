> **Superseded** by fix round 1: see section 5 of `../M0-06b-ui-shell-layout-b.md` and `fix1-logs/`. Kept for the record.

# M0-06b measurements (orchestrator run, 2026-10-25)

Release build, M4 / macOS 27.0.1, Calm Dark, settled machine; raw logs in `raw-measurements/`.

| Config | TTI (median / worst) | Footprint (1 sample pair) | Frames |
|---|---|---|---|
| off, 1 window | 419 / 1038 ms (10/10 over 400; first launch 1038) | 128–138 MB (RSS 205–237) | 60 fps scroll/fling/resize, worst 18 ms (repeat 1); repeat 2 had a 5.2 s stall, then timed out |
| background, 1 window | 405 / 486 ms (5 launches, 3 over 400) | 129–135 MB (RSS 195–236) | **timed out** (no report) |
| whole, 1 window | 396 / 410 ms (5 launches, 2 over 400) | 129–131 MB (RSS 194–236) | **timed out** |
| background, 3 windows | **timed out** | 190–194 MB (RSS 282–336) | **timed out** |
| whole, 3 windows | **timed out** | 191–193 MB (RSS 286–336) | **timed out** |
| off, 3 windows | **timed out** | 183–194 MB (RSS 337) | **timed out** |

Findings:
- **TTI** sits right at the 400 ms budget (median 396–419 ms) — ~30–60 ms slower than the M0-06 shell (345–392 ms).
- **Footprint**: one window ~130 MB (< 150 MB ✅); three windows ~190 MB (one process, each extra WebviewWindow ≈ +30 MB). RSS remains over budget as in M0-06.
- **Every 3-window cold/frames run and every translucent frames run timed out** waiting for the measurement report (`TimeoutError` on the report file). Reproducible across configurations → a readiness/report bug in the harness or the app (pop-out readiness, or frame reporting under translucency), not noise. Must be fixed before the opacity/pop-out performance can be judged.
