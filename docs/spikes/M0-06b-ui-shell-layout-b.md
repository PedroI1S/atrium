# M0-06b: Floating spaces, opacity and pop-out windows

Task `M0-06b-ui-shell-layout-b` · first version GPT-6.1 Sol (2026-10-25) · fix round 1 Opus 5.5 (2026-10-26) · Tauri 2.12.5 + SolidJS 1.9 · macOS 27.0.1, Apple M4, built-in Retina panel at 60 Hz.

Code: `spikes/ui-shell/`. Screenshots: `docs/spikes/M0-06b-ui-shell-layout-b/`. Logs of this round: `docs/spikes/M0-06b-ui-shell-layout-b/fix1-logs/`. The orchestrator's first, partial run (`raw-measurements/`, `measurements.md`) is superseded by section 5.

**Status: done.** Layout B is built to `docs/design/ui-direction.md`, both opacity modes and pop-out windows work in the real app, the full measurement matrix ran (section 5), and all 13 review findings are fixed (section 1). Budgets: TTI medians 327–341 ms in all six configurations (under 400 ms; worst cases 359–472 ms, two configurations had 2 of 10 launches over), 60 fps in every frame test, footprint 125–134 MB with one window (under 150 MB) and **194–207 MB with three windows (over 150 MB: about +33 MB per pop-out)**. The three-window budget is a question for Pedro (section 5).

## 1. Fix round 1 (Opus)

The review (`docs/agents/reviews/M0-06b-ui-shell-layout-b.md`) had 13 accepted findings, plus Pedro's note that the look was underwhelming. Every bug fix has a test that fails without it.

