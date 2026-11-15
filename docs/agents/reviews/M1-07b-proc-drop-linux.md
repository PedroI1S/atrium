# Review: M1-07b-proc-drop-linux
Reviewer: Claude Opus 5.5 (read-only; D-053 unsafe audit) · 2026-11-15

## Verdict
**approve**: the fix is correct and sound. It signals the group before any reap on both paths, removes the dependence on the spawning runtime's reactor, and leaves `terminate` unchanged. All of `cargo test -p atrium-host -p atrium-agents` passed on my run (exit 0; `claude_adapter` 38/38, `supervise` 10/10). The findings below are minor or nits, and none of them is a regression.

## Findings

1. [minor] **A leader that has left its own group is never signalled, so the blocking reap waits for ever. This was already true before the change.**
   - Where: `crates/atrium-host/src/supervise.rs:124-126`, which goes into `reap_leader_blocking` at `:375-383` (a blocking `waitid` with no `WNOHANG`). It is reached from `crates/atrium-agents/src/claude/adapter.rs:440` on the dropping thread.
   - Case: the provider process calls `setpgid` or `setsid` after `spawn_group` has checked `pgid == pid`. `kill(-pgid, SIGKILL)` then either fails with `ESRCH`, which `:155` maps to `Ok(false)`, or reaches only the group's other members. The leader stays alive and `waitid` blocks until it exits on its own.
   - Result: a `Proc` dropped off a runtime blocks its caller indefinitely. The async `terminate` has the same exposure at `:105` (`child.child.wait().await`), and the old macOS drop path did too. So this is not new, but it now sits on a synchronous `Drop`.
   - Fix (optional, not needed for this task): in `terminate_blocking` only, also send `kill(pid, SIGKILL)` to the leader directly after the group kill. This is safe because the leader is unreaped, so its pid is still held. Alternatively, record the limitation in the doc comment.

2. [nit] **The recovery branch for `ECHILD` cannot succeed.**
   - Where: `supervise.rs:389-393`.
   - Case: the first `try_wait()` at `:367` has already returned `None`, so std holds no cached status. After `waitid` returns `ECHILD` (for example, someone else ran `waitpid(-1)`), `child.try_wait()` runs std's `waitpid(pid, WNOHANG)`. That also returns `ECHILD`, and the `?` sends it back as `Err`.
   - Result: the behaviour is correct (it returns `Err` and neither hangs nor spins), but the `let Some(status)` arm is never taken.
   - Fix: `return Err(err)` directly, or add a comment saying why the branch exists.

3. [nit] **`let _ = pid;` at `supervise.rs:143`** only silences an unused-variable warning on Linux. `#[cfg_attr(not(target_os = "macos"), allow(unused_variables))]` on the parameter says what it is for.

4. [nit] **The doc comment on `terminate_blocking` (`supervise.rs:111-120`) leaves out two things:**
   - how long it can block: up to `grace`, then an unbounded reap if the leader does not die (for example, in D state), then up to 2 s of drain;
   - that the caller must not reap first. `check_unreaped_leader` catches only a reap done through tokio.

**Safety order.** `terminate` (`:100-109`) matches the old body at 7042a06 step for step: the reaped check, `wait_until_exited_no_reap`, the group `SIGKILL` match with the macOS `EPERM && peek_exited` arm (now `:153-159`), `wait()`, the drain, and the `group_is_empty` / `EPERM` / `TimedOut` tail (now `finish_termination`, `:162-182`). `terminate_blocking` (`:121-130`) follows the same order. The only reaping calls are the `try_wait()` calls at `:367` and `:396`, and both run after `kill_group_while_unreaped` at `:124`. `peek_exited` and the blocking `waitid` both pass `WNOWAIT`. The macOS `EPERM` zombie branch is kept and shared by both paths.

**tokio interplay (tokio 1.54.1, from the registry source).**
- `tokio::process::Child::try_wait` (`process/mod.rs:1413-1426`) calls `imp::Child::try_wait`. That is `std_child().try_wait()` (`process/unix/mod.rs:168-170`) for both the `PidfdReaper` and `SignalReaper` variants. It is a plain `waitpid(WNOHANG)` with no reactor involved.
- On `Ok(Some)` it sets `guard.kill_on_drop = false` and switches to `FusedChild::Done`. This drops the guard, and `PidfdReaper::drop` (`pidfd_reaper.rs:203-210`) sees std's cached status, so nothing goes on the orphan queue. The `PollEvented<Pidfd>` deregisters with an `epoll_ctl` that is thread-safe, and its errors are ignored.
- So `kill_on_drop` does nothing afterwards, and `child.id()` returns `None`, which the new test asserts.

