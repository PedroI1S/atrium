# Handoff: M1-07b-proc-drop-linux

Model: Gemini 4 Hydrogen `high` · Branch: `agent/hydrogen/M1-07b-proc-drop-linux` · Status: **done**

## Cause (proven)

The orchestrator's reading in `docs/agents/m1-must-fix.md` (N7) was **proven** on Linux:

1. **Reproduction before fix:** `cargo test -p atrium-agents --test claude_adapter a_start_dropped_outside_a_runtime_kills_the_process_group -- --exact` hung indefinitely after printing `running 1 test` (had to be cancelled via task kill).
2. **Diagnostic proof:** A temporary diagnostic test (`prove_n7_linux_pidfd_hang`, removed before commit) spawned `sleep 60` via `supervise::spawn_group` on a `current_thread` runtime `rt1`, blocked `rt1`'s thread in `std::thread::spawn(...).join()`, and ran `rt2.block_on(supervise::terminate(&mut child, Duration::ZERO))` on the spawned thread. After 1 second, a watchdog observed:
   ```text
   TERMINATE_THREAD: calling rt2.block_on(terminate)
   WATCHDOG: still blocked after 1s!
   WATCHDOG: peek_exited(39531) = Ok(true)
   WATCHDOG: group_is_empty(39531) = false
   WATCHDOG: /proc/39531/stat = Ok("39531 (sleep) Z 39529 39531 39522 34816 39522 4228108 37 0 0 0 0 0 0 0 16 -4 1 0 1823335 0 0 18446744073709551615 0 0 0 0 0 0 0 1 0 1 0 0 17 5 0 0 0 0 0 0 0 0 0 0 0 0 9\n")
   WATCHDOG: open pidfds in process = ["anon_inode:[pidfd]"]
   WATCHDOG: waitpid(39531) -> rc=39531, status=9, group_is_empty=true
   ```
   - `signal_group` killed the leader with signal 9, leaving it an unreaped zombie (`Z`, `peek_exited = Ok(true)`).
   - `tokio::process::Child` held an open `pidfd` (`anon_inode:[pidfd]`) registered with `rt1`'s epoll reactor.
   - `child.child.wait().await` on `rt2` waited for a waker driven by `rt1`'s reactor, which never polled because `rt1`'s thread was blocked in `.join()`. Even after the watchdog reaped the zombie with `waitpid`, `rt2.block_on(terminate)` remained hung.

## Changes

- **`0a9322c` (`crates/atrium-host/src/supervise.rs`, `crates/atrium-host/tests/supervise.rs`):**
  - Added `pub fn terminate_blocking(child: &mut SupervisedChild, grace: Duration) -> io::Result<Termination>` with no tokio runtime dependency.
  - Preserved `terminate`'s exact sequence and safety invariants via shared helpers (`check_unreaped_leader`, `kill_group_while_unreaped` including the macOS `EPERM` zombie check, `finish_termination`):
    1. Check for early exit without reaping (`wait_until_exited_no_reap_sync` using `peek_exited` / `WNOWAIT` and `std::thread::sleep`).
    2. `SIGKILL` the process group while the leader is still unreaped (`kill_group_while_unreaped`).
    3. Reap the leader via `reap_leader_blocking`: blocks on `libc::waitid(libc::P_PID, raw_pid, &mut info, libc::WEXITED | libc::WNOWAIT)` (retrying `EINTR`), then calls `tokio::process::Child::try_wait()` so `Child` reaps the zombie and transitions to reaped (`child.id().is_none()`), ensuring `kill_on_drop` does not signal or enqueue a freed PID into tokio's orphan queue.
    4. Drain the group with `wait_for_group_drain_sync` and verify `group_is_empty(pgid)`.
  - Left `pub async fn terminate` behaviour unchanged for existing async callers.
  - Added two integration tests in `crates/atrium-host/tests/supervise.rs`:
    - `terminate_blocking_kills_group_with_grandchild_while_spawning_runtime_is_blocked`
    - `terminate_blocking_after_spawning_runtime_dropped_handles_early_exit_leader`
- **`d64327b` (`crates/atrium-agents/src/claude/adapter.rs`):**
  - Updated `Proc::drop` / `terminate_now` to call `supervise::terminate_blocking(&mut child, Duration::ZERO)` with no tokio runtime. Both the off-runtime drop path and the in-runtime `std::thread::spawn(move || terminate_now(child))` path are now completely independent of any tokio runtime (blocked or shutting down).

## Definition of Done

Note: `timeout` is not on the worker command allowlist, so `cargo test` commands were run directly without the `timeout` wrapper; none hung.

