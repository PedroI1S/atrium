# Handoff: M1-43-n17-supervise-drain-flake

**Status:** Done (worker, rework round 3, D-123). Code and handoff committed on `agent/neon/M1-43-n17-supervise-drain-flake`. `cargo fmt --all` and `cargo check -p atrium-host --tests` passed cleanly. Test execution is left to the orchestrator's verification step per D-123.

## 1. Root cause (with code evidence)

### Product code check (`crates/atrium-host/src/supervise.rs`)
- `kill_recorded` (`src/supervise.rs:298-305`): when `identify(recorded.pid)` returns `Ok(None)`, it calls `wait_for_group_drain_sync(recorded.pgid, GROUP_DRAIN_TIMEOUT)` (`GROUP_DRAIN_TIMEOUT = 2s`, `src/supervise.rs:46`), then returns `Cleanup::Verified` iff `group_is_empty(recorded.pgid)`.
- `wait_for_group_drain_sync` (`src/supervise.rs:378-387`): polls `group_is_empty(pgid)` every 2 ms until `Instant::now() >= deadline`.
- `group_is_empty` (`src/supervise.rs:360-365`): calls `signal_group(pgid, 0)` (`kill(-pgid, 0)`) and returns `true` only on `ESRCH`.
- **Does a zombie member count as "not empty"?** Yes, on both platforms:
  - On **Linux**, `kill(-pgid, 0)` returns `0` while any zombie remains in the group (pinned at `tests/supervise.rs:1174-1175`).
  - On **macOS (XNU `killpg1`)**, `kill(-pgid, 0)` returns `-1` with `EPERM` when the group holds only exiting/`SZOMB` members (documented at `src/supervise.rs:175-182` and pinned at `tests/supervise.rs:1169-1173`).
  - **This is intentional product behaviour, not a bug:** on macOS, `kill(-pgid, 0)` also returns `EPERM` when `pgid` has been reused by another UID's live process, so an exiting/`SZOMB`-only group and another UID's live process group are **indistinguishable via `kill(2)`**. Although macOS could list group members via `proc_listpgrppids` or `sysctl` `KERN_PROC_PGRP` to inspect `SZOMB` state, no product change is made: treating `EPERM` as not-empty is the intentional fail-safe behavior for `Cleanup::Verified`. After a daemon crash, any surviving member whose leader exited is reparented to PID 1 (`launchd`/`init`), which reaps zombies promptly once they exit. No change to `src/supervise.rs` is warranted.

### Most likely cause of the test flake under load (`crates/atrium-host/tests/supervise.rs`)
The most likely cause — consistent with the code, though not reproduced directly with per-phase startup timing measurements — is test-side, from compounding issues in the old `helper_gone_leader_spawns_member_holding_stdin` + `spawn_gone_leader_with_holding_member` (`tests/supervise.rs:274-352` on `dev` before this fix):

1. **No readiness handshake before starting the drain clock:** The leader spawned the member (`Command::new(current_exe)...spawn()`) and immediately printed `MEMBER:<pid>` and exited (`tests/supervise.rs:291-303` on `dev`). `spawn_gone_leader_with_holding_member` only reaped the leader and called `identify(member_pid)` (`tests/supervise.rs:347-350` on `dev`), which succeeds as soon as the kernel creates the process entry—before the member has finished loading or entered `read(stdin)`. As a result, `kill_recorded`'s 2 s `GROUP_DRAIN_TIMEOUT` started while the member was still starting up.
2. **Heavyweight member process (`current_exe`) + PID 1 reap dependence:** The member was a second copy of the full Rust debug integration test binary (`supervise`), running under `libtest` (`--exact helper_gone_leader_spawns_member_holding_stdin --nocapture --quiet`), and was reparented to PID 1 (`launchd`/`init`) when the leader exited. Under CPU/I/O contention on macOS, starting, running, tearing down a second debug test binary, and waiting for PID 1 to reap the zombie can exceed the 2 s drain window.
3. **EOF release exposed to macOS non-atomic CLOEXEC window:** Releasing the member by closing `stdin` (EOF) meant that if a parallel test's `fork` inherited the write end during macOS's non-atomic `Stdio::piped()` CLOEXEC window, the member would never see EOF within the drain window.

