# Handoff: M0-01-claude-adapter

Model: Sonnet 5.5 · Status: done

Spike report: [docs/spikes/M0-01-claude-adapter.md](../../spikes/M0-01-claude-adapter.md) (enforcement matrix, timings, protocol stability, fallback verdict).

## Done

- `ClaudeNormalizer` and `ClaudeAdapter` (`impl Normalizer` / `impl AgentAdapter`, `interception() = AllToolCalls`) in `spikes/agent-claude`.
- Gate enforcement through an SDK-style `PreToolUse` hook callback registered in `initialize` (no user settings touched). `Allow` / `Deny` / `Ask` map to hook decisions; `Ask` continues as `can_use_tool` -> `ApprovalRequested` -> `RequestResolved`. Every error path answers `deny`.
- Mapping: partial messages -> `MessageDelta`, tool_use/tool_result -> `ToolCall*`, `AskUserQuestion` -> `QuestionAsked` (structured options, answers back through `updatedInput.answers`), `TodoWrite` -> `PlanUpdated` (hand-made test only), `result` -> `Usage` + `TurnCompleted`, rate-limit records -> `QuotaUpdated` / `RateLimited`, interrupt -> control-protocol interrupt, per-session MCP via generated `--mcp-config` + `--strict-mcp-config`, `FullAccess` -> `bypassPermissions`.
- 20 live fixtures (haiku, CLI 2.1.308), each with `.jsonl`, `.events.json` (golden) and `.md`: all nine required scenarios plus `gate-deny-exec`, `-edit`, `-delete`, `-fetch`, `-fullaccess`, `-mcp`, `-subagent`, `mcp-allow`, `interrupt-resume`, `rate-limited` (a real refusal captured when the account hit its 5 h limit).
- Saturation test, fail-closed gate tests, stuck-interrupt kill + resume, queue overflow, argument and temp-file tests against an offline `fake_claude` binary.
- `--example record` (re-record, `--list`, `all`) and `--example chat`.

## Not done

- `TodoWrite` and `Reasoning` text cannot be recorded live: 2.1.308 headless has no `TodoWrite` tool and thinking blocks arrive empty. Covered by hand-made tests only.
- No live deny fixtures for `WebSearch`, `Skill`, `ToolSearch`, Cron/Monitor tools (the hook is generic; manual probes only; see report).
- `ClaudeOptions::setting_sources` defaults to inherit user settings; the daemon must choose. Fixtures were recorded with `project,local`.
- The merge of sanitizer-spanning streamed deltas (`merge_split_deltas` in `record.rs`) never triggered in the final recordings, so that code path is unexercised.

## Definition of Done results

- Fixtures for every scenario in `docs/contracts/m0.md` §3.3 (1 to 9), recorded live, sanitized (recorder uses `Sanitizer`; `fixtures_are_sanitized` re-checks), each with a golden test: yes.
- Saturation test: `saturation_unread_events_still_obey_interrupt_and_shutdown` passes (~1.5 s fill + interrupt + shutdown, child reaped, 0 alive tasks).
- `cargo run -p spike-agent-claude --example record -- <scenario>`: works (all fixtures made with it). `--example chat`: ran a live session end to end.
- `cargo fmt --check`: clean.
- `cargo clippy -p spike-agent-claude --all-targets -- -D warnings`: clean.
- `cargo test -p spike-agent-claude`: 1 lib + 8 adapter + 14 golden + 20 normalizer tests, all passing.
- Spike report written; this handoff written.

## Gotchas

- **Claude Code fails open** on a hook answer that is an error response or has no valid decision (only a hook timeout fails closed). Never send those. Tested in `gate_failures_deny`; protocol probes are described in the report.
- `--permission-prompt-tool stdio` (hidden flag) is required for `AskUserQuestion` and `can_use_tool` in `-p` mode.
- `system/init` arrives before the model is called, so `SessionStarted` is held until the first real API response; a bad model is then a pre-session failure as the contract wants. Median spawn -> `SessionStarted` is about 1.9 s (handshake about 0.5 s).
- `strict-mcp-config` does not drop claude.ai connectors; the adapter sets `ENABLE_CLAUDEAI_MCP_SERVERS=false`.
- After an interrupt Claude Code stays alive and sends a `result` (`error_during_execution`). (An early probe of mine closed stdin on `result` and I briefly misread that as the process exiting; the fallback kill-and-`--resume` path remains for a child that ignores the interrupt.)
- Recording consumed a noticeable share of the account's 5 h window and the account hit its limit mid-session (the coordinator saw the rate-limit stop). Re-record in small batches.
- Events created by the adapter itself (queue-overflow `Error`) are not reproduced by replays; everything else is.
- `Cargo.lock` is not part of this change; no contract file or root `Cargo.toml` was touched (`spikes/agent-claude/Cargo.toml` only adds the tokio `io-std` feature).

## Follow-ups

- M1: decide `setting_sources` default and the CLI version range; add a CI job that replays the golden files and a manual job that re-records on CLI upgrades.
- Consider a contract change request for a way for adapters to report adapter-originated events through `AdapterInput` (queue overflow), only if the daemon needs replay fidelity for it.
- Add live fixtures for multi-select questions and plan mode when a use case appears.
