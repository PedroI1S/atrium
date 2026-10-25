# gate-deny-mcp (live)

- Recording: `live`, `cargo run -p spike-agent-claude --example record -- gate-deny-mcp`.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources project,local`, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Call the echo tool of the MCP server named echo with the text ping, and tell me what it returned."
- Gate: deny every Mcp-kind call.
- Covers: MCP row: the gate denies the MCP tool call; the MCP server never receives it.
- Spawn to `SessionStarted`: 1619 ms.
- Events: 51 (0 unmapped). Counts: {"message_completed": 1, "message_delta": 37, "quota_updated": 1, "session_ended": 1, "session_started": 1, "tool_call_finished": 2, "tool_call_started": 2, "turn_completed": 1, "turn_started": 1, "usage": 4}
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents. Nothing the normalizer reads.
