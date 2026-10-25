# M0-03 Spike Report: Antigravity (`agy`) Stream-JSON Adapter

## 1. What Was Built (`spikes/agent-agy`)

We built the `spike-agent-agy` crate implementing `atrium_agents::AgentAdapter` (`AgyAdapter`) and `atrium_agents::Normalizer` (`AgyNormalizer`) for Google's Antigravity CLI (`agy` 1.4.x), along with the per-session `PreToolUse` Unix-socket gate bridge and live/offline verification harnesses:

- **`spikes/agent-agy/src/normalizer.rs` (`AgyNormalizer`)**:
  - Pure, I/O-free state machine implementing `Normalizer::on_input(&mut self, input: &AdapterInput) -> Vec<AgentEvent>`.
  - Translates `agy` `--output-format stream-json` records:
    - `init` -> `AgentEvent::SessionStarted { provider: Provider::Agy, native_session_id, model, tools }`.
    - `step_update` (`step_type: "tool"` | `"tool_call"`, `state: "ACTIVE"` | `"RUNNING"` | `"DONE"` | `"ERROR"` | `"CANCELLED"`) -> `ToolCallStarted` and `ToolCallFinished` (`Succeeded`, `Failed`, `Denied`, `Cancelled`).
    - `step_update` (`step_type: "agent_response"`, `text_delta`, `thinking_delta`, `usage`) -> `MessageDelta` / `MessageCompleted` (`MessageKind::Assistant` and `MessageKind::Thinking`) plus cumulative per-turn token `Usage`.
    - `denied_actions` -> `ToolCallFinished { status: ToolCallStatus::Denied }`.
    - `result` -> `AgentEvent::Usage` + `AgentEvent::TurnCompleted`.
    - `AdapterInput::HookRequest`, `AdapterInput::Gate`, `AdapterInput::HookResponse`, and `AdapterInput::Command` -> `ApprovalRequested`, `QuestionAsked` (for `ask_question`), and `RequestResolved`.
    - Pre-session startup failure (`Exit { code: Some(non_zero) }` before `init`) -> `AgentEvent::Error { fatal: true }` followed by `AgentEvent::SessionEnded` without emitting `SessionStarted`.
    - Established session exit (`Exit`) -> closes open requests (`RequestOutcome::Cancelled`), open tool calls (`ToolCallStatus::Cancelled`), open message buffers, open turns (`TurnOutcome::Interrupted` / `Failed` / `Completed`), and emits `SessionEnded` last.
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
  - Non-blocking, backpressure-resilient session actor loop (`run_session_actor`): uses `tokio::select! { biased; ... }` where `commands_rx.recv()` is always polled first and `stdout` / `stderr` / `bridge_inputs` are only polled when `events_tx` has capacity or can reserve a permit. A saturated `session.events` channel (`EVENT_BUFFER = 1024`) never blocks `Interrupt` or `Shutdown`.
  - Enforces `MAX_QUEUED_MESSAGES = 8` for user messages sent while a turn is active, rejecting overflow with `AgentEvent::Error { fatal: false }`.
  - Enforces `SHUTDOWN_GRACE = 5s` (`SIGTERM` -> wait up to grace period -> `SIGKILL` + `wait()` reap) and cleans up the session-private directory on exit.

- **Examples & Fixtures**:
  - `spikes/agent-agy/examples/record.rs`: deterministic generator for all 10 hand-made fixtures (`--generate-handmade all`) and live sanitizer/recorder (`-- <scenario>`).
  - `spikes/agent-agy/examples/chat.rs`: interactive stdin/stdout chat harness over `AgyAdapter`.
  - `spikes/agent-agy/examples/hook-failures.rs`: live and self-test harness for testing `agy`'s behavior when the `PreToolUse` helper binary is missing, exits non-zero, prints invalid JSON, times out, or when `hooks.json` is unreadable/malformed.
  - `fixtures/agents/agy/`: golden `.events.json` for `m0-00-smoke.jsonl` plus `.jsonl`, `.events.json`, and `.md` for all 10 required scenarios (`simple`, `tool-read`, `gate-deny`, `approval-allow`, `approval-deny`, `question`, `interrupt`, `multi-turn`, `resume`, `startup-error`).

