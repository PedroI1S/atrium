# M1-43 review round 2 — Claude Sonnet 5.5 (agy, read-only, D-169)

Branch at `2d333922`. Orchestrator's verification: workspace 931/0/7; supervise ×200 quiet 0 failures; ×100 at 2×ncpu with a concurrent worker build (load 71) **2 failures, names not captured**; reruns ×100 at load 39, 57 and 88 with capture: 0 failures.

## Verdict

**Request changes.** The fix is sound and all 7 round-1 Lows are really fixed. Two things block approval:

- **Residual flake.** The orchestrator's 2 uncaptured failures are still unexplained. The DoD says "×200 including under load: 0 failures", and that isn't demonstrated. I can't name the failing tests from the code alone, but I rank the candidates below. The most likely one is the gone-leader drain test, because its success still depends on PID 1 reaping `cat` within the 2 s window.
- **Wording.** One handoff claim is overstated (finding 6).

The `src/` code is untouched, and `GROUP_DRAIN_TIMEOUT` and every assertion's meaning are unchanged. The remaining findings are Low.

**Round-1 Lows**

| # | Check | Result |
|---|---|---|
| 1 | Cause worded as "most likely, not reproduced" | Fixed. `handoff:17` says so and cites the 2/100 → 0/100 run. |
| 2 | Repro | Fixed. `handoff:90-92` uses 2500 ms and "about 2 s", and `:83` states DoD 3 is met by injection plus the before/after run. |
| 3 | Citations | Fixed. I checked them against HEAD and `dev`: `tests/supervise.rs:280-284`, `296-298`, `351-365` and zombie pins `1078-1084` are right. The old `dev` ranges `274-352`, `291-303`, `347-350` are right. `src/` `:28`, `:46`, `:175-182`, `:360-365`, `:378-387` are right. |
| 4 | PID 1 dependence | Fixed in the helper doc comment (`:280-284`) and handoff §2. |
| 5 | Member PID check | Fixed (`:351-365`). `identify` returns `executable` (`src/supervise.rs:28`). |
| 6 | `EPERM` wording | Fixed ("indistinguishable via `kill(2)`", `proc_listpgrppids` / `KERN_PROC_PGRP` mentioned, fail-safe, no product change). |
| 7 | CLOEXEC comment | Fixed (`:296-298`), but see finding 3. |

**Check 1, the cause.** The handoff is right, with two caveats (findings 1 and 6).
- `group_is_empty` (`src/supervise.rs:360-365`) is true only on `ESRCH`.
- A zombie counts as "not empty" on both platforms, and that is intended:
  - On Linux, `kill(-pgid, 0)` returns 0 while a zombie remains (`tests/supervise.rs:1083-1084`).
  - On macOS it returns `EPERM` (`:1078-1082`, `src/supervise.rs:175-182`).
- On macOS that `EPERM` can't be told apart from another UID's live group that reused the pgid.
- `Verified` after a crash must mean the group is gone, so this is fail-safe, not a bug.
- The old test did have a heavy `current_exe` member and no readiness handshake before the 2 s clock.

**Check 2, the fix keeps what the test pins.**
- The `sh` leader is non-interactive, so it has no job control and no new group. `cat` stays in `recorded.pgid`, and the helper asserts this at `:350`.
- `exec 3<&0; cat -u <&3 3<&- &` overrides the default `/dev/null` stdin for background jobs. That holds in bash-as-sh (macOS), dash and ash. If it failed, `cat` would read `/dev/null` and exit, and the helper would panic at `:343`. So the failure is loud, not silent.
- `cat` also inherits the leader's stdout pipe, so the `ready` echo comes back through it.
- `ready` proves `cat` is exec'd and has completed one read and write. It does not prove it is blocked in the next `read(0)`, but that happens immediately after.
- `GROUP_DRAIN_TIMEOUT` is unchanged, no assertion is loosened, and there are no retries.

