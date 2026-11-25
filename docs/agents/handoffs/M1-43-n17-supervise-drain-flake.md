# Handoff: M1-43-n17-supervise-drain-flake

**Status:** Done (worker, rework round 2, D-123). Code and handoff committed on `agent/neon/M1-43-n17-supervise-drain-flake`. `cargo fmt --all` and `cargo check -p atrium-host --tests` passed cleanly. Test execution is left to the orchestrator's verification step per D-123.

## 1. Root cause (with code evidence)

### Product code check (`crates/atrium-host/src/supervise.rs`)
- `kill_recorded` (`src/supervise.rs:298-305`): when `identify(recorded.pid)` returns `Ok(None)`, it calls `wait_for_group_drain_sync(recorded.pgid, GROUP_DRAIN_TIMEOUT)` (`GROUP_DRAIN_TIMEOUT = 2s`, `src/supervise.rs:46`), then returns `Cleanup::Verified` iff `group_is_empty(recorded.pgid)`.
- `wait_for_group_drain_sync` (`src/supervise.rs:378-387`): polls `group_is_empty(pgid)` every 2 ms until `Instant::now() >= deadline`.
- `group_is_empty` (`src/supervise.rs:360-365`): calls `signal_group(pgid, 0)` (`kill(-pgid, 0)`) and returns `true` only on `ESRCH`.
- **Does a zombie member count as "not empty"?** Yes, on both platforms:
  - On **Linux**, `kill(-pgid, 0)` returns `0` while any zombie remains in the group (pinned at `tests/supervise.rs:1083-1084`).
  - On **macOS (XNU `killpg1`)**, `kill(-pgid, 0)` returns `-1` with `EPERM` when the group holds only exiting/`SZOMB` members (documented at `src/supervise.rs:175-182` and pinned at `tests/supervise.rs:1078-1082`).
  - **This is intentional product behaviour, not a bug:** on macOS, `kill(-pgid, 0)` also returns `EPERM` when `pgid` has been reused by another UID's live process, so an exiting/`SZOMB`-only group and another UID's live process group are **indistinguishable via `kill(2)`**. Although macOS could list group members via `proc_listpgrppids` or `sysctl` `KERN_PROC_PGRP` to inspect `SZOMB` state, no product change is made: treating `EPERM` as not-empty is the intentional fail-safe behavior for `Cleanup::Verified`. After a daemon crash, any surviving member whose leader exited is reparented to PID 1 (`launchd`/`init`), which reaps zombies promptly once they exit. No change to `src/supervise.rs` is warranted.

### Most likely cause of the test flake under load (`crates/atrium-host/tests/supervise.rs`)
The most likely cause — consistent with the code, though not reproduced directly with per-phase startup timing measurements — is test-side, from two compounding issues in the old `helper_gone_leader_spawns_member_holding_stdin` + `spawn_gone_leader_with_holding_member` (`tests/supervise.rs:274-352` on `dev` before this fix):

1. **No readiness handshake before starting the drain clock:** The leader spawned the member (`Command::new(current_exe)...spawn()`) and immediately printed `MEMBER:<pid>` and exited (`tests/supervise.rs:291-303` on `dev`). `spawn_gone_leader_with_holding_member` only reaped the leader and called `identify(member_pid)` (`tests/supervise.rs:347-350` on `dev`), which succeeds as soon as the kernel creates the process entry—before the member has finished loading or entered `read(stdin)`. As a result, `kill_recorded`'s 2 s `GROUP_DRAIN_TIMEOUT` started while the member was still starting up.
2. **Heavyweight member process (`current_exe`):** The member was a second copy of the full Rust debug integration test binary (`supervise`), running under `libtest` (`--exact helper_gone_leader_spawns_member_holding_stdin --nocapture --quiet`). Under CPU/I/O contention on macOS, starting, running, and tearing down a second debug test binary plus reaping the reparented zombie can exceed the 2 s drain window (1.85 s after the 150 ms release sleep).

