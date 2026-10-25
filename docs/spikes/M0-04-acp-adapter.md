# Spike Report: M0-04 — ACP Adapter (`opencode acp`)

## 1. Summary & Pinned Crate Version

- **Crate**: `spikes/agent-acp` (`spike-agent-acp`)
- **ACP SDK Pinned**:
  - `agent-client-protocol = "=3.5.0"` (with `features = ["unstable"]` for `usage_update`, `elicitation/create`, `session/resume`, and `config_option_update` schema types)
  - `agent-client-protocol-schema = "1.12.0"` (transitively pinned by `agent-client-protocol = "=3.5.0"`)
- **ACP Protocol Version**: `1` (`ProtocolVersion::V1` on the wire)
- **Target Agent Verified**: OpenCode CLI (`opencode acp`, v2.1.3 installed on host; protocol implementation compatible with any ACP v1 stdio agent including Gemini CLI `--acp`, Goose, and Kimi)
- **Reported `Interception`**: `Interception::ProviderGated`

### Why `AcpAdapter` Uses Typed `agent_client_protocol::schema::v1` Over Raw Line Framing

`agent-client-protocol` v3.5.0's `ClientSideConnection` requires `!Send` `futures::AsyncRead`/`AsyncWrite` on a `LocalSet` and hides the raw JSON-RPC wire lines from callers. Because `atrium-agents` requires:
1. `AgentAdapter::start` returning a `Send` future and `Session` channels, and
2. `Normalizer::on_input(&mut self, input: &AdapterInput)` operating purely on `AdapterInput::{Stdin, Stdout, Stderr, Gate, Command, Exit}` so live sessions and offline fixture replays share the exact same normalization code path,

`spikes/agent-acp` uses `agent_client_protocol::schema::v1` types for all request, response, and notification serialization and deserialization while framing newline-delimited JSON-RPC 2.0 directly over `tokio::process::ChildStdin` and `ChildStdout`. Every line crossing the process boundary is fed into `AcpNormalizer` and optionally recorded as a `FixtureLine`.

---

## 2. Enforcement Matrix (`docs/contracts/m0.md` §3.1)

ACP has a dual interception surface:
1. **Client Capabilities (`fs/read_text_file`, `fs/write_text_file`, `terminal/create`)**: When the client advertises `fs: { readTextFile: true, writeTextFile: true }` and `terminal: true` in `InitializeRequest.clientCapabilities`, compliant ACP agents delegate file reads, file writes, and command execution to the client over JSON-RPC. `AcpAdapter` intercepts every incoming `fs/read_text_file`, `fs/write_text_file`, and `terminal/create` request and consults `SessionConfig::gate` **before** touching the filesystem or spawning a process.
2. **Provider Permission Requests (`session/request_permission`)**: For tools executed inside the agent process itself (or when client `fs`/`terminal` capabilities are disabled), the agent sends `session/request_permission` before executing gated operations. `AcpAdapter` routes every `session/request_permission` call through `SessionConfig::gate`.

### 2.1 With Client `fs` + `terminal` Capabilities Advertised (`ClientCapabilityConfig::all()`, Default)

| Operation | Tool Kind | Intercepted Before Execution? | Mechanism | If `ToolGate` Returns `Deny` |
| :--- | :--- | :--- | :--- | :--- |
| File read (`read`) | `ToolKind::Read` | **Yes** (when routed via `fs/read_text_file`) / **No** (if agent reads directly from disk without `fs/read_text_file`) | Client RPC `fs/read_text_file` (or `session/request_permission` for outside-workspace paths) | Adapter returns JSON-RPC error `-32000` without reading disk; emits `ToolCallFinished { status: Denied }`. |
| File write / edit (`edit`, `write`, `patch`) | `ToolKind::Edit` | **Yes** | Client RPC `fs/write_text_file` and/or `session/request_permission` | Adapter returns JSON-RPC error `-32000` or selects `reject_once`; file is never modified; emits `ToolCallFinished { status: Denied }`. |
| File delete / move | `ToolKind::Delete` / `ToolKind::Move` | **Yes** (via permission or `terminal/create`) | `session/request_permission` or `terminal/create` | Blocked before execution; emits `ToolCallFinished { status: Denied }`. |
| Shell / command execution (`bash`) | `ToolKind::Execute` | **Yes** | Client RPC `terminal/create` and/or `session/request_permission` | Adapter refuses to spawn the command and returns JSON-RPC error `-32000` (or `reject_once`); emits `ToolCallFinished { status: Denied }`. |
| Search (`grep`, `glob`, `list`) | `ToolKind::Search` | **Provider-dependent** (`No` inside workspace by default) | Runs inside OpenCode's process using ripgrep; only prompts `session/request_permission` for `external_directory` | If `session/request_permission` is sent, `Deny` blocks it; otherwise runs in-process without a pre-hook. |
| Network / web fetch (`webfetch`, `websearch`) | `ToolKind::Fetch` | **Provider-dependent** | `session/request_permission` (when agent permission config is `ask`) | If `session/request_permission` is sent, `Deny` selects `reject_once`; otherwise runs in-process. |
| MCP tool calls (`mcp__*`) | `ToolKind::Mcp` | **Provider-dependent** | `session/request_permission` (when agent permission config is `ask`) | If `session/request_permission` is sent, `Deny` selects `reject_once`; otherwise runs in-process. |

