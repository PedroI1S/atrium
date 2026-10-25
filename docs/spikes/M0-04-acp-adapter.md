# Spike Report: M0-04 — ACP Adapter (`opencode acp`)

Status after **fix round 1**: the adapter now makes OpenCode ask before every action and routes the asks to the gate, selects the model over ACP, bounds startup, and the recorder checks itself for a bypassed gate. The `ask` behaviour is derived from OpenCode's documentation and from the 2.1.3 binary, **not yet proven live**: the orchestrator re-records (commands in section 8). `interception()` stays `ProviderGated`.

## 1. Summary & Pinned Crate Version

- **Crate**: `spikes/agent-acp` (`spike-agent-acp`)
- **ACP SDK pinned**: `agent-client-protocol = "=3.5.0"` (`features = ["unstable"]`), which pulls `agent-client-protocol-schema = "1.12.0"`.
- **ACP protocol version**: `1` (`ProtocolVersion::V1` on the wire).
- **Target agent**: OpenCode `2.1.3` (`opencode acp`; the `initialize` response reports `agentInfo: {name: "OpenCode", version: "2.1.3"}`). The adapter is generic: other ACP stdio agents work through `AcpAdapter::with_command`, without the OpenCode config injection.
- **Reported `Interception`**: `ProviderGated`.

### Why the adapter uses typed `schema::v1` over raw line framing

`ClientSideConnection` needs `!Send` async I/O on a `LocalSet` and hides the wire lines. `atrium-agents` needs a `Send` `start` future and a normalizer that sees every `AdapterInput` (stdin, stdout, stderr, gate decisions, commands, exit) so live runs and replays share one code path. The adapter therefore uses the schema types for (de)serialization and frames newline-delimited JSON-RPC itself over `tokio::process` pipes.

## 2. Live finding (first recording, `M0-04-live-1`): the gate was bypassed

Recorded on the host with `opencode acp` v2.1.3, model `opencode/deepseek-v4.1-flash-free`, from a scratch directory, with the client advertising `fs.readTextFile`, `fs.writeTextFile` and `terminal`:

| Observation | Evidence |
| :--- | :--- |
| OpenCode **never called** `fs/read_text_file`, `fs/write_text_file` or `terminal/*`, with or without the capabilities. | all 13 recordings; the only server→client traffic was `session/update` |
| OpenCode **never sent** `session/request_permission`, in any scenario (also `approval-allow` / `approval-deny`). | same |
| `gate-deny`: OpenCode read `secret.txt` itself; the canary `TOP-SECRET-CANARY-7731` is in the stream. | `gate-deny.jsonl`, `gate-deny-no-caps.jsonl` |
| `gate-deny-execute`: OpenCode ran `ls -la` and `cat secret.txt` itself; the canary is in the output. | `gate-deny-execute.jsonl`, `-no-caps` |
| `approval-allow`: the `write` tool created the file with no prompt. | `approval-allow.jsonl` |

