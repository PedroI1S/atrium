# Handoff: M1-15-store-launch-recovery

- **Task:** `M1-15-store-launch-recovery` (Store Launch, Unfinished Recovery, Resume Race & Flakes)
- **Worker:** Gemini 4 Hydrogen
- **Branch:** `agent/hydrogen/M1-15-store-launch-recovery`
- **Base commit (branch start):** `5175a92`
- **Note on M1-13:** As instructed, M1-15 was implemented ahead of M1-13 (`D-090`); existing M1-13 adversarial cases (`an_archived_unfinished_thread_is_closed_on_next_start`, `failed_recovery_closure_retries_and_clears_process_record`, etc.) were implemented directly in M1-15's owned files without modifying any schema migrations (`crates/atrium-core/schema/0001_init.sql` unchanged).

---

## Steps Completed

### 1. CR-09 — `Signal::Recovered { cleanup: Unverified }` Attention Transitions & Ordering
- **Store / Attention (`crates/atrium-core/src/attention/mod.rs`):** Updated `next_state` so `Signal::Recovered { cleanup: Unverified }` transitions both `Ended` and `Interrupted { cleanup: Verified }` to `Interrupted { cleanup: Unverified }`, while preserving `Failed` unchanged.
- **Daemon Ordering Verified:**
  - `crates/atrium-daemon/src/sessions.rs`: `Signal::Recovered { cleanup: Unverified }` is emitted before `close_incarnation`.
  - `crates/atrium-daemon/src/upkeep.rs`: `upkeep::recover` records `Signal::Recovered { cleanup }` before appending closing events in `close_incarnation`.
- **Tests:**
  - `crates/atrium-core/tests/attention.rs`: updated `"Signal::Recovered(Unverified)"` row to `"UUUUUU=U=U"` and added `unverified_recovery_overrides_ended_and_verified_interrupted_but_not_failed`.
  - `crates/atrium-daemon/tests/sessions.rs`: added `a_stopped_session_with_unverified_cleanup_becomes_interrupted`.

