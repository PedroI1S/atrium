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

Branch `agent/opus/M1-14b-linux-app`, 2026-11-15, Claude Opus 5.5. Same machine as above: Intel Core Ultra 7 258V (8 cores), Intel Arc 140V (`xe`), 30 GB; CachyOS, kernel 7.3.3-1-cachyos; GNOME Shell 51.1 on Wayland; webkit2gtk-4.1 2.54.2, GTK 3.24.52, Mesa 26.2.6; one monitor, HDMI-1 2560×1440 at 143.87 Hz. **On battery** (47 %, no charger), `balanced` power profile. cargo 1.99.1, node 26.13.0, pnpm 12.17.0. Every build and run was on the host through `distrobox-host-exec`, every GUI run under `flock /tmp/atrium-gui-measure.lock`, with no build running at the same time. Nothing else heavy ran: each run prints the load average and the busiest processes at its start and end (load 1.6–2.3 for the main bench, 3–4.4 for the interleaved A/B batches, which launch the app back to back; once `snapperd` at 10 % CPU). No other worker ran on the machine.

Builds: the workspace's `perf` profile (`release` optimizations with debug assertions), as M1-09b chose: a `--release` build ignores `ATRIUM_HOME` and a release daemon refuses the app until packaging (see `startup_bench.py`). The harness scripts' raw outputs are in `.orchestration/m1-14b/` in the worktree (git-ignored). A/B runs measured saved copies of each build (`startup_bench.py --app`), with `atriumd` beside them, launched in turn (interleaved) so load drift falls on all of them alike.

### Harness fixes (L2), found while making the numbers real

1. **The idle number was USS, not D-040's private PSS.** `startup_bench.py` summed `Private_Clean + Private_Dirty` from `smaps_rollup`: that leaves out shared anonymous pages and counts private pages of shared libraries. D-040 budgets PSS without shared-library pages. `app/scripts/linuxmem.py` (tested, `test_linuxmem.py`) now reads `/proc/<pid>/smaps` per mapping: a `*.so` mapping's clean pages are left out, its dirty pages (relocations, written `.data`: the process's own copy-on-write memory) count; every other mapping counts at its PSS. So `private PSS = PSS − clean shared-library pages`. The stricter reading of M0's §2.5 table (every page of a library mapping left out) is `private PSS − library dirty pages`, also printed. On this machine the old number (USS, 207–208 MiB) is 56 MiB above D-040's (151–152 MiB).
2. **Every earlier Linux run of `startup_bench.py` measured an app that never connected.** The script made its `logs` folder inside `ATRIUM_HOME` with mode 0755; that is the daemon's own log folder, and `atriumd` then exits at once, **without writing anything** (no log, no message). The app retried the connection for the whole run. M1-09a's and M1-09b's Linux idle numbers (219–223, 212–216 MiB) come from that state. The bench's logs now live beside `ATRIUM_HOME`, and each idle launch checks that a daemon ran (all 9 did). A daemon that exits silently on a folder mode is worth a look by the daemon's owner (not in this task's paths).
3. The gate is 150 **MB**; the numbers are MiB. The script now prints both and compares in MB (150 MB = 143.05 MiB). M1-09a's macOS run compared MiB with 150.
4. Also new in the script: GPU memory from DRM fdinfo (deduplicated by DRM client, memory regions only, as M0's `linuxproc.py`), full PSS, RSS and USS beside private PSS, private PSS per process (UI, web, network, helpers), the longest run of samples over the gate, the cost of one pop-out (`--only popout`), a per-mapping breakdown (`--only breakdown`), `--app` (measure a saved binary) and `--env` (an experiment). `stream_bench.py` reads the same private PSS and adds the GPU peak. The app's processes are found as the app's process tree minus the daemon, not by name (a name match also caught other apps' WebKit processes).

Commands: `python3 app/scripts/startup_bench.py` (TTI, idle memory, pop-out), `python3 app/scripts/startup_bench.py --only breakdown`, `python3 app/scripts/webkit_floor.py`, each under `flock /tmp/atrium-gui-measure.lock`.

### Idle memory, one window (contract §14; D-040, D-085)

