# M0-03 Spike Report: Antigravity (`agy`) Stream-JSON Adapter

> **Live Verification Status**: Nothing in this crate has been verified live against `agy` 1.4.7 from inside the worker session because the worker policy blocks invoking `agy`. All unit, golden, bridge, retry-grant, and saturation tests run offline against recorded/hand-made fixtures and local mock processes. Live verification (`--example record`, `--example hook-failures`, `--example chat`) is run by the orchestrator on the host.

## 1. What Was Built (`spikes/agent-agy`)

We built the `spike-agent-agy` crate implementing `atrium_agents::AgentAdapter` (`AgyAdapter`) and `atrium_agents::Normalizer` (`AgyNormalizer`) for Google's Antigravity CLI (`agy` 1.4.7), along with the per-session `PreToolUse` Unix-socket gate bridge and live/offline verification harnesses:

- **`spikes/agent-agy/src/normalizer.rs` (`AgyNormalizer`)**:
  - Pure, I/O-free state machine implementing `Normalizer::on_input(&mut self, input: &AdapterInput) -> Vec<AgentEvent>`.
  - Translates `agy` `--output-format stream-json` records:
    - `init` -> `AgentEvent::SessionStarted { provider: Provider::Agy, native_session_id, model, tools }`.
    - `step_update` (`step_type: "tool"` | `"tool_call"`, `state: "ACTIVE"` | `"RUNNING"` | `"DONE"` | `"ERROR"` | `"CANCELLED"`) -> `ToolCallStarted` and `ToolCallFinished` (`Succeeded`, `Failed`, `Denied`, `Cancelled`).
    - `step_update` (`step_type: "agent_response"`, `text_delta`, `thinking_delta`, `usage`) -> `MessageDelta` / `MessageCompleted` (`MessageKind::Assistant` and `MessageKind::Thinking`) plus cumulative per-turn token `Usage`.
    - `denied_actions` -> `ToolCallFinished { status: ToolStatus::Denied }`.
    - `result` -> `AgentEvent::Usage` + `AgentEvent::TurnCompleted`.
    - `AdapterInput::HookRequest`, `AdapterInput::Gate`, `AdapterInput::HookResponse`, and `AdapterInput::Command` -> `ApprovalRequested`, `QuestionAsked` (for `ask_question`), and `RequestResolved`.
    - Pre-session startup failure (`Exit { code: Some(non_zero) }` before `init`) -> `AgentEvent::Error { fatal: true }` followed by `AgentEvent::SessionEnded` without emitting `SessionStarted`.
    - Established session exit (`Exit`) -> closes open messages, open tool calls (`ToolStatus::Cancelled`), pending requests (`RequestOutcome::Cancelled`), open turns (`TurnOutcome::Interrupted` / `Failed`), and emits `SessionEnded` last.
  - Deterministic recursive JSON key canonicalization (`canonical_json`) so retry grants compare tool arguments canonically regardless of `serde_json`'s `preserve_order` feature.

- **`spikes/agent-agy/src/bridge.rs` (`GateBridge`, `RetryGrantStore`, `ApprovalMode`)**:
  - Creates a per-session private directory with mode `0700` (`create_private_session_dir`) and binds a Unix domain socket (`g.sock`, mode `0600`) with a compact path (`<temp_dir>/agy/s-<pid>-<seq>/g.sock`, ~68 bytes on macOS, safely under `SUN_LEN = 104`).
  - Writes a per-session `hooks.json` (`write_session_hook_config`, mode `0600`) registering a `PreToolUse` command hook with `"matcher": "*"` pointing to `spike-agy-hook --socket <socket_path>`.
  - Writes a per-session MCP configuration (`write_session_mcp_config`, mode `0600`) without touching `~/.gemini`.
  - Evaluates every incoming tool call against `SessionConfig::gate`:
    - `GateDecision::Allow` -> immediately replies `{"decision":"allow"}`.
    - `GateDecision::Deny { reason }` -> immediately replies `{"decision":"deny","reason":...}`.
    - `GateDecision::Ask` -> supports both `ApprovalMode::Block` (holds the hook connection open until `SessionCommand::ResolveApproval` arrives) and `ApprovalMode::DenyAndRetry` (immediately replies `{"decision":"deny"}` and issues a single-use `RetryGrant` bound to `session_id + tool_name + canonical_args` upon `ApprovalDecision::Allow`).
    - Intercepts `ask_question` tool calls into `AgentEvent::QuestionAsked` and unblocks when `SessionCommand::AnswerQuestion` arrives.

