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
