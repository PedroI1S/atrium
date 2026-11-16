# M1-19-host-supervision: the open process-supervision races (N3, N6, N10)

Model: Gemini 4 Hydrogen (`gemini-4-hydrogen-high`, `scripts/agent-run agy`; D-103) · Reviewer: Opus 5.5 (read-only; D-053, because these paths sit beside `unsafe` `libc` calls) · Depends on: nothing open
Go: D-103 (Pedro, 2026-11-16) · Branch: `agent/hydrogen/M1-19-host-supervision` · Worktree: `../atrium-wt/M1-19-host-supervision`

## Goal
Close the three open supervision items in `docs/agents/m1-must-fix.md` (N3, N6, N10). Today each can make Atrium report a provider's cleanup as `Unverified`, or fail a stop, when the process is in fact gone or killable. The rule stays the same everywhere: never report a cleanup as verified, and never kill a group, unless the identity checks hold. Unknown stays `Unverified`.

## Context
- `docs/agents/m1-must-fix.md`: N3, N6, N10, and the "Where M1 stands" summary.
- **N3:** `docs/agents/handoffs/M1-15-store-launch-recovery.md`, section 7, has M1-15's diagnosis with two root causes in `supervise.rs`:
  1. `kill_recorded` can't reap a zombie it didn't spawn, so `group_is_empty` stays false past `GROUP_DRAIN_TIMEOUT` (2 s) under load;
  2. on Linux, `identify_spawned` can read `/proc/<pid>/exe` across `CLONE_VFORK` before `execve`.
- **N6:** `docs/agents/reviews/M1-14a-linux-host-round2.md`, finding 1. `judge_exe_read` (`supervise.rs` around line 777) treats `PF_EXITING` plus `ENOENT` as gone without checking `num_threads`. `read_linux_exe` tries `/proc/<pid>/task/*/exe` only when the first read saw `Z`.
- **N10:** `docs/agents/handoffs/M1-07c-leader-sigkill.md` (open 1) and `docs/agents/reviews/M1-07c-leader-sigkill.md`. On macOS, while a leader is exiting but `waitid` can't report it yet, XNU answers `EPERM` to the group kill. `peek_exited` is then false, and `terminate`/`terminate_blocking` return `Err(EPERM)`.
- Code: `crates/atrium-host/src/supervise.rs` (`kill_group_while_unreaped`, `kill_leader_while_unreaped`, `finish_termination`, `kill_recorded`, `group_is_empty`, `peek_exited`, `judge_exe_read`, `read_linux_exe`, `identify_spawned`, `LinuxStat`); tests `crates/atrium-host/tests/{supervise.rs,adversarial_supervise.rs}`; contract `docs/contracts/m1.md` §10.

## Owned paths
- `crates/atrium-host/src/supervise.rs`, and `crates/atrium-host/tests/supervise.rs` plus `adversarial_supervise.rs`.
- `crates/atrium-daemon/tests/{sessions.rs,recovery.rs}`, only to re-run or tighten the two N3 tests. Don't weaken any assertion.
- `docs/agents/handoffs/M1-19-host-supervision.md`.
- Not yours: the contract, any other crate's `src/`.

## What to do
1. **N10 (macOS `EPERM`).** On `EPERM` from the group kill, when the leader isn't yet seen as exited, poll `peek_exited` for a bounded time before failing:
   - a few tens of ms, with a named constant;
   - async in `terminate`, blocking in `terminate_blocking`.

   If the leader turns out to have exited, carry on as today's exited path does. If the bound runs out, return the error as today.

   Test: the worker's first test from M1-07c, which failed 18 of 25 runs on `dev`. Run it at least 25 times; it must pass every time.
2. **N6 (Linux, fails closed today).** As the round-2 review says:
   - `PF_EXITING` counts as gone only with `num_threads <= 1`;
   - `read_linux_exe` tries the task folders whatever the first state was;
   - add the table row with `after.pgid < first.pgid`.

   `LinuxStat` and `judge_exe_read` compile under `cfg(test)` on macOS, so the table tests run here. Say in the handoff which paths only Linux can run (rodrigo's next shift).
3. **N3.** Fix root cause 1 without breaking the rule above. For example, treat a group whose only members are zombies whose identity was already verified as empty. Or wait on the leader's exit through a means that doesn't need to be its parent, within the existing drain timeout. Choose one and explain it in the handoff.

   For root cause 2 (Linux `CLONE_VFORK`): if a fix is small and safe, make it, with a test. If not, describe it in the handoff for a Linux shift.

   Run `sessions::a_process_the_adapter_left_running_is_killed_or_reported` and `recovery::a_killed_daemon_is_recovered_at_the_next_start` 30 times each, by hand: the worker policy refuses shell loops, so use a single `cargo test` invocation with a repeat count if the test harness allows it. Report the failures.
4. **`unsafe`.** Don't add new `unsafe` blocks unless there's no other way. If you change one, list it in the handoff for the Opus audit (D-053).

## Definition of Done
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast`.
- [ ] The N10 test passes 25 times and the N3 tests 30 times, each counted.
- [ ] Each fix has a test that fails without it (or, for Linux-only paths, a table test that runs on macOS).
- [ ] Handoff:
  - each item: the cause, the fix and the tests;
  - the run counts;
  - the Linux-only paths left for rodrigo;
  - any changed `unsafe` block.

## Rules
- Work only in your worktree, and commit on your branch only. Never push, merge or rebase.
- Tests use temp folders. Stop every process and daemon a test starts.
- Shell loops, `~`, `../` and `$(...)` are refused by the worker policy.

## Escalate if
- a fix would need a change to contract §10's reporting rules;
- N3 can't be fixed without reporting a cleanup verified on weaker evidence than today.
