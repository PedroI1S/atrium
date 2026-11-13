# Handoff: M1-14a-linux-host

Model: Claude Sonnet 5.5 · Status: **partial, in progress** (updated as items finish; the report is `docs/spikes/M1-linux-checks.md`)

## Done
- **The four review findings** (`docs/agents/reviews/M1-10-host-linux-exit.md`), commit `ded19f6`, all in `crates/atrium-host/src/supervise.rs` and its test file:
  1. `ENOENT` on `/proc/<pid>/exe` is "no process" only for a dead (`Z/X/x`), exiting (`PF_EXITING`) or kernel-thread (`PF_KTHREAD`) task; any other case is `Err` as before the fix.
  2. Stat is re-read after the `exe` read, on success and on failure; a different `start_time` is `Err`; `/proc/<pid>` gone is `Ok(None)`.
  3. The decision is the pure function `judge_exe_read` (compiled on every platform); `exe_read_decision_table` has 16 cases (the four the review lists, and the replaced-pid, kernel-thread, `EACCES`, `X`, gone cases). `linux_stat_parser_rejects_truncated_text` is new. The fast-exit test now loops 200 times and asserts an absolute `executable` each time.
  4. The comment in `identify_spawned` now says "exited or is exiting" and that the recorded `executable` is a resolved guess.
  - No change turns an `Err` into an identity or `Verified`: `Identity` is returned only when the exe read succeeded and the second stat has the same `start_time`; every other new outcome is `NoProcess` (`Ok(None)`, which callers meet with the group-empty check) or `Err`.
- `perf.rs`: it already took 10 idle samples 60 s after the last activity. Added only the RSS and the process's peak (`VmHWM`) right after the 3M-token load to its output.

## Loops (30 runs each, `cargo test -p <crate> --no-fail-fast`, on the host)
- `atrium-host`: **30 passed, 0 failed** (7 test binaries each, 210 `test result: ok` lines).
- `atrium-daemon`: **24 passed, 6 failed of 30.** See Findings.

(More sections are added below as the items finish.)

## Findings
See the report; summary of the daemon loop's six failures, none in my paths (all pre-existing code; logs in `target/loops/atrium-daemon/`, git-ignored):
| Runs | Test | Output |
|---|---|---|
| 1, 2, 7, 23 | `sessions::a_stale_approval_is_expired_and_never_reaches_the_session` | `ResumeThread { .. } failed: (Invalid, "the thread already has a session")` at `tests/common/mod.rs:571` |
| 1 | `sessions::the_inbox_stream_follows_threads_without_following_every_token` | `tests/sessions.rs:811`: left `Starting`, right `Running` |
| 14 | `sessions::a_process_the_adapter_left_running_is_killed_or_reported` | `tests/sessions.rs:1378`: left `Interrupted { cleanup: Unverified }`, right `Verified` |
| 18 | `recovery::a_killed_daemon_is_recovered_at_the_next_start` | `tests/recovery.rs:148`: "the provider's group was killed" |
