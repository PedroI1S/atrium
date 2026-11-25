# M1-43-n17-supervise-drain-flake: review round 1

Reviewer: Claude Sonnet 5.5 in agy (`claude-sonnet-5-5-high`, D-162, D-169), read-only. Reviewed: `2794203f`, `c123f94a`.

Orchestrator's verification (macOS): fmt and clippy clean; workspace 924/0/7; `supervise` ×200 quiet, 0 failures; ×100 under 20 `yes` CPU hogs (load up to 46), 0 failures.

## Verdict

**Request changes.** The code change is sound. Under D-120 the findings below (all Low) still count, and they are all in the handoff or in test hardening. I read the diff only (`tests/supervise.rs` plus the handoff, nothing under `src/`) and ran nothing. The test-side fix does what it claims:
- `sh -c 'exec 3<&0; cat -u <&3 3<&- & echo "MEMBER:$!"; exit 0'` leaves `cat` in the leader's process group (`spawn_gone_leader_with_holding_member`, `tests/supervise.rs:279-337`). The shell is non-interactive, so it has no job control and no new group.
- The `ready` round-trip proves `cat` has finished exec and read before `kill_recorded` starts.
- `GROUP_DRAIN_TIMEOUT` is untouched, and no assertion is loosened or retried.

The handoff overstates how well the cause is proven, its repro is wrong, and its line citations are off.

## Findings

1. **Low: the cause is presented as established, but it is a hypothesis.** `handoff.md:16-20` says the test "flaked because" of the heavyweight member plus no handshake. The brief itself calls this "suspected, unconfirmed". The handoff has no timing measurement, such as how long a `current_exe` member takes to start under load. The code reading is correct: `identify(member_pid)` succeeds at fork time, so the 2 s clock starts before the member is ready. But the dyld/AMFI/libtest story is unsupported. Fix: reword to "most likely cause, consistent with the code, not reproduced". Alternatively, add a measurement.

2. **Low: the §5.2 repro is wrong.**
   - It says the test "fails immediately". It can't: `kill_recorded` runs for the full 2 s before returning `Unverified`.
   - The 1950 ms sleep starts at member process start. `kill_recorded`'s clock starts later, after the leader is reaped and `identify` runs. The member would then exit at about 1950 ms plus libtest teardown, which is borderline and may pass.
   - Fix: use a sleep of 2500 ms or more and say it takes about 2 s.
   - The last bullet ("with the fix in place… `kill_recorded` still returns `Verified`") is untested. The injection point no longer exists after the fix.
   - Brief DoD item 3 (reverted fix makes the failure more likely, or explain why not) is therefore only met by a synthetic injection. The handoff should say that.

3. **Low: wrong line citations in the handoff.**
   - `handoff.md:12-13` cites `tests/supervise.rs:1069` and `1064-1067` for the Linux and macOS zombie pins. The actual lines are `1053-1059`. macOS is `1053-1057` and Linux is `1058-1059`. Lines 1062-1075 are a different test, `terminate_blocking_with_only_the_zombie_leader_in_the_group`.
   - The `src/supervise.rs` cites (`298-305`, `360-365`, `378-387`) are about right.
   - Fix: correct the citations.

4. **Low: some drain-clock dependence remains, and the handoff says it was removed.** After the fix, the member still goes through exit, then a zombie, then a reap by PID 1, all inside the 2 s window.
   - **macOS:** a zombie gives `EPERM` (`tests/supervise.rs:1055`), so the test needs launchd to reap within about 1.85 s of the 150 ms release.
   - **Linux:** it needs PID 1 or a subreaper to reap.
   - **Failing case on Linux:** a container where PID 1 doesn't reap orphans (for example `docker run` without `--init`, and PID 1 is `sleep`). The `cat` zombie then stays, `kill(-pgid,0)` returns 0, and the test returns `Unverified` every time. The orphan test at `tests/supervise.rs:446-448` already works around this by making the launcher the reaper.
   - This isn't new. It is the same as before, but now `cat` startup and exit are cheap, so it is the only remaining dependency.
   - Fix: say so in the handoff. Optionally add a comment in the helper that the test assumes a reaping init. A portable way to make the test the reaper would be better, but I don't see one on macOS.

