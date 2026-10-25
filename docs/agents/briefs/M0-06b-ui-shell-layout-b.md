# M0-06b-ui-shell-layout-b: layout B, window opacity, pop-out windows

Model: GPT-6.1 Sol · Reviewer: Sonnet 5.5 · Depends on: M0-06 (merged), D-024 · Wave 2
Branch: `agent/sol/M0-06b-ui-shell-layout-b` · Worktree: `../atrium-wt/M0-06b-ui-shell-layout-b`

## Goal
De-risk what the chosen UI direction (D-024, layout B "Floating spaces") adds on top of the merged M0-06 shell. Specifically:
- **window opacity in both modes**;
- **pop-out windows**;
- the **performance of the floating-card look**.

These must be proven before M1 builds real UI on them.

## Context
- **The spec:** `docs/design/ui-direction.md`.
- **Draft themes:** `docs/design/themes/atrium-calm-{light,dark}.toml`.
- **Proposed token changes:** **CR-02** in `docs/agents/change-requests.md`.
- **Decisions:** D-023 (Pedro's requirements) and D-024 (the choice).
- **The merged shell and its report:** `spikes/ui-shell/`, `docs/spikes/M0-06-ui-shell.md` (sections 1 and 10: vibrancy, `macOSPrivateApi`, measurement method), `docs/agents/reviews/M0-06-ui-shell.md`.
- **Contracts:** `docs/contracts/m0.md` §4–§5.
- **PLAN:** §3.20 and §4 (budgets: TTI < 400 ms, app idle < 150 MB, 60 fps).
- **Prior art:** `docs/agents/README.md` "Prior art". Copy no code.

## Owned paths
- `spikes/ui-shell/**`
- `docs/spikes/M0-06b-ui-shell-layout-b.md`, `docs/spikes/M0-06b-ui-shell-layout-b/` (screenshots, if any)
- `docs/agents/handoffs/M0-06b-ui-shell-layout-b.md`

## What to build (spike quality; CR-02 is implemented here as a proposal, not yet a contract)
1. **Layout B** in the shell: floating main card, topic-grouped sidebar, minimizable right panel, simple settings view + an **Advanced side pane** (no modal). Use the Atrium Calm themes from `docs/design/themes/` (copy them into the spike).
2. **Window opacity, both modes** (D-024):
   - (a) **background-only**: translucent surfaces over native vibrancy via CR-02's runtime tokens (`--window-alpha-*`, `--<surface>-rgb`);
   - (b) **whole-window**: real window alpha.

   Both come from one settings control, persisted, and changed live with no reload. Find the right Tauri 2 / macOS mechanism for each, and document which need `macOSPrivateApi`. Give a Linux fallback in code (opaque, or alpha only where the compositor supports it); it's untested here.
3. **Pop-out windows**: any chat, editor or database view (a dummy is fine) can be moved into its own window and back, without losing its state. Each window gets the same theme and opacity, and theme or opacity changes apply live to all windows. Use dummy shared state, and say how M1 would back it with the daemon (M0-05's `ClientHandle`).
4. **Measurement harness** (`scripts/`, bash with `#!/usr/bin/env bash`, runnable from fish):
   - TTI (median and worst case);
   - app footprint;
   - frame times while scrolling a 5k-message transcript and resizing panes;
   - each **with translucency off, background-only, and whole-window**, and with 1 and 3 windows open.

   The harness pins and prints its configuration.

## Definition of Done
- [ ] `pnpm install && pnpm typecheck && pnpm test` (including the token-conformance test: no hard-coded visual values; tests for the opacity token emission and window-state round-trip), and `cargo clippy --manifest-path spikes/ui-shell/src-tauri/Cargo.toml -- -D warnings`, all passing.
- [ ] `pnpm tauri build --debug` builds.
- [ ] Spike report `docs/spikes/M0-06b-ui-shell-layout-b.md`:
  - each opacity mode's mechanism and macOS/Linux caveats;
  - pop-out design and limits;
  - the exact measurement commands;
  - a recommendation for M1;
  - CR-02 feedback (does the proposal work as written?).
- [ ] Handoff written.

**Your sandbox**:
- **Network:** you have it for crates and npm. If the global cargo cache can't be written, use a `CARGO_HOME` inside your worktree or `/tmp`.
- **The app:** you probably can't launch it or take screenshots in your sandbox. Don't work around that. Make the harness complete and say so in the handoff; **the orchestrator launches the app and runs the measurements**.
- **Git:** if it's blocked, leave the work uncommitted; the orchestrator commits it.

## Out of scope
Real data, the daemon, real editor/terminal/database components, Linux runs, the keybindings file, and changing `docs/design/` or the contracts (file change requests instead).

## Escalate if
- No supported Tauri 2 mechanism gives whole-window opacity on macOS without private APIs. Report the options and stop on that item.