| # | Finding | Fix | Test / evidence |
|---|---|---|---|
| 1 | 3-window and translucent runs timed out: readiness waited on rAF, pop-outs stacked on top of each other, cold/frames launches were never activated | Readiness = mounted + appearance applied, reported per window (`window_ready`), never rAF. Pop-outs cascade from main's origin (`build_popout`). In harness mode the app activates itself, brings main to the front and holds an `NSProcessInfo` activity (no App Nap). Remembered pop-outs are restored once main is interactive (`restore_windows`), not in `setup`. | All 6 configurations × 3 metrics completed (section 5). Discriminating check: every window reports whether rAF fired after it was ready (`rAF fired after ready` in each log); with the fixes, rAF fired after readiness in every window of all 42 three-window launches (cold, frames and footprint runs) (main is activated and frontmost, pop-outs cascade out from under it). The old setup (stacked, unactivated windows) was not re-run; readiness no longer depends on rAF either way. |
| 2 | Failures became silent 40/90 s timeouts | `boot()` and the harness track a stage; any failure writes `{kind:"error", stage, message, windows}` and exits. `measure.py` fails fast on an error report or a dead process, logs the app's stdout/stderr per run and prints its tail. The wrapper runs every cell, reports each failure, exits 1 at the end. Reports are written atomically. | Exercised while developing (a missing composer, a wrong pin); `FAILED (...)` lines name the stage. |
| 3 | `readyWindows` was tautological; only main's applied state was checked | `ready_states` returns each window's applied state (theme, alphas, native alpha read back from `NSWindow`, vibrancy, accessibility, error). `measure.py` checks the exact set of window labels and every window's values. | `verified per-window appearance` line in every log. |
| 4 | ~60 Hz `shell_patch` for scroll; focus re-ran the whole native chain | Scroll position is debounced (150 ms) and flushed before a pop-out or close. Native calls run only when their input changed (theme, vibrancy, alpha are diffed). The focus listener is gone: an `NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification` observer tells every window when Reduce transparency / Increase contrast change. | `sync.test.ts` (`debounced`); code review of `paintNative`. |
| 5 | TTI regression: probe after the gate, themes loaded twice, serial IPC before first render, vibrancy applied then cleared | The TTI probe runs right after mount, before waiting for other windows or native calls. Only the two Calm themes are parsed, once (`theme/calm.ts`; the M0-06 themes and VS Code importer are no longer evaluated at startup). One `startup_info` call carries the state snapshot, so tokens are painted synchronously before the first render. Rust applies the persisted (or pinned) vibrancy/alpha/theme to each window in `setup` / `build_popout`; the frontend only reconciles differences, in parallel, after mount. | Section 5: TTI medians 327–341 ms (was 396–419 ms for one window and timeouts for three). |
| 6 | A rejected patch latched `writeFailure`; close and "Move back" threw; the pop-out could not be closed | Ownership rejections (`not-owner:`) are dropped, never latched (`WriteQueue`). Closing is `flush → bring_back`, and if bring-back fails the window is destroyed anyway (`closeOwnedView`); Rust returns the view on destruction. | `sync.test.ts`: WriteQueue and closeOwnedView cases; real app: `measure.py roundtrip` (section 3). |
| 7 | Opacity slider: synchronous non-atomic file write and an unbounded paint queue per event; arbitrary keys persisted | Frontend: optimistic overlay, settings patches coalesced (one in flight), native paint latest-wins. Rust: whitelisted keys and values (`apply_settings_patch`), persistence on a background thread, debounced 250 ms and atomic (temp + rename), flushed on exit. | `sync.test.ts` (latestWins, coalescing); `shell.rs` tests (whitelist, coalesced persister, atomic write). |
| 8 | Ownership holes: edits during the switch lost; main could not force a hung pop-out back; `IndexMut` inserted null | The view is read-only from the moment a pop-out starts until it lands (`moving`, `canEdit`). Main asks the pop-out politely and forces it after 1.5 s (`bringBackFromMain` + `force_bring_back`). View lookups use `get_mut`; an unknown view is an error and nothing is inserted. | `sync.test.ts` (bringBackFromMain); `shell.rs` (`patching_an_unknown_view_does_not_insert_it`, `only_the_owner_can_patch_a_view`); real app: forced bring-back in `roundtrip`. |
| 9 | Advanced opened in the 46 px strip when the panel was minimized | The right column is a pure function of screen, panel state and Advanced (`layout.ts` `rightColumn`); in Settings, Advanced always takes `--advanced-width` (400 px). | `layout.test.ts` (fails with the old rule). |
| 10 | Inert `--window-opacity`/`--window-blur`; whole-window via `NSWindow.alphaValue` contradicts ui-direction §5; no blur strength; bare `0.35`; no accessibility observer; Linux opaque-only | Documented in CR-03 and section 6. `READING_AREA_LIFT = 0.35` is a named constant. Accessibility observer added (row 4). Blur strength is shown disabled with the reason. | — |
| 11 | Conformance test missed `windows.ts`/`window-model.ts`/`state.ts` and bare fractions; `base.css` duplicated Calm values | The test now scans every CSS file, all TSX, and the TS modules that paint or size anything (`windows.ts`, `window-model.ts`, `state.ts`, `sync.ts`, `layout.ts`, `bench.ts`, ...). New rules: bare fractions (CSS values that are not `calc()` multipliers; TS literals that are not named `UPPER_CASE` constants), lengths/durations in TS. A new test fails if `base.css` defines a token the Calm themes own; the duplicates were removed. | `tokens-conformance.test.ts` (with self-tests for each rule). |
| 12 | Layout B gaps: settings tabs, attention dot, `+` topic, raised selected card, pop-out chrome, per-pop-out opacity; pop-out traffic lights | All done (section 2). Pop-out traffic lights are positioned by `traffic_light_position` to sit centred in the 44 px pop-out bar. | Screenshots. |
| 13 | Tests overclaimed; no `shell_patch` ownership or close/bring-back race tests; stale report; 3-window footprint over budget | Tests renamed and rewritten against pure functions (`shell.rs`: 11, `lib.rs`: 1; TS: 130, was 95). The pop-out lifecycle is also checked end to end in the real app (`measure.py roundtrip`). This report replaces the old one. The 3-window footprint question is for Pedro (section 5). | — |

## 2. What was built: layout B

