# Handoff: M1-07-claude-adapter

Model: Sonnet 5.5 · Status: **partial** (stopped at shift end)

## Done
- `crates/atrium-agents/src/claude/{mod,adapter,normalizer,settings,tools}.rs`: `ClaudeAdapter` (probe + start), `ClaudeNormalizer`, config inspection, tool subjects. Contract rev 3: descriptor via `evaluate`, `AdapterInput::Enforcement`/`Redeem`/`QuestionTicket`, tickets, redeem before `can_use_tool` answer, `spawn_group` + `on_spawn` + `Session::process`, `terminate` on shutdown, bounded terminal tail, `MessageBody` + user messages, `ContextUpdated`, `Error::details`, limits (record, args, output, unmapped), interrupt hang = session ends (no respawn), unparseable/unreadable settings = `inspected:false`.
- Tests: `tests/claude_adapter.rs` (25, offline fake CLI `examples/claude-fake.rs`, incl. saturation with process-group check, stale approval, redeem, oversize record, descriptor/plugin tests), `tests/claude_golden.rs` (20), settings unit tests.
- Live fixtures, all re-recorded with `claude` 2.1.308, haiku, sanitized (`is_clean` asserted by recorder and golden test): simple, tool-read, gate-deny, gate-deny-{exec,edit,delete,fetch,fullaccess,mcp,subagent}, mcp-allow, approval-allow, approval-deny, question, interrupt, interrupt-resume, multi-turn, resume, startup-error, plus new plugin-hook and stale-approval. `rate-limited` is the M0 live capture migrated (Enforcement line added), not re-recorded.
- Recorder `examples/claude-record.rs` (`cargo run -p atrium-agents --example claude-record -- <name>|all`).

## DoD status
- fmt: pass. clippy workspace `-D warnings`: pass. `cargo test -p atrium-agents`: pass (consistency's fixture-wide test is `#[ignore]`d for Codex by M1-12; Claude fixtures show no violations with `--include-ignored`).
- Live re-recording, new fixtures, saturation test: done.
- **Not done:** `tests/claude_normalizer.rs` (port of spike hand-made normalizer tests, `spikes/agent-claude/tests/normalizer.rs`, to rev-3 shapes: tickets, QuestionTicket, MessageBody, details, interrupt now ends session; plus tests for oversize args/output cap, Deadline inputs, pre-session details with no Unmapped). **Handoff sections still missing:** enforcement matrix write-up and the descriptor printed for this machine (print via `probe`, sanitized). No change requests filed.

## Enforcement matrix (all Intercepted via PreToolUse hook, 2.1.308)
Read: gate-deny. Edit: gate-deny-edit. Delete/Move: as Bash, gate-deny-delete. Search: gate-deny. Execute: gate-deny-exec, approval-*, gate-deny-fullaccess. Fetch: gate-deny-fetch. MCP: gate-deny-mcp. Subagent: gate-deny-subagent. ProviderSpecific: mcp-allow (ToolSearch only; weakest evidence). Plugins/hooks in settings: Partial (plugin-hook fixture).

## Gotchas
- Do not set `CLAUDE_CONFIG_DIR` when recording: auth lives there ("Not logged in").
- Account was at ~76% of the 5h window during recording.
- AskUserQuestion hook is answered `allow` without `gate.check`; the gate is consulted through `question_ticket` only.
- Exit detection = pipe EOF or a 500 ms `identify` poll (never reap before `terminate`).
- AskUserQuestion/ProviderSpecific evidence is thin; spike gate files in `spikes/` untouched.

## Next step
Write `tests/claude_normalizer.rs`, add the matrix + printed descriptor to this handoff, rerun the DoD commands, then flip status to done.