Cause (from `opencode debug agents` and OpenCode's v2 permission docs): every agent starts from the base policy `* * allow`, with `ask` only for `.env` reads, and OpenCode only sends `session/request_permission` for a rule that resolves to `ask`. In ACP the client `fs`/`terminal` capabilities are optional for the agent: the ACP file-system page only forbids calling them when they are not advertised, and describes them as a way to reach unsaved editor state. They are not an enforcement mechanism, and OpenCode does not use them.

This is the brief's escalation case; it is also reported in the handoff.

## 3. What the fix does

OpenCode's ACP `session/new` takes only `cwd`, `additionalDirectories` and `mcpServers` (read from the 2.1.3 binary); there is no permission or model parameter. The one lever an ACP client has is the configuration of the `opencode acp` process it spawns. Atrium starts one process per session, so config passed through that process's environment is per-session. Nothing in the user's own config files is read or written.

`AcpAdapter::new()` (the OpenCode default) spawns `opencode acp` with (`src/opencode_config.rs`):

- `OPENCODE_CONFIG_CONTENT`: inline config. In the 2.1.3 loader it ranks above project `opencode.json` files, which a repository could otherwise use to allow everything (order, low to high: global, `OPENCODE_CONFIG`, project, inline content, system/managed).
- `OPENCODE_CONFIG`: a generated file with the same document (the mechanism the orchestrator already proved for the model). A user's own `OPENCODE_CONFIG` is left alone; a user's own inline content is merged, with ours last. The file lives in a private temp dir and is removed when the session ends.
- The document: `permissions: [{"action":"*","resource":"*","effect":"ask"}]` (the last matching rule wins and global rules come after the defaults, so it replaces `* allow`; `*` covers reads), the same rule under `agents.explore` and `agents.general` (agent rules are appended after global rules, and `explore` ships `shell`/`read`/`grep` allows), and `model` when `SessionConfig::model` is set.
- Every `session/request_permission` goes through `SessionConfig::gate`. `Allow` answers the `allow_once` option (`once`), `Deny` the `reject_once` option (`reject`), `Ask` emits `ApprovalRequested` first. An agent-side `allow_always` is **never** selected (OpenCode would save it as a durable project rule that skips the gate from then on); if there is no once-option, the request is cancelled, which is a denial.
- `PermissionPolicy`: `ReadOnly` additionally selects OpenCode's `plan` mode (it denies edits); `Ask` and `FullAccess` keep the default agent. The `ask` rule applies in every policy, so in `FullAccess` the gate still sees every call and applies the user's rules, as D-011 requires; the gate, not OpenCode, decides to let everything through.

## 4. Enforcement matrix (`docs/contracts/m0.md` §3.1)

"Before fix" is what the live recording shows. "After fix" is what the injected config should do; it comes from OpenCode's v2 permission documentation (actions and resources; `ask` = "wait for a decision from the client") and from the 2.1.3 binary, where the ACP layer answers a `permission.asked` event with `connection.requestPermission` and the options `once` / `always` / `reject`. **It has not been run live yet.** The re-recording fills the last column.

| Tool class | OpenCode action | Intercepted before execution, before fix | After fix (expected) | How | Live evidence of the fix |
| :--- | :--- | :--- | :--- | :--- | :--- |
| Read | `read` | **No** (canary read natively) | Yes | `session/request_permission` (rule `* ask`; `*` includes `read`) | `gate-deny` (to record) |
| Edit / write / patch | `edit` | **No** (file written without a prompt) | Yes | `session/request_permission` | `approval-allow`, `approval-deny` (to record) |
| Delete / move | `shell` (`rm`, `mv`) | **No** | Yes, as shell | `session/request_permission` | shape of `gate-deny-execute` (to record) |
| Execute | `shell` | **No** (`cat secret.txt` ran) | Yes | `session/request_permission` | `gate-deny-execute` (to record) |
| Search | `grep`, `glob` | **No** | Yes | `session/request_permission` | `tool-search` (to record) |
| Fetch / web | `webfetch`, `websearch` | **No** | Yes | `session/request_permission` | `tool-fetch` (to record) |
| Outside the workspace | `external_directory` | No (default is allow) | Yes | same | not probed |
| MCP tools | `<server>_<tool>` | not probed | Expected yes (docs list it; `*` matches) | same | **not probed** |
| Subagents | `subagent`; the child's own calls | not probed | Spawn: expected yes. Child calls: depend on the agent rules; the injected rule on `explore` and `general` is best effort (the order against the shipped agent rules is not documented) | same | **not probed** |
| Client `fs/*`, `terminal/*` | n/a | **Never used** by OpenCode | Never used | n/a | all recordings |

Why `ProviderGated` stays: OpenCode enforces this itself, MCP tools and subagent internals are unproven, and a few things can still skip the ask:

- saved "always" approvals in the user's OpenCode project state (durable `allow` rules from the user's own earlier use of OpenCode in the same project; Atrium never creates one);
- user-defined agents, or a system/managed config (`/Library/Application Support/opencode`, managed preferences), which rank above ours;
- anything a tool does without going through OpenCode's permission check;
- a `reject` reply makes OpenCode reject every other pending request of that session.

`interception()` may move to `AllToolCalls` only when the re-recording proves every row, MCP and subagent calls included.

## 5. Model and effort selection

The old adapter put the model in `_meta`, which OpenCode ignores, so every run used the user's default model. Now:

- `session/new` still carries `_meta.model`, but only so the normalizer (and so a replay) knows what was requested.
- The adapter reads the `session/new` / `load` response (`configOptions` with `category: "model"`, or the unstable `models` state). If the agent is not running the requested model, or the id is not among the offered values, it sends `session/set_config_option {configId: "model", value}` (or `session/set_model` for the `models` state); with no model state at all it tries `configId: "model"` and lets the agent answer. The same id is also written into the injected config.
- The normalizer holds `SessionStarted` back until the model is confirmed (the response shows it, or the `set_config_option` response lists it as current). A rejected model, an OK that still reports another model, or an exit while waiting gives the **pre-session failure lifecycle** (`Error {fatal: true}` then `SessionEnded`, no `SessionStarted`). Tests: the `model_*` and `session_start_*` tests in `tests/golden.rs`, `model_selection_test` and `bad_model_is_pre_session_failure_test` in `tests/live_adapter.rs`, fixtures `model-select` and `startup-error` (hand-made, to be re-recorded).
- Replaying the old live `startup-error` recording through the new normalizer gives the pre-session failure instead of a hung session.
- **Effort / variant**: OpenCode exposes its variants as the config option `effort` (category `thought_level`, values `low`, `high`, `max`, `default`). `SessionConfig::effort` is applied with `session/set_config_option` after the session exists; a value the agent refuses is reported as a non-fatal `Error`. This is per session, not per prompt (the HTTP API takes `variant` per prompt, see section 7).