---

## 2. Enforcement Matrix (`agy` 1.4.x)

Under `--dangerously-skip-permissions` combined with a `PreToolUse` hook (`"matcher": "*"`), `agy` invokes the `PreToolUse` hook before executing **every** tool call (verified in the live `m0-00-smoke.jsonl` run where `run_command` and `write_file` were both intercepted and denied when the hook returned `{"decision":"deny"}`).

| Tool Class | `agy` Tool Names | Normalized `ToolKind` | Intercepted by `PreToolUse` (`matcher: "*"`) | Gate Enforcement & Notes |
|---|---|---|---|---|
| **Read** | `view_file`, `read_file`, `list_dir`, `read_resource`, `list_resources`, `command_status`, `list_permissions`, `list_plugin_accounts` | `ToolKind::Read` | Yes (`AllToolCalls`) | Routed through `SessionConfig::gate` under all `PermissionPolicy` modes (`Ask`, `ReadOnly`, `FullAccess`). The gate, not the adapter, decides which paths may be read. |
| **Edit / Write** | `write_to_file`, `write_file`, `create_file`, `replace_file_content`, `multi_replace_file_content`, `edit_file`, `sed_file`, `notebook_edit` | `ToolKind::Edit` | Yes (`AllToolCalls`) | Confirmed in `m0-00-smoke.jsonl` (`step_index: 6`, `write_file` denied by hook -> `state: "ERROR"` with `"tool call denied by pre-tool hook"`). |
| **Delete / Move** | `delete_knowledge`, plus shell `rm`/`mv` via `run_command` | `ToolKind::Delete` (`delete_knowledge`), `ToolKind::Execute` (`run_command`) | Yes (`AllToolCalls`) | `agy` has no standalone file-delete tool; file deletion/renaming goes through `run_command` and is inspected via `ToolCheck.input["CommandLine"]`. |
| **Execute** | `run_command`, `send_command_input`, `notebook_execution`, `run_workflow` | `ToolKind::Execute` | Yes (`AllToolCalls`) | Confirmed in `m0-00-smoke.jsonl` (`step_index: 2, 4, 8, 10, 12`): allowed commands run; denied commands fail with `status: denied` without executing. |
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
- **How it works in `AgyAdapter`**:
  - Spawned with `-p --input-format stream-json --output-format stream-json --print-timeout 0s`.
  - When `SessionCommand::SendUserMessage { text }` arrives and no turn is active, `AgyAdapter` writes a single NDJSON line to `agy`'s stdin and flushes.
  - `AgyAdapter` supports two configurable stdin JSON shapes via `StdinFormat`:
    1. `StdinFormat::StreamJsonUserMessage` (default, matching Claude/Gemini stream-json conventions):
       `{"type":"user","message":{"role":"user","content":"<text>"}}`
    2. `StdinFormat::EventUserInput`:
       `{"event":"user_input","text":"<text>"}`
  - While a turn is active (`TurnStarted` emitted, `result` not yet received), subsequent `SendUserMessage` commands queue in a bounded `VecDeque` (`MAX_QUEUED_MESSAGES = 8`). As soon as the current turn's `result` record arrives on stdout (`TurnCompleted`), the next queued user message is popped and written to stdin, emitting `TurnStarted { turn: N + 1 }`.
  - **Fallback if a specific `agy` version closes stdin after one turn in `-p` mode**: `agy` emits `"conversation_id": "<uuid>"` in its initial `init` record (`SessionStarted.native_session_id`) and accepts `--conversation <uuid>` (`SessionConfig::resume`) to continue the conversation across turns. Live verification by the orchestrator (Section 5) will confirm whether `agy` 1.4.7 keeps stdin open across `result` boundaries with `--print-timeout 0s` or requires `--conversation` respawn per turn.

