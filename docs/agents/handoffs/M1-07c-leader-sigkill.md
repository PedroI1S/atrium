# Handoff: M1-07c-leader-sigkill (N8)

Worker: Claude Opus 5.5 · Branch `agent/opus/M1-07c-leader-sigkill` · 2026-11-15

## Done
- **N8 fixed** (`crates/atrium-host/src/supervise.rs`). `terminate` and `terminate_blocking` now call a new `kill_leader_while_unreaped` right after `kill_group_while_unreaped`. It `SIGKILL`s the leader by pid while the leader is still an unreaped child, so its pid can't have been reused. That is the same argument the group kill already relies on.
  - It goes through tokio's `Child::start_kill` → std `Child::kill` → `kill(pid, SIGKILL)` (tokio 1.54.1 `process/unix/mod.rs:173`; std's `send_signal` sends nothing once a status is cached). **No new `unsafe` block in `src/`.**
  - `ESRCH` is ignored, because a zombie leader may answer it on XNU and it is dead anyway. Any other error is returned.
  - `start_kill` clears tokio's `kill_on_drop` on success. That is harmless: the leader has just been `SIGKILL`ed and is reaped next.
  - The macOS `EPERM` zombie handling in `kill_group_while_unreaped` is unchanged.
- **Review nits:**
  - The `ECHILD` arm in `reap_leader_blocking` now returns `Err` directly, with a comment explaining why a retry can't recover.
  - `let _ = pid` is replaced by `#[cfg_attr(not(target_os = "macos"), allow(unused_variables))]` on the parameter.
  - The doc comments on both functions now give the bounds: up to `grace`, then the reap of the killed leader (unbounded only if the kernel holds it, e.g. in D state), then up to 2 s of drain. The `terminate_blocking` doc also says the caller must not reap first, through tokio or otherwise, and that only a tokio reap is detected.

## Tests (`crates/atrium-host/tests/supervise.rs`, 10 → 15 tests, real processes)
- `helper_leader_leaves_its_group` is helper mode only. It is a no-op without `ATRIUM_SUPERVISE_LEAVE_GROUP_HELPER`.
  - With the variable set, the test binary is spawned through `spawn_group` and becomes the leader. It spawns a `sleep` in a new group, `setpgid(0, that_group)`s itself into it, kills and reaps the `sleep`, prints `LEFT <pgid>` and sleeps.
  - **Note:** the brief's `setsid`/`setpgid(0,0)` can't work for a leader. A process-group leader gets `EPERM` from `setsid`, and `setpgid(0,0)` leaves it in its own group. Joining another group is the only way out.
- `terminate_kills_a_leader_that_left_its_group` and `terminate_blocking_kills_a_leader_that_left_its_group`:
  - They first check that the leader is in the new group and the old group is empty.
  - Then they check that the call returns in under 5 s (bounded at 10 s by `tokio::time::timeout` or `recv_timeout`), with `killed == true` and `exit_code == None`, and that the leader, the old group and the new group are all gone.
- `terminate_kills_a_leader_ignoring_sigterm_after_the_grace` and `terminate_blocking_…_after_the_grace`:
  - The leader is `sh` with `trap '' TERM` and a `sleep` loop. The test sends it `SIGTERM` itself, then calls with `grace = 300 ms`.
  - Expected: elapsed ≥ grace, `killed == true`, and the leader and group gone.
- `terminate_blocking_with_only_the_zombie_leader_in_the_group` (20 rounds):
  - The leader is `sh -c 'exit 3'`. The test waits until `waitid(WNOWAIT|WNOHANG)` sees it exited, so it is a waitable zombie.
  - On macOS it asserts that `kill(-pgid, 0)` answers `EPERM`, which proves the `EPERM` branch is exercised. On Linux it asserts the call succeeds.
  - Then `terminate_blocking(ZERO)` → `killed == false`, `exit_code == Some(3)`, group gone.
