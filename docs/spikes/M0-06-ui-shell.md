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
- Validation reports **every** problem in one `ThemeError`: missing required semantic tokens (the 40 in §4), unresolvable references, and WCAG AA (4.5:1) for `text` and `text-muted` on `surface-0..2` and `text-on-accent` on `accent`. A colour the contrast check cannot parse (anything but `#hex` / `rgb()`) is an error, not a skipped check. Alpha is composited over black (dark) or white (light).
- `runtime.ts`: `applyTheme` sets the variables inline on `:root`, removes the previous theme's variables that the new one doesn't set, and sets `data-theme`, `data-appearance`, `data-vibrancy`, `color-scheme`. No reload, no stylesheet swap. Transitions are suspended for the two frames around a swap. Measured cost: 22-24 ms for the first swap (cold style recalculation of the whole window), 4-9 ms afterwards, to the next frame (3 runs, `themeSwapToNextFrameMs`).
- Components use only semantic and component tokens. Component tokens have defaults in `src/styles/base.css`, built from semantic tokens; a theme overrides them in `[component]` (the three directions use this for panel gap/radius, chat font and measure, label font, tab height, syntax colours).
- `pnpm test` (65 tests, `src/theme/loader.test.ts`): colour maths against the WCAG reference ratios; reference resolution across all three layers; interpolation; missing reference; cycle (and self-reference); a primitive cannot reference upward; **each of the 40 required tokens** removed in turn fails with `missing-token`; low contrast for `text`, `text-muted`, `text-on-accent`; the 4.5:1 boundary (`#767676` on white passes, `#7a7a7a` fails); unparseable colour; all problems reported together; malformed TOML, meta and names; and the CI check on the shipped files: every `themes/*.toml` loads, has the full semantic set and passes AA, and dark/light variants have identical token names. The VS Code import is tested too.

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

## 4. Measurements (macOS, release build)

Machine: Apple M4, macOS 27.0.1, 60 Hz display, other apps open. Build: `pnpm tauri build` (release, `lto = "thin"`, `strip`), app bundle 4.6 MB, frontend `dist/` 368 KB (JS 356 KB / 106 KB gzip). Harness code: `scripts/measure.py`, `src/bench.ts`; the app reads `ATRIUM_BENCH`, `ATRIUM_BENCH_OUT`, `ATRIUM_THEME`, `ATRIUM_PRESET`, `ATRIUM_PALETTE`, `ATRIUM_TAB`, `ATRIUM_RIGHT_TAB`.

### Cold start (budget: time to interactive < 400 ms)

```
cd spikes/ui-shell && pnpm tauri build
ATRIUM_THEME=glass-dark ./scripts/measure.py cold --runs 12
ATRIUM_THEME=paper-dark ./scripts/measure.py cold --runs 12
```

Method: the script takes `T0 = time.time()` immediately before `exec` of the binary. Rust records its wall clock at the top of `run()`; the page records `performance.timeOrigin + performance.now()` at JS start, mount, first paint, and the first handled input. Everything is on the same wall clock, so each number is "ms since process launch".
- **First paint**: two `requestAnimationFrame`s after mount (the frame after the first commit); first-contentful-paint from the Paint Timing API is reported next to it.
- **Time to interactive** = first input handled: after first paint the harness focuses the composer and dispatches a `keydown`, and the composer's handler stamps the time. A second probe dispatches `⌘K` and waits until the palette is in the DOM and has painted.
- **Caveat**: the keystroke is synthetic (dispatched from script), so it measures when the app can handle input, not OS event delivery (a few ms more). A real key event path was not measured.

| Median of 12 runs, ms from launch | Glass dark | Paper dark (no vibrancy) |
|---|---|---|
| exec to Rust `main` | 15 | 16 |
| Rust `main` to JS start (webview creation + page load) | 303 | 292 |
| JS start to mounted | 33 | 33 |
| first-contentful-paint | 369 | 353 |
| **first paint** (rAF x2) | **392** | **378** |
| **time to interactive** (keystroke handled) | **392** | **379** |
| palette open and painted | 421 | 410 |
| range of TTI (min to max) | 367 to 445 | 345 to 409 |

