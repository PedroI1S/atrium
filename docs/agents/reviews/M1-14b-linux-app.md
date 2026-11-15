# Review: M1-14b-linux-app
Reviewer: Gemini 4 Hydrogen (read-only) · 2026-11-15
## Verdict
**approve with changes**: D-040 private PSS, DRM fdinfo deduplication, the two old-harness bug fixes, L1 text/screenshots, startup marks, and M1-09a/M1-09b runs are sound, with a minor pop-out delta skew when `glycin-svg` exits between baseline and open and missing raw A/B and MiniBrowser files in the review bundle.

## Findings
1. [minor] **Pop-out `delta` omits exited `role:helpers` and undercounts pop-out cost by ~4.3 MiB**
   - Where: `app/scripts/startup_bench.py:295`, `delta = {k: after[k] - before[k] for k in after if k in before and isinstance(after[k], (int, float)) and k != "longestOverGateS"}`
   - Case: In `.orchestration/review-input/m1-14b/bench-1.json:3553-3601` (launch 1), `before` has `"processes": 6` including `"role:helpers": 4.275390625` (`glycin-svg`), `"role:ui": 58.93`, `"role:web": 73.21`, `"role:network": 22.28`, `"budgeted": 158.70`. During the 40 s baseline wait, `glycin-svg` exits, so `after` has `"processes": 4` and no `"role:helpers"` key (`"role:ui": 63.47`, `"role:web": 137.11`, `"role:network": 22.30`, `"budgeted": 222.88`).
   - Result: `delta` skips `role:helpers` because it is absent from `after`, so the printed role deltas (`ui +4.5, web +63.9, network +0.0` = `+68.4 MiB`) do not sum to the reported `budgeted` delta (`+64.2 MiB`), and the reported `+66.9 MiB` median pop-out cost is ~4.3 MiB lower than the actual `ui + web + network` increase.
   - Fix: Compute `role:*` deltas over `sorted({k for k in before if k.startswith("role:")} | {k for k in after if k.startswith("role:")})` defaulting missing roles to `0.0` (so `helpers -4.3` is visible alongside `budgeted`).

2. [minor] **Raw outputs for MiniBrowser floor and font-preload A/B runs are absent from the review input bundle**
   - Where: `docs/spikes/M1-linux-checks.md:210` (`124.5 MiB`), `docs/spikes/M1-linux-checks.md:285-293` (`345 / 362 ms` vs `402 / 425 ms`), `docs/spikes/M1-linux-checks.md:228` (`149.5–149.7 MiB` size-optimised binary).
   - Case: `.orchestration/review-input/m1-14b/` contains `bench-1.*`, `breakdown.*`, `floor.txt`, `mem-{nocomp,nodmabuf}.*`, `tti-{nocomp,nodmabuf,nosandbox}.*`, `smoke.json`, `dod.txt`, `checks/*`, `screens/*`, and `experiments-{1..4}.txt` (which record only load averages and PIDs, e.g. `experiments-2.txt:1-5`).
   - Result: The MiniBrowser floor (`124.5 MiB`), the size-optimised binary (`149.5–149.7 MiB`), and the interleaved font-preload A/B runs (`345 / 362 ms`) cannot be independently verified from raw JSON/text in the review bundle, including whether the A/B builds were alternated per launch or per round. (This is low-risk for D-041 only if the orchestrator re-runs `startup_bench.py` after landing the `app/index.html` preload, as planned.)
   - Fix: Copy the corresponding `*.txt`/`*.json` outputs into the evidence archive if they are still needed for the D-041 gate decision.

3. [nit] **Minor number mismatches in the spike report and handoff prose**
   - Where: `docs/spikes/M1-linux-checks.md:242,244` and `docs/agents/handoffs/M1-14b-linux-app.md:11`.
   - Case: In `.orchestration/review-input/m1-14b/bench-1.txt:44-48`, pop-out `network` deltas are `+0.0, -0.2, -0.2, +0.0, -0.2` (median `-0.2`, not `0.0` as in `M1-linux-checks.md:242`), and launch 1 `RSS` delta is `+199.4` (`199.36328125` in `bench-1.json:3597`, not `199.3` as in `M1-linux-checks.md:244`). In `M1-14b-linux-app.md:11`, `(−57 / −63)` is written right after comparing `377 / 391` to `345 / 362`, whereas `−57 / −63` is relative to the interleaved `402 / 425` baseline (`M1-linux-checks.md:287-288`).
   - Fix: Update those three figures in the report/handoff if edited.

