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
- Findings that matter for M1: Chromium does not exit on pipe EOF (reaper + signal handler added and verified); screencast frame size ignores emulated DSF unless the browser is launched with `--force-device-scale-factor`; per-context memory cost is 150 to 250 MB; `--enable-automation` shrinks the viewport; first navigate sometimes returns `ERR_ABORTED` (retry works, cause unknown).
- 4 screenshots (agent driving, typing + IME, context A state, context B empty).

## Not done
- Linux / WebKitGTK not run; the report has the exact checklist (deferral and owner are a driver decision).
- Real OS input-method IME not driven (synthetic composition through CDP only); no clipboard, drag and drop, cursor shape, popups, downloads, a11y snapshots, MCP tools (out of scope).
- Latency is "drawn to canvas" with synthetic DOM events, not on-glass with real mouse input.
- Unexplained: bimodal streaming CPU, the ~3 ms p50 gap between chromiumoxide (port) and the own pipe client, the `ERR_ABORTED` cause.

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
