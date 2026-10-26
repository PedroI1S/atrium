# Handoff: M0-07-stream-perf

Model: Sonnet 5.5 · Status: done (fix round 1 applied)

Report: [docs/spikes/M0-07-stream-perf.md](../../spikes/M0-07-stream-perf.md) · Code: `spikes/stream-perf/` · Raw runs: `spikes/stream-perf/measurements/` · Branch `agent/sonnet/M0-07-stream-perf`

## Done
- Tauri 2 + SolidJS spike: M0-05 `ClientHandle`/`EventStream` (by path, unmodified) -> bridge task (batch, encode, `tauri::ipc::Channel`, ack-driven flow control and `applied`) -> webview (frame-batched apply, incremental markdown, `@tanstack/solid-virtual` transcript, tool rows, composer).
- Five encodings compared (json, json-slim, msgpack, msgpack-slim, bin) with Rust-written golden fixtures decoded by the TS tests; batching sweep 0/4/8/16/33 ms, 3 valid runs per cell.
- Benchmarks reproducible with `pnpm bench ...` (`scripts/bench.py`), reported with median and worst case.
- Report with recommendation: JSON with slim deltas, 8 ms batches + one apply per frame, markdown on the main thread (open block only), solid-virtual over markdown blocks (follow the tail from user input, not scroll distance).

## Key numbers vs budgets (Apple M4, macOS 27.0.1, WKWebView, 60 Hz, release build)
**Verdict: streaming budget met; input-latency budget not proven (synthetic events only).**
- 1 thread x 10k tokens/s x 60 s (fix-round build, 3 runs, load 6 to 8): 60.0 fps, frame p50/p95/p99/max 17/19/19/24 ms (worst run max 35), dropped 0 (worst 4). Latency agent timestamp to next rAF after the update (a rendering proxy) p50/p95/p99 30/39/75 ms, max 226 (worst run 1,527 ms); the tail is agent-to-bridge (daemon side), the webview segments are constant (IPC p99 1 ms, JS queue p99 17 ms).
- 10 threads x 1k tokens/s, one visible, synthetic typing (3 runs): 60.0 fps, dropped 1 (worst 3). Typist timers fire at most 6 ms late while streaming (idle 5.8); keystroke to paint from the scheduled deadline p50/p99/max 26.7 to 27.1 / 36.1 to 37.2 / 38.2 ms vs idle 24.5 to 27.8 / 32.9 to 34.1 / 36.9. **This does not prove "no dropped input"** (counts equal by construction; no OS input).
- 5 minutes at 10k tokens/s, 3.0 M tokens, 79,818 rows (pre-fix build): 60.0 fps, 3 dropped frames. WebContent footprint 1.05 to 1.39 GB while streaming, 248 MB after; app process 37 MB; total 320 MB at rest. Causes are hypotheses, not findings (report 3.3); M1 must profile.

