# approval-deny (live)

- Recording: `live`, `cargo run -p spike-agent-claude --example record -- approval-deny`.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources project,local`, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Run this exact shell command and report the result: touch pwned.txt"
- Gate: Ask for every Execute-kind call, allow the rest.
- Covers: The gate answers Ask for a command; the client denies it; the command does not run.
- Spawn to `SessionStarted`: 1758 ms.
- Events: 47 (0 unmapped). Counts: {"approval_requested": 1, "message_completed": 1, "message_delta": 34, "quota_updated": 1, "request_resolved": 1, "session_ended": 1, "session_started": 1, "tool_call_finished": 1, "tool_call_started": 1, "turn_completed": 1, "turn_started": 1, "usage": 3}
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents. Nothing the normalizer reads.
