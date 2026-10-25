# M0-03 Spike Report: Antigravity (`agy`) Stream-JSON Adapter

> **Live verification status** (agy 1.4.7). The worker policy blocks running `agy` from the worker session, so every live run was done by the orchestrator on the host. After Fix round 1 (`0ef8b90`) it verified: `simple` answers; `tool-read` reads; `gate-deny` denies both `run_command` and `read_file` on `secret.txt` and the secret never appears; `approval-allow` and `approval-deny` produce `ApprovalRequested` -> `RequestResolved` -> the right outcome, so the **blocking hook bridge works**. The hook-failure harness (control case passing) found that a missing helper, a non-zero helper exit, invalid JSON from the helper and a hook timeout all **fail closed**, but that an **unreadable (mode 0000) or malformed `hooks.json` fails OPEN** (section 2.2). Fix round 2 (section 6) hardens the adapter against that. Not yet verified live: `question`, `interrupt`, `multi-turn`, `resume`, `startup-error`, per-session MCP, and the Fix round 2 changes themselves (section 7).

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

- **`spikes/agent-agy/src/sanitize.rs` (`FixtureSanitizer`)** (Fix round 2): wraps the contract `Sanitizer` and first maps the scratch dir (as given, its realpath, `/private/var/folders/...` and `/var/folders/...` forms, `file://` URLs) to `/scratch` and every other temp-dir root to `/scratch-tmp`. `is_clean` fails if a temp-dir path survives.

- **`spikes/agent-agy/src/adapter.rs` (`AgyAdapter`)**:
  - Implements `AgentAdapter` (`provider() == Provider::Agy`, `interception() == Interception::AllToolCalls`).
  - Launches `agy` with `--print= --input-format stream-json --output-format stream-json --print-timeout 0s --mode <accept-edits|plan|auto> --dangerously-skip-permissions` and writes each user turn to stdin as `{"event":"user","message":{"content":"<text>"}}`.
  - Installs the session's `hooks.json` into the child `cwd` `.agents/hooks.json` (`install_cwd_hook_if_absent = true`; mode 0600, created with `create_new`) and removes exactly what it created on cleanup. **Refuses to start** if a hook config is already there (agy would read that one, not ours). Before spawning it reads every hook config back and validates it (`GateSetup::verify`, section 6); while the session runs it aborts on a gate bypass (`enforce_gate_coverage`, section 6).
  - Non-blocking, backpressure-resilient session actor loop (`run_session_actor`): uses `tokio::select! { biased; ... }` where `commands_rx.recv()` is always polled first and `stdout` / `stderr` / `bridge_inputs` are only polled when `events_tx` has capacity or can reserve a permit. A saturated `session.events` channel (`EVENT_BUFFER = 1024`) never blocks `Interrupt` or `Shutdown`.
  - Enforces `MAX_QUEUED_MESSAGES = 8` for user messages sent while a turn is active, rejecting overflow with `AgentEvent::Error { fatal: false }`.
  - Enforces `SHUTDOWN_GRACE = 5s` (`SIGTERM` -> wait up to grace period -> `SIGKILL` + `wait()` reap) and cleans up the session-private directory on exit.

- **Examples & Fixtures**:
  - `spikes/agent-agy/examples/record.rs`: deterministic generator for all 10 hand-made fixtures (`--generate-handmade all`) and live recorder (`-- <scenario>`). Every scenario has a list of expectations (section 6.3); a live run that does not meet them exits non-zero, writes nothing to `fixtures/` and leaves a sanitized dump of the failed recording under the OS temp dir. `-- all` runs every scenario and fails at the end if any did.
  - `spikes/agent-agy/examples/chat.rs`: interactive stdin/stdout chat harness over `AgyAdapter`.
  - `spikes/agent-agy/examples/hook-failures.rs`: live and self-test harness for testing `agy`'s behavior when the `PreToolUse` helper binary is missing, exits non-zero, prints invalid JSON, times out, or when `hooks.json` is unreadable/malformed. Runs a mandatory `control-allow` case first (which must see `init`, attempt the tool call, and create `canary.txt`, or else the entire run reports `INCONCLUSIVE` and exits non-zero) and marks any failure case as `fail_closed` **only** if `init` was seen, the tool call was attempted, and `canary.txt` is absent.
  - `fixtures/agents/agy/`: golden `.events.json` for `m0-00-smoke.jsonl` plus `.jsonl`, `.events.json`, and `.md` for all 10 required scenarios (`simple`, `tool-read`, `gate-deny`, `approval-allow`, `approval-deny`, `question`, `interrupt`, `multi-turn`, `resume`, `startup-error`).