The spike now looks like the "B · Floating spaces" prototype; it was ported view by view from the prototype's source onto the Calm tokens. Everything is calm, near-monochrome and token-only (the conformance test is strict).

- **Frame and cards** (ui-direction §2): the sidebar sits on the window frame (`surface-0`); the main card and the right column are floating cards (`surface-1`, 11 px radius, `--shadow-card`, 8 px gap). Columns are resizable by dragging the gaps, within token bounds.
- **Sidebar** (§2.1): hide button, ⌘K bar, Inbox tile (needs-you count in amber) and Home tile, topic title with ⋯, "New chat in this topic", chats with status dot, title and age; **the selected chat is a small raised card**. Topic switcher: monogram circles, **amber attention dot** on topics with something that needs you, **`+` for a new topic**, Settings. ⌘B collapses it to a strip (show, Inbox with dot, topic circles).
- **Main card**: Chat (title, state pill, worktree chip, ⧉; 640 px measure; user bubbles; plain agent text; one-line tool steps that open the matching panel tab; the approval card; the composer with approval-mode and agent chips); Inbox (ranked groups, inline approval/answer buttons, filter, footer hints, preview in the panel); Editor (file tabs, centred path, hint chip, a dummy highlighted editor whose text is shared state, status bar with the AI badge); Database (query + grid).
- **Right panel** (§2.2): tabs depend on the main card (Changes · Checks · Files · Terminal; Chat · Files · Changes for the editor at 380 px; Preview for the inbox). » minimizes it to a 46 px strip with attention dots; open/tab are remembered per main-card kind.
- **Settings** (§3): a card in place of the main card with **section tabs** (General · Appearance · Agents · Approvals · Notifications · More) and **search across all sections, Advanced included** (a hit opens the section, opens Advanced when needed, and highlights the row). Sections come from a small registry (`settings-registry.ts`: `essentials[]` capped at 5, overflow moves to Advanced). Each section ends with "Advanced \<section\> ›", which opens the 400 px **Advanced card** in the panel's place, with a breadcrumb and a close button; it closes when you leave the section or press Esc.
- **Pop-out chrome** (§3): traffic lights, title, **status pill**, 📌 keep-on-top, ↩ move back; the view in a card below. The main window shows "This chat is open in its own window · Show window / Bring back".
- **Per-pop-out opacity**: Advanced → "Popped-out windows: Same as main / Always opaque" (persisted; applied live to every pop-out).
- **Fonts**: Instrument Sans and Geist Mono are bundled from `@fontsource` (OFL, Latin subset, woff2), never loaded from a CDN.

Real vs visual-only: theme (Light/Dark/System), opacity, blur, opacity mode, solid reading areas and pop-out opacity are real and persisted. Text size, tint, card gap, fonts and the non-Appearance sections are visual stand-ins (local state, no effect); tint needs the Warm/Cool sibling theme files (M1).

Removed: the M0-06 direction components (tab strip, presets, Corvu layout, file preview, home pane). The comparison themes and the VS Code importer stay as loader test fixtures.

## 3. Pop-out windows

- **One owner per view.** `shell.rs` holds a typed, revisioned `ShellState` (settings + chat/editor/database views). Every change goes through Rust and is broadcast to all windows; a window subscribes before it reads its snapshot and drops anything older than what it has.
- **Moving out:** ⧉, or the palette. The view turns read-only, pending edits are flushed, Rust moves ownership and builds the `WebviewWindow` (cascaded, same theme, native appearance applied before its page loads).
- **Moving back:** ↩ in the pop-out, closing it, or "Bring back" in main. The pop-out flushes its edits and hands the view back; main asks politely and forces after 1.5 s if the pop-out does not answer (edits that pop-out never sent are lost, nothing else). Closing main quits the app and its pop-outs.
- **End-to-end check in the real app** (`scripts/measure.py roundtrip`, release build): all 5 checks pass: the chat moves to its pop-out; an edit typed in the pop-out and closed at once (without waiting for the write) survives, together with the draft typed in main; main brings the editor back politely with its text unchanged; main recovers the database view from a pop-out that cannot answer yet (forced after 1.5 s). Log: `fix1-logs/roundtrip.txt`.
- **Limits** (spike): one chat/editor/database view each; positions are cascaded, not remembered; drag-to-detach is not built; state lives for the process (only settings persist).
- **For M1:** replace the dummy snapshot with one Rust-side daemon client built on M0-05's `ClientHandle` that forwards projections to each window (D-022's thin UI); keep view placement and drafts in the per-workspace layout store (PLAN §3.20); add generation ids for rapid close/reopen, remembered bounds, and selection restore.