## 6. Startup, timeouts, sanitizer, recorder

- `start` bounds `initialize` + `session/new`/`load` + model selection by `STARTUP_TIMEOUT` (30 s; `AcpAdapter::with_startup_timeout` for tests), kills and reaps the child on expiry and returns `AdapterError::Handshake`. Test: `startup_timeout_test` (an agent that never answers `session/new`).
- The 27-minute `startup-error` hang was the recorder: with the model ignored, a session started, the scenario sent no prompt, and nothing waited with a limit. The recorder now has an overall per-scenario timeout (`ATRIUM_ACP_SCENARIO_TIMEOUT_SECS`, default 240; exit code 4, nothing written), and `startup-error` fails (exit 5) if a session starts with the bad model.
- **Sanitizer**: `src/sanitize.rs` wraps the contract `Sanitizer`. On decoded values it maps the scratch directory (realpath, the form without `/private`, and the form as given) to `/scratch`, any other `[/private]/var/folders/x/y/T` to `/tmp`, then applies the contract sanitizer; `is_clean` fails if a `/var/folders/`, `/private/var/` or `/private/tmp/` path is left. Tests in `src/sanitize.rs` (raw, prose, `file://`, sibling names) and a check in `tests/golden.rs` over every committed fixture.
- **Recorder self-check** (`src/selfcheck.rs`): for `gate-deny*` the recorder fails (exit 3, still writing the fixtures as evidence, with `GATE BYPASSED: record as ungated in the matrix` in the `.md`) if the canary appears anywhere in the raw stream, if a tool call of a denied class completed, or if tool calls completed without the gate ever being consulted. Every recording's `.md` lists each tool call and whether it reached the gate, which is the evidence for the matrix. `tests/golden.rs` runs the same check over the committed fixtures.
- The recorder creates its own scratch directory (`Cargo.toml`, `secret.txt` with the canary, `notes.txt`), so it runs from the repository root. `resume` now really resumes: a first process creates the session, the recorded second process resumes it by native id. `interrupt` interrupts a running `sleep 30`.
- `examples/replay.rs` replays a recorded `.jsonl` through the normalizer (`--write` regenerates `.events.json`).

## 7. Recommendation for M1

Keep the generic ACP adapter for ACP agents that ask by default (and for the ones that use the client `fs`/`terminal` capabilities). For OpenCode itself, **plan an OpenCode-specific adapter over `opencode serve`**, unless the re-recording proves the ACP path covers every row of the matrix. Reasons, from MonoCode (MIT; used as a reference for protocol shapes only, no code copied), which drives OpenCode through its HTTP server:

- **Per-session permission rules at session creation**: `POST /session` takes the rules (v1 `{permission: [{permission, pattern, action}]}`, v2 `{permissions: [{action, resource, effect}]}`), for example `[* ask, question allow]`, or a full `* allow` for full access. That is a real per-session setting; the ACP path only has per-process environment.
- **Model and variant per prompt** (v1 `prompt_async {model, variant, agent, parts}`; v2 sets them with `setModel` first) instead of config options after session creation.
- Permission replies are explicit (`POST /permission/{id}/reply {once|always|reject}` or the v2 session route), and interrupt is `/abort` (v1) or `/interrupt` (v2).
- It removes the reliance on environment-config precedence and on the background `opencode serve --service` that may already be running on the host.

The cost is a second, OpenCode-only adapter and version handling (v1 and v2 differ). Not built in this round: ACP is not proven a dead end; it awaits the live re-recording.

Design inputs credited: the ACP protocol documentation (agentclientprotocol.com, by Zed) for the `fs`/`terminal` capability semantics; MonoCode's OpenCode HTTP integration (MIT) for the per-session rule and per-prompt model shapes. I did not read Zed's own client code, so it did not inform this design.

## 8. Verification & reproducibility

