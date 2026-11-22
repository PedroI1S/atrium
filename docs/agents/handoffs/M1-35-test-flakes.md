# M1-35-test-flakes handoff

- **Branch:** `agent/neon/M1-35-test-flakes`
- **Model:** Gemini 4 Neon (`gemini-4-neon-high`, D-134)
- **Checks run (D-123):** `cargo fmt --all`, `cargo check -p atrium-app --tests`, `cargo check -p atrium-agents --tests`, `cargo check -p atrium-daemon --tests` — all passed with zero errors and zero warnings. `cargo test`, `cargo clippy`, and `pnpm` were not run.

---

## N13: `atrium-app` `upgrade::tests::the_daemon_in_the_lock_file_gets_the_signal`

### Cause and hypothesis check

- **Brief hypothesis 1 (`supervise::identify(child.id())` captures pre-exec image):** Does **not** explain the observed failure (`sleep 60` running its full 60 s and failing at `upgrade.rs:134`). If `identify` at line 128 recorded the test binary's pre-exec executable while `stop_old_daemon` saw `/usr/bin/sleep`, `stop_old_daemon` would return `Err(UpgradeError::NotTheDaemon)` and panic immediately at line 132 (`.unwrap()`), never reaching `child.wait()` (line 133) or the assertion at line 134. However, spawning `sleep` without `pre_exec` on Linux uses `posix_spawn` (`CLONE_VFORK`), which carries a latent pre-exec `/proc/<pid>/exe` window (the same mechanism M1-19 addressed in `supervise::spawn_group`), so closing that window too is worthwhile.
- **Brief hypothesis 2 (signal lands before exec and is lost):** Does **not** hold for default-disposition signals: POSIX/Linux preserves pending `SIG_DFL` signals across `execve`, so a `SIGTERM` or `SIGINT` sent before `execve` still terminates `sleep`.
- **Actual cause (inherited `SIGINT = SIG_IGN` from backgrounded verification jobs):**
  In a non-interactive POSIX shell without job control, asynchronous background jobs (`cmd &`, such as parallel verifications launched by `.orchestration/verify-queue.sh`) start with **`SIGINT` and `SIGQUIT` set to `SIG_IGN`**. `SIG_IGN` is preserved across `fork`/`posix_spawn` and `execve`, and GNU `sleep` does not reset `SIGINT`.
  - Iteration 1 (`Stop::WhenIdle`, `SIGTERM` = 15) succeeds immediately because `SIGTERM` stays `SIG_DFL`.
  - Iteration 2 (`Stop::Now`, `SIGINT` = 2) passes identity verification in `stop_old_daemon` and `pidfd_send_signal` returns `Ok(())` (line 132 succeeds), but the kernel silently drops `SIGINT` because `sleep`'s disposition is `SIG_IGN`. `sleep 60` then runs the full 60 s and exits 0 (`status.signal() == None`), failing `assert_eq!(status.signal(), Some(signal), "{how:?}")` at line 134.
  - This matches every observation in `docs/agents/m1-must-fix.md`: it failed on two unrelated branches at once (M1-33, M1-34) while both verified in parallel, also failed in the 2026-11-16 parallel verifications, and passed 3/3 in 0.00 s when run alone in the foreground (where `SIGINT` is `SIG_DFL`).
  - **Production code is not affected:** `atriumd` installs explicit `SIGTERM` and `SIGINT` handlers via `tokio::signal::unix::signal` before acquiring the instance lock (`crates/atrium-daemon/src/main.rs:132-134`), overriding any inherited `SIG_IGN`, and has completed `execve` long before writing its lock file. `stop_old_daemon` and `supervise::identify` needed no changes.

### Evidence (quoted lines)

`app/src-tauri/src/upgrade.rs:110-136` (before fix):
```rust
    fn sleeper() -> Child {
        Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("sleep starts")
    }
...
    #[test]
    fn the_daemon_in_the_lock_file_gets_the_signal() {
        for (how, signal) in [(Stop::WhenIdle, 15), (Stop::Now, 2)] {
            let mut child = sleeper();
            let identity = supervise::identify(child.id())
                .unwrap()
                .expect("the child runs");
            let (_folder, lock) = lock_for(&identity);
            stop_old_daemon(&lock, child.id(), how).unwrap();
            let status = child.wait().unwrap();
            assert_eq!(status.signal(), Some(signal), "{how:?}");
        }
    }
```

