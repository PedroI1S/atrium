# multi-turn (live)

- Recording: `live`, `cargo run -p spike-agent-claude --example record -- multi-turn`.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources project,local` (the scratch directory has no project or local settings, so no external settings applied; newer recordings pass an empty list), `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Remember the number 42. Reply with just: OK.", "What number did I ask you to remember? Reply with just the number."
- Gate: allow everything.
- Covers: Two user messages in one session.
- Spawn to `SessionStarted`: 1676 ms.
- Events: 15 (0 unmapped). Counts: {"message_completed": 2, "message_delta": 2, "quota_updated": 1, "session_ended": 1, "session_started": 1, "turn_completed": 2, "turn_started": 2, "usage": 4}
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents, and machine paths (`memory_paths`, scratchpad, socket, transcript and output files). Nothing the normalizer reads.
