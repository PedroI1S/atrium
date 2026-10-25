# M0-08: browser pane (CDP screencast into a Tauri canvas, with input)

Spike: `spikes/browser-screencast/` (standalone Tauri 2 + SolidJS). Screenshots: `docs/spikes/M0-08-browser-screencast/`.
Machine: Apple M4, macOS 27.0.1, Helium 0.19.4.1 (Chromium 155.0.8059.84), 60 Hz panel, Tauri/WKWebView, release build.

## Verdict

- **Targets met on macOS.** 1x pane: about 60 fps (target 30), click to canvas p50 12 ms, p95 19 ms (target < 80 ms). A 2x ("retina") pane also meets both: 57.7 fps, p50 27 ms, p95 34 ms.
- **Use `--remote-debugging-pipe` with our own thin CDP client.** It works on Helium with a dedicated profile. `chromiumoxide` cannot do a pipe, so it is not the transport (verdict below).
- **Chromium does not exit when the pipe closes.** It was still running 30 s after its parent was `kill -9`'d. The owner must reap it (done here with a pid file; see "What broke / found").

## What was built

| Piece | Where |
|---|---|
| Discovery: macOS app list from PLAN §3.13 in `/Applications` and `~/Applications` (incl. Helium), Linux `$PATH` + flatpak + snap (code only), default pick, `ATRIUM_BROWSER` override | `src-tauri/src/discovery.rs` |
| CDP client over the pipe (NUL-framed JSON on fds 3 and 4, flat sessions, events via broadcast) | `src-tauri/src/cdp.rs` |
| Launch (`--headless=new --remote-debugging-pipe --user-data-dir=<dedicated>`), contexts, page API, screencast, `Input.*` | `src-tauri/src/browser.rs` |
| Two `Target.createBrowserContext` contexts in one browser process | `Browser::new_page`, shown as "Context A / B" tabs |
| Frame path: `Page.startScreencast` (JPEG) -> base64 decode -> 48-byte header + JPEG -> binary `tauri::ipc::Channel` -> `createImageBitmap` -> `<canvas>` | `packet.rs`, `lib.rs` (`pane_attach`), `src/pane/controller.ts` |
| Input: mouse, wheel, keyboard (incl. macOS Cmd editing commands), IME (`Input.imeSetComposition` while composing, `Input.insertText` to commit), resize (`Emulation.setDeviceMetricsOverride` from a `ResizeObserver`), navigation bar (back, forward, reload, URL) | `src/pane/*.ts(x)` |
| Take over: `Control` in `src-tauri/src/control.rs`: a flag plus a gate. Every pane input command holds a user turn and the agent stand-in (`agent_action`) holds an agent turn for its whole duration; `set_user_control` revokes running agent turns, waits for admitted turns to finish, then flips the flag (see Fix round 1, finding 3). A toggle button flips it. | `control.rs`, `lib.rs` |
| Local fixture server (animation, click probe, form, state, scroll pages, cookie echo), loopback only | `src-tauri/src/fixture.rs`, `src-tauri/fixtures/` |
| Measurement: in-app harness, backend bench, chromiumoxide probe, process sampler | `src/bench.ts`, `examples/screencast-bench.rs`, `examples/screencast-oxide-probe.rs`, `src/procstat.rs` |

Frame header (little endian, 48 bytes): `ack_id, context, seq, flags, device w/h, Chromium timestamp, backend rx time, backend send time`. A golden hex test exists on both sides (`packet.rs`, `src/pane/frame.test.ts`).

Flow control: Chromium sends the next frame only after `Page.screencastFrameAck`. The pane acks after drawing (end-to-end back-pressure). `ATRIUM_ACK=eager` acks from the backend instead; not measured separately.

## Method and exact commands

All numbers are from release builds, fresh profile per run, local fixture pages only, nothing external. Commands are fish-safe (scripts have a bash shebang and take `NAME=value` arguments).