---

## 2. Enforcement Matrix (`agy` 1.4.7)

### 2.1 Tool classes

Under `--dangerously-skip-permissions` combined with a `PreToolUse` hook (`"matcher": "*"`), `agy` invokes the hook before executing a tool call. **Live evidence**: `m0-00-smoke.jsonl` (`run_command` and `write_file` denied by the hook), and the orchestrator's live runs after Fix round 1 (`read_file` and `run_command` allowed, denied and approved through the adapter's bridge). The rows below for edit, delete, search, fetch, MCP and subagents follow from `matcher: "*"` and are **not individually verified live**; the runtime coverage check (section 6.1) is the safety net for a tool class that turns out not to be hooked.

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

### 2.2 When the hook itself is broken (live, `hook-failures` harness, agy 1.4.7)

The harness runs a control case first (a healthy hook must start `agy`, attempt the tool and create `canary.txt`; the control passed), then breaks the hook one way at a time and reports whether the tool ran.

| Case (exact set-up) | agy 1.4.7 | What `AgyAdapter` does about it |
|---|---|---|
| Helper binary missing (command points at a nonexistent file) | **fail closed**, tool did not run | `start()` refuses: `check_helper_binary` (also checks the executable bit) |
| Helper exits non-zero, no JSON (`sh -c 'exit 42'`) | **fail closed** | n/a: `spike-agy-hook` always prints a decision and exits 0 on every error it can see |
| Helper prints invalid JSON, exit 0 | **fail closed** | same |
| Helper sleeps past the configured hook `timeout` (sleep 5, timeout 1) | **fail closed** (agy enforces its hook timeout and denies) | helper's own timeout is shorter than agy's, so it answers `deny` first |
| `hooks.json` unreadable: file mode `0000` | **FAIL OPEN**: agy ignores the config and runs the tool | `start()` refuses: file mode must be exactly `0600` (`BadFileMode`); re-checked at every tool start |
| `hooks.json` malformed JSON (`{ malformed json `) | **FAIL OPEN**: same | `start()` refuses: the file must parse and register the gate hook (`Malformed`); re-checked at every tool start |

Cases that follow from the same mechanism (agy reads **one** file, `<cwd>/.agents/hooks.json`; if that file is not ours, ours is not used). **Not run live**, covered by unit tests only: a hook config already present in the working directory (previously the adapter silently skipped installing its own and ran ungated; now `start()` refuses, `CwdHooksPresent`); the file replaced by a symlink; valid JSON that does not register a `PreToolUse` command hook with matcher `"*"` and our exact command and timeout (`GateHookAbsent`); a config directory writable by group or others; the config deleted or corrupted after start (by a tool call, say), caught at the next tool start.

### 2.3 `PermissionPolicy` mapping

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
  - `write_session_mcp_config(&session_dir, &config.mcp_servers)` writes a session-scoped `mcp.json` and `settings.json` with mode `0600` inside the mode-`0700` session directory (`{"mcpServers": { ... }}`), and `AgyAdapter` can pass `--add-dir <session_dir>` (`pass_add_dir`, off by default). **Correction**: earlier versions of this report said the adapter also sets `AGY_MCP_CONFIG` / `ANTIGRAVITY_MCP_CONFIG`. It does not (it only sets `ATRIUM_AGY_SESSION_DIR` and `ATRIUM_AGY_GATE_SOCKET`), and those variable names were never confirmed to exist in `agy`.
  - **Not verified live. Open.** Nothing currently hands the generated `mcp.json` to `agy`, so a session with `mcp_servers` set gets no MCP servers. What a live run must find out on `agy` 1.4.7: whether `--add-dir <session_dir>` or a workspace-level config in the working directory makes `agy` load per-session MCP servers into `init.tools`. Whatever is tried must not touch `~/.gemini`, and the MCP tool call must still go through the hook (`call_mcp_tool`).

