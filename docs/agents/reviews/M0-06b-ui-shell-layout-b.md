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


---

# Review 2: Opus fix round 1 (head `ee0b2bc`)

Reviewer: GPT-6.1 Sol (read-only) · 2026-10-26 · Verdict **rework**: 9 findings (majors: close-time edit race, TTI computed before paint, a harness-only scheduling activity), all accepted. Earlier findings #2/#4/#5/#8/#13 are still partly open. The remaining fix round (2/2), or merging as a spike with M1 must-fixes, is Pedro's call.

**Rework.** Prior findings **#2, #4, #5, #8 and #13 remain partly fixed**, and harness-only scheduling prevents accepting the performance claims as representative of normal use. The screenshots substantially match the chosen direction; settings validation and atomic persistence look sound, and the recorded footprint supports “three windows over budget.” The capture path explicitly filters to Atrium and the synthetic backdrop, with no unrestricted fallback. `pnpm typecheck` passed; Vitest, Clippy and Cargo tests were blocked by sandbox write restrictions. Nothing was modified or launched.

1. **Major — Closing a pop-out can still lose edits made during flushing.**  
   [windows.ts:250](/Users/pedro/Documents/atrium-wt/M0-06b-ui-shell-layout-b/spikes/ui-shell/src/windows.ts:250), [sync.ts:107](/Users/pedro/Documents/atrium-wt/M0-06b-ui-shell-layout-b/spikes/ui-shell/src/sync.ts:107). `closeThisPopout()` leaves the view editable while awaiting `flushEdits()`. That flush awaits the queue’s current tail; an edit queued afterward can race with `bring_back`, be rejected after ownership transfers, and disappear when the window closes. This leaves prior **#8/#13** incomplete. **Fix:** synchronously freeze editing when closing begins, make close idempotent, then drain all accepted edits before transferring ownership. Add a test that attempts another edit while an earlier write is deliberately stalled.

2. **Minor — Pop-out drops the pending scroll-position flush.**  
   [windows.ts:229](/Users/pedro/Documents/atrium-wt/M0-06b-ui-shell-layout-b/spikes/ui-shell/src/windows.ts:229), [windows.ts:238](/Users/pedro/Documents/atrium-wt/M0-06b-ui-shell-layout-b/spikes/ui-shell/src/windows.ts:238). `popOut()` sets `moving` before flushing the scroll debounce. The debounce calls `patchView()`, which immediately returns because `canEdit()` is now false. Popping out within 150 ms of scrolling therefore restores an older position. The debounce helper test cannot catch this wiring error; prior **#4/#8** are partly fixed. **Fix:** block new user edits while allowing already-buffered state to flush, and test the complete scroll → pop-out sequence.

3. **Major — Reported TTI occurs before paint and violates contract §5.**  
   [measure.py:143](/Users/pedro/Documents/atrium-wt/M0-06b-ui-shell-layout-b/spikes/ui-shell/scripts/measure.py:143), [bench.ts:225](/Users/pedro/Documents/atrium-wt/M0-06b-ui-shell-layout-b/spikes/ui-shell/src/bench.ts:225). TTI uses only the synthetic keydown timestamp. The committed logs demonstrate the error: background/one-window reports **336 ms TTI versus 357.5 ms first paint**, and background/three-window reports **340.6 versus 361.6 ms**. Consequently, the reported worst cases and counts over 400 ms are understated. Prior **#5** remains incomplete. **Fix:** report input-handler readiness separately; calculate contractual TTI only once paint and handled input are both established. Add an assertion that TTI cannot precede paint, and regenerate the table.

4. **Major — Measurements run under scheduling conditions absent from normal use.**  
   [lib.rs:230](/Users/pedro/Documents/atrium-wt/M0-06b-ui-shell-layout-b/spikes/ui-shell/src-tauri/src/lib.rs:230). Harness mode holds `UserInitiated | LatencyCritical` for the entire process lifetime, including idle memory sampling. Normal launches never hold this activity. Activation and mount-based readiness address the original timeout, but the additional latency-critical activity introduces an uncontrolled difference into startup/frame comparisons and idle behavior. **Fix:** measure the normal foreground path without the activity, or provide paired runs demonstrating its effect. If needed solely for readiness diagnosis, scope and release it before measurements. Until then, qualify the results as activity-held harness measurements.

5. **Minor — Native vibrancy stops following live theme changes.**  
   [windows.ts:73](/Users/pedro/Documents/atrium-wt/M0-06b-ui-shell-layout-b/spikes/ui-shell/src/windows.ts:73). The native-call cache compares only the vibrancy boolean, although `set_vibrancy` also consumes `dark` to select its material. Switching Light ↔ Dark while background translucency remains enabled never updates that material. This leaves prior **#4** incomplete. **Fix:** cache and compare both enabled state and material/theme input. Test a theme switch while vibrancy stays enabled.

6. **Minor — One cold-matrix failure still aborts the remaining configurations.**  
   [measure.py:182](/Users/pedro/Documents/atrium-wt/M0-06b-ui-shell-layout-b/spikes/ui-shell/scripts/measure.py:182). `coldmatrix()` has no per-configuration error handling. An exception from `cold_once()` ends the entire cold matrix; the wrapper can only continue to another metric. This contradicts the claimed per-cell recovery and leaves prior **#2** partly fixed. **Fix:** retain failures by configuration, continue collecting other cells, emit their summaries, then return failure. Add a non-GUI test injecting one failed cell.

7. **Minor — The documented reproduction command collects three cold runs, not ten.**  
   [measure.py:390](/Users/pedro/Documents/atrium-wt/M0-06b-ui-shell-layout-b/spikes/ui-shell/scripts/measure.py:390). Only mode `cold` defaults to ten runs; `coldmatrix` defaults to three. The documented wrapper invokes `coldmatrix` without `--runs`, so it cannot reproduce the report’s ten-launch statistics by default. **Fix:** default both cold modes to ten, or explicitly pass ten from the wrapper and document the exact measured command.

8. **Minor — The forced-bring-back check does not require forced bring-back.**  
   [roundtrip.ts:54](/Users/pedro/Documents/atrium-wt/M0-06b-ui-shell-layout-b/spikes/ui-shell/src/roundtrip.ts:54). The check passes whenever ownership returns to main, even if `bringBack()` returns `"returned"`. Its assumption that the new listener has not loaded is timing-dependent. Also, absence from `ready_states` does not prove a window was destroyed—it might never have reported readiness. Prior **#13** still overclaims regression coverage. **Fix:** deterministically suppress the pop-out’s response, assert `"forced"`, and verify absence through the actual native window list.

9. **Minor — Font licence notices are not packaged as accessible artifacts.**  
   [index.tsx:4](/Users/pedro/Documents/atrium-wt/M0-06b-ui-shell-layout-b/spikes/ui-shell/src/index.tsx:4), [tauri.conf.json:34](/Users/pedro/Documents/atrium-wt/M0-06b-ui-shell-layout-b/spikes/ui-shell/src-tauri/tauri.conf.json:34). Both installed font packages contain OFL 1.1 copyright/licence notices, but the build imports only their CSS/font assets; neither `dist` nor the app’s resources contains accessible copies of those notices. **Fix:** bundle both complete notices in a licences directory or third-party-notices resource, and verify they survive packaging.