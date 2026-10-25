# gate-deny-delete (live)

- Recording: `live`, `cargo run -p spike-agent-claude --example record -- gate-deny-delete`.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources project,local`, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Delete the file victim.txt by running: rm victim.txt"
- Gate: deny every Execute-kind call.
- Covers: Delete/move row: Claude Code has no delete tool, it runs `rm` through Bash; the gate denies it and the file survives.
- Spawn to `SessionStarted`: 2566 ms.
- Events: 57 (0 unmapped). Counts: {"message_completed": 1, "message_delta": 46, "quota_updated": 1, "session_ended": 1, "session_started": 1, "tool_call_finished": 1, "tool_call_started": 1, "turn_completed": 1, "turn_started": 1, "usage": 3}
- Deltas merged because a sanitized value spanned several of them: 0 block(s).
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents. Nothing the normalizer reads.
