# M0-06 UI shell: spike report

Task `M0-06-ui-shell` · Sonnet 5.5 · Tauri 2.12 + SolidJS 1.9 · macOS 27.0.1 on an Apple M4 (MacBook, 2560x1664 Retina panel, 60 Hz)

Code: `spikes/ui-shell/` (standalone `src-tauri/Cargo.toml` + `package.json` + `pnpm-lock.yaml`). Screenshots: `docs/spikes/M0-06-ui-shell/`.

## 1. What was built

A clickable mock of Atrium's window with dummy data:

| Area | What is there |
|---|---|
| Chrome (macOS) | `titleBarStyle: "Overlay"`, `hiddenTitle`, `trafficLightPosition {x:18,y:21}`, `transparent: true`, `data-tauri-drag-region` on the tab strip, native vibrancy via `window-vibrancy` (0.8, the same version Tauri pulls in), CSS `backdrop-filter` on glass surfaces |
| Layout | Left sidebar (Inbox with a badge, Home, workspaces to threads with attention-state dots), draggable tab strip (solid-dnd, reorder + close + new), Corvu resizable panes: chat next to a file preview, or two threads side by side (tab "Flaky test + socket"), right panel tabs Review / Diff / Browser / Terminal / Database (Kobalte Tabs, placeholders), composer, Home tab |
| Palette | Kobalte Dialog around an inline ARIA combobox. `⌘K` opens it; `⌘J` jumps to the next thread needing attention; `⌘B` sidebar; `⌘⇧L` dark/light. Dummy actions raise a toast. |
| Presets | *Default / Review / Focus*, from the palette. Sizes animate on a preset switch and never during a drag. |
| Transcript | 5,000 dummy messages (seeded, user / assistant / tool / approval), virtualized with `@tanstack/solid-virtual`, variable heights measured live |
| Tokens | `src/theme/` loader, validator, runtime applier, VS Code importer; 6 TOML themes + 1 imported VS Code theme |
| Reduced motion | `@media (prefers-reduced-motion: reduce)` sets `transition: none` and `animation: none` on everything. Checked in a browser with emulated media (tab transition 60 ms to 0 s, pulse animation to none); not re-checked inside WKWebView. |

Escalation check: **overlay title bar + vibrancy work together** in Tauri 2 on macOS 27.0.1; no escalation needed. Evidence: the traffic lights sit inline with the tab strip in every screenshot, `apply_vibrancy` returns `Ok`, and the Glass sidebar renders as (58,58,68) although its CSS colour is `rgba(15,17,32,0.5)` (over black that would be about (7,8,16)), so the native material is showing through. Flat sets `vibrancy = false`, calls `clear_vibrancy`, and renders opaque (13,13,14).

Things the vibrancy setup needs:
- `transparent: true` on macOS needs `macOSPrivateApi: true` in `tauri.conf.json` and the `macos-private-api` cargo feature. That is a private-API flag (it blocks Mac App Store distribution); note it as a distribution decision for M1.
- The material follows the window appearance, so the frontend calls `setTheme(dark|light)` on every theme switch; Rust picks `UnderWindowBackground` (dark) or `Sidebar` (light).
- Opaque themes must paint `.app` themselves (the window is transparent); `data-vibrancy="false"` does that.

## 2. Token pipeline

Implements `docs/contracts/m0.md` §4 without changing it.