## 4. Opacity: mechanisms and caveats

| Mode | Mechanism | Native API |
|---|---|---|
| Background only (default) | Themes stay opaque; the loader emits `--surface-*-rgb` channels; components paint `rgb(channel / alpha)`. Frame `a`, cards `a + (1 − a) × 0.35` (or 1 with "Keep reading areas solid"). Native blur behind the window comes from `NSVisualEffectView` via `window-vibrancy`. | Needs `transparent: true` + `macOSPrivateApi` (cargo `macos-private-api`): rules out the Mac App Store. |
| Whole window | Surfaces opaque; the native window alpha is `a` (`NSWindow.setAlphaValue`, read back). Everything fades together, including traffic lights and the shadow. | Public AppKit. |

- One slider drives both modes; the mode is in Advanced. All windows update live; pop-outs follow main unless "Always opaque".
- Reduce transparency / Increase contrast (macOS) force opaque surfaces in every window, with a note in Appearance; changes are observed (no focus polling).
- `--window-blur` drives only the CSS `backdrop-filter` of in-window overlays (the palette). macOS gives no public control over the native blur radius, so "Blur strength" is shown disabled with that reason (CR-03).
- **Linux fallback (in code, untested here):** `vibrancySupported` is false off macOS, so alphas stay 1, native alpha is a no-op, and the slider/blur controls are disabled with an explanation. A compositor-aware path (alpha only where the compositor supports transparent windows) is M1 work; Linux verification remains a driver-owned deferral (contract §5).
- What the screenshots show: opacity shots are captured with ScreenCaptureKit over a synthetic wallpaper window (section 7). With the native frosted material under the frame, the light theme at 60 % reads as only slightly see-through: the material itself is light and dense. If Pedro wants the prototype's clearer glass, M1 can try a thinner material (`.hudWindow`/`.popover`) or none plus CSS blur; noted, not changed.

## 5. Measurements

**How.** Release build (`pnpm tauri build`), Calm Dark, chat screen with the 5,000-message transcript, Changes panel. Every launch pins its configuration through the environment and never touches the user's settings. Runs were serialized with the other agents' GUI work through `/usr/bin/lockf -k /tmp/atrium-gui-measure.lock`; this run started after the M0-07 overlap the orchestrator flagged (02:35–02:41), so none of these numbers come from that window. The machine was not idle: other agents were working (1-minute load average 4.0–5.6 during the cold runs, about 8 earlier), and Messages used ~50 % of a core throughout. A first, sequential cold run during heavier load gave medians of 340–506 ms that moved with the load rather than with the configuration (kept as `fix1-logs/sequential-cold-*.txt`); the harness now interleaves the six configurations launch by launch (`coldmatrix`), so load spreads evenly, and records the load average.

- **TTI** (contract §5): from `exec` (Python timestamp right before `Popen`) to the composer handling a keystroke; the probe runs right after mount, before waiting on other windows. Also reported: ⌘K palette painted, first rAF paint, and "all windows ready" (3-window runs: until both restored pop-outs are mounted and painted). 10 launches per configuration, interleaved across the six configurations (`measure.py coldmatrix`); the first round after the build includes a cold file cache.
- **Footprint**: `open -n` launch; app + the WebKit helpers whose responsible process is the app; `footprint -p` summed, sampled 10 s and 30 s after the ready report. RSS is listed too.
- **Frames**: rAF deltas in main while scrolling the transcript at 1,500 and 20,000 px/s and sweeping both column widths at 0.8 Hz, with pop-outs open behind main in 3-window runs. 3 runs per cell. WebKit clamps `performance.now()` to 1 ms, so a 60 Hz frame reads as 16 or 17 ms; ">16.7 ms" counts are not meaningful, use fps, p95/p99 and max.

