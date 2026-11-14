# stale-approval (live)

- Recording: `live`, `cargo run -p atrium-agents --example claude-record -- stale-approval`, on the M1 contract (revision 3) shapes.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources` empty (no user, project or local settings), no managed settings read, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Run this exact shell command and report the result: touch pwned.txt"
- Gate: Ask for every Execute-kind call, allow the rest.
- Covers: An Allow that arrives after the turn ended is never executed: the gate asks, the turn is interrupted while the approval is open (it is cancelled), the late Allow is refused with a non-fatal Error, and the command never runs.
- Spawn to `SessionStarted`: 1545 ms.
- Events: 14 (0 unmapped). Counts: {"approval_requested": 1, "context_updated": 1, "error": 1, "message_completed": 1, "quota_updated": 1, "request_resolved": 1, "session_ended": 1, "session_started": 1, "tool_call_finished": 1, "tool_call_started": 1, "turn_completed": 1, "turn_started": 1, "usage": 2}
- Deltas merged because a sanitized value spanned several of them: 0 block(s).
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents, and machine paths (`memory_paths`, scratchpad, socket, transcript and output files). Nothing the normalizer reads.
