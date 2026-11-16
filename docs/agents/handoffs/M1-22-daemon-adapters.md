# Handoff: M1-22-daemon-adapters

Branch: `agent/hydrogen/M1-22-daemon-adapters`

## Change

- **`crates/atrium-daemon/src/main.rs` (`run()`):**
  Populated `options.adapters` before `Daemon::start(options)` with both native adapters:
  - `Arc::new(ClaudeAdapter::new(ClaudeOptions::default()))`
  - `Arc::new(CodexAdapter::new())`

### Constructor & CLI resolution review

- **`ClaudeAdapter::new(ClaudeOptions::default())` (`crates/atrium-agents/src/claude/adapter.rs`, `settings.rs`):**
  - **CLI resolution:** `program_of(&SessionConfig)` uses `config.program` if `Some`, otherwise `"claude"` resolved via `PATH`. `crates/atrium-daemon/src/{commands.rs,sessions.rs}` leave `program: None` and `env: Vec::new()`, so `atriumd` resolves `claude` from the daemon's `PATH`.
  - **Why `ClaudeOptions::default()` is right for `atriumd`:**
    - `setting_sources: None`: keeps Claude Code's native user, project, and local settings sources (contract §3, D-017) and inspects them in `settings::inspect`.
    - `extra_args: Vec::new()`, `gate_deadline: Duration::from_secs(20)`, `managed_settings: None` (uses platform default managed policy paths), `tap: None` (production, no fixture recording).
    - `settings::inspect` resolves `CLAUDE_CONFIG_DIR` or `$HOME/.claude` from `config.env` with fallback to `std::env::var`, which matches `atriumd`'s `HOME` in `main.rs`.
- **`CodexAdapter::new()` (`crates/atrium-agents/src/codex/adapter.rs`, `settings.rs`):**
  - **CLI resolution:** `resolve_program(&SessionConfig)` uses `config.program` if `Some`, otherwise `"codex"` resolved via `PATH`.
  - **Why `CodexAdapter::new()` (`CodexOptions::default()`) is right for `atriumd`:**
    - `profile: None`, `extra_args: Vec::new()`, `startup_timeout: None` (defaults to `STARTUP_TIMEOUT`, 30s), `user_dir: None` (inspects `$CODEX_HOME` or `$HOME/.codex` from env rather than hardcoding `$HOME/.codex`), `tap: None`.

## Test (`crates/atrium-daemon/tests/binary_adapters.rs`)

- Added `atriumd_registers_claude_and_codex_adapters`, which spawns the built `CARGO_BIN_EXE_atriumd --dev-socket` in a private `ATRIUM_HOME` with `HOME` pointing to a temp directory, `PATH` pointing to an empty temp directory, and `CLAUDE_CONFIG_DIR` / `CODEX_HOME` removed from the environment (so no real provider CLI can ever be found or launched).
- Disables `notify_failed` so missing-CLI thread failures do not post OS desktop notifications, sends `ProbeProviders { preset: Preset::Home }` via `tests/common/`'s `Wire`, and asserts both `Provider::Claude` and `Provider::Codex` are returned with `cli_version: None`, `level: EnforcementLevel::Partial`, and `error: None`.
- Sends `StartThread` with `AgentLaunch::Native` for both `Provider::Claude` and `Provider::Codex`, verifies neither returns `"no adapter for ... in this daemon"`, waits for each thread to reach `ThreadState::Failed`, and asserts each thread's history contains the adapter's `"failed to start the provider"` fatal `AgentEvent::Error`.
- Stops the daemon cleanly via `ShutdownDaemon { force: false }` and waits for exit (with a `Drop` guard that kills and reaps the child if the test panics).

### Failing run on `dev` (before `main.rs` fix)

Command: `cargo test -p atrium-daemon --test binary_adapters`
Exited `101`:
```text
running 1 test
test atriumd_registers_claude_and_codex_adapters ... FAILED

failures:

---- atriumd_registers_claude_and_codex_adapters stdout ----

thread 'atriumd_registers_claude_and_codex_adapters' (42784773) panicked at crates/atrium-daemon/tests/binary_adapters.rs:129:5:
assertion `left == right` failed: expected both Claude and Codex in ProbeProviders, got []
  left: []
 right: [Claude, Codex]
```