- `loader.ts`: `parseThemeToml` (smol-toml) then `resolveLayers` then `validate`. A `{name}` reference is looked up in the token's own layer and the ones below, lowest first (primitive, semantic, component). Cycles and unknown names are errors that name the chain. `{refs}` also interpolate inside larger values (`0 1px 0 {border}`).
- Validation reports **every** problem in one `ThemeError`: missing required semantic tokens (the 40 in §4), unresolvable references, and WCAG AA (4.5:1) for `text` and `text-muted` on `surface-0..2` and `text-on-accent` on `accent`. A colour the contrast check cannot parse (anything but `#hex` / strict `rgb()`) is an error, not a skipped check. `rgb()`/`rgba()` is parsed with the real CSS grammar (legacy comma form or modern space/slash form, never mixed; junk suffixes, units and exponents rejected), so the loader cannot certify a contrast for a value the browser would drop. Alpha is composited over black (dark) or white (light).
- **Name collisions are errors**: all layers emit into one CSS namespace (primitives as `--p-<name>`, semantic and component as `--<name>`), so a `[component]` or `[semantic]` token that reuses an emitted name would overwrite a validated variable (e.g. `[component] text = ...` replacing `--text` after the contrast check). `nameCollisions` rejects it (`name-collision`).
- `runtime.ts`: `applyTheme` sets the variables inline on `:root`, removes the previous theme's variables that the new one doesn't set, and sets `data-theme`, `data-appearance`, `data-vibrancy`, `color-scheme`. No reload, no stylesheet swap. Transitions are suspended for the two frames around a swap. Measured cost to the next frame (3 runs, `themeSwapToNextFrameMs`): 10 ms for the first swap and 5 to 10 ms afterwards in fix round 1 (22 to 24 ms for the first swap in the original round).
- Components use only semantic and component tokens, **enforced by `src/styles/tokens-conformance.test.ts`**: it fails on hex/`rgb()`/named colours, absolute lengths (`px`, `rem`, ...) and durations (`ms`, `s`) in `components.css`, in `base.css` outside its `:root` token-definition block, and in every `src/ui/*.tsx` and `App.tsx` (including numeric icon sizes and size clamps); it also checks that the checker catches each kind. Relative units (`%`, `em`, `vh`) and unitless values are allowed. Component tokens have defaults in `src/styles/base.css`, built from semantic tokens; a theme overrides them in `[component]` (the three directions use this for panel gap/radius, chat font and measure, label font, tab height, syntax colours).
- `pnpm test` (87 tests: `src/theme/loader.test.ts` and `src/styles/tokens-conformance.test.ts`): colour maths against the WCAG reference ratios; reference resolution across all three layers; interpolation; missing reference; cycle (and self-reference); a primitive cannot reference upward; **each of the 40 required tokens** removed in turn fails with `missing-token`; low contrast for `text`, `text-muted`, `text-on-accent`; the 4.5:1 boundary (`#767676` on white passes, `#7a7a7a` fails); unparseable colour; all problems reported together; malformed TOML, meta and names; and the CI check on the shipped files: every `themes/*.toml` loads, has the full semantic set and passes AA, and dark/light variants have identical token names. The VS Code import is tested too.

### VS Code import (Catppuccin Mocha, MIT, vendored from Shiki's bundled themes)

`src/theme/vscode.ts` maps `colors` to semantic tokens (first key present wins) and `tokenColors` to `--syntax-*` component tokens (most specific TextMate scope wins). The imported theme passes AA unchanged; if a theme fails, `ensureContrast` mixes the colour towards white/black until it passes and records the adjustment (tested with a deliberately bad theme).

| Semantic token | VS Code `colors` keys (in order) |
|---|---|
| `surface-0` | `titleBar.activeBackground`, `activityBar.background`, `sideBar.background`, `editor.background` |
| `surface-1` | `editor.background` |
| `surface-2` | `editorWidget.background`, `dropdown.background`, `input.background`, `list.hoverBackground` |
| `surface-overlay` | `quickInput.background`, `editorSuggestWidget.background`, `editorWidget.background` |
| `text` | `foreground`, `editor.foreground` |
| `text-muted` | `descriptionForeground`, `sideBarTitle.foreground`, `tab.inactiveForeground` |
| `text-faint` | `disabledForeground`, `editorLineNumber.foreground`, `tab.inactiveForeground` |
| `text-on-accent` | `button.foreground` |
| `border` | `panel.border`, `editorGroup.border`, `contrastBorder` |
| `border-strong` | `input.border`, `dropdown.border`, `focusBorder` |
| `focus-ring` | `focusBorder` |
| `accent` | `button.background`, `focusBorder`, `activityBarBadge.background` |
| `accent-hover` | `button.hoverBackground` |
| `accent-muted` | `list.activeSelectionBackground`, `editor.selectionBackground` |
| `danger` | `errorForeground`, `editorError.foreground`, `terminal.ansiRed` |
| `warning` | `editorWarning.foreground`, `list.warningForeground`, `terminal.ansiYellow` |
| `success` | `gitDecoration.addedResourceForeground`, `terminal.ansiGreen`, `charts.green` |
| `info` | `editorInfo.foreground`, `terminal.ansiBlue`, `charts.blue` |
| `code-bg` | `textCodeBlock.background`, `editor.background` |
| `diff-add` / `diff-del` | `diffEditor.insertedLineBackground` / `removedLineBackground`, then `...TextBackground` |
| `diff-add-text` / `diff-del-text` | `gitDecoration.addedResourceForeground` / `deletedResourceForeground`, then `terminal.ansiGreen` / `ansiRed` |
| `ghost-text` | `editorGhostText.foreground`, `editorLineNumber.foreground` |
| derived | `surface-glass` = `surface-0` at 72% alpha; `state-running/needs-you/failed/ready` = `info/warning/danger/success`; density and motion tokens = Atrium defaults (a VS Code theme has none) |

