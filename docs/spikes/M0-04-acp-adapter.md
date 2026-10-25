# Spike Report: M0-04 — ACP Adapter (`opencode acp`)

Status after **fix round 2**. Round 1 injected a per-session `ask` config; the second live recording (OpenCode 2.1.3, `opencode/deepseek-v4.1-flash-free`, commit 114a35c) confirmed it: read, shell, edit, search and the question tool all reached the gate, every `gate-deny*` was denied (with and without client capabilities) and the canary never appears. It also found OpenCode's `execute` tool (Code Mode) completing without a permission request (section 4); round 2 disables it per session and adds a recorder check for any such call. Round 2's `execute` deny and the MCP change are **not proven live yet**: the orchestrator re-records (section 8). `interception()` stays `ProviderGated`.

## 1. Summary & Pinned Crate Version

- **Crate**: `spikes/agent-acp` (`spike-agent-acp`)
- **ACP SDK pinned**: `agent-client-protocol = "=3.5.0"` (`features = ["unstable"]`), which pulls `agent-client-protocol-schema = "1.12.0"`.
- **ACP protocol version**: `1` (`ProtocolVersion::V1` on the wire).
- **Target agent**: OpenCode `2.1.3` (`opencode acp`; the `initialize` response reports `agentInfo: {name: "OpenCode", version: "2.1.3"}`). The adapter is generic: other ACP stdio agents work through `AcpAdapter::with_command`, without the OpenCode config injection.
- **Reported `Interception`**: `ProviderGated`.

### Why the adapter uses typed `schema::v1` over raw line framing

`ClientSideConnection` needs `!Send` async I/O on a `LocalSet` and hides the wire lines. `atrium-agents` needs a `Send` `start` future and a normalizer that sees every `AdapterInput` (stdin, stdout, stderr, gate decisions, commands, exit) so live runs and replays share one code path. The adapter therefore uses the schema types for (de)serialization and frames newline-delimited JSON-RPC itself over `tokio::process` pipes.

## 2. Live finding 1 (first recording, `M0-04-live-1`): the gate was bypassed

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
- `{"execute", "*", "deny"}` after the ask rule, global and on the two subagents (section 4, Code Mode).
- `SessionConfig::mcp_servers` become `mcp.servers.<name>` with `codemode: false` in the same document, and are not sent in ACP `session/new` while the config is injected (section 4, MCP).
- Every `session/request_permission` goes through `SessionConfig::gate`. `Allow` answers the `allow_once` option (`once`), `Deny` the `reject_once` option (`reject`), `Ask` emits `ApprovalRequested` first. An agent-side `allow_always` is **never** selected (OpenCode would save it as a durable project rule that skips the gate from then on); if there is no once-option, the request is cancelled, which is a denial.
- `PermissionPolicy`: `ReadOnly` additionally selects OpenCode's `plan` mode (it denies edits); `Ask` and `FullAccess` keep the default agent. The `ask` rule applies in every policy, so in `FullAccess` the gate still sees every call and applies the user's rules, as D-011 requires; the gate, not OpenCode, decides to let everything through.

## 4. Enforcement matrix (`docs/contracts/m0.md` §3.1)

Columns: what the first recording showed (default permissions), what the second recording showed with the injected `ask` config (`tool-*`, `approval-*`, `gate-deny*`, `question` fixtures; each `.md` lists the calls and whether they reached the gate), and what is still to prove.

