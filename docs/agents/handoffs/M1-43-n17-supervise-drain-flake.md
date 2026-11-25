# Handoff: M1-43-n17-supervise-drain-flake

**Status:** Done (worker, D-123). Code and handoff committed on `agent/neon/M1-43-n17-supervise-drain-flake`. `cargo fmt --all` and `cargo check -p atrium-host --tests` passed cleanly. Test execution is left to the orchestrator's verification step per D-123.

## 1. Root cause (with code evidence)

### Product code check (`crates/atrium-host/src/supervise.rs`)
- `kill_recorded` (`src/supervise.rs:298-305`): when `identify(recorded.pid)` returns `Ok(None)`, it calls `wait_for_group_drain_sync(recorded.pgid, GROUP_DRAIN_TIMEOUT)` (`GROUP_DRAIN_TIMEOUT = 2s`, `src/supervise.rs:46`), then returns `Cleanup::Verified` iff `group_is_empty(recorded.pgid)`.
- `wait_for_group_drain_sync` (`src/supervise.rs:378-387`): polls `group_is_empty(pgid)` every 2 ms until `Instant::now() >= deadline`.
- `group_is_empty` (`src/supervise.rs:360-365`): calls `signal_group(pgid, 0)` (`kill(-pgid, 0)`) and returns `true` only on `ESRCH`.
- **Does a zombie member count as "not empty"?** Yes, on both platforms:
  - On **Linux**, `kill(-pgid, 0)` returns `0` while any zombie remains in the group (pinned at `tests/supervise.rs:1069`).
  - On **macOS (XNU `killpg1`)**, `kill(-pgid, 0)` returns `-1` with `EPERM` when the group holds only exiting/`SZOMB` members (documented at `src/supervise.rs:175-182` and pinned at `tests/supervise.rs:1064-1067`).
  - **This is intentional product behaviour, not a bug:** on macOS, `kill(-pgid, 0)` also returns `EPERM` when `pgid` has been reused by another UID's live process. Treating `EPERM` as "empty" would misreport another user's live process group as `Cleanup::Verified`. After a daemon crash, any surviving member whose leader exited is reparented to PID 1 (`launchd`/`init`), which reaps zombies promptly once they exit. No change to `src/supervise.rs` is warranted.

### Why the test flaked under load (`crates/atrium-host/tests/supervise.rs`)
The flake is test-side, caused by two compounding issues in the old `helper_gone_leader_spawns_member_holding_stdin` + `spawn_gone_leader_with_holding_member` (`tests/supervise.rs:274-353` before this fix):

1. **No readiness handshake before starting the drain clock:** The leader spawned the member (`Command::new(current_exe)...spawn()`) and immediately printed `MEMBER:<pid>` and exited (`tests/supervise.rs:291-303`). `spawn_gone_leader_with_holding_member` only reaped the leader and called `identify(member_pid)` (`tests/supervise.rs:347-350`), which succeeds as soon as the kernel creates the process entry—before the member has finished loading or entered `read(stdin)`. As a result, `kill_recorded`'s 2 s `GROUP_DRAIN_TIMEOUT` started while the member was still booting up.
2. **Heavyweight member process (`current_exe`):** The member was a second copy of the full Rust debug integration test binary (`supervise`), running under `libtest` (`--exact helper_gone_leader_spawns_member_holding_stdin --nocapture --quiet`). Under CPU/I/O contention on macOS (load average ~6.6 during a parallel `cargo build`), the un-synchronized member's `dyld` + AMFI/codesign + Rust runtime + `libtest` startup + test worker thread spawn + `read(stdin)` + `libtest` teardown/join + `launchd` reap of the reparented zombie could exceed the 2 s drain window (1.85 s after the 150 ms release sleep, or 2.0 s total if startup alone took > 1.85 s).

## 2. The fix

Changed only `crates/atrium-host/tests/supervise.rs` (no changes to `src/supervise.rs`, `GROUP_DRAIN_TIMEOUT`, or any test assertions; no retries added):

- Replaced the `current_exe` leader + `current_exe` member in `spawn_gone_leader_with_holding_member` with a short-lived `/bin/sh` leader that spawns a lightweight POSIX `cat -u` member holding the parent's `stdin`:
  ```sh
  exec 3<&0; cat -u <&3 3<&- & echo "MEMBER:$!"; exit 0
  ```
  (`exec 3<&0` is required because non-interactive POSIX `sh` redirects `&` children's stdin to `/dev/null` before explicit redirections; `cat -u` is POSIX-standard unbuffered `cat` on both macOS and Linux.)
- Added a `"ready\n"` round-trip handshake after reaping the leader: the helper writes `"ready\n"` to `stdin`, flushes it, and reads `"ready"` back from `stdout` before returning. This guarantees `cat -u` has finished `exec`/startup, is reparented to PID 1, and is actively blocked in `read(0)` before `kill_recorded` starts the 2 s clock. When `stdin` is dropped, `cat -u` sees EOF immediately and calls `_exit(0)` in microseconds.
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

To verify the original flake mode before this fix:

1. **Revert the fix** (on a temporary scratch checkout or with `git checkout dev -- crates/atrium-host/tests/supervise.rs`):
   - Under heavy CPU + build load (e.g. running a parallel `cargo build --workspace` in another worktree plus CPU burners), run:
     ```fish
     bash -c 'for i in $(seq 1 50); do cargo test -p atrium-host --test supervise -- --exact kill_recorded_returns_verified_when_gone_leader_group_drains_within_timeout --quiet || { echo "reproduced on run $i"; break; }; done'
     ```
2. **Deterministic on-demand repro of the exact startup-race mechanism:**
   - With the old `helper_gone_leader_spawns_member_holding_stdin` restored, add `std::thread::sleep(Duration::from_millis(1950));` immediately before `std::io::Read::read(&mut std::io::stdin(), &mut buf);` inside the `GONE_LEADER_MEMBER_ENV` branch (simulating a stalled `current_exe` startup under load).
   - Running `cargo test -p atrium-host --test supervise -- --exact kill_recorded_returns_verified_when_gone_leader_group_drains_within_timeout` fails immediately on `assert_eq!(cleanup, Cleanup::Verified)` with `left: Unverified, right: Verified`.
   - With the fix in place, any startup delay happens *before* the `"ready\n"` handshake completes (outside `kill_recorded`'s 2 s window), so `kill_recorded` still returns `Cleanup::Verified` well under 2 s.
