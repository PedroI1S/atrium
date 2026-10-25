# resume (live)

- Recording: `live`, `cargo run -p spike-agent-claude --example record -- resume`.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources project,local`, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "What codeword did I ask you to remember? Reply with just the codeword."
- Gate: allow everything.
- Covers: A second process resumes the first session by native id (the first session is not recorded).
- Spawn to `SessionStarted`: 1682 ms.
- Events: 9 (0 unmapped). Counts: {"message_completed": 1, "message_delta": 1, "quota_updated": 1, "session_ended": 1, "session_started": 1, "turn_completed": 1, "turn_started": 1, "usage": 2}
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents. Nothing the normalizer reads.
