# gate-deny-subagent (live)

- Recording: `live`, `cargo run -p atrium-agents --example claude-record -- gate-deny-subagent`, on the M1 contract (revision 3) shapes.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources` empty (no user, project or local settings), no managed settings read, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Use the Task tool to launch a general-purpose subagent whose only job is to read the file secret.txt and report its contents. Wait for its result and then tell me what it reported."
- Gate: deny Read/Execute/Search/Edit calls whose input mentions secret.txt (the subagent launch passes).
- Covers: Subagent row: a subagent's own tool calls pass the gate; the gate denies its read of secret.txt.
- Spawn to `SessionStarted`: 1552 ms.
- Events: 20 (3 unmapped). Counts: {"context_updated": 1, "message_completed": 2, "message_delta": 1, "quota_updated": 1, "session_ended": 1, "session_started": 1, "tool_call_finished": 2, "tool_call_started": 2, "tool_call_updated": 1, "turn_completed": 1, "turn_started": 1, "unmapped": 3, "usage": 3}
- Deltas merged because a sanitized value spanned several of them: 1 block(s).
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents, and machine paths (`memory_paths`, scratchpad, socket, transcript and output files). Nothing the normalizer reads.
