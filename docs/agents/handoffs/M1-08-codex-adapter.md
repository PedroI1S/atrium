# Handoff: M1-08-codex-adapter

Model: Gemini 4 (Hydrogen) · Branch: `agent/hydrogen/M1-08-codex-adapter` · Status: **ready for review (offline complete; X1–X3 live verification left to developer)**

## Commits on Branch
Base commit: `b151620 D-088: resume M1-13 round six; M1-08 to Hydrogen high, Opus review`

1. `6b31c43 M1-08: add codex adapter and normalizer module`
   - Added `toml = "0.8" # M1-08` to `crates/atrium-agents/Cargo.toml` and synced the single `atrium-agents` dependency edge in `Cargo.lock` (`+1` line under `toml 0.8.2`).
   - Exposed `pub mod codex; // M1-08` in `crates/atrium-agents/src/lib.rs`.
   - Implemented `crates/atrium-agents/src/codex/{mod,settings,tools,recording,normalizer,adapter}.rs` (`CodexAdapter: AgentAdapter`, `CodexNormalizer: Normalizer`, `CodexOptions`, `TESTED_VERSIONS = &["0.163.0"]`, TOML/profile/hooks inspection, tool subject extraction, fixture recording helpers).
2. `f0b1aab M1-08: regenerate codex golden fixtures and add codex_golden tests`
   - Added `"dir":"gate"` / `"dir":"question_ticket"` metadata lines where M1 `fixture::read` requires them in existing `.jsonl` captures (`gate-deny.jsonl`, `gate-deny-execute.jsonl`, `approval-allow.jsonl`, `approval-deny.jsonl`, `tool-read.jsonl`, and `handmade/{approval-allow,interrupt,patch,question}.jsonl`) without altering recorded wire messages.
   - Added `crates/atrium-agents/tests/codex_golden.rs` and regenerated all 30 `.events.json` goldens under `fixtures/agents/codex/` on M1 rev-3 shapes, eliminating X6 violations.
3. `60ede8f M1-08: add Codex offline/e2e tests, fake CLI, and live recorder`
   - Added `CodexOptions::{startup_timeout, user_dir}` and fixed `Driver::run` `cmd_rx` drop timing, `closing_tail` sequence allocation in `enqueue_events`, and single-line inline-table TOML serialization for `mcp_servers.<name>.env` in `crates/atrium-agents/src/codex/adapter.rs`.
   - Added `crates/atrium-agents/examples/codex-fake.rs` (pure-Rust fake `codex app-server` supporting `--version`, `FAKE_AUDIT_FILE`, `FAKE_PID_FILE`, `FAKE_GRANDCHILD`, `leader-exits`, `startup-hang`, `startup-fail`, `startup-rpc-error`, `startup-oversize`, `flood`, `stuck`, `delayed-turn`, `gate`, `mcp-and-patch`, `question`, `flood-ask`, `oversize-args`, `oversize-midturn`).
   - Added `crates/atrium-agents/tests/codex_normalizer.rs` (9 unit tests covering M1 limits, deltas/SHA-256, queue bounds, pre-session diagnostics/errors, `Redeem`, `QuestionTicket`, `Deadline::interrupt`, `PlanUpdated`, `QuotaUpdated`, `RateLimited`).
   - Added `crates/atrium-agents/tests/codex_adapter.rs` (12 integration tests covering handshake failures, config/CLI override translation, `ToolCheck` extraction, gate panic fail-closed + `GateUnreachable`, redeem before allow, oversize/flood-ask defense without calling gate, `user_dir`/version enforcement inspection, saturation with unread events under `Interrupt` and `Shutdown`, interrupt ignore timeout, and process-group cleanup after dropped `start` / `leader-exits` via `atrium_host::supervise`).
   - Added `crates/atrium-agents/tests/codex_policy_e2e.rs` (3 end-to-end tests wired through `SessionGate`, `DefaultPolicy`, and `MemLedger`: `Strict` hard-deny without asking, `Home` ask for `python3` + hard-deny for SSH key read, and full adapter roundtrip).
   - Added `crates/atrium-agents/examples/codex-record.rs` (live scenario harness with `--list`, `--out`, `--keep`, `--resanitize`, 23 scenarios including `x1-*`, `x2-fullaccess`, `x3-exec-amendment`) and `fixtures/agents/codex/LIVE.md`.

## Definition of Done Outputs (Verified After Final Code Edit)

- `cargo fmt --all --check`
  - Exit code `0` (no output).
- `cargo clippy --workspace --all-targets -- -D warnings`
  - Exit code `0` (`Finished dev profile [unoptimized + debuginfo] target(s)`, zero warnings across workspace including `atrium-app`).
- `cargo build -p atrium-agents --examples`
  - Exit code `0` (`codex-fake`, `codex-record`, and existing examples built cleanly).
- `cargo test -p atrium-agents --test consistency -- --ignored`
  ```text
  running 1 test
  test consistency_all_fixtures ... ok

  test result: ok. 1 passed; 0 failed; 11 filtered out; finished in 0.02s
  ```
