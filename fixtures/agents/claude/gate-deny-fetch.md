# gate-deny-fetch (live)

- Recording: `live`, `cargo run -p spike-agent-claude --example record -- gate-deny-fetch`.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, `--setting-sources project,local`, `--permission-prompt-tool stdio`, one `PreToolUse` hook callback registered through `initialize`.
- Prompt(s): "Use the WebFetch tool to fetch https://example.com and summarize the page in one sentence."
- Gate: deny every Fetch-kind call.
- Covers: Fetch/web row: the gate denies WebFetch before any request is made.
- Spawn to `SessionStarted`: 1537 ms.
- Events: 45 (0 unmapped). Counts: {"message_completed": 1, "message_delta": 33, "quota_updated": 2, "session_ended": 1, "session_started": 1, "tool_call_finished": 1, "tool_call_started": 1, "turn_completed": 1, "turn_started": 1, "usage": 3}
- Trimmed before sanitizing: the `initialize` response's `commands`, `agents`, `models` and `account` (personal and bulky) and the `system/init` lists of slash commands, skills, plugins and agents. Nothing the normalizer reads.
