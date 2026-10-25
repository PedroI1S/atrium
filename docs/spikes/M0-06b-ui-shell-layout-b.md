# M0-06b: Floating spaces, opacity and pop-out windows

Task `M0-06b-ui-shell-layout-b` · GPT-6.1 Sol · 2026-10-25.

Implementation and build checks are complete. **Native interaction, screenshots and performance measurements have not been run.** The worker did not launch the app, a browser or a dev server. The orchestrator must perform the runtime checks below before treating any of these mechanisms as proven or declaring the performance budgets met.

## What was built

`spikes/ui-shell/` now opens layout B: a sidebar directly on the window frame, floating main/context cards, topic circles, a ranked inbox mock, chat/editor/database mock views, sidebar and context-panel strips, Corvu resize handles, and an Appearance settings card. Advanced appearance occupies the right side, closes on navigation and is never a modal. The editor path is centered. The 5,000-message seeded, virtualized transcript is retained.

The two Calm theme files are copied unchanged from `docs/design/themes/`. The app and palette expose Calm Light/Dark; the older directions and importer remain as loader regression coverage. This is a mechanics spike, not a pixel-perfect implementation of every screen in the design. It uses the theme's system-font fallbacks: Instrument Sans and Geist Mono font binaries are not bundled. Topic management, real editor/terminal/database engines, window-position restoration, drag-to-detach, pinning and per-workspace layout persistence are deferred. There is one movable chat view (its selected thread identity moves with it), one editor and one database. Threads share the dummy chat's draft/message store rather than implementing a real multi-session model.

## Opacity mechanisms

| Mode | Mechanism | Native API / limitations |
|---|---|---|
| Off | Slider at 100%; all surface alphas and native alpha are 1, native vibrancy cleared | The baseline uses the same transparent-window configuration and floating cards as the other modes. |
| Background only | Opaque theme surfaces emit `--surface-{0,1,2}-rgb` and `--surface-overlay-rgb`. CSS composes them through component paint tokens with runtime alpha. Chrome uses `a`; cards use `a + (1-a)*0.35`, or 1 with Keep reading areas solid. | `window-vibrancy` supplies native blur when enabled. Existing `transparent: true`, `macOSPrivateApi: true` and the cargo `macos-private-api` feature remain necessary for transparent macOS backgrounds. Blur failure paints opaque surfaces and reports an error. |
| Whole window | Surface alphas remain 1; `--window-opacity` is passed to **AppKit `NSWindow.setAlphaValue`**. There is no CSS opacity approximation. | A Tauri `with_webview` callback borrows the native window on the main thread, sets its alpha and reads `alphaValue` back. The operation includes native chrome. This is a public AppKit API; the spike still has the private-API flags because it also supports background transparency. |