## Not done / not measured
- Linux / WebKitGTK, 120 Hz, real OS key events (typing is synthetic), presentation latency (the metric stops at the next rAF callback), real agent markdown (a same-length markdown corpus replaces the fake agent's `x` text in bench mode).
- Cause of the 1 GB transient footprint and of the 250 MB retained: unknown; hypotheses listed in the report, profile in M1 (JS allocations, virtualizer measurement arrays, cached HTML, native WebKit memory).
- Overscan and row-height estimate were not tuned.

## Definition of Done results (after fix round 1)
- `cd spikes/stream-perf; pnpm typecheck; pnpm test`: `tsc --noEmit` clean; vitest 5 files, **27 tests passed**.
- `cargo clippy --manifest-path spikes/stream-perf/src-tauri/Cargo.toml -- -D warnings` (also with `--all-targets`): pass.
- `cargo test --release --lib` in `spikes/stream-perf/src-tauri`: **6 passed**. The stop test needs the daemon binaries: run `cargo build --release -p spike-daemon-socket --bins` at the repository root first (or set `ATRIUM_DAEMON_EXE`); without them it fails with a message saying so.
- Benchmarks and recommendation: report sections 3 and 4; change request CR-03 in `docs/agents/change-requests.md`.

## Fix round 1 (Sol review, 8 findings, all accepted)
| # | Change | Evidence |
|---|---|---|
| 1 | Typist uses absolute deadlines, latency counts from the deadline, timer lateness recorded; verdict qualified (input budget not proven) | 3 new 10-thread runs `measurements/fr1-multi-*` |
| 2 | Duplicate ids rejected before any model mutation (webview) and before the corpus offset advances (bridge) | 3 vitest + 2 cargo tests; with the check disabled, 2 of the 3 vitest tests (the third is the no-replay case) and the cargo dedupe test fail |
| 3 | `shutdown()`: drain mode, 5 s deadline, disconnect fallback | cargo test with a real daemon and exhausted credits; fails (`timed_out: true`) with drain mode off |
| 4 | Latency renamed "agent timestamp to next rAF after the update" (rendering proxy) | report |
| 5 | `classify()`: environment (page-logged hidden) vs failure (logged, counted); visible stalls are results; earlier discards disclosed | `bench.py`, report section 2 |
| 6 | Memory causes are hypotheses; profile list | report 3.3 |
| 7 | Decode ratio removed; recommendation rests on end-to-end data and simplicity | report section 4 |
| 8 | CR renumbered CR-03; wire size vs lifecycle-fold memory separated; versioned representation | change-requests.md |

Re-run live: 60 s single and 10-thread scenarios (3 each, under the GUI lock). Not re-run: sweep, 5-minute memory, microbenchmarks (pre-fix build; the code changes do not touch that path).

## Gotchas
- **The window must be visible**: WebKit stops `requestAnimationFrame` when it is hidden or covered. `bench.py` discards such runs (page `visibilitychange` log, frame count) and repeats them; the app is `alwaysOnTop`. Seven sweep runs, a few 60 s runs and several memory attempts were discarded for this (files `*-invalid*.json`).
- **GUI lock**: every run takes `/tmp/atrium-gui-measure.lock` with `flock(2)` inside `bench.py` (verified to interoperate with `/usr/bin/lockf -k`; do not also wrap `bench.py` in `lockf` on the same file, it would deadlock). The final 5-minute run held it throughout (the earlier orchestrator note about an unlocked run referred to a run started before the lock existed in my script, which was discarded).
- **macOS TCC**: a process spawned by an app cannot run a binary under `~/Documents` (it blocks in dyld waiting for a permission prompt). `bench.py` therefore runs the daemon and fake agent from a copy in `/tmp/atrium-m007-bin/`.
- Run-validity policy changed in fix round 1: only runs with page-logged hidden evidence are discarded as environmental; failures are logged in `measurements/failures.jsonl`. Earlier discards (nine retained files re-checked: all have the evidence; a few unretained attempts, listed in the report) are disclosed in report section 2.
- `bench.py` kills everything it launches (app, daemon, fake agents) on exit, error, Ctrl-C and SIGTERM.
- The first 5-minute run exposed a follow-the-tail bug (scroll-distance based); fixed, first run kept as `memory-json-slim-b8-run1-lost-tail-follow.json`.
- Machine shared with other agents (load average 3 to 11 at run start, recorded per run): the two `bin` 60 s runs were hit by daemon stalls and are reported separately as load-affected.
- pnpm: `pnpm bench:build` builds the daemon with the root workspace (`target/release`), then the app with `pnpm tauri build --bundles app`.

## Follow-ups
- M1: heap profile of the webview while streaming; columnar/allocation-light batch decode; cheaper rows and a retention policy for long transcripts; sanitize link schemes; proper block parser for loose lists.
- M0-05 follow-up: batch event ingestion in the daemon (one transaction per event is the CPU cost at 10k events/s); incremental fold at session close.
- CR-03 (contract) for the orchestrator to decide.
- OS-injected key events (Accessibility permission) to prove the input-latency budget.
