# Handoff: M0-04-acp-adapter

Model: argon (round 1 fixes: Sonnet 5.5) · Status: done, fix round 2 applied; re-recording of the round-2 changes pending

## Fix round 2 (Sonnet)

Input: the orchestrator's re-recording with round-1 code (commit 114a35c: live fixtures, OpenCode 2.1.3, `opencode/deepseek-v4.1-flash-free`). Round 1's `ask` config works: read, shell, edit, search and question reach the gate, every `gate-deny*` is denied (with and without client caps), the canary never appears, `model-select`, `startup-error` and the approvals work. Details in `docs/spikes/M0-04-acp-adapter.md` (sections 4 and 6).

1. **Security finding: OpenCode's `execute` tool (Code Mode) bypasses the gate.** It runs model-written JavaScript with a raw `fetch` and nested tool calls; in `tool-fetch` it completed with no `session/request_permission` despite `* * ask`. From the 2.1.3 binary and the v2 permission docs: the only documented control is that a `deny` on action `execute` removes Code Mode. So it is **disabled per session, not gated**: `{execute * deny}` is added after the ask rule (global and on `explore`/`general`). Because OpenCode reaches MCP tools only through Code Mode by default, `SessionConfig::mcp_servers` are now put in the injected config as `mcp.servers.<name>` with `codemode: false` (direct, gated tools) and are no longer sent in ACP `session/new` while injecting. Neither the deny nor `codemode: false` is proven live yet.
2. **Recorder**: in every scenario it now exits 3 (fixtures still written) if any tool call completed without reaching the gate (`detect_ungated_completions`); new `tool-execute` probe scenario. Golden tests run the same check over every scenario and pin it on the real `tool-fetch` evidence.
3. **Resume lifecycle**: `session/load` replays the old conversation as `session/update`s before answering. Decision: replayed history is not turned into events; it is counted and reported as one `Unmapped { record_type: "session/load:history_replay" }` right after `SessionStarted` (raw lines stay in the fixture). More generally, `session/update`s that arrive before the session is established are held until after `SessionStarted`, or dropped if the session never starts (so a pre-session failure begins with its `Error`; this also fixes the live `startup-error` golden, which began with an `Unmapped`). `resume.events.json` and `startup-error.events.json` were re-normalized offline with `examples/replay.rs --write`; nothing was re-recorded. The golden `resume` check now compares with the id in the fixture's `session/load` request (live ids are random).
4. **Matrix** filled from the live `.md` files; the report also records that a reject makes OpenCode end the turn (`stopReason: "cancelled"` -> `TurnOutcome::Interrupted`, live in `gate-deny`, `gate-deny-execute`, `approval-deny`).

`interception()` stays `ProviderGated`.

**Re-record (orchestrator)**: only these change with round 2 (the others are unaffected, but re-running all is harmless). From the worktree root:
```bash
cd /Users/pedro/Documents/atrium-wt/M0-04-acp-adapter
cargo build -p spike-agent-acp --examples
for s in tool-fetch tool-execute; do
  cargo run -q -p spike-agent-acp --example record -- $s || echo "!! $s exited $?"
done
# optional full set:
for s in simple tool-read gate-deny gate-deny-execute approval-allow approval-deny question interrupt multi-turn resume startup-error model-select tool-search; do
  cargo run -q -p spike-agent-acp --example record -- $s || echo "!! $s exited $?"
done
cargo run -q -p spike-agent-acp --example record -- --no-client-caps gate-deny
cargo run -q -p spike-agent-acp --example record -- --no-client-caps gate-deny-execute
cargo test -p spike-agent-acp
```
Exit 3 anywhere means a tool call ran without the gate. Expected now: `tool-fetch` uses `webfetch` (gated) or no tool; `tool-execute` shows a model with no `execute` tool. If `tool-execute` still shows an `execute` call, the deny does not work and `execute` is ungatable on ACP; the M1 HTTP-adapter recommendation applies. If the golden test then complains that `live_tool_fetch_evidence_is_flagged_by_the_ungated_check` expects a finding, check that `tool-fetch.md` still says "did not reach the gate" (the test follows the md).

DoD after round 2: `cargo fmt --check`, `cargo clippy -p spike-agent-acp --all-targets -- -D warnings` clean; `cargo test -p spike-agent-acp`: 22 unit, 12 golden, 9 mock-agent tests pass.

## Fix round 1 (Sonnet)

Input: the orchestrator's first live recording (`.orchestration/evidence/M0-04-live-1`, not committed) and two pointers (MonoCode's OpenCode HTTP integration, MIT; the ACP docs). Details and the matrix are in `docs/spikes/M0-04-acp-adapter.md`.

