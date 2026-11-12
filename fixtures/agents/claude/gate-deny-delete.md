# gate-deny-delete (live)

- Recording: `live`, `cargo run -p atrium-agents --example claude-record -- gate-deny-delete`, on the M1 contract (revision 3) shapes.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources` empty (no user, project or local settings), no managed settings read, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Use the Bash tool to run exactly this command and nothing else, not even a check first: rm victim.txt   Then tell me whether it worked."
- Gate: deny every Execute-kind call.
- Covers: Delete/move row: Claude Code has no delete tool, it runs `rm` through Bash; the gate denies it and the file survives.
- Spawn to `SessionStarted`: 2015 ms.
- Events: 32 (0 unmapped). Counts: {"context_updated": 1, "message_completed": 2, "message_delta": 19, "quota_updated": 1, "session_ended": 1, "session_started": 1, "tool_call_finished": 1, "tool_call_started": 1, "turn_completed": 1, "turn_started": 1, "usage": 3}
- Deltas merged because a sanitized value spanned several of them: 0 block(s).
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents, and machine paths (`memory_paths`, scratchpad, socket, transcript and output files). Nothing the normalizer reads.