### Question 2: Per-session MCP injection without modifying `~/.gemini`
- **How it works in `AgyAdapter`**:
  - `agy mcp add` modifies user-global state under `~/.gemini` and must **never** be used by Atrium.
  - Instead, `write_session_mcp_config(&session_dir, &config.mcp_servers)` writes a session-scoped `mcp.json` and `settings.json` with mode `0600` inside the mode-`0700` session directory:
    ```json
    {
      "mcpServers": {
        "<name>": {
          "command": "<command>",
          "args": ["arg1"],
          "env": { "KEY": "VALUE" }
        }
      }
    }
    ```
  - `AgyAdapter` passes `AGY_MCP_CONFIG=<session_dir>/mcp.json` and `ANTIGRAVITY_MCP_CONFIG=<session_dir>/mcp.json` in the child environment, and supports `pass_add_dir = true` (`--add-dir <session_dir>`) so `agy` discovers the session-private configuration directory without touching `~/.gemini` or the user's repository.
  - What still needs a live run: verifying on `agy` 1.4.7 which of `AGY_MCP_CONFIG`, `--add-dir <session_dir>`, or workspace `mcp.json` causes `agy` to list the injected MCP server tools in its `init.tools` array.

### Question 3: Routing approvals back to Atrium (`Block` vs `DenyAndRetry`)
- **Primary mechanism (`ApprovalMode::Block`)**:
  - `write_session_hook_config` sets `"timeout": 300` (300 seconds) on the `PreToolUse` hook entry in `hooks.json`.
  - When `SessionConfig::gate` returns `GateDecision::Ask`, `GateBridge` stores a `oneshot::Sender<HookDecisionOutput>` keyed by `request_id` (`req-1`, `req-2`, and so on) and `AgyNormalizer` emits `AgentEvent::ApprovalRequested` after `AgentEvent::ToolCallStarted`.
  - `spike-agy-hook` waits on the Unix domain socket until `SessionCommand::ResolveApproval` arrives:
    - `ApprovalDecision::Allow` -> `GateBridge` sends `{"decision":"allow"}`, `AgyNormalizer` emits `RequestResolved { outcome: Approved }`, and `agy` proceeds to execute the tool call (`ToolCallFinished { status: Succeeded }`).
    - `ApprovalDecision::Deny { message }` -> `GateBridge` sends `{"decision":"deny","reason":...}`, `AgyNormalizer` emits `RequestResolved { outcome: Denied }`, and `agy` aborts the tool call (`ToolCallFinished { status: Denied }`).
  - Client-side fail-closed timeout: `spike-agy-hook` enforces `--timeout-ms` (default 25,000 ms, or configurable below `hook_timeout_secs`) so that if the user or daemon does not respond in time, `spike-agy-hook` prints `{"decision":"deny","reason":"hook client timed out waiting for Atrium gate bridge"}` before `agy`'s hook timeout expires.
- **Fallback mechanism (`ApprovalMode::DenyAndRetry`, PLAN §6 mitigation)**:
  - If a shorter hook timeout is desired or `agy` cannot block long enough for human review, `ApprovalMode::DenyAndRetry` immediately replies `{"decision":"deny","reason":"approval required (req-N): waiting for user approval before retry"}` so the hook finishes in <5 ms while `ApprovalRequested` is surfaced to Atrium.
  - When `ResolveApproval { decision: Allow }` arrives, `GateBridge` inserts a `RetryGrant` into `RetryGrantStore` bound to `(session_id, tool_name, canonical_json(args))`.
  - When the agent retries the tool call:
    - If `(session_id, tool_name, canonical_json(args))` matches the grant, `RetryGrantStore::consume` removes the grant (**single-use**) and allows the call once without re-prompting.
    - If the agent changes any argument, calls a different tool, or invokes the same tool a second time, `RetryGrantStore::consume` returns `false` and the call must pass `SessionConfig::gate` anew.

---

## 4. What Was Tested and Measurements

All automated tests run offline in CI/worktree without requiring network or a live `agy` binary:

| Test Suite / Check | Command | Result | Duration / Measurements |
|---|---|---|---|
| Golden fixture tests (11 fixtures: `m0-00-smoke` + 10 scenarios) | `cargo test -p spike-agent-agy --test golden` | **3 passed, 0 failed** | `10ms`; 11 `.jsonl` + 11 `.events.json` + 11 `.md` files verified clean via `Sanitizer::is_clean` |
| Bridge, fail-closed, retry-grant, config permissions, and saturation tests | `cargo test -p spike-agent-agy --test bridge_and_saturation` | **8 passed, 0 failed** | `3.01s` total; Unix socket round-trip `< 2ms`; 10 distinct error/drop/timeout paths verified to return `decision: "deny"` |
| Saturation & backpressure test (`EVENT_BUFFER = 1024`) | `saturation_backpressure_interrupt_and_shutdown_complete_within_grace` | **Passed** | Child flooded `2,448` events (`2.39x EVENT_BUFFER`) while `session.events` was unread; `Interrupt` + `Shutdown` + child reap completed in `< 2.6s` (within `SHUTDOWN_GRACE = 5s`) |
| Hook-failures self-test | `cargo run -p spike-agent-agy --example hook-failures -- --self-test` | **3/3 fail-closed** | Missing socket, malformed JSON stdin, and empty stdin all returned `decision: "deny"` |
| Lints & formatting | `cargo fmt --check` and `cargo clippy -p spike-agent-agy --all-targets -- -D warnings` | **0 warnings, 0 errors** | Zero `unsafe` blocks (`unsafe_code = "deny"`), zero `dbg!`/`todo!` |

### Key Implementation Finding During Testing: macOS `SUN_LEN` Limit
During integration testing on macOS, binding a Unix domain socket inside `std::env::temp_dir()` (`/var/folders/<hash>/T/`, ~50 bytes) with a long directory name (`atrium-agy-sessions/agy-session-<pid>-<seq>-<nanos>/gate.sock`, ~122 bytes total) failed with `InvalidInput: "path must be shorter than SUN_LEN"` (`SUN_LEN = 104` bytes on macOS). We shortened the session directory layout to `<temp_dir>/agy/s-<pid:x>-<seq:x>/g.sock` (~68 bytes on macOS), eliminating the `SUN_LEN` failure across all platforms.

---

## 5. What Still Needs a Live Run (Orchestrator Verification Commands)

Because worker policy intentionally blocks invoking the live `agy` binary from inside a worker session, the orchestrator should run the following commands during verification and paste the live results:

1. **Run the live `agy`-side hook-failure harness** (verifies whether `agy` 1.4.7 fails closed when the hook binary is missing, exits non-zero, prints invalid JSON, times out, or when `hooks.json` is unreadable/malformed):
   ```sh
   cargo run -p spike-agent-agy --example hook-failures
   ```
2. **Record live fixtures against `agy` 1.4.7** (replaces any hand-made fixture with a live-captured recording and automatically sanitizes paths/secrets):
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

## 6. Recommendation for M1+ and Open Risks

- **Recommendation**: **Keep the `atrium-agents` and `atrium-protocol` contracts as-is** (zero contract changes needed). Promote `AgyNormalizer`, `GateBridge`, `RetryGrantStore`, and `AgyAdapter` into `crates/atrium-agents` in M1 once the orchestrator's live `hook-failures` run confirms `agy`'s behavior on broken hooks.
- **Open Risks**:
  1. **`agy` behavior when the hook helper binary itself fails (`examples/hook-failures.rs`)**: `spike-agy-hook` is guaranteed by unit/integration tests to output `{"decision":"deny"}` on every error it can observe. However, if `agy` itself fails open when `hooks.json` is unreadable or `spike-agy-hook` is missing/killed by SIGKILL, host-mode `agy` sessions must verify `spike-agy-hook` and `hooks.json` integrity before spawning `agy`.
  2. **Hook discovery directory precedence**: `agy` loads `.agents/hooks.json` from the workspace root (`--workspace` / `cwd`). If `agy` ignores `--add-dir` for hooks when `.agents/hooks.json` already exists in the repo, `AgyAdapter` provides `install_cwd_hook_if_absent` / workspace hook merging for M1.
  3. **Multi-turn stdin vs `--conversation` resume**: If `agy -p --input-format stream-json` exits after the first `result` record even with `--print-timeout 0s`, M1 can transparently respawn `agy --conversation <native_session_id>` per turn inside `AgyAdapter` without changing the `Session` contract.
