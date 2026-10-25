# mcp-allow (live)

- Recording: `live`, `cargo run -p spike-agent-claude --example record -- mcp-allow`.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources project,local`, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Call the echo tool of the MCP server named echo with the text ping, and tell me what it returned."
- Gate: allow everything.
- Covers: A per-session MCP server (generated --mcp-config) is called; the gate allows it. Control for gate-deny-mcp.
- Spawn to `SessionStarted`: 1983 ms.
- Events: 17 (0 unmapped). Counts: {"message_completed": 1, "message_delta": 3, "quota_updated": 1, "session_ended": 1, "session_started": 1, "tool_call_finished": 2, "tool_call_started": 2, "turn_completed": 1, "turn_started": 1, "usage": 4}
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents. Nothing the normalizer reads.