| `--syntax-*` | TextMate scopes (in order) |
|---|---|
| keyword | `keyword`, `storage.type`, `storage` |
| string | `string` |
| number | `constant.numeric`, `constant` |
| comment | `comment` |
| function | `entity.name.function`, `support.function`, `meta.function-call` |
| type | `entity.name.type`, `support.type`, `entity.name.class` |
| variable | `variable`, `source` |
| property | `variable.other.property`, `support.type.property-name`, `meta.object-literal.key` |
| punctuation | `punctuation`, `meta.brace` |

Not done: `terminal.ansi*` (no terminal in the shell), the CodeMirror theme, and Shiki itself. The file preview uses a 30-line regex highlighter so the `--syntax-*` tokens are visible; Shiki replaces it in M1 (it reads the same `tokenColors`).

## 3. Visual directions

Each has a dark and a light theme, switchable live from the palette (`Theme: ...`), the palette icon in the tab strip, or `⌘⇧L`. Pedro picks; this report does not.

| | **Glass** | **Flat** | **Paper** |
|---|---|---|---|
| Idea | Native vibrancy, translucent chrome, floating rounded panels over the blur | Opaque, dense, hairline grid edge to edge, mono-flavoured labels | Warm and editorial: serif conversation text, generous measure, no blur |
| Accent | indigo `#7c8cff` / `#4452d6` | emerald `#34d399` / `#0b7d56` | terracotta `#e07a4f` / `#b4532a` |
| Density (`space-unit`, text) | 4 px, 13.5 px | 3 px, 12.5 px | 4 px, 13.5 px (chat 16 px serif, 1.65 line height) |
| Radius | 12 px, panels gap 6 px | 4 px, gap 0 | 8 px, gap 0 |
| Vibrancy | yes (`vibrancy = true`) | no | no |
| Files | `themes/glass-{dark,light}.toml` | `themes/flat-*.toml` | `themes/paper-*.toml` |

Imported: `imported-catppuccin-mocha` (dark only), through the VS Code importer.

**Recommendation per direction** (for Pedro to weigh, not a pick):
- *Glass*: the most "native macOS" and the only one that uses vibrancy, so it is where the Linux fallback matters most (section 6). Floating panels cost about 12 px of chrome per edge; at 13.5 px text it is the roomiest. Highest risk, most distinctive. Keep it if native feel is the priority.
- *Flat*: the best fit for the "fast, lightweight" principle: no blur to composite, the densest layout (3 px unit, 12.5 px text, no panel gaps), and expected (not measured) to be the cheapest on WebKitGTK. The least personality; the mono labels carry it.
- *Paper*: the most readable for long agent transcripts and the clearest difference from other tools. Serif body text with code in mono works well; panels are flat, so it costs the same as Flat. Needs a decision on the serif fallback off macOS (`ui-serif` / New York only exist on Apple platforms; it falls back to Georgia).
- All three can ship: the contract already makes the others alternative themes. A reasonable reading is Flat or Paper as the default for performance and legibility, Glass as the macOS-only "vibrant" theme.

## 4. Measurements (macOS, release build; re-measured in fix round 1)

Machine: Apple M4 MacBook, macOS 27.0.1, 60 Hz display, other apps open (browser, editor). Build: `pnpm tauri build` (release, `lto = "thin"`, `strip`), app bundle 4.6 MB, `dist/` 368 KB. Harness: `scripts/measure.py`, `src/bench.ts`. **Every command pins the starting state and prints it** (`ATRIUM_THEME=glass-dark ATRIUM_PRESET=default ATRIUM_TAB=auth` unless `--theme` says otherwise); harness-pinned themes are no longer persisted, so runs cannot leak into each other. The numbers below are from this round's final build unless marked "earlier".