## Numbers checked
- Idle private PSS `151.4–152.5` MiB (`158.7–160.0` MB), median `151.7` MiB (`159.1` MB): **matches** `.orchestration/review-input/m1-14b/bench-1.txt:33-42`.
- Idle per-process split (`ui 59.6/60.5/60.8`, `web 69.0/69.4/69.6`, `network 22.2/22.2/22.3`, `helpers 4.3/4.4/4.4`), full PSS `275.4/276.0/276.8`, RSS `453.5/454.2/455.2`, USS `207.2/208.0/208.5`, GPU `114.6/114.6/120.6`, max sample `165.4/165.8/167.8`: **matches** `bench-1.txt:33-41`.
- Idle breakdown at 30 s (`ui 61.0`, `web 70.4`, `network 22.2`, `GPU 114.6` and per-mapping top entries): **matches** `.orchestration/review-input/m1-14b/breakdown.txt:4-43`.
- WebKitGTK floor (`blank` 144.3–146.2, `text` 144.6–146.6, `atrium` 159.6–162.9 MiB): **matches** `.orchestration/review-input/m1-14b/floor.txt:1-9`. MiniBrowser `124.5 MiB`: **not in raw outputs** (only `experiments-2.txt:5` timestamp/load line).
- Experiments `WEBKIT_DISABLE_DMABUF_RENDERER=1` (mem `135.9–138.1` MiB, TTI `471 / 491` ms — report rounds `472` at line 225 vs `471` in `tti-nodmabuf.txt:3`), `WEBKIT_DISABLE_COMPOSITING_MODE=1` (mem `151.0–151.8` MiB, TTI `397 / 413` ms), `WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1` (TTI `403 / 431` ms): **matches** `mem-nodmabuf.txt:4-7`, `tti-nodmabuf.txt:3`, `mem-nocomp.txt:4-7`, `tti-nocomp.txt:3`, `tti-nosandbox.txt:3`.
- Pop-out cost (`+66.9` MiB median, `64.2–69.8`; `web +66.5`, `ui +4.9`, `full PSS +69.9`, `RSS +201.8`, `GPU +68.2`, ready `229 ms`): **matches** `bench-1.txt:43-49` (except `network` median `-0.2` vs `0.0` and `RSS` min `199.4` vs `199.3`).
- Startup phases (30 launches, TTI `377 / 391` ms, `spawn → main` `30 / 40`, `main → setup` `35 / 41`, `bridgeStarted → windowBuilt` `115 / 122`, `windowBuilt → navigationStart` `86 / 91`, `main → first JS` `252 / 257`, `first JS → mounted` `35 / 42`, `mounted → firstPaint` `88 / 96`, `spawn → TTI` `406 / 424`): **matches** `bench-1.txt:3-31`.
- Font-preload A/B (`345 / 362` ms vs `402 / 425` ms): **not in raw outputs** (only `experiments-2.txt` / `experiments-3.txt` round markers).
- M1-09a & M1-09b checks (`live_check` 12/12, `chat_check` 12/12, `screenshots` 28 PNGs, `stream_bench` 3 runs p99 `21/22/21` ms, long frames `0.3/0.4/0.2 %`, GPU `423.9/422.0/415.9` MiB, `nodmabuf` streaming `61.6` fps / p99 `115` ms / `19.8 %` long / `1557` dropped / web `210.2` MiB): **matches** `checks/live_check.txt`, `checks/chat_check.txt`, `checks/screenshots.txt`, `checks/stream_bench.txt`, `checks/stream_bench_nodmabuf.txt`.

