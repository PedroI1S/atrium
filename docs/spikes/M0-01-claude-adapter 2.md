# M0-01: Claude Code adapter

Status: done. Code in `spikes/agent-claude`, fixtures in `fixtures/agents/claude` (20 scenarios, all `live`, model `haiku`, `claude` 2.1.308).

## What was built

- `ClaudeNormalizer` (pure): stream-json + control protocol records -> `AgentEvent`. Golden-tested over the live fixtures plus 28 hand-made normalizer tests, 6 unit tests, and 15 adapter tests against an offline fake CLI.
- `ClaudeAdapter` (`AgentAdapter`): spawns `claude -p --input-format stream-json --output-format stream-json --verbose --include-partial-messages --permission-prompt-tool stdio --strict-mcp-config --mcp-config <generated>`, with `--permission-mode bypassPermissions` only for `FullAccess`. `Ask` and `ReadOnly` use Claude's normal mode (Claude has no read-only mode); in both, every decision goes to the gate.
- Examples: `record` (re-records any scenario, `--list`, `all`; also acts as the MCP test server) and `chat` (events as JSONL on stdout; gate decisions and commands as JSON lines on stdin).
- `fake_claude` test binary: offline stand-in for the CLI, used by the saturation, fail-closed, queue and argument tests.

## How the gate sees every tool call

1. `initialize` control request registers a `PreToolUse` **hook callback** (SDK-style, no external process, no user settings touched).
2. Before every tool call Claude Code sends a `hook_callback` control request with `tool_name`, `tool_input`, `tool_use_id`. The adapter calls `SessionConfig::gate` and answers `allow`, `deny` (+ reason) or `ask`.
3. `ask` makes Claude Code send a `can_use_tool` request for the same `tool_use_id`; that becomes `ApprovalRequested`, and `ResolveApproval` becomes the `control_response`. `RequestResolved` follows.
4. A `can_use_tool` that no hook `ask` preceded goes through the gate again. `AskUserQuestion` becomes `QuestionAsked` / `AnswerQuestion` (answers go back as `updatedInput.answers`, keyed by question text).
5. `--permission-prompt-tool stdio` is a hidden flag. Without it `AskUserQuestion` is disabled in `-p` mode and `can_use_tool` is not used.

`interception()` is `AllToolCalls` unless Claude Code's own settings contain hooks that run outside the gate; then it is `ProviderGated` (partial). See "External settings and the enforcement level".

### Findings that shaped the design (all measured on 2.1.308)

- The hook fires in every permission mode, including `bypassPermissions`, for built-ins, MCP tools, subagent tools and `ToolSearch`. A hook `deny` wins over bypass; a hook `ask` still prompts in bypass mode.
- **Claude Code fails open on a bad hook answer.** A `control_response` with `subtype: error`, or a success whose payload has no valid decision, lets the tool run (the secret file was read in both probes). Only a hook *timeout* fails closed. So the adapter never sends an error: gate panic, gate timeout (default 20 s, hook timeout is that plus 10 s), malformed hook request, unknown callback id all answer `deny`. Tests: `gate_failures_deny`, `malformed_and_foreign_hook_requests_are_denied`, and `can_use_tool_follows_the_gate_and_fails_closed` (panic and hang on the `can_use_tool` path too).
- `strict-mcp-config` does not remove the user's claude.ai connector tools (they showed up as deferred tools in `ToolSearch`). The adapter sets `ENABLE_CLAUDEAI_MCP_SERVERS=false` for the child, which removes them. They would have been gated anyway.
- `Grep` and `Glob` do not exist in this build's headless tool list; the model searches with `Bash` (`grep`, `find`). `TodoWrite` is not available either, so `PlanUpdated` is covered by a hand-made test only.

## Enforcement matrix

| Tool class | Intercepted before execution? | How | Evidence |
|---|---|---|---|
| Read | yes | PreToolUse hook callback | `gate-deny` (denied; canary contents appear nowhere, asserted on the jsonl and the events) |
| Edit / write | yes | hook | `gate-deny-edit` (Write denied, file absent) |
| Delete / move | yes, as Execute | hook on `Bash` (`rm`, `mv`); Claude has no delete tool | `gate-deny-delete` (file survives) |
| Execute | yes | hook; `ask` -> `can_use_tool` | `gate-deny-exec`, `gate-deny-fullaccess`, `approval-allow`, `approval-deny` (`pwned.txt` absent) |
| Search | yes | hook on `Bash` grep/find and on `ToolSearch` (kind `Search`) | `gate-deny` (Bash attempts), `mcp-allow` / `gate-deny-mcp` (ToolSearch passes the gate). No dedicated deny fixture for `ToolSearch`. |
| Fetch / web | yes | hook | `gate-deny-fetch` (WebFetch). WebSearch uses the same path (manual probe, no fixture). |
| MCP | yes | hook (`mcp__<server>__<tool>`, kind `Mcp`) | `gate-deny-mcp` (server never called) with `mcp-allow` as control (server called) |
| Subagents | yes, both the launch and the subagent's own calls | hook (input carries `agent_id`) | `gate-deny-subagent` (Agent launch passes, its `Read` of secret.txt is denied) |
| FullAccess (`bypassPermissions`) | yes | hook | `gate-deny-fullaccess` |
| AskUserQuestion | yes (gate sees it; the question is the interaction) | hook, then `can_use_tool` | `question` |
| Other provider tools (Skill, Cron*, Monitor, Worktree...) | yes (the hook is generic) | hook | manual probes only (Task/Agent, ToolSearch, Skill); no fixtures |