### Cold start (budget: time to interactive < 400 ms)

```
cd spikes/ui-shell && pnpm tauri build
./scripts/measure.py cold --runs 20                      # Glass dark (pinned)
./scripts/measure.py cold --runs 15 --theme paper-dark
```

Method: the script takes `T0 = time.time()` right before `exec` of the binary. Rust records its wall clock at the top of `run()`; the page records `performance.timeOrigin + performance.now()` at JS start, mount, first paint and first handled input; all are ms since launch on one clock.
- **First paint**: two `requestAnimationFrame`s after mount; first-contentful-paint (Paint Timing) is reported next to it.
- **Time to interactive** = first input handled: after first paint the harness focuses the composer and dispatches a `keydown`; the composer's handler stamps the time. A second probe dispatches `⌘K` and waits for the palette to be in the DOM and painted.
- **Caveat**: the keystroke is synthetic, so this is when the app can handle input, not OS delivery (a few ms more).

| This round, ms from launch | Glass dark, 20 runs | Paper dark, 15 runs |
|---|---|---|
| Rust `main` to JS start (webview + page load) | median 261 | n/a |
| first-contentful-paint, median | 322 | n/a |
| first paint (rAF x2), median | 345 | 346 |
| **time to interactive, median** | **345** | **346** |
| time to interactive, worst of the runs | 354 | 354 |
| palette open and painted, median | 372 | n/a |
| launches over 400 ms | 0 of 20 | 0 of 15 |

Across sessions the picture is less comfortable. Seven sets of 10 to 20 launches on near-identical builds, taken over several hours with other apps running, had **medians of 345 to 392 ms** and **worst warm launches of 354 to 457 ms** (earlier sets: median 384 / worst 457; Glass 392 / 445; Paper 379 / 409; Glass 380 / 445). The launch right after a fresh build was far slower: **957 ms** in one set and **2,836 ms** in this round's first Glass set (that 15-run set had 2 of 15 launches over 400 ms: the 2,836 ms one and one other that I did not record individually; the later 20-run set had none). A true cold boot (page cache purged with `sudo purge`) was not measured.

**Verdict: the median is under 400 ms (345 to 392 ms depending on the session); some launches exceed it** (up to 457 ms warm, and 1 to 3 seconds on the first launch after a build). The contract does not say which statistic is the gate (median, p95, worst, first launch), so a "pass" cannot be declared here; that is a gate decision. About 260 to 300 ms is Rust `main` to the first JS line (WKWebView process spawn and page load) and the app's own JS is about 33 ms; the split inside the 260 ms was not measured. The app is a trivial shell; the real one loads more.

### Idle memory (budget: app idle RSS < 150 MB, Tauri process + WebKit helpers)

```
./scripts/measure.py rss --settle 15                     # Glass dark
./scripts/measure.py rss --settle 15 --theme flat-dark
```

Method (fixed in fix round 1): the app is launched with `open -n --env ...`, which makes it its own *responsible process*. The script then takes the app plus exactly the WebKit helpers whose responsible pid (the macOS `responsibility_get_pid_responsible_for_pid` call Activity Monitor uses; private API, macOS only) is the app, and prints them with their responsible pids, so the process list is auditable. WebKit processes of other apps (6 to 9 were running) are listed as ignored, not guessed at by timing. The script refuses to report a total if the ownership check fails, if any footprint is unreadable, or if the WebContent or Networking helper is missing. Two metrics: `ps` RSS (what the budget literally says) and `footprint -p` (physical footprint).

| Glass dark, 15 s / 35 s after launch | RSS | footprint |
|---|---|---|
| app (Rust) | 100 / 86 MB | 27 / 27 MB |
| WebKit.WebContent | 74 / 58 MB | 80 / 82 MB |
| WebKit.GPU | 29 / 23 MB | 13 / 15 MB |
| WebKit.Networking | 13 / 8.5 MB | 6 / 7 MB |
| **total** | **215 / 176 MB** | **126 / 131 MB** |

