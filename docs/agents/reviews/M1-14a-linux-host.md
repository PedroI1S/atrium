# Review: M1-14a-linux-host
Reviewer: Gemini 4 Argon (read-only) · 2026-11-13
## Verdict
**approve with changes**: The four findings from `M1-10-host-linux-exit` are addressed and the fail-closed rule holds for existing callers, but `judge_exe_read` does not verify `after.pgid == first.pgid` (allowing `first.pgid` to be paired with a post-change `executable`) and several decision-table and measurement gaps should be tightened.

## The four findings
- **Finding 1 (`ENOENT` on live task vs `PF_EXITING` / `PF_KTHREAD`)**: **closed** — `PF_EXITING = 0x0000_0004` (`crates/atrium-host/src/supervise.rs:507`) and `PF_KTHREAD = 0x0020_0000` (`supervise.rs:510`) match Linux `include/linux/sched.h`; `parse_linux_proc_stat` (`supervise.rs:535-576`) anchors on `stat_text.rfind(')')` so spaces, parentheses, or newlines in `comm` (field 2) cannot shift field indices (`next()` = field 3 `state`, `next()` = field 4 `ppid`, `next()` = field 5 `pgrp`, `nth(3)` skips fields 6..=8 to read field 9 `flags`, `nth(12)` skips fields 10..=21 to read field 22 `starttime`); `judge_exe_read` (`supervise.rs:621-624`) returns `ExeVerdict::NoProcess` on `Err(io::ErrorKind::NotFound)` with a live state only when `after.flags & (PF_EXITING | PF_KTHREAD) != 0`, and `ExeVerdict::Unverifiable` (`Err`) for every other live-task error.
- **Finding 2 (re-read `/proc/<pid>/stat` after `read_link` and compare `start_time`)**: **closed with a gap** — `platform_identify` (`supervise.rs:643-670`) re-reads `after = read_linux_stat(pid)?` after `read_link` on both `Ok` and `Err` and `judge_exe_read` (`supervise.rs:615-617`) rejects `after.start_time != first.start_time`, but it does not check `after.pgid != first.pgid` while returning `pgid: first.pgid` (`supervise.rs:661`), and `execve` (which preserves `start_time`) or same-tick (10 ms `USER_HZ`) PID reuse can still change `executable` or `pgid` between `first` and `after` (Findings 1 and 2 below).
- **Finding 3 (extract pure `judge_exe_read` and 16-case decision table)**: **closed with a gap** — `judge_exe_read` (`supervise.rs:607-626`) is pure and compiled under `#[cfg(any(target_os = "linux", test))]`, and `exe_read_decision_table` (`supervise.rs:721-799`) runs on every target OS, but several concrete mutations of `judge_exe_read` and `LinuxStat::is_dead` survive the 16-case table (Finding 3 and Test gaps below).
- **Finding 4 (accurate comments on `identify_spawned` and `judge_exe_read`)**: **closed** — `identify_spawned` (`supervise.rs:354-359`) and `judge_exe_read` (`supervise.rs:590-605`) accurately document both the zombie and `PF_EXITING` (`mm`-dropped) cases and note that `resolve_spawn_executable` is a best-effort guess (only minor nit: `supervise.rs:716` labels `0x0040_0000` as `PF_FORKNOEXEC`, which in Linux `include/linux/sched.h` is `0x0000_0040`; `0x0040_0000` is `PF_RANDOMIZE`).

## Findings
1. [minor] **`judge_exe_read` checks `start_time` across the `read_link` window, but not `pgid`, while `platform_identify` constructs `ProcessIdentity` with `first.pgid`**
   - Where: `crates/atrium-host/src/supervise.rs:615-619` (`judge_exe_read`) and `crates/atrium-host/src/supervise.rs:658-664` (`platform_identify`):
     ```rust
     if after.start_time != first.start_time {
         return ExeVerdict::Unverifiable;
     }
     match exe {
         Ok(()) => ExeVerdict::Identity,
     ```
     ```rust
     match (verdict, exe) {
         (ExeVerdict::Identity, Ok(executable)) => Ok(Some(ProcessIdentity {
             pid,
             pgid: first.pgid,
             start_time: first.start_time,
             executable,
         })),
     ```
   - Case:
     1. Read 1 (`first = read_linux_stat(4242)?`): `/proc/4242/stat` has `state = 'R'`, `pgid = 4242`, `flags = 0x0040_0100`, `start_time = 500`.
     2. Between Read 1 and Read 2, task `4242` calls `setpgid(0, 9000)` (or exits, is reaped, and PID `4242` is reused within the same 10 ms `USER_HZ` tick `start_time = 500` by a process in `pgid = 9000` running `/usr/bin/other`).
     3. Read 2 (`std::fs::read_link("/proc/4242/exe")`): returns `Ok(PathBuf::from("/usr/bin/other"))`.
     4. Read 3 (`after = read_linux_stat(4242)?`): `/proc/4242/stat` has `state = 'R'`, `pgid = 9000`, `flags = 0x0040_0100`, `start_time = 500`.
   - Result: `judge_exe_read` returns `ExeVerdict::Identity` because `after.start_time == first.start_time` (`500 == 500`), and `platform_identify` returns `Ok(Some(ProcessIdentity { pid: 4242, pgid: 4242, start_time: 500, executable: "/usr/bin/other" }))`, mixing `first.pgid` (`4242`) from before `read_link` with `executable` (`"/usr/bin/other"`) from after `read_link`. In `spawn_group` (`supervise.rs:73`: `Ok(identity) if identity.pgid == pid`), a child that changed its process group to `9000` during startup is accepted as having `pgid == 4242`; in `kill_recorded` (`supervise.rs:189-193`), if `recorded` had `{ pid: 4242, pgid: 4242, start_time: 500, executable: "/usr/bin/other" }`, `live == *recorded` succeeds even though `after.pgid` was `9000`. It should return `ExeVerdict::Unverifiable` (`Err` -> `Cleanup::Unverified` in `kill_recorded`, `Err` in `spawn_group`) whenever `pgid` changed across the read window (contract §10: unverified identity must not be trusted).
   - Fix: In `judge_exe_read` (`crates/atrium-host/src/supervise.rs:615`), reject a `pgid` change as well:
     ```rust
     if after.start_time != first.start_time || after.pgid != first.pgid {
         return ExeVerdict::Unverifiable;
     }
     ```