### Question 3: Routing approvals back to Atrium (`Block` vs `DenyAndRetry`)
- **Primary mechanism (`ApprovalMode::Block`, verified live for approve and deny with prompt resolutions)**:
  - `write_session_hook_config` sets `"timeout": 300` (300 seconds) on the `PreToolUse` hook entry in `hooks.json`.
  - When `SessionConfig::gate` returns `GateDecision::Ask`, `GateBridge` stores a `oneshot::Sender<HookDecisionOutput>` keyed by `request_id` (`req-1`, `req-2`, and so on) and `AgyNormalizer` emits `AgentEvent::ApprovalRequested` after `AgentEvent::ToolCallStarted`.
  - `spike-agy-hook` blocks on the Unix domain socket until `SessionCommand::ResolveApproval` arrives (`Allow` -> `{"decision":"allow"}`, `Deny` -> `{"decision":"deny","reason":...}`).
  - Client-side fail-closed timeout: `spike-agy-hook` enforces `--timeout-ms` (default 25,000 ms), after which it prints `{"decision":"deny","reason":"hook client timed out waiting for Atrium gate bridge"}`.
  - **Does `agy` wait for a slow hook, and is there a timeout?** It waits for the hook to answer (approvals resolved after a human-scale delay worked live). It enforces the `timeout` from `hooks.json`: live, a hook that slept past a 1 s timeout was cut off and the tool was **denied** (fail closed). Not measured: how long `agy` really waits at the 300 s the adapter configures.
  - **Known inconsistency, not fixed in round 2**: the hook command registered in `hooks.json` does not pass `--timeout-ms`, so the helper gives up after its 25 s default while `agy` is configured to wait 300 s. An approval that takes the user longer than 25 s is therefore denied by the helper. M1 should pass `--timeout-ms` derived from `hook_timeout_secs` (minus a margin); the command string is built in one place now (`bridge::hook_command`).
- **Fallback mechanism (`ApprovalMode::DenyAndRetry`, PLAN §6 mitigation)**:
  - Immediately replies `{"decision":"deny","reason":"approval required (req-N): waiting for user approval before retry"}` while surfacing `ApprovalRequested`.
  - When `ResolveApproval { decision: Allow }` arrives, `GateBridge` inserts a single-use `RetryGrant` into `RetryGrantStore` bound to `(session_id, tool_name, canonical_json(args))`.
  - Consumed at most once; any argument modification, different tool, or second invocation requires a fresh `SessionConfig::gate` decision.

---

## 4. What Was Tested Offline and Measurements

All automated tests run offline without invoking the live `agy` binary. Fix round 2 numbers (macOS, debug build, `cargo test -p spike-agent-agy`):

