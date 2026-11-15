# M1-17-daemon-start-refusal: a daemon that refuses to start must say why (N9)

Model: Gemini 4 Hydrogen (`gemini-4-hydrogen-medium`, `scripts/agent-run agy`; D-076, D-091) · Reviewer: Sonnet 5.5 (read-only) · Depends on: nothing open
Go: D-091 (Pedro, 2026-11-15) · Branch: `agent/hydrogen/M1-17-daemon-start-refusal` · Worktree: `../atrium-wt/M1-17-daemon-start-refusal`

## Goal
M1-14b found that a `logs` folder inside `ATRIUM_HOME` with mode 0755 makes `atriumd` exit at once **without writing anything**: no log line, no message, and the app just keeps retrying the connection (`docs/spikes/M1-linux-checks.md`, "every earlier Linux run of `startup_bench.py`…"; `m1-must-fix.md` N9). Refusing an insecure folder is right. The silence is the bug: the user (and the app) must learn why the daemon won't start.

## Context
- `crates/atrium-host/src/lifecycle.rs`: `secure_dir`, `spawn_detached` (the daemon's stdout/stderr go to a log file inside the logs folder, opened by `open_private_log`), `default_paths`.
- `crates/atrium-daemon/src/main.rs` and `src/lib.rs` (`Daemon::start`, the `secure_dir` calls).
- Where the app starts the daemon and reports a failed connection: `app/src-tauri/src/bridge.rs`, `app/src-tauri/src/lib.rs`, and `crates/atrium-client/src/lib.rs`.
- Contract `docs/contracts/m1.md` §10 (lifecycle).

## What to do
1. **Find where the reason is lost**, with evidence (a test that reproduces it with a temp `ATRIUM_HOME` whose `logs` folder is 0755): does `spawn_detached` fail to open the log, or does the daemon start, refuse the folder and write its message into a log that can't be opened, or to nowhere? Say which in the handoff.
2. **The daemon side:** a refusal at start (any `LifecycleError` from `secure_dir` or the instance lock) is written to stderr as one line naming the folder, what is wrong (mode, owner, symlink) and what is expected, and the daemon exits non-zero. Keep refusing; don't loosen any check and don't change any mode.
3. **The launcher side:** when the daemon can't be started, or exits within the start-up wait, the launcher returns that reason (the spawn error, or the last lines of the daemon's stderr) instead of retrying silently. If the log file itself can't be opened, capture stderr through a pipe for the start-up wait instead. The app shows it where it already shows a connection failure; **if that needs new UI, stop and report** instead of building it.
4. Tests: the 0755 `logs` folder case and one other refusal (a symlinked folder or the instance lock held), each checking the message text reaches the caller.

## Owned paths
- `crates/atrium-host/src/lifecycle.rs` (no change to its `unsafe` blocks; if one is needed, stop and report) and `crates/atrium-host/tests/lifecycle*.rs`
- `crates/atrium-daemon/src/main.rs`, `crates/atrium-daemon/src/lib.rs` (start-up errors only), new `crates/atrium-daemon/tests/start_refusal.rs`
- The app's launcher code in `app/src-tauri/src/{bridge.rs,lib.rs}` and `crates/atrium-client/src/lib.rs`, start-up error path only
- `docs/agents/handoffs/M1-17-daemon-start-refusal.md`

## Definition of Done
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test -p atrium-host -p atrium-daemon -p atrium-client`; `cargo test -p atrium-app` if you changed it.
- [ ] Handoff: where the reason was lost (with the evidence), what the user now sees, the tests, the outputs.

## Rules
Work only in your worktree. Tests use temp folders; never touch a real home folder and never run a real provider. Stop every daemon a test starts. Commit on your branch only; never push, merge or rebase. Shell substitutions, `~`, `../` and shell loops are refused by the worker policy.

## Escalate if
The fix needs new UI, a wire change, a change to an `unsafe` block, or loosening a folder check.
