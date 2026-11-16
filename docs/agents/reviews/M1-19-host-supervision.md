# Review: M1-19-host-supervision (5c2aea2, ce8c671)

Reviewer: Claude Opus 5.5, read-only (D-103, D-053 audit). Author: Gemini 4 Hydrogen `high`. Saved by the orchestrator from the reviewer's report. The orchestrator applied finding 2 in `670e486`.

## Verdict: **approve with changes**

The N10 and N6 changes are correct: unknown stays `Unverified`, and no group is killed without a pinned or verified identity. The N3 root-cause-2 (`CLONE_VFORK`) fix is sound and fails closed. No blockers. The changes asked for are doc and test-evidence gaps, plus one optional, cheap and safe improvement for N3 root cause 1.

## Findings

1. **N10: correct (no defect).** `supervise.rs:48`, `167-223`.
   - **The bound:** `MACOS_EPERM_EXIT_POLL = 50 ms`, macOS only, bounded backoff with a final `peek_exited`.
   - **It can't hide a real `EPERM`:**
     - XNU `killpg1` returns `EPERM` only if no member was signalled;
     - a setuid member that survives still fails at `finish_termination`'s `group_is_empty` (`ESRCH`) check;
     - the pgid always comes from `spawn_group`.
   - **No reused pgid can be killed:** the poll never reaps, the leader stays an unreaped child, and the branch sends no new signal.
   - **No runtime thread is blocked:** the async path uses `tokio::time::sleep`.
   - **Evidence:** the base code with the new tests failed **25/25**. The fix passed 40/40 serially, 40/40 in the full suite, and 40/40 under 2× CPU load.
2. **Minor: the doc bounds weren't updated** (`supervise.rs:105-107`, `135-138`). They should add up to 50 ms for the macOS `EPERM` poll. *Fixed by the orchestrator, `670e486`.*
3. **N6: correct, but the Linux path has no test that runs here (minor).** `supervise.rs:856-881`, `902-927`.
   - **Semantics:** `PF_EXITING && num_threads <= 1` is the only case where `ENOENT` on `exe` means "going away". With other threads alive the process lives, so `Unverifiable` is right.
   - **The task-folder fallback can't misattribute:** threads share the `mm`, `/proc/<pid>/task/<tid>` is scoped to the thread group, and the start-time and pgid checks still apply.
   - **Behaviour change, failing closed:** a multithreaded provider caught mid-`exit_group` now gives `Unverifiable` instead of `NoProcess`.
   - **The table:** the new rows kill both mutations the M1-14a round-2 review named.
   - **Nits:** add a row (`first.num_threads = 2`, `after` `S|PF_EXITING` with 1 thread, `ENOENT` → `NoProcess`), and an exiting leader whose `exe` was read from a worker (`Ok` → `Identity`).
   - **Linux-only:** `read_linux_exe` is untested here; it goes to rodrigo.
4. **N3 root cause 2 (vfork): sound (minor: weak test evidence).** `supervise.rs:524-566`, `569-580`.
   - **The diagnosis matches the kernel:** `complete_vfork_done` runs before `tsk->mm = mm`. Only glibc `posix_spawn` (`CLONE_VFORK`) races.
   - **Edge cases:**
     - provider binary == parent binary: no poll, and the identity is correct;
     - exec failure: `posix_spawn` errors;
     - a timeout records the same wrong identity as today, which `kill_recorded` treats fail-closed.
   - **Blocking:** worst case a 50 ms `thread::sleep` inside the race window only.
   - **The test** checks only the pure predicate; a deterministic Linux test is impractical.
5. **Minor: the N3 run counts prove nothing about the fix.** The vfork fix is Linux-only, so the 30/30 runs on macOS show only the base rate. N3 must be measured on Linux with 100 or more runs.
6. **`unsafe` (D-053): no change in `src`.** The one new test-only block (`libc::kill(-raw, 0)`, `raw > 1`, signal 0, macOS) is sound and mirrors `wait_for_zombie_only_group`.
7. **Nit:** the test helper busy-spins with `yield_now` for up to 5 s.

## N3 root cause 1: recommendation

The reviewer agrees that both suggested fixes would weaken verification:
- **"A group of only zombies counts as empty":** XNU's `EPERM` is ambiguous, and a Linux `/proc` scan isn't atomic.
- **Waiting on the leader's exit** (kqueue or pidfd) doesn't empty the group.

**Recommendation: keep it as a known fail-safe limit.** No contract §10 change, and the three tests that expect `Unverified` stay. Then:
1. **Measure on Linux first** (rodrigo, 100 or more runs of both N3 tests). M1-14a attributed the Linux failure to the vfork race, which is now fixed.
2. **Optional, cheap and safe:** in `kill_recorded`'s `Ok(None)` branch (`supervise.rs:294-300`), call `wait_for_group_drain_sync(recorded.pgid, GROUP_DRAIN_TIMEOUT)` before the single `group_is_empty` check. It sends no signal and still needs `ESRCH`.
3. **If Linux still flakes,** give `kill_recorded` its own longer drain (about 5 s; it already runs on `spawn_blocking`), and log whether the leader is still `Z` when the drain times out.

## What the reviewer ran
All in a scratch clone at `ce8c671`, deleted afterwards:
- `cargo fmt --all --check`: exit 0.
- Workspace clippy with `-D warnings`: exit 0.
- `cargo test -p atrium-host -p atrium-daemon --no-fail-fast`: all pass.
- **N10:** 40/40 runs of the fix; 0/25 on the base (the tests catch the bug).
- **N3 on macOS:** 30/30 each. This says nothing about the Linux fix.
- Nothing was run on Linux.