**Check 3, the sibling.** `kill_recorded_returns_unverified_and_leaves_member_alive_when_drain_times_out` is unchanged, and the member stays alive because the test holds `stdin`. The removed `GONE_LEADER_*` and `helper_gone_leader_*` names have no other references in `crates`, `docs`, `.orchestration` or `scripts`, except historical text in the M1-19 handoff.

**Check 4, owned paths.** Only `crates/atrium-host/tests/supervise.rs` and the handoff changed. No `src/` change.

## Findings

1. **Low (blocking: the DoD isn't demonstrated). The drain test still depends on PID 1 reaping `cat` within the window.**
   - Evidence: `tests/supervise.rs:371-391` and `src/supervise.rs:299-304, 378-387`.
   - After the release thread drops `stdin` (`:375-378`), three things must finish within 1.85 s of wall clock:
     - the releaser thread wakes;
     - `cat` is scheduled, sees EOF and exits;
     - launchd or init reaps the zombie. Until then `kill(-pgid, 0)` answers `EPERM` or `0`.
   - After a stall, `wait_for_group_drain_sync` checks `group_is_empty`, finds `cat`/launchd not yet run, sees the deadline passed, breaks, and returns `Unverified`. A system stall of about 1.5 s is enough.
   - The change removed the startup and teardown weight of the test binary. It did not remove the PID 1 hop, which the handoff itself lists as part of the cause (§1, item 2). If PID 1 reap latency was the real culprit, the 2/100 → 0/100 improvement is partly luck. 0/100 at a 2% base rate happens about 13% of the time.
   - **Fix (doesn't weaken the pin).** Make the test the member's parent and the reaper, as the repo already does at `:471-476` and `:774`:
     - Run a leader that blocks, for example `sh -c 'read _'`, with piped stdin.
     - Spawn the member from the test with `std::process::Command::new("cat").arg("-u").process_group(recorded.pgid)` while the leader is alive.
     - Release the leader, so it exits and is reaped.
     - In the releaser thread, do `drop(stdin); member.wait()`.
   - That removes PID 1, `echo $!` and the shell quirks on macOS and Linux. It also fixes the container-without-`--init` failure on Linux. The group stays alive through the member, the leader is gone, and the clock still runs for the full `GROUP_DRAIN_TIMEOUT`.
   - Optionally cut the 150 ms release to about 50 ms to leave more margin. That doesn't weaken the pin: any non-zero value still forces polling.

2. **Low. The sibling's `elapsed < 3s` (`:404`) is a pure wall-clock bound.**
   - A thread oversleep of more than 1 s past the 2 s deadline fails it. The lower bound `>= 2s` can't fail.
   - **Fix:** raise the upper bound to 5 s, like the other tests (`:893`, `:988`, `:1018`), or 10 s. It still pins "bounded, and it really waited 2 s".

3. **Low. The macOS CLOEXEC window is described as "in theory", but it is a plausible residual cause.**
   - Evidence: `:296-298`, `:302-303`.
   - On macOS `Stdio::piped()` is `pipe()` followed by a later flag set, and the other tests in this binary run in parallel threads. For example `fast_exiting_process…` spawns 200 shells in a tight loop (`:495-511`).
   - Suppose another test spawns in that window, while a thread is preempted under load, and the child is long-lived (`sleep 60`, the 60 s leaders at `:837`). It then holds `stdin`'s write end past 150 ms, `cat` never sees EOF, and the test returns `Unverified`.
   - Probability is low, but load widens the window, and the old test had the same exposure.
   - **Fix:** release with an explicit message, not with EOF. For example, make the member `sh -c 'read a; echo "$a"; read b'` and have the releaser write `"bye\n"`. This also makes the `cat` executable assertion unnecessary.
   - Fixing finding 1 with a test-owned `Command` also helps, but only if the other pipe ends are not inherited.

4. **Low. The handoff §3 says the other tests have no timing dependence, but several share the 2 s PID 1 reap window.**
   - `kill_recorded_kills_matching_live_orphan_group` (`:426-487`): the launcher plays init only for itself, and `sleep` is reaped by PID 1.
   - `child_spawning_grandchildren…` (`:61-108`), `leader_exiting_early…` (`:111-165`) and `terminate_blocking_after_spawning_runtime_dropped…` (`:596-642`) have the same dependence through `terminate`'s drain (`src/supervise.rs:117,149`).
   - Handoff §3 should say so. Strictly speaking they were out of scope, but a failure in any of them would look like "N17 not fixed".

5. **Low. `gone_leader_with_live_descendant_in_group_is_not_verified` (`:264-271`) polls for 100 × 10 ms for the descendant to disappear, and `pid_exists` counts zombies.**
   - It needs PID 1 to reap within about 1 s, and then another full drain succeeds at `:271`.
   - Safer to reap the descendant explicitly, or poll against a 5 s deadline.

6. **Low. Handoff §2 overstates the handshake.**
   - It says the `ready` round-trip guarantees `cat` "is reparented to PID 1 and is actively blocked in `read(0)`". The round-trip only proves one completed read and write.
   - Reparenting comes from the leader reap at `:326`.
   - Reword it.

7. **Low (pre-existing pattern). `KillPidGuard(member_pid)` can `SIGKILL` a recycled pid.**
   - `cat` exits and is reaped by PID 1 in both tests, but the guard (`:322`) still signals its number at test end.
   - Pid reuse within 2 s is very unlikely.
   - Fix: after a confirmed exit, don't signal, or rely on the group guard only.

8. **Low. The handshake read at `:339-343` has no timeout.**
   - If `cat` doesn't write (broken environment), the test hangs instead of failing. Wrap it in `tokio::time::timeout(5s)`.

9. **Low. In the Verified test, `elapsed < 2s` (`:385-388`) can fail when `Verified` is returned right at the deadline.**
   - `kill_recorded` re-checks `group_is_empty` after the loop (`:299-304`), so a drain landing at the boundary gives `Verified` with `elapsed >= 2s`. That is a spurious failure.
   - Fix: it's implied by the `Verified` assertion, so loosen it only deliberately. At minimum leave a comment.

## The 2 uncaptured failures (check 5)

I can't tell which tests failed from the code. Ranked by how likely they are to fail when CPU, memory and I/O are all loaded:

1. **`kill_recorded_returns_verified_when_gone_leader_group_drains_within_timeout` (`:371`), most likely.** It is the only test that needs a timer thread, `cat` and PID 1 to all progress inside a fixed 1.85 s window, and a stall makes the poller give up before `cat` or launchd has run (finding 1). It also has the CLOEXEC exposure (finding 3) and the boundary assertion (finding 9).
2. **The sibling's `< 3s` (`:404`).** Pure wall clock (finding 2).
3. **The untouched PID-1-reap tests** (finding 4). A failure there is not an N17 regression.
4. **No remaining timing dependence in:** the `ready` read (it blocks until `cat` echoes), the `cat` executable check (runs after `ready`, so `cat` is already exec'd), `sh`/`cat` startup (covered by the handshake), and the 150 ms sleep (shifts the release, doesn't race it). The 5 s `wait_for_*` loops have large margins.

Capturing the failure names with `--no-fail-fast` and the log would turn this ranking into a fact.

## Test gaps

- Nothing pins that a **zombie member** is `Unverified` through `kill_recorded` (only the raw `kill(-pgid, 0)` is pinned, at `:1078-1084`). An unreaped member that is the test's child should give `Unverified`, then `Verified` after the test reaps it. That makes the zombie-counts-as-not-empty decision a tested one.
- Nothing exercises the macOS `EPERM` path of the gone-leader branch.
- The handshake and the executable check have no failure-mode test, and the handshake has no timeout (finding 8).
- No load run keeps the failing test names. The handoff's §4 commands don't use `--no-fail-fast` or save output.