Flat dark: RSS 214 / 214 MB, footprint 125 / 125 MB. Earlier unfiltered runs (helpers attributed by "new since launch", which the review rightly called unreliable) gave RSS 144 to 222 MB and footprint 128 to 135 MB; the owned-process runs agree with the footprint range.

**Verdict: physical footprint (about 125 to 131 MB) is under 150 MB; summed `ps` RSS (176 to 215 MB here, 144 to 222 MB earlier) is over it and moves with memory compression.** This spike does not change the frozen RSS budget; it reports that RSS counts shared system pages and varies by 40 MB or more with the same app state, and recommends the orchestrator decide the metric (footprint is the stable one). The transcript is virtualized (only visible rows are in the DOM).

### Frame times (resize panes, scroll the 5,000-message transcript)

```
./scripts/measure.py frames --runs 3
```

Method: `requestAnimationFrame` deltas for 3 s per test on the release build in a visible 1360x860 window (dpr 2), in-process, no external input.
- *Scroll*: `scrollTop` is set every frame, upwards through the 5,000 messages. "reading" is 1,500 px/s; "fling" is 20,000 px/s (about 60,000 px, more than 500 messages in 3 s, so rows are created and destroyed every frame).
- *Resize*: the three-panel split is driven every frame by a 0.8 Hz sweep through the same signal path a pointer drag uses (`onSizesChange`). Real mouse-drag in WKWebView was not performed.
- *Work per frame* has two parts, **reported separately and not summed into one claim**: (a) the synchronous part (the scroll handler with the virtualizer's Solid DOM updates, or the resize step, plus a forced style/layout); (b) the main-thread time from the start of each rAF callback to the next task, which includes the rendering update (style, layout, paint recording, ResizeObserver callbacks such as the virtualizer's row measurements). Compositing and GPU work are off-thread and invisible to JS. `performance.now()` is rounded to 1 ms in WebKit, so these are coarse. Fix round 1 added (b); the earlier "about 1 ms per frame" covered only (a).

Three runs; run 1 (all runs agreed within noise except where noted):

| Test | fps | mean / p95 / max frame (ms) | frames > 25 ms | (a) synchronous, mean / p95 / max (ms) | (b) rAF to next task, mean / p95 / max (ms) |
|---|---|---|---|---|---|
| idle | 60.0 | 16.7 / 18 / 20 | 0 | n/a | 0.1 / 1 / 1 |
| scroll 1.5k px/s | 60.1 | 16.6 / 19 / 19 | 0 | 0.2 / 1 / 2 | 0.5 / 1 / 2 |
| scroll 20k px/s | 60.1 | 16.6 / 18 / 19 | 0 | 1.0 / 3 / 3 | 1.2 / 3 / 5 |
| pane resize | 60.0 | 16.7 / 18 / 21 | 0 | 1.2 / 2 / 3 | 3.0 / 4 / 7 |

Across all 3 runs the worst frame was 27 ms (one frame in the resize test of run 2, the only frame over 25 ms in 3 x 4 tests); the worst (b) was 8 ms (resize). Roughly 60% of frames read as "more than 16.7 ms" in every row, including idle, because WebKit timestamps are rounded to 1 ms and a 60 Hz frame is 16.67 ms (reads as 17 or 18): that is the display cadence, not drops.

**Verdict: holds 60 fps on this 60 Hz panel with a few ms of measured main-thread work per frame (mean (a)+(b) about 2.5 ms when flinging the transcript and about 4 ms when resizing; worst single frame about 11 ms for resize), with one 27 ms outlier in nine seconds of resizing.** That is not proof of headroom at 120 Hz (8.3 ms): the numbers are coarse, exclude compositing, and the resize worst case is within a few ms of that budget. Not measured: 120 Hz, real pointer drag, streaming tokens (M0-07), paint/composite time.

## 5. What broke, and what I chose not to use

