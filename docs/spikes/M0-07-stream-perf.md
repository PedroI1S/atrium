# M0-07: 10k tokens/s through the client bridge without jank

**Verdict: the streaming budget is met on this machine; the input-latency budget is not proven.** Daemon, unix socket, M0-05 client, Tauri `Channel`, WKWebView and a SolidJS transcript sustained 10,000 events/s (one token per event, the hardest case) at 60.0 fps in every run that has a result, including 5 minutes and 3.0 M tokens, with 0 to 4 skipped display updates per 60 s. PLAN §4's second half, "no dropped input with 10 active threads", is **not demonstrated**: the composer was driven with synthetic events, which cannot show OS or WebKit input routing. What the synthetic typing does show is that, with 10 threads streaming, the main thread keeps its timers within about 6 ms and a keystroke's paint comes within about 3 ms of the idle figure. The encoding barely matters. Three things are **not** fine and are listed under "Risks": the webview's transient memory (1.0 to 1.5 GB footprint while streaming), the retained transcript (about 250 MB per 3 M tokens), and the daemon, which is the first thing to fall behind when the machine is busy.

*Fix round 1 (GPT-6.1 Sol review) changed the code (replay dedupe, shutdown under backpressure), the keystroke measurement, the latency name and the validity policy; section 8 lists what was re-measured and what was not.*

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
- **Latency, "agent timestamp to next rAF after the update"** (reported under the key `latency`; earlier drafts called it "end-to-end" and "committed frame"): from the agent's timestamp (`at_ms`, stamped by the fake agent process) to the start of the next animation-frame callback after the callback that applied the delta to the model and the DOM. It is a **rendering proxy**. It does not observe compositing, GPU raster or scanout, so it is not a presentation or "painted" latency, and nothing here bounds how much later the pixels change; that would need presentation tracing (for example Instruments or a camera). Same wall clock everywhere (one machine); `at_ms` has 1 ms resolution. Histogram with 1 ms buckets, one sample per visible-thread delta (up to 3 M samples). Segments are measured per event: agent to bridge, time in the batch, IPC (encode to JS handler), JS queue wait.
- **Keystroke to paint (synthetic)**: three points relative to the keystroke's origin: input handler start, start of the next rAF, start of the rAF after that. The harness' typist **dispatches synthetic `keydown`/`input` events**; it did not use OS key events (they need Accessibility permission). Since fix round 1 each keystroke has a **scheduled deadline on an absolute, jittered schedule** (120 ms +- 40%, next deadline = previous deadline + interval), the origin of every latency is that deadline, and the **timer lateness** (when the timer actually fired minus the deadline) is reported separately; a late timer fires the following keystrokes back to back, none are skipped. Before the fix the origin was the moment the event object was created, which hid any main-thread delay of the timer itself. "Delivered" counts equal "sent" by construction in this path, so **they prove nothing about dropped input**. An idle reference (4 s of typing before the stream) is taken in the same run. "Paint" includes up to two frame waits by construction, hence about 25 ms even when idle. Not measured: real OS input, which a Linux and macOS run with injected key events must cover.
- **Memory**: `footprint -p` (macOS physical footprint) of the app, the WebKit helpers whose responsible process is the app (`WebContent`, `Networking`/GPU), the daemon and its fake agents, every 30 s during the run and at 0, 15 and 60 s after it. JS heap is not readable in WKWebView.
- **Validity, and what was discarded** (policy since fix round 1, `classify()` in `bench.py`): the window must stay visible, because WebKit stops rAF when it is hidden or covered. A run is **environmental** and discarded and repeated only with independent evidence: the page logged `visibilitychange` to hidden during the stream, or its progress snapshot said `document.hidden` (kept as `*-environment*.json`). Anything else that goes wrong (error, timeout, no report, daemon not ready) is a **failure**: it is logged in `measurements/failures.jsonl`, counted, reported, and never called an artifact. A run with a long frame or a low frame count while the window was visible is **a result**; it is kept and its `anomalies` are printed. Every run that has a result is reported, with the worst repetition next to the median. Disclosure of what was discarded before this policy existed: nine runs kept as `*-invalid*.json` (seven sweep runs, one 60 s `single`, one 60 s `multi`); I re-checked each: all nine carry the page's own `visibilitychange`-to-hidden log inside the stream window. Not kept, and therefore not independently checkable now: four 5-minute memory attempts that timed out without a report (one progress snapshot said hidden, the others are unexplained; I did not investigate), several runs lost to the daemon failing to start (macOS blocks a binary under `~/Documents` that an app spawns; fixed by running a copy from `/tmp`), a 25-run first sweep (kept in `/tmp` only) replaced when the visibility log was added, and one more memory attempt hidden at 172 s. To stop covered windows the app is `alwaysOnTop`; runs take `/tmp/atrium-gui-measure.lock` (`flock`, interoperable with `/usr/bin/lockf -k`, checked) because other agents also open windows.
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

