# Review: M1-10-host, the Linux exiting-process fix (8ef0ff1)
Reviewer: Claude Sonnet 5.5 (read-only, D-060) · 2026-11-12
## Verdict
**approve with changes**: the fix is safe because every caller treats `Ok(None)` conservatively, but it is broader than its comment says (any live mm-less process, not only an exiting one) and its test does not exercise the new branch deterministically.

Checks run on macOS: `cargo test -p atrium-host` passes, clippy `-D warnings` clean. The Linux code was read, not compiled.

## Scope of what was checked
- `identify()` callers: `kill_recorded` (supervise.rs:180), `identify_spawned` (supervise.rs:351), `lifecycle.rs:217` (own pid), `peer.rs:526` (macOS `MacSigned` fallback only).
- Linux peer verification (peer.rs:787-812) reads `/proc/<pid>/exe` itself and uses the pidfd; it does not call `identify()`. The commit does not touch or weaken §4. The macOS path is unchanged.

## Findings

1. **[minor] The branch accepts any `ENOENT` on a live process, not "an exiting process"**
   Where: crates/atrium-host/src/supervise.rs:572-577.
   Problem: `readlink /proc/<pid>/exe` gives `ENOENT` whenever the task has no `mm`/exe file: an exiting task (after `exit_mm`, before `Z`), a kernel thread (visible to root; a non-root caller gets `EACCES` and still errors), and a task whose `/proc` entry vanished. The code does not check that the process is exiting; it only checks that it is not `Z/X/x`. Consequence analysis, all fail-safe: `Ok(None)` means "no such running process" and every consumer then falls back to `group_is_empty(pgid)` (supervise.rs:182), which returns `Verified` only if the group has no members, so a reused pid that is a kernel thread or a different exiting process cannot cause a kill or a false `Verified` while the old group is alive. A zombie owned by someone else is `Z`, so it was already `None`. A live non-exiting process with an unreadable exe for another reason (`EACCES`, `EPERM`) still returns `Err`. Nothing is accepted that was previously refused as an identity; the only change is `Err` to `None`, which is more permissive of "gone" but never produces a `Some`.
   Fix: make the claim true and narrow. On `ENOENT` with a live state, also test `PF_EXITING` (0x4) in the stat flags (field 9) and treat `PF_KTHREAD` (0x200000) as "not a process", returning `Err` for anything else. That keeps the original fail-closed behaviour for the unexplained case (for example `ENOENT` on a live ordinary process, which should never happen). At minimum, reword the comment to say "no executable file (exiting, or kernel thread)".

2. **[minor] Two-step stat/exe read is not atomic, and the retry still cannot detect a replaced process**
   Where: crates/atrium-host/src/supervise.rs:558-579.
   Problem: stat (state, pgid, start_time) is read, then exe, then on error stat again. If the pid is reaped and reused between the first stat and the exe read, the success path records `pgid/start_time` of process A with the `exe` of process B (pre-existing, not introduced here). In the new path, the second stat is only used for the state, not compared with the first `start_time`, so an A-to-B replacement during an `ENOENT` (B mid-`execve` or a kthread) is accepted as `None`. The outcome is still fail-safe (see 1), and in `identify_spawned` the pid cannot be reused because the child is not yet reaped.
   Fix: after a successful `read_link`, and on the `ENOENT` path, re-read stat and require the same `start_time`; otherwise return `Err` (or retry once). This closes the race for the success path too.

3. **[minor] The test does not meaningfully cover the new branch**
   Where: crates/atrium-host/tests/supervise.rs:337.
   Problem: `fast_exiting_process_records_identity_and_terminates_cleanly` hits the window only occasionally (41/1200 under load before the fix per the commit message), and it passes on macOS where the branch does not exist. A regression would show as an intermittent failure, not a reliable one. No unit test covers the classification, and the "recorded without a real exe" outcome (identity built from `resolve_spawn_executable`, supervise.rs:358) is not asserted.
   Fix: split the decision into a pure function over `(io::ErrorKind, Option<state>, flags)` that compiles on all platforms (like `parse_linux_proc_stat`, which has `cfg(any(linux, test))`), and unit-test: `ENOENT` + `S` + exiting flag -> `None`; `ENOENT` + `Z` -> `None`; `EACCES` + `S` -> `Err`; `ENOENT` + `S` + no exiting flag -> `Err`. Optionally run the existing test in a loop (for example 200 iterations) so the window is hit more often in CI.

4. **[nit] An identity recorded without a real exe is a guess**
   Where: crates/atrium-host/src/supervise.rs:345-364.
   Problem: when `identify` now returns `None` for an exiting-but-not-`Z` child, `identify_spawned` takes the "zombie" path and records a resolved-path `executable`. For a shebang script the real exe is the interpreter, so a later `kill_recorded` of a leader that is somehow still alive would see `live != recorded` and answer `Unverified` (safe, nothing killed). It cannot cause a wrong kill, because a process with no mm cannot later regain an exe. The comment at supervise.rs:354 still says "exited into a zombie state", which is now incomplete.
   Fix: update that comment to say "exited or is exiting (no executable yet readable)".

## Answers to the specific questions
- Pid reused by another process: cannot yield a `Some`, only `None`, which routes through the group-empty check. Safe.
- Live process, exe unreadable for another reason: `EACCES`/`EPERM` and other kinds still return `Err` (`Unverified`). Only `ENOENT` is relaxed.
- Zombie owned by someone else: `Z` is detected by the existing state check; unchanged.
- Kernel thread: `ENOENT` for root callers, now `None`; safe, and a kernel-thread pgid is 0 so `kill_recorded` rejects it up front anyway.
- Order of reads: stat is read before and after the exe read, so a `Z` that appears during the window is caught. A full replacement is not detected (finding 2) but is fail-safe.
- Peer verification (§4): not weakened; Linux uses its own `/proc/<pid>/exe` + pidfd path, and the macOS path is unchanged.