| Test file | What it covers | Result |
|---|---|---|
| `tests/golden.rs` | golden test of `m0-00-smoke.jsonl` + 10 hand-made scenarios; stdin shape pin; every fixture file passes `Sanitizer::is_clean` **and** contains no temp-dir path; `ask_question` surfaced once with the hook's id in either arrival order | 4 passed |
| `tests/bridge_and_saturation.rs` | bridge allow / deny / ask (block) / adapter dropped mid-wait / 10 malformed-or-missing error cases all `deny`; retry grants (single use, canonical args); argv + stdin pin; saturation (2,448 events, interrupt + shutdown within `SHUTDOWN_GRACE`); broken-launch rejection by `record` and `hook-failures` | 9 passed (3.0 s) |
| `tests/gate_hardening.rs` (new) | hook-config validation: valid; mode 0000; malformed JSON (4 variants + array); no gate hook (7 mutations); file/dir mode rules; symlink file and dir; missing file; helper missing / not executable; cwd install + refusal of pre-existing configs; adapter refuses to start (pre-existing config in 3 forms, helper missing) and never spawns `agy`; runtime tripwire (ungated tool, config corrupted mid-session in 3 ways, control case with the real helper, explicit opt-out) | 14 passed (0.9 s) |
| `tests/sanitize_and_record.rs` (new) | every spelling of the scratch dir -> `/scratch`; other temp roots -> `/scratch-tmp`; every `AdapterInput` kind; `is_clean` fails when a temp path survives; `record` against a fake `agy` writes fixtures with no temp path; `record` fails loudly (non-zero, nothing written) on unmet expectations and when the secret is in the raw recording | 9 passed (1.3 s) |
| `tests/recorder_scenarios.rs` (new) | the real `record` example against a scripted fake `agy` for all 10 scenarios (approvals through the real helper and socket, question answer, second message of `multi-turn`, seed + `--conversation` for `resume`, interrupt mid-tool-call, invalid-model launch); each written fixture re-normalizes to its own `.events.json` | 1 passed (5.2 s) |
| Lints | `cargo fmt --check`, `cargo clippy -p spike-agent-agy --all-targets -- -D warnings` | clean |

Total: 37 tests. The fake `agy` proves the recorder's control flow, not that the real `agy` behaves the same way.

---

## 5. Fix Round 1 Changes (kept for history)

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

## 6. Security Findings and Fix Round 2

### 6.1 Finding: `agy` fails open on an unusable hook config, and what the adapter does about it

