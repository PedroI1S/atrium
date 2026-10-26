# M0-06b: Floating spaces, opacity and pop-out windows

Task `M0-06b-ui-shell-layout-b` · first version GPT-6.1 Sol (2026-10-25) · fix rounds 1 and 2 Opus 5.5 (2026-10-26) · Tauri 2.12.5 + SolidJS 1.9 · macOS 27.0.1, Apple M4, built-in Retina panel at 60 Hz.

Code: `spikes/ui-shell/`. Screenshots: `docs/spikes/M0-06b-ui-shell-layout-b/`. Logs: `docs/spikes/M0-06b-ui-shell-layout-b/fix2-logs/` (the numbers in section 5) and `fix1-logs/` (round 1, superseded). The orchestrator's first, partial run (`raw-measurements/`, `measurements.md`) is superseded too.

**Status: done.** Layout B is built to `docs/design/ui-direction.md`; both opacity modes and pop-out windows work in the real app; all 13 findings of review 1 and all 9 of review 2 are fixed. The measurement matrix was re-run after round 2 under normal scheduling (section 5). Budgets: **TTI (contract §5: painted and input handled) medians 378–384 ms** in all six configurations, worst cases 387–411 ms (0–2 of 10 launches over 400 ms per configuration); **60 fps** in every frame test; **footprint** medians 133–140 MB with one window (under 150 MB) and 194–207 MB with three, which is within budget under D-028 (150 MB for one window, about 35 MB accepted per pop-out). One open item: in 5 of 18 footprint launches the WebKit GPU process grew by about 40 MB between the 10 s and 30 s samples, taking one window to 192–199 MB (section 5).

## Fix round 2 (Opus)

Review 2 (GPT-6.1 Sol, appended to `docs/agents/reviews/M0-06b-ui-shell-layout-b.md`) had 9 findings, all accepted. Each fix has a test that fails without it (checked by reverting the fix where the test is a unit test).

| # | Finding | Fix | Test |
|---|---|---|---|
| 1 | Closing a pop-out could lose an edit made while its earlier writes were still in flight | Close freezes user edits **synchronously** when it starts (`EditGate`), is idempotent (`makeCloser` reuses the first close), and drains **every** accepted write, including ones enqueued while draining (`WriteQueue.drain`), before `bring_back` | `sync.test.ts` "closing with a stalled write": an edit made after close starts is refused, the stalled and queued edits land before bring-back, a second close reuses the first; "drain also waits for writes enqueued while it waits" |
| 2 | Pop-out dropped the pending scroll position (the debounce flush was refused by the edit block) | User edits are blocked, but buffered state (the debounced scroll) is flagged `buffered` and still lands; `prepareMove` = block → flush buffered → drain, shared by pop-out and close | `sync.test.ts` "scroll then pop-out": scroll, pop out within 150 ms, the last position is written before the move; a late user edit is not |
| 3 | TTI was the keystroke alone and could precede paint | TTI = the later of first paint (second rAF after mount) and the handled keystroke; input readiness and first paint are reported separately; the harness refuses a report without a paint mark and asserts TTI ≥ both | `scripts/test_measure.py` (`Timing`); table regenerated |
| 4 | Harness held a `UserInitiated | LatencyCritical` activity for the whole process | Removed. Harness runs only activate the app and raise main, as a Dock/Finder launch does. All numbers in section 5 are from runs without it | `src/harness.test.ts` (no activity call in `lib.rs`) |
| 5 | The native vibrancy cache ignored the material, so Light ↔ Dark with vibrancy on kept the old material | `nativeCalls` compares enabled state **and** the dark/light material input; Rust records the material it applied | `window-model.test.ts` "re-applies vibrancy when the theme flips while vibrancy stays on" |
| 6 | One failing configuration aborted the whole cold matrix | `coldmatrix` records a failure per configuration, keeps running the others, prints every summary (failed ones marked), then fails | `scripts/test_measure.py` (`ColdMatrix`, injected failure) |
| 7 | The documented command ran 3 cold launches, not 10 | Both cold modes default to 10 (`default_runs`) | `scripts/test_measure.py` |
| 8 | The forced bring-back check passed even when the pop-out answered, and used readiness as proof of destruction | In the round-trip mode the database pop-out deterministically ignores return requests; the check waits until it is loaded, asserts `forced`, and checks the native window list (`window_labels`) | `measure.py roundtrip` in the real app: 7/7 pass |
| 9 | Font licence notices were not packaged | Both complete OFL 1.1 notices are bundle resources (`Contents/Resources/licenses/Instrument-Sans-OFL-1.1.txt`, `Geist-Mono-OFL-1.1.txt`), taken from the font packages | `src/licenses.test.ts`; `scripts/check-bundle.sh` verifies the built `.app` |

