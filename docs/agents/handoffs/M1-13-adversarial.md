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

### 1. Engine fixes (`crates/atrium-core/src/policy/**`, commit `efd29fc`)

Rewritten in the third part (review finding 8): the earlier text described functions and fields that are not in the code (`cwd.dirs`, `cwd.only()`, `is_cd`, `is_echo_flag`, `echo_text`, `CwdUncertainty`). This is what `efd29fc` actually changed.

- **`words.rs`** (findings 1 and 2):
  - `CwdState::Known` holds a `Vec` of candidate directories instead of one.
  - `apply_cd` resolved a relative target against the current candidates, the start and the previous candidates, then dropped some results with a filter for `..`.
  - Every target went into `seen`. After any literal `cd`, the state became `Known(seen)`: every directory named so far, not only the target.
  - A grouped `cd` (subshell or pipeline) added its targets to `seen` and then made the directory unknown.
  - `walk` also resolved words starting with `..` (with the `..` stripped) against the start and home.
- **`deny.rs`**: one line, the fallback for a missing `unit_cwd` (`Known(vec![start])`). Nothing read `cwd_unknown`, so an unknown or multi-candidate directory did not make a prefix rule ask. That is why review finding 2 still reached Allow with the `cd` and `cat` prefix rules.
- **`shell.rs`** (findings 7 and 8):
  - `payload_of` skipped an unquoted `--` after `-c`, and handled `--command`, `--command=` and `eval --`.
  - `echo` flag stripping was limited to words made only of `n`, `e` and `E`, so `echo rm -rf ~` keeps `-rf`.
  - `echo_printf_texts` and a basic `format_printf` read the text `echo` and `printf` write into a shell. This handled `%s` and simple escapes only (review finding 7).
- **Tests** (`crates/atrium-core/tests/adversarial_policy.rs`): un-ignored the part-1 finding 1, 2, 7 and 8 tests and added `findings_1_and_2_table`.