| Tool class | OpenCode action / tool | Default config (live run 1) | With injected `ask` (live run 2) | How | Evidence |
| :--- | :--- | :--- | :--- | :--- | :--- |
| Read | `read` | **No**: canary read natively | **Yes**: gated; denied read never ran, canary absent (with and without client caps) | `session/request_permission` | `tool-read`, `gate-deny`, `gate-deny-no-caps` |
| Edit / write / patch | `edit` (`write`) | **No** | **Yes**: asked, approved and denied both resolve | `session/request_permission` | `approval-allow`, `approval-deny` |
| Execute (shell) | `shell` | **No**: `cat secret.txt` ran | **Yes**: denied command never ran | `session/request_permission` | `gate-deny-execute`, `-no-caps`, `approval-deny` |
| Delete / move | `shell` (`rm`, `mv`) | **No** | **Yes**, as `shell` | same | shape of `approval-deny` (`rm -rf`) |
| Search | `grep`, `glob` | **No** | **Yes** | same | `tool-search` |
| Question | `question` | n/a | **Yes**: asks, then `elicitation/create` | same, then elicitation | `question` |
| **Code Mode** | **`execute`** (ACP kind `other`) runs model-written JavaScript with a raw `fetch` and nested tool calls | not probed | **No: completed with no `session/request_permission`** despite the `* ask` rule (`fetch("https://example.com")` ran) | none | `tool-fetch` (call `call_ebd1a2e3ecdb47b39eeaa90c`, kept as evidence) |
| Fetch / web | `webfetch`, `websearch` (the model used Code Mode instead) | not probed | not exercised: the model chose `execute` | expected `session/request_permission` | re-record `tool-fetch` |
| Outside the workspace | `external_directory` | No (default is allow) | covered by `*` ask; not probed | same | not probed |
| MCP tools | `<server>_<tool>`, or via Code Mode | not probed | not probed | see below | **not probed** |
| Subagents | `subagent`; the child's own calls | not probed | not probed; the injected rule on `explore`/`general` is best effort (order against the shipped agent rules is undocumented) | same | **not probed** |
| Client `fs/*`, `terminal/*` | n/a | **Never used** | **Never used** | n/a | all recordings |

### Code Mode (`execute`): cannot be gated, so it is disabled per session

What it is (2.1.3 binary strings and the v2 permission docs): a tool named `execute` that "runs JavaScript in a confined Code Mode runtime to script tool calls and HTTP requests". Inside the code, `fetch` is available for raw HTTP and every other tool is reached through `tools.<namespace>[...]`. Its input is `{ code }`. The permissions docs list the action `execute` (resource `*`) and say "a deny removes Code Mode and the tools reached only through it, while nested tools enforce their rules".

What the recording shows: with `permissions: [{* * ask}]` in force (the other recordings with the same config asked for every `read`, `shell`, `edit`, `grep` and `question` call), `execute` and the `fetch` inside it completed with no permission request, and `rawOutput.metadata.toolCalls` shows a nested `fetch` that ran unasked. So `ask` on `execute` is not honoured, and the HTTP request inside the code never meets a permission check.

Fix: `{"action":"execute","resource":"*","effect":"deny"}` is added after the ask rule, in the global rules and in the `explore`/`general` agents. This is **disabling, not gating**: the one documented effect of a deny on `execute` is that Code Mode is removed, and it is the only documented control. Consequences:

- The model must use `webfetch` / `websearch` / `shell` directly, which are gated.
- By default OpenCode exposes MCP tools only through Code Mode ("they cannot be called directly"; config `mcp.servers.<name>.codemode` "defaults to true"). Denying `execute` would make them unreachable, so when the config is injected the adapter puts `SessionConfig::mcp_servers` into the config as `mcp.servers.<name>` with `codemode: false` and does not pass them in ACP `session/new` (the ACP path would register them with the default, Code Mode only). With `codemode: false` they should be direct tools `<server>_<tool>`, covered by the `*` ask rule. Not verified live.
- MCP servers in the user's own config keep `codemode: true`, so they are unreachable in an Atrium session. That is the safe direction.
- The recorder's `tool-execute` probe asks the model to use `execute`; the expected result is a model that has no such tool. If the tool still exists and runs, the new any-scenario check (section 6) fails the recording (exit 3).

Why `ProviderGated` stays: OpenCode enforces this itself; MCP tools and subagent internals are unproven; the Code Mode disable is untested live; and a few things can still skip the ask:

- saved "always" approvals in the user's OpenCode project state (durable `allow` rules from the user's own earlier use of OpenCode in the same project; Atrium never creates one);
- user-defined agents, or a system/managed config (`/Library/Application Support/opencode`, managed preferences), which rank above ours;
- anything a tool does without going through OpenCode's permission check (Code Mode was one);
- a `reject` reply makes OpenCode reject every other pending request of that session, **and end the turn**: after Atrium's `Deny`, OpenCode answers `session/prompt` with `stopReason: "cancelled"`, which the normalizer maps to `TurnOutcome::Interrupted`. Live in `gate-deny`, `gate-deny-execute` and `approval-deny`. A denied call therefore ends the turn instead of letting the model carry on.