- **`spikes/agent-agy/src/bin/spike-agy-hook.rs` (`spike-agy-hook`)**:
  - Standalone helper binary invoked by `agy` on every `PreToolUse` event.
  - Reads the hook JSON from stdin, connects to `--socket <path>`, forwards the request, and prints `{"decision":"allow"|"deny","reason":...}` on stdout.
  - **Fail-closed on every error path**: missing `--socket`, empty/invalid UTF-8/malformed JSON stdin, missing `toolCall.name`, missing socket file, connection refused, early socket close, malformed bridge reply, unknown decision string, or client timeout (`DEFAULT_HOOK_CLIENT_TIMEOUT = 25s`) all print `{"decision":"deny","reason":...}` and exit cleanly.

- **`spikes/agent-agy/src/adapter.rs` (`AgyAdapter`)**:
  - Implements `AgentAdapter` (`provider() == Provider::Agy`, `interception() == Interception::AllToolCalls`).
  - Launches `agy` with `--print= --input-format stream-json --output-format stream-json --print-timeout 0s --mode <accept-edits|plan|auto> --dangerously-skip-permissions` and writes each user turn to stdin as `{"event":"user","message":{"content":"<text>"}}`.
  - Installs the session's `hooks.json` into the child `cwd` `.agents/hooks.json` when absent (`install_cwd_hook_if_absent = true`) and removes it on session cleanup.
  - Non-blocking, backpressure-resilient session actor loop (`run_session_actor`): uses `tokio::select! { biased; ... }` where `commands_rx.recv()` is always polled first and `stdout` / `stderr` / `bridge_inputs` are only polled when `events_tx` has capacity or can reserve a permit. A saturated `session.events` channel (`EVENT_BUFFER = 1024`) never blocks `Interrupt` or `Shutdown`.
  - Enforces `MAX_QUEUED_MESSAGES = 8` for user messages sent while a turn is active, rejecting overflow with `AgentEvent::Error { fatal: false }`.
  - Enforces `SHUTDOWN_GRACE = 5s` (`SIGTERM` -> wait up to grace period -> `SIGKILL` + `wait()` reap) and cleans up the session-private directory on exit.

- **Examples & Fixtures**:
  - `spikes/agent-agy/examples/record.rs`: deterministic generator for all 10 hand-made fixtures (`--generate-handmade all`) and live sanitizer/recorder (`-- <scenario>`). Fails loudly (exits non-zero without writing fixture files) if any scenario other than `startup-error` fails to reach `SessionStarted`.
  - `spikes/agent-agy/examples/chat.rs`: interactive stdin/stdout chat harness over `AgyAdapter`.
  - `spikes/agent-agy/examples/hook-failures.rs`: live and self-test harness for testing `agy`'s behavior when the `PreToolUse` helper binary is missing, exits non-zero, prints invalid JSON, times out, or when `hooks.json` is unreadable/malformed. Runs a mandatory `control-allow` case first (which must see `init`, attempt the tool call, and create `canary.txt`, or else the entire run reports `INCONCLUSIVE` and exits non-zero) and marks any failure case as `fail_closed` **only** if `init` was seen, the tool call was attempted, and `canary.txt` is absent.
  - `fixtures/agents/agy/`: golden `.events.json` for `m0-00-smoke.jsonl` plus `.jsonl`, `.events.json`, and `.md` for all 10 required scenarios (`simple`, `tool-read`, `gate-deny`, `approval-allow`, `approval-deny`, `question`, `interrupt`, `multi-turn`, `resume`, `startup-error`).

