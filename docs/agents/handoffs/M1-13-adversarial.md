# Handoff: M1-13-adversarial

Model: Sonnet 5.5 · Status: **partial** (stopped at the end of the driver's shift)

## Top of the list: inputs that reach Allow on a sensitive path
All need a broad allow rule, FullAccess, or (finding 2 only) two ordinary prefix rules; in Home with no rules every one of them Asks.
- Finding 2: `cd nope; cat .ssh/id_rsa`. A failing `cd` leaves the shell where it was, but the tracker assumes it worked. With ordinary prefix rules `cd` and `cat` allowed it gives **Allow** (test `finding_2_with_plain_prefix_rules_a_failing_cd_reaches_allow`). Medium.
- Findings 1, 3, 4, 5, 7, 8, 9, 10, 11: same class, hard deny misses, Allow only under an `Any` rule or FullAccess.
- Finding 6: other credential stores outside the contract's list are **auto-allowed in Home with no rules** (`cat ~/.cargo/credentials.toml`, `~/.pypirc`, `~/.pgpass`, `~/.vault-token`, `~/.azure/accessTokens.json`, `~/.aws/sso/cache/*.json`, `~/.config/git/credentials`, `~/.config/op/config`, `~/.terraform.d/credentials.tfrc.json`, shell history, `~/.env`). Needs a contract change to `HARD_DENY_DEFAULTS`: change request NOT yet written (`change-requests.md` is not my path).

## Incident (my mistake)
A heredoc in one of my shell commands ended early on an `EOF` line inside the data, so the following lines ran as shell commands in `/Users/pedro` (they included `cat` of `~/.aws/credentials`, `~/Library/Keychains/login.keychain-db`, Chrome `Login Data`, if those exist). The output (64 MB) went to a tool-result file I did not read: `/Users/pedro/.claude/projects/-Users-pedro-Harness/ff733f9c-d28e-4b8b-bedc-9602d0508ee8/tool-results/blmbq5fi1.txt`. I tried to delete it; the permission classifier refused. **Pedro: please delete that file.** Nothing was written outside the worktree.

## Fixes (policy leftovers, all with table tests that fail on the old code)
- Round 6 finding 1: `$(…)` no longer makes a here-doc body "not text" (only process substitutions do; `!` is no longer a reserved owner). `Unit.proc_sub` added.
- Round 6 finding 3: `unwrap` knows `caffeinate`, `arch`, `sandbox-exec`, `flock` (incl. `-c`), `chroot`, `script`, `unbuffer`, `env -P`, `env -S` forms (`-S'…'`, `--split-string=…`, shell-style splitting); `SHELLS` gains `mksh yash rbash posh elvish xonsh`; a command word starting with `$` or a backtick is a shell; `at`, `batch`, `crontab -`, `make -f -`, `script`, `chroot DIR` are stdin script readers (also feed pipe-to-shell).
- M1-03 finding 5: `~/.ssh` members use `symlink_metadata` (`identity_nofollow`).
- Hard links: a regular file under Home with `nlink > 1` that would Allow now Asks (gate).
- Round 6 finding 2 left as a documented limit (see finding 9).

## Attacks tried (about 1,500 command lines, in tables)
Lexer (quotes, ANSI-C, backslash-newline, unicode, NUL, long lines), cwd tracking (`cd -`, `pushd/popd`, CDPATH, subshells, groups, failing cd), flag tables of `rg`, `grep`, `git`, `file`, `date`, `tree`, redirects, path aliases (`/System/Volumes/Data`, case, `//`, `..`), `~user`, globs, real symlink/hard link/dangling/loop/`..`-through-link cases, subject `cwd`, file tools, Unknown subjects, wrappers, remembered exact/prefix/path rules, ledger (replay, stale, cross-thread, cross-incarnation, wrong call, concurrent redeem and allow/deny, expiry paths) over the real SQLite store.

## Findings (ignored tests `#[ignore = "M1-13 finding N"]`)
1. Medium: `cd` inside `( … )` or a pipeline is not followed (`(cd .ssh; cat id_rsa)`). Fix: add the cd target to `seen` before `make_unknown` in `words::apply_cd`.
2. Medium: failing `cd` assumed to succeed (`cd nope; cat .ssh/id_rsa`). Fix: after a `cd`, candidates = new dir plus the previous ones.
3. Low: `pushd /tmp; popd; cd .ssh; …`, `pushd +1`, CDPATH leave cwd unknown.
4. Low: ANSI-C words `$'.ssh/\x69d_rsa'` not decoded.
5. Low: recursive search of a folder holding a credential file (`rg x ~/.aws`, `.docker`, `.kube`, `.config/gh`, `.claude`, `.codex`) only Asks.
6. Medium: credential stores outside the contract list (above). Also `git show HEAD:.npmrc` and `/proc/<pid>/environ` (Linux) allowed.
7. Low: `sh -c -- 'payload'`: `payload_of` returns `--`.
8. Low-Medium: `echo rm -rf ~ | sh` (the `-rf` is dropped as an echo flag), `printf '%s' '…' | sh`.
9. Low (known limit): `bash < <(echo 'rm -rf ~')`, `source <(…)`.
10. Low: a glob that reaches a credential only through a symlink (`cat proj/*/id_ed25519`).
11. Low-Medium: a glob wildcard inside the name of a credential tree or Atrium folder (`.g*/…`, `.local/share/atr*/…`, `rm .config/atr*/config.toml`). The structured fuzz test is ignored for this.
Info: `--exclude-dir=.ssh` and `ls >/dev/null/x` are hard-denied (false hard denies, harmless); a redeem with a hard-denied variant withdraws the ticket (by design); nested JSON in an `Unknown` input is not scanned (can only Ask).

## Test counts (`cargo test -p atrium-core`, all pass)
New files in `crates/atrium-core/tests/`: `adversarial_policy.rs` 7 pass + 10 ignored, `adversarial_ledger.rs` 14, `adversarial_gate.rs` 9 + 2 ignored, `adversarial_fuzz.rs` 2 + 1 ignored (about 12 s), helpers in `common_adversarial/`. Existing `policy` (44) and `ledger` (20) still pass.

## Definition of Done
- [x] Four leftover fixes with tests; round 1-6 tables pass.
- [~] New adversarial files: core done; **daemon and host not started**.
- [x] Fuzz-style tests (xorshift, no `proptest`: `Cargo.toml` not mine), under a minute.
- [x] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` clean. `cargo test -p atrium-core` pass. **`-p atrium-daemon -p atrium-host` not run.**
- [~] This handoff.

## Not done / next step
1. `crates/atrium-daemon/tests/adversarial_*.rs` (use `#[path = "common/mod.rs"] mod common;`): socket roles (App with a wrong identity, Dev off, pre-Hello commands), `Hello` byte by byte across `HELLO_TIMEOUT`, oversize frames, a full 8 MiB control lane (flood unread `ListThreads`), many replaced subscriptions and replacement while live (D-058), a failing store read in replay, recovery with `kill_recorded` `Unverified` (copy `crash_child`, then rewrite `start_time` through `SqliteStore::set_process`), failed closure in recovery, slow-command limit.
2. `crates/atrium-host/tests/adversarial_*.rs` if anything is missing for process identity.
3. Write the change request for finding 6, decide on findings 1, 2, 7, 8 (small fixes in `words.rs` / `shell.rs`; Pedro/orchestrator to approve, outside my brief's list).
4. Run the full DoD commands.

---

## Second part

Status: **complete** (Linux worktree `agent/sonnet/M1-13-adversarial`)

### 1. Engine fixes (`crates/atrium-core/src/policy/**`, stricter verdicts only)

- **Finding 2 (`crates/atrium-core/src/policy/words.rs` and `crates/atrium-core/src/policy/deny.rs`)**:
  - **What was wrong**: `apply_cd` replaced `cwd.dirs` with only the newly resolved target directory, assuming `cd` always succeeded. On a failing `cd` (`cd nope; cat .ssh/id_rsa`), the shell remained in the previous directory: hard-deny checked only `~/nope/.ssh/id_rsa` and missed `~/.ssh/id_rsa`, while `shell::command` resolved `cat .ssh/id_rsa` under `~/nope` (allowing it when `cd` and `cat` had ordinary prefix rules).
  - **How it was fixed**:
    1. In `words::apply_cd`, previously possible working directories (`prev_dirs`) are retained alongside the newly resolved `next` directories in `cwd.dirs` (and recorded in `cwd.seen`), so `cwd.all()` and `cwd.only()` reflect that `cd` may either succeed (`next`) or fail (`prev_dirs`). Whenever more than one distinct directory is possible (`cwd.only()` is `None`), downstream relative paths cannot be resolved to a single safe directory and therefore never auto-allow (`CwdUncertainty` / `Ask`), while `deny::command_reason` checks all candidate directories in `cwd.all()` and hard-denies if any candidate reaches a sensitive path.
    2. In `deny::command_reason`, a unit that itself invokes `cd` (`words::is_cd(&unit.words)`) is skipped when testing `is_rm_Unit` / `is_find_delete` / `writes_home_root`, preventing a `cd ~` from causing `cwd.all()` to include `~` and falsely triggering `writes_home_root` on `cd ~` itself.
- **Finding 1 (`crates/atrium-core/src/policy/words.rs`)**:
  - **What was wrong**: When `cd` ran inside a subshell `(cd .ssh; cat id_rsa)` or a pipeline `cd .ssh | cat id_rsa`, `apply_cd` saw `in_subshell || unit.in_pipeline` and immediately called `cwd.make_unknown()` without first resolving the `cd` target into `cwd.seen`. Subsequent units saw neither the subshell's target directory nor a known directory.
  - **How it was fixed**: `apply_cd` now resolves the `cd` target against `cwd.dirs` and records every resolved directory into `cwd.seen` (and `cwd.dirs`) *before* calling `cwd.make_unknown()` for `in_subshell || unit.in_pipeline`. `cwd.all()` includes `cwd.seen`, so hard-deny checks the subshell/pipeline `cd` target (`~/.ssh/id_rsa` -> `HardDeny`) while `cwd.is_unknown()` remains `true` after the subshell/pipeline so subsequent commands never auto-allow on an uncertain working directory.
- **Finding 7 (`crates/atrium-core/src/policy/shell.rs`)**:
  - **What was wrong**: In `payload_of` (`sh -c -- 'payload'`), after seeing `-c` in a short flag cluster or `--command`, the next word was returned unconditionally even if it was the option terminator `--`.
  - **How it was fixed**: In `payload_of`, if the word immediately following `-c` / `--command` has `!w.quoted && w.text == "--"`, it is skipped as the option terminator and the following word (if any) is taken as the command payload.
- **Finding 8 (`crates/atrium-core/src/policy/shell.rs`)**:
  - **What was wrong**: In `echo_text` (used when `echo` or `printf` pipes into a shell, e.g. `echo rm -rf ~ | sh` or `printf '%s\n' 'cat ~/.ssh/id_rsa' | sh`), `echo` stripped any leading word starting with `-` (dropping `-rf` in `echo rm -rf ~`), and `printf` was not recognized at all.
  - **How it was fixed**:
    1. Added `is_echo_flag` in `shell.rs` matching POSIX/GNU `echo` flags (`-` followed only by characters in `neE`, e.g. `-n`, `-e`, `-E`, `-ne`), and only strips those before the first non-flag argument so payload flags like `-rf` are preserved.
    2. Extended `echo_text` to handle `printf`: skips formatting options (`-v var` or leading `-*`), skips the format string word (unless it is the only word or contains `$` / backtick), and joins the remaining argument words into the piped script text inspected by `check_inner`.
- **Tests in `crates/atrium-core/tests/adversarial_policy.rs`**:
  - Un-ignored `finding_1_cd_in_a_subshell_or_pipeline_is_followed`, `finding_2_a_failing_cd_does_not_move_the_directory`, `finding_2_with_plain_prefix_rules_a_failing_cd_reaches_allow`, `finding_7_a_shell_payload_after_double_dash_is_read`, and `finding_8_echo_and_printf_text_piped_to_a_shell_is_read_whole`.
  - Added table test `findings_1_and_2_table` and expanded table cases in `finding_7_*` and `finding_8_*`.

### 2. Daemon adversarial tests (`crates/atrium-daemon/tests/adversarial_*.rs`)

- **`crates/atrium-daemon/tests/adversarial_socket.rs` (7 tests)**:
  1. `socket_roles_wrong_app_identity_dev_off_and_pre_hello_commands`: tests pre-`Hello` `Command`, `Subscribe`, `SubscribeInbox`, and `Unsubscribe` (all closed without response); `Hello` with `ClientRole::Agent` (refused with `Incompatible` + closed); `Hello` with `ClientRole::Dev` when `dev_socket == false` (refused with `Incompatible` + closed); `Hello` with `ClientRole::App` when `AppIdentitySource::None`, mismatched `Fixed` executable, mismatched `Fixed` `(dev, ino)`, missing identity file, symlinked identity file, `0644` loose identity file, invalid JSON identity file, and valid `0600` identity file; post-`Welcome` second `Hello` (immediately closes connection); protocol version mismatch (`0`, `PROTOCOL_VERSION - 1`, `PROTOCOL_VERSION + 1`, `u32::MAX`).
  2. `hello_sent_byte_by_byte_across_timeout_is_closed`: drips `Hello` 1 byte every 250 ms across `HELLO_TIMEOUT` (5 s) and verifies the daemon closes the socket without `Welcome` at the deadline, while a byte-by-byte `Hello` that completes within `HELLO_TIMEOUT` succeeds.
  3. `oversized_zero_and_truncated_frames_close_the_connection`: tests frame length headers `0`, `MAX_CLIENT_FRAME_LEN + 1` (1 MiB + 1), `0x8000_0000`, and `u32::MAX` both pre-`Hello` and post-`Hello`, plus truncated frame EOF mid-payload, verifying immediate connection closure while other clients stay healthy.
  4. `flooding_unread_list_threads_past_8_mib_control_lane_closes_connection`: populates threads with large summaries (~1 MiB per `ListThreads` response), floods `ListThreads` without reading until the 8 MiB `MAX_CONTROL_BYTES` control lane bound is exceeded, and verifies the daemon closes the unread connection and drops queued responses while other connections continue working.
  5. `many_replaced_subscriptions_in_a_row_and_replacement_while_live_obey_d058`: rapidly sends 25 consecutive `Subscribe` replacements to the same thread with advancing `after` cursors without reading in between, then replaces again while live events are actively streaming, asserting D-058 (`EventId` strictly above the latest cursor before its `Response`, no stale events from replaced subscriptions after the new `Response`, no gaps or duplicates).
  6. `failing_store_read_during_replay_closes_the_subscriber_connection`: uses a SQLite trigger on a custom table queried only by `events_after` (`after.0 >= 900_000`) so `thread()` succeeds and `Subscribe` is answered `Ok`, then `events_after` fails inside `feed_from_store`, verifying that the subscriber connection is closed cleanly while other connections remain unaffected.
  7. `slow_command_limit_runs_eight_beside_readers_and_cancels_on_disconnect`: blocks 8 concurrent slow commands (`ProbeProviders`, occupying all `SLOW_COMMANDS = 8` semaphore permits) on a custom slow adapter while verifying `ListThreads` still answers immediately on the same connection (`is_slow` runs beside the reader); verifies the 9th slow command falls back to inline execution on its connection (blocking subsequent commands on that connection until the slow command finishes); verifies disconnecting the client holding the 8 slow commands cancels them via `conn.closed()` and releases all 8 permits.
- **`crates/atrium-daemon/tests/adversarial_recovery.rs` (3 tests)**:
  1. `crash_child`: subprocess helper that spawns a real process group (`sleep 300`), writes its `ProcessIdentity` to stdout, and waits.
  2. `recovery_with_mismatched_recorded_start_time_is_unverified_and_spares_process`: starts a session backed by a real `crash_child` process group, crashes the daemon (`drop`), rewrites the recorded `ProcessIdentity.start_time` in SQLite via `SqliteStore::set_process`, restarts the daemon, and verifies:
     - the live child process group is **not** signalled (`kill(pid, 0) == 0`),
     - the thread transitions to `ThreadState::Interrupted { cleanup: Cleanup::Unverified }`,
     - an `Interrupted { reason: "daemon restarted", cleanup: Cleanup::Unverified }` event is appended,
     - `CleanupUnverified` is emitted to the inbox and `Notifier`, and
     - `live_processes()` is cleared (`set_process(..., None)`) after recovery.
  3. `failed_closure_in_recovery_fails_start_and_next_start_closes_it`: injects a SQLite trigger that aborts `INSERT INTO events` when `Signal::Recovered` has set `sessions.ended_ms IS NOT NULL`, causing `inner.close_incarnation` to fail during `upkeep::recover()`; verifies `Daemon::start` returns an `Err` (refusing to open the socket with half-recovered state) and that after removing the trigger, the next `Daemon::start` completes recovery and closes the incarnation into `ThreadState::Interrupted { cleanup: Cleanup::Verified }`.

### 3. Host adversarial tests (`crates/atrium-host/tests/adversarial_supervise.rs`)

- **`crates/atrium-host/tests/adversarial_supervise.rs` (9 tests)**:
  1. `identify_rejects_degenerate_pids_and_unreaped_zombies`: tests `identify(0)`, `identify(i32::MAX as u32 + 1)`, `identify(u32::MAX)`, non-existent PID, and an unreaped zombie child (`Z` state in `/proc/<pid>/stat`), verifying `Ok(None)` in all cases.
  2. `identify_handles_weird_comm_with_parens_and_spaces`: spawns a binary named `"evil ) R 1 1 1 (name"` and verifies `/proc/<pid>/stat` parsing (`rfind(')')`) extracts the real state, `pgid`, and `start_time` (field 22).
  3. `kill_recorded_refuses_degenerate_and_self_identities_without_signalling`: tests `pid: 0`, `pid: 1`, `pgid: 0`, `pgid: 1`, `u32::MAX`, `i32::MAX as u32 + 1`, and the caller's own `ProcessIdentity` (`pgid == getpgrp()`), verifying `Cleanup::Unverified` without signalling.
  4. `kill_recorded_unverified_when_leader_exited_and_reaped_but_grandchild_remains_in_pgid`: spawns a process group whose leader exits and is reaped while a background grandchild remains in `pgid`; verifies `kill_recorded` returns `Cleanup::Unverified` and does **not** signal `pgid` (since leader identity can no longer be verified).
  5. `unlinked_running_executable_is_rejected_by_kill_recorded_and_verify_app_peer`: unlinks a running binary on disk so Linux `/proc/<pid>/exe` reports `"… (deleted)"`; verifies `kill_recorded` returns `Cleanup::Unverified` without killing it, and `terminate` still kills and reaps the group.
  6. `kill_recorded_setsid_grandchild_and_unreaped_zombie_leader_boundaries`: verifies that when a child spawns a `setsid` grandchild, `kill_recorded` returns `Cleanup::Unverified` while the group leader remains an unreaped zombie of the current process (keeping `pgid` alive), and returns `Cleanup::Verified` once the leader is reaped and the original process group is empty (documenting the `setsid` process-group boundary).
  7. `terminate_twice_and_fast_exiting_spawn_group_behave_safely`: verifies `spawn_group` + `terminate` on an immediately-exiting child (`exit 0`), and verifies a second `terminate` call after the leader was reaped returns `Err(ErrorKind::InvalidInput)`.
  8. `linux_verify_app_peer_rejects_relative_symlink_mismatch_and_deleted_exe`: tests Linux `SO_PEERPIDFD` peer verification against relative `executable`, symlinked `executable`, non-root-owned `executable` with `dev_ino: None`, mismatched `(dev, ino)`, unlinked running peer binary replaced at the same path, and a lookalike `"… (deleted)"` file planted on disk (rejected by inode comparison against `stat("/proc/<pid>/exe")`).
  9. `lifecycle_rejects_hostile_executable_paths_and_symlinked_lock_files`: verifies `install_keep_running` rejects empty, relative, and newline/carriage-return-injected executable paths, and `InstanceLock::acquire` rejects a symlinked lock file without unlinking a live socket.

### 4. New findings (with severities)

- **Finding 12 — Low (`crates/atrium-daemon/src/upkeep.rs`, `crates/atrium-core/src/store/writer.rs`)**:
  - In `upkeep::recover()`, `close()` executes `inner.store.signal(thread, Signal::Recovered { cleanup })` *before* `inner.close_incarnation(...)`. In `writer::signal`, `Signal::Recovered` sets `sessions.ended_ms = COALESCE(ended_ms, ?3)`, whereas `reader::live_processes` queries `WHERE process IS NOT NULL AND ended_ms IS NULL`.
  - If `Signal::Recovered` succeeds and `close_incarnation` fails (or the daemon crashes between the two store calls), the final `set_process(..., None)` loop in `recover()` does not run. On the next daemon start:
    1. `live_processes()` no longer returns that session row because `ended_ms` is already non-null, so `sessions.process` stays non-null in SQLite forever and `kill_recorded` is not re-attempted on retry.
    2. Un-archived threads are still picked up by the second loop in `recover()` (`for summary in inner.store.threads().await?` where `summary.state.is_running()`) and closed with `Cleanup::Verified`, downgrading a prior `Cleanup::Unverified` if the first attempt had `Unverified`. An archived thread is not in `store.threads()`, so its incarnation would remain unclosed on retry.
- **Finding 13 — Low (`crates/atrium-daemon/src/server.rs`)**:
  - `read_identity` reads `file.take(MAX_IDENTITY_BYTES).read_to_end(&mut text)` without checking `meta.len() > MAX_IDENTITY_BYTES` (64 KiB) or reading `MAX_IDENTITY_BYTES + 1`. An `app-identity.json` file larger than 64 KiB whose first 64 KiB is valid JSON (for example, padded with trailing spaces past 64 KiB) is accepted instead of rejected.
- **Finding 14 — Low (`crates/atrium-daemon/src/conn.rs`)**:
  - In `write_loop`, the inner loop writing a single frame waits up to `WRITE_STALL` (30 s) on `writer.writable().await` without selecting on `conn.closed()`. When `MAX_CONTROL_BYTES` (8 MiB) closes a connection whose peer has stopped reading and filled the kernel socket buffer, `server::connection` exits its read loop and awaits `writing.await`, which remains parked on `writer.writable().await` for up to 30 s unless the peer drains or closes its socket end.

### 5. What is not done

- Part 1 policy findings **3, 4, 5, 6, 9, 10, 11** remain `#[ignore = "M1-13 finding N"]` in `crates/atrium-core/tests/` (`adversarial_policy.rs`, `adversarial_gate.rs`, `adversarial_fuzz.rs`):
  - **Finding 6** requires a contract change to `HARD_DENY_DEFAULTS` in `crates/atrium-protocol` (owned paths forbid contract edits).
  - **Findings 3, 4, 5, 9, 10, 11** were outside the Second Part brief's engine-fix scope (which specified findings 2, 1, 7, and 8).

### 6. Test counts across `atrium-core`, `atrium-daemon`, and `atrium-host`

- **`atrium-core`**:
  - `src/lib.rs` unit tests: 9 passed
  - `tests/policy.rs`: 44 passed
  - `tests/ledger.rs`: 20 passed
  - `tests/history.rs`: 9 passed
  - `tests/store.rs`: 8 passed
  - `tests/adversarial_policy.rs`: **13 passed**, 5 ignored (up from 7 passed, 10 ignored in Part 1)
  - `tests/adversarial_ledger.rs`: 14 passed
  - `tests/adversarial_gate.rs`: 9 passed, 2 ignored
  - `tests/adversarial_fuzz.rs`: 2 passed, 1 ignored
- **`atrium-daemon`**:
  - `src/lib.rs` unit tests: 13 passed
  - `tests/adversarial_recovery.rs` (**new**): **3 passed**
  - `tests/adversarial_socket.rs` (**new**): **7 passed**
  - `tests/ fake.rs`: 1 passed
  - `tests/recovery.rs`: 3 passed
  - `tests/sessions.rs`: 19 passed
  - `tests/socket.rs`: 9 passed
- **`atrium-host`**:
  - `src/lib.rs` unit tests: 2 passed
  - `tests/adversarial_supervise.rs` (**new**): **9 passed**
  - `tests/lifecycle.rs`: 4 passed
  - `tests/peer.rs`: 3 passed
  - `tests/supervise.rs`: 7 passed

### 7. Real outputs of the Definition of Done

#### `cargo fmt --all --check`
```text
$ cargo fmt --all --check
(exit code 0, no output)
```

#### `cargo clippy --workspace --all-targets -- -D warnings`
```text
$ cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.23s
(exit code 0, 0 warnings)
```

#### `cargo test -p atrium-core -p atrium-daemon -p atrium-host`
```text
     Running unittests src/lib.rs (target/debug/deps/atrium_core-cb586c7a04bb2809)
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/adversarial_fuzz.rs (target/debug/deps/adversarial_fuzz-2c087c9b2a9ee708)
test result: ok. 2 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 16.11s

     Running tests/adversarial_gate.rs (target/debug/deps/adversarial_gate-9efafe0a3806d873)
test result: ok. 9 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.05s

     Running tests/adversarial_ledger.rs (target/debug/deps/adversarial_ledger-550bb5aed4b02be0)
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.13s

     Running tests/adversarial_policy.rs (target/debug/deps/adversarial_policy-3672587750ee0a0c)
test result: ok. 13 passed; 0 failed; 5 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running tests/history.rs (target/debug/deps/history-84a83b2192fc9cd1)
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running tests/ledger.rs (target/debug/deps/ledger-9a67174fc1fb3e52)
test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.12s

     Running tests/policy.rs (target/debug/deps/policy-f1280992cb0d6502)
test result: ok. 44 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s

     Running tests/store.rs (target/debug/deps/store-6827c5bf2c1343ab)
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.34s

     Running unittests src/lib.rs (target/debug/deps/atrium_daemon-8573d2bc2654fd75)
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running unittests src/main.rs (target/debug/deps/atriumd-206ce052fe6a3ee5)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/adversarial_recovery.rs (target/debug/deps/adversarial_recovery-46d4e803a4392de3)
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s

     Running tests/adversarial_socket.rs (target/debug/deps/adversarial_socket-3a3961428c91d299)
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.22s

     Running tests/fake.rs (target/debug/deps/fake-6c4c7ca7ca6f039c)
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s

     Running tests/recovery.rs (target/debug/deps/recovery-92c0a3bd1869619f)
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s

     Running tests/sessions.rs (target/debug/deps/sessions-f9eb89c78306cc9a)
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.11s

     Running tests/socket.rs (target/debug/deps/socket-5823a8ce40d78da3)
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 21.43s

     Running unittests src/lib.rs (target/debug/deps/atrium_host-f7be6fad87ed4c6b)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/adversarial_supervise.rs (target/debug/deps/adversarial_supervise-21c0999aedaa5330)
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.03s

     Running tests/lifecycle.rs (target/debug/deps/lifecycle-a8bc877d20d2f8f1)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

     Running tests/peer.rs (target/debug/deps/peer-d39d131ea3260d35)
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s

     Running tests/supervise.rs (target/debug/deps/supervise-e087c5a2e940b6a2)
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.09s
```