The UI did not change visibly in this round, so the screenshots are unchanged.

## 1. Fix round 1 (Opus)

The review (`docs/agents/reviews/M0-06b-ui-shell-layout-b.md`) had 13 accepted findings, plus Pedro's note that the look was underwhelming. Every bug fix has a test that fails without it.

| # | Finding | Fix | Test / evidence |
|---|---|---|---|
| 1 | 3-window and translucent runs timed out: readiness waited on rAF, pop-outs stacked on top of each other, cold/frames launches were never activated | Readiness = mounted + appearance applied, reported per window (`window_ready`), never rAF. Pop-outs cascade from main's origin (`build_popout`). In harness mode the app activates itself, brings main to the front and holds an `NSProcessInfo` activity (no App Nap). Remembered pop-outs are restored once main is interactive (`restore_windows`), not in `setup`. | All 6 configurations × 3 metrics completed (section 5). Discriminating check: every window reports whether rAF fired after it was ready (`rAF fired after ready` in each log); with the fixes, rAF fired after readiness in every window of all 42 three-window launches (cold, frames and footprint runs) (main is activated and frontmost, pop-outs cascade out from under it). The old setup (stacked, unactivated windows) was not re-run; readiness no longer depends on rAF either way. |
| 2 | Failures became silent 40/90 s timeouts | `boot()` and the harness track a stage; any failure writes `{kind:"error", stage, message, windows}` and exits. `measure.py` fails fast on an error report or a dead process, logs the app's stdout/stderr per run and prints its tail. The wrapper runs every cell, reports each failure, exits 1 at the end. Reports are written atomically. | Exercised while developing (a missing composer, a wrong pin); `FAILED (...)` lines name the stage. |
| 3 | `readyWindows` was tautological; only main's applied state was checked | `ready_states` returns each window's applied state (theme, alphas, native alpha read back from `NSWindow`, vibrancy, accessibility, error). `measure.py` checks the exact set of window labels and every window's values. | `verified per-window appearance` line in every log. |
| 4 | ~60 Hz `shell_patch` for scroll; focus re-ran the whole native chain | Scroll position is debounced (150 ms) and flushed before a pop-out or close. Native calls run only when their input changed (theme, vibrancy, alpha are diffed). The focus listener is gone: an `NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification` observer tells every window when Reduce transparency / Increase contrast change. | `sync.test.ts` (`debounced`); code review of `paintNative`. |
| 5 | TTI regression: probe after the gate, themes loaded twice, serial IPC before first render, vibrancy applied then cleared | The TTI probe runs right after mount, before waiting for other windows or native calls. Only the two Calm themes are parsed, once (`theme/calm.ts`; the M0-06 themes and VS Code importer are no longer evaluated at startup). One `startup_info` call carries the state snapshot, so tokens are painted synchronously before the first render. Rust applies the persisted (or pinned) vibrancy/alpha/theme to each window in `setup` / `build_popout`; the frontend only reconciles differences, in parallel, after mount. | Round 1 reported keystroke-only medians of 327–341 ms; review 2 #3 showed that undercounts TTI (see Fix round 2 and section 5). |
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
- **Moving out:** ⧉, or the palette. The view turns read-only, buffered state is flushed and accepted edits are drained, Rust moves ownership and builds the `WebviewWindow` (cascaded, same theme, native appearance applied before its page loads).
- **Moving back:** ↩ in the pop-out, closing it, or "Bring back" in main. The pop-out freezes, drains its edits and hands the view back (closing is idempotent); main asks politely and forces after 1.5 s if the pop-out does not answer (edits that pop-out never sent are lost, nothing else). Closing main quits the app and its pop-outs.
- **End-to-end check in the real app** (`scripts/measure.py roundtrip`, release build): all 7 checks pass. The chat moves to its pop-out; an edit typed in the pop-out and closed at once (without waiting for the write) survives together with the draft typed in main, and the window is gone from the native window list; main brings the editor back politely with its text unchanged; the database pop-out, loaded but deterministically not answering, is forced back (`forced`) and is destroyed (native window list). Log: `fix2-logs/roundtrip.txt`.
- **Edits during a move:** from the moment a pop-out or close starts, new user edits to that view are refused; the buffered scroll position and every write accepted earlier land before ownership moves.
- **Limits** (spike): one chat/editor/database view each; positions are cascaded, not remembered; drag-to-detach is not built; state lives for the process (only settings persist).
- **For M1:** replace the dummy snapshot with one Rust-side daemon client built on M0-05's `ClientHandle` that forwards projections to each window (D-022's thin UI); keep view placement and drafts in the per-workspace layout store (PLAN §3.20); add generation ids for rapid close/reopen, remembered bounds, and selection restore.