An earlier 10-run set on the same build with the previously persisted theme gave a median TTI of 384 ms (range 367 to 457). The first launch after a fresh build is slower when the binary has not been scanned yet (one run at 957 ms; the later sets' first runs were 345 to 435 ms), so a first launch after install should be expected to miss the budget. A true cold boot (cache purged, `sudo purge`) was not measured.

**Verdict: inside the 400 ms budget, but with about 10 ms of margin at the median and a maximum of 445 ms on Glass.** About 300 ms of it is Rust `main` to the first JS line (WKWebView process spawn and page load); the app's own JS is 33 ms. That share is outside our control except by trimming what the webview must load; it was not decomposed further (no Rust-vs-WebKit split). The budget is tight for the real app, which will load more JS.

### Idle memory (budget: app idle RSS < 150 MB, Tauri process + WebKit helpers)

```
ATRIUM_THEME=glass-dark ./scripts/measure.py rss --settle 15
./scripts/measure.py rss --settle 30
```

Method: launch the app normally (no harness), wait, then sum the app process and the **new** WebKit helper processes (`com.apple.WebKit.WebContent`, `.GPU`, `.Networking`) that appeared after launch (found by diffing `ps` before and after; the helpers are XPC services, not children of the app). Two metrics: `ps` RSS (what the budget literally says) and `footprint -p` (macOS physical footprint, which excludes shared system pages).

| Glass dark, 15 s after launch | RSS | footprint |
|---|---|---|
| app (Rust) | 100 MB | 27 MB |
| WebKit.WebContent | 63 MB | 81 MB |
| WebKit.GPU | 25 MB | 14 MB |
| WebKit.Networking | 12 MB | 6.5 MB |
| **total** | **199 MB** | **128.5 MB** |

Other runs: total RSS 222 MB (10 s), 220 MB (10 s), 205 MB (30 s); footprint 135, 133, 130 MB. RSS also dropped to 144-150 MB 20 s later in two runs (memory compression of idle pages), so **the RSS total swings between about 145 and 220 MB with the same app state**; footprint is stable at 128-135 MB.

**Verdict: footprint passes (about 130 MB < 150 MB); summed `ps` RSS does not (145-220 MB depending on when it is read).** The RSS figure counts shared system framework pages and is not a reliable budget metric on macOS. Recommend the budget in PLAN §4 be restated as physical footprint (and say whether the WebKit helpers count), because as written the number can flip between pass and fail with no change in the app. The 5,000-message transcript is virtualized (only the visible rows are in the DOM), so memory here is mostly the web content process baseline plus 5,000 JS message objects.

### Frame times (resize panes, scroll the 5,000-message transcript)

```
./scripts/measure.py frames --runs 3
```

Method: `requestAnimationFrame` deltas sampled for 3 s per test on the release build in a visible 1360x860 window (dpr 2), all in-process, no external input.
- *Scroll*: `scrollTop` is set every frame, moving up through the 5,000 messages (virtualized). "reading" is 1,500 px/s; "fling" is 20,000 px/s (about 60,000 px, more than 500 messages scrolled past in 3 s, so rows are created and destroyed every frame).
- *Resize*: the three-panel split is driven every frame by a 0.8 Hz sweep through the same signal path a pointer drag uses (`onSizesChange`), forcing style and layout each frame. Real mouse-drag was not performed in WKWebView; it was exercised in Chrome (handle drag and tab drag both work, section 7).
- *Main-thread cost per frame*: the scroll handler (virtualizer + Solid DOM updates) or the resize step, followed by a forced style/layout, timed with `performance.now()`. This shows headroom even though rAF is pinned at the display rate. `performance.now()` is rounded to 1 ms in WebKit, so these are coarse.

Three runs, all three agreeing within noise; run 1:

| Test | fps | mean / p95 / p99 / max frame (ms) | frames > 16.7 ms | frames > 25 ms | main-thread per frame (mean / p95 / max, ms) |
|---|---|---|---|---|---|
| idle | 60.3 | 16.6 / 18 / 18 / 18 | 57/91 | 0 | n/a |
| scroll 1.5k px/s | 60.1 | 16.7 / 18 / 18 / 18 | 109/181 | 0 | 0.5 / 2 / 3 |
| scroll 20k px/s | 60.1 | 16.7 / 18 / 18 / 18 | 105/181 | 0 | 1.0 / 2 / 3 |
| pane resize | 60.0 | 16.7 / 18 / 18 / 18 | 114/180 | 0 | 1.3 / 2 / 2 |

Max frame over all 3 runs and all tests: 21 ms; never over 25 ms. "Frames > 16.7 ms" is about 60% in every row, including idle, because WebKit timestamps are 1 ms-rounded and 60 Hz frames are 16.67 ms (they read as 17 or 18); this is the display cadence, not drops.

**Verdict: holds 60 fps with about 1 ms of main-thread work per frame (about 6% of a 60 Hz budget, about 12% of a 120 Hz one).** Not measured: 120 Hz (this panel is 60 Hz, and WKWebView's rAF rate on ProMotion panels is unverified), real pointer-drag, streaming tokens (that is M0-07), and paint/composite time (invisible to JS).

## 5. What broke, and what I chose not to use

- **Kobalte Combobox for the palette**: first attempt. Its listbox is rendered in a popper-positioned portal outside the dialog, so the list lost the palette's styling and position. Replaced by Kobalte **Dialog** + a hand-built ARIA combobox (`role=combobox`, `aria-activedescendant`, `role=listbox/option`). Kobalte still supplies the focus trap, scrim, Escape handling and the DropdownMenu and Tabs used elsewhere.
- **Palette filter**: matching on label and hint made "layout review" run *Layout: Default* (its hint mentions "review"). Fixed by matching the label only (found by a scripted check, not by hand).
- `window-vibrancy` 0.6 and 0.8 were both compiled (Tauri itself depends on 0.8); pinned to 0.8.
- Persisted theme: the last chosen theme is kept in `localStorage`, including themes set by the harness env, so a run without `ATRIUM_THEME` uses whatever ran last. The measurement commands above set it explicitly.
- The workspace root `Cargo.toml` lists only Rust spikes; `spikes/ui-shell/src-tauri/Cargo.toml` has its own empty `[workspace]` so cargo doesn't try to adopt it.

## 6. Linux / WebKitGTK: not measured here

A Linux run must check, in this order (the owner and gate are a driver decision, `docs/contracts/m0.md` §5):
1. **Blur fallback.** Glass depends on `window-vibrancy` (macOS/Windows only) plus `backdrop-filter`. On Linux there is no native blur in WebKitGTK: confirm Glass degrades to its near-opaque `--surface-glass` (it is `rgba(.., 0.5)` and would look wrong over a transparent window), that `transparent: true` doesn't force a software compositor, and decide whether Glass is offered at all on Linux (the other two directions use no blur). Check `backdrop-filter` cost on the palette and toast with the blur radius set to 0 versus 30 px.
2. **120 Hz smoothness.** Repeat `scripts/measure.py frames` on a 120/144 Hz Wayland and X11 session; check that rAF actually runs at the display rate and that the 1 ms main-thread cost above stays under the 8.3 ms budget. WebKitGTK is not guaranteed to use the GPU compositor on every driver; record `WEBKIT_DISABLE_COMPOSITING_MODE` behaviour.
3. **Idle memory.** Sum `WebKitWebProcess` + `WebKitNetworkProcess` + the app (footprint-equivalent: PSS from `/proc/<pid>/smaps_rollup`, not RSS) with the same 5,000-message transcript, compared to the 150 MB budget.
4. **Window chrome.** The overlay title bar is macOS-only. Linux needs the client-side decorations the plan describes (the same tab-strip header, with its own window buttons and `data-tauri-drag-region` / `startDragging`), including Wayland drag and resize edges. Not built in this spike.
5. **Fonts and cold start.** The `ui-serif`/`ui-monospace` stacks fall back differently; check Paper's serif and the code font. Re-measure time to interactive with the same harness (the `exec to main` and `main to js` split).
6. **Drag and drop.** `dragDropEnabled: false` is set in `tauri.conf.json` for the web layer's tab dragging; verify solid-dnd pointer dragging works inside WebKitGTK.

## 7. Verification summary

- `pnpm install && pnpm typecheck && pnpm test` pass (65 tests); `cargo clippy --manifest-path spikes/ui-shell/src-tauri/Cargo.toml -- -D warnings` passes; `pnpm tauri build --debug` produces `Atrium UI Shell.app`, which launches (the harness reports a first paint and handled keystroke).
- Interaction was checked in Chrome against the Vite dev server with scripted input: sidebar handle drag (229 to 344 px), tab drag-reorder, palette filtering and run, `Layout: Focus/Review/Default` widths ([1,1358,1], [1,406,946], [229,673,444]), `⌘J` (jumps to the "needs you" thread), `⌘⇧L` (flat-dark to flat-light), reduced motion. Inside the real WKWebView only the harness probes (synthetic keystroke, `⌘K`, programmatic resize and scroll, theme swaps) and the screenshots were run; mouse-drag and tab-drag were not repeated there.
- Not verified: keyboard-only use of the palette and tabs beyond `⌘K`/arrows/Enter; screen-reader behaviour; the focus ring on every direction.

## 8. Recommendation for M1

1. **Keep the stack**: Tauri 2 + Solid + Corvu + Kobalte + solid-virtual. Everything in the brief worked. Plan for the palette to be hand-built on Kobalte Dialog.
2. **Adopt the token pipeline as is** (`src/theme/`): it is pure, tested and about 700 lines including the importer. Fold CR-01 (shadow, scrim, syntax, glass-blur tokens) into the contract before M1 so components don't invent names.
3. **Decide the budget metrics**: restate "app idle RSS" as physical footprint and say whether the WebKit helpers count; add a startup-budget breakdown (Rust `main` to JS start is 300 ms of 390).
4. **Cold start has no slack**: 10 ms margin on a trivial app. In M1, keep the first screen's JS small, lazy-load Shiki, CodeMirror, xterm and the diff view, and track TTI in CI with `measure.py cold`.
5. **Decide distribution**: vibrancy requires `macOSPrivateApi`. If the Mac App Store matters, Glass's native blur is unavailable and only the CSS `backdrop-filter` path remains.
6. **Pick the direction** (Pedro). The other two ship as alternative themes with no code change.

## 9. Open risks

- TTI margin of about 10 ms; 957 ms on a first launch after build.
- RSS vs footprint ambiguity on the budget (section 4).
- Linux Glass: no native blur; may need to be macOS-only.
- Real pointer drag and 120 Hz rAF are unmeasured on WKWebView.
- The Catppuccin theme is MIT (vendored with Shiki's bundled copy); a licence notice file is needed before it ships as a bundled theme.
- The icon is a placeholder (`src-tauri/icons/source.svg`, original, generated by `pnpm tauri icon`).