- **Kobalte Combobox for the palette**: first attempt. Its listbox is rendered in a popper-positioned portal outside the dialog, so the list lost the palette's styling and position. Replaced by Kobalte **Dialog** + a hand-built ARIA combobox (`role=combobox`, `aria-activedescendant`, `role=listbox/option`). Kobalte still supplies the focus trap, scrim, Escape handling and the DropdownMenu and Tabs used elsewhere.
- **Palette filter**: matching on label and hint made "layout review" run *Layout: Default* (its hint mentions "review"). Fixed by matching the label only (found by a scripted check, not by hand).
- `window-vibrancy` 0.6 and 0.8 were both compiled (Tauri itself depends on 0.8); pinned to 0.8.
- Persisted theme: the last chosen theme was kept in `localStorage`, including harness-pinned ones, so runs leaked into each other. Fixed in round 1: a theme pinned by the harness env is not persisted, and every measurement command pins and prints its start state.
- The workspace root `Cargo.toml` lists only Rust spikes; `spikes/ui-shell/src-tauri/Cargo.toml` has its own empty `[workspace]` so cargo doesn't try to adopt it.

## 6. Linux / WebKitGTK: not measured here

A Linux run must check, in this order (the owner and gate are a driver decision, `docs/contracts/m0.md` §5):
1. **Blur fallback (implemented, not validated on Linux).** The theme's request (`data-vibrancy-requested`) is now separate from what is applied (`data-vibrancy`): the frontend asks Rust, and when vibrancy is unsupported or the native call fails, chrome (`.strip`, `.rz-side`) is painted opaque with `--surface-0` and no `backdrop-filter`. Checked in a browser (no native vibrancy): Glass renders with an opaque strip. On a Linux machine confirm that `transparent: true` doesn't force a software compositor, that the opaque fallback looks right, and decide whether Glass is offered at all there (the other two directions use no blur). Check `backdrop-filter` cost on the palette and toast with the blur radius set to 0 versus 30 px.
2. **120 Hz smoothness.** Repeat `scripts/measure.py frames` on a 120/144 Hz Wayland and X11 session; check that rAF actually runs at the display rate and that the 1 ms main-thread cost above stays under the 8.3 ms budget. WebKitGTK is not guaranteed to use the GPU compositor on every driver; record `WEBKIT_DISABLE_COMPOSITING_MODE` behaviour.
3. **Idle memory.** Sum `WebKitWebProcess` + `WebKitNetworkProcess` + the app (footprint-equivalent: PSS from `/proc/<pid>/smaps_rollup`, not RSS) with the same 5,000-message transcript, compared to the 150 MB budget.
4. **Window chrome.** The overlay title bar is macOS-only. Linux needs the client-side decorations the plan describes (the same tab-strip header, with its own window buttons and `data-tauri-drag-region` / `startDragging`), including Wayland drag and resize edges. Not built in this spike.
5. **Fonts and cold start.** The `ui-serif`/`ui-monospace` stacks fall back differently; check Paper's serif and the code font. Re-measure time to interactive with the same harness (the `exec to main` and `main to js` split).
6. **Drag and drop.** `dragDropEnabled: false` is set in `tauri.conf.json` for the web layer's tab dragging; verify solid-dnd pointer dragging works inside WebKitGTK.

## 7. Verification summary (original round; see section 10 for fix round 1)

- `pnpm install && pnpm typecheck && pnpm test` pass (65 tests); `cargo clippy --manifest-path spikes/ui-shell/src-tauri/Cargo.toml -- -D warnings` passes; `pnpm tauri build --debug` produces `Atrium UI Shell.app`, which launches (the harness reports a first paint and handled keystroke).
- Interaction was checked in Chrome against the Vite dev server with scripted input: sidebar handle drag (229 to 344 px), tab drag-reorder, palette filtering and run, `Layout: Focus/Review/Default` widths ([1,1358,1], [1,406,946], [229,673,444]), `⌘J` (jumps to the "needs you" thread), `⌘⇧L` (flat-dark to flat-light), reduced motion. Inside the real WKWebView only the harness probes (synthetic keystroke, `⌘K`, programmatic resize and scroll, theme swaps) and the screenshots were run; mouse-drag and tab-drag were not repeated there.
- Not verified: keyboard-only use of the palette and tabs beyond `⌘K`/arrows/Enter; screen-reader behaviour; the focus ring on every direction.

## 8. Recommendation for M1

