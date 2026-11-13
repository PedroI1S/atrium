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
Command, three times in a row on the same build:

```text
cargo build --profile perf -p atrium-daemon --bin atriumd --example perf
target/perf/examples/perf          # --tokens 3000000 --idle-secs 60 are the defaults
```

(`target/l8.sh` runs it three times and keeps each run's output.) `perf` starts `atriumd` in a temporary home, loads a 100,000-token synthetic session, replays it three times, loads a 3,000,000-token one, reconnects with a cursor 1,000 events behind 20 times, closes both sessions and drops the clients, waits 60 s, then takes 10 RSS samples 1 s apart. `perf.rs` already did this; the one change is that it also prints the RSS and the process's peak (`VmHWM`) right after the load.

| | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| RSS at start | 9.1 MiB | 9.1 MiB | 9.1 MiB |
| 3M-token load: stored in | 15.1 s (198,555 events/s) | 15.1 s (198,553 events/s) | 15.1 s (198,598 events/s) |
| Peak RSS during the load (sampled every 250 ms) | 15.3 MiB | 15.3 MiB | 15.4 MiB |
| Process peak (`VmHWM`) right after the load | 15.3 MiB | 15.3 MiB | 15.4 MiB |
| RSS right after the sessions closed; process peak | 15.3 MiB; 15.3 MiB | 15.3 MiB; 15.3 MiB | 15.5 MiB; 15.5 MiB |
| **Idle RSS, median of 10 samples, 60 s after the last activity** | **18.5 MiB** | **18.4 MiB** | **18.5 MiB** |
| Idle RSS, max of the 10 | 18.5 MiB | 18.4 MiB | 18.5 MiB |
| Store file | 839 MiB | 839 MiB | 839 MiB |
| 100k replay (3 runs) | 200.5, 206.6, 188.1 ms | 178.2, 161.0, 157.6 ms | 175.3, 158.7, 164.2 ms |
| Reconnect, 1,000 events behind, 20 runs: min / median / p95 / max | 2.56 / 3.53 / 6.45 / 7.23 ms | 2.13 / 2.61 / 3.33 / 3.62 ms | 2.15 / 3.31 / 6.74 / 7.70 ms |

**Gate: median < 50 MB. Met in all three runs (18.4 to 18.5 MiB, which is 19.3 to 19.4 MB).** M1-05's single run, before the review fixes, gave 19.4 MiB, so there is no regression.

Observation: the idle RSS (18.5 MiB) is about 3 MiB above the process peak recorded when the sessions closed (15.3 MiB), and the 10 idle samples are flat. Something grows the resident set by about 3 MiB in the first 60 s after the work ends (not found; nothing in this task looks into it). It is far from the gate.

## App (M1-14b)

(Empty on purpose: the app's checks are M1-14b.)
