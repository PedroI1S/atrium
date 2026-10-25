# Handoff: M0-06-ui-shell

Model: Sonnet 5.5 · Status: done

Report: [docs/spikes/M0-06-ui-shell.md](../../spikes/M0-06-ui-shell.md) · Screenshots: `docs/spikes/M0-06-ui-shell/` · Branch `agent/sonnet/M0-06-ui-shell`

## Done
- Tauri 2 + SolidJS shell in `spikes/ui-shell/` with overlay title bar, traffic lights inline with the tab strip, native vibrancy (`window-vibrancy`) plus `backdrop-filter`. Overlay + vibrancy work together on macOS 27.0.1 (no escalation); `transparent` needs `macOSPrivateApi`.
- Layout: sidebar (Inbox badge, Home, workspaces to threads), draggable tab strip (solid-dnd), Corvu split panes (chat + file preview, two threads side by side), right panel tabs (Review / Diff / Browser / Terminal / Database), composer, 5,000-message virtualized transcript, Kobalte palette with `⌘K` and `⌘J` wired to dummy actions, presets Default / Review / Focus.
- Token pipeline (`src/theme/`): TOML loader with 3-layer reference resolution, cycle and missing-reference errors, required-set validation, WCAG AA check, runtime swap without reload, VS Code importer (Catppuccin Mocha, mapping table in the report). 65 tests, including the CI-style check on every shipped theme.
- Three directions (Glass, Flat, Paper), each dark and light, plus the imported theme; all live-switchable. `prefers-reduced-motion` disables transitions and animations.
- 16 screenshots (every direction dark + light, palette x3, split panes x2, presets, right diff, home), taken with `screencapture -l <window id>` (permission worked; `scripts/screenshots.sh`).
- Measurements on macOS with method and exact commands (`scripts/measure.py`). CR-01 appended to `docs/agents/change-requests.md` (shadow, scrim, `--syntax-*`, `--glass-blur`); the spike used component-token stopgaps.

## Not done
- Linux / WebKitGTK is not measured; the report lists what a Linux run must check.
- Real mouse-drag of panes and tabs was exercised in Chrome only, not inside WKWebView; the in-app frame benchmarks drive the same state path programmatically.
- 120 Hz not measurable (60 Hz panel).
- Shiki, CodeMirror and `terminal.ansi*` mapping (a tiny regex highlighter stands in; out of scope per the brief).
- Keyboard-only and screen-reader checks beyond basics; no direction chosen (Pedro's call).

## Definition of Done results
- `pnpm install && pnpm typecheck && pnpm test`: pass (`tsc --noEmit` clean; vitest 1 file, 65 tests passed).
- `cargo clippy --manifest-path spikes/ui-shell/src-tauri/Cargo.toml -- -D warnings`: pass.
- `pnpm tauri build --debug`: produces `src-tauri/target/debug/bundle/macos/Atrium UI Shell.app`; launched with `ATRIUM_BENCH=startup` and it reported first paint and a handled keystroke.
- Screenshots: all captured (list above), `docs/spikes/M0-06-ui-shell/`.
- Measured (release build, Apple M4, macOS 27.0.1, 12 runs each):
  - Time to interactive (synthetic keystroke): median 392 ms Glass, 379 ms Paper (range 345 to 445 ms). Inside the 400 ms budget with about 10 ms of margin. First paint: median 392 / 378 ms. First launch after a fresh build was once 957 ms.
  - Idle memory: physical footprint 128 to 135 MB total (app 27 + WebContent 81 + GPU 14 + Networking 6.5), passes 150 MB; `ps` RSS sum swings 144 to 222 MB for the same state (counts shared pages), so as written the RSS budget is unreliable.
  - Frame times: 60 fps (display-limited), never a frame over 25 ms (max 21 ms) while resizing panes and flinging through the 5,000-message transcript; about 1 ms main-thread work per frame.
- Spike report written with a recommendation per direction.

## Gotchas
- `transparent: true` on macOS needs `macOSPrivateApi` (private API, blocks Mac App Store).
- Kobalte Combobox's listbox is popper-portaled and unsuited to an inline palette; the palette is Kobalte Dialog + a hand-built ARIA combobox.
- The chosen theme persists in `localStorage`, including themes set by the harness env, so measurements pass `ATRIUM_THEME` explicitly.
- `scripts/measure.py` and `scripts/screenshots.sh` expect the release bundle (`pnpm tauri build`); screenshots need Screen Recording permission for the invoking process.
- Playwright was used against `pnpm dev` for functional checks; it wrote its own scratch files outside this worktree.

## Follow-ups
- Orchestrator: decide how to state the memory budget (footprint vs RSS, helpers included) and review CR-01.
- Pedro: pick a direction; the others ship as alternative themes.
- Cold start has no slack (about 300 ms of it is Rust `main` to first JS line); keep the M1 first screen small and lazy-load heavy components.
- Someone with a Linux machine runs the section 6 checklist; decide the owner and gate in `decisions.md`.
- Add a licence notice for the vendored Catppuccin Mocha theme before bundling it.
