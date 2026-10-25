# approval-allow (live)

- Recording: `live`, `cargo run -p spike-agent-claude --example record -- approval-allow`.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources project,local`, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Run this exact shell command and report its output: echo approved"
- Gate: Ask for every Execute-kind call, allow the rest.
- Covers: The gate answers Ask for a command; the client approves; the command runs.
- Spawn to `SessionStarted`: 1438 ms.
- Events: 16 (0 unmapped). Counts: {"approval_requested": 1, "message_completed": 1, "message_delta": 3, "quota_updated": 1, "request_resolved": 1, "session_ended": 1, "session_started": 1, "tool_call_finished": 1, "tool_call_started": 1, "turn_completed": 1, "turn_started": 1, "usage": 3}
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents. Nothing the normalizer reads.
