# Review: M0-06b-ui-shell-layout-b

Author: GPT-6.1 Sol · Reviewer: Sonnet 5.5 (read-only, head `31422e3`) · Triage: orchestrator (Opus 5.5), 2026-10-25

## Verdict
**Approve with changes; the measurements aren't accepted yet.** The opacity mechanisms, the Rust-owned single-writer state, the `-rgb` channel tokens and the reserved runtime names match CR-02 and D-024; no redesign is needed. The six-configuration matrix wasn't produced (timeouts), so its results don't count yet.

## Findings (all accepted → fix round 1, combined with Pedro's hands-on notes)

| # | Sev. | Finding | Fix |
|---|---|---|---|
| 1 | blocker | 3-window/translucent timeouts. The likely cause: readiness waits on rAF in secondary windows, and rAF doesn't fire in occluded or non-activated WKWebViews. Pop-outs stack exactly on top of each other, and cold/frames runs launch via `Popen` without activation (the same 3-window config was ready under `open -n`). | Signal readiness after mount, not rAF; cascade pop-out positions; launch cold/frames like `rss` (`open -n`), or activate and hold an activity in harness mode. Confirm with the discriminating checks. |
| 2 | blocker | Every failure becomes a silent 40/90 s timeout: no catch in `boot`/`runHarness`, `wait_report` doesn't poll the process, stderr goes to DEVNULL, and the matrix aborts on the first failure. | Error reports with a stage; fail fast on a dead process; log stderr; per-cell error handling in the wrapper. |
| 3 | major | `readyWindows` is tautological, and only the main window's applied state is validated. | Report the real ready count and each window's applied state. |
| 4 | major | About 60 Hz `shell_patch` for scroll position (full re-serialise + broadcast); focus re-runs the whole native chain. | Debounce scroll patches; re-run native calls only when inputs change. |
| 5 | major | TTI regression is half harness (extra rAF/IPC before the probe) and half real (themes loaded twice, a serial IPC chain before first render, vibrancy applied then cleared). | Probe before the gate; load themes once; paint tokens synchronously from `startup_info`; native calls in parallel after mount; use the persisted settings in `setup`. |
| 6 | major | A rejected patch latches `writeFailure`; close/"Move back" then throw, and the window can't be closed. | Ownership rejections don't latch; close = `try flush finally bring_back`. |
| 7 | major | Opacity slider drag: a synchronous non-atomic file write plus an unbounded paint queue per event; arbitrary keys can be persisted. | Latest-wins apply; debounced atomic persist; whitelist keys. |
| 8 | minor | Ownership holes: edits during the switch are lost, main can't force Bring back from a hung pop-out, `IndexMut` inserts null. | View read-only as soon as pop-out starts; main-initiated bring-back with a timeout; `get_mut`. |
| 9 | major | Advanced opens in the 46 px strip when the right panel is minimised (the design wants a 400 px card). | Restore the panel width (token) when opening Advanced. |
| 10 | minor | `--window-opacity`/`--window-blur` are inert; whole-window mode uses `NSWindow.alphaValue`, not CSS (contradicts ui-direction §5); `NSVisualEffectView` has no blur strength; bare `0.35`; no accessibility notification observer; Linux fallback is opaque-only. | Document in the CR-02 feedback; name constants. |
| 11 | minor | The conformance test misses `windows.ts`/`window-model.ts`/`state.ts` and bare fractions; `base.css` duplicates Calm TOML literals. | Widen the scan; remove duplicates. |
| 12 | minor | Layout B gaps: settings tabs, attention dot, `+` topic, raised selected card, pop-out chrome, per-pop-out opacity; pop-out traffic-light position. | Note as known gaps; fix the traffic lights. |
| 13 | minor | Tests overclaim (`native_close_preserves…`, `moveView`); no tests for `shell_patch` ownership or close/bring-back races; the report is stale. **The 3-window footprint (183–194 MB) is over the 150 MB budget.** | Pure-function tests; update the report; the 3-window budget question goes to Pedro. |
