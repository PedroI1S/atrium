# simple (live)

- Recording: `live`, `cargo run -p atrium-agents --example claude-record -- simple`, on the M1 contract (revision 3) shapes.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources` empty (no user, project or local settings), no managed settings read, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Reply with exactly the word: pong. Do not use any tools."
- Gate: allow everything.
- Covers: A one-line answer, no tools.
- Spawn to `SessionStarted`: 2359 ms.
- Events: 11 (0 unmapped). Counts: {"context_updated": 1, "message_completed": 2, "message_delta": 1, "quota_updated": 1, "session_ended": 1, "session_started": 1, "turn_completed": 1, "turn_started": 1, "usage": 2}
- Deltas merged because a sanitized value spanned several of them: 0 block(s).
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents, and machine paths (`memory_paths`, scratchpad, socket, transcript and output files). Nothing the normalizer reads.