| **Hooks in Claude Code's own settings** (user, project, local, managed) | **partial** while present | They run on the host outside the gate; a `PreToolUse` one can rewrite a call's input after the gate approved it. The adapter detects them (`detect_external_hooks`) and reports `Interception::ProviderGated` with the reason; see below. | `settings.rs` unit tests, `external_hooks_make_the_enforcement_partial_and_say_why` |

No tool class slipped through the adapter's own hook; no escalation needed. The one gap is settings the user keeps on purpose, below.

### External settings and the enforcement level

Claude Code loads user, project and local settings by default and runs their hooks. Atrium keeps that default (agents keep their native configuration, PLAN §3.2; the user decides, D-011). So the adapter does not disable it; it **reports** it:

- `ClaudeOptions::setting_sources`: `None` (default) = Claude's native behaviour; `Some("")` = no external settings; `Some("project,local")` etc. map to `--setting-sources`.
- `ClaudeAdapter::enforcement_for(cwd, env)` reads the settings files that would apply (managed settings always; user dir from `CLAUDE_CONFIG_DIR` or `~/.claude`; `.claude/settings.json` and `settings.local.json` in the cwd and every parent) and lists every hook handler (any event, any handler type). `Enforcement { interception, external_hooks, reason }`.
- Any such hook makes the level `Interception::ProviderGated` ("partial", D-012), with a reason naming the hooks and, for `PreToolUse`, the input-rewrite risk. `ClaudeAdapter::interception()` returns the level of the last `start` (or of the current directory before any); `last_enforcement()` returns the details; `start` records the reason as an `atrium:` stderr input; the `chat` example prints it.
- Not inspected: hooks shipped by enabled plugins and `--settings` given in `extra_args` (so a machine with such hooks can still read as full). The daemon should treat the level as a lower bound.
- Fixtures are recorded with `setting_sources = Some("")` and are independent of the recording machine's settings.

## Measurements

From the `ms` column of the 18 non-error fixtures (`ms` = since the adapter started the process; one machine, warm cache, `haiku`; reproduce with `cargo run -p spike-agent-claude --example record -- <scenario>` and read the `.jsonl`):

| Milestone | median | min / max |
|---|---|---|
| `initialize` answered (`start` returns) | 506 ms | 452 / 949 |
| `system/init` record | 551 ms | 481 / 997 |
| `SessionStarted` (first real model response begins) | 1945 ms | 1438 / 2650 |

`SessionStarted` is deliberately late. Claude Code emits `system/init` before it has called the API, so a bad model id produces an `init` too. The normalizer holds `SessionStarted` (and `TurnStarted`) until the first non-synthetic API response, which makes `startup-error` a proper pre-session failure (`Error { fatal: true }`, `SessionEnded`, no `SessionStarted`). A provider limit (`rejected` rate-limit record, or the synthetic `rate_limit` message) counts as the provider answering, so it establishes the session.

## Saturation, backpressure, shutdown

Test `saturation_unread_events_still_obey_interrupt_and_shutdown` (offline, `fake_claude` in `flood` mode): nobody reads events; the channel is full (asserted); `Interrupt` reaches the child inside `SHUTDOWN_GRACE`; `Shutdown` ends the session task inside the grace, the child pid is gone, and `num_alive_tasks()` is 0. Mechanism: the adapter is one task; it stops reading the child's pipe once 4096 events are queued (the pipe applies backpressure), and commands, interrupt deadlines and the shutdown path never wait on the event channel. Closing events get 300 ms to reach a reader, then are dropped. A child that ignores `Interrupt` is killed after `SHUTDOWN_GRACE` and the session resumes (`--resume`) with the next message (`stuck_child_is_killed_after_interrupt_and_resumed`). The child leads its own process group and every kill is a group kill, so Bash tasks and MCP servers die with it; bounded drains mean a grandchild holding the pipes can't stall shutdown (`shutdown_kills_the_whole_process_group`). The respawn handshake runs inside the main loop with its own deadline, so `Shutdown` stays prompt while a resumed process starts (`shutdown_during_a_slow_respawn_handshake_is_prompt`).

