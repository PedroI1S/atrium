# Review: M1-22-daemon-adapters (6476d73, 285ba7b)

Reviewer: Claude Sonnet 5.5, read-only (cross-provider). Author: Gemini 4 Hydrogen `high`. Saved by the orchestrator from the reviewer's report. The orchestrator confirmed finding 2 (no `PATH` handling in `lifecycle.rs` `detached_command`, the launchd plist, the app or the daemon) and confirmed the test fails on dev (`ProbeProviders` → `[]`) and passes with the fix.

## Verdict: **approve with changes**
The two-adapter registration is correct and the test is solid. One major finding (`PATH` on a GUI or launchd start) must be fixed, or accepted by the driver, before the gate.

## Findings
1. **Option defaults: correct.** `ClaudeOptions::default()` (`claude/adapter.rs:117-126`) keeps the native user, project and local setting sources (§3, D-017), platform managed settings, a 20 s gate deadline and no tap. `CodexAdapter::new()` (`codex/adapter.rs:83-86`) uses `$CODEX_HOME` or `$HOME/.codex`. Neither needs the daemon's paths. Both get `program: None` (`commands.rs:586-587`, `sessions.rs:340`), so the CLI is resolved through the daemon's `PATH`.
2. **Major: a real Home chat fails when the app is started from Finder, the Dock, Spotlight or launchd.**
   - The app spawns the daemon with `lifecycle::spawn_detached_wait` (`bridge.rs:212`) → `detached_command` (`lifecycle.rs:261`), with no env changes. The launchd plist (`lifecycle.rs:721-744`) has no `EnvironmentVariables`.
   - A GUI-launched app gets launchd's `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin`). `claude` and `codex` live in `~/.local/bin`, `~/.claude/local`, `/opt/homebrew/bin`, `/usr/local/bin` or an npm, bun or nvm global bin.
   - Result: the probe shows no CLI and `StartThread` fails with "failed to start the provider". It works only when the app is started from a terminal, so the M1 demo hides it.
   - Smallest fix: in `atriumd`'s `main` (before the runtime starts), append the well-known dirs that exist to `PATH`, existing entries first, de-duplicated. A fuller fix also merges the login shell's `PATH` (`$SHELL -ilc`, short timeout).
   - The binary test must still not reach a real CLI: make the extra dirs injectable (an env var the test sets empty), and unit-test the merge separately.
3. **The test is solid.** It runs the real binary, isolates `ATRIUM_HOME`, `HOME` and `PATH` (empty dir), removes `CLAUDE_CONFIG_DIR` and `CODEX_HOME`, turns `notify_failed` off, and a `Drop` guard kills and reaps the daemon. Assertions are the right ones.
   - Minor: stderr is piped but read only on early exit; a chatty daemon could fill the pipe. Use a file or `Stdio::null()`.
   - Nits: `wait_for_socket` waits for the file, not for `listen` (fine with tokio); the spawn-error match is a substring.
4. **`Options` comparison: complete.** Nothing tests set is missing from `main.rs`. Release `app_identity: None` is M8's (confirm it is tracked).

## What the reviewer ran
`cargo test -p atrium-daemon --test binary_adapters`: 1 passed. `git status` clean. No provider CLI run.