### Before / after the fix on the hanging test

- **Before:** `cargo test -p atrium-agents --test claude_adapter a_start_dropped_outside_a_runtime_kills_the_process_group -- --exact` hung indefinitely in `running 1 test`.
- **After:**
  ```text
  running 1 test
  test a_start_dropped_outside_a_runtime_kills_the_process_group ... ok

  test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 37 filtered out; finished in 0.03s
  ```

### `cargo fmt --all --check`
- Exit 0 (no output).

### `cargo clippy --workspace --all-targets -- -D warnings`
- Exit 0 (`Finished dev profile [unoptimized + debuginfo] target(s) in 0.14s` on final post-commit run).

### `cargo test -p atrium-host -p atrium-agents --no-fail-fast -- -q` (after final code commit `d64327b`)
```text
     Running unittests src/lib.rs (target/debug/deps/atrium_agents-202eaf9e134056fc)

running 25 tests
.........................
test result: ok. 25 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/claude_adapter.rs (target/debug/deps/claude_adapter-022a47c2078781da)

running 38 tests
......................................
test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.93s

     Running tests/claude_golden.rs (target/debug/deps/claude_golden-d430272684e3d853)

running 20 tests
....................
test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s

     Running tests/claude_normalizer.rs (target/debug/deps/claude_normalizer-1d8ea59ba27a2eda)

running 60 tests
............................................................
test result: ok. 60 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.25s

     Running tests/claude_policy_e2e.rs (target/debug/deps/claude_policy_e2e-1fe43db54ffdf8fe)

running 5 tests
.....
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.21s

     Running tests/claude_review.rs (target/debug/deps/claude_review-f9752c28a0f5da3f)

running 28 tests
............................
test result: ok. 28 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.57s

     Running tests/consistency.rs (target/debug/deps/consistency-f861784a7b20459c)

running 12 tests
i...........
test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/lib.rs (target/debug/deps/atrium_host-c25466d69074aa10)

running 5 tests
.....
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/lifecycle.rs (target/debug/deps/lifecycle-ffc6b4325440bbf9)

running 4 tests
.
running 1 test
.
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.00s

.
running 1 test
.
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.00s

..
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

     Running tests/peer.rs (target/debug/deps/peer-fffd74db8700af69)

running 3 tests
...
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s

     Running tests/supervise.rs (target/debug/deps/supervise-1210ba76918fee22)

running 10 tests
..........
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.50s
```
(The 1 ignored test in `consistency.rs` is pre-existing: `consistency_all_fixtures`, `M1-07, M1-08: fixtures regenerate for rev 3`.)

### `claude_adapter` 20+ sequential runs (all passed, 38/38 each)

Executed 23 sequential runs of `cargo test -p atrium-agents --test claude_adapter` after commit `d64327b`; all 23 exited 0 (including 4 runs in a 6-run batch whose middle output was truncated in the tool log, plus 19 runs with directly observed `test result:` lines — plus 1 more run inside `--no-fail-fast` = 24 total):

```text
Run 1:  test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.41s
Run 2:  test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.94s
Run 3:  test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.92s
Runs 4-9 (6-run batch exited 0; middle 2 truncated in tool preview):
  Run 4: test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.44s
  Run 8: test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.92s
  Run 9: test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.92s
Run 10: test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.92s
Run 11: test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.93s
Run 12: test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.93s
Run 13: test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.93s
Run 14: test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.93s
Run 15: test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.92s
Run 16: test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.92s
Run 17: test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.93s
Run 18: test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.93s
Run 19: test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.42s
Run 20: test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.93s
Run 21: test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.95s
Run 22: test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.94s
Run 23: test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.92s
```

## What the reviewer should check

1. **`reap_leader_blocking` in `crates/atrium-host/src/supervise.rs`:** Confirm `waitid(P_PID, raw_pid, &mut info, WEXITED | WNOWAIT)` followed by `tokio::process::Child::try_wait()` guarantees `Child` marks itself reaped (`child.id().is_none()`) before `SupervisedChild` drops, preventing `kill_on_drop` or tokio's orphan queue from touching a freed PID, while never reaping the leader before `kill_group_while_unreaped`.
2. **macOS parity:** This task ran on Linux; verify `terminate_blocking` (including `waitid(..., WEXITED | WNOWAIT)` and the `#[cfg(target_os = "macos")]` `EPERM` zombie branch in `kill_group_while_unreaped`) passes on macOS.
3. **Unchanged async `terminate`:** Verify `terminate`'s observable behaviour for existing async callers is unchanged.
