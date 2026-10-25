# gate-deny-subagent (live)

- Recording: `live`, `cargo run -p spike-agent-claude --example record -- gate-deny-subagent`.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources project,local` (the scratch directory has no project or local settings, so no external settings applied; newer recordings pass an empty list), `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Use the Task tool to launch a general-purpose subagent whose only job is to read the file secret.txt and report its contents. Wait for its result and then tell me what it reported."
- Gate: deny Read/Execute/Search/Edit calls whose input mentions secret.txt (the subagent launch passes).
- Covers: Subagent row: a subagent's own tool calls pass the gate; the gate denies its read of secret.txt.
- Spawn to `SessionStarted`: 2132 ms.
- Events: 62 (5 unmapped). Counts: {"message_completed": 1, "message_delta": 44, "quota_updated": 1, "session_ended": 1, "session_started": 1, "tool_call_finished": 2, "tool_call_started": 2, "turn_completed": 1, "turn_started": 1, "unmapped": 5, "usage": 3}
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents, and machine paths (`memory_paths`, scratchpad, socket, transcript and output files). Nothing the normalizer reads.