### 2.2 Without Client `fs` + `terminal` Capabilities (`ClientCapabilityConfig::without_fs_and_terminal()`)

When `fs` and `terminal` client capabilities are **not** advertised (`--no-client-caps`):

| Operation | Tool Kind | Intercepted Before Execution? | Mechanism | Behavior in OpenCode (`opencode acp`) |
| :--- | :--- | :--- | :--- | :--- |
| File read (`read`) inside `cwd` | `ToolKind::Read` | **No** | Agent reads directly in-process; sends `session/update` (`tool_call` -> `tool_call_update`) | OpenCode's default permission for `read` inside the workspace is `allow`, so it does **not** send `session/request_permission` for workspace reads. |
| File read (`read`) outside `cwd` | `ToolKind::Read` | **Yes** | `session/request_permission` (`external_directory`) | OpenCode prompts via `session/request_permission`, which `AcpAdapter` gates. |
| File write / edit (`edit`, `write`) | `ToolKind::Edit` | **Mode-dependent** | `session/request_permission` | Gated when the agent's mode/permission policy is `ask`; bypassed in full-access modes if `fs/write_text_file` is disabled. |
| Shell execution (`bash`) | `ToolKind::Execute` | **Mode-dependent** | `session/request_permission` | Gated when the agent's mode/permission policy is `ask`; executed directly by OpenCode's internal shell runner once allowed or in full-access mode. |
| Search / Fetch / MCP | `Search` / `Fetch` / `Mcp` | **Mode-dependent** | `session/request_permission` | Only gated when OpenCode emits `session/request_permission`. |

**Conclusion on `Interception`**: Because ACP agents can execute in-process tools (`grep`, `glob`, in-process `read`, MCP tools) without routing every call through a mandatory pre-execution hook, `AcpAdapter::interception()` honestly reports **`Interception::ProviderGated`**, while advertising `ClientCapabilityConfig::all()` by default to maximize client-side interception of `fs` and `terminal` operations.

---

## 3. OpenCode ACP Feature Support Matrix