## 4. Opacity: mechanisms and caveats

| Mode | Mechanism | Native API |
|---|---|---|
| Background only (default) | Themes stay opaque; the loader emits `--surface-*-rgb` channels; components paint `rgb(channel / alpha)`. Frame `a`, cards `a + (1 − a) × 0.35` (or 1 with "Keep reading areas solid"). Native blur behind the window comes from `NSVisualEffectView` via `window-vibrancy`. | Needs `transparent: true` + `macOSPrivateApi` (cargo `macos-private-api`): rules out the Mac App Store. |
| Whole window | Surfaces opaque; the native window alpha is `a` (`NSWindow.setAlphaValue`, read back). Everything fades together, including traffic lights and the shadow. | Public AppKit. |

- One slider drives both modes; the mode is in Advanced. All windows update live; pop-outs follow main unless "Always opaque". Switching Light ↔ Dark with vibrancy on re-applies the matching material.
- Reduce transparency / Increase contrast (macOS) force opaque surfaces in every window, with a note in Appearance; changes are observed (no focus polling).
- `--window-blur` drives only the CSS `backdrop-filter` of in-window overlays (the palette). macOS gives no public control over the native blur radius, so "Blur strength" is shown disabled with that reason (CR-03).
- **Linux fallback (in code, untested here):** `vibrancySupported` is false off macOS, so alphas stay 1, native alpha is a no-op, and the slider/blur controls are disabled with an explanation. A compositor-aware path (alpha only where the compositor supports transparent windows) is M1 work; Linux verification remains a driver-owned deferral (contract §5).
- What the screenshots show: opacity shots are captured with ScreenCaptureKit over a synthetic wallpaper window (section 7). With the native frosted material under the frame, the light theme at 60 % reads as only slightly see-through: the material itself is light and dense. If Pedro wants the prototype's clearer glass, M1 can try a thinner material (`.hudWindow`/`.popover`) or none plus CSS blur; noted, not changed.

## 5. Measurements

**How.** Release build (`pnpm tauri build`) after fix round 2, Calm Dark, chat screen with the 5,000-message transcript, Changes panel. Every launch pins its configuration through the environment and never touches the user's settings. Harness runs **hold no process activity** (review 2 #4): the app activates itself and raises main, as a launch from the Dock or Finder does, and is otherwise scheduled like a normal launch. All GUI runs held `/usr/bin/lockf -k /tmp/atrium-gui-measure.lock` (run 10:10–10:19 and two footprint repeats after it, 2026-10-26). The machine was shared with other agents: the 1-minute load average fell from 20 to 10.5 during the cold runs and was 3–4 during frames; the cold matrix interleaves the six configurations launch by launch so load spreads evenly, and every summary records the load.

- **TTI** (contract §5): from `exec` (Python timestamp right before `Popen`) until the window **has painted and** handled its first input: the later of *first paint* (the second rAF after mount) and *input ready* (the composer handled a synthetic keystroke dispatched right after mount). Both are reported separately. 10 launches per configuration (`coldmatrix`).
- **Footprint**: `open -n` launch; the app plus the WebKit helpers whose responsible process is the app; `footprint -p` summed 10 s and 30 s after the ready report. Three launches per configuration (the matrix plus two repeats). RSS is in the logs.
- **Frames**: rAF deltas in main while scrolling the transcript at 1,500 and 20,000 px/s and sweeping both column widths at 0.8 Hz, pop-outs open behind main in 3-window runs. 3 runs per configuration. WebKit clamps `performance.now()` to 1 ms, so read fps, p99 and max, not ">16.7 ms" counts.

**Results:**