Also seen live: `interrupt` sent `session/cancel` right after the `shell` tool call was announced, before its permission request; the call ended `failed`, never reached the gate and never ran.

`interception()` may move to `AllToolCalls` only when a re-recording proves every row, MCP and subagent calls included, and `tool-execute` shows Code Mode gone.

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
- **Any-scenario check**: in every scenario the recorder fails with exit 3 (fixtures still written, with `GATE BYPASSED` in the `.md`) if any tool call has completed without a `Gate` for its call id (`detect_ungated_completions`). A call that failed or was cancelled before the gate did not run and is not flagged. `tests/golden.rs` runs it over every golden scenario, and `live_tool_fetch_evidence_is_flagged_by_the_ungated_check` pins it on the real `tool-fetch` recording.
- **Recorder self-check** (`src/selfcheck.rs`): for `gate-deny*` the recorder fails (exit 3, still writing the fixtures as evidence, with `GATE BYPASSED: record as ungated in the matrix` in the `.md`) if the canary appears anywhere in the raw stream, if a tool call of a denied class completed, or if tool calls completed without the gate ever being consulted. Every recording's `.md` lists each tool call and whether it reached the gate, which is the evidence for the matrix. `tests/golden.rs` runs the same check over the committed fixtures.
- The recorder creates its own scratch directory (`Cargo.toml`, `secret.txt` with the canary, `notes.txt`), so it runs from the repository root. `resume` now really resumes: a first process creates the session, the recorded second process resumes it by native id. `interrupt` interrupts a running `sleep 30`.
- `examples/replay.rs` replays a recorded `.jsonl` through the normalizer (`--write` regenerates `.events.json`).

### Lifecycle on `session/load` and `session/resume` (round 2)

`session/load` makes OpenCode replay the old conversation as `session/update`s (`user_message_chunk`, `agent_message_chunk`, ...) before it answers the request, so the first stdout record after the request is a `MessageDelta`, not the start of a session. The event contract has no history event and requires `SessionStarted` first. Decision: **replayed history is not turned into events**. The normalizer treats every `session/update` between a `session/load` / `session/resume` request and its response as history, counts it by update type, and emits one `Unmapped { record_type: "session/load:history_replay", raw: {"updates": {"agent_message_chunk": 1, ...}} }` right after `SessionStarted`. The raw lines stay in the fixture, nothing is dropped silently, and the client does not see old messages as new ones. If the UI ever wants the history, it can come from a future contract event or from the daemon's own log; the adapter would only need to hold the replayed updates instead of counting them.

Related and general: any `session/update` that arrives before the session is established (for example `available_commands_update` between the `session/new` response and a pending model selection) is held and emitted right after `SessionStarted`; if the session never starts, it is dropped, so a pre-session failure still begins with its `Error`. Tests: `replayed_history_is_reported_not_turned_into_events`, `updates_before_the_session_exists_wait_for_session_started_or_vanish`, the `resume` and `startup-error` fixtures (re-normalized offline with `examples/replay.rs`, not re-recorded).

## 7. Recommendation for M1

Keep the generic ACP adapter for ACP agents that ask by default (and for the ones that use the client `fs`/`terminal` capabilities). For OpenCode itself, **plan an OpenCode-specific adapter over `opencode serve`**, unless the re-recording proves the ACP path covers every row of the matrix. Reasons, from MonoCode (MIT; used as a reference for protocol shapes only, no code copied), which drives OpenCode through its HTTP server:

