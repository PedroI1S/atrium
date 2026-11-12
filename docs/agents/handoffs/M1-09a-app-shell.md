# Handoff: M1-09a-app-shell

Model: Claude Fable 5 (orchestrator, inline; D-057, D-059) on rodrigo's machine, finished by Claude Opus 5.5 (orchestrator, inline) on Pedro's Mac · Status: **done**: Flash review **approve** (two nits, fixed), Opus `unsafe` audit **sound with changes** (fixed)

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

## Pedro's shift, 2026-11-12 (macOS, Claude Opus 5.5)
First run on macOS. All on Pedro's MacBook Air.
- **DoD commands**: `cargo fmt --all --check` (two spots reformatted, nothing else), `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (atrium-app: 41), `scripts/check-protocol-ts`, `pnpm typecheck`, `pnpm test` (178), `pnpm build`: all pass.
- **`live_check.py`: 12/12 passed**: draft through a pop-out and back, a forced return, the daemon killed and restarted by the app, the thread listed as interrupted, a crash and relaunch with the draft and thread back, the running daemon reused.
- **Screenshots on macOS** (`app/scripts/screenshots.py`, both runs pass): `snapshot::take` now has a macOS version, `screencapture -l<windowNumber>`, synchronous, so the picture is the real window (traffic lights, shadow, vibrancy). It needs `NSWindow::windowNumber`, a fourth `unsafe` block in `macos.rs` (CR-11). The first capture asks for Screen Recording permission for the app the harness runs from. Pictures: `.orchestration/M1-09a-screenshots-mac/` (Pedro's Mac).
- **Two macOS layout bugs found from the pictures, fixed** (`styles/shell.css`): with the sidebar hidden, the main card's title ran under the traffic lights; the `Incompatible` banner's text was under them too.
- `screenshots.py` now builds the app first, as `live_check.py` does (it used a stale binary once).
- **`unwrap`/`expect`/`panic!` audit** of `app/src-tauri/src` outside test modules: none.
- **CR-11** written (it was referred to in `macos.rs` but missing): `unsafe` allowed in `macos.rs` only, four blocks, Opus audit before the merge. Pedro to decide.
- **The measuring script** the brief asked to port didn't exist: `app/scripts/startup_bench.py` (new) uses contract §14's statistics: 30 warm launches (median, p90) and 9 launches of 1 Hz memory samples from 5 s to 65 s after TTI (`phys_footprint` on macOS, private PSS on Linux). Results on Pedro's MacBook Air, release build:
  - **Cold start, 30 warm launches: median 340 ms, p90 346 ms** (min 324, max 379): within the gate (< 400, < 450).
  - **Idle memory, 9 launches: per-launch medians 82.6–84.8 MiB** (app + 3 WebKit processes): within the gate (< 150). Per-launch max 86.8–93.0 MiB, except launch 9: one sample at 143.7 MiB (under 150, not an excursion by §14's rule; not explained).
- Tray: not exercised by any script on macOS (the app has one there; check by hand).

## Reviews
- Gemini 3.8 Flash (read-only, `reviews/M1-09a-app-shell.md`): **approve**. Nits fixed: a comment in `replace_daemon` on the `WhenIdle` reconnect; the audit fixes committed.
- Opus 5.5 `unsafe` audit (`reviews/M1-09a-app-shell-unsafe.md`, D-061): **sound with changes**. Fixed: a `MainThreadMarker` check in `apply_to_new_window`, the module doc, the observer's SAFETY comment. Left as known: `set_alpha` can apply the alpha after reporting a timeout if the main thread is busy for more than 5 s (stale state until the next change, not unsound).

## For M1-09b and M1-14
- M1-09b: build on the bridge, window model, sync reducer and settings registry as they are; the harness scripts build the app first; screenshots on macOS use `screencapture` (Screen Recording permission).
- M1-14: run `live_check.py`, `screenshots.py` and `startup_bench.py` on Linux (private PSS); the tray on a desktop that has one; M1-10's Linux review findings (`reviews/M1-10-host-linux-exit.md`).

## Exact next step (historical, from rodrigo's handoff)
In a recreated worktree (`git worktree add ../atrium-wt/M1-09a-app-shell agent/fable/M1-09a-app-shell`): run the DoD commands, then `python3 app/scripts/live_check.py`, then `python3 app/scripts/screenshots.py <dir>`; tick the DoD in this note; write the Flash review prompt.

## Gotchas
- `cargo test --workspace` needs WebKitGTK's development files on Linux (`atrium-app` is not a default member).
- Debug harness builds must pass `--features custom-protocol`, or the binary loads the Vite dev server.
- The scripts open windows: run them in the desktop session. No script can capture other windows there, hence the webview snapshots.