### Passing run (with `main.rs` fix)

Command: `cargo test -p atrium-daemon --test binary_adapters`
Exited `0`:
```text
running 1 test
test atriumd_registers_claude_and_codex_adapters ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.53s
```

### 20 consecutive runs

Command: `for i in {1..20}; do cargo test -p atrium-daemon --test binary_adapters -q && echo "RUN $i OK" || echo "RUN $i FAIL"; done`
Exited `0`: `RUN 1 OK` through `RUN 20 OK` (all 20 runs `1 passed; 0 failed`, 0.39s–0.42s each).

## `Options` comparison (`crates/atrium-daemon/src/lib.rs`)

| Field | `Options::new(paths, home)` | `main.rs` (`run()`) | `tests/common/mod.rs` / other tests | Gap? |
|---|---|---|---|---|
| `paths: Paths` | argument | `default_paths()?` (`ATRIUM_HOME` in debug, OS paths in release) | temp folder `paths(folder.path())` | No |
| `home: PathBuf` | argument | `$HOME` (non-empty) | `folder.path().join("home")` | No |
| `dev_socket: bool` | `false` | `--dev-socket` CLI flag (debug builds only) | `true` (tests connect as `ClientRole::Dev`) | No |
| `app_identity: AppIdentitySource` | `None` | `File(paths.app_identity)` in `debug_assertions`; `None` in release | `Fixed(...)` / `File(...)` in `socket.rs`, `adversarial_socket.rs` | No (M8 packaging owns release identity) |
| `adapters: Vec<Arc<dyn AgentAdapter>>` | `Vec::new()` | **Was missing (`Vec::new()`)** -> **Fixed:** `[ClaudeAdapter, CodexAdapter]` | `[Scripted]` (or slow mock) | **Yes (fixed)** |
| `notifier: Arc<dyn Notifier>` | `Arc::new(NoNotifier)` | `Arc::new(CommandNotifier)` | `Arc::new(Notes)` | No |
| `store: StoreOptions` | `StoreOptions::default()` (real `statvfs`) | `StoreOptions::default()` | overrides `free_space` atomic seam | No (production default) |
| `manage_service: bool` | `false` | `true` | `false` (must not alter user's launchd/systemd) | No |
| `connection_queue_bytes: usize` | `MAX_CONNECTION_QUEUE_BYTES` | `MAX_CONNECTION_QUEUE_BYTES` | overridden to small `LANE` in `socket.rs` | No (test seam) |
| `snapshot_check_every: Duration` | `10 * 60` s | `10 * 60` s | `50 ms` / `150 ms` in `gaps_snapshot.rs`, `sessions.rs` | No (test seam) |
| `store_wrapper` | `None` | `None` | fault-injecting wrapper in `gaps_snapshot.rs` | No (test seam) |

No other missing `Options` wiring gaps were found in `main.rs`.

## Definition of Done outputs

- `cargo fmt --all --check`: exited `0`.
- `cargo clippy --workspace --all-targets -- -D warnings`: exited `0`.
- `cargo test -p atrium-daemon --test binary_adapters` (20 consecutive runs): `20/20` passed (`1 passed; 0 failed` per run).
- `cargo test --workspace --no-fail-fast` (and `-- -q` for summary): exited `0`, all active workspace tests passed (0 failed).
  - `atrium_agents`: unit `35`, `claude_adapter` `38`, `claude_golden` `20`, `claude_normalizer` `61`, `claude_policy_e2e` `5`, `claude_review` `28`, `codex_adapter` `16`, `codex_golden` `7`, `codex_normalizer` `10`, `codex_policy_e2e` `3`, `consistency` `12` (`235 passed, 0 failed`).
  - `atrium_app`: unit `45`, `delivery_shapes` `2` (`47 passed, 0 failed`).
  - `atrium_client`: unit `29`, `socket` `11` (`40 passed, 0 failed`).
  - `atrium_core`: unit `9`, `adversarial_fuzz` `2 passed / 1 ignored`, `adversarial_gate` `9 passed / 2 ignored`, `adversarial_ledger` `14`, `adversarial_ordinary` `16`, `adversarial_policy` `15 passed / 3 ignored`, `adversarial_regression` `57`, `attention` `9`, `history` `25`, `ledger` `20`, `policy` `44`, `store` `30` (`250 passed, 0 failed, 6 ignored` pre-existing `M1-13` `#[ignore]` tests).
  - `atrium_daemon`: unit `15`, `adversarial_recovery` `3`, `adversarial_socket` `7`, `binary_adapters` `1`, `client` `1`, `gaps_full_storage` `2`, `gaps_snapshot` `2`, `recovery` `7`, `requests_fake` `6`, `requests_notifications` `2`, `sessions` `22`, `socket` `9`, `start_refusal` `5` (`82 passed, 0 failed`).
  - `atrium_host`: unit `5`, `adversarial_supervise` `7 passed / 1 ignored` (Linux-only test ignored on macOS), `lifecycle` `5`, `peer` `3`, `supervise` `15` (`35 passed, 0 failed, 1 ignored`).
  - `atrium_protocol`: `serde_shape` `13` (`13 passed, 0 failed`).

## What was left

- Nothing within `M1-22-daemon-adapters` scope.
- Pre-existing out-of-scope items remain as on `dev`: release `AppIdentitySource::None` in `crates/atrium-daemon/src/main.rs` (M8 packaging), the 6 pre-existing `M1-13` `#[ignore]` tests in `atrium-core`, and the Linux-only `#[ignore]` test in `atrium-host/tests/adversarial_supervise.rs` on macOS.

## Fix round 1 (D-108: GUI/service `PATH` extension & review fixes)

### Changes

- **`crates/atrium-daemon/src/lib.rs` (`provider_path`, `DEFAULT_PATH_EXTRAS`, `NO_PATH_EXTRAS_ENV`):**
  - Added `provider_path(existing: Option<&OsStr>, home: &Path, candidates, exists: impl Fn(&Path) -> bool) -> OsString`: keeps every non-empty entry from `existing` first in order, then appends each candidate (relative paths joined onto non-empty `home`; absolute paths used as-is) for which `exists` returns `true` and that is not already present. De-duplicates entries (including trailing-slash variants via `Path` component equality) while preserving the first occurrence's raw bytes, including non-UTF-8 entries.
  - `DEFAULT_PATH_EXTRAS` order places `$HOME` directories first and system/Homebrew directories after: `$HOME/.local/bin`, `$HOME/.claude/local`, `$HOME/.npm-global/bin`, `$HOME/.bun/bin`, `$HOME/.cargo/bin`, `/opt/homebrew/bin`, `/usr/local/bin`. Because `$HOME/.local/bin` precedes `/opt/homebrew/bin` and `/usr/local/bin`, user-installed CLIs (and the test's fake `claude`) are resolved ahead of any system/Homebrew binary when inherited `PATH` is minimal.
- **`crates/atrium-daemon/src/main.rs` (`extend_provider_path`):**
  - Called in `main()` right before `tokio::runtime::Builder::new_multi_thread()` while the process is single-threaded, with a `// SAFETY:` comment and tightly scoped `#[allow(unsafe_code)]` around `extend_provider_path` for `std::env::set_var("PATH", &merged)` (unsafe in Rust edition 2024).
  - Checks `ATRIUM_NO_PATH_EXTRAS=1` (`NO_PATH_EXTRAS_ENV`): when set to `"1"`, skips extending `PATH`.
  - **Why `ATRIUM_NO_PATH_EXTRAS=1` is safe to ship:** it is strictly subtractive/opt-out — it only prevents `atriumd` from appending extra directories to its inherited `PATH` and cannot grant access to or execute any binary that was not already on `PATH`.
- **`crates/atrium-daemon/tests/binary_adapters.rs`:**
  - Redirected `atriumd`'s stderr to a file (`atriumd.stderr`) inside each test's temp directory instead of `Stdio::piped()`, avoiding pipe-buffer blocking if a daemon writes to stderr (review minor).
  - Updated `atriumd_registers_claude_and_codex_adapters` to set `ATRIUM_NO_PATH_EXTRAS=1` while keeping `PATH` pointing to the empty temp directory, so it still can never reach any real CLI.
  - Added `atriumd_discovers_claude_in_home_local_bin_when_path_minimal`: runs `atriumd` with `PATH=<empty temp dir>`, `HOME=<temp home>`, `ATRIUM_NO_PATH_EXTRAS` unset, and a minimal fake shell script at `$HOME/.local/bin/claude` (mode `0700`) that prints `9.9.9 (Claude Code)` and exits `0` only when invoked with `--version` (matching `crates/atrium-agents/src/claude/adapter.rs::cli_version`) and exits `1` otherwise. Sends only `ProbeProviders { preset: Preset::Home }` (never starts a thread) and asserts only on Claude's `cli_version == Some("9.9.9")` so the test does not depend on what may exist in `/opt/homebrew/bin` or `/usr/local/bin`.

### Unit tests (`crates/atrium-daemon/src/lib.rs`)

`cargo test -p atrium-daemon --lib provider_path` (`6 passed; 0 failed`):
- `provider_path_keeps_existing_first_and_appends_in_order`
- `provider_path_skips_missing_dirs`
- `provider_path_deduplicates_including_trailing_slashes`
- `provider_path_handles_unset_and_empty_path`
- `provider_path_preserves_non_utf8_entries`
- `provider_path_skips_home_candidates_when_home_empty`

### Binary test before and after `main.rs` fix

- **Failing run without `extend_provider_path` in `main.rs`:**
  Command: `cargo test -p atrium-daemon --test binary_adapters` (exited `101`, 1 passed / 1 failed):
  ```text
  thread 'atriumd_discovers_claude_in_home_local_bin_when_path_minimal' (43024311) panicked at crates/atrium-daemon/tests/binary_adapters.rs:265:5:
  assertion `left == right` failed
    left: None
   right: Some("9.9.9")
  ```
- **Passing run with `extend_provider_path` in `main.rs`:**
  Command: `cargo test -p atrium-daemon --test binary_adapters` (exited `0`, `2 passed; 0 failed`).

### Fix round 1 DoD outputs

- `cargo fmt --all --check`: exited `0`.
- `cargo clippy --workspace --all-targets -- -D warnings`: exited `0`.
- `cargo test --workspace --no-fail-fast`: exited `0` (all workspace unit, integration, and doc tests passed; pre-existing `#[ignore]` counts unchanged).
- `cargo test -p atrium-daemon -- -q`: exited `0`: unit `21`, `adversarial_recovery` `3`, `adversarial_socket` `7`, `binary_adapters` `2`, `client` `1`, `gaps_full_storage` `2`, `gaps_snapshot` `2`, `recovery` `7`, `requests_fake` `6`, `requests_notifications` `2`, `sessions` `22`, `socket` `9`, `start_refusal` `5` (`89 passed, 0 failed`).
- Both binary tests 20 consecutive runs (`for i in 1 2 ... 20; do cargo test -p atrium-daemon --test binary_adapters -q && echo "RUN $i OK" || echo "RUN $i FAIL"; done`): `RUN 1 OK` through `RUN 20 OK` (`20/20` passed, `2 passed; 0 failed` per run, `0.41s`–`0.44s` each).

### Follow-ups

- **Login shell `PATH` discovery (deferred from this round):** CLIs managed via `nvm`, `asdf`, `mise`, or custom shell init live outside the fixed `DEFAULT_PATH_EXTRAS` directories. A follow-up can merge the user's login shell `PATH` (e.g., `$SHELL -ilc` with a short timeout) before the runtime starts.