**Empirical evidence (orchestrator's before/after run on macOS):** under the same load (20 `yes` CPU hogs, load average 15 to 39), the old test on `dev` had **2 failures in 100** (each a single test failing in ~2.02 s, matching `GROUP_DRAIN_TIMEOUT`), while the Round 2 fix had **0 failures in 100** (and **0 failures in 200** quiet runs), though an earlier 2×ncpu + concurrent-build run at load 71 had 2 uncaptured failures that motivated removing the remaining PID 1 reap and CLOEXEC EOF dependencies in Round 3.

## 2. The fix

Changed only `crates/atrium-host/tests/supervise.rs` (no changes to `src/supervise.rs`, `GROUP_DRAIN_TIMEOUT`, or any product behavior; no retries added):

- **Test owns and reaps the group member (no PID 1 reap hop):** In `spawn_gone_leader_with_holding_member` (`tests/supervise.rs:311-392`), the leader is `/bin/sh -c 'read _'` with piped stdin. While the leader is alive, the test spawns the lightweight member directly with `std::process::Command::new("/bin/sh").arg("-c").arg("read a && echo \"$a\" && read b").process_group(recorded.pgid)` so this test is the member's parent and reaper (as the file already does at `:565` and `:865`). The helper then releases the leader with `"bye\n"` and reaps it (`sup.child.wait().await`), leaving only the test-owned member in `recorded.pgid` (`identify(recorded.pid) == Ok(None)` and `identify(member_pid).pgid == recorded.pgid`). This removes PID 1 (`launchd`/`init`) from the drain window on macOS and Linux (including containers without `--init`).
- **Explicit message release (no EOF / macOS CLOEXEC exposure):** Both the leader (`read _`) and member (`read a && echo "$a" && read b`) are released by writing an explicit `"bye\n"` line rather than closing stdin for EOF, so an inherited write end from a parallel test's `fork` during macOS's non-atomic `Stdio::piped()` CLOEXEC window cannot keep either process alive. Because the test spawns `/bin/sh` directly using shell builtins (`read`/`echo`, no background `cat` child), the Round 2 `cat` executable assertion is no longer needed and was dropped.
- **Bounded readiness handshake:** After reaping the leader, `spawn_gone_leader_with_holding_member` writes `"ready\n"` to `member_stdin` and reads `"ready"` back from `member_stdout` under a 5 s `tokio::time::timeout` (`tests/supervise.rs:364-384`, failing with a clear panic message on timeout). The `"ready\n"` round-trip proves the member has finished `exec`/startup and completed one read and write on its pipe before entering `read b` immediately after; leader exit is guaranteed by `sup.child.wait().await` and `identify(recorded.pid) == Ok(None)`.
- **Verified drain test (`tests/supervise.rs:395-434`):** The releaser thread sleeps 50 ms (still well above `wait_for_group_drain_sync`'s 2 ms poll interval so `kill_recorded` must poll while leaving ~1.95 s of margin), writes `"bye\n"`, calls `member.wait()`, and disarms `KillPidGuard` (`KillPidGuard::disarm`, `tests/supervise.rs:47-53`) so `Drop` never sends `SIGKILL` to a reaped pid. The upper bound is `elapsed < GROUP_DRAIN_TIMEOUT + Duration::from_secs(3)` (5 s) with a comment explaining that `kill_recorded` re-checks `group_is_empty` after the 2 s loop, so `Verified` at the boundary is legal and the bound pins bounded completion while keeping `Cleanup::Verified`, `!pid_exists(member_pid)`, and `!group_exists(recorded.pgid)` unchanged.
- **Unverified timeout sibling (`tests/supervise.rs:437-469`):** Keeps `elapsed >= GROUP_DRAIN_TIMEOUT` (2 s), raises the upper bound from 3 s to 5 s to match the other bounded tests in `supervise.rs`, and cleans up by writing `"bye\n"`, calling `member.wait()`, and disarming `KillPidGuard`.
- **Descendant test (`gone_leader_with_live_descendant_in_group_is_not_verified`, `tests/supervise.rs:228-290`):** Spawns `sleep 60` directly as this test's child in `recorded.pgid`, releases and reaps the leader, and kills + reaps the descendant explicitly (`descendant.kill()`, `descendant.wait()`, `desc_guard.disarm()`), removing the 100 × 10 ms PID 1 reap poll.
- **Zombie-member test (`kill_recorded_treats_unreaped_zombie_member_as_unverified_until_reaped`, `tests/supervise.rs:472-514`):** Pins the zombie-counts-as-not-empty decision through `kill_recorded` on both macOS and Linux. After releasing the member without reaping it and waiting for `exited_unreaped(member_pid)`, `kill_recorded(&recorded)` returns `Cleanup::Unverified`; once the test calls `member.wait()` and disarms `KillPidGuard`, a second `kill_recorded(&recorded)` returns `Cleanup::Verified`.
- Removed the unused `GONE_LEADER_HELPER_ENV`, `GONE_LEADER_MEMBER_ENV`, and `helper_gone_leader_spawns_member_holding_stdin`.

## 3. Sibling tests checked & out-of-scope follow-ups

- `kill_recorded_returns_unverified_and_leaves_member_alive_when_drain_times_out` (`tests/supervise.rs:437-469`): updated via `spawn_gone_leader_with_holding_member` to use test-owned member reaping, explicit `"bye\n"` release, disarmed `KillPidGuard`, and `< 5s` upper bound (keeping `>= 2s` lower bound and all liveness assertions).
- `gone_leader_with_live_descendant_in_group_is_not_verified` (`tests/supervise.rs:228-290`): updated to spawn and reap its descendant directly instead of polling 100 × 10 ms for PID 1.
- **Out-of-scope follow-up — other tests sharing the 2 s PID 1 reap window:** The following existing tests in `crates/atrium-host/tests/supervise.rs` were left untouched as out of scope for N17, but still depend on PID 1 (`launchd`/`init`) reaping grandchildren or killed group members within `GROUP_DRAIN_TIMEOUT` (2 s) through `terminate` / `terminate_blocking` (`src/supervise.rs:117, 149`) or `kill_recorded`:
  - `child_spawning_grandchildren_is_fully_gone_after_terminate` (`tests/supervise.rs:69-116`)
  - `leader_exiting_early_still_has_descendants_killed` (`tests/supervise.rs:119-173`)
  - `kill_recorded_kills_matching_live_orphan_group` (`tests/supervise.rs:517-578`, where the launcher reaps itself but `sleep 60` is reaped by PID 1)
  - `terminate_blocking_kills_group_with_grandchild_while_spawning_runtime_is_blocked` (`tests/supervise.rs:615-684`)
  - `terminate_blocking_after_spawning_runtime_dropped_handles_early_exit_leader` (`tests/supervise.rs:687-733`)
  A failure in one of these under extreme load would reflect their own PID 1 reap dependence rather than an N17 regression; tracking them in a separate follow-up is recommended if they ever flake under heavy load.

## 4. Verification commands for the orchestrator

All commands below are fish-compatible (no outer `$(...)`, no `VAR=val cmd`). Every loop passes `--no-fail-fast` and saves per-run logs so failing test names and output are never lost:

1. Formatting, workspace clippy, and full workspace tests:
   ```fish
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace --no-fail-fast
   ```

2. `supervise` test binary ×200 (quiet machine), preserving per-run logs on failure:
   ```fish
   cargo test -p atrium-host --test supervise --no-run
   bash -c '
     set -euo pipefail
     logdir=$(mktemp -d /tmp/m1-43-supervise-quiet.XXXXXX)
     echo "logs: $logdir"
     fails=0
     for i in $(seq 1 200); do
       log="$logdir/run-$i.log"
       if cargo test -p atrium-host --test supervise --no-fail-fast >"$log" 2>&1; then
         rm -f "$log"
       else
         fails=$((fails + 1))
         echo "FAIL run $i (saved: $log)"
         grep -E "^test .* FAILED$|^failures:|panicked at" "$log" || tail -n 40 "$log"
       fi
     done
     echo "quiet summary: $((200 - fails))/200 passed, $fails failed (logs in $logdir)"
     test "$fails" -eq 0
   '
   ```

3. `supervise` test binary ×200 under CPU load (e.g. while running a parallel `cargo build` in another worktree, or with background CPU burners), preserving per-run logs on failure:
   ```fish
   bash -c '
     set -euo pipefail
     logdir=$(mktemp -d /tmp/m1-43-supervise-load.XXXXXX)
     echo "logs: $logdir"
     pids=()
     cleanup() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; }
     trap cleanup EXIT
     ncpu=$(sysctl -n hw.ncpu 2>/dev/null || nproc)
     for _ in $(seq 1 "$ncpu"); do
       yes >/dev/null &
       pids+=($!)
     done
     fails=0
     for i in $(seq 1 200); do
       log="$logdir/run-$i.log"
       if cargo test -p atrium-host --test supervise --no-fail-fast >"$log" 2>&1; then
         rm -f "$log"
       else
         fails=$((fails + 1))
         echo "FAIL run $i under load (saved: $log)"
         grep -E "^test .* FAILED$|^failures:|panicked at" "$log" || tail -n 40 "$log"
       fi
     done
     echo "load summary ($ncpu workers): $((200 - fails))/200 passed, $fails failed (logs in $logdir)"
     test "$fails" -eq 0
   '
   ```

## 5. Revert / repro steps

Brief DoD item 3 is met by the synthetic injection below together with the orchestrator's before/after load run (2 failures in 100 on `dev` vs 0 failures in 100 on M1-43 under 20 `yes` CPU hogs):

1. **Revert the fix** (on a temporary scratch checkout or with `git checkout dev -- crates/atrium-host/tests/supervise.rs`):
   - Under heavy CPU load (e.g. 20 `yes` CPU hogs or a parallel `cargo build --workspace` in another worktree), run:
     ```fish
     bash -c 'for i in $(seq 1 100); do cargo test -p atrium-host --test supervise --no-fail-fast -- --exact kill_recorded_returns_verified_when_gone_leader_group_drains_within_timeout || { echo "reproduced on run $i"; break; }; done'
     ```
2. **Deterministic on-demand synthetic injection of the startup-race mechanism:**
   - With the old `helper_gone_leader_spawns_member_holding_stdin` restored from `dev`, add `std::thread::sleep(Duration::from_millis(2500));` immediately before `std::io::Read::read(&mut std::io::stdin(), &mut buf);` inside the `GONE_LEADER_MEMBER_ENV` branch (simulating a stalled `current_exe` startup under load; 2500 ms ensures the member stays alive past `kill_recorded`'s 2 s window even though the sleep starts before `kill_recorded` begins).
   - Running `cargo test -p atrium-host --test supervise -- --exact kill_recorded_returns_verified_when_gone_leader_group_drains_within_timeout` fails after about 2 s (the full `GROUP_DRAIN_TIMEOUT`) on `assert_eq!(cleanup, Cleanup::Verified)` with `left: Unverified, right: Verified`.

## Round 2

Addressed all 7 Low findings from `docs/agents/reviews/M1-43-n17-supervise-drain-flake-round1.md`:

1. **Cause wording (§1):** Reworded as the most likely cause, consistent with the code and not reproduced directly with startup timing measurements; cited the orchestrator's before/after load run (2/100 failures on `dev` before vs 0/100 on M1-43 after under 20 `yes` CPU hogs, load 15–39, plus 0/200 quiet).
2. **Repro (§5.2):** Updated the synthetic sleep to `2500 ms`, corrected the timing description to "fails after about 2 s", removed the untested "with the fix in place" claim, and stated explicitly that DoD item 3 is met by the synthetic injection plus the before/after load run.
3. **Citations:** Corrected the zombie-pin line citations and re-checked every file/line citation against the current files.
4. **Remaining PID 1 / subreaper dependence:** Documented in Round 2; subsequently eliminated in Round 3.
5. **Member PID executable check:** Added in Round 2 for `cat`; subsequently superseded in Round 3 when the test became the direct parent of `/bin/sh`.
6. **`EPERM` wording (§1):** Updated to "indistinguishable via `kill(2)`", noted that `proc_listpgrppids` or `sysctl` `KERN_PROC_PGRP` could list members on macOS, and stated that no product change is made (fail-safe).
7. **macOS pipe CLOEXEC window:** Noted in Round 2; subsequently eliminated in Round 3 by explicit message release.

## Round 3

Addressed all 9 findings and the zombie-member test gap from `.orchestration/review-input/M1-43-n17-supervise-drain-flake-round2.md`:

1. **Finding 1 (PID 1 reap removed from gone-leader drain tests):** Rewrote `spawn_gone_leader_with_holding_member` (`tests/supervise.rs:311-392`) so the leader blocks on `read _`, the test spawns the member directly with `process_group(recorded.pgid)` while the leader is alive, releases and reaps the leader, and the releaser thread in `kill_recorded_returns_verified_when_gone_leader_group_drains_within_timeout` (`tests/supervise.rs:395-434`) releases the member and calls `member.wait()`. Shortened the release delay from 150 ms to 50 ms (still well above the 2 ms drain poll interval, so `kill_recorded` must enter its polling loop while leaving ~1.95 s of margin).
2. **Finding 2 (sibling upper bound):** Raised the upper bound in `kill_recorded_returns_unverified_and_leaves_member_alive_when_drain_times_out` (`tests/supervise.rs:450`) from `3s` to `5s` (matching the other bounded tests in the file) while keeping `elapsed >= GROUP_DRAIN_TIMEOUT` (2 s).
3. **Finding 3 (explicit message release instead of EOF):** Changed the member command to `/bin/sh -c 'read a && echo "$a" && read b'` (and leader to `/bin/sh -c 'read _'`) and released both by writing `"bye\n"`, so an inherited write end during the macOS non-atomic CLOEXEC window cannot keep either process alive. Dropped the `cat` executable assertion because the test now spawns `/bin/sh` directly with builtins (no background `cat` child).
4. **Finding 4 (other PID-1-reap tests listed in §3):** Updated §3 to list the untouched out-of-scope tests that share the 2 s PID 1 reap window (`child_spawning_grandchildren_is_fully_gone_after_terminate`, `leader_exiting_early_still_has_descendants_killed`, `kill_recorded_kills_matching_live_orphan_group`, `terminate_blocking_kills_group_with_grandchild_while_spawning_runtime_is_blocked`, and `terminate_blocking_after_spawning_runtime_dropped_handles_early_exit_leader`) as a follow-up.
5. **Finding 5 (`gone_leader_with_live_descendant_in_group_is_not_verified`):** Updated `tests/supervise.rs:228-290` to spawn `sleep 60` directly in `recorded.pgid` as the test's child and reap it explicitly (`descendant.kill()`, `descendant.wait()`, `desc_guard.disarm()`), removing the 100 × 10 ms PID 1 poll.
6. **Finding 6 (handshake wording in §2):** Reworded §2 so it accurately states that the `"ready\n"` round-trip proves one completed read and write after `exec`/startup (not reparenting or being blocked in `read(0)`), and that leader exit is guaranteed by `sup.child.wait().await` and `identify(recorded.pid) == Ok(None)`.
7. **Finding 7 (disarm `KillPidGuard` after reaping):** Added `KillPidGuard::disarm` (`tests/supervise.rs:47-53`) and called it immediately after `wait()` in all updated/new tests (`:287`, `:414`, `:466`, `:508`) so `Drop` never sends `SIGKILL` to an already-reaped pid.
8. **Finding 8 (handshake read timeout):** Wrapped the readiness read in `tokio::time::timeout(Duration::from_secs(5), ...)` (`tests/supervise.rs:370-382`) with a clear panic message (`"timed out after 5s waiting for member readiness handshake"`).
9. **Finding 9 (Verified test elapsed bound):** Changed `elapsed < 2s` to `elapsed < GROUP_DRAIN_TIMEOUT + Duration::from_secs(3)` (`tests/supervise.rs:428-431`) with a comment explaining that `kill_recorded` re-checks `group_is_empty` after the loop, so `Verified` at the boundary is legal; kept `Cleanup::Verified`, `!pid_exists(member_pid)`, and `!group_exists(recorded.pgid)` unchanged.
10. **Test gap (zombie member through `kill_recorded`):** Added `kill_recorded_treats_unreaped_zombie_member_as_unverified_until_reaped` (`tests/supervise.rs:472-514`), pinning on both macOS and Linux that an exited, unreaped test-owned member in `recorded.pgid` gives `Cleanup::Unverified` and, after `member.wait()`, a second call gives `Cleanup::Verified`.
11. **Verification commands (§4):** Updated §4 to pass `--no-fail-fast` and save per-run logs on failure so failing test names and output are never lost.

**Checks run (D-123):** `cargo fmt --all` and `cargo check -p atrium-host --tests` passed cleanly. No files under `src/` were modified.