## Screenshots
- `.orchestration/review-input/m1-14b/screens/settings-advanced-light.png` and `settings-advanced-dark.png`: L1 text is exact in both themes ("Atrium uses opaque windows on Linux." under Appearance; "Atrium uses opaque windows on Linux. This only affects overlays." under Overlays → Blur strength; no mention of a compositor).
- Inspected `main-light.png`, `main-dark.png`, `main-with-popout.png`, `popout.png`, `chat-streaming-dark.png`, `chat-approval-light.png`, `incompatible.png`, `inbox-items-dark.png`: all clean, legible, and properly laid out, with no clipping or unreadable text.

## Suspected, not shown
- None.

## Tried and held
- **D-040 measurement & DRM fdinfo**: `linuxmem.py:50-86` correctly parses `/proc/<pid>/smaps` per mapping (`private_pss = pss - lib_clean`), avoiding `smaps_rollup`'s `Private_Clean + Private_Dirty` (USS). `app_tree` (`linuxmem.py:162-172`) walks the process tree skipping `atriumd`, and `gpu_kb` (`linuxmem.py:118-133`) deduplicates by `(drm-pdev, drm-client-id)` while ignoring `drm-*-cycles-*` counters.
- **Two old-harness bugs**: `git show 9ac5cdd:app/scripts/startup_bench.py:97-101` confirms it summed `Private_Clean + Private_Dirty` from `smaps_rollup` (USS), and lines 142–145 created `home / "logs"` with default mode `0755`, which `crates/atrium-host/src/lifecycle.rs:403-409` rejects via `secure_dir`. Moving logs to `work / "bench-logs"` (`startup_bench.py:404-408`) and checking `daemons_of(home)` per idle launch (`startup_bench.py:270`) is sound (`bench-1.json` shows `"daemonRunning": true` for all 9 launches).
- **Startup marks & owned paths**: `app/src-tauri/src/lib.rs:115,187,860,892,910,927` and `app/src/{startup-mark.ts,harness.ts,index.tsx,platform.ts,popout-bench.ts}` add only measurement marks and bench hooks; `startup_info` remains unchanged for pop-outs, and no non-measurement `app/src/**` changes were made.
- **Rust tests**: `cargo test -p atrium-app` ran and passed after all edits (45 unit tests in `src/lib.rs`, 2 integration tests in `tests/delivery_shapes.rs`).

## Test gaps
- `app/scripts/test_linuxmem.py` covers `kind_of`, `parse_smaps`, `parse_fdinfo` (single fd), and `parse_ppid`, but does not unit-test `gpu_kb` deduplication across multiple fds/processes sharing a `(drm-pdev, drm-client-id)` or `app_tree` excluding `atriumd` and its subtree.
- Python unit tests (`test_linuxmem.py`) were not executed in this review session because `python3` is outside the sandbox allowlist (reported passing 7/7 in `.orchestration/review-input/m1-14b/dod.txt:35-39`).


## Orchestrator's notes (2026-11-15)
- **Finding 1, fixed** on the branch (`startup_bench.py`: a role present on one side only counts as 0 on the other). From `bench-1.json`: the SVG loader exits in **all five** pop-out launches (−4.3 MiB each), so the pop-out's own cost (UI + web + network) is **71.2 MiB** (68.5–74.0), not 66.9; the report and handoff say so.
- **Finding 2, closed by the orchestrator:** the raw files exist in the worker's `.orchestration/m1-14b/b2`–`b4`, which were not copied into the review bundle. MiniBrowser 124.5 / 124.5 / 124.4 MiB (`b2/minibrowser.txt`); size-optimised binary 149.5–149.7 MiB (`b4/mem-small-*`); font preload A/B alternated per round of 10 launches, 3 rounds at load 3.7–4.8: base 403 / 400 / 408 ms median, preload 343 / 343 / 347, JS preload 391 / 410 / 412 (`b3/`). The gain is well beyond the noise.
- **Finding 3, fixed:** network median −0.2, RSS min 199.4, the A/B baseline (402 / 425) named in the handoff, DMA-BUF TTI 471.