### 3.1 One thread, 10k tokens/s, 60 s (json-slim, 8 ms batches; fix-round build, 3 runs)

| | median of 3 | worst of 3 |
|---|---:|---:|
| tokens/s achieved | 9,922 | 9,754 |
| events applied (all sent events arrive, none twice: `duplicatesDropped` 0) | 595,488 | 585,412 |
| fps | 60.0 | 60.0 |
| frame interval p50 / p95 / p99 / max (ms) | 17 / 19 / 19 / 24 | 17 / 19 / 19 / 35 |
| dropped frames | 0 | 4 |
| latency (agent timestamp to next rAF after the update) p50 / p95 / p99 / max (ms) | 30 / 39 / 75 / 226 | 30 / 40 / 95 / 1,527 |
| agent to bridge p99 / max (ms), the three runs | 9 / 1,497, 46 / 196, 66 / 180 | |
| webview side per run: IPC p99, JS queue p99 (ms) | 1, 17 | 1, 17 |
| per frame: apply (JS + Solid DOM) p99 / max (ms) | 2 / 12 | 2 / 15 |
| daemon CPU during the stream (% of one core) | 65 to 86 | |

Reports: `measurements/fr1-single-json-slim-b8-r{1,2,3}.json`. The latency floor is the frame structure: about 4 ms mean batching + 8 ms waiting for the next frame + the frame itself = about 30 ms at p50. The tail is **upstream of the webview**: the webview-side segments are the same in all three runs (IPC p99 1 ms, JS queue p99 17 ms), while agent-to-bridge p99 and max vary, and the machine was busier than for the earlier set (load average 6 to 8 at start). An earlier set of three runs on the build before fix round 1 (load 4 to 5) gave latency p50/p95/p99/max 30 / 39 / 41 / 63 ms (worst run max 132), the same frame numbers, and 17 / 19 / 20 / 58 / 3 % CPU for app / WebContent / other WebKit / daemon / agents (`measurements/single-json-slim-b8-r{1,2,3}.json`). I attribute the difference to the daemon under load, not to the fix; the data cannot exclude a small effect of the one-comparison dedupe.

### 3.2 10 threads x 1k tokens/s, one visible, typing in the composer (json-slim, 8 ms; fix-round build, 3 runs of 60 s)

| | median of 3 | worst of 3 |
|---|---:|---:|
| total tokens/s achieved | 9,868 | 9,743 |
| events applied, all 10 threads | 599,573 | 592,259 |
| fps | 60.0 | 60.0 |
| frame interval p50 / p95 / p99 / max (ms) | 17 / 18 / 19 / 33 | 17 / 18 / 19 / 51 |
| dropped frames | 1 | 3 |
| latency (visible thread) p50 / p95 / p99 / max (ms) | 30 / 39 / 41 / 89 | 30 / 118 / 730 / 941 |
| agent to bridge p99 (ms), the three runs | 2, 670, 2 | |
| apply per frame p99 / max (ms) | 1 / 7 | 2 / 8 |
| keystrokes sent = inserted into the textarea | 479 / 479 / 479 (true by construction, see section 2) | |
| **timer lateness** of the typist while streaming, p50 / p99 / max (ms) | 2.3 / 5.0 / 5.9 | 2.4 / 5.6 / 6.0 |
| timer lateness in the idle reference, p50 / p99 / max (ms) | 1.4 to 2.5 / 4.6 to 5.1 / 4.8 to 5.8 | |
| **keystroke to paint, from the deadline,** while streaming, p50 / p95 / p99 / max (ms) | 26.7 to 27.1 / 34.8 to 35.3 / 36.1 to 37.2 / 37.6 to 38.2 (3 runs) | |
| keystroke to paint, idle reference of the same runs, p50 / p95 / p99 / max (ms) | 24.5, 27.2, 27.8 / 32.0, 33.5, 32.6 / 33.6, 34.1, 32.9 / 36.9, 35.0, 33.5 | |
| CPU (earlier build, same scenario): app / WebContent / other WebKit / daemon / agents | 18 / 16 / 15 / 85 / 25 % | |

