# tool-read (live)

- Recording: `live`, `cargo run -p spike-agent-claude --example record -- tool-read`.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources project,local`, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Read the file notes.txt and tell me the codeword in one short sentence."
- Gate: allow everything.
- Covers: The agent reads a file; the gate allows it.
- Spawn to `SessionStarted`: 1978 ms.
- Events: 13 (0 unmapped). Counts: {"message_completed": 1, "message_delta": 2, "quota_updated": 1, "session_ended": 1, "session_started": 1, "tool_call_finished": 1, "tool_call_started": 1, "turn_completed": 1, "turn_started": 1, "usage": 3}
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents. Nothing the normalizer reads.
