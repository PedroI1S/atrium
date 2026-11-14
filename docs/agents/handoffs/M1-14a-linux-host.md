# Handoff: M1-14a-linux-host

Model: Claude Sonnet 5.5 · Status: **done, with findings that need the orchestrator** (the daemon's test suite is flaky on Linux, from before this task; one cause is a real daemon race). The report is `docs/spikes/M1-linux-checks.md` (Host and Daemon sections; "App (M1-14b)" left empty).

## Done
- **The four review findings** (`docs/agents/reviews/M1-10-host-linux-exit.md`), commit `ded19f6`, in `crates/atrium-host/src/supervise.rs` and `crates/atrium-host/tests/supervise.rs`:
  1. `ENOENT` on `/proc/<pid>/exe` is "no process" only for a dead (`Z/X/x`), exiting (`PF_EXITING`) or kernel-thread (`PF_KTHREAD`) task; any other case is `Err`, as before the fix.
  2. Stat is re-read after the `exe` read, on success and on failure; another `start_time` is `Err`; `/proc/<pid>` gone is `Ok(None)`.
  3. The decision is the pure function `judge_exe_read`, compiled on every platform. `exe_read_decision_table` has 16 cases (the four the review lists, plus replaced pid, kernel thread, `EACCES`, `X`, entry gone). New: `linux_stat_parser_rejects_truncated_text`. The fast-exit test loops 200 times and asserts an absolute `executable` each round.
  4. The comment in `identify_spawned` now says "exited or is exiting" and that the recorded `executable` is a resolved guess.
  - No change turns an `Err` into an identity or `Verified`: `Identity` needs a successful exe read and a second stat with the same `start_time`; every other new outcome is `Ok(None)` (callers then require the group to be empty) or `Err`.
- `crates/atrium-daemon/examples/perf.rs`: it already took the 10 idle samples 60 s after the last activity. The only change: it also prints the RSS and the process peak (`VmHWM`) right after the 3M-token load (commit `154ac8c`).
- L8, the loops, the report.

## Definition of Done (last run, on the host, after the final source commit)
| Command | Result |
|---|---|
| `cargo fmt --all --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `cargo test -p atrium-host -p atrium-daemon --no-fail-fast` | exit 0 this time: host 4 unit + lifecycle 4 + peer 3 + supervise 7; daemon 13 unit + 1 + 3 + 19 (sessions) + 9 (socket) all passed. This is one lucky run, see the loop below. |
| 30-run loop `atrium-host` | **30 of 30 passed** |
| 30-run loop `atrium-daemon` | **24 of 30 passed, 6 failed** (below) |

## L8 (three runs of `target/perf/examples/perf`, gate: median idle RSS < 50 MB)
| | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| Idle RSS, median of 10 samples 60 s after the last activity | 18.5 MiB | 18.4 MiB | 18.5 MiB |
| Idle RSS, max of the 10 | 18.5 MiB | 18.4 MiB | 18.5 MiB |
| Peak RSS during/right after the 3M-token load | 15.3 MiB | 15.3 MiB | 15.4 MiB |
| Peak after the sessions closed | 15.3 MiB | 15.3 MiB | 15.5 MiB |
| Reconnect 1,000 behind (median / p95) | 3.53 / 6.45 ms | 2.61 / 3.33 ms | 3.31 / 6.74 ms |
| 100k replay | 188 to 207 ms | 158 to 178 ms | 159 to 175 ms |

**Gate met in all three.** Other load: the desktop only; no other worker was building (load average 1.6 to 2.3). The idle RSS is about 3 MiB above the process peak recorded when the sessions closed, and flat afterwards (see the report). Full outputs in the report; raw files in `target/l8/` (git-ignored).

## Findings (the six failures of the daemon loop)
Failing runs 1, 2, 7, 14, 18, 23 of 30. Logs in `target/loops/atrium-daemon/` (git-ignored).

| Runs | Test | Output |
|---|---|---|
| 1, 2, 7, 23 | `sessions::a_stale_approval_is_expired_and_never_reaches_the_session` | `ResumeThread { .. } failed: (Invalid, "the thread already has a session")` at `tests/common/mod.rs:571` |
| 1 | `sessions::the_inbox_stream_follows_threads_without_following_every_token` | `tests/sessions.rs:811`: left `Starting`, right `Running` |
| 14 | `sessions::a_process_the_adapter_left_running_is_killed_or_reported` | `tests/sessions.rs:1378`: left `Interrupted { cleanup: Unverified }`, right `Verified` |
| 18 | `recovery::a_killed_daemon_is_recovered_at_the_next_start` | `tests/recovery.rs:148`: "the provider's group was killed" |

**Not caused by this task's change.** Base commit `8018e5f` (extracted with `git archive` into `target/base-src`, own target folder) vs this branch, `--test sessions` 60 runs each, idle machine: base 8 failed runs (5 `already has a session`, 3 `Unverified`), branch 5 failed runs (4 and 3; some runs had both). `recovery` test alone, 100 runs: base 1 failure (same message and line), branch 0. The single `a_process_the_adapter_left_running...` test alone, 100 runs each: 0 failures on both, so it needs the whole suite running.

Causes, read from the code (not proven by instrumentation; none of these files is mine):
- **`already has a session` is a real daemon race, not only a test one.** `crates/atrium-daemon/src/sessions.rs` `run()` stores `SessionEnded` (the summary becomes `Ended`) and only at its very end removes the thread from `Inner::sessions`; `launch()` refuses a thread still in the map. A client that sees `Ended` and resumes straight away gets `Invalid`. The app's Resume button will do this. Needs a daemon change (remove the entry before the final state is published, or let `launch` replace a finished session). Suggest a change request or a small task for the daemon's owner.
- inbox test: it drains `watcher.aside` right after a `ListInbox` answer and expects `ThreadUpdated { Running }` already there; that frame is on another lane. Test ordering race.
- `Unverified`/recovery: both go through `kill_recorded` on a `sleep 600` that is a zombie until reaped (the test's reaper task, or init). Either the 2 s drain ended before the reaper ran, or the test looked before init reaped it. Not established which.

## Not done / not covered
- The Linux-only branches no test reaches are listed in the report (peer: error paths and the whole release-identity success path in `check_root_owned_tree`; supervise: real `PF_EXITING`/`PF_KTHREAD` tasks and pid replacement are only in the pure table; lifecycle: the XDG fallbacks). `SO_PEERPIDFD` is exercised on kernel 7.3.3, by inference from the passing self-verification (no `strace` here); not tried on a kernel older than 6.5.
- Whether the 200-round fast-exit test ever lands in the "exiting, no exe yet" window was not measured (needs a counter in the source).
- The idle RSS growth of about 3 MiB after the work ends was not investigated.
- `crates/atrium-host/tests/adversarial_supervise.rs` (M1-13) is on another branch and was not seen here; it may exercise `identify` with the new behaviour, so run it after merging.

## Gotchas
- `cargo test` of `atrium-daemon` takes 22 to 31 s a run; the loop took about 12 minutes.
- Stopping everything: no process of mine is left (checked with `ps` for `atriumd`, `perf`, `sleep 600`, and the worktree path). The temporary folders of the perf runs are `/tmp/atrium-perf*` (removed by their tempdir guards).
- `target/` holds scripts I wrote (`loop.sh`, `repeat.sh`, `l8.sh`, `compare*.sh`, `dod.sh`) and the logs; it is git-ignored and not part of the branch. The commands in the report are the cargo lines; the scripts only repeat them.
- I used a shell heredoc once to append prose (not test data, no `EOF` inside it) to this handoff; the rest of the files were written with the file tools.

## Fix round 1

### Review findings (1–4 and nit)
- **Finding 1** (`823aa7e`): `judge_exe_read` now treats a changed `pgid` between `first` and `after` stat reads as `ExeVerdict::Unverifiable`.
- **Nit (`PF_FORKNOEXEC`)** (`823aa7e`): `0x0040_0000` in `supervise.rs` test flags is now labelled `PF_RANDOMIZE` instead of `PF_FORKNOEXEC`.
- **Finding 2** (`4dcf30a`): `num_threads` (stat field 20) is parsed; a `Z` thread-group leader with other live threads is judged as live and its executable is read from a live thread when `/proc/<pid>/exe` is `ENOENT`, with a new integration test.
- **Finding 3** (`2cd28c3`): added decision-table rows to `exe_read_decision_table` that kill all four review mutations plus four additional ones.
- **Finding 4** (`6c61e73`): updated `crates/atrium-daemon/examples/perf.rs` (`wait_for_snapshot`) to wait for `upkeep::snapshot_daily`'s first `VACUUM INTO` snapshot in `ATRIUM_HOME/snapshots` to appear and settle (size > 0, `len` and `modified` unchanged for 4 x 250 ms polls + 1 s settle for `record_snapshot`) before starting the 60 s idle window, instead of sleeping 60 s from session close (which previously overlapped the snapshot at ~42 s). `perf.rs` prints the snapshot's filename, size, `modified` epoch ms, and RSS/peak, followed by the idle window's start epoch ms and elapsed time since daemon start, proving the snapshot finished before the idle window began. `crates/atrium-daemon/src/**` was not modified.

### L8 (three runs after finding 4 fix, gate: median idle RSS < 50 MB)
Note: `cat /proc/loadavg`, `uptime`, and `ps` were denied by the worker command policy, so load average could not be recorded and process cleanup relies on `perf`'s `ShutdownDaemon { force: true }` and `Daemon::drop` (`kill` + `wait`).

| | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| Idle RSS, median of 10 samples 60 s after last activity (snapshot included) | **18.4 MiB** | **18.6 MiB** | **18.6 MiB** |
| Idle RSS, max of the 10 | 18.4 MiB | 18.6 MiB | 18.6 MiB |
| Snapshot finished (`modified` ms) -> idle window start (ms) | `1794672326632` -> `1794672328513` | `1794672464979` -> `1794672466769` | `1794672602715` -> `1794672604651` |

Gate met in all three runs (18.4–18.6 MiB = 19.3–19.5 MB < 50 MB). Full raw outputs are in `docs/spikes/M1-linux-checks.md`.

### Definition of Done (after final edit)
- `cargo fmt --all --check`: exit 0
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0 (`Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 1.13s`)
- `cargo test -p atrium-host -p atrium-daemon --no-fail-fast`: exit 0:
  - `atrium-daemon` unit (`src/lib.rs`): `test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s`
  - `atrium-daemon` bin (`src/main.rs`): `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s`
  - `atrium-daemon` `tests/client.rs`: `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s`
  - `atrium-daemon` `tests/recovery.rs`: `test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s`
  - `atrium-daemon` `tests/sessions.rs`: `test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.11s`
  - `atrium-daemon` `tests/socket.rs`: `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 17.68s`
  - `atrium-host` unit (`src/lib.rs`): `test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s`
  - `atrium-host` `tests/lifecycle.rs`: `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s`
  - `atrium-host` `tests/peer.rs`: `test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s`
  - `atrium-host` `tests/supervise.rs`: `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.48s`