Reports: `measurements/fr1-multi-json-slim-b8-r{1,2,3}.json` (earlier build: `multi-json-slim-b8-r*.json`, origin = event creation). Reading: while ten threads stream, the typist's timers fire at most 6 ms late (about 1 ms more than idle at the median) and a keystroke's paint is within about 3 ms of the idle figure at every percentile; per-frame work is 1 to 8 ms against a 16.7 ms budget. The nine hidden threads are applied to their models but never rendered. The worst repetition's latency tail (p99 730 ms) is the daemon: agent-to-bridge p99 was 670 ms in that run and 2 ms in the others. **This supports "streaming does not starve the main thread". It does not demonstrate "no dropped input"**, which needs OS-injected keys.

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

Reading: after the stream the WebContent footprint settles at about **250 MB per 3 M tokens (about 80 bytes per token, 3 KB per row)**, and the footprint **while streaming** is four to five times that and falls within seconds of the stream ending. The measurements establish only those numbers and that the transient did **not** change with the encoding (a 90 s run with `bin` peaked at 0.88 to 1.07 GB, `measurements/memory-90s-bin-b8.json`). Process footprint cannot attribute either number, so the causes below are **hypotheses, not findings**: (a) retained: the row objects with a Solid signal each, strings, and possibly the virtualizer's measurement storage, which the pinned `@tanstack/solid-virtual` rebuilds at the size of the row count whenever the count changes (the count changes on most frames); cached HTML is not a suspect here because closed rows that were never mounted are never rendered; (b) transient: JS garbage from per-event objects and parsed messages that the engine has not collected, or WebKit-native memory (layout, the IPC fetch path). M1 must profile before choosing a remedy: JS allocations (Web Inspector allocation timeline), virtualizer measurement arrays, cached HTML, and native WebKit memory (`vmmap`/Instruments). A first 5-minute attempt finished with the view stuck 622,738 px above the tail: a flaw in the follow-the-tail logic (it decided from scroll distance, which grows when rows arrive faster than a frame). It was fixed (follow is dropped only on user wheel/key/pointer input) and the run above repeated; the first run is kept as `memory-json-slim-b8-run1-lost-tail-follow.json`. A 150 s run is also kept (`memory-150s-json-slim-b8.json`).

### 3.4 Encodings and batching interval (1 thread, 10k tokens/s, 20 s, 3 valid runs per cell, median (worst))

