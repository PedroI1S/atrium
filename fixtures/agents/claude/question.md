# question (live)

- Recording: `live`, `cargo run -p spike-agent-claude --example record -- question`.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources project,local`, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Use the AskUserQuestion tool to ask me which color I prefer: red, green or blue. Then tell me my answer in one sentence."
- Gate: allow everything.
- Covers: The agent asks a multiple-choice question (AskUserQuestion) and gets a structured answer.
- Spawn to `SessionStarted`: 1837 ms.
- Events: 12 (0 unmapped). Counts: {"message_completed": 1, "message_delta": 1, "question_asked": 1, "quota_updated": 1, "request_resolved": 1, "session_ended": 1, "session_started": 1, "turn_completed": 1, "turn_started": 1, "usage": 3}
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents. Nothing the normalizer reads.