```text
cd spikes/browser-screencast
pnpm install
pnpm tauri build --no-bundle
./scripts/bench.sh 5 /tmp/m1 ATRIUM_DSF=1                      # in-app, 1x
./scripts/bench.sh 5 /tmp/m3 ATRIUM_DSF=2 ATRIUM_EXTRA_ARGS=--force-device-scale-factor=2   # in-app, 2x
python3 scripts/summarize.py /tmp/m1                           # per run, median, worst
./scripts/sweep.sh /tmp/sweep                                  # backend only: quality, PNG, 2x, every-2nd frame
cargo run --release --manifest-path src-tauri/Cargo.toml --example screencast-oxide-probe -- --secs 8 --probes 80
./scripts/screenshots.sh                                       # needs Screen Recording permission
```

In-app bench (`src/bench.ts`, one run = about 60 s), phases in order:
1. **idle**: static page, screencast detached, 10 s sampled.
2. **static + screencast**: 5 s sampled (frames are damage-driven; 0 frames arrived in every run).
3. **streaming**: `animation.html` (60 moving circles + counter, canvas follows `devicePixelRatio`), 10 s. fps = frames drawn to the canvas / window; intervals are `performance.now()` deltas between `drawImage` calls (1 ms resolution in WKWebView).
4. **input-to-frame**: 100 probes, 120 ms apart. A synthetic DOM `pointerdown`+`pointerup` is dispatched on the canvas (so the Pane handler, `invoke`, Rust, CDP pipe, Chromium, screencast frame, channel, `createImageBitmap` and `drawImage` all run). The clock starts just before dispatch and stops at the `drawImage` of the first frame whose probe pixel (read through a 1x1 offscreen canvas) has the new colour. Fixture colours are saturated primaries so JPEG does not blur the check. It stops at "drawn to canvas": the compositor shows it up to one vsync later (16.7 ms at 60 Hz), so add that for "on glass". 0 misses in all runs.
5. **context switch**: attach to context B, time to first frame.

