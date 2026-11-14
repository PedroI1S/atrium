# M1 Linux checks (M1-14a: host and daemon)

Machine: Linux 7.3.3 (CachyOS), x86_64, the developer's own desktop. All `cargo` commands ran on the host through `distrobox-host-exec`, not in the agent container. Date: 2026-11-12 and 2026-11-13. Branch `agent/sonnet/M1-14a-linux-host`.

What else was running: the desktop session (Claude desktop, a browser, gnome-shell) and, on 2026-11-12, other workers' builds and tests (load average 4 to 7 when the host loop started). On 2026-11-13, at the time of the L8 runs, the load average was 1.5 to 2.3 and no other worker was compiling (the first run started right after my own build: load 4.6 before, 2.2 after; runs 2 and 3 started at 2.2 and 1.6). Each run's load and busiest processes are in `target/l8/run-N.env` (git-ignored).

## Host

### The four findings of the review of M1-10's Linux fix
All in `crates/atrium-host/src/supervise.rs` (commit `ded19f6`). See `docs/agents/reviews/M1-10-host-linux-exit.md`.
- `identify` on Linux reads `stat`, then `exe`, then `stat` again. `judge_exe_read` (pure, compiled on all platforms) decides: `/proc` entry gone: `Ok(None)`; other `start_time`: `Err`; exe read: `Identity`; exe error `ENOENT` with a dead, `PF_EXITING` or `PF_KTHREAD` task: `Ok(None)`; any other exe error of a live task: `Err`.
- `LinuxStat` now carries the flags (stat field 9). `parse_linux_proc_stat` returns it.
- Unit tests: `exe_read_decision_table` (16 cases), `linux_stat_parser_rejects_truncated_text`, and the existing parser test now checks flags. The fast-exit integration test loops 200 times.
- Nothing turns an `Err` into an identity or `Verified`.

### The 30-run loops
Command (in `target/loop.sh`): `cargo test -p <crate> --no-fail-fast`, 30 times in a row, one log per run.

| Crate | Result |
|---|---|
| `atrium-host` | **30 of 30 passed** (7 test binaries per run, every `test result` line `ok`; a run took 0 to 1 s) |
| `atrium-daemon` | **24 of 30 passed, 6 failed** (runs 1, 2, 7, 14, 18, 23; a run took 22 to 31 s) |

The six failing runs, with their output:

| Run | Test | Output |
|---|---|---|
| 1, 2, 7, 23 | `sessions::a_stale_approval_is_expired_and_never_reaches_the_session` | `ResumeThread { thread_id: ThreadId("th-...-1") } failed: (Invalid, "the thread already has a session")` at `tests/common/mod.rs:571:37` |
| 1 | `sessions::the_inbox_stream_follows_threads_without_following_every_token` | `assertion left == right failed, left: Starting, right: Running` at `tests/sessions.rs:811:5` |
| 14 | `sessions::a_process_the_adapter_left_running_is_killed_or_reported` | `left: Interrupted { cleanup: Unverified }, right: Interrupted { cleanup: Verified }` at `tests/sessions.rs:1378:5` |
| 18 | `recovery::a_killed_daemon_is_recovered_at_the_next_start` | `the provider's group was killed` at `tests/recovery.rs:148:5` |

Not retried away. To see whether the `supervise.rs` fix caused them, the base commit (`8018e5f`, extracted with `git archive`, built in its own target folder) was run against this branch on the idle machine:

| | Base `8018e5f` | This branch |
|---|---|---|
| `--test sessions`, 60 runs | 8 runs failed (5 x `already has a session`, 3 x `Unverified` for `Verified`) | 5 runs failed (4 x `already has a session`, 3 x `Unverified` for `Verified`) |
| `a_process_the_adapter_left_running_is_killed_or_reported` alone, 100 runs | 0 failed | 0 failed |
| `recovery::a_killed_daemon_is_recovered_at_the_next_start` alone, 100 runs | 1 failed (same message and line) | 0 failed |

So the failures predate this task and occur at a similar rate before and after. They are in `atrium-daemon` code and tests that are not this task's paths, so they are reported, not fixed.