**Results** (median / worst case):

| Config | TTI (keystroke) median / worst | Launches over 400 ms | All windows ready (median) | Footprint (10 s / 30 s) | Frames: scroll 1.5k · fling 20k · resize (median fps; worst p99 / max) |
|---|---|---|---|---|---|
| off, 1 window | 329 / 466 ms ⚠️ | 2 / 10 | 384 ms | 132 / 126 MB ✅ | 60.1 · 60.1 · 60.1 fps; p99 ≤ 19 ms, max 26 ms ✅ |
| background, 1 window | 336 / 389 ms ✅ | 0 / 10 | 389 ms | 133 / 125 MB ✅ | 60.1 · 60.1 · 60.1 fps; p99 ≤ 19 ms, max 28 ms ✅ |
| whole, 1 window | 328 / 362 ms ✅ | 0 / 10 | 383 ms | 134 / 131 MB ✅ | 60.1 · 60.1 · 60.1 fps; p99 ≤ 19 ms, max 23 ms ✅ |
| off, 3 windows | 327 / 359 ms ✅ | 0 / 10 | 528 ms | 207 / 195 MB ❌ | 60.1 · 60.1 · 60.1 fps; p99 ≤ 19 ms, max 23 ms ✅ |
| background, 3 windows | 341 / 397 ms ✅ | 0 / 10 | 537 ms | 194 / 194 MB ❌ | 60.1 · 60.1 · 60.1 fps; p99 ≤ 19 ms, max 24 ms ✅ |
| whole, 3 windows | 332 / 472 ms ⚠️ | 2 / 10 | 528 ms | 203 / 194 MB ❌ | 60.1 · 60.1 · 60.0 fps (worst run 59.1); p99 ≤ 19 ms, max 59 ms ✅ |

Other columns per configuration (palette painted, first rAF paint, RSS, per-process lists, per-run rows) are in `fix1-logs/` (`cold-matrix.log`, `rss-*.log`, `frames-*.log`). Frame runs: 3 per configuration; footprint: one launch per configuration, two samples.

**Against the budgets (PLAN §4):**
- **TTI < 400 ms: met at the median in every configuration, not in every launch.** Worst cases of 466 and 472 ms (2 of 10 launches in two configurations) came with the load average above 5. The breakdown is stable across configurations: Rust `main` → JS start ≈ 245 ms (WebKit spawning its web process; the same platform cost M0-06 measured), JS start → mounted ≈ 73 ms (module evaluation, two theme parses, first render), mounted → keystroke handled ≈ 10 ms. Opacity mode and pop-outs cost nothing here: pop-outs are restored after main is interactive and finish 150–200 ms later ("all windows ready" ≈ 530 ms). Versus the first version: 396–419 → 327–341 ms medians for one window; three windows went from timing out to the same TTI as one.
- **60 fps: met** in all 18 frame runs (scroll at 1.5k and 20k px/s through 5,000 messages, column-resize sweep), opacity and pop-outs included. p95 18–19 ms is the 1 ms timer clamp around 16.7 ms; the one 59 ms max is a single hitch in one resize run.
- **Footprint < 150 MB: met with one window (125–134 MB), missed with three (194–207 MB).** Each pop-out is its own `WKWebView` and WebKit web-content process, about 30 MB of footprint plus app-side overhead, so +33 MB per window. Sharing one web process between windows is not something WKWebView exposes through Tauri today. **Question for Pedro:** is the 150 MB budget for the app with one window (as it reads today), or must it hold with pop-outs open? If the latter, M1 must cap or lazily create pop-outs, or the budget becomes "150 MB + ~35 MB per extra window".
- RSS (the literal budget word) is 165–212 MB for one window and swings by 40–180 MB between samples (72–293 MB for three windows) with memory compression; as M0-06 found, footprint is the stable metric.

