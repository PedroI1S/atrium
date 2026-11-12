# mcp-allow (live)

- Recording: `live`, `cargo run -p atrium-agents --example claude-record -- mcp-allow`, on the M1 contract (revision 3) shapes.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources` empty (no user, project or local settings), no managed settings read, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Call the echo tool of the MCP server named echo with the text ping, and tell me what it returned."
- Gate: allow everything.
- Covers: A per-session MCP server (generated --mcp-config) is called; the gate allows it. Control for gate-deny-mcp.
- Spawn to `SessionStarted`: 1555 ms.
- Events: 20 (0 unmapped). Counts: {"context_updated": 1, "message_completed": 2, "message_delta": 4, "quota_updated": 1, "session_ended": 1, "session_started": 1, "tool_call_finished": 2, "tool_call_started": 2, "turn_completed": 1, "turn_started": 1, "usage": 4}
- Deltas merged because a sanitized value spanned several of them: 0 block(s).
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents, and machine paths (`memory_paths`, scratchpad, socket, transcript and output files). Nothing the normalizer reads.
