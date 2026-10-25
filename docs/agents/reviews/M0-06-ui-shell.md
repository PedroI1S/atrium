# Review: M0-06-ui-shell

Author: Sonnet 5.5 · Reviewer: GPT-6.1 Sol (read-only, commit `6cc0343`) · Triage: orchestrator (Opus 5.5), 2026-10-25

## Triage
All 9 findings are accepted → fix round 1 (Sonnet). Also in the round: shrink the screenshots (10 MB → about 2 MB, ≤1600 px wide) before merge.

| # | Sev. | Finding | Fix |
|---|---|---|---|
| 1 | major | Components hard-code sizes, durations and caps (components.css, Composer.tsx) | Component tokens + a static conformance check |
| 2 | major | A `[component]` token can overwrite a validated semantic variable (e.g. `--text`) and bypass contrast checks | Reject name collisions; regression tests |
| 3 | minor | `parseFloat` accepts malformed `rgb()` channels | Strict channel parsing; tests |
| 4 | major | `measure.py` attributes any new WebKit process to the app; failed footprints count as 0 | Verify ownership or run isolated; reject incomplete totals; re-measure |
| 5 | minor | Frame/RSS commands depend on persisted theme state | Pin theme/preset/tab in every command; print the config |
| 6 | minor | "Inside the 400 ms budget" overstated (445 ms and 957 ms launches observed) | Say "median under 400 ms; some launches exceed it"; the gate decides the statistic |
| 7 | minor | "~1 ms main-thread work per frame" excludes deferred virtualization work | Relabel, or capture full frame work |
| 8 | minor | Linux blur fallback not implemented (the frontend ignores Rust's "vibrancy unsupported") | Requested vs applied vibrancy; opaque fallback |
| 9 | minor | Closing the Home tab breaks the sidebar's Home action | Recreate Home, or make it non-closable |

CR-01: the reviewer agrees it's useful (blur as optional component config; syntax needs a documented bridge to Shiki/CodeMirror), but it doesn't fix #1. **The CR-01 decision is still Pedro's.**

## Reviewer's report
**Approve with changes.** The shell and all 16 screenshots demonstrate the requested layouts and three distinct visual directions. The loader tests substantively cover the requested cases, but token enforcement and measurement reliability need fixes before M1 adopts this unchanged. The disclosed synthetic-input TTI probe is a reasonable readiness measurement; its budget conclusion is overstated. Typecheck passed; Vitest could not start because the read-only sandbox blocked Vite’s temporary config write. I found no blocker in the inspected CSP or capabilities.

1. **Major — Components still hard-code visual values.**  
   [components.css:306](/Users/pedro/Documents/atrium-wt/M0-06-ui-shell/spikes/ui-shell/src/styles/components.css:306) hard-codes badge dimensions, padding, radius and font size; lines 324 and 744 hard-code animation durations (`1.6s`, `1.1s`). Similar literals occur throughout the stylesheet and TSX icon sizes. [Composer.tsx:10](/Users/pedro/Documents/atrium-wt/M0-06-ui-shell/spikes/ui-shell/src/ui/Composer.tsx:10) independently caps height at `200px`. This contradicts §3.20 and the report’s token-only claim. **Fix:** introduce component tokens defaulted from semantic tokens, consume them consistently in CSS/TSX, and add a static conformance check. CR-01 is useful for standardizing shared names, but does not resolve these existing violations; blur can remain optional component configuration, while syntax needs a documented bridge to Shiki/CodeMirror.

2. **Major — Component tokens can bypass semantic contrast validation.**  
   [loader.ts:272](/Users/pedro/Documents/atrium-wt/M0-06-ui-shell/spikes/ui-shell/src/theme/loader.ts:272) checks `layers.semantic`, but line 281 subsequently overwrites the same CSS namespace with arbitrary component keys. Adding `[component] text = "{surface-1}"` to an otherwise valid theme passes validation and emits unreadable `--text`. No test covers this collision. **Fix:** reject component names that collide with semantic or emitted primitive names, and add regression tests proving validated variables cannot be overwritten.

3. **Minor — Malformed RGB values can pass validation.**  
   [color.ts:29](/Users/pedro/Documents/atrium-wt/M0-06-ui-shell/spikes/ui-shell/src/theme/color.ts:29) uses `parseFloat`, so `rgb(255junk,255junk,255junk)` is treated as white although CSS rejects it. The loader can therefore certify a contrast ratio for a value the browser cannot render. **Fix:** validate complete channel syntax and separator structure before conversion; test malformed numeric suffixes and mixed separators.

4. **Major — Memory attribution and footprint totals are not reliable enough for a budget decision.**  
   [measure.py:96](/Users/pedro/Documents/atrium-wt/M0-06-ui-shell/spikes/ui-shell/scripts/measure.py:96) attributes every newly appearing WebKit process to Atrium. Other open applications can create those processes during the settling interval. At line 105, failed footprint measurements silently contribute zero to a seemingly complete total. **Fix:** verify helper ownership, or require an isolated run with an auditable process list; reject incomplete footprint totals. Re-measure before using these numbers to justify changing the frozen RSS budget.

5. **Minor — Frame and memory commands depend on persisted theme state.**  
   [report:138](/Users/pedro/Documents/atrium-wt/M0-06-ui-shell/docs/spikes/M0-06-ui-shell.md:138) specifies Glass for one RSS run, but the second RSS command and frame command omit `ATRIUM_THEME`. The frame harness itself finishes by persisting Glass Dark, so successive runs can start differently. The report’s line 185 claim that measurement commands set the theme explicitly is incorrect. **Fix:** pin theme, preset and transcript tab in every command, and print that configuration with every result.

6. **Minor — The startup verdict claims more than the measurements establish.**  
   [report:133](/Users/pedro/Documents/atrium-wt/M0-06-ui-shell/docs/spikes/M0-06-ui-shell.md:133) declares the shell “inside the 400 ms budget,” despite Glass reaching 445 ms and a separate first launch reaching 957 ms. The contract does not define median as the acceptance statistic. **Fix:** state “median under 400 ms; observed launches exceed the target,” mirror that qualification in the handoff, and obtain an explicit gate decision on the accepted statistic and launch conditions.

7. **Minor — The reported main-thread cost excludes deferred virtualization work.**  
   [bench.ts:108](/Users/pedro/Documents/atrium-wt/M0-06-ui-shell/spikes/ui-shell/src/bench.ts:108) ends scroll timing inside the scroll listener, while [ChatPane.tsx:144](/Users/pedro/Documents/atrium-wt/M0-06-ui-shell/spikes/ui-shell/src/ui/ChatPane.tsx:144) queues row measurement in microtasks. Deferred measurement and subsequent updates fall outside that timing span. Consequently, the report’s “about 1 ms of main-thread work per frame” and derived 120 Hz headroom are too broad. **Fix:** label this as synchronous handler/forced-layout cost, or capture complete frame work with a native performance trace; retain the rAF results separately.

8. **Minor — The Linux fallback described in the checklist is not implemented.**  
   [runtime.ts:19](/Users/pedro/Documents/atrium-wt/M0-06-ui-shell/spikes/ui-shell/src/theme/runtime.ts:19) sets effective vibrancy directly from theme metadata. Rust returns `false` off macOS, but the frontend discards that result, leaving Glass’s window background transparent and its surfaces approximately 50% opaque. **Fix:** distinguish requested vibrancy from supported/applied vibrancy, paint an opaque fallback when unavailable, and describe Linux work as implementation plus validation. The platform deferral itself is permitted by the brief.

9. **Minor — Closing Home makes its sidebar action permanently ineffective.**  
   [TabStrip.tsx:42](/Users/pedro/Documents/atrium-wt/M0-06-ui-shell/spikes/ui-shell/src/ui/TabStrip.tsx:42) allows Home to be removed, while [Sidebar.tsx:29](/Users/pedro/Documents/atrium-wt/M0-06-ui-shell/spikes/ui-shell/src/ui/Sidebar.tsx:29) only activates an existing Home tab. **Fix:** recreate Home when selected from the sidebar, or make its tab nonclosable.