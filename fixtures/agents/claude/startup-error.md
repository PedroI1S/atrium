# startup-error (live)

- Recording: `live`, `cargo run -p spike-agent-claude --example record -- startup-error`.
- CLI: 2.1.308 (Claude Code), model `nonexistent-model-xyz`, permission policy `Ask`, `--setting-sources project,local`, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "hi"
- Gate: allow everything.
- Covers: An invalid model id: the first turn fails, pre-session lifecycle (Error fatal then SessionEnded, no SessionStarted).
- Spawn to `SessionStarted`: never (no SessionStarted).
- Events: 2 (0 unmapped). Counts: {"error": 1, "session_ended": 1}
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents. Nothing the normalizer reads.