**Security finding (escalation case, reported)**: with its default permissions OpenCode bypasses the gate: it read `secret.txt` and ran `cat secret.txt` natively (canary in the stream), never called `fs/*` or `terminal/*` although advertised, and never sent `session/request_permission`.

What changed:
1. **Gating**: `AcpAdapter::new()` spawns `opencode acp` with a per-session generated config (`OPENCODE_CONFIG_CONTENT` inline, plus an `OPENCODE_CONFIG` file removed at session end) holding the rule `* * ask` (covers reads), the same on the `explore`/`general` subagents, and the model. OpenCode's ACP `session/new` has no permission parameter, so per-process environment is the only per-session lever; user config files are untouched. `session/request_permission` goes to the gate; Allow = `once`, Deny = `reject`, an agent-side `allow_always` is never chosen. `interception()` stays `ProviderGated`: the `ask` path is documented and visible in the 2.1.3 binary but **unproven live**, and MCP/subagent internals are unprobed. Per-class matrix with caveats in the report.
2. **Model**: `SessionConfig::model` is now selected over ACP (`session/set_config_option`, or `session/set_model`), verified, and `SessionStarted` waits for it. `effort` is applied through OpenCode's `effort` config option; `ReadOnly` selects the `plan` mode. Pinned by `model_*` tests (golden and mock-agent).
3. **Startup**: `STARTUP_TIMEOUT` covers `initialize`, session creation and model selection (`startup_timeout_test`); a bad model gives the pre-session lifecycle. The 27-minute hang was the recorder (no prompt, no limit): it now has a per-scenario timeout (`ATRIUM_ACP_SCENARIO_TIMEOUT_SECS`, default 240).
4. **Sanitizer**: `FixtureSanitizer` (`src/sanitize.rs`) maps the scratch dir and macOS temp roots; `is_clean` fails on a surviving temp path; tested, and checked over every committed fixture.
5. **Recorder self-check**: for `gate-deny*`, exit code 3 and `GATE BYPASSED: record as ungated in the matrix` if the canary shows in the raw stream, a denied-class call completed, or calls completed without the gate. Fixtures are still written as evidence. The recorder also makes its own scratch dir, really resumes in `resume`, interrupts a running command in `interrupt`, and adds `model-select`, `tool-search` and `tool-fetch` probes. New `examples/replay.rs`.
6. Normalizer: OpenCode's top-level tool `name` is kept (`shell`, not `execute`); the real tool input (sent only with the permission request) becomes a `ToolCallUpdated`.
7. Fixtures (hand-made, shaped after the live recording): `gate-deny`, `gate-deny-execute`, `tool-read`, `startup-error` rewritten; `model-select` added; the old fs/terminal shapes kept as `gate-deny-fs`, `gate-deny-terminal`, `tool-read-fs`.

M1 recommendation (in the report): keep the generic ACP adapter; plan an OpenCode-specific adapter over `opencode serve` (per-session rules at `POST /session`, model/variant per prompt) unless the re-recording shows ACP gates every class. Not built.

**Live re-record (orchestrator)**, from the worktree root (the recorder creates its own scratch dir; exit 3 = gate bypassed, fixtures still written):
```bash
cd /Users/pedro/Documents/atrium-wt/M0-04-acp-adapter
cargo build -p spike-agent-acp --examples
for s in simple tool-read gate-deny gate-deny-execute approval-allow approval-deny question interrupt multi-turn resume startup-error model-select tool-search tool-fetch; do
  cargo run -q -p spike-agent-acp --example record -- $s || echo "!! $s exited $?"
done
cargo run -q -p spike-agent-acp --example record -- --no-client-caps gate-deny
cargo run -q -p spike-agent-acp --example record -- --no-client-caps gate-deny-execute
cargo test -p spike-agent-acp
```
Then fill the last column of the enforcement matrix from each recording's `.md` ("Tool calls ... reached the gate").

Definition of Done after round 1: `cargo fmt --check` clean; `cargo clippy -p spike-agent-acp --all-targets -- -D warnings` clean; `cargo test -p spike-agent-acp`: 20 unit, 9 golden, 9 mock-agent (`live_adapter`) tests pass. `Cargo.lock` has uncommitted changes from the original task (new dependencies); it is not an owned path, left for the orchestrator.

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

- Orchestrator: re-record live with the commands in "Fix round 1 (Sonnet)" below; the fixtures committed now are hand-made.
