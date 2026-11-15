# Review: M1-07c-leader-sigkill (commits 3ae01cf, 0df1509)

Reviewer: Gemini 4 Hydrogen `high`, read-only (D-091). Author: Claude Opus 5.5. Saved by the orchestrator from the run's final message.


**Verdict:** **approve**

Commits [`3ae01cf`](crates/atrium-host/src/supervise.rs) and [`0df1509`](docs/agents/handoffs/M1-07c-leader-sigkill.md) cleanly fix N8 within the brief's owned paths (`crates/atrium-host/src/supervise.rs`, `crates/atrium-host/tests/supervise.rs`, `docs/agents/handoffs/M1-07c-leader-sigkill.md`), resolve all three nits and both host-level test gaps from [M1-07b-proc-drop-linux.md](.orchestration/review-input/M1-07b-proc-drop-linux.md), add no `unsafe` in `src/`, and pass formatting, clippy, and 10 consecutive runs of the `supervise` suite on macOS.

---

## Checks

### 1. The fix and unreaped-pid invariant
- **Both paths signal the leader by pid before reaping:**
  - `terminate` ([supervise.rs:106-116](crates/atrium-host/src/supervise.rs#L106-L116)) calls `check_unreaped_leader` → `wait_until_exited_no_reap` → `kill_group_while_unreaped` → `kill_leader_while_unreaped(&mut child.child)` (`:110`) → `child.child.wait().await` (`:112`).
  - `terminate_blocking` ([supervise.rs:137-147](crates/atrium-host/src/supervise.rs#L137-L147)) calls `check_unreaped_leader` → `wait_until_exited_no_reap_sync` → `kill_group_while_unreaped` → `kill_leader_while_unreaped(&mut child.child)` (`:141`) → `reap_leader_blocking(&mut child.child, pid)` (`:143`).
- **Cannot be reaped/reused before `kill_leader_while_unreaped`:**
  - `check_unreaped_leader` ([supervise.rs:149-157](crates/atrium-host/src/supervise.rs#L149-L157)) rejects an already-reaped tokio `Child` (`child.child.id().is_none()`).
  - `wait_until_exited_no_reap` / `_sync` and `kill_group_while_unreaped` only inspect via `peek_exited` ([supervise.rs:434-478](crates/atrium-host/src/supervise.rs#L434-L478)), which passes `WEXITED | WNOHANG | WNOWAIT`, leaving any zombie unreaped.
  - Both `terminate` and `terminate_blocking` hold `&mut SupervisedChild`, preventing concurrent reaping through `Child`. As documented at [supervise.rs:97-100](crates/atrium-host/src/supervise.rs#L97-L100) and [supervise.rs:129-131](crates/atrium-host/src/supervise.rs#L129-L131), the only excluded case is an out-of-band `waitpid(-1)` by the caller before `terminate*`, which violates the caller contract and affects `kill_group_while_unreaped` identically.
  - `kill_leader_while_unreaped` ([supervise.rs:187-193](crates/atrium-host/src/supervise.rs#L187-L193)) calls `child.start_kill()`, which sends `SIGKILL` by pid while unreaped and ignores `ESRCH` (e.g. an already-zombie leader on XNU). If `kill_group_while_unreaped` returns `Err`, `?` returns early without calling `start_kill`, preserving `kill_on_drop`.
- **macOS `EPERM` zombie handling:** Unchanged in `kill_group_while_unreaped` ([supervise.rs:172-178](crates/atrium-host/src/supervise.rs#L172-L178)) and `finish_termination` ([supervise.rs:195-215](crates/atrium-host/src/supervise.rs#L195-L215)).

### 2. `unsafe` audit
- **`src/`:** Confirmed zero new `unsafe` blocks in `git diff 564272d..HEAD -- crates/atrium-host/src/supervise.rs`.
- **New test-only `unsafe` blocks in [tests/supervise.rs](crates/atrium-host/tests/supervise.rs):**
  - `group_exists` ([tests/supervise.rs:643](crates/atrium-host/tests/supervise.rs#L643)): **sound**. Checks `raw > 1` (`-raw <= -2`) and uses signal `0`.
  - `helper_leader_leaves_its_group` ([tests/supervise.rs:674](crates/atrium-host/tests/supervise.rs#L674)): **sound**. `setpgid(0, new_pgid)` has no memory-safety preconditions and targets only the helper itself and its own live child's pgid.
  - `ready_then_sigterm` ([tests/supervise.rs:817](crates/atrium-host/tests/supervise.rs#L817)): **sound**. Checks `raw > 1` and signals this test's own unreaped child `sup.identity.pid`.
  - `exited_unreaped` ([tests/supervise.rs:892-906](crates/atrium-host/tests/supervise.rs#L892-L906)): **sound**. `siginfo_t` is zero-initialized POD, `&mut info` outlives `waitid`, `WNOWAIT` leaves the zombie unreaped, and `si_pid` / `si_pid()` is read only when `rc == 0`.
  - `wait_for_zombie_only_group` ([tests/supervise.rs:925](crates/atrium-host/tests/supervise.rs#L925)): **sound**. Checks `raw > 1` and uses signal `0`.
- **Signalling only test-spawned processes:** Verified across all new tests (`helper_leader_leaves_its_group` kills its own `sleep` child; `ready_then_sigterm` signals `sup.identity.pid`). See the minor/nit observation below on the 10 s timeout fallback in `terminate_blocking_kills_a_leader_that_left_its_group`.

### 3. Tests
- **Leave-group tests (`terminate_kills_a_leader_that_left_its_group`, [tests/supervise.rs:730-751](crates/atrium-host/tests/supervise.rs#L730-L751); `terminate_blocking_kills_a_leader_that_left_its_group`, [tests/supervise.rs:754-793](crates/atrium-host/tests/supervise.rs#L754-L793)):**
  - Because `spawn_group` calls `process_group(0)`, the leader cannot `setsid()` (`EPERM`) and `setpgid(0, 0)` would keep it in its own group; joining a short-lived child's group and killing/reaping that child ([tests/supervise.rs:656-687](crates/atrium-host/tests/supervise.rs#L656-L687)) reliably produces a live leader outside an empty `old_pgid` (verified by `wait_until_left` at `:703-727`).
  - **Would fail without the fix:** On `dev`'s code, `kill_group_while_unreaped` gets `ESRCH` on `old_pgid` and never signals `leader_pid` in `new_pgid`, so `child.wait().await` / `reap_leader_blocking` waits on the helper's 60 s sleep and both tests hit their 10 s timeout panic (matching the handoff's reproduction).
- **`SIGTERM`-ignoring leader with grace ([tests/supervise.rs:824-880](crates/atrium-host/tests/supervise.rs#L824-L880)):** Covers both `terminate` and `terminate_blocking` with `SOFT_GRACE = 300 ms`, asserting `elapsed >= SOFT_GRACE`, `elapsed < 5 s`, `killed == true`, `exit_code == None`, and leader/group gone.
- **Zombie-only group ([tests/supervise.rs:937-963](crates/atrium-host/tests/supervise.rs#L937-L963)):** Runs 20 rounds, waits for `exited_unreaped(pid)`, asserts `kill(-pgid, 0)` returns `EPERM` on macOS (proving the `EPERM` branch is exercised) and `0` on Linux, and verifies `terminate_blocking(Duration::ZERO)` returns `killed == false`, `exit_code == Some(3)`, and reaps/empties the group.
- **Timing/load robustness:** The entire 15-test suite finishes in `0.43–0.49 s`; the `5 s` per-test and `10 s` timeout bounds have >10× headroom. (`elapsed >= SOFT_GRACE` holds monotonically because `wait_until_exited_no_reap` / `_sync` sets its deadline inside `terminate*` after `started` is captured and sleeps until `now >= deadline`.)
- **Unverified platform:** Linux-only test lines (`info.si_pid()` at [tests/supervise.rs:904](crates/atrium-host/tests/supervise.rs#L904) and `assert_eq!(rc, 0, ...)` at [tests/supervise.rs:933](crates/atrium-host/tests/supervise.rs#L933)) were checked by reading only; this review ran on macOS.

### 4. Review nits from M1-07b
All three are addressed cleanly:
- `ECHILD` arm ([supervise.rs:422-425](crates/atrium-host/src/supervise.rs#L422-L425)): returns `Err(err)` directly with a comment explaining why `try_wait` cannot recover a cached status.
- Unused `pid` ([supervise.rs:159-162](crates/atrium-host/src/supervise.rs#L159-L162)): replaced `let _ = pid;` with `#[cfg_attr(not(target_os = "macos"), allow(unused_variables))]` on the parameter.
- Doc comment bounds ([supervise.rs:90-105](crates/atrium-host/src/supervise.rs#L90-L105), [supervise.rs:118-136](crates/atrium-host/src/supervise.rs#L118-L136)): both doc comments document the `grace` + leader-reap (unbounded only if held by the kernel, e.g. D state) + `2 s` drain bounds, and `terminate_blocking` documents the non-tokio (`waitpid(-1)`) reap precondition.

### 5. Reported unfixed macOS `EPERM` race
- **Real and reachable:** Yes. In `kill_group_while_unreaped` ([supervise.rs:175-177](crates/atrium-host/src/supervise.rs#L175-L177)), `peek_exited(pid)?` is a single non-blocking `waitid(P_PID, ..., WEXITED | WNOHANG | WNOWAIT)`. In production, when `grace == Duration::ZERO` or `grace` expires in the narrow window while the leader is mid-exit (XNU `killpg1` already returns `EPERM` because no signalable live member remains in `pgid`, before `waitid` can report the child), `peek_exited` returns `Ok(false)` and `terminate` / `terminate_blocking` fail with `Err(EPERM)`, leaving the leader unreaped until `Drop`.
- **Is a short bounded poll on `EPERM` the right fix?** Yes. Because `killpg1` only returns `EPERM` when it found no live signalable member in `pgid` (if a live descendant existed, `kill(-pgid, SIGKILL)` would signal it and return `0`), `EPERM` with `peek_exited(pid) == false` on a `spawn_group` child indicates either a mid-exit leader (which becomes waitable within a very short time) or a genuine permission failure (e.g. a setuid child, which still returns `Err(EPERM)` once the short bound expires). If implemented, keep the poll bounded to a few tens of milliseconds (or give `kill_group_while_unreaped` an async variant for `terminate`) so `terminate` does not block a tokio worker thread on a genuine `EPERM`.

---

## Findings

1. **[nit] Timeout-failure cleanup in `terminate_blocking_kills_a_leader_that_left_its_group` signals `leader_pid` after `rx.recv_timeout`**
   - Where: [crates/atrium-host/tests/supervise.rs:773-779](crates/atrium-host/tests/supervise.rs#L773-L779)
   - Evidence: On `rx.recv_timeout(Duration::from_secs(10))` timeout, `drop(KillPidGuard(leader_pid))` kills `leader_pid` before `worker.join()`. On a genuine hang (such as `dev`'s code), `worker` is blocked in `reap_leader_blocking` and `leader_pid` is guaranteed unreaped. Under severe >10 s scheduling starvation on working code, `worker` could theoretically finish reaping just as `recv_timeout` expires and before `KillPidGuard` drops. Acceptable as a test failure-path guard; sharing an `AtomicBool` cleared right after `reap_leader_blocking` (or only signalling while not finished) would close even that window.

2. **[nit] `exited_unreaped` asserts `rc == 0` on `waitid` without `EINTR` retry**
   - Where: [crates/atrium-host/tests/supervise.rs:894-900](crates/atrium-host/tests/supervise.rs#L894-L900)
   - Evidence: `assert_eq!(rc, 0, ...)` is memory-safe (if `rc != 0`, it panics before reading `info.si_pid`), unlike `peek_exited` ([supervise.rs:473](crates/atrium-host/src/supervise.rs#L473)) which retries `EINTR`. Unlikely to trigger with `WNOHANG`, and harmless if left as-is.

---

## What I ran
- `git status` → clean (`On branch agent/opus/M1-07c-leader-sigkill`, nothing to commit)
- `cargo fmt --all --check` → exit 0
- `cargo clippy -p atrium-host --all-targets -- -D warnings` → exit 0 (`Finished ...` with no warnings)
- `cargo test -p atrium-host --test supervise` × 10 consecutive runs → all 10 exited 0 (`15 passed; 0 failed; 0 ignored`, `0.43 s–0.49 s` per run)
