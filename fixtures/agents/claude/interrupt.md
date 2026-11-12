# interrupt (live)

- Recording: `live`, `cargo run -p atrium-agents --example claude-record -- interrupt`, on the M1 contract (revision 3) shapes.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources` empty (no user, project or local settings), no managed settings read, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Run this exact shell command in the foreground and wait for it: python3 -c 'import time; time.sleep(60)'"
- Gate: allow everything.
- Covers: Interrupt while a command runs: the turn ends interrupted (Claude Code answers with a result of subtype error_during_execution) and the open call ends cancelled.
- Spawn to `SessionStarted`: 1567 ms.
- Events: 11 (0 unmapped). Counts: {"context_updated": 1, "message_completed": 1, "quota_updated": 1, "session_ended": 1, "session_started": 1, "tool_call_finished": 1, "tool_call_started": 1, "turn_completed": 1, "turn_started": 1, "usage": 2}
- Deltas merged because a sanitized value spanned several of them: 0 block(s).
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents, and machine paths (`memory_paths`, scratchpad, socket, transcript and output files). Nothing the normalizer reads.