`pnpm bench sweep` output, from the build before fix round 1 (the latency columns are the "agent timestamp to next rAF after the update" metric). "MB over IPC" is bytes sent through the channel in 20 s (200,000 events); "Rust encode" and "JS decode" are total time in those functions per run (the JS number excludes Tauri's own JSON parse for the `json*` encodings).

| encoding | batch ms | fps | frame p99 | frame max | dropped | latency p50 | latency p95 | latency p99 | latency max | MB over IPC | Rust encode ms | JS decode ms | app cpu % | webcontent cpu % |
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
- **Batching, not encoding, moves the numbers.** With no batching (0 ms) CPU triples (app + WebContent 129 % versus 42 % of a core at 8 ms) and the tail gets worse (json p99 161 ms). At 4 and 8 ms the latency p99 is 38 to 42 ms; 16 and 33 ms only add latency (p50 34 and 43 ms) and save no CPU. The wide "worst" values (a few cells reach p99 of 100 to 400 ms, e.g. `msgpack` 8 ms worst 222) come from single repetitions that were disturbed by other load on the machine; the cell medians and the second repetition do not show them.
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

1. **Encoding: JSON, with deltas slimmed (`json-slim`), through the Tauri JSON `Channel`.** The basis is comparable end-to-end data plus simplicity: at batch 8 ms `json-slim` and `bin` give the same frame times, the same latency (p99 41 vs 41 ms, worst 41 vs 48) and the same CPU (18/19 vs 20/21 % app/WebContent), `json-slim` needs no custom format to version or debug, and it is 2.4x smaller than the daemon's own JSON. `bin` is 5.3x smaller and about 8x cheaper to encode in Rust; its JS decode cost is not comparable with the streaming figure for JSON (that figure excludes Tauri's own `JSON.parse`), and the inclusive microbenchmark has them level (149 vs 164 ns per event). Adopt `bin` only if real workloads (large tool outputs, diffs) show IPC volume to matter, and then take `bin`, **not MessagePack**: `@msgpack/msgpack` costs 236 ms per 200,000 events in the stream against 6 to 12 ms for `json-slim`/`json`, the only encoding with measurable main-thread time.
2. **Batching: 8 ms timer in the Rust bridge, plus one apply per animation frame in the webview.** Never send one message per event (3x CPU, worst tail); 4 ms buys nothing over 8; 16 and 33 ms add 5 and 13 ms of latency. Keep the 8 MiB unacknowledged-bytes limit and the ack coalesced to at most one per 30 ms.
3. **Markdown parses on the main thread**, incrementally: closed blocks once (`marked` or its M1 replacement), only the open block per frame, an unfinished fence as plain text, raw HTML escaped. Cost is 2 to 10 ms per second at 10k tokens/s, so a worker adds structured-clone cost and latency for nothing. Revisit a worker for Shiki highlighting and for any single open block above about 20 KB (parse cost is linear, roughly 35 ns per character; 100 KB would be 3.5 ms per frame).
4. **Virtualization: `@tanstack/solid-virtual`, rows are markdown blocks (not messages), overscan 6, dynamic measurement, estimate from text length; one list keyed by row index** (a `For` over index numbers so rows keep their DOM). 80,000 rows stayed at about 130 DOM nodes. Follow the tail until the user scrolls up, decided from wheel/key/pointer events, **not** from scroll distance. Overscan and estimate size were not tuned. Hidden threads should keep applying events to their model without a view (10 threads x 1k tokens/s: apply p99 1 ms per frame).
5. **Cursor and acknowledgement**: keep the "ack after the frame that applied the events was committed" design as built. Both ends dedupe `id <= last applied`: the webview model drops a replayed event before touching any state, and the bridge drops it before its corpus offset advances (fix round 1, with tests).
6. **Contract note** (change request CR-03): `MessageCompleted.text` repeats every delta; for this stream it is 12 MB, within reach of the 16 MiB wire limit. Separately, the daemon's 487 MB peak at session close comes from its lifecycle fold materializing the whole history, which dropping the repeated text does not fix.

## 5. Risks and what was not measured

- **Webview memory.** 1.05 to 1.39 GB footprint while streaming 10k events/s (not changed by the encoding) and about 250 MB after the stream for 3 M tokens (about 80 bytes per token), against PLAN §4's app budget of 150 MB idle RSS excluding agents (the Rust app process itself is 37 MB). The causes are unknown; section 3.3 lists hypotheses. M1 must first **profile** (JS allocations, the virtualizer's count-sized measurement arrays, cached HTML, native WebKit memory) and only then choose among the candidate remedies (a columnar batch decode instead of one object per event, rows without a signal each, dropping cached HTML far from the viewport, a retention policy that keeps the last N thousand rows and re-pages older history from the daemon).
- **The daemon is the weakest link under load**: 55 to 85 % of a core for 10k events/s plus the fake agents (one process write and one SQLite transaction per event). Under CPU contention it delivered 8.6 to 8.9k events/s and p99 delays of 0.3 to 1.3 s. Real agents emit far fewer events (tens to hundreds per second per thread), but the M0-05 ingestion batching follow-up is the first thing to do for headroom.
- **Not measured**: Linux / WebKitGTK (a Linux run must check: `rAF` cadence while streaming, that raw `Channel` messages and the fetch path behave, `footprint` equivalent via smem/`/proc`, and the same typing probe); 120 Hz panels; real OS key events (synthetic only: the input-latency budget is **not proven**); presentation latency (the latency metric stops at the next rAF callback, with no compositor or GPU evidence); real agent markdown (the corpus is synthetic, with a tool call every 4 s); chunk sizes above 4 characters (the bridge is not the limit there: fewer events carry the same bytes); more than one visible thread; reconnect while streaming (the M0-05 tests cover the client, not this bridge).
- **Environment**: results are from a shared interactive Mac (load average 3 to 11). Runs with evidence that the window was hidden were discarded (section 2); failures are logged, not discarded.
- `marked` renders links without sanitizing the scheme; M1 needs an allow-list. Block splitting breaks loose lists and quotes into several blocks (an ordered list restarts its numbering).

