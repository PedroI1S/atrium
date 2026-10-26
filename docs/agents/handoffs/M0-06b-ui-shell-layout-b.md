# Handoff: M0-06b-ui-shell-layout-b

First version: GPT-6.1 Sol (2026-10-25, partial). Fix rounds 1 and 2: Opus 5.5 (2026-10-26). **Status: done (final round).**

Report: [docs/spikes/M0-06b-ui-shell-layout-b.md](../../spikes/M0-06b-ui-shell-layout-b.md) (the "Fix round 2 (Opus)" section, the round-1 table in §1, and the measurements in §5). Branch `agent/sol/M0-06b-ui-shell-layout-b`, committed, not pushed.

## Done

**Fix round 2: all 9 review-2 findings fixed, each with a test that fails without the fix** (checked by reverting the fix for the unit-tested ones):
1. **Close race:** close freezes user edits synchronously and is idempotent; it drains every accepted write, including ones enqueued during the drain, before bring-back. Tests: `sync.test.ts` "closing with a stalled write", plus a drain test.
2. **Scroll lost on pop-out:** buffered state still flushes while user edits are blocked; pop-out and close share `prepareMove`. Test: `sync.test.ts` "scroll then pop-out".
3. **TTI per contract §5:** TTI is the later of first paint (second rAF) and the handled keystroke. Input readiness and paint are reported separately, and the harness asserts TTI ≥ both. Test: `scripts/test_measure.py`. The table is regenerated.
4. **Harness-only scheduling:** the harness no longer holds a `LatencyCritical` activity; the measurements were re-run without it. Guard test: `src/harness.test.ts`.
5. **Stale vibrancy material:** the native cache keys on enabled state plus the light/dark material. Test: `window-model.test.ts` (`nativeCalls`).
6. **Cold matrix abort:** `coldmatrix` continues past a failed configuration, prints every summary, then fails. Test: `scripts/test_measure.py` (injected failure).
7. **Wrong launch count:** both cold modes default to 10 launches, so the documented command reproduces the table. Test: `scripts/test_measure.py`.
8. **Forced bring-back check:** the database pop-out deterministically ignores return requests; the check asserts `forced` and verifies destruction through the native window list (`window_labels`). The real app passes 7/7 (`measure.py roundtrip`).
9. **Font licences:** both complete OFL 1.1 notices are bundle resources under `Contents/Resources/licenses/`. Test: `src/licenses.test.ts`. `scripts/check-bundle.sh` verifies them in the release and debug `.app`.

**Measurements re-run** (release, every GUI run under the shared lock, no activity held). Report §5 has the full table.
- **TTI:** medians 378–384 ms in all six configurations; 5 of 60 launches reached 401–411 ms.
- **Frames:** 60 fps everywhere.
- **Footprint:** one window, medians 133–140 MB per configuration. Three windows: 192–208 MB in 16 of 18 samples, within D-028 (150 MB + about 35 MB per pop-out).

Earlier rounds: the 13 review-1 findings, layout B rebuilt to the design, screenshots and CR-03 (see the report).

## Not done / open

- **Footprint jump, cause not found:** in 5 of 18 footprint launches the WebKit GPU process grew by about 40 MB between the 10 s and 30 s samples, with RSS unchanged. That took one window to 192–199 MB (3 of 9 launches) and three windows to 250–259 MB (2 of 9). It looks like graphics memory being attributed (IOSurfaces). Round 1, which still held the activity, showed no jump in 6 launches, but that sample is too small to call it the cause. M1 should sample footprint over a longer idle period.
- **TTI has little slack:** paint, not input, sets it. Rust main → JS start is about 245 ms (WebKit process spawn).
- **Still visual-only:** text size, tint, card gap, fonts, and the non-Appearance settings sections. Linux was not run (driver-owned deferral).
- **Screenshots:** unchanged in this round; the UI did not change visibly.

## Definition of Done results (2026-10-26, from `spikes/ui-shell`)

| Command | Result |
|---|---|
| `pnpm install && pnpm typecheck && pnpm test` | Exit 0. Vitest: 7 files, **141 tests passed**. `pnpm test` also runs `python3 scripts/test_measure.py`: 4 tests OK. |
| `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` | Exit 0, no warnings. |
| `cargo test --manifest-path src-tauri/Cargo.toml` | Exit 0, **12 passed**. |
| `pnpm tauri build` then `scripts/check-bundle.sh` | Exit 0. Release `.app` 4.82 MiB; both licence notices present. |
| `pnpm tauri build --debug` then `scripts/check-bundle.sh <debug .app>` | Exit 0. 29.41 MiB; notices present. |
| `/usr/bin/lockf -k /tmp/atrium-gui-measure.lock python3 scripts/measure.py roundtrip` | 7/7 pass. |
| `/usr/bin/lockf -k /tmp/atrium-gui-measure.lock scripts/measure-layout-b.sh all <dir>` (+ `rss` twice more) | All cells passed. |
| `bash -n scripts/*.sh`, `python3 -m py_compile scripts/*.py`, `git diff --check` | Exit 0. |

## Gotchas

- **Lock every GUI run:** `/usr/bin/lockf -k /tmp/atrium-gui-measure.lock <command>`.
- **Shared machine:** the load average was 10–20 during the cold runs. Interleaving keeps the configurations comparable, but absolute worst cases are load-sensitive.
- **Stray file:** a stray `src/ui/Sidebar 2.tsx` appeared in the worktree, an old copy of the deleted M0-06 sidebar created by a file-sync tool. It broke nothing that was committed. I moved it out of the tree, to `/private/tmp/claude-501/-Users-pedro-Documents-Harness/8e6bea1b-45aa-4dcf-8265-b67ec14db98b/scratchpad/stray/`. `dist/index 2.html` is the same kind of copy (git-ignored).
- **Processes:** I stopped every process I started; a final `pgrep` found no app, backdrop or harness process.

## Follow-ups

- **M1:** back the shared state with M0-05's `ClientHandle`; keep `sync.ts` (gate, drain, idempotent close) and its tests; track TTI, footprint (longer idle sampling) and frames in CI with this harness.
- **Pedro (fish), to look at it:**

```text
cd /Users/pedro/Documents/atrium-wt/M0-06b-ui-shell-layout-b/spikes/ui-shell
open "src-tauri/target/release/bundle/macos/Atrium UI Shell.app"
```