### 2. CR-08 — `Store::launch` and Resume Effort/Preset Preservation
- **Store API (`crates/atrium-core/src/lib.rs`, `crates/atrium-core/src/store/mod.rs`, `crates/atrium-core/src/store/reader.rs`):** Added `Store::launch(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<Option<NewThread>>>` reading `(provider, model, effort, preset, location, title)` from `threads` (reflecting the thread's current title after any renames). Refactored row decoding into a local `LaunchRow` struct (satisfies `clippy::type_complexity`). Updated mock/stub implementation in `crates/atrium-daemon/tests/gaps_snapshot.rs`.
- **Daemon (`crates/atrium-daemon/src/commands.rs`):** Updated `resume_thread` to call `inner.store.launch(thread)` and construct `Launch` (`SessionConfig` effort and `SessionGate` permissions/preset) using the stored `provider`, `model`, `effort`, and `preset`.
- **Tests:**
  - `crates/atrium-core/tests/store.rs`: added `launch_reads_stored_launch_fields_and_current_title`.
  - `crates/atrium-daemon/tests/sessions.rs`: added `resume_keeps_effort_and_non_home_preset` (verifies both `SessionConfig` effort/permissions and `SessionGate::check` behaviour).

### 3. CR-10 — `Store::unfinished_incarnations` & Archived/Open Recovery
- **Store API (`crates/atrium-core/src/lib.rs`, `crates/atrium-core/src/store/mod.rs`, `crates/atrium-core/src/store/reader.rs`):** Added `Store::unfinished_incarnations(&self) -> BoxFuture<'_, StoreResult<Vec<(ThreadId, u32)>>>` returning every thread's current incarnation that has stored events and no `SessionEnded` event, regardless of archive status. Updated mock/stub implementation in `crates/atrium-daemon/tests/gaps_snapshot.rs`.
- **Store Writer (`crates/atrium-core/src/store/writer.rs`):** On `AgentEvent::SessionEnded`, updated `UPDATE sessions` to also clear `process = NULL` atomically within the same SQLite transaction so a crash between `SessionEnded` and `set_process(..., None)` cannot leave a stale `sessions.process`.
- **Daemon Recovery (`crates/atrium-daemon/src/upkeep.rs`):** Updated `upkeep::recover` to kill recorded `live_processes()` first, close every `(thread, incarnation)` returned by `unfinished_incarnations()` (archived threads included), still close zero-event `Starting` listed threads, and only clear `set_process(..., None)` once closures succeed.
- **Contract (`docs/contracts/m1.md`):** Updated §§6.2 and 10 sentences describing `ResumeThread` / `Store::launch` (`CR-08`, `D-067`) and `Store::unfinished_incarnations` / recovery (`CR-10`, `D-067`).
- **Tests:**
  - `crates/atrium-core/tests/store.rs`: added `unfinished_incarnations_lists_open_current_incarnations_archived_or_not` (including direct SQLite check that `SessionEnded` clears `sessions.process`).
  - `crates/atrium-daemon/tests/recovery.rs`: added `an_archived_unfinished_thread_is_closed_on_next_start`.

### 4. Finding 12 — Failed Recovery Closure Retries & Preserves Process Record
- **Daemon Recovery (`crates/atrium-daemon/src/upkeep.rs`):** `upkeep::recover` includes unfinished incarnations in `records` and defers `set_process(..., None)` until after closing events append cleanly, ensuring that if closure fails mid-recovery (e.g. disk full / append error), the next daemon start finds the incarnation via `unfinished_incarnations()` and clears the remaining `sessions.process` entry once closure succeeds.
- **Test (`crates/atrium-daemon/tests/recovery.rs`):** Added `failed_recovery_closure_retries_and_clears_process_record` (uses a SQLite trigger to fail the recovery append on start #1 and verifies retry + `sessions.process` cleanup via `assert_recorded_processes_count` on start #2).

### 5. N1 — `ResumeThread` After Terminal State Race
- **Daemon (`crates/atrium-daemon/src/sessions.rs`, `crates/atrium-daemon/src/commands.rs`):**
  - Added `terminal: AtomicBool` and `done: watch::Sender<bool>` (`mark_terminal`, `is_terminal`, `finish`, `finished`) on `Live`, an RAII `SessionGuard` removing `live` from `sessions`, signaling `finish()`, and notifying `quieter` on `Drop`, and `Inner::wait_if_ending`.
  - Marked sessions terminal in `Live::stop`, before `store.append` in `store_batch` when `SessionEnded` or fatal `Error` is in the batch (stopping the live handle on fatal error), before `Signal::StorageReadOnly` in `pump`, at `pump` exit in `run`, and before `store.signal(Stopped)` in `Shutdown`.
  - Updated `Inner::launch` and `resume_thread` (`Inner::wait_if_ending`) to wait for an already-terminal ending session's teardown before checking `session_of` / launching the replacement incarnation, while still immediately rejecting non-terminal active sessions with `Invalid: "the thread already has a session"`.
- **Test (`crates/atrium-daemon/tests/sessions.rs`):** Added `wait_update` helper and `resume_right_after_final_state_in_a_tight_loop` (201 tight rounds cycling across `Ended`, `Interrupted`, `Start::Fail` -> `Failed`, and `Shutdown` -> `Ended`, plus a concurrent double-resume rejection check).

### 6. N2 — Inbox Stream Ordering Race in Test
- **Fix (`crates/atrium-daemon/tests/sessions.rs`):** Updated `the_inbox_stream_follows_threads_without_following_every_token` to await `ThreadUpdated` / `InboxUpdated` frames directly on `watcher` via `wait_update` rather than polling SQLite on `wire` and assuming `watcher.aside` caught up across a `ListInbox` barrier; also shut down `other` and `rig.stop(true).await` cleanly.

### 7. N3 — Intermittent Zombie `sleep` Cleanup (`Unverified`) Investigation
- **Finding:** Neither `a_process_the_adapter_left_running_is_killed_or_reported` (`crates/atrium-daemon/tests/sessions.rs`) nor `a_killed_daemon_is_recovered_at_the_next_start` (`crates/atrium-daemon/tests/recovery.rs`) checks liveness prematurely: `sessions.rs` awaits `thread_when(Interrupted)` and its reaper task before asserting `!alive(pid)`, and `recovery.rs` reaps `crash_child` before `rig.boot` synchronously runs `upkeep::recover`.
- **Root Causes in `crates/atrium-host/src/supervise.rs` (outside M1-15 owned paths):**
  1. `kill_recorded` (`supervise.rs:206-243`, `GROUP_DRAIN_TIMEOUT = 2s` at line 46) cannot reap a child it did not spawn; an exited zombie process remains in `recorded.pgid` until the test's reaper task (Tokio global `SIGCHLD` driver) or `init`/subreaper reaps it, so if reaping takes >2s under load, `group_is_empty` returns `false` and marks cleanup `Unverified`.
  2. On Linux, `spawn_group` / `identify_spawned` (`supervise.rs:62-73, 449-474`; see `docs/agents/reviews/M1-14a-linux-host.md:216-219`) can read `/proc/<pid>/exe` across `CLONE_VFORK` before `execve` replaces the address space, recording `<test-binary>` as `recorded.executable` so `kill_recorded` later fails the executable identity match (`live != *recorded`) and returns `Unverified` without `SIGKILL`.
- **Change in owned paths (`crates/atrium-daemon/tests/sessions.rs`):** Added `rig.stop(true).await;` at the end of `a_process_the_adapter_left_running_is_killed_or_reported` and `a_dead_provider_nobody_reaped_is_not_called_stopped` so no daemon is left running by tests. Both passed on macOS in our runs (`supervise.rs` untouched as out of owned paths).

---

## Files Modified (Commits `fd6ddb4` .. `6320403`)

- `crates/atrium-core/src/attention/mod.rs`
- `crates/atrium-core/src/lib.rs`
- `crates/atrium-core/src/store/mod.rs`
- `crates/atrium-core/src/store/reader.rs`
- `crates/atrium-core/src/store/writer.rs`
- `crates/atrium-core/tests/attention.rs`
- `crates/atrium-core/tests/store.rs`
- `crates/atrium-daemon/src/commands.rs`
- `crates/atrium-daemon/src/sessions.rs`
- `crates/atrium-daemon/src/upkeep.rs`
- `crates/atrium-daemon/tests/gaps_snapshot.rs`
- `crates/atrium-daemon/tests/recovery.rs`
- `crates/atrium-daemon/tests/sessions.rs`
- `docs/contracts/m1.md`

---

## Removed Workarounds

1. **Removed CR-08 resume workaround (`crates/atrium-daemon/src/commands.rs`):** Removed the comment `// The store doesn't give back the effort and preset...` and the fallback to `Preset::Home` with `effort: None`; `resume_thread` now calls `inner.store.launch(thread)` to reconstruct `SessionConfig` and `SessionGate` from stored launch parameters.
2. **Removed CR-10 recovery scan workaround (`crates/atrium-daemon/src/upkeep.rs`):** Removed the `threads(Archived::No, None)` + per-thread `last_event` scan and its stale `Not covered: an archived thread...` doc comment; `upkeep::recover` now queries `store.unfinished_incarnations()` directly (retaining the zero-event `Starting` thread sweep and verifying CR-09 `Signal::Recovered` before `close_incarnation` order).

---

## Checks Not Run & Blockers

1. **`scripts/check-protocol-ts`:** Denied by the worker pre-tool hook (`'scripts/check-protocol-ts' is not on the worker allowlist`).
2. **30-run stability loop & Linux execution for N3:** Multi-iteration shell loops (`for`/`while`) are refused by the worker pre-tool policy, and this environment runs on macOS (`atrium-host/src/supervise.rs` Linux `/proc/<pid>/exe` timing behavior was verified by code inspection rather than executed on Linux; `supervise.rs` is also outside M1-15's owned paths).

---

## Definition of Done Outputs (After Final Code/Contract Edits)

### 1. `cargo fmt --all --check`
- Exit code: `0` (no output)

### 2. `cargo clippy --workspace --all-targets -- -D warnings`
- Exit code: `0`
- Summary output:
  - Checked `atrium-protocol`, `atrium-host`, `atrium-client`, `atrium-agents`, `atrium-core`, `atrium-daemon`, `atrium-app`
  - `Finished 'dev' profile [unoptimized + debuginfo] target(s) in 15.08s`

### 3. `cargo test -p atrium-core -p atrium-daemon -p atrium-client` (+ individual un-truncated crate/lib observations)
All commands exited with code `0` with `0 failed` and `0 ignored`:

- **`atrium-client`:**
  - `unittests src/lib.rs`: `28 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.39s`
  - `tests/socket.rs`: `11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.67s`
- **`atrium-core`:**
  - `unittests src/lib.rs`: `9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s`
  - `tests/attention.rs`: `9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s`
  - `tests/history.rs`: `25 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.65s`
  - `tests/ledger.rs`: `20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s`
  - `tests/policy.rs`: `44 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.31s`
  - `tests/store.rs`: `30 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 47.36s`
  - `Doc-tests atrium_core`: `0 passed; 0 failed; 0 ignored`
- **`atrium-daemon`:**
  - `unittests src/lib.rs` (`--lib`): `15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s`
  - `tests/client.rs`: `1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s`
  - `tests/gaps_full_storage.rs`: `2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.06s`
  - `tests/gaps_snapshot.rs`: `2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.40s`
  - `tests/recovery.rs`: `5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.13s`
  - `tests/requests_fake.rs`: `6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.23s`
  - `tests/requests_notifications.rs`: `2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.29s`
  - `tests/sessions.rs`: `22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.13s`
  - `tests/socket.rs`: `9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 18.98s`
  - `Doc-tests atrium_daemon`: `0 passed; 0 failed; 0 ignored`
- **`Doc-tests atrium_client`:** `0 passed; 0 failed; 0 ignored`

### 4. `scripts/check-protocol-ts`
- **Not run (blocked):** denied by worker pre-tool hook (`'scripts/check-protocol-ts' is not on the worker allowlist`).
