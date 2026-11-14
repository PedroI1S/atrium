# question (live)

- Recording: `live`, `cargo run -p atrium-agents --example claude-record -- question`, on the M1 contract (revision 3) shapes.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources` empty (no user, project or local settings), no managed settings read, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Use the AskUserQuestion tool to ask me which color I prefer: red, green or blue. Then tell me my answer in one sentence."
- Gate: allow everything.
- Covers: The agent asks a multiple-choice question (AskUserQuestion) and gets a structured answer.
- Spawn to `SessionStarted`: 1738 ms.
- Events: 14 (0 unmapped). Counts: {"context_updated": 1, "message_completed": 2, "message_delta": 1, "question_asked": 1, "quota_updated": 1, "request_resolved": 1, "session_ended": 1, "session_started": 1, "turn_completed": 1, "turn_started": 1, "usage": 3}
- Deltas merged because a sanitized value spanned several of them: 0 block(s).
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents, and machine paths (`memory_paths`, scratchpad, socket, transcript and output files). Nothing the normalizer reads.
