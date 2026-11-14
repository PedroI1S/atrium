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