| ACP Feature / Method | Supported by OpenCode? | Wire Details & Adapter Handling |
| :--- | :--- | :--- |
| `initialize` | **Yes** | Negotiates `protocolVersion: 1`, returns `agentCapabilities` and `agentInfo: { name: "opencode", version: "1.2.15" }`. |
| `session/new` | **Yes** | Accepts `cwd` and `mcpServers` (`stdio`, `http`, `sse`); returns `sessionId`, `modes`, and `models`. |
| `session/load` (`loadSession`) | **Yes** | Advertised via `agentCapabilities.loadSession: true`. Replays prior history notifications or resumes the session in `cwd`, then returns `LoadSessionResponse`. Used by `AcpAdapter` when `SessionConfig::resume` is set. |
| `session/resume` (unstable) | **Yes** | Advertised via `agentCapabilities.sessionCapabilities.resume: {}`. Supported in `AcpNormalizer` alongside `session/load`. |
| `session/fork` / `session/list` (unstable) | **Yes** | Advertised via `agentCapabilities.sessionCapabilities.{fork, list}`. |
| `session/prompt` | **Yes** | Accepts `prompt: [ContentBlock::Text]`. Streams `session/update` notifications and resolves with `PromptResponse { stopReason, usage }`. |
| `session/cancel` | **Yes** | Notification sent on `SessionCommand::Interrupt` and `SessionCommand::Shutdown`. OpenCode aborts in-flight tool calls and resolves the active `session/prompt` with `stopReason: "cancelled"`. |
| `session/set_mode` | **Yes** | Switches between OpenCode agents/modes (e.g., `build`, `plan`). `AcpAdapter` maps `PermissionPolicy` (`Ask`, `ReadOnly`, `FullAccess`) to matching advertised modes after session creation. |
| `session/set_config_option` | **Partial / Newer versions** | Handled in `AcpNormalizer`; mode/config updates emit `AgentEvent::Unmapped` so no wire record is dropped. |
| Plans (`session/update: plan`) | **Yes** | OpenCode maps its `todowrite` / plan updates to `SessionUpdate::Plan`, which `AcpNormalizer` maps to `AgentEvent::PlanUpdated { entries }`. |
| Permissions (`session/request_permission`) | **Yes** | Sends `toolCall` details + `options` (`allow_once`, `allow_always`, `reject_once`). Routed through `SessionConfig::gate` (`Allow` -> `allow_once`, `Deny` -> `reject_once`, `Ask` -> `ApprovalRequested`). |
| Questions (`elicitation/create`) | **Protocol-supported** | Part of ACP unstable schema (`CreateElicitationRequest` with `mode: "form"` or `"url"`). Mapped to `AgentEvent::QuestionAsked` -> `SessionCommand::AnswerQuestion` -> `RequestResolved { outcome: Answered }`. |
| Usage (`usage_update` & `PromptResponse.usage`) | **Yes** | `SessionUpdate::UsageUpdate` and `PromptResponse.usage` (plus `_meta.usage`) are normalized to `AgentEvent::Usage` and `TurnCompleted.usage`. |

---

## 4. `AgentEvent::Unmapped` Records

Per the M0 contract, `AcpNormalizer` never silently drops an unmapped ACP notification or method. The following ACP records have no direct `AgentEvent` counterpart and are emitted as `AgentEvent::Unmapped { record_type, raw }`:

1. `session/update:available_commands_update` — Slash commands advertised by the ACP agent (`AvailableCommandsUpdate`) after `session/new`.
2. `session/update:current_mode_update` — Notification that the agent switched its internal mode ID (`CurrentModeUpdate`).
3. `session/update:config_option_update` — Notification that session configuration options changed (`ConfigOptionUpdate`).
4. `session/update:session_info_update` — Notification updating session title/metadata (`SessionInfoUpdate`).
5. Any unrecognized `session/update` tag (`session/update:<tag>`) or unrecognized JSON-RPC method (`rpc:<method>`).

---

## 5. Verification & Reproducibility

### Offline Golden & Contract Tests
```bash
cargo test -p spike-agent-acp
```
- `tests/golden.rs`: Replays all 10 scenarios in `fixtures/agents/acp/*.jsonl` (`simple`, `tool-read`, `gate-deny`, `gate-deny-execute`, `approval-allow`, `approval-deny`, `question`, `interrupt`, `multi-turn`, `resume`, `startup-error`), checks `Sanitizer::is_clean`, verifies against `*.events.json`, and asserts lifecycle/request/queue invariants.
- `tests/live_adapter.rs`: Spawns a live child process over stdio to verify:
  - **Saturation test (`docs/contracts/m0.md` §3.2)**: Floods > `2 * EVENT_BUFFER` (2,304) events while `session.events` is never read, then verifies `SessionCommand::Interrupt` and `SessionCommand::Shutdown` complete well within `SHUTDOWN_GRACE` (5s) both for cooperative and stubborn (ignoring EOF) processes.
  - **Live `ToolGate` enforcement**: Verifies `fs/read_text_file` and `terminal/create` against a canary secret file (`TOP_SECRET_CANARY_VALUE_9941`), confirming that denied calls never read the file or leak the canary into events or recorded transcripts.
  - **Live `ApprovalRequested` and `QuestionAsked` round-trips**.
  - **Live pre-session startup error lifecycle** (`Error { fatal: true }` -> `SessionEnded`, with no `SessionStarted`).

### Recording Live Fixtures from `opencode acp`
Because the worker sandbox restricts direct execution of `opencode`, the committed fixtures in `fixtures/agents/acp/` are marked `hand-made` from the exact `agent_client_protocol::schema::v1` types. To record live transcripts on a host with `opencode` configured:

```bash
# With client fs + terminal capabilities enabled (default):
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

# Without client fs + terminal capabilities:
cargo run -p spike-agent-acp --example record -- --no-client-caps tool-read
cargo run -p spike-agent-acp --example record -- --no-client-caps gate-deny
```