| Config | TTI median / worst | Over 400 ms | Input ready median / worst | First paint median | All windows ready | Footprint, 3 launches (10 s / 30 s) | Frames: scroll · fling · resize median fps (worst run); worst p99 / max |
|---|---|---|---|---|---|---|---|
| off, 1 window | 384 / 401 ms | 2 / 10 | 338 / 355 ms | 384 ms | 390 ms | 141/192, 138/132, 133/131 MB | 60.1 · 60.1 · 60.1 (60.0); 19 / 23 ms |
| background, 1 window | 378 / 395 ms | 0 / 10 | 330 / 351 ms | 378 ms | 384 ms | 133/132, 143/199, 133/131 MB | 60.1 · 60.1 · 60.0 (60.0); 19 / 23 ms |
| whole, 1 window | 379 / 394 ms | 0 / 10 | 334 / 347 ms | 379 ms | 385 ms | 140/130, 140/131, 142/198 MB | 60.1 · 60.1 · 60.0 (60.0); 19 / 22 ms |
| off, 3 windows | 381 / 411 ms | 1 / 10 | 334 / 366 ms | 381 ms | 519 ms | 204/194, 205/194, 207/259 MB | 60.1 · 60.1 · 60.1 (59.7); 19 / 35 ms |
| background, 3 windows | 378 / 405 ms | 2 / 10 | 333 / 358 ms | 378 ms | 520 ms | 208/194, 198/195, 197/193 MB | 60.1 · 60.1 · 60.1 (60.0); 20 / 22 ms |
| whole, 3 windows | 379 / 387 ms | 0 / 10 | 335 / 341 ms | 379 ms | 521 ms | 203/250, 207/196, 204/192 MB | 60.1 · 60.1 · 60.1 (60.0); 19 / 24 ms |

Logs: `fix2-logs/cold-matrix.log` (every launch, with the breakdown), `fix2-logs/frames-*.log`, `fix2-logs/rss-*.log` and `fix2-logs/rss-repeat-{2,3}/` (per-process lists).

**Against the budgets (PLAN §4; D-028 for windows):**
- **TTI < 400 ms: met at the median in every configuration** (378–384 ms), **not in every launch**: 5 of 60 launches reached 401–411 ms. Paint, not input, sets TTI in every launch: input is ready about 45 ms before the first frame (the 336 ms of round 1 was input readiness alone, as review 2 #3 found; the honest number is about 40 ms higher). Breakdown, stable across configurations: Rust `main` → JS start ≈ 245 ms (WebKit spawning its web process; the platform cost M0-06 also measured), JS start → mounted ≈ 72 ms (module evaluation, two theme parses, first render), mounted → input handled ≈ 15 ms, mounted → first paint ≈ 60 ms. Opacity mode and pop-outs do not change TTI: pop-outs are restored after main is interactive and are ready about 140 ms later. There is little slack: M1 must keep the first screen's JS small and track this in CI.
- **60 fps: met** in all 18 frame runs, opacity and pop-outs included (worst single frame 35 ms, in one resize run).
- **Footprint: within budget under D-028.** One window: medians 133–140 MB per configuration (budget 150 MB). Three windows: 192–208 MB in 16 of 18 samples, about +33 MB per pop-out (each pop-out has its own WebKit content process), inside the accepted 150 + 2 × 35 = 220 MB.
- **Open item, cause not found:** in 5 of 18 launches (spread over all three opacity modes and both window counts) the 30 s sample was 50–60 MB higher than the 10 s one: 192–199 MB in 3 of 9 one-window launches and 250–259 MB in 2 of 9 three-window launches. The per-process lists show it is the WebKit GPU process (23 → 62 MB) plus main's content process (+12 MB) while their RSS does not change, which points to graphics memory (IOSurfaces) being attributed, not app data. The other launches stayed flat. These runs no longer hold a process activity (review 2 #4); round 1's six launches, which did, showed no jump, but that sample is too small to call it the cause. M1 should sample footprint over a longer idle period to see whether this settles or recurs.
- RSS is 214–221 MB with one window and 304–315 MB with three, and moves with memory compression; footprint remains the metric to budget (as in M0-06).

**Commands** (from `spikes/ui-shell`, any shell including fish). This exact sequence produced the table (10 cold launches per configuration by default):

```text
pnpm tauri build
scripts/check-bundle.sh
/usr/bin/lockf -k /tmp/atrium-gui-measure.lock python3 scripts/measure.py roundtrip
/usr/bin/lockf -k /tmp/atrium-gui-measure.lock scripts/measure-layout-b.sh all /tmp/atrium-layout-b-measurements
python3 scripts/summarize.py /tmp/atrium-layout-b-measurements
```

The footprint repeats: `/usr/bin/lockf -k /tmp/atrium-gui-measure.lock scripts/measure-layout-b.sh rss /tmp/atrium-rss-repeat`. One cold configuration alone: `python3 scripts/measure.py cold --windows 3 --opacity background` (10 launches). Modes: `cold | coldmatrix | rss | frames | roundtrip`. Another binary: `env ATRIUM_MEASURE_BIN=/abs/path/to/atrium-ui-shell ...`. Harness self-tests (no app launched): `python3 scripts/test_measure.py`.

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
4. Restore secondary windows after the main window is interactive; each extra window costs about 35 MB (D-028).
5. Track TTI, footprint and frames in CI with this harness (`measure.py`), always under the shared GUI lock.