**Finding (live, agy 1.4.7, orchestrator's `hook-failures` run).** With `--dangerously-skip-permissions`, the `PreToolUse` hook is the only policy. If `<cwd>/.agents/hooks.json` is **unreadable (mode 0000)** or **malformed JSON**, `agy` ignores the hook config, does not complain, and runs the tool. Every failure of the helper itself (missing, non-zero exit, invalid JSON, timeout) fails closed; a failure to *load the hook config* fails open. The exact cases and evidence are in the table in section 2.2. Brief rule: any case where the tool runs blocks `agy` use on the host until fixed; the orchestrator decides. The adapter-level mitigation below makes `AgyAdapter` refuse those configurations, but `agy` run by anything else with such a config is not safe.

**Fix round 2 hardening, in four layers** (`spikes/agent-agy/src/bridge.rs`, `adapter.rs`; tests in `tests/gate_hardening.rs`):

1. **Read-back before spawn.** `start()` builds a `GateSetup` and calls `verify()` right before spawning `agy` (after the socket is bound). It checks, for the session-private copies (`<session>/hooks.json`, `<session>/<agents dir>/hooks.json`) and the working-directory copy that `agy` actually reads: the directory is a real directory (not a symlink) with mode exactly `0700` (session) or not group/other-writable (project dir); the file is a regular file (not a symlink) with mode exactly `0600`; it reads back; it parses as a JSON object; and it contains a `PreToolUse` entry with matcher `"*"` and a `command` hook whose command line and `timeout` are exactly what the adapter wrote. The helper binary must exist and be executable (a bare name is resolved on `PATH`). Any failure returns `AdapterError::Handshake("refusing to start agy, gate hook config check failed: ...")`, `agy` is never spawned, and everything the call created (session dir, working-directory config, the config dir if the adapter made it) is removed. A pre-existing config of the user's is never touched.
2. **Refuse to run behind someone else's config.** If `<cwd>/.agents/hooks.json` already exists, `start()` refuses (`CwdHooksPresent`). Before Fix round 2 the adapter silently skipped installing its own in that case, so `agy` read the other file and ran ungated. The new file is created with `create_new` and mode `0600`, so a race cannot make it overwrite one. `install_cwd_hook_if_absent = false` is the explicit opt-out for a caller that installs the gate itself; layers 3 and 4 still apply.
3. **Re-verify on every tool start.** Each `ToolCallStarted` re-runs `GateSetup::verify()`. If a tool call (or anything else) changed, chmod'ed or deleted the config, the session is aborted.
4. **Coverage tripwire.** The adapter counts hook requests it answered `allow` and tool calls that finished `Succeeded` or `Failed` (that is, that ran). If more tools ran than hooks were allowed, a tool ran with no gate decision: this is exactly what a fail-open `agy` looks like from outside. The adapter emits `AgentEvent::Error { fatal: true, message: "gate bypass suspected, session aborted: tool '<name>' (<call id>) finished ... without a preceding allowed hook request ..." }`, locks the bridge down (every later hook request is denied, pending approvals are cancelled), and SIGKILLs the child at once (no grace period). Pending hook traffic is drained before each stdout line so a hook reply is always counted before the tool step it covers. `enforce_gate_coverage` (default `true`; `--no-coverage-check` on `record` and `chat`) turns layers 3 and 4 off for investigation.

**Startup self-check: what is and is not feasible.** The hook only fires when the model calls a tool, and the adapter cannot make the model call one at startup, so "confirm the first gate call arrives" cannot be done before the first user turn. Layer 1 is the pre-start check; layer 4 is the "first gate call must arrive" check, evaluated after the fact, per tool.

### 6.2 Sanitizer (`src/sanitize.rs`, `FixtureSanitizer`)

The live fixtures recorded after Fix round 1 leaked the per-user macOS temp path (`/private/var/folders/<id>/T/...` and `/var/folders/...`) in 7 files (discarded). `atrium_agents::fixture::Sanitizer` is a contract type and only knows secrets, the home directory and the user name, so `FixtureSanitizer` runs first:

- scratch dir (as given, its realpath, and the `/private` and non-`/private` spellings of both) -> `/scratch`; so `file:///var/folders/.../x/hello.txt` -> `file:///scratch/hello.txt`;
- any other temp root (`std::env::temp_dir()`, `$TMPDIR`, and any `[/private]/var/folders/<id>/<id>/<T|C|0|X>` of another machine) -> `/scratch-tmp`;
- matches respect path boundaries (a sibling directory sharing a prefix is not rewritten), and every `AdapterInput` kind is covered (stdin/stdout/stderr lines are parsed as JSON and their string values rewritten; hook payloads, gate checks and commands go through a serde round trip).
- `is_clean` additionally fails if a temp-dir path survives. `record` asserts it on both the `.jsonl` and the `.events.json` before writing; `tests/golden.rs` fails if any committed fixture file contains one (`leaked_temp_path`); `tests/sanitize_and_record.rs` has unit tests for each spelling and an end-to-end test through `record`.
- Hand-made fixtures keep their `~/scratch` paths; live ones will say `/scratch`. The live `.md` says so.

### 6.3 Recorder (`examples/record.rs`)

Verified invocation, unchanged: `agy --print= --input-format stream-json --output-format stream-json --print-timeout 0s --mode accept-edits --dangerously-skip-permissions --model <id>`, stdin `{"event":"user","message":{"content":"..."}}` (pinned in tests). What changed so the five remaining scenarios record correctly or fail loudly:

- **Expectations per scenario**, checked on the normalized events before anything is written. Common: last event is `SessionEnded`; every `ToolCallStarted` has a `ToolCallFinished`; every `ApprovalRequested` / `QuestionAsked` has a `RequestResolved`; the gate-bypass tripwire did not fire; one `SessionStarted` and no fatal `Error` (except `startup-error`). Then: `simple` the reply has PONG, no tools; `tool-read` a read tool succeeded and the reply quotes `hello.txt`; `gate-deny` at least one tool `Denied`, none `Succeeded`; `approval-allow` an approval resolved `Approved` and the command ran; `approval-deny` resolved `Denied`, the call `Denied`, nothing ran; `question` exactly one `QuestionAsked`, resolved `Answered`; `interrupt` a tool started, the turn ended `Interrupted`, nothing `Succeeded`; `multi-turn` two completed turns with `TURN_ONE_OK` and `TURN_TWO_OK`; `resume` the reply contains `COBALT-42`; `startup-error` no `SessionStarted` and a fatal `Error`. All unmet expectations are listed in one error, and a sanitized dump of the failed recording is written to `$TMPDIR/atrium-agy-record-failures/<scenario>.sanitized.jsonl` so a failed live run need not be repeated to see what happened.
- **Secret check before sanitizing.** `SCENARIO_SECRET` is a sanitizer secret, so after sanitizing it is `<redacted>` and the old "events do not contain the secret" assertion could never fail. The raw recording is now searched first; finding the secret in a `gate-deny` run means the gate did not hold, and nothing is written.
- **`resume`**: the seed session is awaited until its events channel closes (the adapter has removed its hook config from the scratch dir by then); before this the resumed session could start while the first was still shutting down, and with the new refusal rule would hit its leftover config. The seed also has a timeout and must complete a turn.
- **`interrupt`**: after `Interrupt` the harness waits up to 5 s for the session to end by itself before `Shutdown` (it used to shut down after 200 ms, which could pre-empt `agy`'s own handling).
- **Deadline and drain**: on the 90 s deadline the run now fails (nothing written) and the actor is drained so the final `exit` input is recorded; `-- all` runs every scenario and exits non-zero at the end if any failed.
- **`question`**: expected to need a live look; if `agy` in `--print` mode does not offer `ask_question` or does not deliver the hook's answer, the run fails after its deadline with the dump, which the contract allows to be reported as "provider can't ask".

### 6.4 Residual risks (stated, not solved)

- **The first ungated call cannot be prevented by the tripwire.** If `agy` fails open in a way layers 1 to 3 do not see, that call has already run when the tripwire fires; the tripwire stops the rest of the session. Processes that call started (grandchildren of `agy`) are not killed with it.
- **A tool call can try to disable the gate.** `write_file` / `run_command` on `<cwd>/.agents/hooks.json` is itself a tool call, so the real policy behind `SessionConfig::gate` must deny any write, move, delete or `chmod` of the config directory (and of anything that makes `agy` load other hooks or config). Layers 3 and 4 are the backstop if it does not.
- **`agy` may also read hook configs from places other than `<cwd>/.agents/hooks.json`** (user-level, parent directories, `--add-dir`). Not investigated; if one exists it can only add hooks, and `deny` from our hook still wins, but an unusable user-level config would presumably fail open the same way.
- **`ask_question` bypasses `SessionConfig::gate` by design**: the bridge turns it into `QuestionAsked` and answers the hook itself, without consulting the gate. It has no side effects, but `interception() == AllToolCalls` is therefore slightly more than true; M1 should decide whether the gate sees it.
- **Tool classes other than `read_file` and `run_command`** are assumed hooked (matcher `"*"`) but not individually verified live. If some internal tool is never hooked, the tripwire will abort the session on its first use with the tool's name in the message; that is a finding to report, not a reason to switch the check off.
- Root bypasses file modes; the mode check still rejects mode `0000` regardless, but a root-owned `agy` would read it. Not a supported set-up.

### 6.5 Other corrections in round 2

- `ask_question` is surfaced from the **hook request** only (`QuestionAsked.request_id` = the bridge's request id). Live `agy` sends the tool step before the hook request, and the old code emitted a second, duplicate `QuestionAsked` with a different id from the step, so an `AnswerQuestion` for it would never reach the bridge and the hook would time out and deny. Found by the offline end-to-end recorder test; pinned in `tests/golden.rs` for both arrival orders.
- Session cleanup (hook config, session dir) now happens before the final event flush, so a consumer that reacts to `SessionEnded` by starting the next session in the same directory does not race the cleanup.
- `examples/hook-failures` prints `FAIL-OPEN: ...` and exits 3 if any case ran the tool, and lists inconclusive cases, instead of exiting 0 with the verdict only inside the table.
- Report fixes: the "nothing verified live" claims (now superseded by the orchestrator's runs), the `AGY_MCP_CONFIG` claim (false, see question 2), and the stale open risk about helper failures (now answered, section 2.2).

---

## 7. What Still Needs a Live Run (Orchestrator Verification Commands)

The worker never ran `agy`. Build first: the adapter now refuses to start if `spike-agy-hook` is missing, and `cargo run --example` does not necessarily build the package's binaries.

```sh
cargo build -p spike-agent-agy --bins --examples
```

1. **Remaining scenarios**, with the verified invocation. `question` is the one most likely to need a look: it may fail after its 90 s deadline if headless `agy` does not deliver the hook's answer; the failure prints where the sanitized dump is.
   ```sh
   cargo run -p spike-agent-agy --example agy-record -- question
   cargo run -p spike-agent-agy --example agy-record -- interrupt
   cargo run -p spike-agent-agy --example agy-record -- multi-turn
   cargo run -p spike-agent-agy --example agy-record -- resume
   cargo run -p spike-agent-agy --example agy-record -- startup-error
   ```
2. **Re-record the first five** (the earlier live fixtures were discarded for leaking the temp path; the new files must say `/scratch`, and `git grep -n -e /var/folders -e /private/var fixtures/` must find nothing):
   ```sh
   cargo run -p spike-agent-agy --example agy-record -- simple
   cargo run -p spike-agent-agy --example agy-record -- tool-read
   cargo run -p spike-agent-agy --example agy-record -- gate-deny
   cargo run -p spike-agent-agy --example agy-record -- approval-allow
   cargo run -p spike-agent-agy --example agy-record -- approval-deny
   ```
   (`record -- all` runs all ten and exits non-zero at the end if any failed.) Watch for a `gate bypass suspected` error: it means a tool ran without a hook request (or the config stopped validating) and names the tool. Add `--no-coverage-check` to look at what `agy` does without the abort.
3. **Hook-failure harness** (optional re-run; expect `fail_closed` for the four helper cases and `fail_open` for `unreadable-config-permissions` and `malformed-config-json`, then a `FAIL-OPEN:` line and exit 3):
   ```sh
   cargo run -p spike-agent-agy --example hook-failures
   ```
4. **Adapter refuses what `agy` fails open on.** Quick live check that needs no model call: run `chat` from a directory that already has a hook config and see it refuse (use a fresh directory for the normal run):
   ```sh
   d=$(mktemp -d) && mkdir "$d/.agents" && echo '{ malformed' > "$d/.agents/hooks.json" && cargo run -p spike-agent-agy --example agy-chat -- --cwd "$d" --prompt hi   # must print: chat failed: ... refusing to start agy ...
   cargo run -p spike-agent-agy --example agy-chat -- --cwd "$(mktemp -d)" --prompt "Run pwd and tell me the directory."
   ```

---

## 8. Recommendation for M1+ and Open Risks

- **Recommendation**: **Keep the `atrium-agents` and `atrium-protocol` contracts as-is** (zero contract changes needed). Promote `AgyNormalizer`, `GateBridge`, `RetryGrantStore`, `GateSetup` and `AgyAdapter` into `crates/atrium-agents` in M1 once the live runs in section 7 pass. Treat the fail-open finding (section 6.1) as a standing requirement for any hook-based provider: validate the hook config yourself, because the provider will not tell you it ignored it.
- **Open risks**:
  1. **Fail-open on a hook config `agy` cannot load** (section 6.1): mitigated in `AgyAdapter`, not in `agy`. Anything that launches `agy` with `--dangerously-skip-permissions` outside this adapter (for example `scripts/agent-run`) has the same exposure; `scripts/agy-policy.py`'s own config is written by the launcher, so a launcher-side read-back is worth adding (out of scope here).
  2. **Per-session MCP discovery**: nothing hands `mcp.json` to `agy` yet (section 3, question 2).
  3. **Multi-turn stdin vs `--conversation` resume**: `record -- multi-turn` and `record -- resume` will show whether `agy --print= --input-format stream-json` accepts a second user line after the first `result`, or must be respawned with `--conversation <native_session_id>`.
  4. **Approval wait longer than 25 s** is denied by the helper's default timeout (section 3, question 3).
  5. **`ask_question` and the gate**, and **tool classes not verified live** (section 6.4).
