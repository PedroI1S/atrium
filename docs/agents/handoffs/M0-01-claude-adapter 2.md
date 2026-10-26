# Handoff: M0-01-claude-adapter

Model: Sonnet 5.5 · Status: done (fix round 1 applied)

Spike report: [docs/spikes/M0-01-claude-adapter.md](../../spikes/M0-01-claude-adapter.md) (enforcement matrix, timings, protocol stability, fallback verdict).

## Done

- `ClaudeNormalizer` and `ClaudeAdapter` (`impl Normalizer` / `impl AgentAdapter`, `interception() = AllToolCalls`, or `ProviderGated` while external settings hooks exist; see Fix round 1) in `spikes/agent-claude`.
- Gate enforcement through an SDK-style `PreToolUse` hook callback registered in `initialize` (Claude Code's own settings are left as they are). `Allow` / `Deny` / `Ask` map to hook decisions; `Ask` continues as `can_use_tool` -> `ApprovalRequested` -> `RequestResolved`. Every error path answers `deny`.
- Mapping: partial messages -> `MessageDelta`, tool_use/tool_result -> `ToolCall*`, `AskUserQuestion` -> `QuestionAsked` (structured options, answers back through `updatedInput.answers`), `TodoWrite` -> `PlanUpdated` (hand-made test only), `result` -> `Usage` + `TurnCompleted`, rate-limit records -> `QuotaUpdated` / `RateLimited`, interrupt -> control-protocol interrupt, per-session MCP via generated `--mcp-config` + `--strict-mcp-config`, `FullAccess` -> `bypassPermissions`.
- 20 live fixtures (haiku, CLI 2.1.308), each with `.jsonl`, `.events.json` (golden) and `.md`: all nine required scenarios plus `gate-deny-exec`, `-edit`, `-delete`, `-fetch`, `-fullaccess`, `-mcp`, `-subagent`, `mcp-allow`, `interrupt-resume`, `rate-limited` (a real refusal captured when the account hit its 5 h limit).
- Saturation test, fail-closed gate tests, stuck-interrupt kill + resume, queue overflow, argument and temp-file tests against an offline `fake_claude` binary.
- `--example record` (re-record, `--list`, `all`) and `--example chat`.

## Not done

- `TodoWrite` and `Reasoning` text cannot be recorded live: 2.1.308 headless has no `TodoWrite` tool and thinking blocks arrive empty. Covered by hand-made tests only.
- No live deny fixtures for `WebSearch`, `Skill`, `ToolSearch`, Cron/Monitor tools (the hook is generic; manual probes only; see report).
- `ClaudeOptions::setting_sources` defaults to Claude's native behaviour (confirm with Pedro). Hooks in those settings make the level partial (reported, not blocked); plugin hooks are not inspected.
- The merge of sanitizer-spanning streamed deltas (`merge_split_deltas` in `record.rs`) never triggered in the final recordings, so that code path is unexercised.

## Definition of Done results

- Fixtures for every scenario in `docs/contracts/m0.md` §3.3 (1 to 9), recorded live, sanitized (recorder uses `Sanitizer`; `fixtures_are_sanitized` re-checks), each with a golden test: yes.
- Saturation test: `saturation_unread_events_still_obey_interrupt_and_shutdown` passes (~1.5 s fill + interrupt + shutdown, child reaped, 0 alive tasks).
- `cargo run -p spike-agent-claude --example record -- <scenario>`: works (all fixtures made with it). `--example chat`: ran a live session end to end.
- `cargo fmt --check`: clean.
- `cargo clippy -p spike-agent-claude --all-targets -- -D warnings`: clean.
- `cargo test -p spike-agent-claude`: 6 lib + 15 adapter + 14 golden + 28 normalizer tests, all passing (after fix round 1; it was 1 + 8 + 14 + 20 before).
- Spike report written; this handoff written.

## Gotchas

- **Claude Code fails open** on a hook answer that is an error response or has no valid decision (only a hook timeout fails closed). Never send those. Tested in `gate_failures_deny`; protocol probes are described in the report.
- `--permission-prompt-tool stdio` (hidden flag) is required for `AskUserQuestion` and `can_use_tool` in `-p` mode.
- `system/init` arrives before the model is called, so `SessionStarted` is held until the first real API response; a bad model is then a pre-session failure as the contract wants. Median spawn -> `SessionStarted` is about 1.9 s (handshake about 0.5 s).
- `strict-mcp-config` does not drop claude.ai connectors; the adapter sets `ENABLE_CLAUDEAI_MCP_SERVERS=false`.
- After an interrupt Claude Code stays alive and sends a `result` (`error_during_execution`). (An early probe of mine closed stdin on `result` and I briefly misread that as the process exiting; the fallback kill-and-`--resume` path remains for a child that ignores the interrupt.)
- Recording consumed a noticeable share of the account's 5 h window and the account hit its limit mid-session (the coordinator saw the rate-limit stop). Re-record in small batches.
- `Cargo.lock` is not part of this change; no contract file or root `Cargo.toml` was touched (`spikes/agent-claude/Cargo.toml` only adds the tokio `io-std` feature).

## Follow-ups

- M1: confirm the `setting_sources` default and decide the CLI version range; add a CI job that replays the golden files and a manual job that re-records on CLI upgrades.
- The partial level for external hooks is exposed through `ClaudeAdapter::interception()` / `last_enforcement()`, not through `Session`; if the daemon wants it per session as an event, that needs a contract change request.
- Add live fixtures for multi-select questions and plan mode when a use case appears.

## Fix round 1

Review: `docs/agents/reviews/M0-01-claude-adapter.md` (main checkout), 11 findings, fixed in the order 1, 2, 3, 4, 7, 5, 6, 9, 8, 10, 11.

| # | Change | Where / evidence |
|---|---|---|
| 1 | Native settings stay the default. New `settings.rs`: `detect_external_hooks` reads managed, user (`CLAUDE_CONFIG_DIR` aware), project and local settings that `--setting-sources` would load and lists every hook handler; any hook -> `Enforcement { interception: ProviderGated, reason }`. Exposed as `ClaudeAdapter::enforcement_for / last_enforcement / interception()`, recorded as an `atrium:` stderr input at `start`, printed by `chat`. Recorder now runs with `setting_sources = Some("")`. Report: matrix row and new section. | `settings.rs` tests (5), `external_hooks_make_the_enforcement_partial_and_say_why`. Plugin hooks and `--settings` in `extra_args` are not inspected (stated in the report). |
| 2 | `can_use_tool` without `tool_use_id`: gate decisions are matched by tool name and exact input (`gate_key`), so `Ask` now reaches the client. `AskUserQuestion` on `can_use_tool` is accepted only if the hook allowed it; otherwise the adapter asks the gate (`Reply::Question`) and the normalizer shows the question only after Allow/Ask (deny -> never shown). | normalizer tests `can_use_tool_without_an_id_is_matched_by_tool_and_input`, `a_question_is_shown_only_after_the_gate_lets_it_through`; adapter tests `an_ask_on_can_use_tool_reaches_the_client_with_or_without_an_id`, `a_question_nobody_let_through_is_not_shown`. |
| 3 | An approval for a call not yet started opens the call first (`ToolCallStarted`, then `ApprovalRequested`); the late `tool_use` doesn't start it twice. `TodoWrite` is now a normal tool call plus `PlanUpdated` (it can be approved like any tool); only `AskUserQuestion` stays virtual. | `an_approval_opens_the_call_it_refers_to`, `todo_write_is_a_tool_call_and_a_plan`. |
| 4 | `result` cancels pending approvals/questions inside the turn (order: messages, calls, requests, usage, `TurnCompleted`). `ensure_turn` also on `message_delta` usage and any `can_use_tool`. `Unmapped` records before `SessionStarted` are held until after it (or after a failed start's `Error`). The golden suite now asserts every turn-scoped event is inside a turn and nothing but an `Error` precedes `SessionStarted`. | `the_turn_end_cancels_requests_still_open`, `notes_about_unmapped_records_never_precede_session_started`, `every_scenario_obeys_the_lifecycle_rules`. |
| 7 | The child leads its own process group; every kill (grace expiry, interrupt deadline, handshake failure) is a group kill, and the group is also swept after the child is reaped. Drains were already bounded; a grandchild holding the pipes no longer matters. | `shutdown_kills_the_whole_process_group` (fake with a `sleep 600` grandchild that ignores EOF). |
| 5 | The message queue limit and its `Error` moved into the normalizer (`waiting` count, `take_rejected`); the adapter keeps only the texts. Replays refuse the same messages. | `the_message_queue_limit_is_decided_by_the_normalizer`, `messages_beyond_the_queue_limit_are_rejected`. |
| 6 | Turn usage is the delta of `result.modelUsage` totals (includes subagents) and of `total_cost_usd`; totals that go down mean a new process, so counting restarts (fixes the cost after a respawn). Running `Usage` counts subagent messages and `reasoning_tokens`. Goldens regenerated offline. | `turn_usage_is_the_model_usage_delta_and_survives_a_new_process`, `running_usage_counts_reasoning_and_subagent_messages`; the subagent fixture now shows 1174 output tokens instead of 610. |
| 9 | Recorder scrubs `memory_paths`, scratchpad/socket/transcript/output/styles paths, `/private/tmp/claude-<uid>` and the time zone name, and merges deltas split across a value the sanitizer rewrites (now also `input_json_delta`, with a second pass for the leftover `/Users/user`). `record --resanitize` re-applied it to all 20 fixtures offline. The golden suite checks for these patterns. | `fixtures_are_sanitized`; the one split path (`gate-deny-subagent`) was merged. |
| 8 | `gate-deny-delete` re-recorded (live) with a prompt that demands `rm victim.txt`; the recorder refuses to write a fixture where it was not attempted (`must_attempt`), and the golden test asserts the denied `Bash` call contains `rm victim.txt`. | `gate_deny_covers_execute_mcp_subagent_and_full_access`. |
| 10 | Fake CLI modes for malformed hook calls, a foreign `callback_id`, and `can_use_tool` (with id, without id, `AskUserQuestion`). Tests assert deny for malformed/foreign requests (gate not called) and deny for a denying, panicking and hanging gate on the `can_use_tool` path. The report no longer claims more than the tests show. | `malformed_and_foreign_hook_requests_are_denied`, `can_use_tool_follows_the_gate_and_fails_closed`. |
| 11 | The respawn handshake no longer blocks the actor: `launch` + a `handshaking` state with its own deadline in the main loop; messages wait in the queue and are written when `initialize` is answered; `Shutdown` is handled at once. | `shutdown_during_a_slow_respawn_handshake_is_prompt` (second process sleeps 20 s; shutdown returns within the grace). |

Account use in this round: 1 re-recorded fixture (`gate-deny-delete`) plus 2 live check runs (`approval-allow`, `question`, written to a temp dir, not committed) to confirm the new adapter against the real CLI. All other fixture changes were offline.