API references: [Apple NSWindow alphaValue](https://developer.apple.com/documentation/appkit/nswindow/alphavalue), [Tauri WebviewWindow / with_webview](https://docs.rs/tauri/2.12.5/tauri/webview/struct.WebviewWindow.html). The native call compiles and links here; its visual effect still needs runtime verification. No unsupported API workaround was required, so the brief's whole-window-opacity escalation did not trigger.

One slider controls both modes; Advanced selects the mode and the solid-reading option. Settings are changed live and serialized by the Rust shared-state command, persisted in Tauri's app-config directory as `calm-settings.json`. Harness runs pin settings and never overwrite that file. Live theme/opacity changes are broadcast to every window, without reload. Only settings persist across application relaunch; dummy view contents live for the process lifetime.

On macOS, native `NSWorkspace` Reduce transparency / Increase contrast preferences and the matching CSS media queries force opaque rendering. Native preferences are re-read on settings changes and window focus; media-query changes also trigger reapplication. Appearance shows the accessibility override. There is no dedicated native notification observer for changes while focus never changes.

**Linux fallback is implemented, untested:** `vibrancySupported` is false; channel alphas and native alpha stay at 1, the slider/blur controls are disabled with an explanation, and native blur is not applied. Detached Linux windows use normal decorations and opaque backgrounds. The main window's existing configuration remains the inherited M0-06 configuration; client-side decorations and WebKitGTK compositor testing are still deferred. The macOS measurement harness intentionally refuses an opaque/accessibility fallback instead of reporting it as a translucent run.

## Pop-out state and lifecycle

`src-tauri/src/shell.rs` owns a mutex-protected, revisioned snapshot. Frontends subscribe **before** requesting their initial snapshot, and ignore stale revisions. Settings and view updates are emitted to all windows. Each view has one owner; only that owner may edit its content. State includes selected dummy thread, draft, sent messages, transcript scroll offset and editor/query text.

The header or command palette creates a `WebviewWindow` at `index.html?view=<id>` and transfers ownership before the child reads its initial snapshot. The source displays an “Open in its own window / Bring back” placeholder. Repeated pop-out focuses the existing window; failed creation restores ownership. Child windows have the same capabilities, theme, runtime opacity and accessibility handling as main.

Input patches are optimistic locally and sent in order. A header return, native close, or source-window Bring back flushes the **owning window's** queued edits before destruction. Source requests are delivered to that owner, rather than destroying it from main before its edits finish. Rust also restores ownership on native destruction. Closing main exits the spike and its child windows. Pure TypeScript and Rust tests verify serialized state survives an ownership round-trip; they do not exercise WebKit, close-button delivery or focus.

For M1, replace the dummy snapshot with one Rust-managed daemon client built around M0-05's `ClientHandle`: subscribe once to daemon events, forward projections to view owners, and route commands through the shared client. Keep view placement and unsent drafts in app/workspace state, not in the agent protocol. Add native lifecycle integration tests, per-view generation IDs for rapid close/reopen, crash recovery, bounds, selection/cursor restoration and atomic settings-file replacement before shipping.

## Measurement harness and exact commands

Run from the worktree root, in fish or another shell:

```text
cd /Users/pedro/Documents/atrium-wt/M0-06b-ui-shell-layout-b/spikes/ui-shell
pnpm tauri build
scripts/measure-layout-b.sh all /tmp/atrium-M0-06b-measurements
```

`pnpm tauri build` above is the **release** build for measurements. The worker built debug only. Run individual metric matrices with:

```text
scripts/measure-layout-b.sh cold /tmp/atrium-M0-06b-measurements
scripts/measure-layout-b.sh rss /tmp/atrium-M0-06b-measurements
scripts/measure-layout-b.sh frames /tmp/atrium-M0-06b-measurements
```

The executable bash wrapper runs all six configurations: **off / background / whole × 1 / 3 windows**. Off pins alpha 1; the other two pin 0.6, blur on, reading areas not forced solid, Calm Dark, default split and the auth chat. Three windows means the main chat plus detached editor and database. These are real WebviewWindows in a single process. Child settings/native alpha must finish applying, mounting and two rAF callbacks before a window reports ready; main waits for the expected ready count before its input probe. Each report prints both requested configuration and main-window applied alphas, native readback, accessibility override and vibrancy. Mismatches fail the run. Raw startup/frame JSON is included in the logs.

The wrapper accepts an absolute executable override and a repeat-count override, using fish-compatible `env` syntax. For a debug **smoke test**, not release budget evidence:

```text
env ATRIUM_MEASURE_BIN='/Users/pedro/Documents/atrium-wt/M0-06b-ui-shell-layout-b/spikes/ui-shell/src-tauri/target/debug/bundle/macos/Atrium UI Shell.app/Contents/MacOS/atrium-ui-shell' ATRIUM_MEASURE_RUNS=1 scripts/measure-layout-b.sh cold /tmp/atrium-M0-06b-debug
```

Methods inherited from M0-06, with the configuration/readiness checks added:

- **TTI:** ten launches per configuration by default; reports median, worst, first launch, warm-launch median and count over 400 ms. Launch time comes from Python immediately before process creation. First paint is a two-rAF proxy, separately marked after main mounts; FCP is included when WebKit exposes it. TTI requires handled synthetic composer input after the window readiness gate, and palette handling is reported separately. It does not measure OS input delivery or a cache-purged boot. With three windows, TTI includes creating and readying the two additional views.
- **Memory:** `open -n` creates a separate responsible process. After the ready report, sample at 10 and 30 seconds. Attribute app and WebKit helpers by macOS responsible PID, print each PID/RSS/physical footprint and the total, and refuse totals on missing helpers or unreadable footprints. This preserves the frozen RSS metric alongside footprint; it does not redefine the 150 MB budget. Opening multiple windows may share helpers; helpers are summed once by PID, not once per window.
- **Frames:** three repeats per configuration. Main samples idle, 1,500 and 20,000 px/s transcript scrolling and a 0.8 Hz Corvu pane resize sweep. Report rAF deltas/fps, p95/p99/max, synchronous forced-layout cost and rAF-to-next-task cost separately. Extra windows stay open and idle; these are **main-window frame measurements with extra windows present**, not simultaneous scrolling/resizing in all three windows. Theme swapping is disabled during these pinned-mode runs. Compositor/GPU time, real pointer dragging and 120 Hz remain unmeasured.

Use an otherwise idle machine, keep windows visible (a larger display helps), record refresh rate/display arrangement, and avoid interacting during frame runs. Existing windows of this spike should be closed before starting. Startup/frame launches are stopped and reaped in `finally`, including timeout/configuration failures; memory launches are terminated in `finally`. Main exit closes the extra views. The harness never uses an application-wide `killall`.

**Results:** no TTI, RSS, footprint or frame numbers were collected by this worker. There is no pass/fail verdict against 400 ms, 150 MB or 60 fps.

## Orchestrator runtime checklist

1. Launch the debug bundle normally. Check Calm Light/Dark, sidebar and context strips, resizing, and Settings → Appearance → Advanced. Confirm Advanced closes on navigation and focus remains usable.
2. At 60% background opacity, inspect against a patterned desktop: labels/native chrome should remain opaque while surfaces fade. Toggle solid reading areas and blur. Switch to whole-window opacity: text and traffic lights should fade together. Return to 100%. Repeat in Light and Dark.
3. Type an unsent chat draft, send a message and scroll to a recognizable point. Pop out chat, then return using its header, native close, and source Bring back on separate attempts. Repeat edited editor text and database query. Try typing immediately before each return. Verify no content/draft is lost and there is one owner/placeholder at every step.
4. Leave all three windows open. Switch theme and both opacity modes in main; verify every window changes live. Close and reopen one child; verify it inherits the latest settings. Close main; verify all child windows terminate. Relaunch normally; verify appearance settings persist.
5. Enable macOS Reduce transparency / Increase contrast and refocus the app; verify opaque rendering and the note. Disable the preference before measurements (the harness fails on the override).
6. Run the release measurement commands above and attach results/screenshots to this report. If `ps`, `footprint`, LaunchServices or app launching is denied, report that failure; do not replace responsible-PID attribution with timing guesses.

Linux verification remains an orchestrator-owned deferral, with its owner/gate to be recorded by the driver: opaque fallback/explanation, theme channels in WebKitGTK, decorated child windows, one-owner return/close, resize/scroll at 60/120 Hz, startup and app/helper RSS/PSS. No Linux results are implied here.

## CR-02 feedback and M1 recommendation

The runtime/channel split is implementable without relative-colour syntax and without changing the frozen contract. Runtime tokens remain outside the theme variable replacement set. The loader rejects runtime-name overrides (including primitive definitions), derived-channel collisions and non-opaque/non-six-digit-hex surfaces. State text contrast is checked on surfaces 0–2 for all loaded themes, including imports. The old light comparison themes needed stronger state colours; the importer now adjusts state colours and flattens surface alpha. Old Glass overlay surfaces were migrated to opaque hex. Calm itself required no edits.

Keep the draft's component stopgaps (`hover-bg`, `selected-bg`, `state-*-bg`, shadows/scrim/glass-blur) until the orchestrator approves their required semantic names; the frozen required-token set is unchanged. CR-02 should clarify opaque **six-digit** hex versus shorthand hex, define channel tokens as reserved derived outputs, and state how existing translucent imported surfaces are migrated. AA validation is against opaque theme surfaces; it cannot guarantee contrast against arbitrary desktops at user-selected alpha, especially whole-window alpha. Keep the solid-reading option and OS accessibility override.

Recommendation: retain floating cards, surface-channel composition and a shared Rust owner for window state, pending the runtime checks and six-configuration measurements. Public AppKit alpha is the candidate for whole-window opacity. Do not declare the performance/distribution gate passed from compilation: native background transparency still has the inherited Mac App Store constraint, and each extra WebviewWindow adds work that has not yet been measured.
