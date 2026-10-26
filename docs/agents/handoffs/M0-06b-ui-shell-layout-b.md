# Handoff: M0-06b-ui-shell-layout-b

First version: GPT-6.1 Sol (2026-10-25, partial). **Fix round 1: Opus 5.5 (2026-10-26). Status: done.**

Report: [docs/spikes/M0-06b-ui-shell-layout-b.md](../../spikes/M0-06b-ui-shell-layout-b.md). Branch `agent/sol/M0-06b-ui-shell-layout-b`, committed, not pushed.

## Done (fix round 1)

- **All 13 review findings fixed** (report §1 has the finding → fix → test table). Every bug fix has a test that fails without it: `src/sync.test.ts` (#6, #7, #8, #4), `src/layout.test.ts` (#9, settings registry), `src-tauri/src/shell.rs` tests (#7 whitelist/persister/atomic write, #8 `get_mut`, ownership), `src/styles/tokens-conformance.test.ts` (#11). Plus an end-to-end pop-out check in the real app: `python3 scripts/measure.py roundtrip` (5/5 pass).
- **Harness** (#1–#3, #5): mount-based per-window readiness with each window's applied appearance; cascaded pop-outs restored after main is interactive; app self-activates and holds an activity in harness mode; error reports with a stage; fail-fast on a dead process or error report; per-run app logs; per-cell failure handling; interleaved cold matrix with the load average recorded; `scripts/summarize.py` prints the table.
- **Layout B rebuilt to `docs/design/ui-direction.md`**, ported from the prototype's source onto Calm tokens: sidebar on the frame (raised selected chat, topic circles with amber attention dot, `+` topic), chat / inbox / editor / database cards, minimizable context panel, settings with section tabs, search (covers Advanced) and the 400 px Advanced card, pop-out chrome (title, status pill, keep-on-top, move back), "Popped-out windows: Same as main / Always opaque", pop-out traffic lights aligned. Instrument Sans + Geist Mono bundled (OFL).
- **Measured** (release, full matrix, under the shared GUI lock; report §5): TTI medians 327–341 ms in all six configurations; 60 fps in all frame tests; footprint 125–134 MB with one window, **194–207 MB with three (over 150 MB)**.
- **Screenshots** (12 JPEGs, 1.6 MB) in `docs/spikes/M0-06b-ui-shell-layout-b/`; logs in `.../fix1-logs/`.
- **CR-03** appended to `docs/agents/change-requests.md` (CR-02 feedback; ui-direction §3/§5 amendments). No contract or `docs/design/` file changed.

## Not done / limits

- Visual-only controls: text size, tint (needs Warm/Cool theme files), card gap, fonts, and the non-Appearance settings sections. Pop-out positions are cascaded, not remembered; no drag-to-detach. Editor is a highlighted textarea (CodeMirror is M1).
- Background-mode opacity at 60 % looks only slightly see-through: the native frosted material under the frame is dense, and ScreenCaptureKit captures may under-show the live blur. Worth Pedro's eye on a real desktop (report §4); M1 can try a thinner material.
- Linux not run (driver-owned deferral).

## Questions for Pedro

1. **Three-window footprint:** each pop-out adds about 33 MB (its own WebKit process). Is 150 MB the budget for one window, or with pop-outs open?
2. Whether the background-mode glass should be clearer than the native material gives (report §4).

## Definition of Done results (2026-10-26, from `spikes/ui-shell`)

| Command | Result |
|---|---|
| `pnpm install && pnpm typecheck && pnpm test` | Exit 0. 5 test files, **130 tests passed** (was 95). |
| `cargo clippy --manifest-path spikes/ui-shell/src-tauri/Cargo.toml --all-targets -- -D warnings` | Exit 0, no warnings. |
| `cargo test --manifest-path spikes/ui-shell/src-tauri/Cargo.toml` | Exit 0. **12 passed** (11 `shell.rs`, 1 `lib.rs`). |
| `pnpm tauri build` | Exit 0. `Atrium UI Shell.app` release bundle, 4.81 MiB. |
| `pnpm tauri build --debug` | Exit 0. Debug bundle, 29.38 MiB. |
| `/usr/bin/lockf -k /tmp/atrium-gui-measure.lock scripts/measure-layout-b.sh all <dir>` | All cells passed (cold rerun interleaved with `coldmatrix` after the first sequential run hit heavy machine load). |
| `/usr/bin/lockf -k /tmp/atrium-gui-measure.lock python3 scripts/measure.py roundtrip` | 5/5 checks pass. |
| `bash -n scripts/*.sh`, `python3 -m py_compile scripts/*.py`, `git diff --check` | Exit 0. |

## Gotchas

- **Run every GUI command under the lock**: `/usr/bin/lockf -k /tmp/atrium-gui-measure.lock <command>` (other agents' windows throttle rAF and skew numbers). The M0-07 overlap the orchestrator flagged (02:35–02:41) predates every number in the report (runs started 02:58).
- The machine was busy during measurements (load average 4–8, Messages at ~50 % of a core). Interleaving configurations keeps comparisons fair; absolute worst cases are load-sensitive.
- WebKit clamps `performance.now()` to 1 ms: ">16.7 ms" frame counts are meaningless; read fps, p99 and max.
- A first screenshot attempt that captured a screen region picked up unrelated screen content; it was deleted at once and never committed. Screenshots now use `screencapture -l` (app window only) or ScreenCaptureKit with a filter that includes only the app and the synthetic backdrop window.
- Harness runs never read or write the user's `calm-settings.json`.
- All processes started by this round were stopped; a final `pgrep` found no app, backdrop or harness process.

## Follow-ups

- Orchestrator: decide the three-window footprint budget with Pedro; consider CR-03; record the `macOSPrivateApi` distribution decision when M1 starts.
- M1: back the shared state with M0-05's `ClientHandle` (report §3), keep `sync.ts`'s ordering helpers, track TTI/frames/footprint in CI with this harness.
- Pedro (fish), to look at it:

```text
cd /Users/pedro/Documents/atrium-wt/M0-06b-ui-shell-layout-b/spikes/ui-shell
open "src-tauri/target/release/bundle/macos/Atrium UI Shell.app"
```