- Test-only `unsafe` blocks (each with a `// SAFETY:` comment): `group_exists` (signal 0), `setpgid` in the helper, `kill(pid, SIGTERM)` to the test's own unreaped child, `exited_unreaped` (`waitid` + `si_pid` read, same reasoning as `peek_exited`), and one more signal-0 check.
- Only processes the tests spawned are signalled. On the dev-code timeout path, the blocking test kills the leader by pid; this is safe because the leader is still unreaped, since the worker is blocked reaping it. The async test relies on `kill_on_drop`. No helper processes were left after any run (`pgrep -fl helper_leader_leaves`: none).

## How the failure on dev's code was shown
I copied `supervise.rs` to the scratchpad and replaced it with `git show HEAD:crates/atrium-host/src/supervise.rs`, keeping the new tests. Then I ran `cargo test -q -p atrium-host --test supervise left_its_group` and restored the file (the diff was checked before committing). Result:
```
terminate_blocking_kills_a_leader_that_left_its_group --- FAILED
  panicked: terminate_blocking waited for a leader outside its group
terminate_kills_a_leader_that_left_its_group --- FAILED
  panicked: terminate must not wait for a leader outside its group: Elapsed(())
test result: FAILED. 0 passed; 2 failed; ... finished in 10.01s
```
With the fix, both pass in well under a second.

## Found: a pre-existing macOS race in the `EPERM` branch (not fixed: the brief says keep it exactly)
My first version of the zombie test waited only for `kill(-pgid, 0) == EPERM`. It failed intermittently (`Operation not permitted`), **on dev's code too** (2 of 5 runs, then 18 of 25 with a debug print).
- Instrumented, the cause was `group kill err EPERM peek=Ok(false)`. While the leader is **exiting but not yet waitable**, XNU's `killpg1` already answers `EPERM`, but `waitid(WNOHANG|WNOWAIT)` doesn't report the child yet. So `kill_group_while_unreaped` returns `Err(EPERM)`.
- It is reachable in production when `grace` is zero, or ends exactly as the leader exits. `terminate` and `terminate_blocking` then return `Err(EPERM)`, and the leader is left unreaped for `kill_on_drop` or `Drop` to handle.
- Suggested fix, for the orchestrator to decide: on `EPERM` with `peek_exited == false`, poll `peek_exited` for a short bounded time (a few ms; the leader is mid-exit) before failing.
- The committed test waits until the zombie is waitable, which is the state the brief describes. So it tests the branch as designed, not the race.

## DoD outputs
- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: `Finished`, no warnings. The first attempt failed with `No space left on device`: the Mac's disk was full (120 MiB free, shared with other workers). It passed once space came back.
- `cargo test -p atrium-host -p atrium-agents`: all ok.
  - atrium-agents: lib 25, claude_adapter 38, claude_golden 20, claude_normalizer 61, claude_policy_e2e 5, claude_review 28, consistency 11 (+1 ignored).
  - atrium-host: lib 5, lifecycle 1+1+4, peer 3, **supervise 15**.
- 20-run loops (test binaries run directly, one at a time):
  - `claude_adapter` (it drops through `terminate_blocking`): **20/20**.
  - `supervise`: the first loop was **19/20**. The one failure was not captured, because the loop only counted results and the disk was full at the time. I then ran 240 more runs with failures printed: 30 + 60 + 150 runs with 0 failures, then the final DoD loop at **20/20**. I couldn't attribute the single failure. The likeliest cause is the full disk (spawns and pipes during ENOSPC), but I can't prove it.

## Not done / follow-ups
- The macOS `EPERM` mid-exit race above (needs a go to change the `EPERM` handling).
- The review's adapter test gap (drop inside a current-thread runtime, then block it) is outside this brief's paths.
- Linux was not built or run here. The Linux-only lines in the tests, the `info.si_pid()` read and the `rc == 0` assertion in the zombie test, are checked only by reading. The next Linux check should run `cargo test -p atrium-host --test supervise`.
