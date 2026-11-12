# plugin-hook (live)

- Recording: `live`, `cargo run -p atrium-agents --example claude-record -- plugin-hook`, on the M1 contract (revision 3) shapes.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources` empty (no user, project or local settings), no managed settings read, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Run this exact shell command and report its output: echo plugin-test"
- Gate: allow everything.
- Covers: A plugin (`--plugin-dir`) with a hook: the session is Partial (the plugin is uninspected and its hooks run outside the gate), and the hook really ran (marker file). The gate still allowed the command.
- Spawn to `SessionStarted`: 1581 ms.
- Events: 17 (0 unmapped). Counts: {"context_updated": 1, "message_completed": 2, "message_delta": 4, "quota_updated": 1, "session_ended": 1, "session_started": 1, "tool_call_finished": 1, "tool_call_started": 1, "turn_completed": 1, "turn_started": 1, "usage": 3}
- Deltas merged because a sanitized value spanned several of them: 0 block(s).
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents, and machine paths (`memory_paths`, scratchpad, socket, transcript and output files). Nothing the normalizer reads.
