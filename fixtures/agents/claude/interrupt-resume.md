# interrupt-resume (live)

- Recording: `live`, `cargo run -p atrium-agents --example claude-record -- interrupt-resume`, on the M1 contract (revision 3) shapes.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources` empty (no user, project or local settings), no managed settings read, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Run this exact shell command in the foreground and wait for it: python3 -c 'import time; time.sleep(60)'", "Just say OK."
- Gate: allow everything.
- Covers: Interrupt, then a second message: the same Claude Code process takes the next turn.
- Spawn to `SessionStarted`: 1857 ms.
- Events: 19 (0 unmapped). Counts: {"context_updated": 2, "message_completed": 3, "message_delta": 1, "quota_updated": 1, "session_ended": 1, "session_started": 1, "tool_call_finished": 1, "tool_call_started": 1, "turn_completed": 2, "turn_started": 2, "usage": 4}
- Deltas merged because a sanitized value spanned several of them: 0 block(s).
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents, and machine paths (`memory_paths`, scratchpad, socket, transcript and output files). Nothing the normalizer reads.
