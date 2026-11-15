# M1-07b-proc-drop-linux: a `Proc` dropped outside a runtime must not hang on Linux (N7)

Model: Gemini 4 Hydrogen `high` (`gemini-4-hydrogen-high`, `scripts/agent-run agy`; D-084: Rust, `cargo` only, no credential literals, no GUI; `high` because it is process-lifecycle work) · Reviewer: Sonnet 5.5 (read-only) · Depends on: M1-07 (merged)
Go: D-086 (rodrigo, 2026-11-15) · Branch: `agent/hydrogen/M1-07b-proc-drop-linux` · Worktree: `../atrium-wt/M1-07b-proc-drop-linux`

## Goal
Make `crates/atrium-agents/tests/claude_adapter.rs::a_start_dropped_outside_a_runtime_kills_the_process_group` pass on Linux, every time, by fixing the code, not the test. Today it hangs for ever on Linux (10 of 10 runs; macOS passes). `m1-must-fix.md` N7 has the details.

## What is known
- `crates/atrium-agents/src/claude/adapter.rs` `impl Drop for Proc` (around line 419): off a runtime it calls `terminate_now(child)` on the dropping thread. Inside a runtime it calls it on a fresh `std::thread`.
- `terminate_now` builds a new current-thread runtime and `block_on(supervise::terminate(&mut child, Duration::ZERO))`.
- `crates/atrium-host/src/supervise.rs` `terminate`: it waits without reaping, sends `SIGKILL` to the group, then calls `child.child.wait().await` (the tokio `Child`), then drains the group.
- **The orchestrator's reading (not proven):** tokio 1.54 on Linux waits for a child through a pidfd that is registered with the reactor of the runtime that spawned it. In the test, that runtime is the `#[tokio::test]` current-thread runtime, which is blocked in `std::thread::spawn(..).join()`. So the wake never arrives. The hung thread sat in `epoll_wait` with a `pidfd` open, and the leader was a zombie with no other group member left. macOS reaps through `SIGCHLD`, so it passes there.
- **Prove or disprove this reading first.** For example, a minimal test or a log line shows whether the leader is already dead and only the reap is stuck.

## What to do
1. Reproduce: `cargo test -p atrium-agents --test claude_adapter a_start_dropped_outside_a_runtime -- --exact` (it hangs; use `timeout 60`).
2. Fix it so the reap does not depend on the spawning runtime's reactor. The orchestrator's suggestion: a blocking variant of `terminate` in `supervise.rs` (for example `terminate_blocking`) with the same order. The order is: check for an early exit without reaping (`WNOWAIT`), `SIGKILL` the group while the leader is unreaped, reap the leader with a blocking `waitpid` on its pid, then verify the group is empty (`ESRCH`). `terminate_now` then uses it with no runtime at all.
   - Keep `terminate`'s safety rule: never reap the leader before the group is signalled.
   - Keep the macOS `EPERM` zombie quirk handling.
   - After a blocking reap, the tokio `Child` must not reap again or report an error that changes the outcome. `kill_on_drop` on an already-reaped child must be harmless: check what tokio does.
3. Check the in-runtime path too (`std::thread::spawn(move || terminate_now(child))`). It must also not depend on the caller's runtime, which may be shutting down.
4. Tests:
   - the existing three `a_start_dropped_*` tests and `r3_a_start_dropped_outside_a_runtime_after_the_leader_exited_kills_the_group` pass;
   - add a unit test for the blocking variant in `atrium-host` if one fits (a child in its own group, with a grandchild);
   - run the whole `claude_adapter` binary 20 times in a row with `timeout 180` each, and paste the results.

## Owned paths
- `crates/atrium-agents/src/claude/adapter.rs`: only `Proc`'s drop and `terminate_now`.
- `crates/atrium-host/src/supervise.rs`: only a new blocking variant and its helpers. `terminate`'s behaviour for its existing callers must not change.
- Tests: `crates/atrium-agents/tests/claude_adapter.rs` (no change to the hanging test's assertions), `crates/atrium-host/tests/` or `supervise.rs`'s own test module.
- `docs/agents/handoffs/M1-07b-proc-drop-linux.md`.
- Not yours: the contract, `crates/atrium-core/**`, the daemon, the app. If `supervise`'s public API counts as contract (check `docs/contracts/m1.md`), write a CR in `change-requests.md` and stop.

## Definition of Done
- [ ] Before the fix, the hang reproduced; afterwards the test passes. Show both.
- [ ] `claude_adapter` 20/20 runs pass (raw output in the handoff).
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test -p atrium-host -p atrium-agents --no-fail-fast`.
- [ ] Handoff: the cause as proven, the change, what the reviewer should check.

## Rules
- Worker rules in `docs/agents/README.md`.
- Stop every process you start. A hung test leaves `claude-fake` processes behind: kill them by name, never with a pattern that matches your own shell.

## Escalate if
The fix needs `terminate`'s behaviour to change for its existing callers, or a contract change.
