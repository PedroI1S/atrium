# M1-14b-linux-app: Linux checks for the app (L1, L2 harness, L3 profiling)

Model: Claude Fable 5.2 (D-065: Opus 5.5's role on rodrigo's shifts) · Reviewer: Gemini 4 Hydrogen (D-068; read-only, with the numbers and screenshots) · Depends on: M1-09a (merged), **M1-09b merged** (it owns `app/src/**` and `app/scripts/**` until then)
Go: D-059 (rodrigo), D-066 · Branch: `agent/fable/M1-14b-linux-app` · Worktree: `../atrium-wt/M1-14b-linux-app`

The second half of M1-14 (contract §15). The first half, M1-14a, covers the host crate and the daemon. **Not dispatched until M1-09b is merged into `dev`.**

## Goal
Make the app's Linux numbers real and repeatable: the harnesses report what D-040 budgets, WebKitGTK's startup is profiled so the driver can decide D-041's open point, and M1-09a's scripts and M1-09b's runs are checked on Linux.

## Context
- `docs/agents/m1-must-fix.md`, "From the M0 Linux checks": L1, L2, L3 (and L10, which is not M1's).
- Decisions D-040 (private PSS is the budgeted number; GPU memory, full PSS and RSS beside it; each window's cost tracked), D-041 (400 ms is a goal on Linux until profiling shows what is reachable), D-042.
- `docs/contracts/m1.md` §11.1 (the blur-strength note per platform, L1), §14 (how startup and idle memory are measured).
- `docs/spikes/M0-linux-checks.md` (§2.4 startup phases, §2.5 memory), `.orchestration/m0-linux/` on rodrigo's machine.
- `docs/agents/handoffs/M1-09a-app-shell.md` ("M1-14" follow-ups) and `handoffs/M1-09b-home-chat.md` (what it ran on Linux and what it left).
- `app/scripts/live_check.py`, `screenshots.py`, `startup_bench.py`; `docs/spikes/M1-linux-checks.md` (M1-14a wrote the Host and Daemon sections).

## Owned paths
- `app/scripts/**` (the harnesses), `app/src/**` only for L1's text and for startup changes that the profiling justifies, each one listed in the handoff.
- `docs/spikes/M1-linux-checks.md`: the "App" section.
- `docs/agents/handoffs/M1-14b-linux-app.md`
- Not yours: `crates/**`, `bindings/**`, the contract, `app/src-tauri/**` except a startup measurement hook if one is missing (say so).

## What to do
1. **L1.** On Linux, Settings → Appearance says "Atrium uses opaque windows on Linux" for blur strength, and nothing blames the compositor. Check what M1-09a shipped; fix what is off; screenshot.
2. **L2.** `startup_bench.py` on Linux reports private PSS as the budgeted number, with full PSS, RSS and GPU memory (DRM fdinfo) beside it, summed over the app and its WebKit processes, with §14's statistics. Add the cost of one pop-out window (measured, not capped). Run it and report.
3. **L3.** Profile WebKitGTK startup with marks for: process start → `main`, `main` → first JS, SolidJS mount, input-ready, first paint. 30 warm launches, median and p90 per phase, release build. Say which phase can be shortened and by what (name the change and its measured effect if you try it); don't land a risky startup change without the numbers. End with a recommendation for D-041: whether 400 ms can become a gate on Linux.
4. **M1-09a's and M1-09b's runs on Linux:** `live_check.py`, `screenshots.py`, M1-09b's streaming run (§14: 10 threads × 1k tok/s × 30 s). Report pass/fail and numbers. The tray can't be checked on this desktop (no tray support): say so, don't work around it.

## Definition of Done
- [ ] L1 text right on Linux, with a screenshot.
- [ ] `startup_bench.py` reporting the D-040 numbers on Linux; results for one window and for a pop-out.
- [ ] The startup profile table and the D-041 recommendation in `docs/spikes/M1-linux-checks.md`.
- [ ] `live_check.py`, `screenshots.py` and the streaming run on Linux: results.
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test -p atrium-app`; `pnpm typecheck`; `pnpm test`; `pnpm build`.
- [ ] Handoff.

## Rules
- GUI runs are exclusive: `flock /tmp/atrium-gui-measure.lock <command>`, held only while measuring. Build first, measure after, with no other build running; say what else was running.
- The agent's shell is a container without `git`, `node`, `python3` or WebKitGTK's development files: run those, and every `cargo` command that builds the app, through `distrobox-host-exec`.
- Stop every app and daemon you start.

## Out of scope
L8 and the host crate (M1-14a), L10, macOS, packaging, new UI.

## Escalate if
The idle budget (150 MB private PSS, one window) is missed; a startup fix needs a change in `crates/**` or the contract.
