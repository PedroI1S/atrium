# Handoff: M0-04-acp-adapter

Model: argon · Status: done

## Done

- Pinned `agent-client-protocol = { version = "=3.5.0", features = ["unstable"] }` (which pins `agent-client-protocol-schema = "1.12.0"`) in `spikes/agent-acp/Cargo.toml`.
- Implemented `AcpNormalizer` (`spikes/agent-acp/src/normalizer.rs`), a pure `atrium_agents::Normalizer` over `agent_client_protocol::schema::v1` wire records:
  - Maps ACP `initialize`, `session/new`, `session/load`, `session/resume`, `session/prompt`, `session/cancel`, `session/update` (`agent_message_chunk`, `agent_thought_chunk`, `tool_call`, `tool_call_update`, `plan`, `usage_update`), `session/request_permission`, `fs/read_text_file`, `fs/write_text_file`, `terminal/create`, `terminal/output`, `terminal/wait_for_exit`, `terminal/kill`, `terminal/release`, and `elicitation/create` into `atrium_protocol::agent::AgentEvent`.
  - Enforces established session (`SessionStarted` first, `SessionEnded` last) vs pre-session failure (`Error { fatal: true }` then `SessionEnded`, no `SessionStarted`) lifecycles.
  - Enforces exact closing order on process exit (`MessageCompleted` -> `ToolCallFinished { status: Cancelled }` -> `RequestResolved { outcome: Cancelled }` -> `TurnCompleted { outcome: Interrupted | Failed }` -> `SessionEnded`).
  - Emits `AgentEvent::Unmapped` for unmapped ACP updates (`available_commands_update`, `current_mode_update`, `config_option_update`, `session_info_update`) so nothing is silently dropped.
- Implemented `AcpAdapter` and `ClientCapabilityConfig` (`spikes/agent-acp/src/adapter.rs`) implementing `atrium_agents::AgentAdapter`:
  - Reports `Provider::Acp` and `Interception::ProviderGated`.
  - Configurable client capability advertisement (`ClientCapabilityConfig::all()` vs `ClientCapabilityConfig::without_fs_and_terminal()`).
  - Routes `session/request_permission`, `fs/read_text_file`, `fs/write_text_file`, and `terminal/create` through `SessionConfig::gate` before execution (fail-closed).
  - Decouples `session.events` forwarding from the session command loop so `Interrupt` and `Shutdown` never block even when `session.events` (`EVENT_BUFFER = 1024`) is completely full and unread.
  - Bounds startup handshake by `STARTUP_TIMEOUT` (30s) and reaps child processes within `SHUTDOWN_GRACE` (5s).
- Created hand-made fixtures for all 10 scenarios in `fixtures/agents/acp/` (`simple`, `tool-read`, `gate-deny`, `gate-deny-execute`, `approval-allow`, `approval-deny`, `question`, `interrupt`, `multi-turn`, `resume`, `startup-error`), each with `<scenario>.jsonl`, `<scenario>.events.json`, and `<scenario>.md` (marked `hand-made`), verified by `spikes/agent-acp/tests/golden.rs`.
- Implemented live adapter and saturation integration tests in `spikes/agent-acp/tests/live_adapter.rs` (`harness = false`).
- Implemented `spikes/agent-acp/examples/record.rs` (supports `--no-client-caps` and `<scenario>`) and `spikes/agent-acp/examples/chat.rs`.
- Wrote the spike report at `docs/spikes/M0-04-acp-adapter.md`.

## Not done

- Live fixture recording from `opencode acp`: the worker sandbox pre-tool hook blocks running `opencode` directly (`'opencode' is not on the worker allowlist`), as anticipated in the brief. All 10 fixtures are committed as `hand-made` from the `agent-client-protocol` v3.5.0 schema and `opencode acp` source implementation, and `examples/record.rs` is ready for the orchestrator to record live transcripts.

## Definition of Done results

### 1. `cargo fmt --check`
```text
(exit code 0, no output)
```

### 2. `cargo clippy -p spike-agent-acp --all-targets -- -D warnings`
```text
    Checking spike-agent-acp v0.0.0 (/Users/pedro/Documents/atrium-wt/M0-04-acp-adapter/spikes/agent-acp)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.66s
(exit code 0, 0 warnings)
```

### 3. `cargo test -p spike-agent-acp`
```text
     Running unittests src/lib.rs (target/debug/deps/spike_agent_acp-6206e4bb9f7ec163)
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/golden.rs (target/debug/deps/golden-15a9a570192fe260)
running 4 tests
test resolving_unknown_or_duplicate_requests_emits_non_fatal_error ... ok
test message_queue_overflow_emits_non_fatal_error ... ok
test unexpected_exit_closes_open_items_in_contract_order ... ok
test all_fixtures_pass_golden_and_invariants ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s

     Running tests/live_adapter.rs (target/debug/deps/live_adapter-855996ebf62e1043)
running 4 live_adapter tests
test saturation_test ... ok
test live_fs_read_and_gate_deny_test ... ok
test live_approval_and_question_test ... ok
test live_startup_error_test ... ok
test result: ok. 4 passed; 0 failed; 0 ignored
```

## Gotchas

- `agent-client-protocol = "=3.5.0"`'s high-level `ClientSideConnection` requires `!Send` `futures::AsyncRead` / `AsyncWrite` on a `LocalSet` and hides raw wire lines from `Normalizer`. Using `agent_client_protocol::schema::v1` types directly over `tokio::process` stdio lines gives full `Send` compatibility while feeding every `Stdin`/`Stdout` line through `AcpNormalizer` and the fixture recorder.
- In `agent-client-protocol-schema = "1.12.0"`, single-select titled enum options (`EnumOption`) serialize the option ID under `"const"` (`#[serde(rename = "const")]`), not `"value"`.
- Even when advertising `fs` and `terminal` client capabilities, OpenCode executes search (`grep`, `glob`) and MCP tools inside its own process and only prompts via `session/request_permission` when configured to `ask`, so `AcpAdapter::interception()` must report `Interception::ProviderGated`.

## Follow-ups

- Orchestrator can record live `opencode acp` fixtures (with and without client `fs`/`terminal` capabilities) using:
  ```bash
  cargo run -p spike-agent-acp --example record -- simple
  cargo run -p spike-agent-acp --example record -- tool-read
  cargo run -p spike-agent-acp --example record -- gate-deny
  cargo run -p spike-agent-acp --example record -- gate-deny-execute
  cargo run -p spike-agent-acp --example record -- approval-allow
  cargo run -p spike-agent-acp --example record -- approval-deny
  cargo run -p spike-agent-acp --example record -- question
  cargo run -p spike-agent-acp --example record -- interrupt
  cargo run -p spike-agent-acp --example record -- multi-turn
  cargo run -p spike-agent-acp --example record -- resume
  cargo run -p spike-agent-acp --example record -- startup-error
  cargo run -p spike-agent-acp --example record -- --no-client-caps tool-read
  cargo run -p spike-agent-acp --example record -- --no-client-caps gate-deny
  ```