1. **Keep the stack**: Tauri 2 + Solid + Corvu + Kobalte + solid-virtual. Everything in the brief worked. Plan for the palette to be hand-built on Kobalte Dialog.
2. **Adopt the token pipeline as is** (`src/theme/`): it is pure, tested and about 700 lines including the importer. Fold CR-01 (shadow, scrim, syntax, glass-blur tokens) into the contract before M1 so components don't invent names.
3. **Decide the budget metrics**: restate "app idle RSS" as physical footprint and say whether the WebKit helpers count; add a startup-budget breakdown (Rust `main` to JS start is 300 ms of 390).
4. **Cold start has little slack**: medians of 345 to 392 ms with some launches over 400 ms, on a trivial app. In M1, keep the first screen's JS small, lazy-load Shiki, CodeMirror, xterm and the diff view, and track TTI in CI with `measure.py cold`.
5. **Decide distribution**: vibrancy requires `macOSPrivateApi`. If the Mac App Store matters, Glass's native blur is unavailable and only the CSS `backdrop-filter` path remains.
6. **Pick the direction** (Pedro). The other two ship as alternative themes with no code change.

## 9. Open risks

- Startup: median under 400 ms in every session, but some launches exceed it (457 ms warm; 957 ms and 2,836 ms on the first launch after a build); the accepted statistic and launch conditions are an open gate decision.
- RSS vs footprint ambiguity on the budget (section 4).
- Linux Glass: no native blur; may need to be macOS-only.
- Real pointer drag and 120 Hz rAF are unmeasured on WKWebView.
- The Catppuccin theme is MIT (vendored with Shiki's bundled copy); a licence notice file is needed before it ships as a bundled theme.
- The icon is a placeholder (`src-tauri/icons/source.svg`, original, generated by `pnpm tauri icon`).

## 10. Fix round 1 (review by GPT-6.1 Sol, 9 findings, all fixed)

Order as requested. Tests: 87 pass (was 65); typecheck, clippy and `pnpm tauri build --debug` pass.

| # | Finding | What changed |
|---|---|---|
| 2 | `[component]` token could overwrite a validated semantic variable | `nameCollisions` in the loader rejects any semantic/component name that would emit an already-emitted CSS variable (also `p-*` vs primitives); 6 tests (the `--text` bypass, every required semantic name, `--p-*` clashes, collision still reported when the value is also unresolvable, valid distinct names still load) |
| 1 | Components hard-coded sizes, durations, caps | About 25 new component tokens in `base.css` (`--hairline`, `--dot-size`, `--icon-xs/sm/md`, `--tab-max-width`, `--pulse-duration`, `--composer-max-height`, `--palette-width`, ...); all literals removed from `components.css`, `base.css` (outside `:root`) and the TSX (icon sizes are token classes; the composer's 200 px cap is the CSS token; the layout-animation timer reads `--motion-normal`). `tokens-conformance.test.ts` enforces it (section 2). CR-01 untouched; the stopgaps stay. |
| 4 | Memory attribution wrong; failed footprints counted as 0 | `open -n` launch + responsible-pid ownership, auditable process list, no total on any gap (section 4). Re-measured. |
| 8 | Linux blur fallback not implemented | Requested vs applied vibrancy, opaque fallback (section 6); Rust's `set_vibrancy` result is no longer discarded. Not validated on Linux. |
| 6 | Startup verdict overstated | Reworded to "median under 400 ms; some launches exceed it" in the report and handoff; the statistic is a gate decision. Re-measured with worst cases and counts. |
| 5 | Commands depended on persisted theme | Every command pins theme, preset and tab and prints the configuration; harness-set themes are not persisted. The report's commands now match what was run. |
| 7 | "About 1 ms per frame" was only the synchronous part | Relabelled; added rAF-to-next-task cost (style/layout/ResizeObserver). Both reported separately (section 4). |
| 3 | `rgb()` parsed with `parseFloat` | Strict CSS grammar; 3 tests with 15 malformed forms, plus a theme-level test that junk is `unparseable-color`. |
| 9 | Closing Home broke the sidebar's Home | `openHome()` recreates the tab; checked in a browser (close Home, click Home in the sidebar). |

Screenshots: regenerated from the final release build, resized to 1600 px wide with Pillow (`sips -Z 1600` alone left 7.7 MB because the images are mostly flat-colour UI at full 24-bit depth) and quantized to 256 colours without dithering: 16 PNGs, 2.2 MB total, same file names. Check them for colour banding in gradients before using them as design references; the originals are regenerated by `scripts/screenshots.sh`. The traffic lights show grey in these captures because the app was not the frontmost window when captured.
