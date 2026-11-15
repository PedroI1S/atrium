# Handoff: M1-14b-linux-app

Model: Claude Opus 5.5 (worker, D-085) on rodrigo's machine · Branch `agent/opus/M1-14b-linux-app` (from `dev` at `9ac5cdd`) · Status: **done, with an escalation**: the Linux idle budget (150 MB private PSS, one window) is **missed** by 9–10 MB (brief: escalate), and a measured 57 ms startup fix sits in a file outside this task's paths (`app/index.html`), not landed.

The report is the "App (M1-14b)" section of `docs/spikes/M1-linux-checks.md`. Raw outputs (every bench's text and JSON, the screenshots) are in the worktree's `.orchestration/m1-14b/` (git-ignored).

## Done, per Definition of Done

- [x] **L1 text right on Linux, with a screenshot.** M1-09a shipped it right; nothing changed. Settings → Appearance says "Atrium uses opaque windows on Linux."; the Advanced card's Blur strength row says "Atrium uses opaque windows on Linux. This only affects overlays."; nothing mentions a compositor. Pictures: `.orchestration/m1-14b/screens/settings-advanced-light.png`, `settings-advanced-dark.png`.
- [x] **`startup_bench.py` reports the D-040 numbers on Linux; one window and one pop-out.** Three harness bugs fixed (below). Idle, one window, 9 launches: **private PSS 151.4–152.5 MiB = 158.7–160.0 MB per launch: OVER the 150 MB gate**, every sample. Per process: UI 60.5, web 69.4, network 22.2, GTK's sandboxed SVG loader 4.4 MiB. Beside it: PSS 276.0, RSS 454.2, USS 208.0, GPU 114.6 MiB. **One pop-out: +66.9 MiB private PSS in the total, 71.2 MiB of its own** (64.2–69.8 in the total, 68.5–74.0 of its own; GTK's SVG loader, 4.3 MiB, exits during every run; a second web process, +4.9 in the UI process), +68.2 MiB GPU, ready in 229 ms.
- [x] **Startup profile table and the D-041 recommendation** in the spike report. 30 warm launches, perf build, load 2: **TTI from `main` median 377 ms, p90 391** (spawn → TTI 406 / 424). Phases (median / p90): spawn → main 30/40, main → setup 35/41, window built 115/122, web process start → navigation 86/91, `main` → first JS 252/257, first JS → mounted 35/42, **mounted → first paint 88/96**. The first paint waits on the fonts: preloading the five bundled fonts in `index.html` takes **TTI to 345 / 362 ms** (−57 / −63 against the base build in the same interleaved rounds, 402 / 425; 30 each). **Recommendation: make 400 ms a Linux gate with the macOS statistic once the preload is merged**, with the load stated and the spawn → TTI figure beside it.
- [x] **`live_check.py` 12/12, `screenshots.py` 28 pictures and 3 × `pass`, `chat_check.py` 12/12, streaming run 3 × within on p99 (21–22 ms) and long frames (0.2–0.4 %)**; the literal "over 16.7 ms" share (58–60 %) can't be read with 1 ms rAF timestamps (open, D-085). Tray: **not checked, this desktop has no tray support**; not worked around.
- [x] DoD commands (below).
- [x] This handoff.

## Escalations and decisions for the drivers

1. **Idle memory over the gate: 158.7–160.0 MB (151.4–152.5 MiB) private PSS, one window.** Not a measurement artefact: it is D-040's metric, measured connected, every sample over. WebKitGTK's own floor here is **124.5 MiB (130.5 MB)** (its MiniBrowser on a blank page); Atrium adds about 27 MiB (UI +9, web +23); the gate leaves room for 18.5. What I tried: DMA-BUF renderer off (−14 MiB, within the gate, but streaming collapses and TTI +76 ms: not usable), compositing off and `MALLOC_ARENA_MAX=2` (no change), a size-optimised binary (−2.5 MiB, UI process only). Not reachable by any single Atrium change I measured. Options for the drivers: a Linux gate defined against the measured floor, or a size project (lazy-loaded bundle, fewer web-process objects at idle, a smaller binary). D-085 says the gate decision is theirs.
2. **The font preload (`app/index.html`, five `<link rel="preload">` lines, in the report).** −57 ms median TTI on Linux, low risk, not in my paths. For the orchestrator to land (and check on macOS).
3. **The §14 frame statistic on Linux** stays open (D-085): the run gives p99 and skipped updates, both within.
4. **GPU memory while streaming: 416–424 MiB** (115 idle). D-040 watches it; no budget.
5. **`atriumd` exits silently when its log folder has mode 0755** (no log, no message): how the old bench measured a disconnected app for two tasks. For the daemon's owner (`crates/**`, not mine).

## Files changed (all committed)

