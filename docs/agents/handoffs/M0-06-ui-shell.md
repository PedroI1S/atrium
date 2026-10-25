# Handoff: M0-06-ui-shell

Model: Sonnet 5.5 · Status: done

Report: [docs/spikes/M0-06-ui-shell.md](../../spikes/M0-06-ui-shell.md) · Screenshots: `docs/spikes/M0-06-ui-shell/` · Branch `agent/sonnet/M0-06-ui-shell`

## Done
- Tauri 2 + SolidJS shell in `spikes/ui-shell/` with overlay title bar, traffic lights inline with the tab strip, native vibrancy (`window-vibrancy`) plus `backdrop-filter`. Overlay + vibrancy work together on macOS 27.0.1 (no escalation); `transparent` needs `macOSPrivateApi`.
- Layout: sidebar (Inbox badge, Home, workspaces to threads), draggable tab strip (solid-dnd), Corvu split panes (chat + file preview, two threads side by side), right panel tabs (Review / Diff / Browser / Terminal / Database), composer, 5,000-message virtualized transcript, Kobalte palette with `⌘K` and `⌘J` wired to dummy actions, presets Default / Review / Focus.
- Token pipeline (`src/theme/`): TOML loader with 3-layer reference resolution, cycle and missing-reference errors, required-set validation, WCAG AA check, runtime swap without reload, VS Code importer (Catppuccin Mocha, mapping table in the report). 65 tests at first delivery (87 after fix round 1), including the CI-style check on every shipped theme.
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
- `pnpm install && pnpm typecheck && pnpm test`: pass (`tsc --noEmit` clean; vitest 1 file, 65 tests passed at first delivery; 87 in 2 files after fix round 1).
- `cargo clippy --manifest-path spikes/ui-shell/src-tauri/Cargo.toml -- -D warnings`: pass.
- `pnpm tauri build --debug`: produces `src-tauri/target/debug/bundle/macos/Atrium UI Shell.app`; launched with `ATRIUM_BENCH=startup` and it reported first paint and a handled keystroke.
- Screenshots: all captured (list above), `docs/spikes/M0-06-ui-shell/`.
- Measured (release build, Apple M4, macOS 27.0.1, 12 runs each):
  - Time to interactive (synthetic keystroke): superseded by fix round 1 below.
  - Idle memory: superseded by fix round 1 below (footprint 125 to 131 MB; `ps` RSS 176 to 215 MB).
  - Frame times: superseded by fix round 1 below (60 fps; one 27 ms frame in resize).
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

## Fix round 1 (review GPT-6.1 Sol, 9 findings, all fixed)
Details in section 10 of the report. Status stays **done**.

- **#2** component/semantic name collisions are now load errors (6 tests). **#3** strict CSS `rgb()` parsing (15 malformed forms tested).
- **#1** about 25 component tokens; every literal colour, absolute size and duration removed from component CSS/TSX; `src/styles/tokens-conformance.test.ts` fails the build on regressions. CR-01 untouched (Pedro decides); stopgaps kept.
- **#4/#5/#6/#7** re-measured on the final release build, each command pins and prints its starting state:
  - Startup (Glass, 20 launches): TTI median 345 ms, worst 354 ms; Paper (15): median 346 ms, worst 354 ms. Across seven sessions the medians were 345 to 392 ms and the worst warm launch 457 ms; the first launch after a build was 957 ms once and 2,836 ms once. **Median is under 400 ms; some launches exceed it.** The accepted statistic is a gate decision, so no pass is claimed.
  - Memory (owned processes via `open -n` + responsible pid, auditable): footprint 125 to 131 MB (under 150 MB); `ps` RSS 176 to 215 MB (over 150 MB, and it moves with compression). The frozen RSS budget is unchanged; the orchestrator should decide the metric.
  - Frames: 60 fps; one 27 ms frame in 9 s of resizing, none elsewhere over 25 ms; synchronous work and rAF-to-next-task work are reported separately (mean about 2.5 ms fling, about 4 ms resize; worst single frame about 11 ms). Not proof of 120 Hz headroom.
- **#8** requested vs applied vibrancy with an opaque fallback (checked in a browser; not on Linux). **#9** Home recreated from the sidebar.
- Screenshots: 16 PNGs, 2.2 MB total, 1600 px wide (Pillow quantization, since `sips` alone left 7.7 MB); regenerated from the final build.
- DoD: `pnpm typecheck`, `pnpm test` (87 tests) and `cargo clippy ... -D warnings` pass; `pnpm tauri build --debug` builds and launches.