---

## 2. Enforcement Matrix (`agy` 1.4.7)

Under `--dangerously-skip-permissions` combined with a `PreToolUse` hook (`"matcher": "*"`), `agy` invokes the `PreToolUse` hook before executing **every** tool call (observed in `m0-00-smoke.jsonl` where `run_command` and `write_file` were both intercepted and denied when the hook returned `{"decision":"deny"}`).

| Tool Class | `agy` Tool Names | Normalized `ToolKind` | Intercepted by `PreToolUse` (`matcher: "*"`) | Gate Enforcement & Notes |
|---|---|---|---|---|
| **Read** | `view_file`, `read_file`, `list_dir`, `read_resource`, `list_resources`, `command_status`, `list_permissions`, `list_plugin_accounts` | `ToolKind::Read` | Yes (`AllToolCalls`) | Routed through `SessionConfig::gate` under all `PermissionPolicy` modes (`Ask`, `ReadOnly`, `FullAccess`). The gate, not the adapter, decides which paths may be read. |
| **Edit / Write** | `write_to_file`, `write_file`, `create_file`, `replace_file_content`, `multi_replace_file_content`, `edit_file`, `sed_file`, `notebook_edit` | `ToolKind::Edit` | Yes (`AllToolCalls`) | Observed in `m0-00-smoke.jsonl` (`step_index: 6`, `write_file` denied by hook -> `state: "ERROR"` with `"tool call denied by pre-tool hook"`). |
| **Delete / Move** | `delete_knowledge`, plus shell `rm`/`mv` via `run_command` | `ToolKind::Delete` (`delete_knowledge`), `ToolKind::Execute` (`run_command`) | Yes (`AllToolCalls`) | `agy` has no standalone file-delete tool; file deletion/renaming goes through `run_command` and is inspected via `ToolCheck.input["CommandLine"]`. |
| **Execute** | `run_command`, `send_command_input`, `notebook_execution`, `run_workflow` | `ToolKind::Execute` | Yes (`AllToolCalls`) | Observed in `m0-00-smoke.jsonl` (`step_index: 2, 4, 8, 10, 12`): allowed commands run; denied commands fail with `status: denied` without executing. |
| **Search** | `grep_search`, `find_by_name`, `search_marketplace` | `ToolKind::Search` | Yes (`AllToolCalls`) | Extracted title from `Query` / `Pattern`. |
| **Fetch / Web / Browser** | `search_web`, `read_url_content`, `open_browser_url`, `read_browser_page`, `browser_*`, `capture_browser_*`, `click_browser_pixel`, `execute_browser_javascript` | `ToolKind::Fetch` | Yes (`AllToolCalls`) | Extracted title from `Url` / `url` / `query`. |
| **MCP** | `call_mcp_tool`, `<server>:<tool>`, `mcp__*` | `ToolKind::Mcp` | Yes (`AllToolCalls`) | `matcher: "*"` matches `call_mcp_tool` and any dynamic MCP tool names before the MCP request is dispatched. |
| **Subagents** | `invoke_subagent`, `define_subagent`, `manage_subagents`, `send_message`, `manage_inbox` | `ToolKind::Other` | Yes (`AllToolCalls`) | Subagent spawns are regular tool calls in `agy` and pass through `PreToolUse` before any subagent starts. |
| **Provider-Specific / Questions** | `ask_question`, `ask_permission`, `ask_custom_permission`, `schedule`, `manage_task`, `generate_image`, `wait`, `wait_5_seconds`, `finish` | `ToolKind::Other` | Yes (`AllToolCalls`) | `ask_question` is intercepted by `GateBridge` to emit `AgentEvent::QuestionAsked` and wait for `SessionCommand::AnswerQuestion`. |

### `PermissionPolicy` Mapping