- `app/scripts/linuxmem.py` (new): D-040's private PSS from `smaps`, per process and per kind; DRM fdinfo GPU memory; the app's process tree without the daemon. `app/scripts/test_linuxmem.py` (new, 7 tests).
- `app/scripts/startup_bench.py`: D-040 numbers (private PSS, PSS, RSS, USS, GPU, per process), the startup profile table (median, p90, min, max per phase), `--only popout` (cost of one pop-out), `--only breakdown`, `--app`, `--env`, `--out`; logs outside `ATRIUM_HOME`; daemon check per idle launch; MB vs MiB; failed runs keep their logs.
- `app/scripts/stream_bench.py`: same private PSS (`linuxmem.py`), GPU peak; uses the new `startup_bench` helpers.
- `app/scripts/webkit_floor.py` (new): WebKitGTK's own floor (PyGObject window or MiniBrowser) and a page-timing mode for bisecting the first paint.
- `app/src-tauri/src/lib.rs`: **the startup measurement hook** the brief allows (there was none): `setupStart`, `bridgeStarted`, `windowBuilt`, `setupEnd` wall-clock marks, sent in `startup_info` as `startupMarks`. No behaviour change.
- `app/src/` (measurement only, no startup behaviour change): `startup-mark.ts` (new, the bundle's first module: its start time), `harness.ts` (the startup report carries the profile: native marks, navigation timing, each boot stage's start), `popout-bench.ts` (new, `ATRIUM_BENCH=startup-popout`), `index.tsx` (imports `startup-mark` first; runs the pop-out bench after the startup report in that mode), `platform.ts` (`startupMarks` in `StartupInfo`).
- `docs/spikes/M1-linux-checks.md`: the "App (M1-14b)" section. This handoff.

No startup change was landed: the one that helps is in `app/index.html` (not owned); one that is in my paths (preloading from the bundle) gains nothing (measured) and was removed.

## Definition of Done commands (on the host, at `d91c299`, the last code commit; only docs after it)

| Command | Result |
|---|---|
| `cargo fmt --all --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 (`Finished dev profile`) |
| `cargo test -p atrium-app` | exit 0: lib 45 passed; `delivery_shapes` 2 passed; the others 0 |
| `pnpm typecheck` | exit 0 (`tsc --noEmit`) |
| `pnpm test` | exit 0: 13 files, 261 tests passed |
| `pnpm build` | exit 0: `index-CWiOT49v.js 272.65 kB │ gzip: 90.27 kB` |
| `python3 -m unittest test_linuxmem` (in `app/scripts`) | 7 tests OK |
| `python3 app/scripts/live_check.py` | 12/12 passed |
| `python3 app/scripts/screenshots.py <dir>` | 28 pictures; `real`, `chat`, `refused` all `pass` |
| `python3 app/scripts/chat_check.py` | 12/12 passed |
| `python3 app/scripts/stream_bench.py --runs 3` | p99 21 / 22 / 21 ms, long frames 0.3 / 0.4 / 0.2 % (within); "over 16.7 ms" 57.6–59.9 % as read (unreadable at 1 ms resolution) |
| `python3 app/scripts/startup_bench.py` | TTI median 377 / p90 391 (goal on Linux); memory OVER (158.7–160.0 MB); pop-out +66.9 MiB |

Every GUI run held `flock /tmp/atrium-gui-measure.lock`; no build ran during a measurement (the scripts' own no-op build step aside). Conditions: on battery, `balanced`, load 1.6–2.3 for the main bench, 3–4.4 for the interleaved A/B batches, 4–7 for the checks (right after a debug build).

## Gotchas

- `startup_bench.py --app <copy>` needs `atriumd` next to the copy (the app starts the daemon beside its own executable); without it the app never connects.
- A/B startup numbers drift with load by 20–30 ms between batches: compare builds interleaved (the A/B tables in the report are).
- WebKitGTK reports no resource timing for Tauri's custom protocol (`resources` is empty in the profile).
- The app's processes are found by the process tree; the WebKit web process is not under `bwrap` here, but GTK's `glycin-svg` loader is (the "helpers" row).

## Processes

Every app, daemon and bench process I started was stopped; `ps` matching the worktree path, `atriumd`, `MiniBrowser` and `webkit_floor` showed none at the end (checked 04:48). No `/tmp/atrium-{bench,stream,live,shots,chat}-*` folder of mine is left (`/tmp/atrium-shots-out` is from 2026-11-12, not mine). The size-optimised build is in `target/m1-14b-small/` (git-ignored; delete it at will).

## Exact next step

For the orchestrator: (1) take the idle-memory escalation to the drivers with the floor numbers; (2) land the font preload in `app/index.html` and re-run `startup_bench.py` here and on macOS; (3) Hydrogen review (read-only) with the report, the JSON in `.orchestration/m1-14b/` and the screenshots.
