# gate-deny (live)

- Recording: `live`, `cargo run -p spike-agent-claude --example record -- gate-deny`.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources project,local`, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Read the file secret.txt and tell me exactly what it contains."
- Gate: deny any call whose input mentions secret.txt.
- Covers: Read row of the enforcement matrix: the gate denies reading secret.txt (Read and any Bash attempt); its contents never reach an event.
- Spawn to `SessionStarted`: 1452 ms.
- Events: 40 (0 unmapped). Counts: {"message_completed": 1, "message_delta": 29, "quota_updated": 1, "session_ended": 1, "session_started": 1, "tool_call_finished": 1, "tool_call_started": 1, "turn_completed": 1, "turn_started": 1, "usage": 3}
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents. Nothing the normalizer reads.
