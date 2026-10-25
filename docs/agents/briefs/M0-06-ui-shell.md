# M0-06-ui-shell: UI shell, token pipeline and visual directions

Model: Sonnet 5.5 · Reviewer: GPT-6.1 Sol (+ screenshots) · Depends on: none · Wave 1
Branch: `agent/sonnet/M0-06-ui-shell` · Worktree: `../atrium-wt/M0-06-ui-shell`

## Goal
A clickable Tauri 2 + SolidJS mock of Atrium's window, with dummy data, that proves the native chrome, the layout system and the theme-token pipeline, and offers **2–3 visual directions** for Pedro to choose from.

## Context
- PLAN §3.16 (frontend), **§3.20 (look & feel: read it fully)**, §5 M0 item 5, §4 budgets (cold start < 400 ms, app idle RSS < 150 MB).
- Token contract: `docs/contracts/m0.md` §4 (layers, required semantic tokens, theme TOML format, validation).
- Atrium has its own design language. Other tools are references for density and polish only; copy nothing.

## Owned paths
- `spikes/ui-shell/**` (standalone: its own `src-tauri/Cargo.toml`, `package.json`, `pnpm-lock.yaml`)
- `docs/spikes/M0-06-ui-shell.md`, `docs/spikes/M0-06-ui-shell/` (screenshots)
- `docs/agents/handoffs/M0-06-ui-shell.md`

## Contract to implement (do not modify)
- **Chrome (macOS)**: `titleBarStyle: "Overlay"`, `hiddenTitle`, a custom `trafficLightPosition` so the traffic lights sit inline with the tab strip, `data-tauri-drag-region`, native vibrancy via `window-vibrancy`, and CSS `backdrop-filter` on glass panels.
- **Layout**:
  - left sidebar (Inbox with a badge, Home, workspaces → threads);
  - a draggable tab strip;
  - resizable split panes (Corvu): chat next to a file preview, and two threads side by side;
  - right panel tabs (Review / Diff / Browser / Terminal / Database: placeholders are fine);
  - a composer;
  - a command palette (Kobalte) with `⌘K` and `⌘J` wired to dummy actions;
  - layout presets *Default / Review / Focus* switchable from the palette.
- **Tokens**: components use only semantic and component tokens (`docs/contracts/m0.md` §4).
  - The theme loader reads `themes/*.toml`, resolves references, validates the required set and WCAG AA, and swaps CSS variables at runtime with no reload.
  - One imported VS Code theme (a permissively licensed one, e.g. from Shiki's bundled themes), mapped onto semantic tokens. Record the mapping table in the report.
- **Visual directions**: 2–3 genuinely different token sets on the same shell (e.g. glassy/vibrant, flat/dense, warm/editorial), each with dark and light, all switchable live.
- `prefers-reduced-motion` disables transitions.

## Definition of Done
- [ ] `pnpm install && pnpm typecheck && pnpm test` (at least the theme loader: reference resolution, a missing token, a cycle, contrast failure) and `cargo clippy --manifest-path spikes/ui-shell/src-tauri/Cargo.toml -- -D warnings`, all passing.
- [ ] `pnpm tauri build --debug` produces an app that launches.
- [ ] Screenshots of every direction (dark + light) and of the palette and split panes in `docs/spikes/M0-06-ui-shell/`. Use `screencapture`; if it's blocked by permissions, say so and list the exact shots for the orchestrator to take.
- [ ] Measured on macOS, with method and numbers in the report: cold start to first paint, app idle RSS (Tauri process + WebKit helpers), frame times while resizing panes and scrolling a long dummy transcript (sampled with `requestAnimationFrame` and logged).
- [ ] Spike report `docs/spikes/M0-06-ui-shell.md` with a recommendation per direction. Linux/WebKitGTK is **not** measured here (no Linux machine yet): list what a Linux run must check.
- [ ] Handoff written.

## Out of scope
Real data, the daemon, real editor/terminal/diff components, keybinding files, theme import UI. Don't pick the winning direction; Pedro does.

## Escalate if
- Overlay title bar + vibrancy can't be made to work together in Tauri 2 on macOS 27. Report what you tried, then stop.
- You need a contract change (e.g. a missing semantic token) → `docs/agents/change-requests.md`, then continue with a component token as a stopgap.