Real interrupt: Claude Code answers the `interrupt` control request, writes the tool result "The user doesn't want to proceed", then a `result` with `subtype: error_during_execution`, and stays alive for the next turn (`interrupt`, `interrupt-resume`). The turn ends `interrupted`, the open call `cancelled`. After the client's `Shutdown` the process exits with code 1.

## Unmapped and deliberately ignored records

- Stay `Unmapped`: `system/task_started`, `system/task_updated`, `system/task_notification` (subagent bookkeeping; `task_progress` maps to `ToolCallUpdated`). Only `gate-deny-subagent` has them. `system/background_tasks_changed` and anything new stay `Unmapped` by design.
- Ignored on purpose (progress noise, listed in `normalizer.rs`): `system/status`, `system/thinking_tokens`, `system/post_turn_summary`, `system/task_summary`, `keep_alive`, our own `control_response`s, structural stream events (`content_block_start/stop`, `message_stop`, `signature_delta`, `input_json_delta`), the subagent's echoed prompt, and the "[Request interrupted by user]" marker.
- Thinking blocks arrive with empty text in 2.1.308 (even with `--thinking-display summarized`): `Reasoning` messages are mapped (hand-made test) but never appear in live fixtures.

## Other mapping decisions

- `AskUserQuestion` is *virtual*: no `ToolCall*` events, only `QuestionAsked`, and only once the gate has let it through (at the hook, or on `can_use_tool` if the hook did not decide it). `TodoWrite` is a normal tool call plus a `PlanUpdated`, so an approval for it refers to a call the client has seen. An approval whose call has not started yet (a subagent can ask first) opens the call first.
- Pending approvals and questions are cancelled when the turn ends (`result`), inside the turn. `can_use_tool` requests without a `tool_use_id` are matched to the gate decision by tool name and input. Records the adapter does not map are held back until after `SessionStarted`.
- Gate `Deny` and a denied approval finish the call immediately as `denied`; the provider's later error result is swallowed.
- `Usage`: running sum of per-message usage in the turn (subagents and reasoning tokens included); the `result` replaces it with the difference of `modelUsage` totals (session-cumulative over every model and subagent; `result.usage` leaves subagents out) and of `total_cost_usd`. A total that went down means a new process, so counting restarts from zero.
- Quota: `rate_limit_event.unifiedWindows` -> `QuotaUpdated` (`5h`, `weekly`), `utilization` is a 0..1 fraction (confirmed in the CLI's schema text) times 100; `resetsAt` seconds -> ms. One `RateLimited` per refusal, carrying the CLI's message (`rate-limited` fixture, a real refusal captured when the account hit its 5 h limit during recording).
- Queue: the adapter keeps the waiting texts; the normalizer counts them and emits the overflow `Error { fatal: false }` (max 8), so a replay refuses the same messages.
- Per-session MCP: `--mcp-config <tmp>/mcp.json` (mode 0600, removed with the session) + `--strict-mcp-config`; the user's MCP config is untouched.

## Stability of the control protocol

Moderate. Everything used is the same set the Agent SDK uses (`initialize` with hooks, `hook_callback`, `can_use_tool`, `interrupt`, `control_cancel_request`), and the CLI advertises a `capabilities` list in `system/init`, which is the right thing to feature-detect. But: `--permission-prompt-tool stdio` is undocumented in `--help`; the hook failure semantics (fail-open on error) are surprising and not documented; there are many unrelated record types (`task_*`, `background_tasks_changed`, `post_turn_summary`) that change between versions; `TodoWrite` and `Grep`/`Glob` vanished from headless tools. The golden fixtures are the early-warning system: re-record on each CLI upgrade and diff the `.events.json`. Recommendation: pin and test a CLI version range in M1, and make the first M1 test "the denied Read never runs".

## Is the `claude-code-acp` fallback needed?

No. The native control protocol reaches `AllToolCalls` including MCP and subagents, supports approvals, structured questions and interrupts. ACP would be a fallback only if a future CLI breaks the hook path; the fixtures would show it.

## Open risks

- Hook fail-open on malformed answers is a CLI behavior we depend on being stable; the adapter never sends one, but a CLI crash mid-hook is not something we can test.
- `--permission-prompt-tool stdio` could be renamed. The `question` and `approval-*` fixtures break first.
- Settings inherit by default (native behaviour, confirm with Pedro): external hooks make the level partial and are reported, not blocked; plugin hooks are not inspected.
- Recording used about 1% of a 5 h Claude window per scenario run, and an unrelated heavy day pushed the account to its limit; re-record in small batches.
- Not covered by fixtures: multi-select answers (unit-level only), `WebSearch`, `Skill`, plan mode, background Bash tasks, auth failure (same synthetic-error path as `startup-error`).