`crates/atrium-daemon/src/main.rs:132-134`:
```rust
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut daemon = Daemon::start(options).await?;
```

### Fix

- **Where:** `app/src-tauri/src/upgrade.rs` (test helper `sleeper` / `spawn_sleeper`).
- Added a `pre_exec` hook in `spawn_sleeper` that resets `SIGINT` and `SIGTERM` to `SIG_DFL` via async-signal-safe `signal(2)` before `execve("sleep")`, matching `atriumd`'s startup guarantee. Using `pre_exec` also forces Rust `std`'s `fork`+`exec` `O_CLOEXEC`-pipe synchronization path, ensuring `spawn()` returns only after `execve` has installed `sleep`'s image (closing the latent Linux `CLONE_VFORK` `/proc/<pid>/exe` window across all four tests in `upgrade::tests`).
- **Commit:** `9d208dc` (`M1-35 (N13): reset SIGINT/SIGTERM to SIG_DFL in upgrade test sleeper`).
- **Regression test:** `upgrade::tests::sleeper_receives_signals_even_when_parent_ignores_them` — in `pre_exec` (isolated to the child, without mutating the test process's global signal handlers), sets `SIGINT` and `SIGTERM` to `SIG_IGN` first and then invokes `reset_stop_signals()`, then verifies `stop_old_daemon` terminates `sleep` with `SIGTERM` (15) and `SIGINT` (2).

---

## N14: `atrium-agents` `claude_adapter::questions_use_the_gates_question_ticket`

### Cause and hypothesis check

- **Brief hypothesis holds directly:** `wait_file` in `claude_adapter.rs` returned as soon as `fs::read_to_string(path)` returned `Ok(text)` — without checking whether the fake provider had finished writing the file.
- `crates/atrium-agents/examples/claude-fake.rs:154` writes `canuse_response.json` in place with `fs::write(dir.join("canuse_response.json"), line)` (same for `hook_response.json`, `grandchild.pid`, and `interrupted`). `fs::write` opens the file with `O_WRONLY | O_CREAT | O_TRUNC` (making a 0-byte file immediately visible) and then writes the payload.
- In `questions_use_the_gates_question_ticket`, right after receiving `RequestResolved`, the test polls `wait_file(&dir.join("canuse_response.json"), 10)` while `claude-fake` is concurrently creating and writing `canuse_response.json`. Polling in the window between `open` and `write` returns `""` (or a truncated prefix), and `serde_json::from_str(&wait_file(...)).unwrap()` panics at `claude_adapter.rs:1088:87`.
- The same reader race also existed at every other `serde_json::from_str(&wait_file(...))` site in `claude_adapter.rs`, in `claude_adapter.rs::responses()` (which called `.map(|l| serde_json::from_str(l).unwrap())` on live-appended `responses.jsonl`), in PID/marker `wait_file` calls in `claude_adapter.rs` (0-byte read), and in `codex_adapter.rs::wait_file` (which checked `!text.trim().is_empty()` but did not wait for a complete newline-terminated line or complete JSON).

### Evidence (quoted lines)

`crates/atrium-agents/examples/claude-fake.rs:153-155`:
```rust
                } else if id == Some("c1") {
                    fs::write(dir.join("canuse_response.json"), line).ok();
                    emit(&assistant_text("done"));
```

`crates/atrium-agents/tests/claude_adapter.rs:201-210` (before fix):
```rust
async fn wait_file(path: &Path, secs: u64) -> String {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Ok(text) = fs::read_to_string(path) {
            return text;
        }
        assert!(Instant::now() < end, "{} never appeared", path.display());
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
```

`crates/atrium-agents/tests/claude_adapter.rs:1087-1088` (before fix):
```rust
    let v: Value =
        serde_json::from_str(&wait_file(&dir.join("canuse_response.json"), 10).await).unwrap();
```

`crates/atrium-agents/tests/claude_adapter.rs:1854-1860` (before fix):
```rust
fn responses(dir: &Path) -> Vec<Value> {
    fs::read_to_string(dir.join("responses.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}
```

`crates/atrium-agents/tests/codex_adapter.rs:169-180` (before fix):
```rust
async fn wait_file(path: &Path, secs: u64) -> String {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Ok(text) = fs::read_to_string(path)
            && !text.trim().is_empty()
        {
            return text;
        }
        assert!(Instant::now() < end, "{} never appeared", path.display());
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
```

### Fix

- **Where:** `crates/atrium-agents/tests/claude_adapter.rs` and `crates/atrium-agents/tests/codex_adapter.rs` (within owned paths; `examples/*-fake.rs` were not modified).
- Added `wait_file_until(path, timeout, parse)` in both files, with typed helpers:
  - `wait_json`: waits until `serde_json::from_str::<Value>(text)` succeeds; replaced all JSON `wait_file` call sites in `claude_adapter.rs`.
  - `wait_pid`: waits until the file contains a valid non-zero `u32` PID (and a trailing `\n` in `codex_adapter.rs`, where `codex-fake.rs` terminates every PID/marker file with `\n`); replaced all PID `wait_file` call sites in both files.
  - `wait_file`: waits for non-empty content in `claude_adapter.rs` and newline-terminated non-empty content in `codex_adapter.rs`.
  - Updated `claude_adapter.rs::responses()` to parse only complete `\n`-terminated JSON lines without panicking on a half-written trailing line.
- **Commit:** `8529e4e` (`M1-35 (N14): wait for complete files/JSON in adapter test helpers`).
- **Regression tests:**
  - `claude_adapter::wait_helpers_ignore_empty_and_half_written_files` — deterministically verifies `wait_json` stays pending on 0-byte and truncated JSON files and resolves once complete JSON is written; `wait_pid` and `wait_file` stay pending on 0-byte files; `wait_response` ignores a half-written trailing line in `responses.jsonl` until completed.
  - `codex_adapter::wait_helpers_ignore_empty_and_half_written_files` — deterministically verifies `wait_json` stays pending on 0-byte and truncated JSON, and `wait_pid` / `wait_file` stay pending on 0-byte and unterminated partial writes until `\n`-terminated content lands.

---

## N15: `atrium-daemon` `M1-28-followups::discover_login_shell_path_handles_noise_failures_and_timeouts`

### Cause and hypothesis check

- **Brief hypothesis 1 (`ETXTBSY` from parallel test threads writing scripts while sibling threads fork) holds:**
  `M1-28-followups.rs` runs five tests concurrently in one process. Every test writes shell scripts with `write_executable` (`std::fs::write` + `set_permissions(0o700)`) while sibling test threads simultaneously spawn subprocesses (`DaemonProcess::spawn` at line 61 and `discover_login_shell_path` at `lib.rs:247`).
  On Linux, `fork()` / `clone(CLONE_VFORK)` copies the process's open file descriptor table (without `CLONE_FILES`), so a sibling thread's newly forked child inherits a writable fd to `noisy-sh` until that child reaches its own `execve` (which closes `O_CLOEXEC` fds). Under heavy workspace load, the sibling child is delayed before `execve`; meanwhile `discover_login_shell_path` executes `cmd.spawn()` on `noisy-sh`, Linux `deny_write_access` sees `inode->i_writecount > 0` and fails `execve` with `ETXTBSY` (`io::ErrorKind::ExecutableFileBusy`), and `lib.rs:247` (`cmd.spawn().ok()?`) silently converts `ETXTBSY` into `None`, failing the assertion at `M1-28-followups.rs:148`.
- **Brief hypothesis 2 (2 s timeout under load):** Unlikely for `noisy-sh` (two `/bin/sh` builtin `printf` invocations, ~1 ms even on loaded machines), whereas `ETXTBSY` fails instantaneously and is specifically amplified under CPU load (longer fork-to-exec window) and when tests run in parallel.

### Evidence (quoted lines)

`crates/atrium-daemon/tests/M1-28-followups.rs:127-151` (before fix):
```rust
fn write_executable(path: &Path, body: &str) {
    std::fs::write(path, body).expect("write script");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).expect("chmod 0700");
}
...
#[test]
fn discover_login_shell_path_handles_noise_failures_and_timeouts() {
    let root = folder();
    let noisy_sh = root.path().join("noisy-sh");
    write_executable(
        &noisy_sh,
        "#!/bin/sh\nprintf 'MOTD banner\\n'\nPATH='/fake/nvm/bin:/fake/asdf/bin'\n/bin/sh -c \"$2\"\nprintf '\\ntrailing\\n'\n",
    );
    let found = discover_login_shell_path(Some(noisy_sh.as_os_str()), Duration::from_secs(2));
    assert_eq!(
        found.as_deref(),
        Some(OsStr::new("/fake/nvm/bin:/fake/asdf/bin"))
    );
```

`crates/atrium-daemon/src/lib.rs:236-251` (before fix):
```rust
pub fn discover_login_shell_path(shell: Option<&OsStr>, timeout: Duration) -> Option<OsString> {
    let shell = shell.filter(|s| !s.is_empty())?;
    let shell_path = Path::new(shell);
    let (mut stdout_rx, stdout_tx) = UnixStream::pair().ok()?;
    let child = {
        let mut cmd = Command::new(shell_path);
        cmd.args(["-ilc", login_shell_path_command(shell_path)])
            .stdin(Stdio::null())
            .stdout(Stdio::from(OwnedFd::from(stdout_tx)))
            .stderr(Stdio::null())
            .process_group(0);
        cmd.spawn().ok()?
    };
    let mut shell_child = LoginShellChild::new(child);

    let deadline = Instant::now() + timeout;
```

### Fix

- **In test (`crates/atrium-daemon/tests/M1-28-followups.rs`):** Added a process-wide `EXEC_WRITE_LOCK: RwLock<()>` taken exclusively by `write_executable` and shared by `DaemonProcess::spawn` and `discover_path` (wrapper over `discover_login_shell_path`), preventing any forked child in the test binary from inheriting a writable fd to any freshly written test script.
- **In code (`crates/atrium-daemon/src/lib.rs`, `discover_login_shell_path`):** Moved `deadline = Instant::now() + timeout` before spawning and extracted `spawn_login_shell(shell_path, deadline)` to handle transient `io::ErrorKind::ExecutableFileBusy` (`ETXTBSY`) with short capped backoff (`1 ms`..`10 ms`) strictly bounded by the caller's `deadline`, while still returning `None` immediately on all other spawn errors (`ENOENT`, `EACCES`, etc.).
- **Commit:** `f19117a` (`M1-35 (N15): prevent and handle ETXTBSY in login-shell PATH discovery`).
- **Regression test:** `M1-28-followups::discover_login_shell_path_handles_etxtbsy_within_timeout` — deterministically holds a writable fd open on `busy-sh` (triggering Linux `ETXTBSY`) and releases it from a helper thread after 50 ms, verifying `discover_login_shell_path` recovers and returns `Some("/fake/busy/bin")`; on Linux (`#[cfg(target_os = "linux")]`), also holds a writable fd for the full duration of an 80 ms timeout call and verifies it returns `None` promptly without exceeding the timeout.

---

## Verification commands for the orchestrator

(All commands are fish- and bash-compatible.)

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
cargo test --workspace --no-fail-fast
```

Per-binary 30× stress loops (run in parallel with a second verification for load; wrapped in `bash -c` so they work in fish too):

```sh
# N13 (also verify with SIGINT=SIG_IGN via backgrounded subshell)
bash -c 'for i in $(seq 1 30); do cargo test -p atrium-app --lib upgrade::tests || exit 1; done'
bash -c 'cargo test -p atrium-app --lib upgrade::tests & wait $!'

# N14
bash -c 'for i in $(seq 1 30); do cargo test -p atrium-agents --test claude_adapter --test codex_adapter || exit 1; done'

# N15
bash -c 'for i in $(seq 1 30); do cargo test -p atrium-daemon --test M1-28-followups || exit 1; done'
```

---

## Round 2

Addressed all 3 findings from `.orchestration/review-input/review-round1.md` (D-120).

- **Checks run (D-123):** `cargo fmt --all`, `cargo check -p atrium-app --tests`, `cargo check -p atrium-agents --tests`, `cargo check -p atrium-daemon --tests` — all passed with zero errors and zero warnings. `cargo test`, `cargo clippy`, and `pnpm` were not run.

### Finding 1: Completed malformed JSONL records silently discarded (`claude_adapter.rs`, `codex_adapter.rs`)

- **Fix:**
  - Updated `responses(dir)` in `crates/atrium-agents/tests/claude_adapter.rs` to slice `raw[..=last_nl]` (ignoring only an unfinished trailing line without `\n`, while including every `\n`-terminated line including blank completed lines) and panic on any completed line that fails `serde_json::from_str` with a diagnostic containing the 1-based line number, file path, parse error, and raw line (`{line:?}`).
  - Added the same `responses(dir)` helper in `crates/atrium-agents/tests/codex_adapter.rs` and switched all 6 `responses.jsonl` readers (including the live polling loop in `saturation_declines_approvals_questions_and_reports_dropped_events`) from `.filter_map(|l| serde_json::from_str(l).ok())` / `.ok().is_some_and(...)` to `responses(&dir)`.
- **Commit:** `2ab435d` (`M1-35 (R2 F1): fail on malformed completed JSONL lines in adapter tests`).
- **Tests:**
  - `claude_adapter::wait_response_fails_on_malformed_completed_jsonl_line` — verifies that `wait_response` panics with a diagnostic showing the bad line and line number on both `"not-json-at-all\n<valid>\n"` and `"\n<valid>\n"`, while still succeeding on `"<valid>\n{\"truncated"` (only the unfinished trailing line without `\n` is ignored).
  - `codex_adapter::responses_fails_on_malformed_completed_jsonl_line` — verifies the same three cases plus an unfinished-only file (`"{\"truncated"` -> empty `Vec`) for `codex_adapter::responses`.
- **How each test fails without the fix:** With the round-1 `.filter_map(|l| serde_json::from_str(l).ok())` implementation (and `raw[..last_nl]` for blank lines), both `"not-json-at-all\n<valid>\n"` and `"\n<valid>\n"` silently discard the malformed line and return the following valid record (`Ok(...)`), failing `.expect_err("malformed completed JSONL line must fail ...")` / `.expect_err("blank completed JSONL line must fail ...")`.

### Finding 2: Retry can launch the login shell after the deadline (`crates/atrium-daemon/src/lib.rs`)

- **Fix:**
  - Extracted `#[doc(hidden)] pub fn spawn_login_shell_with_retry(deadline, now, sleep, spawn)` and routed `spawn_login_shell` / `discover_login_shell_path` through it.
  - Added an explicit `if now() >= deadline { return None; }` check at the top of the retry loop immediately before each spawn attempt (including right after every backoff sleep), so no shell can launch once the deadline has passed—whether the final backoff consumed the remaining budget, the OS overslept past the deadline, or the budget was already zero.
  - Added `#[doc(hidden)] pub fn discover_login_shell_path_with_spawn_hook` (called by `discover_login_shell_path` with a no-op hook) to observe real spawn attempts without changing `discover_login_shell_path`'s public signature.
- **Commit:** `a0c6885` (`M1-35 (R2 F2): check deadline before each login-shell spawn retry`).
- **Test:**
  - `atrium_daemon::tests::spawn_login_shell_with_retry_returns_none_when_deadline_expires_before_retry` — injects a virtual clock and spawn closure to test: (1) final backoff consuming the remaining `3 ms` budget (`1 ms + 2 ms`) where attempt 3 would return `Ok("started-after-deadline")`, asserting `None` and `attempts == 2`; (2) simulated oversleep past a `10 ms` deadline where attempt 2 would succeed, asserting `None` and `attempts == 1`; (3) already-expired deadline (`0 ms`), asserting `None` and `attempts == 0`; and (4) non-`ETXTBSY` (`NotFound`) aborting immediately (`attempts == 1`, no sleep).
- **How the test fails without the fix:** Without the pre-attempt `if now() >= deadline { return None; }` check, subcase 1 loops directly from `sleep(2ms)` (at `t == deadline`) into attempt 3, launching the shell and returning `Some("started-after-deadline")` (`attempts == 3`) instead of `None` (`attempts == 2`); subcase 2 similarly launches attempt 2 after oversleeping past `deadline` and returns `Some("started-after-oversleep")` (`attempts == 2`).

### Finding 3: Deterministic ETXTBSY regression test (`crates/atrium-daemon/tests/M1-28-followups.rs`)

- **Fix:**
  - Replaced the scheduling-dependent `thread::sleep(50 ms)` releaser thread in `M1-28-followups::discover_login_shell_path_handles_etxtbsy_within_timeout` with two deterministic layers:
    1. **Virtual-clock + injected-outcome seam tests** via `spawn_login_shell_with_retry` (all platforms, zero wall-clock sleep): transient `[ETXTBSY, ETXTBSY, Ok]` recovery (asserting `Some`, `3` attempts, sleeps `[1ms, 2ms]`, all attempt times `< deadline`); permanently busy (`20 ms` virtual budget, asserting `None`, `5` attempts = 1 initial + 4 retries, exact backoff schedule `[1ms, 2ms, 4ms, 8ms, 5ms]`, all attempt times `< deadline`); and expiry right before retry (`3 ms` budget, asserting `None` and `2` attempts).
    2. **Real Linux kernel `ETXTBSY` end-to-end tests** (`#[cfg(target_os = "linux")]`) via `discover_login_shell_path_with_spawn_hook`: holds a writable fd on `busy-sh` on the test thread and drops it synchronously **only inside the spawn-attempt hook after observing `Err(ExecutableFileBusy)`**, asserting recovery to `Some("/fake/busy/bin")` with `prior` non-empty and all `Err(ExecutableFileBusy)` followed by `Ok(())`; and for the permanently held writable fd (`200 ms` timeout), asserts `None`, `busy_attempts.len() >= 2` (confirming real retries occurred), all attempts `Err(ExecutableFileBusy)`, and elapsed `< 2 s`.
- **Commit:** `b34fdd8` (`M1-35 (R2 F3): make ETXTBSY regression tests deterministic`).
- **Test:** `M1-28-followups::discover_login_shell_path_handles_etxtbsy_within_timeout`.
- **How each test fails without the fix (old non-retrying `cmd.spawn().ok()?`):**
  - **1a (injected transient busy -> success):** Old code does not retry `ETXTBSY`; attempt 1 receives `Err(ExecutableFileBusy)` and immediately returns `None` (`attempt_times.len() == 1`, `sleeps == []`), failing `assert_eq!(recovered, Some("recovered-shell"))`.
  - **1b (injected permanently busy):** Old code stops after 1 attempt (`attempt_times.len() == 1`, `sleeps == []`), failing `assert_eq!(attempt_times.len(), 5, ...)`.
  - **1c (expiry before retry):** Old non-retrying code stops after 1 attempt (`attempts == 1`, failing `assert_eq!(attempts, 2)`); round-1 retrying code without Finding 2's pre-retry deadline check invokes attempt 3 at `t == deadline` and returns `Some("started-after-deadline")`, failing `assert_eq!(expired, None)`.
  - **2a (real Linux transient `ETXTBSY` with hook-synchronized release):** Because `writer` stays open on the calling thread until `on_spawn_attempt` observes `Err(ExecutableFileBusy)`, attempt 1 deterministically hits kernel `ETXTBSY` regardless of scheduling. Old non-retrying code immediately returns `None` without retrying after `writer` drops, failing `assert_eq!(found.as_deref(), Some(OsStr::new("/fake/busy/bin")))`.
  - **2b (real Linux permanently held writable fd):** Old non-retrying code performs only 1 spawn attempt (`busy_attempts.len() == 1`) before returning `None`, failing `assert!(busy_attempts.len() >= 2, ...)`.

### Additional Round 2 verification commands for the orchestrator

(All commands are fish- and bash-compatible.)

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
cargo test --workspace --no-fail-fast
cargo test -p atrium-daemon --lib spawn_login_shell_with_retry
bash -c 'for i in $(seq 1 30); do cargo test -p atrium-agents --test claude_adapter || exit 1; done'
bash -c 'for i in $(seq 1 30); do cargo test -p atrium-agents --test codex_adapter || exit 1; done'
bash -c 'for i in $(seq 1 30); do cargo test -p atrium-daemon --test M1-28-followups || exit 1; done'
bash -c 'for i in $(seq 1 30); do cargo test -p atrium-app --lib upgrade::tests || exit 1; done'
```
