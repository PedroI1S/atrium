# gate-deny-edit (live)

- Recording: `live`, `cargo run -p spike-agent-claude --example record -- gate-deny-edit`.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources project,local`, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Use the Write tool to create a file named made.txt that contains the word hello."
- Gate: deny every Edit-kind call.
- Covers: Edit/write row: the gate denies the Write tool; the file is not created.
- Spawn to `SessionStarted`: 2271 ms.
- Events: 57 (0 unmapped). Counts: {"message_completed": 1, "message_delta": 46, "quota_updated": 1, "session_ended": 1, "session_started": 1, "tool_call_finished": 1, "tool_call_started": 1, "turn_completed": 1, "turn_started": 1, "usage": 3}
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents. Nothing the normalizer reads.