9 launches, per launch the median of 1 Hz samples from 5 s to 65 s after TTI, daemon connected in all 9 (`startup_bench.py`; the app as of `abb37ad`, unchanged since; the bench as of `7bea6da`):

| MiB (9 launches) | min | median | max |
|---|---:|---:|---:|
| **private PSS (D-040, budgeted)** | **151.4** | **151.7** | **152.5** |
| = in MB | 158.7 | 159.1 | 160.0 |
| per process: UI (`atrium`) | 59.6 | 60.5 | 60.8 |
| per process: web (`WebKitWebProcess`) | 69.0 | 69.4 | 69.6 |
| per process: network (`WebKitNetworkProcess`) | 22.2 | 22.2 | 22.3 |
| per process: helpers (`glycin-svg` in `bwrap`: GTK's sandboxed SVG loader) | 4.3 | 4.4 | 4.4 |
| by kind: anonymous (heap, `WKFastMalloc`, JS heaps, stacks) | 111.9 | 112.5 | 113.0 |
| by kind: shared libraries' dirty pages (relocations, `.data`) | 27.1 | 27.1 | 27.1 |
| by kind: other files (the `atrium` binary, caches, fonts) | 12.1 | 12.4 | 12.5 |
| left out: shared libraries' clean pages | 124.0 | 124.1 | 124.3 |
| full PSS | 275.4 | 276.0 | 276.8 |
| RSS (summed) | 453.5 | 454.2 | 455.2 |
| USS (the old harness number) | 207.2 | 208.0 | 208.5 |
| GPU, DRM fdinfo, resident = total | 114.6 | 114.6 | 120.6 |
| each launch's max sample (the first seconds) | 165.4 | 165.8 | 167.8 |

**Gate: every launch < 150 MB. Missed: 158.7–160.0 MB, by 9–10 MB (8.4–9.5 MiB).** Every sample of every launch is over 150 MB (61 of 61), so this is a steady level, not an excursion. Escalated (brief: "the idle budget is missed"); the decision is the drivers' (D-085).

**Where it goes** (one idle launch at 30 s, `--only breakdown`; each mapping's counted PSS, MiB):
- UI process 61.0: `[heap]` 18.8 (glibc malloc: Rust, GTK/GLib, WebKit's UI side), `WKFastMalloc` 12.4 (WebKit's allocator in the UI process), the `atrium` binary's own pages 7.9 (a 14.6 MB executable), other anonymous 6.1, `libwebkit2gtk` dirty 3.1, JavaScriptCore's JIT and structure heaps 3.5 (in the UI process), Mesa (`libgallium` 1.6, `libLLVM` 1.3: the UI process sets up EGL to show the web process's DMA-BUF frames).
- Web process 70.4: `WKFastMalloc` 26.6, other anonymous 17.6, `[heap]` 5.7, JS gigacage 3.5, JS structure and JIT 4.0, `libwebkit2gtk` dirty 3.1, Mesa 2.9, Mesa's shader cache index 1.3.
- Network process 22.2: `WKFastMalloc` 11.9, `libwebkit2gtk` dirty 3.1, JIT 2.0.

**What is WebKitGTK's and what is Atrium's.** `app/scripts/webkit_floor.py` opens one plain WebKitWebView (same library, same window size) and samples it the same way (3 launches each, medians):

| private PSS, MiB | UI | web | network | helpers | total | GPU |
|---|---:|---:|---:|---:|---:|---:|
| **WebKitGTK's MiniBrowser (C, GTK 3), `about:blank`** | 51.2 | 46.6 | 22.3 | 4.4 | **124.5** (130.5 MB) | 82.3 |
| PyGObject window, `about:blank` | 72.1 (Python) | 45.6–47.6 | 22.3 | 4.3 | 144.3–146.2 | 98.2 |
| PyGObject window, one paragraph of text | 72.0–74.2 | 45.9 | 22.3 | 4.3–4.4 | 144.6–146.6 | 108.4 |
| PyGObject window, Atrium's built page in browser mode (no Tauri, no daemon, empty shell) | 72.0–72.2 | 62.6–64.7 | 20.1–21.7 | 4.4 | 159.6–162.9 | 126.0 |
| **Atrium** (above) | 60.5 | 69.4 | 22.2 | 4.4 | **151.7** | 114.6 |

- **WebKitGTK's floor for one window on this machine is about 124.5 MiB (130.5 MB)**: a blank page in WebKitGTK's own browser. The network process (22 MiB) and the sandboxed SVG loader (4.4 MiB) are entirely WebKitGTK's and GTK's; Atrium's are the same size.
- **Atrium adds about 27 MiB over that floor**: about 9 MiB in the UI process (60.5 against MiniBrowser's 51.2: the Rust binary's own pages are 7.9 of it) and about 23 MiB in the web process (69.4 against 46.6). Of the web process's 23, the page alone costs about 17 (Atrium's page in a plain WebKitGTK view, no Tauri: 62.6–64.7), and running inside Tauri with the daemon's state about 6 more.
- The gate leaves Atrium 18.5 MiB above MiniBrowser's floor (143.05 − 124.5); it uses 27. **It needs about 9 MiB less** to pass on this machine.

**What changes could reach (measured where tried):**

| Change | Private PSS, MiB (launches) | Effect | Cost |
|---|---|---|---|
| none (above) | 151.4–152.5 (9) | — | — |
| `WEBKIT_DISABLE_DMABUF_RENDERER=1` | **135.9–138.1 (3)**, 142.5–144.8 MB | −14 (web −12, UI −2.5); GPU 114.6 → 5.6 | **Streaming collapses** (below: 19.8 % long frames, p99 115 ms) and TTI +76 ms (472 vs 396, interleaved). Not usable. |
| `WEBKIT_DISABLE_COMPOSITING_MODE=1` | 151.0–151.8 (3) | none; GPU 130.7 | TTI 397 / 413 (not interleaved; head 377 / 391 an hour earlier) |
| `MALLOC_ARENA_MAX=2` | 151.0–153.5 (3) | none | — |
| Size-optimised binary (`opt-level = "s"`, `lto = "fat"`, `codegen-units = 1` through `CARGO_PROFILE_PERF_*`; 7.8 MB instead of 14.6) | 149.5–149.7 (3), against 151.7–153.0 for head in the same interleaved batch (3) | **−2.5** (UI process only: 57.9 vs 58.7–60.7) | TTI 394 / 410 vs 386 / 404 (20 each, interleaved): no difference beyond noise. A profile change in `Cargo.toml` (not this task's paths); slower release builds |
| Atrium's page without its web fonts (in a plain WebKitGTK view) | web process 61.1–63.0 vs 66.0–66.1 (2 each) | −3 to −5 | not a proposal: the design's fonts |

Not tried, with what the numbers say about them: the page with the CSS and none of the app (11 elements, a 97 kB bundle) costs 48.3–49.9 MiB in the web process of a plain view, against 66.0 with the app and 45.6–47.6 for a blank page, so the app's JavaScript and DOM are about 17 MiB of it; lazy-loading what an idle window doesn't show (Settings, the inbox cards, markdown, the chat view) would cut an unknown part of that. The 6 MiB of Tauri and daemon state in the web process and the UI process's glibc heap (18.8, shared with GTK and WebKit) were not split further. The measured Atrium-side savings add up to about 2.5 MiB (the binary); none reaches 9 MiB alone. **My reading: no single change I measured gets one window under 150 MB on WebKitGTK 2.54; it needs about 9 MiB of Atrium's 27, which is a size project (the binary, a lazily loaded bundle, less state in the web process), not a fix. The drivers decide** whether the Linux gate moves (for example to "floor + N", with MiniBrowser's floor measured on the same machine) or stays and becomes a project.

### Cost of one pop-out (D-028, D-040: measured, not capped)

`startup_bench.py --only popout`, 5 launches. The main window shows an idle thread (the fake agent, 1 s of text, then idle), sampled 5–35 s; the thread moves to its own window, sampled 5–35 s after the window reports ready; the cost is the difference of the medians.

| MiB, 5 launches | median | min | max |
|---|---:|---:|---:|
| **private PSS added by one pop-out** | **66.9** | 64.2 | 69.8 |
| of which: a second `WebKitWebProcess` | 66.5 | 63.9 | 69.0 |
| of which: UI process | 4.9 | 4.5 | 5.0 |
| of which: network process | 0.0 | −0.2 | 0.0 |
| full PSS | 69.9 | 67.0 | 72.6 |
| RSS | 201.8 | 199.3 | 204.8 |
| GPU resident | 68.2 | 64.2 | 70.4 |
| the pop-out ready after it was asked for | 229 ms | 228 | 231 |

Each window gets its own web process; the network process is shared. One window showing a thread (the pop-out baseline) is 158.6–162.7 MiB, against 151.7 idle. M0 measured ~75 MB of PSS and ~88 MB of GPU per pop-out; macOS's budget is ~35 MB (D-028).

### Startup profile (L3; D-041)

`startup_bench.py`, 30 warm launches (one warm-up discarded), `ATRIUM_BENCH=startup-exit`. New marks (all wall-clock ms since the epoch, a clock read each): the script's spawn time; Rust's `setupStart`, `bridgeStarted`, `windowBuilt`, `setupEnd` (`app/src-tauri/src/lib.rs`, the measurement hook the brief allows; there was none); the webview's navigation timing (`performance.timeOrigin`, the document's `responseEnd`); `boot.js`; the bundle's first module (`app/src/startup-mark.ts`); the start of every boot stage (`setStage`, already there); mount; first paint (the second animation frame after mount, unchanged); input handled. TTI is unchanged: from `main` to the later of first paint and the handled probe key. The marks cost nothing measurable (head 396 ms vs the build without them 402 ms, interleaved, 30 each).

Load average 2.0 at start, 1.6 at the end:

| phase (ms) | median | p90 | min | max | whose |
|---|---:|---:|---:|---:|---|
| spawn → `main` | 30 | 40 | 23 | 43 | the loader and the libraries' constructors (`LD_DEBUG=statistics`: relocation 2–3 ms of it) |
| `main` → `setup` | 35 | 41 | 33 | 44 | Tauri and GTK start |
| `setup` → bridge started | 0 | 0 | 0 | 1 | Atrium |
| bridge started → main window built | 115 | 122 | 100 | 132 | wry/WebKitGTK: window, web context, web view |
| window built → navigation start | 86 | 91 | 74 | 96 | the web process starting (DMA-BUF renderer set-up included) |
| navigation → HTML loaded | 3 | 3 | 2 | 3 | |
| HTML → `boot.js` | 11 | 14 | 0 | 15 | parsing, the stylesheet |
| `boot.js` → bundle starts | 10 | 11 | 9 | 12 | fetching and compiling the 273 kB bundle |
| bundle evaluated | 2 | 2 | 1 | 2 | Atrium |
| subscribe (3 `listen` calls) | 12 | 19 | 8 | 20 | Atrium + Tauri IPC |
| `startup_info` | 4 | 5 | 3 | 7 | Atrium + Tauri IPC |
| render → mounted | 8 | 9 | 7 | 11 | Atrium (SolidJS) |
| **mounted → first paint** | **88** | **96** | 82 | 106 | style, layout and fonts (below) |
| first paint → input handled | 0 | 1 | 0 | 1 | |
| **= `main` → first JS (`boot.js`)** | **252** | 257 | 233 | 266 | |
| **= first JS → mounted** | **35** | 42 | 31 | 44 | |
| **= `main` → first paint** | **376** | 391 | 350 | 405 | |
| **= TTI (`main` → later of the two)** | **377** | **391** | 350 | 405 | |
| = spawn → TTI | 406 | 424 | 381 | 434 | |

Against M0's §2.4 (TTI 472 ms from exec, main → JS 212, JS → mounted 143): JS → mounted is now 35 ms (the shell paints from one startup call), and the pre-JS block is larger (252) because the window and the web process start in it; M0's harness also measured from exec, M1's from `main` (about 30 ms later).

The same TTI depends on the load: interleaved runs at load 3.3–4.4 gave a median of 396–402 ms and p90 425–441 ms for the same build (below); M1-09b measured 513 ms at load 7.4.

**Which phase can be shortened, and by what.**
- **Mounted → first paint (88 ms): the fonts. Fix measured: preload the five bundled fonts in `app/index.html`.** Bisected in a plain WebKitGTK view (`webkit_floor.py --seconds 0`, first contentful paint from navigation start, 15 launches each, interleaved): the shell is 108 elements and 12 SVGs; without the CSS it paints at 37 ms, with the CSS and no app DOM at 27 ms, with both at 97–110 ms. Shadows, border radii and the SVGs don't matter (±5 ms). The fonts do: one generic family for everything 55 ms; without the macOS-only names 90; the web fonts with generic fallbacks only 77. WebKitGTK meets the first layout with the web fonts still loading and resolves each family's fallback stack through fontconfig first. **With the fonts preloaded in `<head>`, first paint is at 49–52 ms instead of 105–110.** In the app (30 launches each, interleaved, load 3.3–4.4):

  | build | TTI median | TTI p90 | mounted → first paint | spawn → TTI |
  |---|---:|---:|---:|---:|
  | head | 402 | 425 | 100 | 438 |
  | **fonts preloaded in `index.html`** | **345** | **362** | **34** | **374** |
  | fonts preloaded from the bundle (`<link rel=preload>` added by the first module) | 405 | 429 | 100 | 438 |
  | no web fonts at all | 393 | 410 | 91 | 429 |
  | the daemon attach moved after the first paint | 404 | 432 | 99 | 437 |

  **−57 ms median, −63 ms p90.** Preloading from the bundle starts the loads about 20 ms later and gains nothing: it must be in the document's `<head>`. The change is five lines in `app/index.html`, which is not in this task's paths, so it is **not landed**; Vite rewrites the paths to the same hashed assets the CSS uses (checked: same five files, no duplicate):

  ```html
  <link rel="preload" href="./node_modules/@fontsource/instrument-sans/files/instrument-sans-latin-400-normal.woff2" as="font" type="font/woff2" crossorigin />
  <link rel="preload" href="./node_modules/@fontsource/instrument-sans/files/instrument-sans-latin-500-normal.woff2" as="font" type="font/woff2" crossorigin />
  <link rel="preload" href="./node_modules/@fontsource/instrument-sans/files/instrument-sans-latin-600-normal.woff2" as="font" type="font/woff2" crossorigin />
  <link rel="preload" href="./node_modules/@fontsource/geist-mono/files/geist-mono-latin-400-normal.woff2" as="font" type="font/woff2" crossorigin />
  <link rel="preload" href="./node_modules/@fontsource/geist-mono/files/geist-mono-latin-500-normal.woff2" as="font" type="font/woff2" crossorigin />
  ```
  (before `<script src="/boot.js">`). Low risk: 71 kB of fonts the page loads anyway, earlier. Unmeasured on macOS. Dropping the macOS-only names from the theme's font stacks on Linux (`-apple-system`, `BlinkMacSystemFont`, `SF Mono`, `Menlo`) would save a further few ms where a fallback is still resolved; that is a theme file, not measured in the app.
- **Window built (115 ms) and the web process start (86 ms): WebKitGTK's.** `WEBKIT_DISABLE_DMABUF_RENDERER=1` makes the web process start 50 ms sooner but the first frame 30–40 ms later, and breaks streaming; compositing off (397 / 413) and the sandbox off (`WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS`, for attribution only: 403 / 431) gain nothing (not interleaved; head was 377 / 391 an hour earlier). Nothing in Atrium's code runs in these 200 ms except the window's options.
- **`main` → setup (35 ms) and spawn → `main` (30 ms): GTK, Tauri and the libraries' constructors.** Not Atrium's to shorten.
- **Atrium's own JavaScript (35 ms from `boot.js` to mounted)**: small; the three `listen` calls (12 ms) could be one, a few ms at most.

### D-041 recommendation

With the font preload, TTI from `main` is **345 ms median, 362 ms p90 at load 3.3–4.4** on a laptop on battery in the `balanced` profile (spawn → TTI 374 / 390); without it, 377 / 391 at load 2 and 396–402 / 425–441 at load 3.3–4.4. **400 ms is reachable on Linux.** I recommend making it a gate on Linux with the macOS statistic (30 warm launches, median < 400 ms and p90 < 450 ms, measured from `main` by `startup_bench.py`) **once the font preload is merged**, with two conditions written into §14: the run states its load average and starts below 2.5 with no build running, and the spawn → TTI figure is reported beside it. Without the preload the median sits 0–25 ms under 400 ms and a busy machine pushes it over, which would make the gate flaky. The remaining 345 ms is mostly WebKitGTK's own (window 115, web process 86, start-up 65), so later UI work should not be allowed to eat the 55 ms of margin unnoticed: the profile table above is printed by every `startup_bench.py` run.

### M1-09a's and M1-09b's runs on Linux

All on the commit measured above (perf build for the benches, debug build for the checks), each alone under the GUI lock, 2026-11-15 04:28–04:32. Load average 4–7 during these (the debug build had finished a minute earlier; the 1-minute average lags).

| Run | Result |
|---|---|
| `python3 app/scripts/live_check.py` (M1-09a) | **12/12 passed**: draft through a pop-out and back, a forced return, the daemon killed and restarted by the app (pid 1928541 → 1928893), the thread listed as interrupted, a crash and relaunch with the draft and thread back, the running daemon reused |
| `python3 app/scripts/screenshots.py <dir>` (M1-09a, M1-09b) | **28 pictures, all three runs report `pass`** (`real`, `chat`, `refused`); in `.orchestration/m1-14b/screens/` in the worktree (git-ignored) |
| `python3 app/scripts/chat_check.py` (M1-09b) | **12/12 passed** (daemon killed mid-stream, pid 1930859 → 1931157) |
| `python3 app/scripts/stream_bench.py --runs 3` (§14, M1-09b) | below |
| Tray | **Not checked: this desktop (GNOME Shell 51.1) has no tray support.** Not worked around. |

**L1.** Settings → Appearance on Linux says "Atrium uses opaque windows on Linux." under the essentials, and the Advanced card's Blur strength row says "Atrium uses opaque windows on Linux. This only affects overlays." Nothing mentions a compositor (`grep -ri compositor app/src` finds only a comment in the stream bench). M1-09a shipped it right; nothing changed. Pictures: `settings-advanced-light.png`, `settings-advanced-dark.png` in the folder above. Seen and left (new UI is out of scope): the Window opacity and Blur rows above the note keep their macOS hints ("Lets the desktop show through") while disabled.

**§14 streaming run** (10 threads × 1,000 tok/s × 30 s, rAF timestamps, window visible in every run, display at 143.87 Hz; WebKitGTK paces a busy page at 60 Hz, M1-09b's note):

| run | tok/s | fps | frame p50 / p95 / p99 / max (ms) | over 16.7 ms as read | over 17.7 ms | long (> 1.5 × interval) | web process peak | app + webviews peak | GPU resident peak |
|---|---:|---:|---|---:|---:|---:|---:|---:|---:|
| 1 | 9,994 | 59.8 | 17 / 19 / **21** / 54 | 57.6 % | 16.9 % | **6 (0.3 %)** | 123.1 | 218.9 | 423.9 |
| 2 | 9,989 | 59.6 | 17 / 18 / **22** / 60 | 59.9 % | 12.9 % | **8 (0.4 %)** | 122.9 | 216.0 | 422.0 |
| 3 | 9,990 | 59.9 | 17 / 18 / **21** / 34 | 58.0 % | 15.8 % | **3 (0.2 %)** | 126.8 | 217.6 | 415.9 |

Memory in MiB of D-040's private PSS. **p99 21–22 ms (gate ≤ 33): within. Frames that skipped an update (over 1.5 × the interval, M1-09b's proposed reading) 0.2–0.4 %: within 1 %.** The literal "≤ 1 % over 16.7 ms" can't be read with 1 ms rAF timestamps (an on-time frame reads 17): the open decision M1-09b left (D-085 keeps it open). The web process peak is 123–127 MiB of private PSS (M1-09b's 179–183 were USS). **GPU memory grows from 115 MiB idle to 416–424 MiB while streaming**: D-040 says it is watched for growth; it has no budget. With `WEBKIT_DISABLE_DMABUF_RENDERER=1` (one run): 61.6 fps but p99 115 ms, 19.8 % long frames, 1,557 updates dropped, web process 210 MiB: the shared-memory path can't stream at this rate.
