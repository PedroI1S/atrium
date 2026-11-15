# Handoff: M1-17-daemon-start-refusal

Branch: `agent/hydrogen/M1-17-daemon-start-refusal`

## Where the reason was lost (and evidence)

1. **Client start-up loop discarded `start_daemon` errors (`crates/atrium-client/src/connection.rs`):**
   `run()` previously executed `let _ = tokio::task::spawn_blocking(move || start()).await;`, ignoring any `Err` returned by `start_daemon` (including `open_private_log`'s `secure_dir` refusal on `logs_dir` from `start_daemon_hook` in `app/src-tauri/src/bridge.rs`) and continuing the reconnect loop silently. Because `open_private_log` refused before `cmd.spawn()`, no daemon was spawned at all.
2. **Detached launcher returned immediately without observing early exit (`crates/atrium-host/src/lifecycle.rs`, `app/src-tauri/src/bridge.rs`):**
   `spawn_detached` returned `Ok(pid)` right after `cmd.spawn()`, so if `atriumd` started and immediately exited with a refusal on stderr/log (for example, `InstanceLock::acquire` or `secure_dir` on `runtime_dir`/`data_dir`), the launcher reported `Ok(())`. In addition, `open_private_log` fails when `logs` is mode `0755` or a symlink, so without a piped-stderr fallback during the start-up wait, any refusal written by `atriumd` had nowhere to go if `spawn_detached` ran.
3. **Daemon start-up checks (`crates/atrium-daemon/src/lib.rs`, `crates/atrium-host/src/lifecycle.rs`):**
   On `dev`, `atriumd` itself starts with a `0755` `logs` folder (it only checked `secure_dir` on `runtime_dir` and `data_dir`); the silence N9 saw came from the launcher (`open_private_log` / the hook's `secure_dir` refused, the client discarded the `Err`, and no daemon was spawned). Adding `secure_dir(&paths.logs_dir)` in `Daemon::start` is therefore a new daemon-side refusal, kept by Pedro's decision D-095. `LifecycleError` variants for symlinks, non-regular files, and `AlreadyRunning` were also updated to state the expected state in every message.

Evidence is exercised in:
- `crates/atrium-daemon/tests/start_refusal.rs`:
  - `loose_0755_logs_dir_refusal_reaches_stderr_and_client`
  - `symlinked_logs_dir_refusal_reaches_stderr_and_client`
  - `held_instance_lock_refusal_reaches_stderr_and_client`
- `crates/atrium-host/tests/lifecycle.rs`:
  - `spawn_detached_wait_returns_stderr_on_early_exit_and_pipes_when_log_unopenable`

## What the user now sees

- **Daemon (`atriumd`):** `Daemon::start` validates `paths.logs_dir` with `secure_dir` alongside `runtime_dir` and `data_dir`. Any `LifecycleError` from `secure_dir` or `InstanceLock::acquire` is formatted on one line naming the path (or lock), what is wrong (mode, owner, symlink, file type, or running pid), and what is expected (`expected 0700`, `expected a 0700 directory`, `expected a 0600 regular file`, `expected only one instance`), printed to stderr by `atriumd` and exiting non-zero.
- **Launcher (`atrium-host` + `app/src-tauri/src/bridge.rs`):** `start_daemon_hook` calls `lifecycle::spawn_detached_wait` with a 500ms start-up wait for `paths.socket`.
  - When `atriumd.log` can be opened privately (`0600`, `O_NOFOLLOW`), if `atriumd` exits during the start-up wait, `spawn_detached_wait` returns the last non-empty lines written to `atriumd.log` during that launch.
  - When `atriumd.log` cannot be opened, `spawn_detached_wait` captures stderr through a pipe during the start-up wait and returns the daemon's stderr refusal if it exits (or kills the process and returns the log-open error if it stays alive without a private log).
- **App / Client (`crates/atrium-client/src/connection.rs`):** `start()` errors are no longer discarded; they transition the client into `ConnectionState::Failed { message }` and pause retries until `Control::Reconnect` or `Control::Shutdown`, surfacing the refusal in the existing connection-failure UI without new UI.

## Changes made

- `crates/atrium-host/src/lifecycle.rs`: updated `LifecycleError` messages to include expected mode/type/instance phrasing; refactored `detached_command` and `reap_in_background` (keeping the `unsafe { pre_exec(...) }` block unchanged); added `spawn_detached_wait` and tail helpers.
- `crates/atrium-daemon/src/lib.rs`: added `secure_dir(&paths.logs_dir)?;` in `Daemon::start`.
- `app/src-tauri/src/bridge.rs`: switched `start_daemon_hook` to `lifecycle::spawn_detached_wait(..., &socket, Duration::from_millis(500))`.
- `crates/atrium-client/src/connection.rs`: propagated `start_daemon` failures to `ConnectionState::Failed { message }` and waited for `Reconnect` / `Shutdown`.
- `crates/atrium-host/tests/lifecycle.rs`: added `spawn_detached_wait_returns_stderr_on_early_exit_and_pipes_when_log_unopenable`.
- `crates/atrium-daemon/tests/start_refusal.rs`: added integration tests for `0755` `logs`, symlinked `logs`, and held `InstanceLock` verifying both `atriumd` stderr and `Client` `ConnectionState::Failed`.

## Definition of Done

- `cargo fmt --all --check`: exited 0.
- `cargo clippy --workspace --all-targets -- -D warnings`: exited 0.
- `cargo test -p atrium-host -p atrium-daemon -p atrium-client`: exited 0
  - `atrium_client` unit tests: 28 passed, 0 failed
  - `atrium_client` `tests/socket.rs`: 11 passed, 0 failed
  - `atrium_daemon` unit tests: 15 passed, 0 failed (`src/main.rs`: 0 tests)
  - `atrium_daemon` `tests/client.rs`: 1 passed, 0 failed
  - `atrium_daemon` `tests/sessions.rs`: 19 passed, 0 failed
  - `atrium_daemon` `tests/socket.rs`: 9 passed, 0 failed
  - `atrium_daemon` `tests/start_refusal.rs`: 3 passed, 0 failed
  - `atrium_host` unit tests: 5 passed, 0 failed
  - `atrium_host` `tests/lifecycle.rs`: 5 passed, 0 failed
  - `atrium_host` `tests/peer.rs`: 3 passed, 0 failed
  - `atrium_host` `tests/supervise.rs`: 9 passed, 0 failed
- `cargo test -p atrium-app`: exited 0
  - `atrium_app` unit tests: 45 passed, 0 failed (`src/main.rs`: 0 tests)
  - `atrium_app` `tests/delivery_shapes.rs`: 2 passed, 0 failed

## Fix round 1 (D-095)

- **F1 (raced daemon start):** `crates/atrium-client/src/connection.rs` now treats transient start errors (`AddrInUse`, `"another daemon is already running"`, or `socket.exists()`) as retryable (`Connecting` with backoff) instead of parking in `ConnectionState::Failed`. Updated `held_instance_lock_refusal_reaches_stderr_and_client_keeps_retrying` and added `held_instance_lock_connects_once_socket_appears` in `crates/atrium-daemon/tests/start_refusal.rs` (verifying that when the lock holder binds the socket shortly after the refused start, the client reaches `ConnectionState::Connected`).
- **F2 (diagnosis & D-095):** Corrected the diagnosis above: `dev`'s `atriumd` starts with a `0755` `logs` folder; the silence came from the launcher (`open_private_log`'s `secure_dir` refused, the client discarded the error, and no daemon was spawned). The new `secure_dir(&paths.logs_dir)` in `Daemon::start` is a new daemon-side refusal, kept by Pedro's decision D-095.
- **F3 (log-unopenable deadline & `try_wait` error):** `crates/atrium-host/src/lifecycle.rs` now returns `"daemon was started and then stopped because its log could not be opened: {log_err}"` when killing the child at the deadline, and reaps/kills the child if `try_wait()` returns `Err` so no child is leaked. Updated the assertion in `crates/atrium-host/tests/lifecycle.rs`.
- **F4 (one-line client message):** `crates/atrium-client/src/connection.rs` adds `one_line_failure` so `ConnectionState::Failed { message }` always receives a single relevant line. `spawn_detached_wait` keeps the full tail (up to 5 lines) in its `io::Error`; `ConnectionState::Failed` only has `message: String` (a wire/TS contract type with no `details` field), so only the one-line summary is stored in `ConnectionState::Failed`. Added unit test `one_line_failure_picks_relevant_single_line`.
- **F5 (real-daemon stderr-pipe fallback test):** Added `default_paths_0755_logs_dir_refuses_open_private_log_and_falls_back_to_stderr_pipe` in `crates/atrium-daemon/tests/start_refusal.rs` using a subprocess with `ATRIUM_HOME` set so `default_paths()` matches the `0755` `logs` folder, `open_private_log` refuses without creating `atriumd.log`, and the piped stderr refusal from `atriumd` reaches `Client` in `ConnectionState::Failed`.

### Fix round 1 verification

- `cargo fmt --all --check`: exited 0.
- `cargo clippy --workspace --all-targets -- -D warnings`: exited 0.
- `cargo test -p atrium-host -p atrium-daemon -p atrium-client -p atrium-app`: exited 0 (`atrium-daemon` `tests/start_refusal.rs`: 5 passed; `atrium-client` unit tests: 29 passed, `tests/socket.rs`: 11 passed; `atrium-host`: 5 + 5 + 3 + 9 passed; `atrium-app`: 45 + 2 passed; `atrium-daemon` unit/other tests: 15 + 1 + 19 + 9 passed).
