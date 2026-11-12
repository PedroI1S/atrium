# multi-turn (live)

- Recording: `live`, `cargo run -p atrium-agents --example claude-record -- multi-turn`, on the M1 contract (revision 3) shapes.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources` empty (no user, project or local settings), no managed settings read, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Remember the number 42. Reply with just: OK.", "What number did I ask you to remember? Reply with just the number."
- Gate: allow everything.
- Covers: Two user messages in one session.
- Spawn to `SessionStarted`: 1678 ms.
- Events: 19 (0 unmapped). Counts: {"context_updated": 2, "message_completed": 4, "message_delta": 2, "quota_updated": 1, "session_ended": 1, "session_started": 1, "turn_completed": 2, "turn_started": 2, "usage": 4}
- Deltas merged because a sanitized value spanned several of them: 0 block(s).
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents, and machine paths (`memory_paths`, scratchpad, socket, transcript and output files). Nothing the normalizer reads.