### Offline (what a worker can run)
```bash
cargo fmt --check
cargo clippy -p spike-agent-acp --all-targets -- -D warnings
cargo test -p spike-agent-acp
```
- `tests/golden.rs`: replays 15 scenarios (`simple`, `tool-read`, `gate-deny`, `gate-deny-execute`, `approval-allow`, `approval-deny`, `question`, `interrupt`, `multi-turn`, `resume`, `startup-error`, `model-select`, and the client-capability variants `tool-read-fs`, `gate-deny-fs`, `gate-deny-terminal`), checks golden events, invariants, `Sanitizer::is_clean`, no temp path, no canary, and the bypass self-check; plus normalizer unit tests for model confirmation and the top-level tool name.
- `tests/live_adapter.rs` (a mock ACP agent over real stdio): saturation (§3.2), fs/terminal gate enforcement, approval and question round trips, pre-session failure, model selection, bad model (rejected and silently ignored), injected config (inline = file, `ask` rule, file removed), startup timeout, permission option choice (never `allow_always`).
- `src/*` unit tests: model state parsing, config merge, sanitizer, self-check, permission option choice.

All fixtures are `hand-made` until re-recorded. The OpenCode-shaped ones (`gate-deny`, `gate-deny-execute`, `tool-read`, `model-select`, `startup-error`, `approval-*`) are shaped after the first live recording (real `configOptions`, top-level tool `name`, option ids `once`/`always`/`reject`).

### Live re-recording (orchestrator, on the host)
From the worktree root (the recorder makes its own scratch directory):
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
(bash syntax; in fish use `for s in ...; ...; end`.) Exit code 3 means the gate was bypassed: the fixtures are still written, the class is recorded as ungated in the matrix, and the M1 recommendation above becomes the plan. `ATRIUM_ACP_MODEL` and `ATRIUM_ACP_SELECT_MODEL` override the models. Check a recording offline with `cargo run -p spike-agent-acp --example replay -- fixtures/agents/acp/<name>.jsonl`. `tool-search`, `tool-fetch` and the `-no-caps` recordings are matrix probes; they are not golden scenarios.

## 9. OpenCode ACP feature support (v2.1.3, from the live `initialize` and the binary)

| ACP feature | Supported | Notes |
| :--- | :--- | :--- |
| `initialize` | Yes | `protocolVersion: 1`; `authMethods: opencode-login`. |
| `session/new` | Yes | `cwd`, `additionalDirectories`, `mcpServers` only (stdio and HTTP; SSE rejected). Returns `sessionId` and `configOptions` (`model`, `effort`, `mode`); no `modes`/`models` state. |
| `session/load`, `session/resume` | Yes | `loadSession: true`, `sessionCapabilities.resume`; also `fork`, `list`, `close`, `delete`. Used when `SessionConfig::resume` is set. |
| `session/prompt`, `session/cancel` | Yes | Stop reason maps to `TurnOutcome`. |
| `session/set_config_option` | Yes | The way to set `model`, `effort`, `mode` (values are strings). |
| `session/set_mode` | Yes | Used only if the agent reports a `modes` state; OpenCode reports the mode as a config option. |
| Plans | Not seen | `todowrite` is off by default (experimental `todo`). |
| `session/request_permission` | Yes, **only for `ask` rules** | Options `once` (`allow_once`), `always` (`allow_always`), `reject` (`reject_once`). Not sent under the default policy. |
| `fs/*`, `terminal/*` (client) | Never called | Advertised in `initialize`; OpenCode runs its own tools. |
| `elicitation/create` | Via the question tool | The `question` scenario is to be recorded. |
| Usage | Yes | `usage_update` and `PromptResponse.usage`. |
| Tool shape | Non-standard `name` | `tool_call`/`tool_call_update` and the permission `toolCall` carry a top-level `name` (`shell`, `read`, `write`); the normalizer moves it to `_meta.toolName`. A call is announced with empty `rawInput`; the real input arrives with the permission request or the `in_progress` update, and the normalizer emits `ToolCallUpdated` for it. |

## 10. `AgentEvent::Unmapped` records

`AcpNormalizer` never drops an unknown record. Unmapped: `session/update:available_commands_update` (seen live), `current_mode_update`, `config_option_update`, `session_info_update`, any unknown `session/update:<tag>`, unknown `rpc:<method>`, non-JSON / non-object stdout, `rpc_response:unknown_id`.

## 11. Open risks

1. The injected `ask` config is unproven live through `opencode acp`. A background service (`opencode serve --service`) is running on the host; if ACP sessions are served by it rather than by the `acp` process, environment config may not reach it. The orchestrator's earlier `OPENCODE_CONFIG` model override worked, which suggests the config is read in the `acp` process, but permission rules in the same config are unverified.
2. MCP tools and subagent-internal calls are unprobed.
3. Saved "always" approvals and managed config can override the per-session rules (section 4).
4. The exact OpenCode error text for an unknown model is unknown (the hand-made fixture invents one).