**Commands** (from `spikes/ui-shell`, any shell including fish):

```text
pnpm tauri build
/usr/bin/lockf -k /tmp/atrium-gui-measure.lock scripts/measure-layout-b.sh all /tmp/atrium-layout-b-measurements
python3 scripts/summarize.py /tmp/atrium-layout-b-measurements
/usr/bin/lockf -k /tmp/atrium-gui-measure.lock python3 scripts/measure.py roundtrip
```

One cell: `python3 scripts/measure.py cold --windows 3 --opacity background --runs 10` (modes `cold | rss | frames | roundtrip`). Fewer launches per cell: `env ATRIUM_MEASURE_RUNS=3 scripts/measure-layout-b.sh cold /tmp/out`. Another binary: `env ATRIUM_MEASURE_BIN=/abs/path/to/atrium-ui-shell ...`.

## 6. CR-02 feedback

CR-02 works as written; the spike implements it without changing the frozen contract:
- Runtime tokens (`--window-alpha-chrome`, `--window-alpha-content`, `--window-opacity`, `--window-blur`) are reserved: a theme defining one (in any layer) fails to load.
- Surface channels (`--surface-0/1/2/overlay-rgb`) are emitted by the loader and are themselves reserved; surfaces must be opaque six-digit hex.
- State colours are checked as text on `surface-0..2`; the Calm themes pass unchanged.
- Hover/selected/state backgrounds, shadows, scrim and glass blur are used as the Calm themes' component tokens.

Amendments (CR-03 in `docs/agents/change-requests.md`): whole-window mode uses the native window alpha, not CSS `opacity`; native blur strength is not adjustable on macOS; say "six-digit hex" and "reserved derived names"; move `advanced-width`, `panel-width-editor` and the drag bounds into the themes; observe the OS accessibility setting. AA is checked against opaque surfaces only: no check can guarantee contrast over an arbitrary desktop at low alpha, which is why the 30 % floor, "Keep reading areas solid" and the accessibility override matter.

## 7. Screenshots

`docs/spikes/M0-06b-ui-shell-layout-b/` (release build, pinned settings, 1.6 MB in total):

| File | What |
|---|---|
| `chat-light.jpg`, `chat-dark.jpg` | Chat, panel on Changes |
| `settings-advanced-light.jpg`, `settings-advanced-dark.jpg` | Settings → Appearance with the Advanced card |
| `popout-chat-dark.jpg`, `popout-placeholder-light.jpg` | The chat in its own window, and main's placeholder |
| `inbox-light.jpg`, `editor-dark.jpg` | Inbox with preview; editor with the chat panel |
| `opacity60-background-{light,dark}.jpg` | 60 %, background only (frame 0.6, cards 0.74, native blur) |
| `opacity60-whole-{light,dark}.jpg` | 60 %, whole window (native alpha) |

How (`scripts/screenshots.sh`): opaque shots capture only the app's window (`screencapture -l`) and composite it over a synthetic wallpaper. Opacity shots open a full-screen synthetic wallpaper window and capture with ScreenCaptureKit through a filter that includes only that window and the app's windows, so the real vibrancy and native alpha show and nothing else on the screen can appear.

## 8. Recommendation for M1

1. Keep layout B as built here: floating cards on token-driven paint, the settings registry with essentials/Advanced, and the per-kind panel memory.
2. Keep the Rust single writer for window state and back it with one daemon client (section 3). Keep the ordering helpers (`sync.ts`) and their tests.
3. Keep both opacity modes: channel tokens for background mode, native alpha for whole-window. Record the `macOSPrivateApi` distribution decision (ui-direction §9.5).
4. Restore secondary windows after the main window is interactive; budget each extra window (section 5).
5. Track TTI, footprint and frames in CI with this harness (`measure.py`), always under the shared GUI lock.
