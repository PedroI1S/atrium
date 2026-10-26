# M0-07: 10k tokens/s through the client bridge without jank

**Verdict: PLAN §4 ("60 fps, no dropped input with 10 active threads") is met on this machine, with a lot of headroom in the UI path, and the encoding barely matters.** Daemon, unix socket, M0-05 client, Tauri `Channel`, WKWebView and a SolidJS transcript sustained 10,000 events/s (one token per event, the hardest case) at 60.0 fps in every valid run, including 5 minutes and 3.0 M tokens. Typing in the composer while streaming was indistinguishable from typing while idle, and every keystroke arrived. Three things are **not** fine and are listed under "Risks": the webview's transient memory (1.0 to 1.5 GB footprint while streaming), the retained transcript (about 250 MB per 3 M tokens), and the daemon, which is the first thing to fall behind when the machine is busy.

Branch `agent/sonnet/M0-07-stream-perf`. Code: `spikes/stream-perf/` (standalone Tauri 2 + SolidJS project; the Rust side depends on `spikes/daemon-socket` by path and uses its `ClientHandle`/`EventStream`, unmodified). Raw reports (one JSON per run, with the `visibility` log, CPU and memory series): `spikes/stream-perf/measurements/`.

## 1. What was built

```
fake agent process -> spike-daemon (SQLite WAL) -> unix socket -> M0-05 ClientHandle/EventStream
   -> bridge task (Rust, in the Tauri process): batch on a timer, encode, tauri::ipc::Channel
   -> webview: decode -> queue -> once per animation frame: apply to model + Solid batch()
   -> next frame (the update has been committed): bridge_ack(batch seq, per-thread highest id)
   -> bridge calls ClientHandle::applied(thread, id)
```