2. [minor] **A multithreaded process whose leader thread (`tid == pid`) has exited (`'Z'`) or a process mid-`execve` in `de_thread()` is treated by `identify` as `Ok(None)` ("gone") while the process and its process group are still running**
   - Where: `crates/atrium-host/src/supervise.rs:527-531` (`LinuxStat::is_dead`), `crates/atrium-host/src/supervise.rs:642-657` (`platform_identify`), and `crates/atrium-host/src/supervise.rs:180-188` (`kill_recorded`):
     ```rust
     impl LinuxStat {
         fn is_dead(&self) -> bool {
             matches!(self.state, 'Z' | 'X' | 'x')
         }
     }
     ```
     ```rust
     let Some(first) = read_linux_stat(pid)? else {
         return Ok(None);
     };
     if first.is_dead() {
         return Ok(None);
     }
     ```
     ```rust
     match identify(recorded.pid) {
         Err(_) => Cleanup::Unverified,
         Ok(None) => {
             if group_is_empty(recorded.pgid) {
                 Cleanup::Verified
             } else {
                 Cleanup::Unverified
             }
         }
     ```
   - Case:
     - On Linux, `/proc/<pid>/stat` reports the `state` (field 3) and `flags` (field 9) of the thread-group leader task (`tid == pid`), not of the thread group as a whole (`fs/proc/array.c: do_task_stat`).
     - **Subcase A (leader thread exited via `pthread_exit` / `SYS_exit` while worker threads in `tgid == pid` remain alive)**: Recorded provider process `pid = 4242, pgid = 4242, start_time = 500, executable = "/usr/bin/provider"` has a live worker thread `tid = 4243` (`tgid = 4242, pgid = 4242`), and its main thread `tid = 4242` calls `pthread_exit`. `/proc/4242/stat` now has `state = 'Z'`, `pgid = 4242`, `flags = 0x0040_0104` (`PF_EXITING`), `start_time = 500` (while field 20 `num_threads` is `2` and `/proc/4242/exe` still resolves to `"/usr/bin/provider"` via `find_lock_task_mm`).
     - **Subcase B (non-leader thread mid-`execve` inside `fs/exec.c: de_thread`)**: Thread `tid = 4243` calls `execve`. While `de_thread()` tears down the old leader `tid = 4242` before `exchange_tids()`, `/proc/4242/stat` has `state = 'Z'` (or `flags & PF_EXITING != 0` with `read_link("/proc/4242/exe") == Err(ENOENT)`), `pgid = 4242`, `start_time = 500`.
     - Order of reads: Read 1 (`first = read_linux_stat(4242)?`) reads `state = 'Z'`, `pgid = 4242`, `start_time = 500` (or in Subcase B before `'Z'`, Read 1 reads `state = 'R'`, Read 2 `read_link` returns `Err(ENOENT)`, Read 3 `after` reads `state = 'Z'` or `PF_EXITING`, `start_time = 500`).
   - Result: `identify(4242)` returns `Ok(None)` ("no process") even though process `4242` is still alive and running. In `kill_recorded(&recorded)`, `Ok(None)` enters lines 182–188: it skips `signal_group(4242, libc::SIGKILL)`, calls `group_is_empty(4242)` which returns `false` (because thread `4243` is alive in `pgid = 4242`), and returns `Cleanup::Unverified` **without killing the live recorded provider process**. (It fails safe as `Cleanup::Unverified` rather than `Cleanup::Verified`, so it does not violate the fail-closed rule, but it fails to kill a live recorded process that could still be identified via `/proc/<pid>/exe` when `num_threads > 1`.)
   - Fix: Parse field 20 (`num_threads`) in `LinuxStat` and treat a task with `num_threads > 1` as not dead/exiting in `LinuxStat::is_dead` and `judge_exe_read`, or proceed to `read_link("/proc/<pid>/exe")` when `num_threads > 1`.