| `PermissionPolicy` | `agy` CLI Flags | Enforcement Behavior |
|---|---|---|
| `PermissionPolicy::Ask` | `--mode accept-edits --dangerously-skip-permissions` | Every tool call passes through `GateBridge` -> `SessionConfig::gate`. |
| `PermissionPolicy::ReadOnly` | `--mode plan --dangerously-skip-permissions` | Runs in `agy`'s `plan` mode and still routes every tool call through `GateBridge` -> `SessionConfig::gate`. |
| `PermissionPolicy::FullAccess` | `--mode auto --dangerously-skip-permissions` | Runs in `agy`'s `auto` mode (allowed on host per D-011) while keeping `GateBridge` active for every tool call. |

---

## 3. Answers to the Three Open `agy` Questions (PLAN §3.2)

### Question 1: Multi-turn over `--input-format stream-json`
- **Verified `agy` 1.4.7 CLI invocation and stdin shape**:
  - `-p` / `--print` takes a value, so passing `-p --input-format ...` causes `-p` to consume `"--input-format"` as the prompt string. The working launch invocation (verified by the orchestrator in Fix round 1) attaches an empty prompt with `=`:
    `agy --print= --input-format stream-json --output-format stream-json --print-timeout 0s --mode <accept-edits|plan|auto> --dangerously-skip-permissions --model <id>`
  - On stdin, `agy` 1.4.7 requires an `"event": "user"` field (rejecting `{"type":"user",...}` with `"missing the event field"`, and rejecting `"user_message"`, `"user_input"`, and `"message"` as unsupported):
    `{"event":"user","message":{"content":"<text>"}}`
  - While a turn is active (`TurnStarted` emitted, `result` not yet received), subsequent `SendUserMessage` commands queue in a bounded `VecDeque` (`MAX_QUEUED_MESSAGES = 8`). When the current turn's `result` record arrives on stdout (`TurnCompleted`), the next queued user message is popped and written to stdin.
  - Multi-turn survival across multiple `{"event":"user","message":{"content":"..."}}` lines (or fallback via `--conversation <native_session_id>`) will be confirmed by the orchestrator's live `record -- multi-turn` and `record -- resume` runs.

### Question 2: Per-session MCP injection without modifying `~/.gemini`
- **How it works in `AgyAdapter` (pending live verification)**:
  - `agy mcp add` modifies user-global state under `~/.gemini` and must **never** be used by Atrium.
  - `write_session_mcp_config(&session_dir, &config.mcp_servers)` writes a session-scoped `mcp.json` and `settings.json` with mode `0600` inside the mode-`0700` session directory (`{"mcpServers": { ... }}`), and `AgyAdapter` passes `AGY_MCP_CONFIG=<session_dir>/mcp.json` and `ANTIGRAVITY_MCP_CONFIG=<session_dir>/mcp.json` (plus optional `--add-dir <session_dir>`).
  - What still needs a live run: verifying on `agy` 1.4.7 which of `AGY_MCP_CONFIG`, `--add-dir <session_dir>`, or workspace `mcp.json` causes `agy` to load per-session MCP servers into `init.tools`.

### Question 3: Routing approvals back to Atrium (`Block` vs `DenyAndRetry`)
- **Primary mechanism (`ApprovalMode::Block`, pending live verification)**:
  - `write_session_hook_config` sets `"timeout": 300` (300 seconds) on the `PreToolUse` hook entry in `hooks.json`.
  - When `SessionConfig::gate` returns `GateDecision::Ask`, `GateBridge` stores a `oneshot::Sender<HookDecisionOutput>` keyed by `request_id` (`req-1`, `req-2`, and so on) and `AgyNormalizer` emits `AgentEvent::ApprovalRequested` after `AgentEvent::ToolCallStarted`.
  - `spike-agy-hook` blocks on the Unix domain socket until `SessionCommand::ResolveApproval` arrives (`Allow` -> `{"decision":"allow"}`, `Deny` -> `{"decision":"deny","reason":...}`).
  - Client-side fail-closed timeout: `spike-agy-hook` enforces `--timeout-ms` (default 25,000 ms) so that if the user or daemon does not respond in time, `spike-agy-hook` prints `{"decision":"deny","reason":"hook client timed out waiting for Atrium gate bridge"}` before `agy`'s hook timeout expires.
