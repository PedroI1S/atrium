# Handoff: M1-19-host-supervision (N3, N6, N10)

- **Task:** `M1-19-host-supervision` (D-103)
- **Worker:** Gemini 4 Hydrogen (high)
- **Branch:** `agent/hydrogen/M1-19-host-supervision`
- **Code commit:** `5c2aea2`

---

## Items

### 1. N10 — macOS mid-exit `EPERM` race
- **Cause:** On macOS, when a process-group leader is mid-exit (after XNU's `killpg1` already sees zero live signalable members in `pgid` and returns `EPERM`, before `waitid(P_PID, ..., WEXITED | WNOHANG | WNOWAIT)` can report the child as a waitable zombie), `kill_group_while_unreaped` saw `Err(EPERM)` with `peek_exited(pid) == Ok(false)` and returned `Err(EPERM)` immediately from `terminate` and `terminate_blocking`.
- **Fix (`crates/atrium-host/src/supervise.rs`):**
  - Added `MACOS_EPERM_EXIT_POLL = Duration::from_millis(50)` under `#[cfg(target_os = "macos")]`.
  - Split `kill_group_while_unreaped` into `try_kill_group` (shared `signal_group(pgid, SIGKILL)` match), async `kill_group_while_unreaped` (used by `terminate`, polling `wait_until_exited_no_reap(pid, MACOS_EPERM_EXIT_POLL).await`), and blocking `kill_group_while_unreaped_blocking` (used by `terminate_blocking`, polling `wait_until_exited_no_reap_sync(pid, MACOS_EPERM_EXIT_POLL)`).
  - If `peek_exited` becomes `true` within 50 ms, it returns `Ok(true)` (`macos_zombie_eperm`) and carries on through `kill_leader_while_unreaped`, reaping, `wait_for_group_drain`, and `finish_termination` as today's exited path does; if the 50 ms bound expires without `peek_exited` becoming `true`, it returns the original `Err(EPERM)`.
- **Tests (`crates/atrium-host/tests/supervise.rs`):**
  - Added `wait_for_mid_exit_eperm_group` (reproducing M1-07c's first test: on macOS, polls only `kill(-pgid, 0) == EPERM` without waiting for `waitid`), `terminate_blocking_handles_macos_mid_exit_eperm`, and `terminate_handles_macos_mid_exit_eperm`.
  - **Failure without fix:** Ran `cargo test -p atrium-host --test supervise mid_exit_eperm` before editing `src/supervise.rs`; both tests failed immediately with `Os { code: 1, kind: PermissionDenied, message: "Operation not permitted" }`.
  - **After fix:** 30/30 runs passed (`2 passed; 0 failed` per run).

### 2. N6 — Linux `PF_EXITING` multithreaded leader, `read_linux_exe`, and `after.pgid < first.pgid`
- **Cause:**
  1. `judge_exe_read` treated `ENOENT` with `after.flags & PF_EXITING != 0` as `ExeVerdict::NoProcess` even when `after.num_threads > 1` (a multithreaded process whose leader is in `do_exit` while worker threads live).
  2. `read_linux_exe` gated its `/proc/<pid>/task/*/exe` fallback on `first.state == 'Z'`, skipping live thread links when the leader had `PF_EXITING` (between `exit_mm()` and `EXIT_ZOMBIE`) at `first`.
  3. `exe_read_decision_table` only tested `after.pgid > first.pgid` (`9000 > 100`), leaving `after.pgid != first.pgid` -> `after.pgid > first.pgid` and `PF_KTHREAD` reordering above the `pgid` check undetected.
- **Fix (`crates/atrium-host/src/supervise.rs`):**
  - Updated `judge_exe_read` so `PF_EXITING` counts as `ExeVerdict::NoProcess` only when `after.num_threads <= 1` (`(after.flags & PF_EXITING != 0 && after.num_threads <= 1) || after.flags & PF_KTHREAD != 0`).
  - Updated `read_linux_exe` to drop `|| first.state != 'Z'`, consulting `/proc/<pid>/task/*/exe` whenever `err.kind() == NotFound && first.num_threads > 1`.
- **Tests (`crates/atrium-host/src/supervise.rs`):**
  - Updated `"Z leader with a worker, no exe, exiting flag"` in `exe_read_decision_table` from `NoProcess` to `Unverifiable` (would fail without the `num_threads <= 1` guard).
  - Added `"exiting leader (not yet Z) with a worker, no exe"` (`Unverifiable`).
  - Added `"exe ok, process group went down"` (`after.pgid = 50 < 100`) and `"ENOENT, kernel thread, process group went down"` (`after.pgid = 0 < 100`, `PF_KTHREAD`) -> `Unverifiable`.

### 3. N3 — Intermittent zombie `sleep` cleanup (`Unverified`)
- **Root Cause 2 (Linux `CLONE_VFORK` in `spawn_group` / `identify_spawned`) — Fixed:**
  - **Cause:** On Linux, `tokio::process::Command::spawn()` uses `posix_spawnp` (`clone(CLONE_VM | CLONE_VFORK)`). Inside `exec_mmap`, `mm_release` -> `complete_vfork_done` wakes the parent slightly before `tsk->mm = mm` swaps the `mm`, so `identify_spawned` -> `identify(pid)` can read `/proc/<pid>/exe` from `old_mm` (the parent's executable) and record `<parent-binary>` instead of the spawned binary; `kill_recorded` later fails `live == *recorded` and returns `Cleanup::Unverified` without signalling.
  - **Fix (`crates/atrium-host/src/supervise.rs`):** Added `is_pre_exec_vfork_exe` (`#[cfg(any(target_os = "linux", test))]`) and `identify_after_vfork_exec` (`#[cfg(target_os = "linux")]`, `VFORK_EXEC_POLL = 50 ms`) in `identify_spawned`. When `observed == self_exe && expected_exe != self_exe` (where `self_exe` is `/proc/self/exe` and `expected_exe` is `resolve_spawn_executable`), it yields/polls up to 50 ms until `execve` swaps `tsk->mm` or the child exits into the zombie fallback.
  - **Test (`crates/atrium-host/src/supervise.rs`):** Added `vfork_pre_exec_predicate_detects_parent_exe_only` covering `(self_exe, sleep_exe) -> true`, `(sleep_exe, sleep_exe) -> false`, and `(self_exe, self_exe) -> false`.
- **Root Cause 1 (`kill_recorded` cannot reap a zombie it did not spawn) — Escalated / Left unchanged:**
  - **Why not changed:** Both brief-suggested mechanisms (treating a zombie-only group as empty, or waiting on the leader's exit without being its parent instead of waiting for `group_is_empty` / `ESRCH`) conflict directly with existing workspace tests and the fail-closed rule:
    1. `crates/atrium-daemon/tests/sessions.rs`: `a_dead_provider_nobody_reaped_is_not_called_stopped` (`:1425`) and `a_stopped_session_with_unverified_cleanup_becomes_interrupted` (`:1580`) both leave `script.process` (`sleep 600`) unreaped while `script.die()` triggers `kill_recorded`, and explicitly assert `ThreadState::Interrupted { cleanup: Cleanup::Unverified }`.
    2. `crates/atrium-host/tests/adversarial_supervise.rs`: `kill_recorded_setsid_grandchild_and_unreaped_zombie_leader_boundaries` (`:384`, Linux) runs `kill_recorded` while the leader is unreaped and explicitly asserts `Cleanup::Unverified` ("unreaped zombie leader keeps process group alive in kernel").
    3. Weakening evidence: waiting on leader exit alone does not verify descendants in `recorded.pgid` are gone (violates contract §10); treating zombie members as empty cannot verify descendant zombies' identity and is non-atomic against `kill(-pgid, 0)`.
    4. Additionally, in `recovery::a_killed_daemon_is_recovered_at_the_next_start` (`recovery.rs:154`), `assert!(!alive(pid))` runs `kill -0 <pid>`, which still returns true for an unreaped zombie even if `kill_recorded` returned `Verified`.
  - **Decision needed if Root Cause 1 still bites on Linux after the `CLONE_VFORK` fix:** Either keep `kill_recorded` requiring `ESRCH` (and increase `GROUP_DRAIN_TIMEOUT` if loaded CI/test runners starve Tokio's `SIGCHLD` reaper / `init` past 2 s), or explicitly approve changing contract §10 and updating those three existing tests (`sessions.rs:1425, 1580`, `adversarial_supervise.rs:384`) to use a different `Unverified` trigger (e.g., mismatched identity or live descendant after a gone leader).

---

## Run Counts (macOS)

- **N10 (`cargo test -q -p atrium-host --test supervise mid_exit_eperm`, 30 sequential runs):**
  - `terminate_blocking_handles_macos_mid_exit_eperm`: **30 / 30 passed** (0 failed)
  - `terminate_handles_macos_mid_exit_eperm`: **30 / 30 passed** (0 failed)
- **N3 sessions (`cargo test -q -p atrium-daemon --test sessions -- --exact a_process_the_adapter_left_running_is_killed_or_reported`, 30 sequential runs):**
  - **30 / 30 passed** (0 failed)
- **N3 recovery (`cargo test -q -p atrium-daemon --test recovery -- --exact a_killed_daemon_is_recovered_at_the_next_start`, 30 sequential runs):**
  - **30 / 30 passed** (0 failed)

---

## Linux-Only Paths Left for Rodrigo's Linux Shift

The following paths are `#[cfg(target_os = "linux")]` and were checked by reading only on this macOS host:
1. `read_linux_exe` (`crates/atrium-host/src/supervise.rs`): `/proc/<pid>/task/*/exe` fallback when `first.state != 'Z'` and `first.num_threads > 1` (N6).
2. `platform_identify` (`crates/atrium-host/src/supervise.rs`): end-to-end `/proc/<pid>/stat` + `read_linux_exe` + `judge_exe_read` path (N6).
3. `leader_exited::a_process_whose_main_thread_exited_is_still_identified_and_killed` (`crates/atrium-host/tests/supervise.rs`): live multithreaded Linux leader-exit test (N6).
4. `identify_after_vfork_exec` (`crates/atrium-host/src/supervise.rs`): live `/proc/self/exe` check and yield/poll loop in `spawn_group` (N3 root cause 2).
5. `kill_recorded_setsid_grandchild_and_unreaped_zombie_leader_boundaries` (`crates/atrium-host/tests/adversarial_supervise.rs`, `#[cfg_attr(not(target_os = "linux"), ignore)]`), `linux_verify_app_peer_rejects_relative_symlink_mismatch_and_deleted_exe`, and 30+ parallel/solo runs of `atrium-daemon` `sessions` + `recovery` on Linux to verify whether the `CLONE_VFORK` fix eliminates the M1-14a N3 failures.

---

## `unsafe` Block Audit (D-053)

- **`crates/atrium-host/src/supervise.rs`:** **0 `unsafe` blocks added or modified.**
- **`crates/atrium-host/tests/supervise.rs`:** 1 new test-only `unsafe` block in `wait_for_mid_exit_eperm_group` (`libc::kill(-raw, 0)` with `assert!(raw > 1)` and `// SAFETY:` comment; macOS only, signal `0` existence/permission check).

---

## Definition of Done Outputs (After Final Code Edit)

1. **`cargo fmt --all --check`:** exited `0` (no output).
2. **`cargo clippy --workspace --all-targets -- -D warnings`:** exited `0` (`Finished 'dev' profile [unoptimized + debuginfo] target(s) in 0.44s`, 0 warnings).
3. **`cargo test --workspace --no-fail-fast -q`:** exited `0` (`0 failed` across all crates):
   - `atrium-agents`: `35` (`lib`) + `38` (`claude_adapter`) + `20` (`claude_golden`) + `61` (`claude_normalizer`) + `5` (`claude_policy_e2e`) + `28` (`claude_review`) + `16` (`codex_adapter`) + `7` (`codex_golden`) + `10` (`codex_normalizer`) + `3` (`codex_policy_e2e`) + `12` (`codex_review`) + `45` (`consistency`) + `0` (`doc-tests`)
   - `atrium-app`: `2` (`lib`) + `0` (`doc-tests`)
   - `atrium-client`: `29` (`lib`) + `11` (`socket`) + `0` (`doc-tests`)
   - `atrium-core`: `9` (`lib`) + `2` (+1 ignored, `adversarial_fuzz`) + `9` (+2 ignored, `adversarial_gate`) + `14` (`adversarial_ledger`) + `16` (`adversarial_ordinary`) + `15` (+3 ignored, `adversarial_policy`) + `57` (`adversarial_regression`) + `9` (`attention`) + `25` (`history`) + `20` (`ledger`) + `44` (`policy`) + `30` (`store`) + `0` (`doc-tests`)
   - `atrium-daemon`: `15` (`lib`) + `0` (`bin`) + `3` (`adversarial_recovery`) + `7` (`adversarial_socket`) + `1` (`client`) + `2` (`gaps_full_storage`) + `2` (`gaps_snapshot`) + `7` (`recovery`) + `6` (`requests_fake`) + `2` (`requests_notifications`) + `22` (`sessions`) + `9` (`socket`) + `5` (`start_refusal`) + `0` (`doc-tests`)
   - `atrium-host`: `6` (`lib`) + `7` (+1 ignored, `adversarial_supervise`) + `5` (`lifecycle`) + `3` (`peer`) + `17` (`supervise`) + `0` (`doc-tests`)
   - `atrium-protocol`: `0` (`lib`) + `13` (`serde_shape`) + `0` (`doc-tests`)

---

## Fix round 1

### 1. `kill_recorded` gone-leader drain wait (`crates/atrium-host/src/supervise.rs:275-305`)
- **Change:** In `kill_recorded`'s `Ok(None)` branch (leader already gone, `:298-305`), added `wait_for_group_drain_sync(recorded.pgid, GROUP_DRAIN_TIMEOUT);` (`:299`) before the single `group_is_empty(recorded.pgid)` check (`:300`). No signal is sent in this branch (`group_is_empty` only calls `signal_group(pgid, 0)`), and `Cleanup::Verified` still requires `ESRCH` from `group_is_empty`. Updated `kill_recorded`'s doc comment (`:275-286`) to document the `GROUP_DRAIN_TIMEOUT` drain wait without signalling when the leader is gone.
- **`wait_for_group_drain_sync` early-return check (`crates/atrium-host/src/supervise.rs:378-387`):**
  - Line `:380` checks `while !group_is_empty(pgid)` at the top of the loop before any `std::thread::sleep` (`:385`, `Duration::from_millis(2).min(deadline - now)`).
  - When the group is already empty on entry, `group_is_empty(pgid)` returns `true`, the loop body is skipped, and `wait_for_group_drain_sync` returns immediately with zero sleep (no fixed sleep added to the common case). When a group member is still exiting, it re-checks every `<= 2 ms` and returns on the next check as soon as `group_is_empty(pgid)` becomes `true` (or `now >= deadline`).

### 2. Tests added (`crates/atrium-host/tests/supervise.rs`, `crates/atrium-host/src/supervise.rs`)
- **Gone leader, member exiting within timeout (`crates/atrium-host/tests/supervise.rs:355-376`):**
  - `kill_recorded_returns_verified_when_gone_leader_group_drains_within_timeout` (using helper `helper_gone_leader_spawns_member_holding_stdin` + `spawn_gone_leader_with_holding_member`, `:277-353`, which spawns a same-pgid member blocking on stdin, reaps only the leader, then drops stdin after `150 ms`).
  - **Before fix (parent commit `670e486`):** ran `cargo test -p atrium-host --test supervise -- --exact kill_recorded_returns_verified_when_gone_leader_group_drains_within_timeout --nocapture`; failed at `:367` with `assertion 'left == right' failed: left: Unverified, right: Verified` (`0.16s`).
  - **After fix:** passes (`Cleanup::Verified`, `< 2s`, `~0.16s`).
- **Gone leader, member outlives timeout (`crates/atrium-host/tests/supervise.rs:378-408`):**
  - `kill_recorded_returns_unverified_and_leaves_member_alive_when_drain_times_out` keeps stdin open past `GROUP_DRAIN_TIMEOUT` (`2s`), asserts `Cleanup::Unverified`, verifies `pid_exists(member_pid)` and `identify(member_pid).unwrap().is_some()` afterwards (nothing was killed), then drops stdin and waits for clean exit (`2.02s`).
- **N6 review nits in `exe_read_decision_table` (`crates/atrium-host/src/supervise.rs:1060, 1271-1277, 1313-1319`):**
  - Added `"exiting leader (not yet Z) with a worker, exe read from the worker"` (`leader_exiting` = `S | PF_EXITING` with 2 threads, `Ok(())`, `Some(leader_exiting)` -> `ExeVerdict::Identity`).
  - Added `"exiting leader, the worker is gone by the second read"` (`first.num_threads = 2`, `ENOENT`, `Some(same('S', LIVE | PF_EXITING))` with 1 thread -> `ExeVerdict::NoProcess`).
- **All new tests run under ~3 s:** `0.16s`, `2.02s`, `0.00s`.
- **`unsafe` blocks (D-053):** **0 added or modified** in round 1 (`src` and `tests`).

### 3. Round 1 Run Counts (macOS, 30 sequential runs each)
- `cargo test -q -p atrium-host --test supervise -- --exact kill_recorded_returns_verified_when_gone_leader_group_drains_within_timeout`: **30 / 30 passed** (`0` failed; `0.16s`-`0.17s` per run)
- `cargo test -q -p atrium-host --test supervise -- --exact kill_recorded_returns_unverified_and_leaves_member_alive_when_drain_times_out`: **30 / 30 passed** (`0` failed; `2.02s` per run)
- `cargo test -q -p atrium-host --lib -- --exact supervise::unit_tests::exe_read_decision_table`: **30 / 30 passed** (`0` failed; `0.00s` per run)
- `cargo test -q -p atrium-daemon --test sessions -- --exact a_process_the_adapter_left_running_is_killed_or_reported`: **30 / 30 passed** (`0` failed; `0.04s`-`0.06s` per run)
- `cargo test -q -p atrium-daemon --test recovery -- --exact a_killed_daemon_is_recovered_at_the_next_start`: **30 / 30 passed** (`0` failed; `0.09s`-`0.14s` per run)

### 4. Round 1 Definition of Done Outputs (After Final Code Edit)
1. **`cargo fmt --all --check`:** exited `0` (no output).
2. **`cargo clippy --workspace --all-targets -- -D warnings`:** exited `0` (0 warnings).
3. **`cargo test --workspace --no-fail-fast -q`:** exited `0` (`0` failed across all workspace crates; `atrium-host` `supervise` now `20 passed; 0 failed` including the helper no-op test and 2 new drain tests). *(Note: an earlier run hit an unrelated CPU-contention timing flake in `atrium-app` `shell::tests::a_draft_survives_a_crash_once_the_quiet_period_has_passed`, which passed 45/45 in `cargo test -p atrium-app --lib` and passed on the clean full workspace rerun.)*