- **Per-session permission rules at session creation**: `POST /session` takes the rules (v1 `{permission: [{permission, pattern, action}]}`, v2 `{permissions: [{action, resource, effect}]}`), for example `[* ask, question allow]`, or a full `* allow` for full access. That is a real per-session setting; the ACP path only has per-process environment.
- **Model and variant per prompt** (v1 `prompt_async {model, variant, agent, parts}`; v2 sets them with `setModel` first) instead of config options after session creation.
- Permission replies are explicit (`POST /permission/{id}/reply {once|always|reject}` or the v2 session route), and interrupt is `/abort` (v1) or `/interrupt` (v2).
- The Code Mode finding is a second reason: with the HTTP server the adapter sees tool events and permission requests of the whole server API, and can deny a tool at session creation (`execute` `*` deny) as a documented per-session rule rather than through a process-wide environment config.
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
- `tests/golden.rs`: replays 15 scenarios (live fixtures where recorded) (`simple`, `tool-read`, `gate-deny`, `gate-deny-execute`, `approval-allow`, `approval-deny`, `question`, `interrupt`, `multi-turn`, `resume`, `startup-error`, `model-select`, and the client-capability variants `tool-read-fs`, `gate-deny-fs`, `gate-deny-terminal`), checks golden events, invariants, `Sanitizer::is_clean`, no temp path, no canary, and the bypass self-check; plus normalizer unit tests for model confirmation and the top-level tool name.
- `tests/live_adapter.rs` (a mock ACP agent over real stdio): saturation (§3.2), fs/terminal gate enforcement, approval and question round trips, pre-session failure, model selection, bad model (rejected and silently ignored), injected config (inline = file, `ask` rule, file removed), startup timeout, permission option choice (never `allow_always`).
- `src/*` unit tests: model state parsing, config merge, sanitizer, self-check, permission option choice.

All fixtures are `hand-made` until re-recorded. The OpenCode-shaped ones (`gate-deny`, `gate-deny-execute`, `tool-read`, `model-select`, `startup-error`, `approval-*`) are shaped after the first live recording (real `configOptions`, top-level tool `name`, option ids `once`/`always`/`reject`).

### Live re-recording (orchestrator, on the host)
From the worktree root (the recorder makes its own scratch directory):
```bash
cd /Users/pedro/Documents/atrium-wt/M0-04-acp-adapter
cargo build -p spike-agent-acp --examples
for s in simple tool-read gate-deny gate-deny-execute approval-allow approval-deny question interrupt multi-turn resume startup-error model-select tool-search tool-fetch tool-execute; do
  cargo run -q -p spike-agent-acp --example record -- $s || echo "!! $s exited $?"
done
cargo run -q -p spike-agent-acp --example record -- --no-client-caps gate-deny
cargo run -q -p spike-agent-acp --example record -- --no-client-caps gate-deny-execute
cargo test -p spike-agent-acp
```
(bash syntax; in fish use `for s in ...; ...; end`.) Exit code 3 means the gate was bypassed: the fixtures are still written, the class is recorded as ungated in the matrix, and the M1 recommendation above becomes the plan. `ATRIUM_ACP_MODEL` and `ATRIUM_ACP_SELECT_MODEL` override the models. Check a recording offline with `cargo run -p spike-agent-acp --example replay -- fixtures/agents/acp/<name>.jsonl`. `tool-search`, `tool-fetch`, `tool-execute` and the `-no-caps` recordings are matrix probes; they are not golden scenarios.

## 9. OpenCode ACP feature support (v2.1.3, from the live `initialize` and the binary)

| ACP feature | Supported | Notes |
| :--- | :--- | :--- |
| `initialize` | Yes | `protocolVersion: 1`; `authMethods: opencode-login`. |
| `session/new` | Yes | `cwd`, `additionalDirectories`, `mcpServers` only (stdio and HTTP; SSE rejected). Returns `sessionId` and `configOptions` (`model`, `effort`, `mode`); no `modes`/`models` state. |
| `session/load`, `session/resume` | Yes | `loadSession: true`, `sessionCapabilities.resume`; also `fork`, `list`, `close`, `delete`. Used when `SessionConfig::resume` is set. `session/load` replays the old conversation as `session/update`s before it answers (live); see "Lifecycle on session/load". |
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

`AcpNormalizer` never drops an unknown record. Unmapped: `session/load:history_replay` (a count of the history replayed by `session/load`), `session/update:available_commands_update` (seen live), `current_mode_update`, `config_option_update`, `session_info_update`, any unknown `session/update:<tag>`, unknown `rpc:<method>`, non-JSON / non-object stdout, `rpc_response:unknown_id`.

## 11. Open risks

1. The injected `ask` config is proven live for read, shell, edit, search and question (run 2), so the environment config does reach the session. Still unproven: that `deny` on `execute` removes Code Mode (`tool-execute`), and `codemode: false` MCP servers as direct, gated tools.
2. MCP tools and subagent-internal calls are unprobed; `webfetch`/`websearch` have not been exercised (the model chose Code Mode).
3. Saved "always" approvals and managed config can override the per-session rules (section 4).
4. The exact OpenCode error text for an unknown model is unknown (the hand-made fixture invents one).
