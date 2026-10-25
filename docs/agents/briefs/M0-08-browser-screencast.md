# M0-08-browser-screencast: CDP screencast into a Tauri canvas, with input

Model: Sonnet 5.5 · Reviewer: GPT-6.1 Sol · Depends on: none (starts when a Sonnet slot frees) · Wave 2
Branch: `agent/sonnet/M0-08-browser-screencast` · Worktree: `../atrium-wt/M0-08-browser-screencast`

## Goal
Prove the browser pane in PLAN §3.13: drive an installed Chromium over CDP, show its screencast in a Tauri canvas, forward input, and keep two isolated browser contexts in one browser process. Measure it.

## Context
- PLAN §3.13 (discovery, process model, pane), §4 (≥ 30 fps screencast, < 80 ms input-to-frame locally), §6 (Chrome blocks remote debugging on the default profile → always a dedicated `--user-data-dir`).
- This machine has **Helium** (`/Applications/Helium.app`, Chromium-based) and no Chrome. Discovery must find it; the spike runs against it.

## Owned paths
- `spikes/browser-screencast/**` (standalone Tauri + Solid project)
- `docs/spikes/M0-08-browser-screencast.md`, `docs/spikes/M0-08-browser-screencast/` (screenshots)
- `docs/agents/handoffs/M0-08-browser-screencast.md`

## Contract to implement (do not modify)
- **Discovery**: the macOS app list from PLAN §3.13 (including Helium), in `/Applications` and `~/Applications`. The Linux list is code-only and untested here.
- **Process**: one Chromium owned by the Tauri backend, with `--headless=new`, a dedicated `--user-data-dir` under the spike's data dir, and `--remote-debugging-pipe` (try it first; if `chromiumoxide` can't use a pipe, fall back to a port bound to `127.0.0.1` and report why).
- **Contexts**: two `Target.createBrowserContext` contexts. Show that cookies and localStorage don't leak between them (a test against a local fixture page).
- **Pane**: `Page.startScreencast` (JPEG) → binary `tauri::ipc::Channel` → `createImageBitmap` → `<canvas>`. Mouse, wheel, keyboard and IME (`Input.imeSetComposition` / `insertText`) are forwarded. Resizing uses `Emulation.setDeviceMetricsOverride`. Add a navigation bar.
- **Take over**: a toggle that pauses agent input and gives the user control. A dummy is fine; just show where the control flag lives.

## Definition of Done
- [ ] Measured on macOS, method and numbers in the report:
  - screencast fps on a local animation page (target ≥ 30);
  - input-to-frame latency (click → first frame showing the change; target < 80 ms), p50/p95;
  - CPU and RSS of the Chromium process while idle and while streaming.
- [ ] Tests: discovery against fake app dirs, and context isolation.
- [ ] `pnpm typecheck && pnpm test` and `cargo clippy --manifest-path spikes/browser-screencast/src-tauri/Cargo.toml -- -D warnings` passing.
- [ ] A screenshot of the pane showing a page, in `docs/spikes/M0-08-browser-screencast/`.
- [ ] Spike report with the M1/M3 recommendation (pipe vs port, frame format/quality, the `chromiumoxide` verdict).
- [ ] Handoff written.

## Out of scope
The browser MCP tools, a11y snapshots, the daemon (the backend owns Chromium in-process for this spike), Linux runs.

## Escalate if
- No Chromium flag combination gives ≥ 20 fps. Report the numbers and stop.
- Remote debugging can't be enabled on Helium with a dedicated profile.