5. **Low (nit): `member_pid` is never proven to be `cat`.** It is `$!` from the shell. The only check is `member_id.pgid == recorded.pgid` (`tests/supervise.rs:337`). If a shell forked an intermediate subshell for `&`, the PID would belong to a different process than the one holding stdin. The `ready` echo does prove some group member holds the pipe. In the Verified test, `!pid_exists(member_pid)` could pass while `cat` is still alive, and `!group_exists` would catch that later. Fix: assert the executable is `cat`, if `ProcessIdentity` exposes it. Otherwise leave it.

6. **Low (wording): "EPERM can't be told apart from another UID's live group" is only true of `kill(2)`.**
   - Treating `EPERM` as not-empty is intentional and correct for fail-safe `Verified`; no product change is needed.
   - macOS can list group members (`proc_listpgrppids`, or sysctl `KERN_PROC_PGRP`) and check `SZOMB`. So the claim should read "indistinguishable via `kill(2)`".

7. **Low (pre-existing, hardening): on macOS, `Stdio::piped()` sets CLOEXEC after `pipe()`, not atomically.**
   - The `cat` stdin write end can leak into another parallel test's child forked in that microsecond window, for example the `sleep 60` orphan helper at `:405`.
   - `cat` would then never see EOF when `stdin` drops, and the Verified test would return `Unverified`.
   - It is vanishingly rare, which fits the 0/300 failures, and the old design had it too.
   - Fix: none required. Optionally note it in a comment.

## Review checks

- **1 Cause.** Partly confirmed. The mechanism is real in the code (`kill_recorded` `:287-322`, `wait_for_group_drain_sync` `:378`, `group_is_empty` `:360`; a zombie member counts as not empty). It isn't demonstrated. See findings 1 and 4.
- **2 Fix keeps what the test pins.**
  - `GROUP_DRAIN_TIMEOUT` is unchanged at `src/supervise.rs:46`.
  - The Verified test's assertions are unchanged.
  - **POSIX sh:** `&` sets stdin to `/dev/null` before explicit redirections (dash and bash agree), so `exec 3<&0` is needed and correct.
  - **Pipe handling:** `3<&-` closes the extra fd in `cat`, so fd 0 inside `cat` is the only copy. The leader's fd 3 and fd 0 close when it exits. `cat` inherits the stdout pipe, so `MEMBER:` and `ready` reach the test through the same `BufReader`.
  - **Linux and macOS:** `cat -u` is accepted by GNU, BSD and busybox `cat`, and `/bin/sh` is bash or dash. I see no portability problem.
  - **Handshake:** it proves exec and one completed read-and-write cycle. `cat` can be a few instructions from re-entering `read` when the clock starts, which is harmless.
- **3 Sibling test.** `kill_recorded_returns_unverified_and_leaves_member_alive_when_drain_times_out` (`:369`) is unchanged and keeps its meaning. `cat` blocks on the pipe held by the `stdin` local for the whole 2 to 3 s, so it stays alive. The `drop(stdin)` cleanup is now fast. The only two users of the helper are `:348` and `:371`. No other reference to `GONE_LEADER_*` or `helper_gone_leader_*` remains in `.rs` files (`grep -n -e GONE_LEADER … tests/supervise.rs` finds none). The other `current_exe` users (`:415`, `:816`) use `READY` or `LEFT` handshakes.
- **4 Owned paths.** Only `tests/supervise.rs` and the handoff changed. Nothing under `src/`.

## Test gaps

- Nothing ran the reverted code under load, so the claim that the fix changes failure likelihood is untested. A before/after run of the old test under the same 20-`yes` load would settle it. Without it, 0/100 under load for the fix alone is weak evidence.
- Nothing ran on Linux. The orchestrator's verification was macOS-only, and the Linux dash, `cat -u` and PID 1 reaping assumptions (finding 4) have no run behind them.
- No test checks that `member_pid` is `cat` (finding 5).
- The Unverified test's `elapsed < 3 s` upper bound was untouched. It is still a load-sensitive edge, with 1 s of slack over 2 ms sleeps. It is separate from N17 but worth watching in the load runs.


## Orchestrator's before/after run (answers the first test gap)
Same machine and the same load: 20 `yes` CPU hogs, load average 15 to 39.
- **`dev`, the old test:** `supervise` ×100 had **2 failures**, each one test in a 2.02 s run, which is the drain timeout.
- **M1-43, the fixed test:** ×100 had **0 failures**.

Modest evidence that the fix lowers the failure rate. Linux has not been run.
