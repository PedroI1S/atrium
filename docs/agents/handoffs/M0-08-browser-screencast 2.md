# Handoff: M0-08-browser-screencast

Model: Sonnet 5.5 · Status: done

Report: [docs/spikes/M0-08-browser-screencast.md](../../spikes/M0-08-browser-screencast.md) · Screenshots: `docs/spikes/M0-08-browser-screencast/` · Branch `agent/sonnet/M0-08-browser-screencast`

## Done
- Tauri 2 + SolidJS spike in `spikes/browser-screencast/`: Helium discovery (macOS list from PLAN §3.13, Linux code-only), one headless Chromium over `--remote-debugging-pipe` with a dedicated profile, two `Target.createBrowserContext` contexts, screencast (JPEG) over a binary `tauri::ipc::Channel` into a canvas, mouse / wheel / keyboard / IME forwarding, resize via `Emulation.setDeviceMetricsOverride`, navigation bar, context tabs, and a take-over flag (`control.rs`) that gates pane input and the agent stand-in.
- Own ~250-line CDP pipe client. `chromiumoxide` 0.9.1 cannot use a pipe (WebSocket only); a probe example measured it over the port for the verdict.
- Measured on macOS (Apple M4, Helium 155, release build, 5 runs each):
  - 1x: 59.8 fps (worst run 59.6), click to canvas p50 12 ms / p95 19 ms (worst run p95 28 ms), 0 misses in 500 probes.
  - 2x: 57.7 fps, p50 27 ms / p95 34 ms.
  - Chromium idle 2.1% CPU, footprint 599 MB (RSS 1375 MB, over-counts); streaming 1x 55% CPU (34 to 59% across runs), footprint 727 MB; 2x 117% CPU.
  - All targets met. Method and commands in the report.
- Findings that matter for M1: Chromium does not exit on pipe EOF (gated reaper + profile lock + signal handler added and verified); screencast frame size ignores emulated DSF unless the browser is launched with `--force-device-scale-factor`; per-context memory cost is 150 to 250 MB; `--enable-automation` shrinks the viewport; first navigate sometimes returns `ERR_ABORTED` (retry works, cause unknown).
- 5 screenshots (agent driving, typing + IME, and three isolation captures from one launch).

## Not done
- Linux / WebKitGTK not run; the report has the exact checklist (deferral and owner are a driver decision).
- Real OS input-method IME not driven (synthetic composition through CDP only); no clipboard, drag and drop, cursor shape, popups, downloads, a11y snapshots, MCP tools (out of scope).
- Latency is "drawn to canvas" with synthetic DOM events, not on-glass with real mouse input.
- Unexplained: bimodal streaming CPU, the `ERR_ABORTED` cause.

## Definition of Done results
- `pnpm typecheck && pnpm test` (in `spikes/browser-screencast`): `tsc --noEmit` clean; vitest 3 files, 14 tests passed.
- `cargo clippy --manifest-path spikes/browser-screencast/src-tauri/Cargo.toml -- -D warnings`: pass (also with `--all-targets`).
- `cargo test --manifest-path spikes/browser-screencast/src-tauri/Cargo.toml -- --test-threads=1`: 17 unit tests (discovery against fake app dirs, CDP framing, packet golden, take-over flag, procstat parsing, stale-browser reaper) and 3 live tests against Helium (context isolation, screencast + click reaching the page, keyboard + IME + wheel + resize) passed. Live tests skip with a message if no browser is installed.
- Measurements: report sections "Results".
- Screenshot of the pane showing a page: `docs/spikes/M0-08-browser-screencast/pane-form-input-ime.png` (and three more).
- Spike report with the M1/M3 recommendation (pipe vs port, frame format/quality, chromiumoxide verdict): written.
- All Chromium processes I started are closed (checked by profile dir); the Helium with automation flags (pid 78844) belongs to the Playwright MCP, not this task, and was left alone.

## Gotchas
- Scripts and commands use a bash shebang and `NAME=value` arguments (fish-safe); my shell tool is zsh, so `$a`-style word splitting bit me once in an ad-hoc loop, not in the committed scripts.
- `pnpm tauri build --no-bundle` is needed before `scripts/bench.sh` and `scripts/screenshots.sh`; screenshots need Screen Recording permission.
- `ATRIUM_DATA_DIR` picks the profile location (default: the Tauri app data dir); always set it for runs so the profile is throwaway.
- Always ack screencast frames, even discarded ones, or the stream stops.
- `cargo test` live tests launch real headless browsers (about 3 s each); run with `--test-threads=1` to keep CPU noise down.

## Follow-ups
- Orchestrator: decide the live-context memory budget and eviction policy; decide whether the browser MCP tools go through chromiumoxide (needs a port) or the same pipe client.
- Someone with a Linux machine runs the report's checklist; record owner and gate in `decisions.md`.
- Re-check DSF and `--enable-automation` behaviour on stock Chrome.

## Fix round 1 (review GPT-6.1 Sol, 9 findings, all fixed)
Details and per-finding tests in "Fix round 1" of the report. Status stays **done**.

- **#1** profile `flock` for the owner's lifetime, recorded pid + start time + exe, reaper only signals an orphan whose argv has the exact `--user-data-dir=<profile>`; a second live owner is refused. Tests: unrelated process mentioning the path, longer prefix path, reused pid / wrong exe / live parent, verified orphan, second owner (unit and live).
- **#3** `Control` turns: an agent action holds a turn and checks `checkpoint()` between CDP commands; take-over revokes, waits for admitted turns, then flips; hand-back waits for in-flight user input. Tests in `control.rs`.
- **#2** per-attach generation in the pane, stream id in the frame header (now 52 bytes, golden test updated both sides), acks echo it and stale ones are ignored. Test with a delayed decoder.
- **#5** IME sink keeps the marked text until `compositionend`; cancel and blur cancel on the page. Tests drive the textarea (happy-dom added as a dev dependency).
- **#6** pointer capture and held-button release on cancel, blur, context switch, hand-back. Tests in `controller.test.ts`.
- **#9** CDP EOF closes event subscribers; `FrameStream` returns `None`. Both tests verified to fail without the fix.
- **#7** 2x button disabled unless launched with `--force-device-scale-factor` (`env ATRIUM_LAUNCH_DSF=2 <binary>`); decoded frame size shown separately. **#8** isolation screenshots from one launch, wording corrected. **#4** both probes now stamp at the consumer and record decoded size; the "chromiumoxide 3 ms faster" claim is withdrawn (it was an artifact of 748x442 frames and different timestamps).
- Re-measured only what the fixes touch: 4 in-app 1x runs (fps 58.6 to 60.0, p50 11 to 20 ms, p95 22 to 37 ms, one probe of 400 at 132 ms) and one 2x run (58.5 fps, p50 34 / p95 41 ms). Slightly noisier than the 5-run baseline, cause not isolated; the baseline tables were not regenerated.
- DoD again: `pnpm typecheck` clean; `pnpm test` 5 files, 25 tests; clippy `-D warnings` (also `--all-targets`) clean; `cargo test -- --test-threads=1`: 29 unit + 4 live tests pass. No Chromium or app process left running (checked by profile dir).