**Empirical evidence (orchestrator's before/after run on macOS):** under the same load (20 `yes` CPU hogs, load average 15 to 39), the old test on `dev` had **2 failures in 100** (each a single test failing in ~2.02 s, matching `GROUP_DRAIN_TIMEOUT`), while the fixed test on M1-43 had **0 failures in 100** (and **0 failures in 200** quiet runs).

## 2. The fix

Changed only `crates/atrium-host/tests/supervise.rs` (no changes to `src/supervise.rs`, `GROUP_DRAIN_TIMEOUT`, or any test assertions; no retries added):

- Replaced the `current_exe` leader + `current_exe` member in `spawn_gone_leader_with_holding_member` (`tests/supervise.rs:285-368`) with a short-lived `/bin/sh` leader that spawns a lightweight POSIX `cat -u` member holding the parent's `stdin`:
  ```sh
  exec 3<&0; cat -u <&3 3<&- & echo "MEMBER:$!"; exit 0
  ```
  (`exec 3<&0` is required because non-interactive POSIX `sh` redirects `&` children's stdin to `/dev/null` before explicit redirections; `cat -u` is POSIX-standard unbuffered `cat` on both macOS and Linux.)
- Added a `"ready\n"` round-trip handshake after reaping the leader: the helper writes `"ready\n"` to `stdin`, flushes it, and reads `"ready"` back from `stdout` before returning. This guarantees `cat -u` has finished `exec`/startup, is reparented to PID 1, and is actively blocked in `read(0)` before `kill_recorded` starts the 2 s clock. When `stdin` is dropped, `cat -u` sees EOF immediately and exits.
- Asserted via `ProcessIdentity::executable` (`src/supervise.rs:28`, checked at `tests/supervise.rs:351-365`) that `member_pid` is the `cat` process holding stdin (not an intermediate subshell), accepting either `file_name() == Some("cat")` or a match against the canonicalized `cat` on `PATH` (for multicall binaries such as BusyBox / uutils).
- Documented two remaining environment notes in `spawn_gone_leader_with_holding_member`:
  - **Remaining PID 1 / subreaper dependence (`tests/supervise.rs:280-284`):** once `stdin` is dropped and `cat -u` exits, the test still relies on PID 1 (`launchd` on macOS, or `init` / a subreaper on Linux) reaping the reparented zombie within `GROUP_DRAIN_TIMEOUT` (about 1.85 s after the 150 ms release). On Linux, the test fails in environments where PID 1 does not reap orphans (for example, a container started without `--init`).
  - **Pre-existing macOS pipe CLOEXEC window (`tests/supervise.rs:296-298`):** on macOS, `Stdio::piped()` sets `FD_CLOEXEC` after `pipe()` rather than atomically, so in theory a parallel test's `fork` in that narrow window could inherit the `stdin` write end and delay `cat`'s EOF.
- Removed the now-unused `GONE_LEADER_HELPER_ENV`, `GONE_LEADER_MEMBER_ENV`, and `helper_gone_leader_spawns_member_holding_stdin`.

## 3. Sibling tests checked

- `kill_recorded_returns_unverified_and_leaves_member_alive_when_drain_times_out`: uses `spawn_gone_leader_with_holding_member`, so it automatically gets the lightweight `cat -u` member and readiness handshake. All assertions (`Cleanup::Unverified`, `2s <= elapsed < 3s`, `pid_exists(member_pid)`, `identify(member_pid).is_some()`) are unchanged, and its post-test `drop(stdin)` cleanup now completes cleanly in milliseconds instead of waiting for a debug test binary to exit.
- Other tests in `crates/atrium-host/tests/supervise.rs` (`kill_recorded_kills_matching_live_orphan_group`, `terminate[_blocking]_kills_a_leader_that_left_its_group`, `terminate[_blocking]_kills_a_leader_ignoring_sigterm_after_the_grace`) already wait for `READY:` / `LEFT ` / `ready` before starting their timed operations and terminate via `SIGKILL`.

## 4. Verification commands for the orchestrator

All commands below are fish-compatible (no `$(...)`, no `VAR=val cmd`):

1. Formatting, workspace clippy, and full workspace tests:
   ```fish
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```

2. `supervise` test binary ×200 (quiet machine):
   ```fish
   cargo test -p atrium-host --test supervise --no-run
   bash -c 'set -euo pipefail; for i in $(seq 1 200); do cargo test -p atrium-host --test supervise --quiet || { echo "failed on run $i"; exit 1; }; done; echo "200/200 passed"'
   ```

3. `supervise` test binary ×200 under CPU load (e.g. while running a parallel `cargo build` in another worktree, or with background CPU burners):
   ```fish
   bash -c '
     set -euo pipefail
     pids=()
     cleanup() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; }
     trap cleanup EXIT
     ncpu=$(sysctl -n hw.ncpu 2>/dev/null || nproc)
     for _ in $(seq 1 "$ncpu"); do
       yes >/dev/null &
       pids+=($!)
     done
     for i in $(seq 1 200); do
       cargo test -p atrium-host --test supervise --quiet || { echo "failed on run $i under load"; exit 1; }
     done
     echo "200/200 passed under CPU load ($ncpu workers)"
   '
   ```

## 5. Revert / repro steps

Brief DoD item 3 is met by the synthetic injection below together with the orchestrator's before/after load run (2 failures in 100 on `dev` vs 0 failures in 100 on M1-43 under 20 `yes` CPU hogs):

1. **Revert the fix** (on a temporary scratch checkout or with `git checkout dev -- crates/atrium-host/tests/supervise.rs`):
   - Under heavy CPU load (e.g. 20 `yes` CPU hogs or a parallel `cargo build --workspace` in another worktree), run:
     ```fish
     bash -c 'for i in $(seq 1 100); do cargo test -p atrium-host --test supervise -- --exact kill_recorded_returns_verified_when_gone_leader_group_drains_within_timeout --quiet || { echo "reproduced on run $i"; break; }; done'
     ```
2. **Deterministic on-demand synthetic injection of the startup-race mechanism:**
   - With the old `helper_gone_leader_spawns_member_holding_stdin` restored from `dev`, add `std::thread::sleep(Duration::from_millis(2500));` immediately before `std::io::Read::read(&mut std::io::stdin(), &mut buf);` inside the `GONE_LEADER_MEMBER_ENV` branch (simulating a stalled `current_exe` startup under load; 2500 ms ensures the member stays alive past `kill_recorded`'s 2 s window even though the sleep starts before `kill_recorded` begins).
   - Running `cargo test -p atrium-host --test supervise -- --exact kill_recorded_returns_verified_when_gone_leader_group_drains_within_timeout` fails after about 2 s (the full `GROUP_DRAIN_TIMEOUT`) on `assert_eq!(cleanup, Cleanup::Verified)` with `left: Unverified, right: Verified`.

## Round 2

Addressed all 7 Low findings from `docs/agents/reviews/M1-43-n17-supervise-drain-flake-round1.md`:

1. **Cause wording (§1):** Reworded as the most likely cause, consistent with the code and not reproduced directly with startup timing measurements; cited the orchestrator's before/after load run (2/100 failures on `dev` before vs 0/100 on M1-43 after under 20 `yes` CPU hogs, load 15–39, plus 0/200 quiet).
2. **Repro (§5.2):** Updated the synthetic sleep to `2500 ms`, corrected the timing description to "fails after about 2 s", removed the untested "with the fix in place" claim, and stated explicitly that DoD item 3 is met by the synthetic injection plus the before/after load run.
3. **Citations:** Corrected the zombie-pin line citations and re-checked every file/line citation against the current files (after the Round 2 additions in `spawn_gone_leader_with_holding_member`, the zombie pins are at `tests/supervise.rs:1078-1084`: macOS `1078-1082`, Linux `1083-1084`; pre-Round-2 they were at `1053-1059`).
4. **Remaining PID 1 / subreaper dependence:** Documented in both the handoff (§2) and the doc comment on `spawn_gone_leader_with_holding_member` (`tests/supervise.rs:280-284`), including the Linux failing case where PID 1 does not reap orphans (e.g. a container without `--init`).
5. **Member PID executable check:** Confirmed `ProcessIdentity` exposes `pub executable: PathBuf` (`src/supervise.rs:28`) and added an assertion in `spawn_gone_leader_with_holding_member` (`tests/supervise.rs:351-365`) verifying `member_pid` is `cat` (via `file_name() == Some("cat")` or canonicalized `cat` on `PATH`).
6. **`EPERM` wording (§1):** Updated to "indistinguishable via `kill(2)`", noted that `proc_listpgrppids` or `sysctl` `KERN_PROC_PGRP` could list members on macOS, and stated that no product change is made (fail-safe).
7. **macOS pipe CLOEXEC window:** Added a short comment in `spawn_gone_leader_with_holding_member` (`tests/supervise.rs:296-298`); no code change.

**Checks run (D-123):** `cargo fmt --all` and `cargo check -p atrium-host --tests` passed cleanly. No files under `src/` were modified.
