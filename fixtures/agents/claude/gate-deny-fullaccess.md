# gate-deny-fullaccess (live)

- Recording: `live`, `cargo run -p spike-agent-claude --example record -- gate-deny-fullaccess`.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `FullAccess`, `--setting-sources project,local` (the scratch directory has no project or local settings, so no external settings applied; newer recordings pass an empty list), `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Run this exact shell command and report the result: touch pwned.txt"
- Gate: deny every Execute-kind call.
- Covers: FullAccess (`--permission-mode bypassPermissions`) still passes the gate: the gate denies the command, which does not run.
- Spawn to `SessionStarted`: 2650 ms.
- Events: 45 (0 unmapped). Counts: {"message_completed": 1, "message_delta": 34, "quota_updated": 1, "session_ended": 1, "session_started": 1, "tool_call_finished": 1, "tool_call_started": 1, "turn_completed": 1, "turn_started": 1, "usage": 3}
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents, and machine paths (`memory_paths`, scratchpad, socket, transcript and output files). Nothing the normalizer reads.