- **Fallback mechanism (`ApprovalMode::DenyAndRetry`, PLAN §6 mitigation)**:
  - Immediately replies `{"decision":"deny","reason":"approval required (req-N): waiting for user approval before retry"}` while surfacing `ApprovalRequested`.
  - When `ResolveApproval { decision: Allow }` arrives, `GateBridge` inserts a single-use `RetryGrant` into `RetryGrantStore` bound to `(session_id, tool_name, canonical_json(args))`.
  - Consumed at most once; any argument modification, different tool, or second invocation requires a fresh `SessionConfig::gate` decision.

---

## 4. What Was Tested Offline and Measurements

All automated tests run offline in the worker worktree without invoking the live `agy` binary:

| Test Suite / Check | Command | Result | Duration / Measurements |
|---|---|---|---|
| Golden fixture & stdin shape tests (11 fixtures: `m0-00-smoke` + 10 hand-made scenarios) | `cargo test -p spike-agent-agy --test golden` | **3 passed, 0 failed** | `10ms`; 11 `.jsonl` + 11 `.events.json` + 11 `.md` files verified clean via `Sanitizer::is_clean`; all stdin lines verified to match `{"event":"user","message":{"content":"..."}}` |
| Bridge, fail-closed, retry-grant, argv/stdin pin, harness rejection, and saturation tests | `cargo test -p spike-agent-agy --test bridge_and_saturation` | **9 passed, 0 failed** | `3.01s` total; Unix socket round-trip `< 2ms`; 10 error/drop/timeout paths verified to return `decision: "deny"`; broken launches verified to fail `record` and return `INCONCLUSIVE` in `hook-failures` |
| Saturation & backpressure test (`EVENT_BUFFER = 1024`) | `saturation_backpressure_interrupt_and_shutdown_complete_within_grace` | **Passed** | Child flooded `2,448` events (`2.39x EVENT_BUFFER`) while `session.events` was unread; `Interrupt` + `Shutdown` + child reap completed in `< 2.6s` (within `SHUTDOWN_GRACE = 5s`) |
| Hook-failures bridge self-test | `cargo run -p spike-agent-agy --example hook-failures -- --self-test` | **3/3 fail-closed** | Missing socket, malformed JSON stdin, and empty stdin all returned `decision: "deny"` |
| Lints & formatting | `cargo fmt --check` and `cargo clippy -p spike-agent-agy --all-targets -- -D warnings` | **0 warnings, 0 errors** | Zero `unsafe` blocks (`unsafe_code = "deny"`), zero `dbg!`/`todo!` |

---

## 5. Fix Round 1 Changes

During orchestrator verification of the initial submission, live runs against `agy` 1.4.7 failed because:
1. `-p` without an attached value consumed `"--input-format"` as its prompt argument, and `{"type":"user","message":{"role":"user","content":"..."}}` on stdin was rejected by `agy` (`"missing the event field"`).
2. `examples/hook-failures.rs` passed invalid flags (`--mode agent` and `--workspace <dir>`), causing `agy` to print usage and exit 2 without starting or attempting any tool call, which the harness vacuously reported as `fail_closed: true`.
3. `examples/record.rs` wrote broken live fixtures even when `SessionStarted` was never reached.

**Fixes applied in Fix round 1**:
1. **`AgyAdapter` CLI argv and stdin shape (`spikes/agent-agy/src/adapter.rs` & fixtures)**:
   - Changed `build_cli_args` to pass `--print=` (empty prompt attached with `=`) instead of `-p`.
   - Changed `StdinFormat::default()` (`StdinFormat::EventUserMessage`) to emit `{"event":"user","message":{"content":"<text>"}}`.
   - Enabled `install_cwd_hook_if_absent = true` by default on `AgyAdapter` so sessions spawned in fresh directories automatically get `.agents/hooks.json` in `cwd` pointing to `spike-agy-hook`.
   - Regenerated all 10 hand-made fixtures in `fixtures/agents/agy/` to use `{"event":"user","message":{"content":"..."}}` and pinned both the exact `argv` and `stdin` shape in `tests/bridge_and_saturation.rs` and `tests/golden.rs`.