## 6. Escalation check

The brief says to escalate if WKWebView cannot hold 60 fps at 10k tokens/s with batching. It can, in every valid run (60.0 fps; one 79 ms frame in 3 x 60 s; 3 dropped frames in 5 minutes). No escalation.

## 7. Credit

Design ideas only, no code copied: the M0-05 report's client example (cursor rule), Tauri's `Channel` documentation. `marked`, `@msgpack/msgpack`, `@tanstack/solid-virtual` are MIT-licensed dependencies.

## 8. Fix round 1

Review: GPT-6.1 Sol (`docs/agents/reviews/M0-07-stream-perf.md` in the main checkout), 8 findings, all accepted.

| # | Finding | What changed | Evidence |
|---|---|---|---|
| 1 | Synthetic typing hid main-thread delay; "no dropped input" overclaimed | Keystrokes have absolute deadlines; latency counts from the deadline; timer lateness is reported; the verdict is now "streaming budget met, input-latency budget not proven"; OS-injected keys are listed as required | 3 new runs of the 10-thread scenario (section 3.2). Not done: OS key injection (needs Accessibility permission) |
| 2 | Replayed events applied twice (webview) and shifted corpus text (bridge) | `Transcript.apply` rejects `id <= lastId` before any mutation and returns false; the pipeline counts only applied events; the bridge's `Prep` drops duplicates before the corpus offset advances | vitest: 3 new tests (replay before delayed acks, lifecycle rows, replay after acks); cargo: 2 new tests. With the check disabled, 2 of the 3 new vitest tests fail (the third covers the no-replay case) and the cargo dedupe test fails |
| 3 | `bridge_stop` could deadlock once credits were exhausted | `shutdown()`: drain mode (keep reading and dropping events, ignore credits), StopThread under a 5 s deadline, then abort the pump and drop the connection; `bridge_stop` returns a report | cargo test with a real daemon and no acknowledgements: passes; fails (`timed_out: true`) with drain mode disabled |
| 4 | "Committed frame" / presentation overclaim | Metric renamed to "agent timestamp to next rAF after the update", described as a rendering proxy; presentation bound removed | Report, `bench.py` output |
| 5 | Validity policy could discard real failures | `classify()` separates evidenced environmental invalidation from failures; failures are logged and counted; visible-window stalls are results; disclosure of earlier discards in section 2 | `bench.py`; the nine retained invalid runs re-checked |
| 6 | Memory causes asserted | Reworded as hypotheses; profiling list in section 3.3 and Risks | |
| 7 | Unequal decode comparison | The 9x ratio is gone; the JSON recommendation rests on end-to-end data and simplicity | Section 4 |
| 8 | CR-02 misattributed the daemon peak | Renumbered CR-03, wire size and lifecycle-fold memory separated, versioned representation and reconstruction rule defined | `docs/agents/change-requests.md` |

**Re-measured after the code changes**: the 60 s single-thread and 10-thread scenarios (3 runs each). **Not re-run**: the encoding/batching sweep, the 5-minute memory run and the microbenchmarks; they were taken on the build before fix round 1. The changes do not touch encode, decode or batching; the dedupe adds one integer comparison per event and the stop path is not used while streaming. The memory run used the pre-fix typing-free single scenario, so it is unaffected by finding 1.
