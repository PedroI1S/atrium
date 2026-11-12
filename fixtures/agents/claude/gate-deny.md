# gate-deny (live)

- Recording: `live`, `cargo run -p atrium-agents --example claude-record -- gate-deny`, on the M1 contract (revision 3) shapes.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources` empty (no user, project or local settings), no managed settings read, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Read the file secret.txt and tell me exactly what it contains."
- Gate: deny any call whose input mentions secret.txt.
- Covers: Read row of the enforcement matrix: the gate denies reading secret.txt (Read and any Bash attempt); its contents never reach an event.
- Spawn to `SessionStarted`: 1454 ms.
- Events: 46 (0 unmapped). Counts: {"context_updated": 1, "message_completed": 2, "message_delta": 33, "quota_updated": 1, "session_ended": 1, "session_started": 1, "tool_call_finished": 1, "tool_call_started": 1, "turn_completed": 1, "turn_started": 1, "usage": 3}
- Deltas merged because a sanitized value spanned several of them: 1 block(s).
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents, and machine paths (`memory_paths`, scratchpad, socket, transcript and output files). Nothing the normalizer reads.