The third part replaces the `words.rs` walk (see "Third part: fixes" at the end; it uses the review's numbering).

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


## Third part: tables (Claude Sonnet 5.5, step 5's tables only)

Two new files, no `src/**` touched, nothing ignored. The fixer needs them red.

**Sizes.**
- `crates/atrium-core/tests/adversarial_regression.rs`: 21 tests, about 117 engine lines (each run under the review's four settings: home, prefix rules `cd cat pushd popd ls`, one `Any` rule, FullAccess) and about 25 gate checks on a temp folder with a fake HOME (home, any, full). Expected verdicts are the ones the review says are required.
- `crates/atrium-core/tests/adversarial_ordinary.rs`: 5 tests, 107 lines, each with its verdict in the four settings: finding 6's inputs and their `;`/newline forms, finding 12's nested `cd`s, finding 10's `$PAGER`/`$CLIP` pipes, "tried and held" ordinary lines, here-docs of text, and 43 everyday Home lines (`ls`, `git status`, `cat README.md`, `cargo build`, `rm -rf target`, `cat ~/.zshrc` and so on).

**How dev's verdicts were established (run, not guessed).** I exported `dev`'s `crates/atrium-core/src` (`git archive dev`) over a copy of this branch in a scratch folder (its own target dir), and ran a scratch dump test that prints each line's verdict in the four settings, on dev and on the branch. The ordinary table's rows are dev's output, pasted by script from the JSON, so no hostile text went through a shell. The same two binaries were then run on dev: the ordinary table passes on dev except one test (below), and the regression table's tests for findings 1, 4, 5 (here-docs) and 12 pass on dev, which confirms those four are regressions of the branch.

**One deliberate difference from dev.** `ordinary_heredoc_text_with_a_substitution_is_not_a_false_hard_deny` (round 6 finding 1: `cat > notes-$(date).md <<EOF … sudo make install`) expects the branch's verdict (Ask, Allow with Any/Full), because dev hard-denies it and part 1 correctly fixed that. It passes on the branch and fails on dev.

**Failing on the branch now (19 of the 26 tests; they cover findings 1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 12, 13 and 14).**
- Ordinary table, 3 tests: `ordinary_lines_after_a_working_cd` (finding 6: HardDeny for dot-file reads, glob deletes, `find -delete`, `..` after a working `cd`; dev Ask or Allow), `ordinary_nested_cds` (finding 12: `Deny("too large to check")`), `ordinary_pipes_variables_and_heredocs` (finding 10: `| $PAGER` hard-denied as `pipe-to-shell`, `| $CLIP` as `privilege-escalation`; dev Ask).
- Regression table, 16 tests: `finding_1_a_key_symlinked_into_ssh_is_still_a_credential`, `finding_1_a_folder_of_keys_linked_into_ssh` (Allow; see the tension below), `finding_2_failing_cds_then_a_credential`, `finding_2_failing_cds_joined_by_or` (Ask in home, Allow with rules, Any and Full; required HardDeny), `finding_3_pushd_popd_and_cd_dash_leave_the_cwd_unknown` (Allow with rules, Any, Full; required not Allow), `finding_3_popd_back_into_a_credential_folder_is_a_hard_deny`, `finding_4_a_cd_through_a_symlink_keeps_the_write_check` (Ask in home, Allow with Any/Full), `finding_5_heredoc_to_a_shell_named_with_a_substitution`, `finding_5_payload_for_a_shell_named_with_a_substitution`, `finding_7_text_piped_to_a_shell_is_code_however_it_is_spelled`, `finding_9_wrapper_long_options_and_the_commands_own_dash_c`, `finding_12_seven_nested_cds_are_not_refused`, `finding_13_shell_options_that_take_a_value`, `finding_14_crontab_bodies_and_other_stdin_readers`, and two for finding 11 (`…recursive_search_over_a_hard_linked_credential`, `…hard_link_to_a_credential_kept_outside_home`: stricter than dev, which allows them too).
- Passing on the branch (guards, must stay green): `finding_4_control_the_same_writes_without_a_cd`, `held_one_failing_cd_forms`, `held_deletes_after_a_failing_cd`, `held_sh_dash_c_forms`, `held_echo_and_printf_into_a_shell` (the review's "tried and held" inputs; the exact lines are my reconstruction from the families the review names, and every one held on the branch).
- Not covered here: finding 8 (a document), finding 10 as a required verdict (the table pins dev's Ask), the review's "open findings" list (3, 4, 5, 9, 10, 11, 12, 13, 14 of the earlier numbering: recursive search of a credential folder, ANSI-C quotes, CR-12 stores, the daemon and host ones).

**Judgement calls the fixer should check.**
- Finding 2's "the same joined by `&&`" is left out: after a failing `cd` the rest of an `&&` chain doesn't run, so a hard deny there could be a false one. `||` and `;`/newline forms are in. The first `||` line I tried (`cd .config || cd n1 || cd n2 || cd gh; cat hosts.yml`) can't reach `~/.config/gh`, so I removed it.
- Finding 3 asks for "not Allow" (the review's minimum, §9); one separate test pins the hard deny for the `popd` line.
- Finding 1's linked folder (`~/.ssh/work -> ~/vault/work` holding `id_work`) is in its own test: dev denied it, but M1-03 finding 5 wants a link to an ordinary folder to stay ordinary. A fix that follows only file links turns this test green only if it also decides a folder with a key in it; the review says "decide separately".
- Findings 7, 9, 13 and 14 use "not Allow" in all four settings (the review gives no exact id); the fixer may tighten them.
- Not established: the review's "docker exec -i $(docker ps -q) sh" case (round 6 finding 1 as asked, Ask) is not in a table, and I did not run daemon, host or socket tests.


## Third part: fixes (Claude Opus 5.5, review items 1 to 14)

Status: **complete** for every item except the limits listed under "What is left". Fixes are in `crates/atrium-core/src/policy/**` only, and every change is stricter, apart from removing false hard denies back to dev's verdict. Test files changed: the two tables, `adversarial_policy.rs`, `crates/atrium-daemon/tests/adversarial_socket.rs` (flood test) and `crates/atrium-host/tests/adversarial_supervise.rs` (macOS only). Numbers below are the review's.

### Fixes

1. **[blocker] Key symlinked into `~/.ssh`** (`gate.rs`).
   - `identity_nofollow` is gone. `ssh_members` follows a link to a file, so the key's real inode maps onto `~/.ssh/<name>` again.
   - For a folder linked into `~/.ssh`, its files (depth 1) become members but the folder does not. So `ls proj` stays allowed, as M1-03 finding 5 wants, and `cat proj/a.txt` is denied.
   - Tests: `finding_1_a_key_symlinked_into_ssh_is_still_a_credential`, `finding_1_a_folder_of_keys_linked_into_ssh`, and `leftover_symlink_in_ssh_to_an_ordinary_folder` (updated, see the table corrections).
2. **[blocker] A failing `cd` reaches Allow with prefix rules** (`shell.rs`, `words.rs`, `deny.rs`, `engine.rs`).
   - The lexer records how each unit joins the next (`Join::{Seq, And, Or}`).
   - `walk` was rewritten:
     - After `cd X &&`, only the target is a candidate for the rest of the chain.
     - After `;`, `||` or a newline, the target and every earlier candidate are candidates (the `cd` may have failed).
     - At the end of an `&&` chain, the directories where the chain could have stopped come back.
     - `cd X || exit` (or `return`) leaves only the target.
   - A relative target resolves against every current candidate.
   - `Analysis.cwd_uncertain` is true when the cwd is unknown or a unit has more than one candidate. When it is true, a prefix, exact or path-glob rule can't cover the line (`allow_covers`), and Home asks.
   - Candidates are never compared by value, so the gate's and the engine's lists keep the same shape.
   - No candidate directories are listed: an uncertain cwd asks.
   - Tests: `finding_2_failing_cds_then_a_credential`, `finding_2_failing_cds_joined_by_or`, `finding_2_and_chains_follow_only_success`, `held_one_failing_cd_forms`, `held_deletes_after_a_failing_cd`, `findings_1_and_2_table`, `finding_2_*` in `adversarial_policy.rs`.
3. **[blocker] `pushd`/`popd`/`cd -`** (`words.rs`).
   - These make the cwd unknown ("Lost"), and so does a `cd` to `$VAR`, a glob, `~+`/`~-`, or one inside a group.
   - The candidates are then home, the start, and every directory the line named (at most 64), so a later relative `cd gh` is resolved against all of them.
   - 2's `cwd_uncertain` makes the line ask.
   - Tests: `finding_3_pushd_popd_and_cd_dash_leave_the_cwd_unknown`, `finding_3_popd_back_into_a_credential_folder_is_a_hard_deny`, `finding_3_popd_pushd_and_cdpath_leave_the_directory_unknown`.
4. **[blocker] A `cd` through a symlink un-aligns the gate** (`words.rs`, `deny.rs`). Both fixes from the review are in:
   - No value dedupe: both walks build lists of the same shape.
   - The belt: when the lists still don't line up, each gate path is checked with the strongest role of any use (Write, then MaybeWrite, then Read).
   - Tests: `finding_4_a_cd_through_a_symlink_keeps_the_write_check`, `finding_4_control_the_same_writes_without_a_cd`.
   - The flaky flood test was fixed too (see "Flaky flood test").
5. **[major] Here-doc to a shell named with a substitution** (`shell.rs`).
   - `maybe_shell` is a command word containing `$` or a backtick anywhere. Such a word counts as a possible shell for here-docs and `-c` payloads.
   - A here-doc owned by a command runner (`ssh`, `nsenter`, `docker`, …) whose argv has a substitution is read as code.
   - `$(…)` output that is run as code is read through `output_texts`: the output of a `$(…)` in a payload or in the command word, and `<(…)` run by a shell, `source` or `.`.
   - Tests: `finding_5_heredoc_to_a_shell_named_with_a_substitution`, `finding_5_payload_for_a_shell_named_with_a_substitution`.
6. **[major] Ordinary lines hard-denied after a working `cd`** (`words.rs`).
   - Fixed by 2's join tracking (after `cd X &&` only the target counts) and by deleting the `..` special case in `walk`.
   - The `;` and newline forms still hard-deny, because the `cd` may have failed and the old directory then holds the file. See the table corrections.
   - Tests: `ordinary_lines_after_a_working_cd`, `after_a_cd_that_may_fail_the_old_directory_counts`.
7. **[major] `echo`/`printf` decoding** (`shell.rs`).
   - The decoders are `unescape_one`, `unescape_to` and `format_printf`.
     - `echo`: every escape is decoded (`\xHH`, `\NNN`/`\0NNN`, `\e`, `\c`, …), in both octal styles, and the raw text is kept too.
     - `printf`: width, precision, `*`, `%c` and `%b` are applied; a conversion with no argument prints nothing, and `\c` stops the output. Padding is capped at 256.
     - `yes`: its arguments, or `y`.
   - A pipe into `( … )` or `{ …; }` is followed.
   - Text from `$(…)` that is the payload of `sh -c` or `eval` is read the same way.
   - Tests: `finding_7_text_piped_to_a_shell_is_code_however_it_is_spelled`, `held_echo_and_printf_into_a_shell`, `finding_8_echo_and_printf_text_piped_to_a_shell_is_read_whole`.
8. **[major] Handoff "Second part" described code that isn't there.** Section 1 of the Second part was rewritten from `git show efd29fc`.
9. **[minor] Wrapper long options and the command's own `-c`** (`shell.rs`, `unwrap`).
   - Value-taking options are skipped:
     - `time -f/--format/-o/--output`
     - `arch -arch`
     - `nice --adjustment`
     - `ionice`, `stdbuf` and `xargs` long options
     - `timeout --signal/--kill-after`
     - `env -a/--argv0`
   - `flock`/`script` take `-c` only among the wrapper's own options.
   - `env -S` text is a payload, `env -C DIR CMD` is read as `cd 'DIR' && CMD`, and an `S` inside an `env` cluster (`-iS`) counts.
   - Test: `finding_9_wrapper_long_options_and_the_commands_own_dash_c`.
10. **[minor] `| $PAGER` and `| $CLIP` hard-denied** (`shell.rs`, `deny.rs`).
    - `is_shell` means a known shell name, `$SHELL`, `${SHELL}`, `$BASH` or `$0`; it is used for pipes. `maybe_shell` (any `$`) stays for here-docs and `-c`.
    - A non-interpreter `maybe_shell` word reads stdin as code only with `-s` or `-`.
    - `| $PAGER` and `| $CLIP` are back to dev's Ask, and `| $SHELL` is still a hard deny.
    - Test: `ordinary_pipes_variables_and_heredocs`.
11. **[minor] Hard links reached by a recursive search** (`gate.rs`, `reaches_multiply_linked`). Three cases are checked:
    - A named file anywhere, not only under Home.
    - The folder a glob names, at depth 1.
    - The roots of a recursive reader (`rg`, `ag`, `ack`, `grep -r/-R/--recursive`, the Grep tool's recursive search). These get a bounded walk: depth 8, at most 4096 entries, skipping `node_modules` and `.git`.

    A multiply-linked file found this way asks.
    - Tests: `finding_11_a_recursive_search_over_a_hard_linked_credential`, `finding_11_a_hard_link_to_a_credential_kept_outside_home`.
12. **[minor] Seven nested `cd`s refused.**
    - `MAX_CANDIDATES` is 32.
    - With too many candidates the cwd becomes unknown and the line asks; it is no longer an overflow `Deny`.
    - With `&&`, seven `cd`s keep one candidate.
    - Tests: `finding_12_seven_nested_cds_are_not_refused`, `ordinary_nested_cds`.
13. **[minor] Shell options that take a value** (`shell.rs`, `payloads_of`).
    - The values of `--rcfile`, `--init-file`, `--emulate`, `-o` and `-O` are skipped.
    - fish `-C`/`--init-command` (and `--init-command=`) are payloads.
    - Test: `finding_13_shell_options_that_take_a_value`.
14. **[minor] stdin readers** (`shell.rs`).
    - `crontab -` (also `/dev/stdin` and `/dev/fd/0`): each line loses its five schedule fields or its `@word` before it is lexed. This applies to here-docs and to text piped into `crontab`.
    - `make` reads stdin when given a flag cluster with `f` (`-sf -`, `-f/dev/stdin`, `--file=`).
    - `chroot` skips the `--userspec`/`--groups` values.
    - `at`/`batch` always read stdin.
    - Test: `finding_14_crontab_bodies_and_other_stdin_readers`.

**Flaky flood test** (`adversarial_socket.rs`, `flooding_unread_list_threads_past_8_mib_control_lane_closes_connection`).
- It is now deterministic, in two phases:
  1. About 6 MiB of `ListThreads` and a `RenameThread` marker are sent. The test waits until the daemon has applied the marker.
  2. `LIMIT/size + 2` more requests are sent.
- The client then asserts it never receives every answer, and that the connection closes.
- `crates/atrium-daemon/src/**` is untouched.

**Host test on macOS** (`adversarial_supervise.rs`).
- The Linux-only imports are behind `#[cfg(target_os = "linux")]`, which fixes clippy's unused-import errors on macOS.
- `kill_recorded_setsid_grandchild_and_unreaped_zombie_leader_boundaries` is ignored on non-Linux (`cfg_attr`):
  - On macOS, `/usr/bin/python3` is a stub that execs another Python, so the recorded identity no longer matches.
  - `kill_recorded` then refuses (Unverified) without signalling, and the leader is never a zombie.
  - The test then waited 30 s, and the grandchild had already exited by the time it was checked. Its premise is Linux-only. Part 2 wrote it on Linux, and it was never run on macOS before.

### Table corrections (expected verdicts changed, with the reason)

Settings are [home, prefix rules, Any rule, FullAccess]. CR = `credential-read`, RD = `recursive-delete-outside-cwd`.

- `adversarial_regression.rs`
  - **Finding 2, `&&` lines** (new test `finding_2_and_chains_follow_only_success`):
    - `cd X && …` lines get [Ask, Allow, Allow, Allow]: the rest of an `&&` chain only runs if the `cd` worked, so a hard deny would be false (review finding 6).
    - The `cd -` chain, `cd proj && ls; cat .npmrc` and `false && cd proj; cat .npmrc` get Hard(CR): after `;` the old directory counts.
    - `cd build || exit 1; rm -rf *` and `cd build || exit; cat .npmrc` get [Ask, Ask, Allow, Allow]: `|| exit` narrows to the target.
  - **Finding 3**: tightened from "not Allow" to exact verdicts.
    - `cd -; cat id_rsa` and `cd /tmp; cd -; cat id_rsa` get [Ask, Ask, Allow, Allow]. `$OLDPWD` isn't named on the line, so no hard deny is provable; it asks. Any and Full allow, the same as dev's `cd $X`.
    - The `pushd`/`popd` lines get Hard(CR) everywhere.
  - **Finding 5**: added `docker exec -i $(docker ps -q) sh <<EOF …` as Hard(RD). That is dev's verdict, and the brief wants the branch at least as strict as dev on every named input. The payload rows and `$(echo rm -rf ~)` were tightened to Hard(RD).
  - **Findings 7, 9, 13 and 14**: tightened from "not Allow" to exact ids:
    - RD in general;
    - CR for the `.aws` lines and the `env -S`/`env -C` rows;
    - `privilege-escalation` for the `flock`/`script` `sudo` rows;
    - `force-push-protected` for `flock … git push --force`.

    Added `| { sh; }`.
- `adversarial_ordinary.rs`
  - The `;` and newline rows were removed from `ordinary_lines_after_a_working_cd`. They moved to the new test `after_a_cd_that_may_fail_the_old_directory_counts`:
    - `cd proj; cat .npmrc` and `cd proj` + newline + `cat .npmrc` get Hard(CR) in all four settings. If the `cd` fails, the shell reads `~/.npmrc`; this is the same class as review finding 2, so the hard deny is right, not a false one.
    - `cd build; rm -rf *` gets Hard(RD) in all four settings, for the same reason (`~/*`).
    - `cd proj; ls`, `cd proj; cat README.md`, `cd build || exit 1; rm -rf *` and `(cd sub && make)` get [Ask, Ask, Allow, Allow]: the cwd is uncertain, so prefix rules ask.
  - `cd a; cd b; …; cd g; ls` with prefix rules: Allow became Ask (uncertain cwd).
  - Added `tee notes-$(date).md <<EOF …` as [Ask, Ask, Allow, Allow], a guard for item 5's runner rule. The rule first fired on any substituted argv word; it is now limited to command runners.
- `adversarial_policy.rs`
  - `leftover_symlink_in_ssh_to_an_ordinary_folder`: `ls proj` and `ls proj/sub` stay allowed; `cat proj/a.txt`, `cat proj/sub/b.txt` and the Read tool on `proj/sub/b.txt` are now denied (item 1: files in a folder linked into `~/.ssh` are credentials).
  - The finding 2 test row `cd /nonexistent; cat ../.ssh/id_rsa` became `../u/.ssh/id_rsa`: from `~` the old path is `/h/.ssh`, not the user's key.
  - The part-1 findings 3 and 9 tests are un-ignored and pass.

### What is left, and why

- **Decision for the owner: Any rules and FullAccess don't ask on an uncertain cwd.** They still allow what isn't hard-denied, the same as dev. `cd -; cat id_rsa` at the start of a line is therefore Allow under Any or Full, the same as dev's `cd $X; cat id_rsa`. A stricter choice would make an uncertain cwd ask under Any as well.
- **Decision recorded: a folder linked into `~/.ssh`.** Its files are credentials; the folder itself is not. This partly reverses the M1-03 finding 5 nit, which wanted the whole folder ordinary.
- **Limits**:
  - Item 11's walk: when its budget runs out (4096 entries, depth 8), the verdict doesn't change.
  - Text sent through `tr`, `sed`, `base64 -d` and similar can't be read.
  - `CDPATH` isn't modelled.
  - A plain `docker exec -i web sh <<EOF` still asks; only the substituted-argv form is a hard deny.
- **Still ignored, outside this brief**:
  - part-1 findings 4, 5 and 6 (ANSI-C quotes, recursive search of a credential folder, other credential stores);
  - gate findings 10 and 11;
  - fuzz finding 11.
- **Host test**: `kill_recorded_setsid_grandchild_and_unreaped_zombie_leader_boundaries` is ignored on macOS (reason above); it still runs on Linux.
- **The fuzz test's time bound**: `random_hostile_lines_never_panic_and_nothing_sensitive_is_allowed` asserts under 40 s in a debug build. It failed once (40.2 s) in the full run while the machine's load average was about 100.
  - Run alone it takes 15 to 22 s on the branch and 14 to 17 s at the previous commit.
  - The test was not changed.
- No input reached Allow on a sensitive path that the review didn't name.

### Real outputs of the Definition of Done (macOS, this worktree)

- `cargo fmt --all --check`: clean (no output).
- `cargo clippy --workspace --all-targets -- -D warnings`: `Finished`, no warnings.
- `cargo test -p atrium-core -p atrium-daemon -p atrium-host --no-fail-fast`:

```
atrium-core lib 9 passed | adversarial_fuzz 1 passed 1 FAILED (too slow: 40.21s, load avg ~100) 1 ignored
adversarial_gate 9 passed 2 ignored | adversarial_ledger 14 | adversarial_ordinary 6 | adversarial_policy 15 passed 3 ignored
adversarial_regression 22 | attention 8 | history 25 | ledger 20 | policy 44 | store 28
atrium-daemon lib 13 | adversarial_recovery 3 | adversarial_socket 7 | client 1 | recovery 3 | sessions 19 | socket 9
atrium-host lib 5 | adversarial_supervise 7 passed 1 ignored (macOS) | lifecycle 1+1+4 | peer 3 | supervise 7
```

- 20 runs in a row (fish: `for i in (seq 20); cargo test -q -p atrium-core --test adversarial_regression; or break; end`):

```
adversarial_regression 20/20   adversarial_ordinary 20/20   adversarial_policy 20/20
adversarial_gate       20/20   policy               20/20   adversarial_socket 20/20
adversarial_supervise  20/20   adversarial_fuzz 19/20 (the one failure: the 40 s bound, 46.6 s while the load average was ~80; the other 19 took 15 to 31 s)
```

- No daemon is left running (`pgrep -fl atriumd`: none).


## Fourth round (Claude Opus 5.5, D-079): the re-review of `253c7f4`

Review: `docs/agents/reviews/M1-13-adversarial-third-part.md` (Gemini 4 Hydrogen `high`, rework). Its item 8 is settled by D-078: Any rules and FullAccess keep dev's behaviour on an uncertain cwd. None of the blockers below is an uncertain-cwd case: each one bypassed a hard deny, so they are hard denies under every preset.

**Method.**
1. Every input the review names went into the tables first, with its verdict per setting:
   - blockers in `adversarial_regression.rs` (`round_4_*`);
   - majors in `adversarial_ordinary.rs` (`round_4_majors_derived_units_keep_their_own_chain`).
2. All of them failed on `253c7f4`.
3. The fixes are in `crates/atrium-core/src/policy/**` only.
4. The majors' expected verdicts are dev's. I measured them by running a scratch test over dev's `crates/atrium-core/src` (the merge base, `8677018`) in a scratch copy, as in the third part.

### Blockers

1. **A recursive search of a folder linked into `~/.ssh`, and keys deeper than the walk** (`gate.rs`).
   - `ssh_members` now also returns the real paths of the folders linked into `~/.ssh`. Its walk was raised to depth 8 and 1024 entries.
   - A new check, `reaches_linked_ssh_dir`, turns the verdict into a hard deny (`credential-read`) when any of these holds:
     - a named path is a file below such a folder, at any depth, with no walk needed;
     - a glob's folder is inside one;
     - a recursive reader's root is the folder, inside it, or holds it. Recursive readers are `rg`, `ag`, `ack`, every `grep` spelling, and the Grep tool with `recursive`.
   - Listing or entering the folder stays ordinary (M1-03 finding 5): `ls vault/work` and `ls vault` are allowed.
   - Test: `round_4_blocker_1_recursive_search_of_a_folder_linked_into_ssh`. It uses `rg`, `grep -r`/`-R`, the Grep tool, `cat` and Read of keys at depth 3 and 7, and `cat vault/work/*` in home, any and full.
2. **`grep -d recurse` and `--directories recurse`** (`gate.rs`, `engine.rs`).
   - `recursive_reader` now calls the same parser as Home's read-only list (`engine::search_roots`, now `pub(crate)`), so the two can't drift.
   - A `grep` line that parser can't read counts as recursive.
   - Test: `round_4_blocker_2_every_grep_recursion_spelling_reaches_the_hard_link_check`, with `-d recurse`, `--directories recurse`, `--directories=recurse`, `--dir=recurse`, `--rec`, `-id recurse`, `-drecurse`, `egrep -r` and `-ri`.
3. **`|| return` and a piped `exit`** (`words.rs`, `exits_after`).
   - Only a plain `exit` narrows the candidates to the target: the next command in the same text, not in a pipeline or a group.
   - `return` never narrows, because it fails outside a function and the line goes on.
   - Test: `round_4_blocker_3_return_and_piped_exit_do_not_narrow`, which also has `|| (exit 1)` and `|| true | exit 1`.
4. **`cd` option clusters** (`words.rs`, `cd_effect`).
   - Clusters of `L`, `P`, `e` and `@` (`-LP`, `-Pe`, `-eP`, `-L -P`, then `--`) are read, and the target is kept.
   - These make the cwd unknown but keep their targets as candidates:
     - any other option;
     - two or more operands (zsh's `cd OLD NEW`).
   - Test: `round_4_blocker_4_cd_option_clusters_keep_the_target`, which has `-Q` and `cd .ssh extra`.
5. **The six lexer and payload gaps** (`shell.rs`, `deny.rs`). Test: `round_4_blocker_5_lexer_and_payload_gaps`.
   1. `format_printf` reads `%N$…`, an argument named by number. The format is then not reused.
   2. A pipe into a group reaches every command in the group, not only the first. Groups are `( … )` and `{ …; }`, and also `if`, `while`, `until`, `for`, `case` and `select` bodies. The lexer keeps a stack of open groups for this.
   3. `${…}` scanning knows about quotes, escapes and a nested `${`.
   4. `flock` and `script` read `-c` with its value attached or inside a cluster: `-c'…'`, `-nc'…'`, `-qc …`. Each wrapper's short options that take a value are listed. No other wrapper in `unwrap` takes a `-c` shell string.
   5. A bare `crontab` (or `crontab -u USER`) with no file reads its table from stdin. `-l`, `-r` and `-e` don't.
   6. A pipe into a command word built by a command substitution counts as code: `s$(echo h)`, `$(echo sh)`, a backtick word. This holds in the lexer (`is_pipe_code`) and in `deny.rs` (`reads_stdin_as_code`). A variable alone, such as `| $PAGER` or `| $CLIP`, still asks, as on dev.

### Majors (back to dev's verdict)

1 and 2. **Derived units keep their own chain** (`shell.rs`, `words.rs`).
- Every unit now records:
  - which text it came from (`ctx`: one number per lex of the line, a payload, a substitution, a here-doc or a `find -exec`);
  - how deeply that text is nested (`level`);
  - whether the text runs in the shell around it (`same_shell`: the payload or stdin text of `eval`, `source` or `.`).
- `walk` keeps a stack of states, one per text, each with its own candidates, `&&` pool and unknown flag:
  - A text starts where the text around it is, and uses its own joins. Derived units are no longer forced to `Seq`.
  - When it ends, the text around it carries on where it was. A same-shell text (`eval`, `source`) instead hands over where it ended.
  - A substitution or payload in the middle of an `&&` chain no longer flushes the chain's pool. `cd X && Y` inside `sh -c`, `eval` or `env -C` follows the `&&` rule.
- **`env -C DIR CMD` keeps dev's verdict**, a hard deny for `env -C proj cat .npmrc` and `env -C build rm -rf *`. dev resolves `CMD` in the start directory: the `env` unit itself, not the payload. Turning that into Ask would be weaker than dev, so the row pins dev's hard deny. **This is a decision for the owner:** the hard deny is a false one (if `proj` doesn't exist, `env` fails and `cat` never runs). Fixing it means reading the `env` unit's own words in `DIR`.
- Test: `round_4_majors_derived_units_keep_their_own_chain`. It has the review's ten lines. Eight get [Ask, Ask, Allow, Allow], dev's verdict; the two `env -C` lines get Hard in all four settings, also dev's.

### Found by the fixer (not named by the review; Ask in home, Allow with Any/Full on dev and on `253c7f4`)

- `source /dev/stdin <<EOF` + `cd .ssh` + `EOF` + `cat id_rsa`, `. /dev/stdin <<<'cd .ssh'; cat id_rsa`, `echo 'cd .ssh' | source /dev/stdin; cat id_rsa`:
  - Here-doc and here-string bodies were appended after every command of the line, so their `cd` came too late.
  - A text that `eval`, `source` or `.` runs in this shell is now placed right after its owner, and is marked `same_shell`. A pipe into one counts too, because zsh runs the last command of a pipeline in this shell.
- `echo 'rm -rf ~' | if true; then sh; fi`, and the same with `while`, `until`, `for` and `case` bodies: fixed with blocker 5.2.
- Test: `round_4_same_shell_texts_and_pipes_into_compound_commands`, all Hard.

### Minor and nit

- `adversarial_fuzz`: the debug wall-clock bound of `random_hostile_lines_never_panic_and_nothing_sensitive_is_allowed` is now 60 s. `policy.rs`'s `pipe_to_shell_fan_out_is_linear_and_denied` stays the guard against a blowup.
- Host test: the macOS ignore reason now names the obstacle, the re-execing `/usr/bin/python3` stub. The zombie-leader premise is ordinary POSIX.

### Checks against dev for false hard denies

I ran 22 ordinary lines that touch these fixes on dev and on the branch, in all four settings. They gave the same verdicts everywhere. Among them:
- `cd -P build && make`, `cd -L .. && ls`;
- pipes into `while`, `if`, `{ …; }` and `( … )`;
- `sh -c`/`bash -c` with `cd X &&`, `eval "$(ssh-agent -s)"`;
- `cd proj || exit 1; make`, `cd proj || return; make`;
- `crontab -l`, `flock /tmp/l make`, `echo ${HOME:-"/tmp"}`, `rg TODO src`, `grep -rn TODO src`, `printf '%s\n' hello | cat`.

### What is left

- **Owner decision:** `env -C DIR CMD` keeps dev's hard deny (above).
- **Limits:**
  - A glob in a folder linked into `~/.ssh` is a hard deny even for a lister: `ls vault/work/*` is denied. `ls vault/*` (a glob that only names the linked folder) is not, and neither is a plain `ls` of the folder. (Wording corrected in the fifth round.)
  - A same-shell text's directory is followed. A child's (`sh -c`, a substitution) is dropped when it ends, which is right for a child. But a `cd` the engine can't follow anywhere in the line still marks the whole line as having an unknown cwd, so Home and prefix rules ask (same as before).
  - A here-doc a child shell reads (`bash <<EOF`) still starts from where the line ends, not from where its owner is (as before; stricter).
- **Still ignored, outside this brief** (as in the third part): part-1 findings 4, 5 and 6, gate findings 10 and 11, fuzz finding 11.

### Real outputs of the Definition of Done (macOS, this worktree)

- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test -p atrium-core -p atrium-daemon -p atrium-host --no-fail-fast`: exit 0.

```
atrium-core lib 9 | adversarial_fuzz 2 passed 1 ignored (12.9 s) | adversarial_gate 9 passed 2 ignored
adversarial_ledger 14 | adversarial_ordinary 7 | adversarial_policy 15 passed 3 ignored | adversarial_regression 28
attention 8 | history 25 | ledger 20 | policy 44 | store 28
atrium-daemon lib 13 | adversarial_recovery 3 | adversarial_socket 7 | client 1 | recovery 3 | sessions 19 | socket 9
atrium-host lib 5 | adversarial_supervise 7 passed 1 ignored (macOS) | lifecycle 1+1+4 | peer 3 | supervise 7
```

- 20 runs in a row (fish: `for i in (seq 20); cargo test -q -p atrium-core --test adversarial_regression; or break; end`):

```
adversarial_regression 20/20   adversarial_ordinary 20/20   adversarial_policy 20/20
adversarial_gate       20/20   policy               20/20   adversarial_supervise 20/20
adversarial_fuzz       20/20
```

- No daemon left running (`pgrep -fl atriumd`: none).

## Fifth round (Claude Opus 5.5, D-080): the round-4 review of `26262aa`

Every review input is a `round_5_*` row: blockers in `adversarial_regression.rs` (7 tests), the major in `adversarial_ordinary.rs` (1 test). Each test failed on `26262aa` and passes now. All changes are in `crates/atrium-core/src/policy/**`. D-078 is unchanged: none of these are uncertain-cwd cases, and the new cwd-root check reads only directories the walk is sure of.

### Blockers

1. **Searches and walkers reaching a folder linked into `~/.ssh`** (`gate.rs`)
   - The Grep tool with no paths searches the session's folder: that folder is now the root of the linked-folder check.
   - A recursive reader with no folder operand (`rg x`, `grep -r x`, `rgrep x`, `ag x`, `ack x`, `find -type f`) uses each directory the walk is sure of for that command (`cd vault/work && rg x` is caught).
   - `rgrep` and `find` were added to the recursive readers.
   - New tree walkers count as recursive: `cp`/`scp` with `-r`, `-R`, `-a`, `--recursive` or `--archive`, `rsync`, `tar`, `zip -r`, `rm -r`, `mv`, `ditto`, `pax`, `cpio`, `7z`, `rclone`.
   - Globs are now matched component by component against the linked folder (`vault/*/id_work`, `vault/?ork/*`, `vault/**/id_work`). A glob that goes below the folder is denied. A glob that only names the folder is denied only for a recursive reader or walker (`rg x vault/*`), so `ls vault/*` stays ordinary.
   - **Verdict choice for the review's `cp -r`, `find -exec cat`, `rm -rf` and `cat vault/*/id_work`:** hard deny (`credential-read`) in every setting, which is stricter than dev (Ask/Allow/Allow). Each of these reads, copies, moves or deletes the key files themselves, which is exactly what the deny list protects. The same goes for `find vault -type f`, which prints only names but is one `-exec` away from the contents.
   - **Home's `root_ok` is not changed.** The engine has no filesystem access, so it can't know about links. The gate's hard deny runs after the engine and overrides any Allow, Home's included, so the outcome is the same in every setting.
   - **Cost:** while a folder is linked into `~/.ssh`, a recursive search or copy of any folder that holds it is denied. For example, with `~/vault/work` linked, `rg x` or `tar czf b.tgz ~` run from `~` is denied. Without such a link nothing changes.
2. **`${…}` inside double quotes** (`shell.rs`, `scan_dollar_brace`): the scan now handles quotes, escapes and nesting inside double quotes too, and there a `'` is an ordinary character. If no closing `}` is found, the text after the first `}` is also lexed as a command.
3. **Compound commands nested inside a piped compound** (`shell.rs`): a group opener or closer counts only in command position (after `then`, `do`, `else`, `{`, `!` and the like), so inner `fi`, `done`, `}` and `esac` close their own group and the outer group keeps its pipe.
4. **Option clusters with values** (`shell.rs`):
   - For `flock` (`w`, `E`, `c`) and `script` (`c`, `F`, `t`, `T`, `I`, `O`, `B`, `E`, `m`), the first letter that takes a value ends the cluster. Its value is the rest of the word or the next word. For `c`, that value is the shell string.
   - `env` clusters (`-iu PATH`, `-iuPATH`, `-iC proj`, `-iCproj`, `-0iu`) are read the same way, both when unwrapping and for `-S` and `-C` payloads.
5. **`crontab -uUSER`** (`shell.rs`): the user name after `u` is skipped, whether it is attached or the next word, and is never read as flags. `-l`, `-r` and `-e` before the `u` still count. A file operand still means the job comes from the file.
6. **Directory-stack `cd`s and an exit that doesn't end this shell** (`words.rs`):
   - `cd ~-`, `cd ~+`, `cd ~+N`, `cd ~-N`, `cd -N`, `cd +N` and `cd -- ~-` make the directory unknown.
   - An `exit` that runs in the background (`|| exit 1 &`, `|| exit &`) no longer narrows the candidates. Neither does one in a pipeline (`|& cat`), as before. `! exit` still ends the shell.
7. **A substituted command word behind a wrapper** (`shell.rs`): the word after the wrappers (`env`, `nohup`, `command`, `builtin`, `time`, `exec`, `nice -n 5`, `timeout 5`, `env -i FOO=1`) is checked for `$(…)` and backticks, and its output is read as code, like a bare first word.

### Major (back to dev's verdict)

- **Change** (`words.rs`, `walk`): when an `&&` chain ends, the places it may have stopped (its pool) no longer join the candidates right away. They join before the next command at the same level. Payloads, substitutions and `-exec` of the chain's last command run only if the whole chain succeeded, so they see only the end of the chain.
- **`eval` and `source`:** when one of these hands back to the outer level, its pool moves into the outer pool, and the outer chain decides when it counts.
- **Result:** the review's eight lines and eight neighbours (`bash -c`, `make && sh -c`, `cd a && cd b && sh -c`, `time sh -c`, `eval && eval`, `source /dev/stdin <<<`, backticks, `nohup sh -c`) get dev's verdict, Ask/Ask/Allow/Allow.

### Trade-off: a pipe into a substituted program (fixed)

- **Now dev's verdict (Ask/Ask/Allow/Allow):** `curl … | $(brew --prefix)/bin/jq .`, `"$(brew --prefix)/bin/jq"`, `| $(which less)`, `cat data.json | …`, `echo hi | …` and `| $HOME/bin/jq .`.
- **How:** `shell::substituted_word_may_be_shell` decides whether such a word may be a shell. It is used by the lexer's pipe check and by `deny.rs`. The raw-text fallback now keeps `$(…)` whole when it splits words.
- **Not a shell:** a word ending in a literal program name that isn't a shell, or a `$(which|whence|command -v|type -p NAME)` of a literal name that isn't one.
- **Still a hard deny (`pipe-to-shell`), with rows:** anything else, including `$(echo sh)`, `$(brew --prefix)/bin/sh`, `$(which bash)`, `` `which sh` ``, `$(command -v zsh)`, `$(x)/bin/$Y`, `$(x)/bin/s*`, `s$(echo h)`, `$(echo /bin)/sh`, `$(printf 'sh #')/jq` (known output that splits into words), `$(which $S)` and `$(x)/bin/s\h`.
- **Known limit:** the output of an unknown command (`$(brew --prefix)`) is not quoted, so it could split into words and name a different program. The engine can't see that output, so it trusts the literal tail, as dev did.

### Neighbouring spellings checked against dev (no new false hard deny)

- Engine lines with the same verdict on dev and on this branch:
  - `cd proj && make && make test`, `eval "$(ssh-agent -s)" && echo ok`, `echo "${HOME}/x"`, `echo "${x:-'a'}" ; ls`
  - `env -C proj make` (dev's hard deny, as before), `env -u FOO make`, `env -iu FOO make`
  - `flock -n /tmp/l make`, `flock -nw 10 /tmp/l make`, `script -q /dev/null make`
  - `crontab -l`, `crontab -u bob -l`, `crontab -ubob -l`
  - `cd ~- && ls`, `make || exit 1`, `make || exit 1 &`
  - `rg TODO src`, `grep -rn TODO src`, `find src -name '*.rs'`, `cp -r src /tmp/x`, `tar czf /tmp/x.tgz src`, `rm -rf build`, `mv a b`
  - piped `if`/`while` bodies, `cd proj && sh -c 'make'`, `env $(cat .env) make`, `nohup $(which node) server.js`, `ls | $(which wc) -l`, `git log | $(git var GIT_PAGER)`
- The only difference is `cd - && ls` with prefix rules (dev Allow, branch Ask). `cd -` was already unknown before this round. That's D-078 (an uncertain cwd asks under prefix rules), not a round-5 change.
- Gate lines with a folder linked into `~/.ssh` that are not denied: `ls vault/work`, `ls vault`, `ls -la vault/work`, `ls vault/*`, `ls vault/w*`, `cat vault/notes.txt`, `cat vault/*.txt`, `cp vault/notes.txt /tmp/n`, `rg x vault/notes.txt`, `cd /tmp && rg x`, `find /tmp -name x`.

### Found by the fixer

- These neighbours reached Allow under Any/Full on `26262aa` and are now hard denies with rows:
  - `cp -pr`, `cp --recursive`, `rsync -r`, `tar -cf - vault | ssh …`, `ditto`, `zip -qr`, `rm -fr`;
  - `cd vault/work && rg x`, `cd vault && grep -r x`;
  - `cat vault/**/id_work`, `cat vault/*/*`, `head vault/[w]ork/id_work`;
  - `rgrep -n x vault`, `grep -rl x vault/*`, `find vault/w* -type f`;
  - from inside the linked folder: `grep -rn x`, `grep -rl x .`, `ack x`, `rg -n x`, `find -type f`, `rgrep -n x`, `tar czf /tmp/x.tgz .`, `cp -r . /tmp/exfil`.
- Fixed while running the DoD: in the new substituted-word check, the nesting depth now carries through the lexer. Before, `$(` repeated 5,000 times overflowed the stack in `engine_never_panics_on_garbage`.

### What is left

- **Owner decision (unchanged):** `env -C DIR CMD` keeps dev's hard deny.
- **Limits:**
  - The output of an unknown substitution is trusted to be a single word (see the trade-off above).
  - The linked-folder check reads only directories the walk is sure of. After a `cd` it can't follow, `rg x` keeps D-078's verdict.
  - Walkers not on the list (for example `rsync`-like tools under other names) are read only as plain file uses.
- **Still ignored, outside this brief:** part-1 findings 4, 5 and 6, gate findings 10 and 11, fuzz finding 11.

### Real outputs of the Definition of Done (macOS, this worktree)

- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test -p atrium-core -p atrium-daemon -p atrium-host --no-fail-fast`: exit 0, no failures. Test counts that changed: `adversarial_regression` 35 (was 28), `adversarial_ordinary` 8 (was 7). Everything else is as in the fourth round.
- 20 runs in a row (fish: `for i in (seq 20); cargo test -q -p atrium-core --test adversarial_regression; or break; end`):

```
adversarial_regression 20/20   adversarial_ordinary 20/20   adversarial_policy 20/20
adversarial_gate       20/20   policy               20/20   adversarial_ledger   20/20
adversarial_fuzz       20/20   atrium-core lib      20/20
```

- No daemon left running (`pgrep -fl atriumd`: none).

## Sixth round (Claude Opus 5.5, D-081, D-088): the round-5 review of `5a8c57c`

The round-5 review's "Convergence assessment" asked for fixes to mechanisms, not spellings, so each item below gives:
- the mechanism;
- the general rule now implemented;
- the table of neighbouring spellings, generated where possible, in `crates/atrium-core/tests/adversarial_regression.rs` (`round_6_*`) and `adversarial_ordinary.rs`;
- the fix.

Every `round_6_*` test fails on `5a8c57c`, except `round_6_ordinary_lines_keep_devs_verdict`, which is a no-regression table. All of them pass now. Changes are only in `crates/atrium-core/src/policy/**`.

The orchestrator's reading of the work-in-progress commit `87d6679` was partly wrong. Items 3, 4, 6 and 7, and most of item 5, were already written there. What was missing was the gate side of items 1 and 2: the `Reach` enum, glob expansion, the reader tables, and `find` through `find_roots`.

D-078 is kept: an uncertain cwd under Any/FullAccess keeps dev's Allow, and the cwd-root checks read only directories the walk is sure of. `env -C` is unchanged (still open for Pedro). New fail-closed paths end at Ask, never at a new hard deny.

### Item 1: globs reaching a folder linked into `~/.ssh` (`gate.rs`)

- **Mechanism:** `glob_reaches` compared pattern components with the linked folder's components one to one, so `**` matched exactly one folder. A glob whose prefix didn't resolve was missed.
- **Rule:** a glob is expanded on the filesystem the way the shell expands it (`expand_glob`):
  - it goes component by component from its literal prefix;
  - `**` matches zero or more components, files included when it is last;
  - `{a,b}` is alternation, and dot-files are skipped unless the pattern names them;
  - a literal component matches only a name that exists.
  - The result is a hit when any expansion is inside the linked folder. A recursive reader or walker is also a hit when an expansion is the folder or a folder above it (`rg x vault/*`).
  - When the expansion can't be done (more than 4,096 entries, `**` more than 12 deep, a component globset can't read, `$` or a backtick in the prefix), the result is `Reach::Maybe`, which turns an Allow into an Ask.
  - `reaches_linked_ssh_dir` now returns `Reach { No, Maybe, Yes }`. Yes gives a hard deny (`credential-read`); Maybe gives Ask.
- **Table** (`round_6_items_1_and_2_…`):
  - 15 glob shapes × 5 readers (`cat`, `head -n 1`, `cp -t /tmp`, `grep x`, `tar czf`). Shapes: `~/**/id_work`, `~/**/id_*`, `~/**/*`, `~/*/**/id_work`, `~/**/work/*`, `~/**/w*/*`, `~/v*/w*/id_work`, `~/vault/**/id_work`, `~/vault/**`, `~/{vault,x}/work/id_work`, `~/vault/{work,x}/*`, `~/vault/*/*`, `vault/**/id_work`, `./**/id_work`, `~/**/../vault/work/*`.
  - 4 folder globs × 5 recursive readers or walkers.
  - All are Deny in home, any and full.
  - Not denied: `ls ~/vault/*`, `ls vault/*`, `ls ~/**/notes.txt`, `cat ~/**/notes.txt`, `cat ~/proj/**/*.rs`, `cat ~/{proj,x}/src/main.rs`, and others.

### Item 2: recursive readers given no folder (`gate.rs`, `shell.rs`)

- **Mechanism:** `searches_cwd` guessed. For `find` it looked only at the first word, so `find -L` and `find -H` were missed. For `ag` and `ack` it counted words not starting with `-`, so `-C 3` hid the search of the cwd.
- **Rule:** each reader's options are read with a table.
  - `find`: through `shell::find_roots_explicit`, the same parser `find_roots` and `-exec` use. Its table covers GNU `-H -L -P -O<n> -D x` and BSD `-E -X -d -s -x -f PATH`, where `-f` gives a root. With no root it walks `.`.
  - `rg` and `grep`: through `engine::search_roots`, the parser Home's read-only list uses. `-R` is read as `-r`, since it has the same roots.
  - `ag` and `ack`: through new tables (`AG_OPTS`, `ACK_OPTS`). `-g` and `-f` mean there is no pattern word. A short option whose value is optional (`ack -C [N]`) gives two readings, and the folder is searched if either reading leaves no path.
  - An option the table doesn't know gives `None`. If the unit's known cwd holds the linked folder, that is `Reach::Maybe` (Ask), not a hard deny.
- **Table:** 46 reader spellings × three ways to be in the folder holding the link (`cd vault && …`, cwd `vault`, cwd `vault/work`): Deny everywhere.
  - Reader spellings include `find -L/-H/-P/-O3/-D tree/-E/-X/-s/-x`, `find -L -exec cat {} +`, `rg -C 3 x`, `rg -tpy x`, `grep -R x`, `grep -r -C 3 x`, `rgrep -C 3 x`, `ag -C 3 x`, `ag -G pat x`, `ag -g pat`, `ack -C 3 x`, `ack -C x`, `ack -f` and `ack --type rust x`.
  - Not denied: `cd proj && rg x`, `cd proj && find -L . -name '*.rs'`, `ag -C 3 x proj`, `ack -C 3 x proj`, `find -L proj -name '*.rs'`.

### Item 3: quoting inside `${…}`, `$(…)`, `"…"` and backticks (`shell.rs`)

- **Mechanism:** `scan_dollar_brace` didn't skip `$(…)` or backticks, and `scan_paren` skipped a `"…"` without its own `$(…)`. A quote inside a nested substitution then ended the wrong construct.
- **Rule:** every scanner skips every construct the shell reads inside it, using the same scanners, at any depth:
  - in `$(…)`: quotes, `${…}`, backticks, and nested `$(…)` and `$((…))`;
  - in `"…"` (`skip_dq`): `$(…)`, backticks and `${…}`;
  - in `${…}`: all of them, with `'` counting only outside double quotes.
  - Past `MAX_SCAN_NEST` (128) a scanner reports the construct as unclosed (fail closed: unterminated, so it asks, and the `${` ambiguous tail is still lexed).
- **Table:** a property test, `round_6_item_3_generated_quote_and_substitution_prefixes`.
  - Generators build words valid in unquoted and double-quoted contexts by nesting `"…"`, `$(…)`, `${x:-…}` and backticks to depth 2, plus selected depth-3 words. The leaves hold `'"'`, `"'"`, `'}'`, `')'`, `\"`, `\)`, `\}` and similar.
  - For each word P, `echo P ; rm -rf ~`, `echo P && rm -rf ~` and `echo P<newline>rm -rf ~` must be hard denies in all four settings. That is more than 1,000 lines.

### Item 4: command position and chain state (`shell.rs`, `words.rs`)

- **Mechanism:**
  - A reserved word opened or closed a compound only after a short list of words, so `function f {`, `coproc {` and `coproc N {` were missed, and their `}` popped the outer piped group.
  - Chain state was flat: an internal `;` inside a compound ended the enclosing `&&` chain (the round-5 major).
- **Rule, command position:** `opens_body` accepts every word that can precede a command: `then`, `do`, `else`, `elif`, `!`, `{`, `if`, `while`, `until`, `time [-p]`, `coproc [NAME]` and `function NAME`.
  - `NAME()` / `NAME ()` is read as a function definition: the next compound is its body, and the name and body are one command in the chain.
  - Compounds have a kind (`Compound::{Paren, Brace, Cond, Loop, Func}`) and a number unique to that compound.
  - A closer with no open compound, or a compound still open at the end, adds the reason "unbalanced compound" (Ask).
- **Rule, chain state:** every unit carries the compounds it is inside. The walk keeps a `Frame` per open compound, holding the `&&` chain around it.
  - On entry, the outer chain is put aside.
  - At the end, the compound's stopping places join the outer chain's pool, which decides when they count.
  - A body that may not run (`if`, `case`, loops, functions, `coproc`, and a `{ }` after `||`) leaves the entry directory as a candidate.
  - A `cd` in a loop or function body may repeat, so it makes the directory unknown.
  - A `( … )` group's place in the chain is the join written after it (`close_join`).
- **Also fixed (not named by the review; found by the generator):**
  - After `||` the next command may be skipped. `make || cd /tmp && cat ~/.ssh/id_rsa` and `true || { cd /tmp && true; } && cat …` reached Allow under Any/Full on dev and on `5a8c57c`. They now keep the old directory as a candidate and are hard denies.
  - The candidate is added only when the skipped command moved, so `a || b || c` doesn't grow the list.
- **Tables:**
  - `round_6_item_4_generated_compounds_inside_a_piped_compound`: 18 compound shapes, each nested inside every other, inside 7 piped containers (2,268 lines). Each is `echo 'rm -rf ~' | …; sh; …` and must be a hard deny.
  - `round_6_item_4_generated_chain_state_keeps_the_old_directory` (105 lines):
    - `cd /tmp && C ; cat …` and `|| cat …` for every compound;
    - 10 bodies that may not run × 3 `cd` forms × `;`/`&&`;
    - loops and functions that repeat `cd ..`;
    - `(cd /tmp) && …` and similar.
    - All are `credential-read`.
  - `adversarial_ordinary.rs::round_6_major_generated_compounds_inside_a_cd_chain`: 20 compound shapes × 4 tails (`cd proj && C && cat .npmrc`, `cd build && C && rm -rf *`, `… && sh -c …`, and the compound twice), all at dev's Ask/Ask/Allow/Allow. The review's seven lines are among them.

### Item 5: one option reader for every wrapper (`shell.rs`)

- **Mechanism:** most wrappers had a list of exact value-taking words. A cluster ending in a value letter (`exec -ca foo`, `caffeinate -it 5`, `ionice -tc 2`, `xargs -tI {}`) left the value as the command. `flock` and `script` stopped reading options after their file.
- **Rule:** `unwrap` now runs on one table-driven reader, `read_opt` with `wrapper_opts`.
  - Each wrapper's table lists short flags, short options with a value (the rest of the word or the next word), short options with an optional attached value (`xargs -i{}`), long flags and long options with a value (`--opt v` or `--opt=v`), and bare numbers (`nice -10`).
  - The table covers `time`, `nohup`, `builtin`, `command`, `setsid`, `unbuffer`, `caffeinate`, `sandbox-exec`, `chroot`, `exec`, `nice`, `ionice`, `stdbuf`, `timeout`, `xargs`, `chrt`, `taskset`, `flock`, `script`, `busybox`/`toybox`, `arch` (multi-letter) and `env` (its own cluster reader).
  - `flock` and `script` read their options after the file too, which is the stricter reading. `script` gains `-o`/`--output-limit`, and `-F` is now a flag.
  - `chrt -p` and `taskset -p` act on a process: no command.
  - **Fail closed:** an option the table doesn't know is read as a flag, as before, and adds the reason "unknown wrapper option". A prefix or exact rule then can't allow the line (Ask), while Any and FullAccess keep dev's Allow.
- **Tables:**
  - `round_6_item_5_generated_wrapper_option_clusters`: for 16 wrappers, every flag letter clustered with every value letter in several shapes: `-fv V`, `-fvV`, `-f -v V`, `-v V -f`, `-vV`, `-v V`, all flags together. Each comes with `rm -rf ~` and with `sh -c 'rm -rf ~'`. `flock` and `script` also have their options after the file, and there are nested wrappers. More than 500 lines, all `recursive-delete-outside-cwd`.
  - `round_6_item_5_unknown_wrapper_options_never_reach_allow_by_prefix`: `W -Z`, `W --zzz` and `W -Z9` with a prefix rule for `W`: never Allow.

### Item 6: a substituted word naming an interpreter (`shell.rs`, `deny.rs`)

- **Mechanism:** `substituted_word_may_be_shell` answered only "is it a shell?", so `$(which python3)` and `$(which crontab)` passed.
- **Rule:** `shell::substituted_program` returns the program a substituted word plainly names, or `None` when it may be anything. It handles a literal tail after `/`; `which`, `whence`, `command -v` and `type -p` of a literal name; and `$(echo NAME)` (one known word). `None` is read as a shell.
  - `deny.rs` replaces the word with that program and reads the command again with `reads_stdin_as_code`, so interpreters, `crontab`, `at`, `script` and `make -f -` all count.
  - The lexer does the same for shells, `eval`, `source` and `reads_stdin_as_script`.
- **Table:** `round_6_item_6_generated_substituted_interpreters`: 18 programs × 9 spellings (`$(which N)`, `$(command -v N)`, backticks, `$(type -p N)`, `$(echo N)`, quoted, `$(brew --prefix)/bin/N`, `$(whence N)`), plus `$(which make) -f -`. All are pipe-to-shell.
  - The same 9 spellings with jq, less, wc, cat, grep, sort, head and tee keep dev's Ask/Ask/Allow/Allow (checked against dev).

### Item 7: `eval` and `source` that may run in a child (`shell.rs`, `words.rs`)

- **Mechanism:** a same-shell text always replaced the parent's candidates, even when its owner was in a pipeline (a subshell in bash, the current shell in zsh).
- **Rule:** `Unit::shell_union` is set when the owner is in a pipeline, a group, a subshell or the background, or when its stdin text came through a pipe. Then the walk unions the parent's candidates with where the text ended, and never drops them. An unknown cwd in the text makes the parent unknown.
- **Table:** `round_6_item_7_generated_same_shell_text_in_a_child`: 6 owners × 6 contexts (`echo hi | O &&`, `… | O ;`, `(O) &&`, `O &`, `{ O; } | cat;`, `O | cat &&`) plus 4 stdin forms. All are `credential-read`.

### Ordinary lines against dev

- `round_6_ordinary_lines_keep_devs_verdict` has 87 lines, all at dev's verdict, probed on dev and on this branch.
- They include `nice -n 10 make`, `nice -10 make`, `xargs -I {} echo {}`, `xargs -tI {} echo {}`, `timeout 5 cargo test`, `timeout -s KILL 5 cargo test`, `find -L . -name '*.rs'`, `stdbuf -oL make`, `ionice -c3 -n7 make`, `caffeinate -dims make`, `exec -a foo make`, `env -iu FOO make`, `flock -w 10 /tmp/l make`, `chrt -f 10 make`, `taskset -c 0 make`, and `arch -arm64 make`.
- They also include compound commands in `&&` chains, function definitions, piped loops, `curl … | $(brew --prefix)/bin/jq .`, `git log | $(git var GIT_PAGER)`, `eval "$(ssh-agent -s)"`, `source .venv/bin/activate && pytest`, `rg`, `ag`, `ack`, `cp -r`, `tar` and `rm -rf build`.
- Together with the 80 generated major lines and the 72 good-program lines of item 6, nothing differs from dev.

### What is left

- **Owner decision, unchanged:** `env -C DIR CMD` keeps dev's hard deny.
- **Limits:**
  - A glob expansion bigger than 4,096 entries or deeper than 12 folders asks (only while a folder is linked into `~/.ssh`).
  - The output of an unknown substitution is assumed not to split into words (as in round 5).
  - Wrappers not in the table (`sudo`, `doas`, `watch`, …) are not unwrapped (as before).
  - `if` and `case` conditions are read as part of the body (the stricter reading).
- **Still ignored, outside this brief:** part-1 findings 4, 5 and 6, gate findings 10 and 11, fuzz finding 11.

### Real outputs of the Definition of Done (macOS, this worktree)

- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace --no-fail-fast`: exit 0, no failures, and no daemon left running (`pgrep -fl atriumd`: none). Test counts for atrium-core:

```
atrium-core lib 9 | adversarial_fuzz 2 passed 1 ignored (13.8 s) | adversarial_gate 9 passed 2 ignored
adversarial_ledger 14 | adversarial_ordinary 10 | adversarial_policy 15 passed 3 ignored
adversarial_regression 44 | attention 8 | history 25 | ledger 20 | policy 44 | store 28
```

- 20 runs in a row (fish: `for i in (seq 20); cargo test -q -p atrium-core --test adversarial_regression; or break; end`):

```
adversarial_regression 20/20   adversarial_ordinary 20/20   adversarial_policy 20/20
adversarial_gate       20/20   policy               20/20   adversarial_fuzz     20/20
```

## Seventh round (D-092): stopped

Stopped by the coordinator (usage limit) before the 20/20 loops; work-in-progress commit. It compiles, and `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace --no-fail-fast` all pass.

Done (each has a generated `round_7_*` table in `adversarial_regression.rs`; all 7 tables were checked to fail on eb0aa88):

- **Item 1 (globs).** `expand_glob` returns `None` (Maybe, so it asks) when any component has a `$`, a backtick or a brace. A gate path word with a `$` or a backtick is Maybe. `words::brace_expand` now expands `{x..y[..n]}` sequences, letters and integers, up to 64 words; a longer one stays literal.
- **Item 2 (`||` before `eval`/`source`/`.`).** The walk records whether the last command came after `||` (`Level::last_skippable`). Same-shell text it runs is unioned with where the shell was, never put in its place.
- **Item 3 (`ag`/`ack`).** Both tables were audited against the real option lists. `ag -A/-B/-C` and `--after/--before/--context`, and `ack --context`, are optional values read both ways (`long_opt_vals`). The missing entries were added.
- **Item 4 (`NAME()`).** A definition is recognised at any command position (`opens_body` on the words before the name, with or without `function`).
- **Item 5 (substituted wrappers).** `deny::reads_stdin_as_code` and `shell::is_pipe_code` unwrap after `substituted_program`.
  - Beside it, not named by the review: any command word built by a substitution that names a program is also read as that program behind its wrappers, for every hard deny. On eb0aa88, `$(which sudo) id`, `$(which env) rm -rf ~/` and `$(which git) push --force` reached Allow. Test: `round_7_beside_item_5_generated_substituted_command_words`.
- **Item 6 (wrapper tables).**
  - `unwrap_readings` and the `Opt::Either`/`Opt::Split` cases give every reading. Wrapper options that can be read more than one way add the reason "unknown wrapper option", and the other readings become derived units, so hard deny checks each one.
  - These forms are read both ways: `script -t`/`--timing`, a long flag with `=value` (`taskset --cpu-list=0-3`), and short flags followed by letters they don't take (`taskset -c0-3`).
  - `script FILE CMD` is also read as if the command started at FILE.
  - The flags of `setsid`, `chroot`, `ionice`, `timeout`, `chrt`, `taskset` and `flock` were audited, and `xargs --replace/--eof/--max-lines` were moved to optional long values.
- **Major 7.** Redirections after `( … )` keep the group's join (`after_paren` and `group_tail`), and so does `( … ) | x`. A command piped into the next one no longer ends an `&&` chain: this also fixes `cd proj && ls | wc -l && cat .npmrc`, which eb0aa88 hard-denied. A `cd` inside `( … )` is followed in the group, and the group's frame adds where the shell was.
  - `cd proj && (cd /tmp && true) && cat .npmrc` stays a hard deny, as on dev. Part 1's finding 1 needs subshell `cd`s to stay candidates, and `/tmp/.npmrc` is one.
  - Ordinary table: `round_7_major_generated_redirected_and_piped_compounds_in_a_cd_chain` (324 lines) and `round_7_ordinary_lines_keep_devs_verdict`.

Not done: the 20/20 loops of the six binaries, and a final review of this section. `script FILE CMD` now asks under a `script` prefix rule; this is the fail-closed fallback.