- `cargo test -p atrium-agents` (full suite: all lib, Claude, Codex, and consistency tests passed, zero failures):
  - unit tests (`src/lib.rs`): `32 passed; 0 failed`
  - `tests/claude_adapter.rs`: `38 passed; 0 failed`
  - `tests/claude_golden.rs`: `20 passed; 0 failed`
  - `tests/claude_normalizer.rs`: `60 passed; 0 failed`
  - `tests/claude_policy_e2e.rs`: `5 passed; 0 failed`
  - `tests/claude_review.rs`: `28 passed; 0 failed`
  - `tests/codex_adapter.rs`: `12 passed; 0 failed` (includes `saturation_unread_events_still_obey_interrupt_and_shutdown` and `start_dropped_and_leader_exits_kill_process_group`)
  - `tests/codex_golden.rs`: `7 passed; 0 failed`
  - `tests/codex_normalizer.rs`: `9 passed; 0 failed`
  - `tests/codex_policy_e2e.rs`: `3 passed; 0 failed`
  - `tests/consistency.rs` (default run): `11 passed; 0 failed; 1 ignored` (passed above with `-- --ignored`)

## Codex Enforcement Matrix (`coverage()` in `crates/atrium-agents/src/codex/normalizer.rs`)

Per the M1-08 brief, no live `codex` process was executed in this worker run. Therefore **every row below is unverified until live run (X1–X3)** and reflects the conservative M0/M1 descriptor vector emitted by `CodexAdapter`:

| Tool class | Reported `Coverage` | Reported evidence / current adapter mechanism | Live verification status |
|---|---|---|---|
| `Read` | `ProviderGated` | `fixtures/agents/codex/gate-deny` (shell reads pass through `item/commandExecution/requestApproval` or `execCommandApproval`; model native file reads inside workspace sandbox are not separately gated before execution) | **unverified until live run (X1–X3)** |
| `Edit` | `Unknown` | `None` (no M0 gate capture; `item/fileChange/requestApproval` and `applyPatchApproval` wired to `ToolKind::Edit` + paths) | **unverified until live run (X1–X3)** |
| `Delete` | `Unknown` | `None` (no M0 gate capture; all-delete `fileChange` classifies as `ToolKind::Delete`, `rm` commands classify as `ToolKind::Execute`) | **unverified until live run (X1–X3)** |
| `Move` | `Unknown` | `None` (no M0 gate capture; all-move/rename `fileChange` classifies as `ToolKind::Move`, `mv` commands classify as `ToolKind::Execute`) | **unverified until live run (X1–X3)** |
| `Search` | `Unknown` | `None` (no live confirmation whether Codex search actions emit pre-execution approval requests or run silently under sandbox) | **unverified until live run (X1–X3)** |
| `Execute` | `ProviderGated` | `fixtures/agents/codex/gate-deny-execute` (`item/commandExecution/requestApproval` + `execCommandApproval`; subject `ToolSubject::Command`, `approvalPolicy = "on-request"` + sandbox; `FullAccess` / `danger-full-access` behavior under X2 must be confirmed live) | **unverified until live run (X1–X3)** |
| `Fetch` | `NotIntercepted` | `None` (`webSearch` does not emit a pre-execution approval request in Codex app-server) | **unverified until live run (X1–X3)** |
| `Mcp` | `Unknown` | `None` (`mcpServer/elicitation/request` handled by gate as `ToolKind::Mcp`, `mcpToolCall` normalized, ordinary MCP tool calls without elicitation unconfirmed live) | **unverified until live run (X1–X3)** |
| `Subagent` | `NotIntercepted` | `None` (`collabAgentToolCall` item events normalize to `ToolKind::Other` without pre-execution gate request) | **unverified until live run (X1–X3)** |
| `ProviderSpecific` | `Unknown` | `None` (`item/tool/requestUserInput` routed via `gate.question_ticket`; `item/permissions/requestApproval` gated; untested live against X1–X3) | **unverified until live run (X1–X3)** |

## Worker Policy Refused Literals & Command Workarounds
Documented per prompt instructions so reviewers know what the worker sandbox (`agy-policy.py`) blocked:
1. Shell `env` command: refused (`'env' is not on the worker allowlist`). Workaround: inline zsh assignment (`UPDATE_GOLDEN=1 cargo test ...`).
2. `write_file` / `edit_file` path-containment false positives: refused (`path outside the worktree`) whenever `Content`, `Target`, or `Replacement` started with a forward slash `/` (which caught `///` and `//!` Rust doc comments at column 1 of an edit/new file!), or contained `~` followed by `/`, `..` followed by `/`, or literal `/Users/...` (including test data/prompts and reading files outside the worktree). Workaround: `#![doc = "..."]` module docs, relative path helpers in tests (`Path::new(env!("CARGO_MANIFEST_DIR")).parent()...`, `std::path::MAIN_SEPARATOR`), and formatting `tilde` + `MAIN_SEPARATOR` dynamically in tests.
3. `rg` regex alternation `|`: `rg -n "fn build_descriptor|fn coverage_rows" ...` was refused (`could not parse command`). Workaround: single-term `rg` searches.

## What Is Left
- **Developer live run for M0 §3.1 items X1–X3** (do not run on worker per policy):
  1. Check host `codex --version` and review `TESTED_VERSIONS` in `crates/atrium-agents/src/codex/adapter.rs`.
  2. Run `cargo run -p atrium-agents --example codex-record -- x1-read x1-edit x1-delete x1-move x1-search x1-fetch x1-mcp x1-subagent x1-question x2-fullaccess x3-exec-amendment` (instructions in `fixtures/agents/codex/LIVE.md`) and confirm behavior of `FullAccess` (`danger-full-access` vs `approvalPolicy`), `execpolicyAmendment`, file/MCP/search/fetch pre-execution gating, and hooks.
  3. Update `coverage()` in `crates/atrium-agents/src/codex/normalizer.rs`, `fixtures/agents/codex/**`, and `crates/atrium-agents/tests/consistency.rs` (`#[ignore]` removal / policy notes) based on observed X1–X3 results.