2. **`examples/hook-failures.rs` non-vacuous verdicts & control case**:
   - Replaced invalid `--mode agent` and `--workspace` flags with `cmd.current_dir(&workspace)` and `--print=<prompt> --model <model> --mode accept-edits --dangerously-skip-permissions --output-format stream-json --print-timeout 0s`.
   - Added a mandatory `control-allow` case first (healthy `PreToolUse` hook returning `{"decision":"allow"}`). If `control-allow` does not emit `init`, attempt a tool call, and create `canary.txt`, the harness prints `OVERALL RUN: INCONCLUSIVE` and exits with code 2.
   - Parsed `stdout` stream-json in every case for `init_seen` and `tool_attempted`: a failure case is marked `fail_closed: true` (`verdict: "fail_closed"`) **only** when `init_seen && tool_attempted && !canary_created`. Otherwise, if `!init_seen || !tool_attempted`, it is reported as `verdict: "inconclusive"` and `fail_closed: false`.
3. **`examples/record.rs` loud failure on missing `SessionStarted`**:
   - `write_fixture_files` and `record_live_scenario` now verify that `AgentEvent::SessionStarted` was emitted for every scenario other than `startup-error` (and that `resume`'s seed session reached `SessionStarted`). If not, `record` returns an `Err` and exits non-zero without writing any fixture files.

---

## 6. What Still Needs a Live Run (Orchestrator Verification Commands)

Nothing has been verified live against `agy` 1.4.7 from inside the worker session. The orchestrator should run:

1. **Live `agy`-side hook-failure harness** (runs `control-allow` first, then the 6 broken-hook cases):
   ```sh
   cargo run -p spike-agent-agy --example hook-failures
   ```
2. **Record live fixtures against `agy` 1.4.7**:
   ```sh
   cargo run -p spike-agent-agy --example record -- simple
   cargo run -p spike-agent-agy --example record -- tool-read
   cargo run -p spike-agent-agy --example record -- gate-deny
   cargo run -p spike-agent-agy --example record -- approval-allow
   cargo run -p spike-agent-agy --example record -- approval-deny
   cargo run -p spike-agent-agy --example record -- question
   cargo run -p spike-agent-agy --example record -- interrupt
   cargo run -p spike-agent-agy --example record -- multi-turn
   cargo run -p spike-agent-agy --example record -- resume
   cargo run -p spike-agent-agy --example record -- startup-error
   ```
3. **Interactive chat smoke check**:
   ```sh
   cargo run -p spike-agent-agy --example chat -- --prompt "Run pwd and tell me the directory."
   ```

---

## 7. Recommendation for M1+ and Open Risks

- **Recommendation**: **Keep the `atrium-agents` and `atrium-protocol` contracts as-is** (zero contract changes needed). Promote `AgyNormalizer`, `GateBridge`, `RetryGrantStore`, and `AgyAdapter` into `crates/atrium-agents` in M1 once the orchestrator's live `hook-failures` and `record` runs complete.
- **Open Risks**:
  1. **`agy` behavior when the hook helper binary itself fails (`examples/hook-failures.rs`)**: To be determined by the orchestrator's live run of `cargo run -p spike-agent-agy --example hook-failures`.
  2. **Per-session MCP discovery**: Live verification needed on `agy` 1.4.7 to confirm whether `AGY_MCP_CONFIG` or `--add-dir` injects MCP servers per session without touching `~/.gemini`.
  3. **Multi-turn stdin vs `--conversation` resume**: Live verification of `record -- multi-turn` will confirm whether `agy --print= --input-format stream-json` accepts a second `{"event":"user","message":{"content":"..."}}` line after the first `result` record or requires respawning with `--conversation <native_session_id>`.