**In-runtime path** (`adapter.rs:436-437`). Nothing is lost compared with before. The old fallback (`kill_recorded` when building a runtime failed) is no longer needed. The change also fixes a latent case: before, the detached thread waited on a pidfd that the caller's current-thread runtime had to drive. If the caller then blocked or shut down, that thread was leaked and the leader was left a zombie.

**Paths.** Only `adapter.rs` (the `Drop` doc comment and `terminate_now`), `supervise.rs`, `tests/supervise.rs` and the handoff changed. `terminate`'s body was split into shared helpers, but its behaviour is identical, which fits "a new blocking variant and its helpers". The contract (`m1.md:21`, `:160`) does not fix `supervise`'s signatures, and the new function is purely additive, so no change request is needed.

## unsafe audit
There is one new `unsafe` block, `supervise.rs:375-383`. It is **sound**.
- `std::mem::zeroed::<libc::siginfo_t>()`: `siginfo_t` is a POD made of C ints and unions, so all-bits-zero is a valid value.
- `&mut info` stays valid for the whole call, and `info` is never read afterwards, so there is no union-field read.
- `raw_pid as libc::id_t`: `parse_raw_pid` (`:351-358`) guarantees `1..=i32::MAX`, and `id_t` is `u32` on both Linux and macOS, so the cast loses nothing.
- errno: `io::Error::last_os_error()` at `:385` is the first call after `waitid` returns. Leaving the block drops only `info`, which has no `Drop`, so errno cannot be overwritten.
- `EINTR` is retried.

**Busy-looping.** Without `WNOHANG`, `rc == 0` means the kernel reported a child in an exited state that is still waitable (`WNOWAIT` leaves it a zombie). The `waitpid(pid, WNOHANG)` that follows checks the same task with the same rules, so it reaps and returns `Some`. A concurrent reaper turns this into `ECHILD` and an `Err`, not `None`. So `rc == 0` followed by `None` cannot happen.

**Blocking.** Under ptrace, or when the leader is stopped, `SIGKILL` still ends the process; ptrace only delays the exit notification. With `SIGCHLD` set to `SIG_IGN` or `SA_NOCLDWAIT`, `waitid` returns `ECHILD` and the function returns `Err`. The real ways to block for ever are finding 1 and a leader stuck in D state, which `terminate` shares.

## Suspected, not shown
- **macOS was not run.** A blocking `waitid(P_PID, WEXITED|WNOWAIT)` on XNU, and `terminate_blocking` going through the `EPERM` branch, are unverified there. Of the existing tests, `fast_exiting_process_records_identity_and_terminates_cleanly` (`tests/supervise.rs:339`) covers only the async `terminate`.
- **The new tests were not checked against the old code.** The rules forbid creating the worktree I would need for that. My reading of the code: the first new test, with `terminate` on a fresh runtime in place of `terminate_blocking`, would hang. Its spawning runtime (`tests/supervise.rs:374-411`) is idle and never driven, which is the same mechanism as N7. The adapter regression has already been shown (N7, 10 of 10 hangs; handoff lines 9-22).

## Test gaps
- **Meaning of the new host tests.**
  - The first test (`tests/supervise.rs:373`) is a faithful reproduction of the mechanism: a grandchild, a spawning runtime that nobody drives, and a check that `child.id()` is `None`. On the old code it would hang rather than fail.
  - The second test (`:445`) covers an early exit after the runtime has been dropped. It is deterministic: EOF on stdout proves the leader has exited, so `killed == false` and `exit_code == Some(7)`.
- **Missing cases:**
  - a `terminate_blocking` test where the group holds only the zombie leader (the macOS `EPERM` path);
  - a test with `grace > 0` where the leader ignores it (`killed == true` after the grace period);
  - an adapter test that drops inside a current-thread runtime and then blocks that runtime, for the in-runtime path's detached thread.