Causes (from reading the code; not proven by instrumentation):
1. **`already has a session`: a real daemon race.** `sessions::run` stores `SessionEnded`, which makes the thread `Ended`, and removes the thread from `Inner::sessions` only at the end of the task. `launch` refuses a thread still in that map. A client that sees `Ended` and resumes straight away is refused with `Invalid`. The app's Resume will do exactly this. A fix belongs to the daemon's owner (remove the entry before publishing the final state, or let `launch` replace a finished session).
2. **inbox test:** the test drains `watcher.aside` right after a `ListInbox` answer and expects a `ThreadUpdated { Running }` to be there; that frame is on another lane and can come later. A test ordering race. Seen once.
3. **`Unverified` for `Verified`, and the recovery test:** both run `kill_recorded` on a `sleep 600` that stays a zombie until something reaps it (the test's reaper task, or init for the recovery test, where `kill -0` also succeeds on a zombie). Either `kill_recorded`'s 2 s drain ended before the reaper ran, or the test looked before init had reaped. Which one is not established. The new `identify` code is not involved (same rate on the base commit).

### Linux-only branches no test reaches
Read from the code and the tests (`cfg(target_os = "linux")` blocks and the Linux halves of shared functions).

`peer.rs` (module `platform`, Linux):
- `peer_pidfd`: `getsockopt` failing (kernel older than 6.5), a negative descriptor, a wrong returned length.
- `verify_peer`: `SO_PEERCRED` without a pid (or `<= 0`); `read_link` or `metadata` of `/proc/<pid>/exe` failing (peer gone); `verify_pidfd_alive` failing (the peer is dead by then).
- `check_root_owned_tree`, which only a release identity (`dev_ino: None`) reaches: reached only up to its first failure (the test's own binary is not owned by root). Not reached: the non-absolute path error, the `stat` failure, the "peer inode does not match packaged executable" error, the "writable by group or others" error on a root-owned ancestor, the canonicalisation and its second ancestor walk (including "canonical executable inode changed" and the symlink refusal). So **the success path of a release identity has never run**, and there is no root-owned test binary to run it on short of a packaged install.
- The shared `verify_app_peer`: the `OtherUser` branch (a peer with another uid) and a failing `peer_cred`.

`SO_PEERPIDFD` is exercised on this kernel (7.3.3): `test_binary_verifying_itself_succeeds` can only succeed if `peer_pidfd` returned a descriptor and `verify_pidfd_alive` accepted it, and the wrong-OS and release-identity assertions in `linux_release_identity_refuses_user_writable_path_and_wrong_os_identity` are `NotApp`, not `Inspect`, which they would be if `SO_PEERPIDFD` had failed first. No `strace` on this machine, so the syscall itself was not traced. It was not tried on a kernel older than 6.5, where the contract says App authority is refused.

`supervise.rs` (Linux):
- `platform_identify` with a real `PF_EXITING` or `PF_KTHREAD` task, or with a pid replaced between the two reads: only the pure `judge_exe_read` table covers these. Whether the 200-round fast-exit test ever lands in the "exiting, no exe yet" window on this machine was not measured (it would need a counter in the source).
- `read_linux_stat`: an error other than "not found" (for example permission denied), and a malformed stat from the kernel.
- `platform_zombie_timings` reaching its "spawned child vanished" error.

`lifecycle.rs` (Linux):
- `default_platform_paths`: `XDG_DATA_HOME` unset (falls back to `~/.local/share`); `XDG_RUNTIME_DIR` unset with `/run/user/<uid>` usable, and with it unusable (falls back to the data folder); a relative `XDG_*` value rejected by `env_abs_dir`. The test sets all three to absolute temporary folders.
- `resolve_linux_config_home` falling back to `~/.config`.
- `install_systemd_unit` over an existing unit and link (the replace path of `remove_if_exists` on the link), and a symlink at the unit path (`write_atomic_no_symlink`'s refusal).
- Release builds (`not(debug_assertions)`) ignoring `ATRIUM_HOME`: the test has that branch but `cargo test` builds debug.

## Daemon

### L8: idle memory (contract §14, "Daemon idle RSS < 50 MB")
Command, three times in a row on the same build (2026-11-14):

```text
cargo build --profile perf -p atrium-daemon --bin atriumd --example perf
cargo run --profile perf -p atrium-daemon --example perf   # --tokens 3000000 --idle-secs 60 are the defaults
```

The earlier L8 window (`perf.rs` before fix round 1) held the daemon's first daily `VACUUM INTO` snapshot (`upkeep::snapshot_daily` fires 60 s after daemon start, ~42 s into the wait), so the 10 samples were taken ~15 s after the snapshot rather than >= 60 s after the last activity. `perf.rs` now waits for the first daily snapshot to finish before starting the 60 s idle window, and prints the snapshot's `modified` timestamp and the idle window's start timestamp as evidence.

Load average before each run: not recorded (`cat /proc/loadavg` and `uptime` were denied by the worker command policy).

| | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| RSS at start | 9.0 MiB | 9.0 MiB | 9.1 MiB |
| 3M-token load: stored in | 15.1 s (198,771 events/s) | 15.4 s (195,439 events/s) | 15.1 s (198,627 events/s) |
| Peak RSS during the load (sampled every 250 ms) | 15.2 MiB | 15.5 MiB | 15.4 MiB |
| Process peak (`VmHWM`) right after the load | 15.2 MiB | 15.5 MiB | 15.4 MiB |
| RSS right after the sessions closed; process peak | 15.2 MiB; 15.2 MiB | 15.5 MiB; 15.5 MiB | 15.4 MiB; 15.4 MiB |
| RSS after first daily snapshot (`VACUUM INTO`); process peak | 18.4 MiB; 18.4 MiB | 18.6 MiB; 18.6 MiB | 18.6 MiB; 18.6 MiB |
| **Idle RSS, median of 10 samples, 60 s after the last activity (snapshot included)** | **18.4 MiB** | **18.6 MiB** | **18.6 MiB** |
| Idle RSS, max of the 10 | 18.4 MiB | 18.6 MiB | 18.6 MiB |
| Store file / snapshot file | 838 MiB / 811 MiB | 839 MiB / 811 MiB | 838 MiB / 811 MiB |
| 100k replay (3 runs) | 178.0, 166.2, 161.6 ms | 189.0, 164.1, 157.6 ms | 174.5, 191.1, 188.0 ms |
| Reconnect, cursor 1,000 behind, 20 runs: min / median / p95 / max | 2.39 / 2.77 / 3.23 / 3.41 ms | 1.95 / 2.54 / 3.74 / 3.81 ms | 2.17 / 2.83 / 3.81 / 3.82 ms |

**Gate: median < 50 MB. Met in all three runs (18.4 to 18.6 MiB, which is 19.3 to 19.5 MB).**

Raw output lines:

Run 1:
```text
daemon pid 1599322, RSS at start 9.0 MiB

100k-token thread: stored in 0.76 s
full replay, run 1: 100005 events in 178.0 ms (561785 events/s)
full replay, run 2: 100005 events in 166.2 ms (601838 events/s)
full replay, run 3: 100005 events in 161.6 ms (618937 events/s)

3000000-token thread: stored in 15.1 s (198771 events/s), peak RSS during the load 15.2 MiB (sampled every 250 ms), RSS right after 15.2 MiB, the process's peak so far 15.2 MiB

reconnect, cursor 1,000 behind, 20 runs: min 2.39 ms, median 2.77 ms, p95 3.23 ms, max 3.41 ms

sessions closed, clients gone: RSS 15.2 MiB (the process's peak so far 15.2 MiB)
snapshot finished: atrium-20261114-160525-980.db (811 MiB, modified 1794672326632 ms); RSS 18.4 MiB (peak 18.4 MiB)
idle window starts at 1794672328513 ms (62.5 s after daemon start), waiting 60 s…
idle RSS, 10 samples from 60 s after the last activity (snapshot included): median 18.4 MiB, max 18.4 MiB [18.4, 18.4, 18.4, 18.4, 18.4, 18.4, 18.4, 18.4, 18.4, 18.4]
store file: 838 MiB
```

Run 2:
```text
daemon pid 1601922, RSS at start 9.0 MiB

100k-token thread: stored in 0.76 s
full replay, run 1: 100005 events in 189.0 ms (529192 events/s)
full replay, run 2: 100005 events in 164.1 ms (609368 events/s)
full replay, run 3: 100005 events in 157.6 ms (634662 events/s)

3000000-token thread: stored in 15.4 s (195439 events/s), peak RSS during the load 15.5 MiB (sampled every 250 ms), RSS right after 15.5 MiB, the process's peak so far 15.5 MiB

reconnect, cursor 1,000 behind, 20 runs: min 1.95 ms, median 2.54 ms, p95 3.74 ms, max 3.81 ms

sessions closed, clients gone: RSS 15.5 MiB (the process's peak so far 15.5 MiB)
snapshot finished: atrium-20261114-160743-701.db (811 MiB, modified 1794672464979 ms); RSS 18.6 MiB (peak 18.6 MiB)
idle window starts at 1794672466769 ms (63.1 s after daemon start), waiting 60 s…
idle RSS, 10 samples from 60 s after the last activity (snapshot included): median 18.6 MiB, max 18.6 MiB [18.6, 18.6, 18.6, 18.6, 18.6, 18.6, 18.6, 18.6, 18.6, 18.6]
store file: 839 MiB
```

Run 3:
```text
daemon pid 1616017, RSS at start 9.1 MiB

100k-token thread: stored in 0.76 s
full replay, run 1: 100005 events in 174.5 ms (573201 events/s)
full replay, run 2: 100005 events in 191.1 ms (523328 events/s)
full replay, run 3: 100005 events in 188.0 ms (532063 events/s)

3000000-token thread: stored in 15.1 s (198627 events/s), peak RSS during the load 15.4 MiB (sampled every 250 ms), RSS right after 15.4 MiB, the process's peak so far 15.4 MiB

reconnect, cursor 1,000 behind, 20 runs: min 2.17 ms, median 2.83 ms, p95 3.81 ms, max 3.82 ms

sessions closed, clients gone: RSS 15.4 MiB (the process's peak so far 15.4 MiB)
snapshot finished: atrium-20261114-161001-285.db (811 MiB, modified 1794672602715 ms); RSS 18.6 MiB (peak 18.6 MiB)
idle window starts at 1794672604651 ms (63.4 s after daemon start), waiting 60 s…
idle RSS, 10 samples from 60 s after the last activity (snapshot included): median 18.6 MiB, max 18.6 MiB [18.6, 18.6, 18.6, 18.6, 18.6, 18.6, 18.6, 18.6, 18.6, 18.6]
store file: 838 MiB
```

## App (M1-14b)

(Empty on purpose: the app's checks are M1-14b.)