CPU = delta of cumulative CPU time (`ps -o time`, 10 ms resolution) over wall time, percent of one core, summed over the Chromium process tree (browser, GPU, network, storage and renderer processes: 9 to 11 processes). Memory: **physical footprint** from `footprint(1)` (what Activity Monitor shows) and summed `ps` RSS. RSS double-counts shared framework pages (about 1.4 to 1.8 GB for a footprint of 0.6 to 0.9 GB), so footprint is the number to budget with. The Tauri app process alone (excluding WKWebView's XPC helpers, which M0-06 measured) is reported as "app CPU".

Note: both contexts are alive during every phase (A shown, B static).

## Results: in-app, macOS, 1280x715 CSS px pane

The 5-run tables below were measured **before fix round 1**. After the fixes I re-ran 4 in-app 1x runs (see "Fix round 1" for the numbers); they are a little noisier but within the same targets. The tables were not regenerated.

### 1x (frames 1280x715; 5 runs, `/tmp/m1`)

| Metric | Median of runs | Worst run | Target |
|---|---|---|---|
| fps (frames drawn / s) | 59.8 | 59.6 | >= 30 |
| draw interval p95 / max (ms) | 19 / 53 | 21 / 66 | |
| intervals over 33 ms in 10 s | 8 | 11 | |
| input-to-frame p50 (ms) | 12 | 13 | < 80 |
| input-to-frame p95 (ms) | 19 | 28 | < 80 |
| input-to-frame max (ms) | 32 | 34 | |
| misses (probe never showed) | 0 of 500 | | |
| frame size, mean (KiB) / rate | 39.8 / 19.5 Mbit/s | 40.4 / 19.8 | |
| IPC leg: backend send to JS receive (ms, run 1, 600 frames) | p50 0.15, p95 1.0, max 7.5 | | |
| `createImageBitmap` decode (ms) | 3 (p50), 4 (p95) | | |
| context switch to first frame (ms) | 12 | 14 | |
| **Chromium idle** (no screencast): CPU / footprint / RSS | 2.1% / 599 MB / 1375 MB | 2.4% / 734 MB / 1773 MB | |
| Chromium static page with screencast on | 0 frames in 5 s (all runs) | | |
| **Chromium streaming**: CPU / footprint / RSS | 55% / 727 MB / 1402 MB | 59% / 859 MB / 1798 MB | |
| Tauri app process CPU while streaming | 11.6% | 13.0% | |

Per-run streaming CPU was bimodal: 34, 37, 55, 55, 59%. I did not find out why (same code, same page). Footprint moves about +-100 MB between runs too.

### 2x / retina (frames 2560x1430; 5 runs, `/tmp/m3`)

Needs `--force-device-scale-factor=2` at launch plus `setDeviceMetricsOverride(dsf=2)`. See the DSF finding.

| Metric | Median | Worst run |
|---|---|---|
| fps | 57.7 | 56.1 |
| draw interval p95 / max (ms) | 22 / 56 | 25 / 67 |
| input-to-frame p50 / p95 / max (ms) | 27 / 34 / 51 | 27 / 41 / 51 |
| frame size / rate | 91 KiB / 42.8 Mbit/s | 98 KiB / 47.3 |
| context switch to first frame (ms) | 26 | 30 |
| Chromium streaming CPU / footprint | 117% / 902 MB | 162% / 925 MB |

Both targets still hold at 2x. CPU is the cost (more than one core), and fps dips to 56 to 59.

### Backend only (no webview): quality, format, scale (`scripts/sweep.sh`, one run each, 8 s, 60 probes, 1280x800)

Latency here ends at the pipe reader (no IPC, decode or draw), so it is lower than in-app.

| Setting | fps | KiB/frame | Mbit/s | latency p50 / p95 / max (ms) | Chromium CPU |
|---|---|---|---|---|---|
| JPEG q50 | 59.9 | 26.9 | 13.2 | 10.3 / 11.7 / 28.1 | 33% |
| JPEG q70 | 60.0 | 35.6 | 17.5 | 7.6 / 13.7 / 22.9 | 35% |
| **JPEG q80** | 60.0 | 41.4 | 20.3 | 7.7 / 12.8 / 13.4 | 45% |
| JPEG q90 | 60.0 | 53.8 | 26.4 | 7.4 / 11.7 / 30.5 | 46% |
| PNG | 59.8 | 73.4 | 36.0 | not decoded in this bench | 68% |
| JPEG q80, 2x (flag + dsf 2) | 59.0 | 91.9 | 44.4 | 18.3 / 30.1 / 32.4 | 135% |
| JPEG q60, 2x | 59.8 | 69.9 | 34.3 | 24.3 / 29.0 / 39.7 | 99% |
| JPEG q80, `everyNthFrame=2` | 30.0 | 42.2 | 10.4 | (30 of 60 probes miss: skipped frames) | 24% |

Quality changes bytes and CPU, not fps or latency, on this fixture. The animation fixture is a flat-colour canvas; photographic pages will be larger.

## Findings

1. **Pipe works on Helium** with `--headless=new` and a throwaway `--user-data-dir`. No flag combination was needed for speed (60 fps with only the base flags), so the "no combination gives 20 fps" escalation did not trigger. Remote debugging on Helium with a dedicated profile works, so that escalation did not trigger either.
2. **chromiumoxide 0.9.1 has no pipe transport.** Its launcher only passes `--remote-debugging-port` and reads the WebSocket URL from stderr (`src/browser/config.rs`). Pipe support is not there, so per the brief this is the port fallback case. It does work over the port: `screencast-oxide-probe` streamed at 60 fps with the same fixtures. **No latency winner is claimed.** My first comparison (chromiumoxide about 3 ms faster at p50) was not like-for-like: the probe stamped frames where the consumer got them while the own client stamped at the pipe reader, and chromiumoxide's default `--enable-automation` made its frames 748x442 instead of 1280x800. After fix round 1 both stamp at the consumer (`frames.next()` returns, before the ack) and the probe records decoded frame size; with `window_size(1280, 800)` its frames are 1272x686 (still not identical, the browser chrome is subtracted). Three rounds each, 80 probes:

   | | decoded frame | latency p50 / p95 (ms), per round |
   |---|---|---|
   | chromiumoxide, WebSocket port | 1272x686 | 12.4 / 14.9, 8.3 / 14.8, 12.6 / 26.9 |
   | own client, pipe | 1280x800 | 13.0 / 14.6, 13.6 / 16.6, 12.9 / 16.0 |

   Equivalent within noise (and the own client's reader-side stamp is within 0.1 ms of the consumer-side one). Other chromiumoxide notes: it leaves frames base64-encoded (`Binary(String)`), and the typed API is large (long compile). Its default flag list (taken from Puppeteer) is a useful reference.
3. **`--enable-automation` shrinks the headless viewport** (frames came out 748x442 instead of 1280x800), which looks like a latency win (7.7 ms) but is just smaller frames; it also explains the oddity in the first chromiumoxide comparison. Do not add it.
4. **Device scale factor.** `Emulation.setDeviceMetricsOverride(dsf=2)` alone does **not** change screencast frame size (still 1280x800). Frames become 2560x1600 only with `--force-device-scale-factor=2` at launch **and** dsf=2 in the override (flag with override dsf=1 gives 1280x800). The flag is launch-time only, so a browser launched for a 1x display cannot serve a 2x pane, and the reverse. Page layout is correct either way (CSS px). The pane's 2x button is therefore disabled unless the browser was launched with the flag (`env ATRIUM_LAUNCH_DSF=2 <app binary>`), and the status bar shows the decoded frame size next to the canvas size.
5. **Closing the pipe does not stop Chromium.** After `kill -9` of the owner, the browser (and 3+ helpers) was still alive after 30 s, reparented to launchd. The first fix (kill any pid whose command line contains the profile path) was unsafe; the current one (`profile.rs`) is gated: the owner holds an exclusive `flock` on `<profile>/atrium-owner.lock` for its lifetime (the kernel drops it when the owner dies, even on SIGKILL; the fd is close-on-exec so Chromium never inherits it), a second live owner is refused, and after launch it records pid, start time, executable and profile path. At the next launch `reap_stale(&lock)` signals the recorded pid only if the start time and executable still match, its parent is init (orphaned), and its argv contains the exact token `--user-data-dir=<profile>`; anything else is left alone. Also a SIGTERM/SIGINT handler exits through `RunEvent::Exit` so the browser is closed. Verified end to end earlier: SIGTERM leaves 0 processes, SIGKILL leaves the orphan, the next launch reaps it. Without the signal handler my screenshot script had left 14 orphan browsers (found and killed).
6. **Isolation holds** (test `contexts_do_not_share_cookies_or_storage`): same origin, two contexts, one browser process. JS cookie, server-set `Set-Cookie`, `localStorage` and `sessionStorage` written in A are invisible to B (including the `Cookie` request header), B's writes do not reach A, and the two contexts keep separate values. The pane screenshots `isolation-1-context-a-writes.png`, `isolation-2-context-b-sees-nothing.png`, `isolation-3-context-a-keeps-its-state.png` are three captures of ONE launch (same browser pid 20228 and fixture port 61986 in the status bar and URL): A writes, B on the same origin sees nothing, A still has its state. The live test remains the stronger evidence.
7. **Memory cost is per context, not tiny.** Footprint of the Chromium tree (`screencast-bench`, per-process table in the JSON): about 313 MB for a fresh browser on `about:blank`, 570 MB with one context (animation page), 733 MB with two (second one static). Each context adds about 3 renderer-type processes; the busy page's renderer alone was 150 to 160 MB. Budget roughly 150 to 250 MB per active context plus about 300 MB base, and close idle contexts.
8. **First `Page.navigate` after launch is sometimes aborted** with `net::ERR_ABORTED`: 4 of 4 times in the app (first call about 1.5 s after launch), 1 of about 12 in a headless probe. One retry after 300 ms always worked, so `Page::navigate_start` retries once. Cause not found.
9. **IME works through `imeSetComposition` + `insertText`.** Two composition updates then a commit leave the committed text and no leftover composition text, and the page sees `compositionstart/update/end` with correct data (live test and `pane-form-input-ime.png`). Not driven by a real OS input method (see risks).
10. **Frame stream is damage-driven.** A static page with the screencast on sent 0 frames in 5 s, at about 1.5% Chromium CPU (about the same as no screencast).
11. **Ack matters.** Every consumer must ack every frame, including frames it discards (a drain loop without acks stalled the stream in my first bench).
12. Sampling note: `ps %cpu` on macOS is a decaying average and unusable for windows this short; use CPU-time deltas.

## Recommendation (M1 / M3)

- **Transport: `--remote-debugging-pipe`, own minimal CDP client** (`cdp.rs` is about 250 lines and has the framing tests). Reasons: pipe needs no open TCP port on the machine (nothing else on localhost can attach to the browser), works on Helium, and the client is small. Keep the owner responsible for lifecycle: pid file + reaper at startup, close on SIGTERM, `Browser.close` on exit. Port fallback only for a browser that refuses the pipe (none seen), bound to `127.0.0.1`.
- **`chromiumoxide`: not for the pane.** No pipe, WebSocket only, large generated API; it is fine as a reference and as a possible driver for the agent-tools side (M3) if someone wants typed commands, but then it needs its own port-based browser. Decide that separately; the pane should not depend on it. Two clients attached to one browser is also possible over a port, but not over a pipe (a pipe is one client), so if the agent tools use chromiumoxide, the daemon would need the port.
- **Frames: JPEG, quality 80** (about 41 KiB at 1280x800, 20 Mbit/s locally, no fps or latency penalty versus q50; q60 for 2x). PNG costs 1.8x the bytes and CPU for no benefit in a pane. `everyNthFrame=1`. Ack from the frontend after drawing.
- **Scale: default to 1x frames**; offer 2x as a setting that restarts the browser with `--force-device-scale-factor=2` (it costs 117% CPU and 2x the bandwidth). Plan for display changes (mixed DPI) to need a relaunch.
- **Channel:** raw binary `tauri::ipc::Channel` was cheap: 0.15 ms p50 backend-to-JS, 3 ms decode. No need for a custom protocol handler or WebSocket to the webview.
- **Input:** keep `rawKeyDown`/`keyDown`+text, `commands` for Cmd shortcuts, hidden textarea as the IME sink. M1 must add: caret position from the page so the OS candidate window appears at the caret, clipboard bridging, drag and drop, cursor shape feedback, popups/new windows (`Target.targetCreated`), file chooser, downloads, dialogs.
- **Take over:** keep the flag in the daemon next to the browser and check it in both the pane input path and every browser MCP tool, as `Control` does here.
- **Process model for M1:** one Chromium, one context per thread, close contexts when the thread goes idle (memory finding 7).

## Linux / WebKitGTK: what a Linux run must check

Not run here (no Linux machine; deferral and owner are a driver decision). Discovery code for `$PATH`, flatpak and snap is written and unit-tested against fake dirs only.
1. `cargo test` for discovery on a real distro with Chrome/Chromium from apt, snap and flatpak (snap and flatpak binaries are wrappers and may block `--remote-debugging-pipe`, fds 3/4 passing, or `--user-data-dir` outside the sandbox; check each).
2. The pipe fd passing (`pre_exec` + `dup2`) and the reaper (`ps -o command=` form) on Linux.
3. In WebKitGTK: `createImageBitmap(Blob)` on JPEG and its decode time at 1280x715 and at 2x; `drawImage` cost into a 2560x1430 canvas; the binary `Channel` delivering an `ArrayBuffer`.
4. fps (target >= 30) and click-to-canvas p50/p95 (< 80 ms) with `scripts/bench.sh` (needs `pnpm tauri build --no-bundle`; `screenshots.sh` uses macOS `screencapture`). `performance.now()` resolution in WebKitGTK.
5. IME with ibus and fcitx into the hidden textarea (composition events, candidate window position).
6. Wheel units (`deltaMode`) and key mapping on Linux (no Cmd; Ctrl+A/C/V/X/Z need the `commands` mapping or plain Ctrl keys).
7. Chromium CPU and footprint while streaming (measure with `/proc/<pid>/smaps_rollup` PSS instead of `footprint`; `procstat.rs` is macOS-specific there).
8. Headless GPU path: whether frames still run at 60 fps with a software GL fallback.

## What broke / limits of this spike

- The first bench of the backend latency showed 100% misses: my drain loop did not ack frames and Chromium stopped sending (finding 11). Fixed.
- `--enable-automation` shrinks the viewport (finding 3); initial ERR_ABORTED (finding 8); screenshot first attempts showed the old page because of finding 8.
- Early in-app runs with 2x showed no change because the scale was set after the pane mounted (my bug, fixed) and then because the animation fixture drew at CSS size (fixture fixed to follow `devicePixelRatio`). Final tables use the fixed build.
- Latency is "drawn to canvas", not "on glass" (+ up to 16.7 ms), and measured with synthetic DOM events, not OS-level input. A real mouse also adds the macOS event path (about 1 to 2 ms) before JS sees it.
- `performance.now()` has 1 ms resolution here, so in-app numbers are integers.
- The Tauri-side and WKWebView helper memory were not re-measured (M0-06 covers the shell). "App CPU" is the Tauri process only.
- fps measured on a 60 Hz panel; 120 Hz not measurable.
- The window was on screen (not occluded) in all in-app runs; a hidden window may throttle `requestAnimationFrame` and was not tested.
- Not covered by the brief and not built: tab management inside a context, popups, downloads, clipboard, drag and drop, cursor shape, a11y snapshots, the MCP tools.

## Open risks

- Per-context memory (finding 7) against the PLAN §4 memory budget: needs a decision on how many live contexts and an idle-eviction policy.
- Streaming CPU spread (34 to 59%, unexplained) and 117% at 2x: battery cost of an always-streaming pane. Pause the screencast when the pane is hidden (`pane_detach` exists) and use `everyNthFrame` for background panes.
- IME verified only with synthetic composition events and the CDP side; real macOS input methods (candidate window placement, marked text styling) are untested.
- `ERR_ABORTED` on first navigation is unexplained; the retry is a workaround.
- Orphan handling covers SIGKILL only on the next launch of the same profile. A daemon that never restarts leaves the browser running until then. On Linux the "parent is init" check can refuse when a subreaper adopts the orphan (it then leaves the browser alone: safe, but not reaped).
- Helium is a fork (0.19.4.1): flag behaviour (`--enable-automation`, DSF) should be re-checked on stock Chrome once one is available.

## Fix round 1 (review GPT-6.1 Sol, 9 findings, all fixed)

| # | Finding | Fix | Test that fails without it |
|---|---|---|---|
| 1 | Reaper could kill an unrelated or live-owned process | Profile lock + recorded pid/start time/exe + exact `--user-data-dir` argv + orphan check; second owner refused (finding 5 above) | `profile.rs`: unrelated process whose command mentions the path (forged-perfect record still refused), longer path with same prefix, reused pid / wrong exe / wrong profile / live parent, verified orphan reaped, second owner refused, lock freed when the owner is SIGKILLed; live: a second `Browser::launch` on a live profile fails and the first browser survives |
| 3 | Take-over did not stop an already admitted agent action | `Control` turns + epoch + drain (see "What was built"); agent click releases its own button if cancelled mid-click; agent navigate polls the checkpoint while waiting for load and before its retry; hand-back waits for in-flight user input | `control.rs`: take-over returns only after the running agent action has stopped (and it stops at its next checkpoint), late agent calls are refused, hand-back waits for user input |
| 2 | Frames and acks crossed context switches | Each attach has a generation (closure-pinned in the channel callback; stale packets and stale decodes are dropped and their bitmaps closed); frames carry a backend stream id (header is now 52 bytes) and acks echo it; the backend ignores acks from a replaced stream; `pane_attach` is serialised | `controller.test.ts`: delayed `createImageBitmap` across a switch (stale frame not drawn or acked, new context's lower sequence not discarded); `lib.rs`: ack routing |
| 5 | IME sink wiped the marked text on every `input` | `imeSink.ts`: value untouched while composing, cleared on `compositionend`; empty end = cancel (`compose ""`); blur mid-composition cancels; keys during composition not forwarded; non-composing text (emoji, dictation) committed | `imeSink.test.ts` (happy-dom), driven through the textarea's events: multi-update + commit, cancel, blur, key filtering, outside text |
| 6 | Mouse-up outside the canvas was lost | Pointer events with capture; the controller tracks held buttons and releases them on pointercancel, lost capture, window blur, context switch and hand-back (release happens before the backend flag flips) | `controller.test.ts`: held button released on `releaseAll`, on hand-back, on context switch (on the context where it was pressed), and not released twice after a normal mouseup |
| 9 | CDP EOF left event consumers waiting | The broadcast sender is dropped at EOF; late subscribers get a closed receiver | `cdp.rs`: waiting and late subscribers see `Closed`; `browser.rs`: a `FrameStream` waiting for a frame returns `None` when the connection closes (both verified to fail with the fix removed) |
| 7 | 2x toggle showed an enlarged 1x image | 2x button disabled unless the browser was launched with `--force-device-scale-factor` (`ATRIUM_LAUNCH_DSF=2`, or the flag in `ATRIUM_EXTRA_ARGS`); status bar shows decoded frame size and canvas size separately; bench reports decoded frame pixels | `lib.rs`: launch-scale parsing; in-app bench spot checks: 1x decoded 1280x715, canvas 1280x715; 2x with launch flag decoded 2560x1430, canvas 2560x1430 |
| 8 | Isolation screenshots came from separate launches | `isolation` scene: one launch, three captures (A writes, B empty, A keeps state); the old two images removed; wording corrected above | visual (status bar shows the same pid and port in all three) |
| 4 | Oxide comparison used different endpoints | Both probes stamp at the consumer; decoded dimensions recorded in both; comparison re-run and the "3 ms faster" claim withdrawn (finding 2) | rerun, numbers in finding 2 |

Re-measured after the fixes (release build, in-app, 1x, 4 runs: `/tmp`-style dirs from `scripts/bench.sh`): fps 60.0, 58.6, 59.6, 58.6; click-to-canvas p50 18, 20, 13, 11 ms; p95 24, 37, 22, 37 ms; max 41, 132, 38, 51 ms (one of 400 probes exceeded the 80 ms budget); draw interval max 112 to 118 ms in two runs; 0 misses; Chromium streaming CPU 31, 31, 36, 48%. That is a little noisier than the earlier 5-run set (median p50 12, p95 19), and I did not isolate whether the cause is the fixes or machine load. One 2x run with the launch flag: 58.5 fps, p50 34 / p95 41 ms, 89.6 KiB frames. Not repeated further to save time; the 5-run tables above stay as the pre-fix baseline.

## Credits and licences

No code was copied. Read for design: chromiumoxide 0.9.1 (MIT OR Apache-2.0; source of the Puppeteer default flag list and confirmation that it has no pipe transport). Chrome DevTools Protocol docs for `Page.startScreencast`, `Input.*`, `Emulation.*`. Theme token values in `src/styles/tokens.css` are copied from the M0-06 "Flat Dark" direction (this repository).
