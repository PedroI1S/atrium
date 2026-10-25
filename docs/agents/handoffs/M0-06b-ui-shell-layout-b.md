# Handoff: M0-06b-ui-shell-layout-b

Model: GPT-6.1 Sol · Status: partial

## Done

- Implemented layout B's floating cards, topic sidebar, ranked inbox mock, collapsible sidebar/context strips, resize handles, simple Appearance settings and non-modal Advanced side pane.
- Copied both Calm themes unchanged into the spike. Added CR-02 runtime opacity/channel emission, reserved-name/collision checks, opaque surface validation and state-text AA validation. Migrated old comparison-theme surfaces/state colours and adjusted the VS Code importer for the stronger checks.
- Implemented background-only opacity with surface alpha/native vibrancy and whole-window opacity using public AppKit `NSWindow.setAlphaValue` with native readback, not CSS opacity. Settings persist and update live. Added native accessibility preference checks and an explained opaque Linux fallback.
- Added a Rust-owned, revisioned shared dummy model and movable chat/editor/database WebviewWindows. Drafts, sent messages, selected thread, scroll offset and edited text survive ownership changes. Return/close flushes the owning window's queued writes. Settings/theme changes broadcast to all windows.
- Added the bash matrix harness for off/background/whole × 1/3 windows, covering TTI, owned-process RSS/footprint and main-window scroll/resize frames. It pins/prints settings, verifies applied opacity/readiness, logs raw reports, and cleans up launched apps on report errors/timeouts.
- Added early JS timing before module/theme evaluation and separate paint/input marks.
- Report: [M0-06b-ui-shell-layout-b.md](../../spikes/M0-06b-ui-shell-layout-b.md), including mechanisms, limitations, exact fish-compatible commands, Linux deferral, CR-02 feedback and the native verification checklist.

## Not done

- No app/browser/dev server was launched and no screenshots were taken, following the sandbox instruction. Native alpha/vibrancy, window move/close/focus, live cross-window synchronization and visual layout still need orchestrator verification.
- No TTI, footprint/RSS, frame or Linux measurements were collected. No performance budget is claimed to pass.
- The broader design is not fully implemented: bundled font files, topic management, per-workspace layout persistence, native window bounds/pinning, drag-to-detach, real editor/database/terminal and per-thread independent dummy stores remain outside this mechanics implementation. One chat/editor/database view can be detached at a time. These limits are detailed in the report.

## Definition of Done results

Commands were run in `spikes/ui-shell` unless an explicit manifest/worktree-relative path is shown.

| Command | Actual result |
|---|---|
| `pnpm install && pnpm typecheck && pnpm test` | Exit 0. Install completed; typecheck passed. Final reruns after all frontend changes: **3 test files, 95 tests passed**, including token conformance, opacity emission/validation and serialized view-state round-trip. |
| `cargo clippy --manifest-path spikes/ui-shell/src-tauri/Cargo.toml -- -D warnings` (worktree root) | Exit 0, final native source checked without warnings. |
| `pnpm tauri build --debug` | Exit 0 on the final frontend/native sources. Bundled `src-tauri/target/debug/bundle/macos/Atrium UI Shell.app`; final bundle output **28.71 MiB**. The app was not launched. |
| `cargo test --manifest-path spikes/ui-shell/src-tauri/Cargo.toml` (worktree root) | Exit 0: **2 native unit tests passed**, 0 failures; main/doc-test targets also passed. Tests cover native ownership restoration/serialization and settings bounds/mode validation. |
| `bash -n spikes/ui-shell/scripts/measure-layout-b.sh` | Exit 0. |
| `spikes/ui-shell/scripts/measure-layout-b.sh --help` | Exit 0; displays matrix usage without launching anything. |
| `env PYTHONPYCACHEPREFIX=/tmp/atrium-m006b-pycache python3 -m py_compile spikes/ui-shell/scripts/measure.py` | Exit 0. Default macOS Python cache location was initially denied; the permitted `/tmp` cache resolves it. |
| `python3 spikes/ui-shell/scripts/measure.py --help` | Exit 0; lists mode, window-count and opacity options. |
| `git diff --check` | Exit 0. |

The code/build/test and report/handoff portions are complete. Runtime proof remains outstanding by design of this worker sandbox.

## Gotchas

- Changes are **uncommitted**. Git metadata resolves to `/Users/pedro/Documents/Harness/.git/worktrees/M0-06b-ui-shell-layout-b`, outside the sandbox's writable roots; no commit was attempted. The orchestrator should review and commit.
- No contracts or `docs/design/` files changed. All final changed/untracked files are in the brief's owned paths.
- pnpm initially chose a worktree-root package store; it was moved into the owned spike directory. `pnpm-workspace.yaml` pins the local store and `.gitignore` excludes it. No root store remains.
- The existing `macOSPrivateApi` / `macos-private-api` flags remain necessary for background translucency. Public AppKit whole-window alpha does not remove that inherited distribution constraint.
- The new measurement wrapper uses a release bundle by default. The debug bundle above is for smoke checks; the report gives an explicit executable override for it.
- Three-window frame measurements animate the main window while editor/database windows remain open and idle; they do not claim simultaneous frame performance in all three windows. Harness accessibility/fallback mismatches fail instead of silently measuring opaque windows as translucent.
- Native close/reopen races, crashes and cursor/selection restoration still need M1 lifecycle hardening. Native state transformation tests are not WebKit integration tests.
- Cleanup: all tracked install/test/build command sessions completed. No long-running app/server/browser process was started. A final `ps` audit was attempted but the sandbox returned `operation not permitted`; no process-list workaround was used.

## Follow-ups

Orchestrator: run the report's native checklist, then execute the following from fish, starting at the worktree root:

```text
cd /Users/pedro/Documents/atrium-wt/M0-06b-ui-shell-layout-b/spikes/ui-shell
pnpm tauri build
scripts/measure-layout-b.sh all /tmp/atrium-M0-06b-measurements
```

This builds release and runs every metric across all six configurations. Keep windows visible, record display/refresh conditions, and attach logs/screenshots plus actual median/worst TTI, RSS/footprint and frame results to the report. Close main after manual checks and confirm children exit. Record Linux fallback verification's owner/gate as a driver decision. Review CR-02's migration/required-token feedback before adopting it for M1; no frozen-contract update was made by this worker.
