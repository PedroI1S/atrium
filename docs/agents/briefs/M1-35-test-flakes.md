# M1-35-test-flakes: three intermittent tests on Linux (N13, N14, N15)

Model: Gemini 4 Neon (`gemini-4-neon-high`, `scripts/agent-run agy`, D-134) · Reviewer: Codex GPT-6.1 Sol `high`, read-only (D-124) · Depends on: nothing open
Go: D-137 · Branch: `agent/neon/M1-35-test-flakes` · Worktree: `../atrium-wt/M1-35-test-flakes`

## Goal
Make three tests deterministic, by fixing the real cause (in the test or in the code). Each fails rarely: once in a full `cargo test --workspace` run, or under parallel load, and passes when run alone. Their entries in `docs/agents/m1-must-fix.md` give the evidence.

- **N13:** `app/src-tauri/src/upgrade.rs`, `tests::the_daemon_in_the_lock_file_gets_the_signal`. The `sleep 60` child ran its full 60 s without being signalled (`upgrade.rs:134`). It happened under load. Hypothesis to check: `supervise::identify(child.id())` runs right after `spawn`, possibly before the child has exec'd `sleep`, so the recorded identity is the pre-exec image's; or the signal lands before exec and is lost. If it is a real race in `stop_old_daemon` or `identify` (the code the app uses on upgrades), fix the code, not only the test.
- **N14:** `crates/atrium-agents/tests/claude_adapter.rs`, `questions_use_the_gates_question_ticket`. It panicked at line ~1088, in the `unwrap` of `serde_json::from_str(&wait_file(…canuse_response.json…))`. Hypothesis: `wait_file` (~201) returns as soon as the file exists, and reads what the fake provider is still writing. If so, fix `wait_file` (wait for a complete file: the fake writes to a temp name and renames, or the reader waits until it parses) in both `claude_adapter.rs` and `codex_adapter.rs`, which has the same helper (~169).
- **N15:** `crates/atrium-daemon/tests/M1-28-followups.rs`, `discover_login_shell_path_handles_noise_failures_and_timeouts`: the assertion at line ~148 failed (the discovered PATH). Hypotheses: `ETXTBSY` (the test writes a script and executes it while other test threads fork, so a forked child inherits the write descriptor), or the 2 s timeout in a loaded full run. If `discover_login_shell_path` (`crates/atrium-daemon/src/lib.rs`) mishandles `ETXTBSY` or a slow shell, fix it there.

## Owned paths
- `app/src-tauri/src/upgrade.rs`; `crates/atrium-host/src/supervise.rs` (`identify` only, if N13 needs it).
- `crates/atrium-agents/tests/claude_adapter.rs`, `crates/atrium-agents/tests/codex_adapter.rs` (their helpers and the fakes' scripts).
- `crates/atrium-daemon/tests/M1-28-followups.rs`; `crates/atrium-daemon/src/lib.rs` (`discover_login_shell_path` only, if N15 needs it).
- The handoff `docs/agents/handoffs/M1-35-test-flakes.md`.

## Rules
- Worker rules in `docs/agents/README.md`. D-123: run only `cargo fmt --all` and `cargo check -p <crate> --tests`. No `cargo test`, `cargo clippy` or `pnpm`; the orchestrator runs the tests, many times.
- Find each cause with evidence from the code; never fix a flake by raising a timeout alone or by retrying. Never weaken an assertion.
- Where a deterministic regression test is possible (for example, a reader that sees a half-written file), add it.

## Definition of Done (the orchestrator's verification)
- [ ] fmt, workspace clippy, `cargo test --workspace --no-fail-fast` clean.
- [ ] Each of the three tests' binaries 30 times in a row with no failure, and the full workspace run 3 times, with a second verification running in parallel for load.
- [ ] Codex review: approve with no findings (D-120).
