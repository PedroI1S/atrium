# Handoff: M0-07-stream-perf

Model: Sonnet 5.5 · Status: done

Report: [docs/spikes/M0-07-stream-perf.md](../../spikes/M0-07-stream-perf.md) · Code: `spikes/stream-perf/` · Raw runs: `spikes/stream-perf/measurements/` · Branch `agent/sonnet/M0-07-stream-perf`

## Done
- Tauri 2 + SolidJS spike: M0-05 `ClientHandle`/`EventStream` (by path, unmodified) -> bridge task (batch, encode, `tauri::ipc::Channel`, ack-driven flow control and `applied`) -> webview (frame-batched apply, incremental markdown, `@tanstack/solid-virtual` transcript, tool rows, composer).
- Five encodings compared (json, json-slim, msgpack, msgpack-slim, bin) with Rust-written golden fixtures decoded by the TS tests; batching sweep 0/4/8/16/33 ms, 3 valid runs per cell.
- Benchmarks reproducible with `pnpm bench ...` (`scripts/bench.py`), reported with median and worst case.
- Report with recommendation: JSON with slim deltas, 8 ms batches + one apply per frame, markdown on the main thread (open block only), solid-virtual over markdown blocks (follow the tail from user input, not scroll distance).

## Key numbers vs budgets (Apple M4, macOS 27.0.1, WKWebView, 60 Hz, release build)
- 1 thread x 10k tokens/s x 60 s (3 runs): 60.0 fps, frame p50/p95/p99 17/18/18 ms, worst frame 79 ms (one run), dropped 0 (worst run 4), end-to-end delta latency p50/p95/p99/max 30/39/41/63 ms (worst run max 132 ms). Budget (60 fps): met.
- 10 threads x 1k tokens/s, one visible, typing (3 runs): 60.0 fps, 0 dropped (worst 1), latency p99 43 ms, **469/469 keystrokes delivered**, keystroke to paint p50/p99/max 25/34/36 ms vs 24/32/34 ms idle (synthetic key events). Budget (no dropped input): met.
- 5 minutes at 10k tokens/s, 3.0 M tokens, 79,818 rows: 60.0 fps, 3 dropped frames, latency p99 41 ms. Memory: WebContent footprint **1.05 to 1.39 GB while streaming**, 248 MB retained after (about 80 bytes/token), app process 37 MB, total 320 MB at rest. **No budget is set for the transcript, but this is the open risk** (PLAN §4 app idle RSS < 150 MB excluding agents).

## Not done / not measured
- Linux / WebKitGTK, 120 Hz, real OS key events (typing is synthetic), GPU/scanout time (latency stops at the frame commit), real agent markdown (a same-length markdown corpus replaces the fake agent's `x` text in bench mode).
- Cause of the 1 GB transient footprint: not found (not encoding-dependent: `bin` peaked at 0.9 to 1.07 GB). Needs a heap profile (M1).
- Overscan and row-height estimate were not tuned.

## Definition of Done results
- `cd spikes/stream-perf; pnpm typecheck; pnpm test`: `tsc --noEmit` clean; vitest 5 files, **24 tests passed** (blocks, markdown, model, metrics, decoders vs Rust fixtures).
- `cargo clippy --manifest-path spikes/stream-perf/src-tauri/Cargo.toml -- -D warnings` (also with `--all-targets`): pass. `cargo test --release --lib` in `src-tauri`: 3 passed.
- Benchmarks: all three required runs plus the sweep, microbenchmarks and a Rust encode table are in the report; commands in section 2 of the report.
- Recommendation for M1: report section 4. Change request CR-02 appended (drop the repeated text from `MessageCompleted`).

## Gotchas
- **The window must be visible**: WebKit stops `requestAnimationFrame` when it is hidden or covered. `bench.py` discards such runs (page `visibilitychange` log, frame count) and repeats them; the app is `alwaysOnTop`. Seven sweep runs, a few 60 s runs and several memory attempts were discarded for this (files `*-invalid*.json`).
- **GUI lock**: every run takes `/tmp/atrium-gui-measure.lock` with `flock(2)` inside `bench.py` (verified to interoperate with `/usr/bin/lockf -k`; do not also wrap `bench.py` in `lockf` on the same file, it would deadlock). The final 5-minute run held it throughout (the earlier orchestrator note about an unlocked run referred to a run started before the lock existed in my script, which was discarded).
- **macOS TCC**: a process spawned by an app cannot run a binary under `~/Documents` (it blocks in dyld waiting for a permission prompt). `bench.py` therefore runs the daemon and fake agent from a copy in `/tmp/atrium-m007-bin/`.
- `bench.py` kills everything it launches (app, daemon, fake agents) on exit, error, Ctrl-C and SIGTERM.
- The first 5-minute run exposed a follow-the-tail bug (scroll-distance based); fixed, first run kept as `memory-json-slim-b8-run1-lost-tail-follow.json`.
- Machine shared with other agents (load average 3 to 11 at run start, recorded per run): the two `bin` 60 s runs were hit by daemon stalls and are reported separately as load-affected.
- pnpm: `pnpm bench:build` builds the daemon with the root workspace (`target/release`), then the app with `pnpm tauri build --bundles app`.

## Follow-ups
- M1: heap profile of the webview while streaming; columnar/allocation-light batch decode; cheaper rows and a retention policy for long transcripts; sanitize link schemes; proper block parser for loose lists.
- M0-05 follow-up: batch event ingestion in the daemon (one transaction per event is the CPU cost at 10k events/s); incremental fold at session close.
- CR-02 (contract) for the orchestrator to decide.
