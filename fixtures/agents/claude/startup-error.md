# startup-error (live)

- Recording: `live`, `cargo run -p atrium-agents --example claude-record -- startup-error`, on the M1 contract (revision 3) shapes.
- CLI: 2.1.308 (Claude Code), model `nonexistent-model-xyz`, permission policy `Ask`, `--setting-sources` empty (no user, project or local settings), no managed settings read, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "hi"
- Gate: allow everything.
- Covers: An invalid model id: the first turn fails, pre-session lifecycle (Error fatal then SessionEnded, no SessionStarted).
- Spawn to `SessionStarted`: never (no SessionStarted).
- Events: 2 (0 unmapped). Counts: {"error": 1, "session_ended": 1}
- Deltas merged because a sanitized value spanned several of them: 0 block(s).
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents, and machine paths (`memory_paths`, scratchpad, socket, transcript and output files). Nothing the normalizer reads.
