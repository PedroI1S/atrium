# Handoff: M1-09a-app-shell

Model: Claude Fable 5 (orchestrator, inline; D-057, D-059) · Status: **partial** (shift ended; the session stopped mid-task)

Written at the handoff of 2026-11-12 from the branch's state. The session that wrote the code left no note, so what follows is taken from its commits, its files and the outputs it left.

## Done
- `f7d75e3` (WIP): the `atrium-app` crate (bridge, shell state, tray, links, upgrade), the frontend on daemon data, the M1 token contract (theme loader, Calm themes in `app/themes/`, conformance test), window model, settings registry, sync reducer, UI components.
- The WIP commit on top of it (this handoff): harness screenshots from the webview's own snapshot (`app/src-tauri/src/snapshot.rs`, `webkit2gtk` + `cairo-rs` on Linux), a `custom-protocol` feature so debug harness builds serve `app/dist`, the window/draft round trip (`app/src/roundtrip.ts`, `ATRIUM_BENCH=roundtrip` / `roundtrip-verify`), and the scripts `app/scripts/live_check.py` and `app/scripts/screenshots.py`.
- `screenshots.py` ran on Linux at 12:21 on 2026-11-12 and passed (`ATRIUM_REPORT {"kind":"shots","pass":true}` for both the real daemon and the refusing stand-in). Pictures: main light and dark, sidebar hidden, panel minimized, inbox, Settings with Advanced (light, dark), a pop-out, main with a pop-out, the `Incompatible` banner. They are only on rodrigo's machine: `.orchestration/M1-09a-screenshots/`. Re-run the script to get them elsewhere.
- This machine has no tray support: the app logged "no tray item" and runs in the close-quits mode.

## Not done / not verified
- **Unknown whether `live_check.py` passed** (no output was kept). Run it first.
- The DoD command set was not re-run after the last changes: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `pnpm typecheck`, `pnpm test`, `pnpm build`, `scripts/check-protocol-ts`.
- The `unwrap`/`expect`/`panic!` audit of `app/src-tauri`, the TTI and idle-memory report with the ported harness.
- Flash review (read-only, with screenshots; D-048, D-059), then merge into `dev`.
- macOS: not run (Pedro's shift). Tray: not exercised (no tray here).

## Exact next step
In a recreated worktree (`git worktree add ../atrium-wt/M1-09a-app-shell agent/fable/M1-09a-app-shell`): run the DoD commands, then `python3 app/scripts/live_check.py`, then `python3 app/scripts/screenshots.py <dir>`; tick the DoD in this note; write the Flash review prompt.

## Gotchas
- `cargo test --workspace` needs WebKitGTK's development files on Linux (`atrium-app` is not a default member).
- Debug harness builds must pass `--features custom-protocol`, or the binary loads the Vite dev server.
- The scripts open windows: run them in the desktop session. No script can capture other windows there, hence the webview snapshots.