- **Bridge** (`src-tauri/src/bridge.rs`): one task drains the `EventStream` continuously (M0-05 requires it), groups events into batches (`batchMs` timer, 0 = one message per event, or 4096 events) and sends each batch as one `Channel` message. **Flow control**: the webview acknowledges `(batch seq, highest applied id per thread)` after the frame that applied them was committed; only then does the bridge call `applied` (the cursor rule in `wire.rs`). While more than 8 MiB of batches are unacknowledged the task stops reading the stream, so a slow webview pushes back on the daemon instead of growing a queue (it engaged once in 3 M events).
- **Five encodings** (`src-tauri/src/encode.rs`, decoders in `src/wire.ts`, cross-checked by golden fixtures that the Rust example writes and vitest decodes): `json` (the daemon's own `StoredEvent`), `json-slim` (deltas reduced to `{t,i,th,a,m,k,s}`), `msgpack` and `msgpack-slim` (same shapes, `rmp-serde` to `@msgpack/msgpack`), `bin` (hand-rolled little-endian layout; non-delta events carry JSON).
- **Webview**: `pipeline.ts` (frame loop, acknowledgements, latency histograms), `model.ts` (per-thread transcript; hidden threads are applied to the model too, only the visible thread has a view), `blocks.ts` (incremental block splitter: blank-line separated blocks, fenced code kept whole), `markdown.ts` (`marked`, raw HTML escaped, an unfinished code fence skips the parser), `Transcript.tsx` (`@tanstack/solid-virtual`, rows are markdown blocks, dynamic heights, follows the tail until the user scrolls up), tool-call rows from `ToolCall*` events, `Composer.tsx`.
- **Harness**: `src/bench.ts` (scenarios, frame sampler, report) and `scripts/bench.py` (build, launch, memory/CPU sampling, validity checks). Theme tokens: the semantic names and values from M0-06's Flat Dark theme, copied into `src/theme.css` (nothing imported).
- **Bench corpus**: the fake agent streams `x` repeated, which would test neither the parser nor layout. In bench mode the bridge replaces each delta's text with a slice of a markdown corpus (headings, lists, code fences, tables, inline code, links) of **identical length**, so event count, bytes on the socket and token rate are the daemon's.

## 2. Method

- **Machine**: Apple M4 (10 cores), macOS 27.0.1, WKWebView (Tauri 2.12), 60 Hz built-in panel, release build (`pnpm tauri build --bundles app`), Rust 1.99.1, window 1200 x 800 (always on top, see below).
- **One token = 4 characters** (the contract's convention). 10k tokens/s is the fake agent at `chunk_chars: 4`, i.e. **10,000 `MessageDelta` events/s, one per token**, plus a tool call every 4 s. Achieved: 9,979 to 9,993 tokens/s in the valid runs.
- **Frame time**: `requestAnimationFrame` timestamp deltas for the whole stream. WebKit rounds timers to 1 ms and a 60 Hz frame is 16.67 ms, so p50 reads 17. **Dropped frames** = sum over frames longer than 1.5x the refresh interval of `round(dt / 16.67) - 1`. Compositor/GPU drops are invisible to JS.
- **End-to-end delta latency**: agent timestamp (`at_ms`, stamped by the fake agent process) to the **start of the animation frame after the one that applied the delta**, i.e. after that frame has been committed. It excludes GPU raster and scanout (up to one more frame), so it understates what the eye sees by up to ~17 ms. Same wall clock everywhere (one machine); `at_ms` has 1 ms resolution. Histogram with 1 ms buckets, one sample per visible-thread delta (up to 3 M samples). Segments are measured per event: agent to bridge, time in the batch, IPC (encode to JS handler), JS queue wait.
- **Keystroke to paint**: three points relative to the event's `timeStamp`: input handler start, start of the next rAF, start of the rAF after that (the frame holding the new text was committed). The harness' typist **dispatches synthetic `keydown`/`input` events** every 120 ms +- 40% (it did not use OS key events, which need Accessibility permission). It therefore measures main-thread contention and paint, not OS or WebKit input routing (a few ms more). An idle reference (4 s of typing before the stream) is taken in the same run. "Paint" includes up to two frame waits by construction, hence about 24 ms even when idle.
- **Memory**: `footprint -p` (macOS physical footprint) of the app, the WebKit helpers whose responsible process is the app (`WebContent`, `Networking`/GPU), the daemon and its fake agents, every 30 s during the run and at 0, 15 and 60 s after it. JS heap is not readable in WKWebView.
- **Validity**: the window must stay visible (WebKit stops rAF when it is hidden or covered). A run is **discarded and repeated** (kept as `*-invalid*.json`) if the page reports `visibilitychange` to hidden during the stream, produced under 90% of the expected frames, stalled over 1 s, timed out or errored. No numbers were tuned or cherry-picked; every valid run is reported, with the worst repetition next to the median. To stop covered windows the app is `alwaysOnTop`, and runs take `/tmp/atrium-gui-measure.lock` (`flock`, interoperable with `/usr/bin/lockf -k`, checked) because other agents also open windows; the script waited for it at least once.
- **Shared machine**: other agents compiled and benchmarked during these runs. The 1-minute load average at start is recorded in every report (3 to 11); the sweep and the headline runs started at 4 to 7. Treat tails as "on a lightly shared machine".

Commands (fish-compatible; run from the repository root):

```fish
cd spikes/stream-perf
pnpm install
pnpm bench:build                                   # daemon (root workspace), frontend, release .app
pnpm bench micro                                   # decode + markdown microbenchmarks in the webview
pnpm bench sweep --secs 20 --reps 3 --encodings json,msgpack,bin
pnpm bench sweep --secs 20 --reps 3 --encodings json-slim,msgpack-slim --batches 8
pnpm bench single --secs 60 --encoding json-slim --batch-ms 8 --label single-json-slim-b8-r1
pnpm bench multi  --secs 60 --encoding json-slim --batch-ms 8 --label multi-json-slim-b8-r1
pnpm bench memory --secs 300 --encoding json-slim --batch-ms 8
cargo run --release --manifest-path src-tauri/Cargo.toml --example stream-perf-encode-bench
```

(`pnpm bench` is `python3 scripts/bench.py`; `single`/`multi` ran 3 repetitions each with `-r1..r3` labels.) The window must be visible and unobstructed during a run.

## 3. Results

### 3.1 One thread, 10k tokens/s, 60 s (json-slim, 8 ms batches; 3 runs)

| | median of 3 | worst of 3 |
|---|---:|---:|
| tokens/s achieved (events/s) | 9,979 | 9,976 |
| events sent / applied | 598,954 / 598,954 | no loss, no gaps in batch sequence |
| fps | 60.0 | 59.9 |
| frame interval p50 / p95 / p99 / max (ms) | 17 / 18 / 18 / 24 | 17 / 18 / 18 / **79** |
| dropped frames | 0 | **4** (one slow frame) |
| delta latency, agent to committed frame, p50 / p95 / p99 / max (ms) | 30 / 39 / 41 / 63 | 30 / 39 / 41 / **132** |
| segments p50 / p99 (ms): agent to bridge, batching, IPC, JS queue | 0 / 2, 4 / 8, 0 / 1, 8 / 16 | 0 / 5, 4 / 8, 0 / 1, 8 / 16 |
| per frame: apply (JS + Solid DOM) p50 / p99 / max (ms) | 1 / 2 / 11 | 1 / 2 / 14 |
| per frame: rAF callback to next task (style/layout/paint recording) p99 / max (ms) | 3 / 10 | 3 / 14 |
| CPU during stream (mean of one core): app / WebContent / other WebKit / daemon / agents | 17 / 19 / 20 / 58 / 3 % | |

Reports: `measurements/single-json-slim-b8-r{1,2,3}.json`. One earlier run of this configuration was discarded because another window hid the app (`single-json-slim-b8-r1-invalid1.json`). The latency floor is the frame structure, not the transport: 4 ms mean batching + 8 ms waiting for the next frame + a frame to commit = about 30 ms; IPC is under 1 ms at p99.

### 3.2 10 threads x 1k tokens/s, one visible, typing in the composer (json-slim, 8 ms; 3 runs of 60 s)

| | median of 3 | worst of 3 |
|---|---:|---:|
| total tokens/s achieved | 9,811 | 9,807 |
| events sent / applied (all 10 threads) | 598,273 / 598,273 | no loss |
| fps | 60.0 | 60.0 |
| frame interval p50 / p95 / p99 / max (ms) | 17 / 18 / 19 / 23 | 17 / 18 / 19 / 32 |
| dropped frames | 0 | 1 |
| delta latency (visible thread), p50 / p95 / p99 / max (ms) | 30 / 39 / 43 / 100 | 30 / 39 / 48 / 109 |
| apply per frame p99 / max (ms) | 1 / 5 | 1 / 6 |
| **keystrokes delivered** | 469/469, 471/471, 470/470 (100%) | |
| **keystroke to paint** while streaming, p50 / p95 / p99 / max (ms) | 25 / 33 / 34 / 35 | 25 / 33 / 34 / **36** |
| keystroke to paint, idle reference in the same runs | 24 / 32 / 32 / 34 | 26 / 33 / 33 / 34 |
| input handler delay p50 / p99 / max (ms) | 0 / 1 / 1 | 0 / 1 / 1 |
| CPU: app / WebContent / other WebKit / daemon / agents | 18 / 16 / 15 / 85 / 25 % | |

Reports: `measurements/multi-json-slim-b8-r{1,2,3}.json`. Streaming made no measurable difference to typing (the handler runs within 1 ms of the event, paint latency within 1 ms of idle): the per-frame work is 1 to 5 ms against a 16.7 ms budget. The nine hidden threads are applied to their models but never rendered. This is the PLAN §4 budget, with the synthetic-input caveat above.

### 3.3 Memory after 5 minutes (1 thread, 10k tokens/s, 300 s, json-slim, 8 ms)

2,999,239 tokens (11,996,956 characters) streamed; 79,818 transcript rows (markdown blocks); `measurements/memory-json-slim-b8.json`.

| | |
|---|---|
| fps / dropped frames over 5 min | 60.0 / 3 (2 slow frames, max 53 ms) |
| delta latency p50 / p95 / p99 / max (ms) | 30 / 38 / 41 / 72 |
| events received = applied | 2,999,392 (all) |
| DOM nodes at the end (virtualized) | 130; view at the tail (distance 0 px) |
| **WebContent footprint while streaming** | **1.05 to 1.39 GB** (samples every 30 s), 8 MB at start |
| WebContent footprint after the stream, +0 / +15 / +60 s | 357 / 248 / 248 MB |
| app process (Rust) footprint | 37 to 38 MB streaming, 37 MB after |
| other WebKit helpers | 81 to 94 MB streaming, 26 MB after |
| daemon + agents | 5.6 + up to 13 MB streaming; **487 MB at session close** (the daemon folds the 3 M-event history, M0-05's known behaviour), 8.9 MB 15 s later |
| total after 15 s / 60 s | 321 / 320 MB |

Reading: the retained cost of the transcript is about **250 MB per 3 M tokens (about 80 bytes per token, 3 KB per row)**, mostly the Solid-signal-carrying row objects and strings. The footprint **while streaming** is four to five times that; it is garbage that the JS engine has not collected yet (it drops within seconds of the stream ending) and it did **not** depend on the encoding (a 90 s run with `bin` peaked at 0.88 to 1.07 GB, `measurements/memory-90s-bin-b8.json`). Without a heap profile I cannot say which allocations dominate (per-event objects, the parsed JSON, row signals); that is an M1 task (Risks). A first 5-minute attempt finished with the view stuck 622,738 px above the tail: a flaw in the follow-the-tail logic (it decided from scroll distance, which grows when rows arrive faster than a frame). It was fixed (follow is dropped only on user wheel/key/pointer input) and the run above repeated; the first run is kept as `memory-json-slim-b8-run1-lost-tail-follow.json`. A 150 s run is also kept (`memory-150s-json-slim-b8.json`).

### 3.4 Encodings and batching interval (1 thread, 10k tokens/s, 20 s, 3 valid runs per cell, median (worst))

`pnpm bench sweep` output. "MB over IPC" is bytes sent through the channel in 20 s (200,000 events); "Rust encode" and "JS decode" are total time in those functions per run (the JS number excludes Tauri's own JSON parse for the `json*` encodings).

| encoding | batch ms | fps | frame p99 | frame max | dropped | e2e p50 | e2e p95 | e2e p99 | e2e max | MB over IPC | Rust encode ms | JS decode ms | app cpu % | webcontent cpu % |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| json | 0 | 60.0 | 19 (22) | 27 (47) | 1 (5) | 28 | 96 | 161 (162) | 259 (302) | 44.2 | 88 | 53 | 67 | 62 |
| json | 4 | 60.0 | 18 (19) | 24 (27) | 0 (2) | 28 | 36 | 39 (93) | 60 (149) | 36.7 | 59 | 15 | 20 | 23 |
| json | **8** | 60.0 | 18 (19) | 20 (23) | 0 (0) | 30 | 39 | 42 (42) | 58 (65) | 36.9 | 49 | 12 | 20 | 22 |
| json | 16 | 60.0 | 18 (19) | 21 (22) | 0 (0) | 34 | 46 | 49 (70) | 74 (314) | 36.8 | 48 | 11 | 20 | 21 |
| json | 33 | 60.0 | 19 (20) | 23 (32) | 0 (4) | 44 | 61 | 66 (78) | 88 (116) | 36.6 | 41 | 8 | 18 | 18 |
| msgpack | 0 | 60.0 | 20 (20) | 26 (27) | 1 (3) | 27 | 35 | 69 (179) | 108 (239) | 34.4 | 146 | 446 | 77 | 72 |
| msgpack | 4 | 60.0 | 19 (21) | 23 (29) | 0 (3) | 29 | 37 | 39 (169) | 55 (331) | 28.5 | 50 | 226 | 25 | 27 |
| msgpack | 8 | 60.0 | 19 (21) | 30 (30) | 1 (3) | 31 | 40 | 52 (222) | 97 (417) | 28.1 | 56 | 236 | 27 | 29 |
| msgpack | 16 | 60.0 | 19 (19) | 20 (22) | 0 (0) | 35 | 56 | 107 (117) | 154 (258) | 28.0 | 45 | 279 | 22 | 24 |
| msgpack | 33 | 60.0 | 19 (19) | 22 (23) | 0 (0) | 43 | 61 | 65 (399) | 70 (452) | 28.3 | 32 | 230 | 17 | 19 |
| bin | 0 | 60.0 | 19 (22) | 28 (34) | 1 (4) | 28 | 77 | 103 (145) | 168 (216) | 9.9 | 10 | 174 | 71 | 65 |
| bin | 4 | 60.0 | 19 (19) | 28 (30) | 1 (1) | 28 | 36 | 38 (101) | 53 (161) | 6.9 | 6 | 88 | 25 | 25 |
| bin | **8** | 60.0 | 19 (19) | 20 (28) | 0 (1) | 30 | 39 | 41 (48) | 46 (72) | 6.9 | 6 | 53 | 20 | 21 |
| bin | 16 | 60.0 | 19 (20) | 23 (30) | 0 (1) | 35 | 97 | 119 (180) | 244 (263) | 6.6 | 9 | 64 | 25 | 25 |
| bin | 33 | 60.0 | 19 (19) | 28 (30) | 1 (1) | 43 | 61 | 65 (75) | 92 (187) | 6.8 | 6 | 49 | 20 | 19 |
| json-slim | 8 | 60.0 | 19 (19) | 21 (24) | 0 (0) | 30 | 39 | 41 (41) | 47 (53) | 15.1 | 30 | 6 | 18 | 19 |
| msgpack-slim | 8 | 60.0 | 19 (19) | 21 (21) | 0 (0) | 30 | 39 | 41 (42) | 49 (55) | 8.9 | 27 | 99 | 20 | 21 |

(The slim rows are a separate 3-run sweep at 8 ms only. Raw files: `measurements/sweep-*.json`; seven sweep runs were discarded as invalid and are kept.)

What the table says:

- **Every cell holds 60 fps.** At this rate the encoding does not decide whether the UI keeps up.
- **Batching, not encoding, moves the numbers.** With no batching (0 ms) CPU triples (app + WebContent 129 % versus 42 % of a core at 8 ms) and the tail gets worse (json p99 161 ms). At 4 and 8 ms the end-to-end p99 is 38 to 42 ms; 16 and 33 ms only add latency (p50 34 and 43 ms) and save no CPU. The wide "worst" values (a few cells reach p99 of 100 to 400 ms, e.g. `msgpack` 8 ms worst 222) come from single repetitions that were disturbed by other load on the machine; the cell medians and the second repetition do not show them.
- **Bytes**: `bin` sends 5.3x fewer bytes than `json` (6.9 vs 36.9 MB per 20 s), `json-slim` 2.4x fewer. **Rust encode**: `bin` 6 ms vs `json` 49 ms per 200 k events. **JS decode**: `json` 12 ms, `json-slim` 6 ms, `bin` 53 ms, **`msgpack` 236 ms**: the MessagePack library is 2.5 times (micro) to 20 times (in the stream) slower than `JSON.parse` in JavaScriptCore and is the only encoding that costs measurable main-thread time. All are 0.03 to 1.2 % of one core.

Standalone microbenchmarks (the report's numbers behind this; `measurements/micro.json`, `measurements/encode-bench.md`), per event at 4-character deltas, 160 events per batch:

| encoding | bytes/event | Rust encode (ns/event) | JS decode in WKWebView (ns/event) |
|---|---:|---:|---:|
| json | 156 | 218 | 238 |
| json-slim | 68 | 82 | 164 |
| msgpack | 114 | 120 | 603 |
| msgpack-slim | 37 | 82 | 328 |
| bin | 30 | 8 | 149 |

Markdown in WKWebView (`measurements/micro.json`): a closed 600-character block parses in 39 us; the open block re-parsed every frame costs 7 us (100 chars), 39 us (600), 47 us (1,200), 145 us (2,400), 167 us (4,800); an unfinished code fence rendered as plain text costs 40 us at 100 KB; the block splitter costs 18 ns per character. At 60 fps that is 2 to 10 ms of main-thread time per second in the worst realistic case.

### 3.5 Runs affected by machine load (reported, not used for conclusions)

`single-bin-b8-r1.json` and `multi-bin-b8-r1.json` (60 s each) started at load averages of 11.5 and 8.0 while other agents compiled. The daemon, not the UI, fell behind: delivery 8,948 and 8,637 events/s, agent-to-bridge p99 308 and 1,330 ms, end-to-end p99 338 ms and 1,346 ms. The UI numbers in the same runs are normal (60 fps, apply p99 2 ms; typing paint p50/p99 26/34 ms, 482/482 keystrokes). The daemon used 79 to 132 % of a core in them (55 to 85 % in the quiet runs, plus 3 to 25 % for the fake agents).

## 4. Recommendation for M1

1. **Encoding: JSON, with deltas slimmed (`json-slim`), through the Tauri JSON `Channel`.** Numbers: indistinguishable from `bin` in frame time, latency and CPU at batch 8 ms (e2e p99 41 vs 41 ms, CPU 18/19 vs 20/21 %), no custom format to version or debug, smaller than the daemon's own JSON (2.4x). `bin` is 5.3x smaller and 8x cheaper to encode but 9x more JS decode time than `json-slim`; adopt it only if real workloads (large tool outputs, diffs) show IPC volume to matter, and then take `bin`, **not MessagePack**. Do not use `msgpack` with `@msgpack/msgpack`. Whatever the encoding, the webview must keep the per-event decode allocation-light (see Risks).
2. **Batching: 8 ms timer in the Rust bridge, plus one apply per animation frame in the webview.** Never send one message per event (3x CPU, worst tail); 4 ms buys nothing over 8; 16 and 33 ms add 5 and 13 ms of latency. Keep the 8 MiB unacknowledged-bytes limit and the ack coalesced to at most one per 30 ms.
3. **Markdown parses on the main thread**, incrementally: closed blocks once (`marked` or its M1 replacement), only the open block per frame, an unfinished fence as plain text, raw HTML escaped. Cost is 2 to 10 ms per second at 10k tokens/s, so a worker adds structured-clone cost and latency for nothing. Revisit a worker for Shiki highlighting and for any single open block above about 20 KB (parse cost is linear, roughly 35 ns per character; 100 KB would be 3.5 ms per frame).
4. **Virtualization: `@tanstack/solid-virtual`, rows are markdown blocks (not messages), overscan 6, dynamic measurement, estimate from text length; one list keyed by row index** (a `For` over index numbers so rows keep their DOM). 80,000 rows stayed at about 130 DOM nodes. Follow the tail until the user scrolls up, decided from wheel/key/pointer events, **not** from scroll distance. Overscan and estimate size were not tuned. Hidden threads should keep applying events to their model without a view (10 threads x 1k tokens/s: apply p99 1 ms per frame).
5. **Cursor and acknowledgement**: keep the "ack after the frame that applied the events was committed" design as built. The UI dedupes `id <= last applied`.
6. **Contract note** (see change request CR-02): `MessageCompleted.text` repeats every delta. For this stream it is 12 MB (the 16 MiB wire limit is close) and it is why the daemon's fold at session close peaked at 487 MB. The bridge should drop it when the webview already holds the deltas.

## 5. Risks and what was not measured

- **Webview memory.** 1.05 to 1.39 GB footprint while streaming 10k events/s (independent of encoding) and about 250 MB retained for 3 M tokens (about 80 bytes per token) against PLAN §4's app budget of 150 MB idle RSS excluding agents (the Rust app process itself is 37 MB). Needs a JS heap profile, fewer per-event allocations (a columnar batch decode instead of one object per event), cheaper rows (no signal per closed row, drop cached HTML for rows far from the viewport) and a retention policy (keep the last N thousand rows in memory and re-page older history from the daemon). **I did not find the cause of the transient.**
- **The daemon is the weakest link under load**: 55 to 85 % of a core for 10k events/s plus the fake agents (one process write and one SQLite transaction per event). Under CPU contention it delivered 8.6 to 8.9k events/s and p99 delays of 0.3 to 1.3 s. Real agents emit far fewer events (tens to hundreds per second per thread), but the M0-05 ingestion batching follow-up is the first thing to do for headroom.
- **Not measured**: Linux / WebKitGTK (a Linux run must check: `rAF` cadence while streaming, that raw `Channel` messages and the fetch path behave, `footprint` equivalent via smem/`/proc`, and the same typing probe); 120 Hz panels; real OS key events (synthetic only; the harness has no OS key injection); GPU raster and scanout (latency stops at the commit of the frame); real agent markdown (the corpus is synthetic, with a tool call every 4 s); chunk sizes above 4 characters (the bridge is not the limit there: fewer events carry the same bytes); more than one visible thread; reconnect while streaming (the M0-05 tests cover the client, not this bridge).
- **Environment**: results are from a shared interactive Mac. Runs where the window was hidden were discarded by rule; their evidence is kept.
- `marked` renders links without sanitizing the scheme; M1 needs an allow-list. Block splitting breaks loose lists and quotes into several blocks (an ordered list restarts its numbering).

## 6. Escalation check

The brief says to escalate if WKWebView cannot hold 60 fps at 10k tokens/s with batching. It can, in every valid run (60.0 fps; one 79 ms frame in 3 x 60 s; 3 dropped frames in 5 minutes). No escalation.

## 7. Credit

Design ideas only, no code copied: the M0-05 report's client example (cursor rule), Tauri's `Channel` documentation. `marked`, `@msgpack/msgpack`, `@tanstack/solid-virtual` are MIT-licensed dependencies.