3. [minor] **Four mutations of `judge_exe_read` and `LinuxStat::is_dead` are not caught by `exe_read_decision_table`**
   - Where: `crates/atrium-host/src/supervise.rs:528-530` (`LinuxStat::is_dead`), `crates/atrium-host/src/supervise.rs:607-626` (`judge_exe_read`), and `crates/atrium-host/src/supervise.rs:721-799` (`exe_read_decision_table`):
     ```rust
     fn is_dead(&self) -> bool {
         matches!(self.state, 'Z' | 'X' | 'x')
     }
     ```
     ```rust
     let Some(after) = after else {
         return ExeVerdict::NoProcess;
     };
     if after.start_time != first.start_time {
         return ExeVerdict::Unverifiable;
     }
     match exe {
         Ok(()) => ExeVerdict::Identity,
         Err(_) if after.is_dead() => ExeVerdict::NoProcess,
         Err(io::ErrorKind::NotFound) if after.flags & (PF_EXITING | PF_KTHREAD) != 0 => {
             ExeVerdict::NoProcess
         }
         Err(_) => ExeVerdict::Unverifiable,
     }
     ```
   - Case: The 16-row table (`supervise.rs:725-791`) leaves the following mutations of `judge_exe_read` / `is_dead` undetected (`cargo test -p atrium-host` still passes with each):
     1. **Move `if after.flags & PF_KTHREAD != 0 { return ExeVerdict::NoProcess; }` (on `ENOENT`) or `Err(io::ErrorKind::PermissionDenied) if after.is_dead() => ExeVerdict::NoProcess` above `if after.start_time != first.start_time`**: Every reused-PID (`start_time: 501`) row in the table uses `(Ok(()), 'S', LIVE, 501)` (line 735), `(ENOENT, 'S', LIVE | PF_EXITING, 501)` (line 764), or `(ENOENT, 'Z', LIVE, 501)` (line 769). There is no row for `(ENOENT, 'S', PF_KTHREAD, 501)` (PID reused by a kernel thread between `first` and `after`) or `(EACCES, 'Z', LIVE, 501)` (PID reused by another user's process that became a zombie before `after`).
     2. **Mutate `after.start_time != first.start_time` (`supervise.rs:615`) to `after.start_time > first.start_time`**: Every reused-PID row in the table uses `501 > 500`; none tests `after.start_time < first.start_time` (e.g. `499`).
     3. **Mutate `matches!(self.state, 'Z' | 'X' | 'x')` (`supervise.rs:529`) to `matches!(self.state, 'Z' | 'X')`**: No row in the table tests state `'x'` (`TASK_DEAD` in older/trace Linux states).
     4. **Mutate `let Some(after) = after` (`supervise.rs:612`) to return `NoProcess` only for `Ok(()) | Err(NotFound)` and `Unverifiable` for `Err(PermissionDenied)`**: `after = None` is tested only with `Ok(())` (line 740) and `ENOENT` (line 762), not `EACCES`.
   - Result: A regression that checks `PF_KTHREAD` or `(EACCES, 'Z')` before `after.start_time != first.start_time` would turn a PID-reuse race (`Unverifiable` / `Err`) into `NoProcess` (`Ok(None)`), and `exe_read_decision_table` would not fail.
   - Fix: Add rows to `exe_read_decision_table` for `(ENOENT, Some(stat('S', PF_KTHREAD, 501)), Unverifiable)`, `(EACCES, Some(stat('Z', LIVE, 501)), Unverifiable)`, `(Ok(()), Some(stat('S', LIVE, 499)), Unverifiable)`, `(ENOENT, Some(same('x', LIVE)), NoProcess)`, and `(EACCES, None, NoProcess)`.

4. [minor] **`perf.rs`'s 60 s idle window includes an 839 MiB background `VACUUM INTO` snapshot triggered ~42 s into the wait, so the RSS samples are taken ~15 s after the snapshot rather than ≥ 60 s after the last daemon activity**
   - Where: `crates/atrium-daemon/examples/perf.rs:350-375` (`main`) and `crates/atrium-daemon/src/upkeep.rs:19, 156-188` (`snapshot_daily`):
     ```rust
     // crates/atrium-daemon/examples/perf.rs:350-365
     close(&mut wire, &large.thread_id).await?;
     close(&mut wire, &small.thread_id).await?;
     drop(wire);
     let (after_close, high_water) = rss_kib(pid);
     ...
     tokio::time::sleep(Duration::from_secs(idle_secs)).await;
     let mut samples = Vec::new();
     for _ in 0..10 {
         samples.push(rss_kib(pid).0);
         tokio::time::sleep(Duration::from_secs(1)).await;
     }
     ```
     ```rust
     // crates/atrium-daemon/src/upkeep.rs:19, 159-185
     const SNAPSHOT_FIRST_CHECK: Duration = Duration::from_secs(60);
     ...
     let mut wait = every.min(SNAPSHOT_FIRST_CHECK);
     loop {
         tokio::select! {
             () = tokio::time::sleep(wait) => {}
             () = stopping(&inner) => return,
         }
         ...
         if let Err(error) = inner.store.snapshot(&dir).await {
     ```
   - Case: `perf.rs` spawns `atriumd` at `t = 0 s`, spends ~17.5 s loading the 100k-token and 3M-token threads (`15.1 s` in `docs/spikes/M1-linux-checks.md:78`), replaying 3 times, reconnecting 20 times, and closing both sessions, then sleeps `idle_secs = 60` (`t = 17.5 s .. 77.5 s`). At `t = 60.0 s` from daemon start (~42.5 s into the "idle" sleep), `upkeep::snapshot_daily` wakes from `SNAPSHOT_FIRST_CHECK` (`60 s`), sees no snapshot in `paths.snapshots_dir` and `running_turns() == 0`, and runs `inner.store.snapshot(&dir)` (`VACUUM INTO`, copying the 839 MiB SQLite database into `snapshots/`).
   - Result:
     1. The 10 RSS samples at `t = 77.5 s .. 87.5 s` are taken 60 s after the last *client* command, but only ~15 s after the daemon finished an 839 MiB `VACUUM INTO` snapshot (contract §14 specifies: *"median of 10 samples at 1 s intervals, taken ≥ 60 s after the last activity, after a 3M-token session was closed"*).
     2. This is the exact cause of the unexplained +3.2 MiB RSS jump during the idle wait reported in `docs/spikes/M1-linux-checks.md:97` (*"At session close RSS is 15.3 MiB; during the 60 s idle wait it rises by ~3.2 MiB to 18.5 MiB ... What allocates during the idle minute was not found"*): `VACUUM INTO` populates the writer connection's `SQLITE_CACHE_KIB` page cache and allocator pages at `t = 60 s`. Even with that extra cache residency, 18.5 MiB is well below the 50 MiB ceiling, so the contract §14 budget still passes.
     3. In addition, `docs/spikes/M1-linux-checks.md:17, 33-40, 74` cites log files in git-ignored `target/` (`target/loop.sh`, `target/l8/run-{1,2,3}.out`, `target/l8/run-{1,2,3}.env`) without embedding the raw `ps` / `uptime` output, the 10-sample arrays printed by `perf.rs:372-375`, or the exact shell commands used for the 60-run and 100-run base-vs-branch comparison table (`M1-linux-checks.md:35-40`).
   - Fix: In `perf.rs` (or `docs/spikes/M1-linux-checks.md`), either pre-create a fresh snapshot in `snapshots/` before the run / wait `≥ 60 s` after `SNAPSHOT_FIRST_CHECK` (`--idle-secs 80`), and paste the raw `perf` output lines (including `[samples]`) and `uptime`/`ps` output into `docs/spikes/M1-linux-checks.md`.

---

## Caller-by-caller audit of `identify` and the 200-round fast-exit test

### Callers of `identify` (`crates/atrium-host/src/supervise.rs:208`)
1. **`kill_recorded(recorded: &ProcessIdentity) -> Cleanup` (`crates/atrium-host/src/supervise.rs:171-206`)**:
   - `Err(_)` (`supervise.rs:181`): returns `Cleanup::Unverified` without signalling.
   - `Ok(None)` (`supervise.rs:182-188`, now also reached for `(ENOENT, PF_EXITING)` and `(ENOENT, PF_KTHREAD)` when `after.start_time == first.start_time`): does **not** return `Cleanup::Verified` directly; it checks `if group_is_empty(recorded.pgid) { Cleanup::Verified } else { Cleanup::Unverified }`. Because `group_is_empty` (`supervise.rs:243-248`) requires `libc::kill(-raw_pgid, 0)` to fail with `ESRCH`:
     - If `recorded.pid` is `PF_EXITING` (`!is_dead()`) and has not been reaped yet, `recorded.pid` is still in `recorded.pgid`, so `kill(-recorded.pgid, 0)` returns `0` (`group_is_empty == false`) and `kill_recorded` returns `Cleanup::Unverified`.
     - If `recorded.pid` exited, was reaped, and its PID was reused by a kernel thread (`PF_KTHREAD`, `pgid == 0`), `identify` returns `Ok(None)` (or `Err` if `start_time` changed between `first` and `after`), and `group_is_empty(recorded.pgid)` checks whether any descendant remains in `recorded.pgid`. The kernel thread is never signalled, and `Cleanup::Verified` is returned only when `recorded.pgid` has zero members (`ESRCH`).
   - `Ok(Some(live))` (`supervise.rs:189-205`): signals `-recorded.pgid` with `SIGKILL` only when `live == *recorded` (matching `pid`, `pgid`, `start_time`, and `executable`).
2. **`identify_spawned(pid, raw_program, raw_cwd, path_override)` (`crates/atrium-host/src/supervise.rs:345-370`, called only by `spawn_group` at `supervise.rs:67-87`)**:
   - `Err(err)`: propagated by `?` at `supervise.rs:351`; `spawn_group` (`supervise.rs:82-86`) kills `pid` and its group and returns `Err(err)`.
   - `Ok(None)`: falls back (`supervise.rs:360-369`) to `platform_zombie_timings(raw_pid)?` (which reads `pgid` and `start_time` from `/proc/<pid>/stat` of the unwaited child) plus `resolve_spawn_executable(...)`, and `spawn_group` (`supervise.rs:73`) verifies `identity.pgid == pid`. Because `child` in `spawn_group` has not been waited on yet, its PID cannot have been reaped or reused by the kernel.
3. **`InstanceLock::acquire(paths: &Paths)` (`crates/atrium-host/src/lifecycle.rs:217-219`)**:
   - Calls `crate::supervise::identify(std::process::id())?.ok_or_else(|| std::io::Error::other("cannot determine current daemon ProcessIdentity"))?` on the daemon's own live PID *after* acquiring the exclusive `flock` on `paths.lock`. Both `Err(e)` and `Ok(None)` abort `InstanceLock::acquire` with `LifecycleError::Io` before `std::fs::remove_file(&paths.socket)` (`lifecycle.rs:227`).
4. **`peer::platform::verify_peer` (`crates/atrium-host/src/peer.rs:526-529`, `#[cfg(target_os = "macos")]` only)**:
   - Compiled only on macOS (`AppIdentity::MacSigned` fallback when `details.executable` is `None`); maps both `Err(e)` and `Ok(None)` to `Err(PeerError::Inspect(...))`. Linux `peer::platform::verify_peer` (`peer.rs:643-715`) does not call `supervise::identify`.

### The four cases in Question 2
- **Reused PID**:
  - If `recorded.pid` was reused before `first = read_linux_stat(pid)?`: `first.start_time != recorded.start_time` (unless reused within the same 10 ms `USER_HZ` tick; see Suspected below). If the new process is live and readable, `live.start_time != recorded.start_time` gives `Cleanup::Unverified` without signalling. If the new process exits during `identify` and has `PF_EXITING` or `'Z'` at `after` (`after.start_time == first.start_time`), `identify` returns `Ok(None)` and `kill_recorded` returns `Cleanup::Verified` only if `group_is_empty(recorded.pgid)` (`ESRCH`) holds.
  - If `recorded.pid` was reused *between* `first` and `after` with a different `start_time`: `judge_exe_read` (`supervise.rs:615`) returns `ExeVerdict::Unverifiable` -> `identify` returns `Err` -> `kill_recorded` returns `Cleanup::Unverified`. (For same-`start_time` `pgid` changes, see Finding 1.)
- **Kernel thread (`PF_KTHREAD`)**:
  - `first` has `flags & PF_KTHREAD != 0`, `pgid == 0`; `read_link("/proc/<pid>/exe")` returns `Err(ENOENT)`; `after` has the same `start_time` and `PF_KTHREAD`. `judge_exe_read` returns `ExeVerdict::NoProcess` -> `identify` returns `Ok(None)`. In `kill_recorded`, `group_is_empty(recorded.pgid)` checks `-recorded.pgid`, never signalling the kthread.
- **Zombie owned by someone else**:
  - If `recorded.pid` was reused by another process (same or different UID) that is now in state `'Z'`: `first = read_linux_stat(pid)?` succeeds (since `/proc/<pid>/stat` is `0444`) and `first.is_dead()` (`supervise.rs:646`) returns `Ok(None)` immediately (or if it entered `'Z'` between `first` and `after` with the same `start_time`, `judge_exe_read` line 620 returns `ExeVerdict::NoProcess` -> `Ok(None)`). In `kill_recorded`, `Ok(None)` checks `group_is_empty(recorded.pgid)` (`libc::kill(-recorded.pgid, 0) == ESRCH`):
    - If the other user's zombie is itself a process-group leader (`pgid == recorded.pid == recorded.pgid`), `libc::kill(-recorded.pgid, 0)` returns `0` (same UID) or `EPERM` (different UID, `!= ESRCH`), so `group_is_empty` returns `false` and `kill_recorded` returns `Cleanup::Unverified` without signalling.
    - If the other user's zombie is in some other process group (`pgid != recorded.pgid`) and `recorded.pgid` has no members (`ESRCH`), `group_is_empty(recorded.pgid)` returns `true` and `kill_recorded` returns `Cleanup::Verified` (which is true: `recorded.pgid` is empty and gone).
- **Process mid-`execve`**:
  - On Linux, `execve` does **not** change field 22 (`starttime`) of `/proc/<pid>/stat`.
  - If a single-threaded process `pid` is mid-`execve` between `first` and `after`, `after.start_time == first.start_time` holds while `/proc/<pid>/exe` transitions from the pre-`execve` binary to the post-`execve` binary in `fs/exec.c: exec_mmap` / `begin_new_exec`. In `kill_recorded`, if `read_link` observes the pre-`execve` binary matching `recorded.executable` just before `exec_mmap` swaps `mm`, `kill_recorded` treats the task as `recorded` and signals `-recorded.pgid` (which is still the same task/group started by Atrium). Conversely, in `spawn_group` -> `identify_spawned`, if `read_link` observes the parent binary before `exec_mmap` swaps `tsk->mm` (see "The daemon's flaky tests" below) or a wrapper binary before `exec`, `recorded.executable` will not match the post-`execve` binary at `kill_recorded`, yielding `Cleanup::Unverified`.
  - If a multithreaded process is mid-`execve` in `de_thread()` while a non-leader thread takes over `pid`, see Finding 2.

### The 200-round fast-exit test (`crates/atrium-host/tests/supervise.rs:337-359`)
- **Can it be flaky?**
  - Not from the kernel's `do_exit()` ordering: in Linux `kernel/exit.c: do_exit`, `exit_signals(tsk)` sets `PF_EXITING` (`0x0000_0004`) *before* `exit_mm()` drops `tsk->mm` (which causes `/proc/<pid>/exe` to return `ENOENT`), and `exit_notify()` sets `EXIT_ZOMBIE` (`'Z'`) afterwards. Therefore, whenever `read_link("/proc/<pid>/exe")` returns `ENOENT` on an exiting `/bin/sh -c "exit 0"` child, the subsequent `after = read_linux_stat(pid)` guaranteed by kernel ordering sees either `PF_EXITING` (`!is_dead()`) or `'Z'`/`'X'` (`is_dead()`) or `None`.
  - Its only potential flakiness is the 1-second grace timeout in `terminate(&mut sup, Duration::from_secs(1)).await` (`crates/atrium-host/tests/supervise.rs:353, 357`: `assert!(!term.killed)`): if the test runner stalls `/bin/sh -c "exit 0"` for `> 1 s` under extreme load, `term.killed` is `true` and line 357 panics.
- **Does it prove anything the pure table doesn't?**
  - Yes, partially: it exercises live `/proc/<pid>/stat` reading, `parse_linux_proc_stat` on the running kernel's format, and — whenever a round lands in `identify(pid) == Ok(None)` (either `'Z'` at `first`, or `ENOENT` + `PF_EXITING`/`'Z'` at `after`) — the fallback path in `identify_spawned` (`supervise.rs:360-369`: `platform_zombie_timings` + `resolve_spawn_executable`) followed by `terminate` (`waitid(WNOWAIT)` + `child.wait()`). However, it does **not** deterministically prove that any of the 200 iterations landed in the microsecond `[exit_mm, EXIT_ZOMBIE)` (`PF_EXITING` while still `'R'`/`'S'`) window rather than before `exit_mm` (`Ok(Some)`) or after `EXIT_ZOMBIE` (`'Z'`).

## Suspected, not shown
- **Same-10 ms-tick (`USER_HZ = 100`) PID wraparound on Linux**: Field 22 (`starttime`) of `/proc/<pid>/stat` has 10 ms granularity (`clock_t` at `USER_HZ = 100`). If a process exits, is reaped, and the kernel wraps `pid_max` and reallocates the exact same `pid` (and `pgid` if both are group leaders) within the same 10 ms tick (`after.start_time == first.start_time`), `judge_exe_read` cannot distinguish the two tasks. Triggering this requires cycling through `pid_max` (default 32,768 or 4,194,304) within 10 ms.

## Tried and held
- **Checked whether `parse_linux_proc_stat` (`supervise.rs:535-576`) can misparse a `comm` containing `)`, spaces, or `\n`**: Held. The kernel formats `/proc/<pid>/stat` as `%d (%s) %c ...` where no field after `comm` contains `)`. Using `stat_text.rfind(')')` (`supervise.rs:540`) always locates the closing parenthesis of field 2, and `nth(3)` / `nth(12)` on `split_whitespace()` land on field 9 (`flags`) and field 22 (`starttime`).
- **Checked whether `judge_exe_read` (`supervise.rs:607-626`) can ever turn an `Err` from `read_link` into `ExeVerdict::Identity`**: Held. `ExeVerdict::Identity` is reachable only on line 619 (`Ok(()) => ExeVerdict::Identity`), after `after.is_some()` and `after.start_time == first.start_time`, and `platform_identify` (`supervise.rs:659`) requires `(ExeVerdict::Identity, Ok(executable))`.
- **Checked whether `(ENOENT, PF_EXITING)` returning `Ok(None)` from `identify` can make `kill_recorded` return a false `Cleanup::Verified` while the exiting process or its descendants are still in `recorded.pgid`**: Held. When `identify(recorded.pid)` returns `Ok(None)`, `kill_recorded` (`supervise.rs:182-187`) checks `group_is_empty(recorded.pgid)` (`libc::kill(-raw_pgid, 0) == ESRCH`). An exiting (`PF_EXITING`) or zombie (`'Z'`) leader that has not yet been reaped by `waitpid` is still present in `PIDTYPE_PGID`, so `libc::kill(-raw_pgid, 0)` returns `0` (`group_is_empty == false`) and `kill_recorded` returns `Cleanup::Unverified`.

## Test gaps
- **`exe_read_decision_table` (`crates/atrium-host/src/supervise.rs:721-799`)**: Missing rows for `(ENOENT, Some(stat('S', PF_KTHREAD, 501)), Unverifiable)`, `(EACCES, Some(stat('Z', LIVE, 501)), Unverifiable)`, `after.start_time < first.start_time`, `state == 'x'`, and `(EACCES, None, NoProcess)` (Finding 3), plus a row where `after.pgid != first.pgid` once Finding 1 is fixed.
- **Linux `peer.rs` release-identity path (`crates/atrium-host/src/peer.rs:730-828`)**: As noted in `docs/spikes/M1-linux-checks.md:54`, `check_root_owned_tree` has no unit test covering its success path or its deeper failure branches (symlinked ancestor, group/other-writable root-owned ancestor, `dev/ino` mismatch) because it hardcodes `stat("/")`..`stat(exe)` on the real filesystem rather than taking a path/stat seam or testing against `/usr/bin/true`.
- **Linux `lifecycle.rs` XDG fallback branches (`crates/atrium-host/src/lifecycle.rs:638-678`)**: No test exercises `default_platform_paths` when `XDG_DATA_HOME` / `XDG_RUNTIME_DIR` are unset or relative (only the all-set happy path is tested in `crates/atrium-host/tests/lifecycle.rs:424-449`).

## The daemon's flaky tests (N1–N3): causes
1. **N1 (`sessions::a_stale_approval_is_expired_and_never_reaches_the_session`, and the `already has a session` failure in N2 `sessions::detached_is_cleared_by_resuming_the_thread`) — the worker's diagnosis is right, and the race window starts even earlier than `run()`'s end**:
   - In `crates/atrium-daemon/src/sessions.rs:196-203`, `Inner::launch` checks `if sessions.contains_key(thread)` first and refuses `ResumeThread` with `(ErrorCode::Invalid, "the thread already has a session")`.
   - Meanwhile, when a session emits `AgentEvent::SessionEnded`, `Inner::pump` (`crates/atrium-daemon/src/sessions.rs:514-521`) calls `self.store_batch(thread, incarnation, batch, Some(live)).await` (which commits `ThreadState::Ended` to SQLite in `crates/atrium-core/src/store/writer.rs:481-503`) and immediately broadcasts the `Ended` summary via `self.refresh_thread(thread).await` (`sessions.rs:340`).
   - After `pump` returns to `run()` (`sessions.rs:658`), `run()` still executes `process.over.lock().await` (`sessions.rs:736`), `inner.store.set_process(&thread, incarnation, None).await` (`sessions.rs:739`), `inner.store.signal(&thread, ending).await` (`sessions.rs:742`), and `inner.refresh_thread(&thread).await` (`sessions.rs:743`) — three async SQLite round-trips — before finally removing `thread` from `inner.sessions()` at `sessions.rs:746-754`!
   - Thus any client (such as `wire.thread_when(&thread, |s| s.state == ThreadState::Ended)` at `crates/atrium-daemon/tests/sessions.rs:186`) that sends `ResumeThread` as soon as it observes `Ended` races `sessions.rs:515..752` and gets `Invalid: "the thread already has a session"`.
   - In N2 (`crates/atrium-daemon/tests/sessions.rs:1196-1197`), the second failure mode (`left: Starting, right: Idle`) is a separate test/daemon race: `Inner::resume` (`crates/atrium-daemon/src/sessions.rs:207, 220-221`) returns `ThreadResumed` while the new incarnation is still in `ThreadState::Starting`, before the spawned `run()` task processes `AgentEvent::SessionStarted` in `pump` (`tests/common/mod.rs:1203` in `a_stopped_thread_can_be_resumed` uses `thread_when(..., Idle)`, whereas `tests/sessions.rs:1197` asserts `wire.thread(&id).await.state == ThreadState::Idle` immediately).

2. **N3 (`sessions::a_process_the_adapter_left_running_is_killed_or_reported` at `crates/atrium-daemon/tests/sessions.rs:1378` and `recovery::a_killed_daemon_is_recovered_at_the_next_start` at `crates/atrium-daemon/tests/recovery.rs:148`) — why a zombie `sleep 600` produces `Cleanup::Unverified` (and `alive(pid) == true`)**:
   - Both tests fail at the exact same underlying point (`supervise::kill_recorded` returning `Cleanup::Unverified` while `pid` is still in the kernel PID table; `sessions.rs:1378` asserts `summary.state == Interrupted { cleanup: Verified }` before `assert!(!alive(pid))` at line 1386, whereas `recovery.rs:148` asserts `assert!(!alive(pid))` before `summary.state == Interrupted { cleanup: Verified }` at line 152).
   - **Direct cause (unreaped zombie `sleep 600` in `recorded.pgid`)**:
     - When `supervise::kill_recorded` (`crates/atrium-host/src/supervise.rs:193-205`) sends `SIGKILL` to `-recorded.pgid`, `sleep 600` (`pid == pgid`) enters `EXIT_ZOMBIE` (`state == 'Z'`).
     - On Linux (`kernel/signal.c: kill_pgrp_info`), an `EXIT_ZOMBIE` task remains attached to its `PIDTYPE_PGID` and `PIDTYPE_PID` lists until its parent reaps it via `waitpid`/`waitid` (`release_task`). Consequently, while `sleep 600` is a zombie, `signal_group(recorded.pgid, 0)` (`libc::kill(-raw_pgid, 0)`) returns `0` (not `ESRCH`), so `group_is_empty(recorded.pgid)` (`supervise.rs:243-248`) returns `false` and `kill -0 <pid>` (`alive(pid)` in `recovery.rs:63-69` and `sessions.rs:1350-1356`) returns `true`.
     - Unlike `supervise::terminate` (`supervise.rs:130`: `let status = child.child.wait().await?;`), which reaps the group leader before calling `wait_for_group_drain`, `kill_recorded` does not own `Child` and cannot reap `recorded.pid`. It blocks in `wait_for_group_drain_sync(recorded.pgid, Duration::from_secs(2))` (`supervise.rs:200`) waiting for an external parent to reap the zombie `sleep 600`.
     - In `sessions::a_process_the_adapter_left_running_is_killed_or_reported` (`crates/atrium-daemon/tests/sessions.rs:1371-1372`), `sleep 600` is a child of the test binary itself, reaped only by `let reaping = tokio::spawn(async move { provider.child.wait().await });`. Furthermore, if `reaping` is scheduled between `script.die()` and `kill_recorded` only after `SIGKILL`, or if Tokio's process-global `SIGCHLD` driver across 57 concurrent `#[tokio::test]` runtimes in the `sessions` binary delays waking `reaping` beyond `GROUP_DRAIN_TIMEOUT` (2 s), `wait_for_group_drain_sync` times out with `sleep 600` still a zombie (`'Z'`) in `recorded.pgid` and returns `Cleanup::Unverified`. (Similarly, if `sleep 600` is already `'Z'` when `identify(recorded.pid)` runs at `supervise.rs:180`, `identify` returns `Ok(None)` and line 183 checks `group_is_empty(recorded.pgid)` once without waiting at all, immediately returning `Cleanup::Unverified`.)
   - **Second race in `spawn_group` -> `identify_spawned` (`posix_spawn` `CLONE_VFORK` window on Linux) that also explains why `recovery.rs:148` failed in a solo run on an idle machine (`docs/spikes/M1-linux-checks.md:39`)**:
     - In `supervise::spawn_group` (`crates/atrium-host/src/supervise.rs:60-72`), `command.process_group(0)` + `command.spawn()?` uses glibc `posix_spawnp` (`clone(CLONE_VM | CLONE_VFORK)`). Inside the kernel's `execve("/usr/bin/sleep")` (`fs/exec.c: exec_mmap`), `exec_mm_release(tsk, old_mm)` -> `mm_release` -> `complete_vfork_done(tsk)` wakes the parent thread from `clone(CLONE_VFORK)` *before* `task_lock(tsk); tsk->mm = mm; task_unlock(tsk)` replaces `old_mm` with the new binary's `mm`.
     - If the parent thread on another CPU core runs `identify_spawned(pid)` -> `identify(pid)` during those microseconds before `tsk->mm = mm`, `first = read_linux_stat(pid)` and `after = read_linux_stat(pid)` both see the child's `pgid == pid` and `start_time`, while `read_link("/proc/<pid>/exe")` (`get_task_exe_file`) reads `old_mm->exe_file` — the **test binary** (`recovery-...` or `sessions-...`) instead of `/usr/bin/sleep`.
     - `spawn_group` then records `recorded.executable = <test-binary>`. Later, `kill_recorded(&recorded)` reads `live.executable = "/usr/bin/sleep" != recorded.executable` (`supervise.rs:190`) and returns `Cleanup::Unverified` immediately **without sending `SIGKILL`**, leaving `sleep 600` alive at `recovery.rs:148` (`assert!(!alive(pid))`) and `sessions.rs:1378`.
