# Review: M1-14a-linux-host, round 2 (fix round 1)
Reviewer: Gemini 4 Argon (read-only) · 2026-11-14
## Verdict
**approve**: All four round-1 findings and the nit are resolved, the fail-closed rule holds across every caller of `identify`, and the one remaining edge case in multithreaded leader exit (`PF_EXITING` when `after.num_threads > 1`) is non-blocking and fails closed (`Cleanup::Unverified`).

## Round 1 findings
- **Finding 1 (`after.pgid != first.pgid` -> `Unverifiable`)**: **closed** — `judge_exe_read` (`crates/atrium-host/src/supervise.rs:636`) now rejects `after.start_time != first.start_time || after.pgid != first.pgid` with `ExeVerdict::Unverifiable`, and the three new table rows (`supervise.rs:846-866`) fail on the old code (where `after.pgid == 9000 != first.pgid == 100` with `start_time == 500` returned `Identity` on `Ok(())` and `NoProcess` on `ENOENT`/`PF_EXITING` and `EACCES`/`'Z'`); see below for the `after.pgid < first.pgid` variant not in the table.
- **Finding 2 (thread-group leader in `'Z'` with `num_threads > 1`)**: **closed with a gap** — `parse_linux_proc_stat` (`crates/atrium-host/src/supervise.rs:576-580`) parses field 20 (`num_threads`), `LinuxStat::is_dead` (`supervise.rs:534-540`) treats `'Z'` as dead only when `self.num_threads <= 1`, `read_linux_exe` (`supervise.rs:668-693`) falls back to `/proc/<pid>/task/<tid>/exe`, and the integration test `leader_exited::a_process_whose_main_thread_exited_is_still_identified_and_killed` (`crates/atrium-host/tests/supervise.rs:456-504`) fails on the old code (`identify(pid)` returned `Ok(None)` at `first.is_dead()`), while preserving the fail-closed rule; however, `judge_exe_read` (`supervise.rs:642`) still treats `after.flags & PF_EXITING != 0` as `NoProcess` even when `after.num_threads > 1` and `read_linux_exe` (`supervise.rs:673`) gates its `/proc/<pid>/task` fallback on `first.state == 'Z'` (New finding 1 below).
- **Finding 3 (four surviving mutations of `judge_exe_read` and `LinuxStat::is_dead`)**: **closed** — the new rows in `exe_read_decision_table` (`crates/atrium-host/src/supervise.rs:920-969`) and `dead_states_table` (`supervise.rs:788-810`) fail on the old table/code and kill all four round-1 mutations (`(ENOENT, 'S', PF_KTHREAD, 501)` at line 920 and `(EACCES, 'Z', LIVE, 501)` at line 927 kill reordering `PF_KTHREAD`/`is_dead` above `start_time`; `start_time: 499` at lines 934–947 kills `!=` -> `>`; `'x'` at lines 795, 802, 948–961 kills dropping `'x'`; `(EACCES, None, NoProcess)` at line 969 kills restricting `after == None`); see below for two remaining `pgid` mutations not caught by the table.
- **Finding 4 (`perf.rs` 60 s idle window overlapping `upkeep::snapshot_daily`)**: **closed** — `wait_for_snapshot` (`crates/atrium-daemon/examples/perf.rs:284-329, 414-443`) waits until the first daily snapshot in `snapshots/` is non-empty and unchanged in `(name, bytes, modified)` across four 250 ms polls (`750 ms` span) plus a `1 s` settle (`~1.8–1.9 s` total after `VACUUM INTO`'s final write) before starting the 60 s idle window, and `docs/spikes/M1-linux-checks.md:72-160` backs every number in its summary table with the raw `perf` output lines (median idle RSS `18.4`, `18.6`, `18.6 MiB` < `50 MB`).
- **Nit (`PF_FORKNOEXEC` label on `0x0040_0000`)**: **closed** — `crates/atrium-host/src/supervise.rs:783` now labels `const LIVE: u64 = 0x0040_0000; // PF_RANDOMIZE, an ordinary task`.

---

## Detailed verification of Questions 1.1–1.4

### 1.1 Finding 1: `pgid` changed across `read_link`
- **New test on old code**: In `exe_read_decision_table` (`crates/atrium-host/src/supervise.rs:846-866`), `first` has `pgid: 100, start_time: 500` (`supervise.rs:761-769`) and `after` has `pgid: 9000, start_time: 500`. On the pre-fix `judge_exe_read` (`if after.start_time != first.start_time`), line 847 (`"exe ok, process group changed"`) returned `ExeVerdict::Identity` instead of `ExeVerdict::Unverifiable`, and lines 853 and 860 returned `ExeVerdict::NoProcess` instead of `ExeVerdict::Unverifiable`.
- **Variant not covered by the test**: `after.pgid < first.pgid` — e.g., `first` has `pgid = 4242, start_time = 500` and `after` has `pgid = 0, start_time = 500` (same-tick PID reuse by a kernel thread `PF_KTHREAD`) or `pgid = 50, start_time = 500` (joining an earlier process group via `setpgid(0, 50)`). At `supervise.rs:636`, `after.pgid != first.pgid` evaluates `0 != 4242` / `50 != 4242` -> `true` and returns `ExeVerdict::Unverifiable`.

### 1.2 Finding 2: thread-group leader in `'Z'` with `num_threads > 1` and the fail-closed rule
- **New test on old code**: In `leader_exited::a_process_whose_main_thread_exited_is_still_identified_and_killed` (`crates/atrium-host/tests/supervise.rs:456-504`), the helper's main thread calls `syscall(SYS_EXIT, 0i64)` (`tests/supervise.rs:429`) while a worker thread sleeps 600 s (`tests/supervise.rs:423`). Once `state_of(pid) == Some('Z')` (`tests/supervise.rs:476`), `identify(pid)` is called (`tests/supervise.rs:492`). On the old code (`matches!(self.state, 'Z' | 'X' | 'x')`), `first.is_dead()` returned `true` and `identify(pid)` returned `Ok(None)`, failing `identity.expect("the process is alive: it has an identity")` at `tests/supervise.rs:493`.
- **Why the fail-closed rule holds (can `identify` turn a `None` into anything other than a full field-for-field matching identity or `Err`?)**:
  - In `platform_identify` (`crates/atrium-host/src/supervise.rs:695-724`), `Ok(Some(ProcessIdentity { pid, pgid: first.pgid, start_time: first.start_time, executable }))` is constructed only at lines 712–717 when `verdict == ExeVerdict::Identity` and `exe == Ok(executable)`.
  - `judge_exe_read` (`supervise.rs:626-647`) returns `ExeVerdict::Identity` only when `after` is `Some`, `after.start_time == first.start_time`, `after.pgid == first.pgid`, and `exe` is `Ok(())`.
  - `read_linux_exe` (`supervise.rs:668-693`) only reads `/proc/{pid}/exe` or `/proc/{pid}/task/<tid>/exe` for threads in `pid`'s thread group during the `[first, after]` window (bounded by the same `start_time` and `pgid`).
- **Cases where a real zombie (all threads gone, zombie owned by someone else, or reused PID) is treated as live, and what `kill_recorded` does**:
  1. **All threads gone between `read_linux_exe` and `after` (real zombie at `after`, still treated as `Ok(Some(live))`)**:
     - Where: `crates/atrium-host/src/supervise.rs:636-641` (`judge_exe_read`) and `695-717` (`platform_identify`):
       ```rust
       if after.start_time != first.start_time || after.pgid != first.pgid {
           return ExeVerdict::Unverifiable;
       }
       match exe {
           Ok(()) => ExeVerdict::Identity,
           Err(_) if after.is_dead() => ExeVerdict::NoProcess,
       ```
     - Case: At `first = read_linux_stat(pid)?` (`supervise.rs:696`), the leader is `'Z'` and `first.num_threads == 2` (`first.is_dead() == false`). `read_linux_exe` (`supervise.rs:688`) reads `Ok(executable)` from `/proc/<pid>/task/<worker_tid>/exe`. Immediately afterward, `<worker_tid>` exits and is auto-reaped by the kernel, leaving the process a true zombie with all threads gone (`state == 'Z'`, `num_threads == 1`, unreaped by its parent). `after = read_linux_stat(pid)?` (`supervise.rs:705`) reads `state = 'Z'`, `num_threads = 1` (`after.is_dead() == true`), with the same `start_time` and `pgid`. Because `judge_exe_read` (`supervise.rs:640`) matches `Ok(()) => ExeVerdict::Identity` before `after.is_dead()` (just as it already did when `first` was `'S'` and `after` was `'Z'`, `supervise.rs:830-835`), `identify(pid)` returns `Ok(Some(live))` for a process that is now a full zombie.
     - What `kill_recorded` (`crates/atrium-host/src/supervise.rs:189-206`) does:
       - It checks `if live != *recorded { return Cleanup::Unverified; }` (`supervise.rs:190`).
       - If the zombie belongs to a reused PID or different executable (`live != *recorded`), `kill_recorded` returns `Cleanup::Unverified` without signalling.
       - Only if `live == *recorded` on every field (`pid`, `pgid`, `start_time`, `executable` — i.e., it is the recorded Atrium provider process that just finished dying) does `kill_recorded` call `signal_group(recorded.pgid, libc::SIGKILL)` (`supervise.rs:193`), wait up to `GROUP_DRAIN_TIMEOUT` (2 s) in `wait_for_group_drain_sync` (`supervise.rs:200`) for the zombie to be reaped and any descendants in `recorded.pgid` to exit, and return `Cleanup::Verified` iff `group_is_empty(recorded.pgid)` holds.
  2. **Process with `'Z'` leader and live worker (`num_threads > 1`) owned by another user (UID mismatch)**:
     - Case: `first = read_linux_stat(pid)?` succeeds (`/proc/<pid>/stat` is mode `0444`) with `state = 'Z'`, `num_threads = 2` (`first.is_dead() == false`, whereas pre-fix it returned `Ok(None)`). `read_linux_exe` (`supervise.rs:669`) calls `std::fs::read_link("/proc/<pid>/exe")`, which fails in the kernel's `proc_exe_link -> ptrace_may_access` check with `EACCES` (`io::ErrorKind::PermissionDenied`). `read_linux_exe` (`supervise.rs:673`) sees `err.kind() != io::ErrorKind::NotFound` and returns `Err(PermissionDenied)`. `after = read_linux_stat(pid)?` reads `state = 'Z'`, `num_threads = 2` (`after.is_dead() == false`). `judge_exe_read` (`supervise.rs:645`, tested at `supervise.rs:1024-1029`) returns `ExeVerdict::Unverifiable`, so `identify` returns `Err(PermissionDenied)`.
     - What `kill_recorded` (`supervise.rs:180-181`) does: `Err(_) => Cleanup::Unverified` immediately, without signalling anything.
  3. **Reused PID (same UID) whose leader is in `'Z'` with `num_threads > 1`**:
     - Case: `first.is_dead()` is `false`, `read_linux_exe` reads the new process's worker `exe`, and `identify` returns `Ok(Some(live))` with the new process's `start_time`, `pgid`, and `executable`.
     - What `kill_recorded` (`supervise.rs:189-192`) does: `live.start_time != recorded.start_time` (and/or `live.executable != recorded.executable`), so `live != *recorded` returns `Cleanup::Unverified` without signalling.

### 1.3 Finding 3: mutations of `judge_exe_read` and `LinuxStat::is_dead`
- **All four round-1 mutations are killed** by `supervise.rs:788-810, 920-969`.
- **Two mutations the current table (`crates/atrium-host/src/supervise.rs:813-1059`) would still not catch**:
  1. **Mutate `after.pgid != first.pgid` (`supervise.rs:636`) to `after.pgid > first.pgid`**: `first` has `pgid: 100` (`supervise.rs:766`), and every changed-`pgid` row in `exe_read_decision_table` (`supervise.rs:846-866`) uses `in_group(..., 9000)` (`9000 > 100`). No row tests `after.pgid < first.pgid` (e.g., `in_group(same('S', LIVE), 50)` or `in_group(same('S', PF_KTHREAD), 0)`).
  2. **Move `Err(io::ErrorKind::NotFound) if after.flags & PF_KTHREAD != 0 => ExeVerdict::NoProcess` (`supervise.rs:642`) above `if after.pgid != first.pgid` (`supervise.rs:636`)**: the changed-`pgid` rows test `(Ok(()), 'S', LIVE, pgid=9000)`, `(ENOENT, 'S', LIVE | PF_EXITING, pgid=9000)`, and `(EACCES, 'Z', LIVE, pgid=9000)`, but not `(ENOENT, in_group(same('S', PF_KTHREAD), 0), Unverifiable)`.

### 1.4 Finding 4: `perf.rs` (`wait_for_snapshot`), `upkeep.rs`, and the L8 report
- **Can `wait_for_snapshot` start the 60 s window while `VACUUM INTO` is still writing?**
  - `upkeep::snapshot_daily` (`crates/atrium-daemon/src/upkeep.rs:156-188`) sleeps `SNAPSHOT_FIRST_CHECK` (`60 s`, line 19), acquires `inner.publish.lock().await` (`upkeep.rs:181`), verifies `inner.running_turns() == 0` (`upkeep.rs:182`), and calls `inner.store.snapshot(&dir).await` (`upkeep.rs:185`).
  - `snapshot::snapshot` (`crates/atrium-core/src/store/snapshot.rs:76-102`) pre-creates an empty `0600` file `atrium-YYYYMMDD-HHMMSS-mmm.db` (`snapshot.rs:82-95`) and runs `VACUUM INTO ?1` (`snapshot.rs:96`).
  - `wait_for_snapshot` (`crates/atrium-daemon/examples/perf.rs:284-329`) ignores empty files (`meta.len() == 0`, line 296) and requires 4 consecutive polls separated by `250 ms` (`750 ms` span, lines 306–310, 327) with identical `(name, bytes, modified)`, followed by a `1 s` sleep (`perf.rs:312`).
  - On this machine, the entire `811 MiB` `VACUUM INTO` took `0.65–1.43 s` from file creation (filename UTC timestamp to `modified`: `1794672325980` -> `1794672326632 ms` in Run 1, `1794672463701` -> `1794672464979 ms` in Run 2, `1794672601285` -> `1794672602715 ms` in Run 3, `docs/spikes/M1-linux-checks.md:116, 136, 156`), and all three runs observed the completed `811 MiB` snapshot before starting the window. Only a disk stall of `>= 750 ms` mid-`VACUUM INTO` after writing at least one page could make `wait_for_snapshot` return early.
- **Can it start the window while `record_snapshot` (or anything else the daemon does after a snapshot) still runs?**
  - After `VACUUM INTO` returns, `snapshot::snapshot` (`crates/atrium-core/src/store/snapshot.rs:103-107`) calls `Writer::record_snapshot` (`crates/atrium-core/src/store/writer.rs:712-733`), which executes one `INSERT OR REPLACE INTO snapshots`, calls `snapshot::prune(dir)` (1 file <= `SNAPSHOTS_KEPT = 7`, so 0 deletions), and commits. With all sessions closed and no clients connected, the writer thread is idle and finishes `record_snapshot` in milliseconds — well within `wait_for_snapshot`'s `750 ms` stability check + `1 s` sleep (`1,790–1,936 ms` between `snapshot_ms` and `window_ms` in Runs 1–3, `M1-linux-checks.md:116-117, 136-137, 156-157`).
  - After `snapshot_daily` releases `inner.publish` (`upkeep.rs:188`), it sleeps `SNAPSHOT_EVERY` (`24 h`, `upkeep.rs:18, 165`). The only task that runs during the 60 s window is `upkeep::watch_storage` (`upkeep.rs:114-145`), which calls `statvfs` (`crates/atrium-core/src/store/mod.rs:179-185`) every `2 s` without touching SQLite or allocating.
- **Can `wait_for_snapshot` pick a file that isn't the snapshot?**
  - `start_daemon` (`perf.rs:250-253`) creates a fresh `/tmp/atrium-perf*` directory for each run, so `snapshots/` contains only what `snapshot::snapshot` creates. While `wait_for_snapshot` (`perf.rs:294-301`) checks `meta.is_file() && meta.len() > 0` without filtering by `name.starts_with("atrium-") && name.ends_with(".db")` (unlike `snapshot::prune` at `snapshot.rs:53`), SQLite removes its `-journal` sidecar when `VACUUM INTO` commits, and the raw output (`M1-linux-checks.md:116, 136, 156`) confirms the file picked in all three runs was `atrium-20261114-*.db` (`811 MiB`).
- **Does the report support "10 samples, 60 s or more after the last activity" with an output line behind each number?**
  - Yes: every cell in the L8 table (`docs/spikes/M1-linux-checks.md:84-97`) is backed directly by the pasted output blocks at `M1-linux-checks.md:102-160`, including `snapshot finished: ... (811 MiB, modified <ms>); RSS ...`, `idle window starts at <ms> (... after daemon start), waiting 60 s…`, and the 10-sample arrays `[18.4, ...]` / `[18.6, ...]`. It also resolves the round-1 mystery: RSS is `15.2–15.5 MiB` at session close, rises to `18.4–18.6 MiB` during `VACUUM INTO`, and stays flat at `18.4–18.6 MiB` (`19.3–19.5 MB` < `50 MB`) across all 10 samples 60 s later.
- **Does the unrecorded load average before each run matter for this gate?**
  - No. Contract §14's gate here is `Daemon idle RSS < 50 MB` (`docs/contracts/m1.md:198`), measured from `/proc/<pid>/status` (`VmRSS` and `VmHWM`, `perf.rs:141-151`). Host CPU load does not reduce a process's resident anonymous and SQLite page-cache memory (and `VmRSS == VmHWM == 18.4–18.6 MiB` proves zero pages were reclaimed/swapped). Furthermore, the companion timing lines in those same runs (`15.1–15.4 s` for 3M-token load, `2.54–2.83 ms` median reconnect, `M1-linux-checks.md:87, 96`) confirm the host was not under heavy contention.

---

## New findings
1. [minor] **When a multithreaded process's leader thread is in `PF_EXITING` or `'Z'` (`after.flags & PF_EXITING != 0`) while worker threads are still alive (`after.num_threads > 1`), `judge_exe_read` still returns `ExeVerdict::NoProcess` on `ENOENT`, and `read_linux_exe` skips `/proc/<pid>/task` if `first.state != 'Z'`**
   - Where: `crates/atrium-host/src/supervise.rs:639-646` (`judge_exe_read`), `crates/atrium-host/src/supervise.rs:668-675` (`read_linux_exe`), and `crates/atrium-host/src/supervise.rs:819, 1010-1022` (`exe_read_decision_table`):
     ```rust
     match exe {
         Ok(()) => ExeVerdict::Identity,
         Err(_) if after.is_dead() => ExeVerdict::NoProcess,
         Err(io::ErrorKind::NotFound) if after.flags & (PF_EXITING | PF_KTHREAD) != 0 => {
             ExeVerdict::NoProcess
         }
         Err(_) => ExeVerdict::Unverifiable,
     }
     ```
     ```rust
     fn read_linux_exe(pid: u32, first: &LinuxStat) -> io::Result<PathBuf> {
         let own = std::fs::read_link(format!("/proc/{pid}/exe"));
         let Err(err) = &own else {
             return own;
         };
         if err.kind() != io::ErrorKind::NotFound || first.state != 'Z' || first.num_threads <= 1 {
             return own;
         }
     ```
     ```rust
     let leader_z = with_threads(stat('Z', LIVE | PF_EXITING, 500), 2);
     ...
     (
         "Z leader with a worker, no exe, exiting flag",
         leader_z,
         ENOENT,
         Some(leader_z),
         NoProcess,
     ),
     (
         "Z leader with a worker, no exe, no exiting flag",
         leader_z,
         ENOENT,
         Some(with_threads(stat('Z', LIVE, 500), 2)),
         Unverifiable,
     ),
     ```
   - Case:
     - In Linux `kernel/exit.c: do_exit`, `exit_signals(tsk)` sets `PF_EXITING` (`0x0000_0004`) on the leader task before `exit_mm()` drops `tsk->mm` and before `exit_notify()` sets `EXIT_ZOMBIE` (`'Z'`), and `PF_EXITING` remains set on the leader for its entire time in `'Z'`. Like `state`, field 9 (`flags`) of `/proc/<pid>/stat` is the leader thread's `task->flags`.
     - **Subcase A (leader exits between `first` and `read_link`, or is in `do_exit()` between `exit_mm()` and `exit_notify()` at `first`)**: `first` has `state = 'R'` (or `'S'`), `num_threads = 2`. `std::fs::read_link("/proc/<pid>/exe")` returns `Err(ENOENT)`. Because `first.state != 'Z'` (`supervise.rs:673`), `read_linux_exe` does not check `/proc/<pid>/task/<tid>/exe` and returns `Err(ENOENT)`. `after` has `state = 'Z'` (or `'R'`), `flags = LIVE | PF_EXITING`, `num_threads = 2`.
     - **Subcase B (`first.state == 'Z'`, `num_threads == 2`, and `read_linux_exe` fails to read a worker's `exe` during thread churn)**: `after` is `leader_z` (`state = 'Z'`, `flags = LIVE | PF_EXITING`, `num_threads = 2`). (Note that row `"Z leader with a worker, no exe, no exiting flag"` at `supervise.rs:1016-1022` uses a `'Z'` state without `PF_EXITING`, which the Linux kernel never produces.)
   - Result: In both subcases, `judge_exe_read` matches `Err(io::ErrorKind::NotFound) if after.flags & (PF_EXITING | PF_KTHREAD) != 0` (`supervise.rs:642`) and returns `ExeVerdict::NoProcess` (`Ok(None)`) even though `after.num_threads == 2` says the process still has live threads. In `kill_recorded` (`supervise.rs:182-188`), `Ok(None)` skips `signal_group(recorded.pgid, libc::SIGKILL)`, sees `group_is_empty(recorded.pgid) == false`, and returns `Cleanup::Unverified` without killing the live recorded process (failing closed per contract §10, never returning false `Verified` or signalling an unverified group). It should return `ExeVerdict::Unverifiable` when `exe` is `ENOENT` and `after.num_threads > 1` (and `read_linux_exe` should consult `/proc/<pid>/task` whenever `first.num_threads > 1`, even if `first.state != 'Z'`).
   - Fix: In `read_linux_exe` (`crates/atrium-host/src/supervise.rs:673`), drop `|| first.state != 'Z'`; in `judge_exe_read` (`crates/atrium-host/src/supervise.rs:642`), require `after.num_threads <= 1` for `PF_EXITING` (matching `LinuxStat::is_dead` at `supervise.rs:537`):
     ```rust
     Err(io::ErrorKind::NotFound)
         if (after.flags & PF_EXITING != 0 && after.num_threads <= 1)
             || after.flags & PF_KTHREAD != 0 =>
     {
         ExeVerdict::NoProcess
     }
     ```

## Suspected, not shown
- None beyond the pre-existing same-10 ms-tick (`USER_HZ = 100`) `pid_max` wraparound noted in round 1.

## Tried and held
- **Checked whether `read_linux_exe` (`crates/atrium-host/src/supervise.rs:668-693`) can return an executable from a reused PID and cause `kill_recorded` to signal the wrong group**: Held. Even if `pid` exited, was reaped, and was reused while `read_linux_exe` walked `/proc/{pid}/task`, `platform_identify` (`supervise.rs:705-710`) reads `after = read_linux_stat(pid)?` afterwards and `judge_exe_read` (`supervise.rs:636`) rejects any change in `start_time` or `pgid` with `ExeVerdict::Unverifiable` (`Err` -> `Cleanup::Unverified`).
- **Checked whether `parse_linux_proc_stat` (`crates/atrium-host/src/supervise.rs:568-586`) shifts field 22 (`starttime`) after adding field 20 (`num_threads`)**: Held. After field 9 (`flags`), `.nth(10)` skips fields 10..=19 (10 fields) and consumes field 20 (`num_threads`), and `.nth(1)` skips field 21 (`itrealvalue`) and consumes field 22 (`starttime`), matching the pre-change `.nth(12)` (`10 + 1 + 1 = 12`).
- **Checked whether `leader_exited` (`crates/atrium-host/tests/supervise.rs:373-505`) can leak its compiled binary, temp directory, or helper process on test failure**: Held. `Dir` (`tests/supervise.rs:402-406`) removes the temp directory on drop, and `_group = KillGroupGuard(pid)` (`tests/supervise.rs:465`) sends `SIGKILL` to `-pid` on drop.

## Test gaps
- **`exe_read_decision_table` (`crates/atrium-host/src/supervise.rs:846-866`)**: Add a row with `after.pgid < first.pgid` (e.g., `(ENOENT, Some(in_group(same('S', PF_KTHREAD), 0)), Unverifiable)`) to kill `after.pgid != first.pgid` -> `after.pgid > first.pgid` and reordering `PF_KTHREAD` above the `pgid` check.
