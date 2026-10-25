# Fixture: `gate-deny-fs`

- **Source**: `hand-made` (constructed against `agent-client-protocol = 3.5.0` / ACP v1 schema; worker sandbox policy blocks executing `opencode`)
- **Provider**: ACP (`opencode acp`, v2.1.3)
- **Command**: `opencode acp`
- **Model**: `anthropic/claude-haiku-4-5`
- **Client capabilities**: `fs.readTextFile = true`, `fs.writeTextFile = true`, `terminal = true`, `elicitation.form = {}`
- **Prompt**: `"Read secret.txt and tell me its contents."`
- **Covers**: Pre-execution gate denial for `Read` (`fs/read_text_file`). The gate returns `GateDecision::Deny`, the adapter returns a JSON-RPC error without reading `secret.txt`, the tool call finishes with `ToolCallFinished { status: Denied }`, and the contents of `secret.txt` never appear in any event.

- **Note (fix round 1)**: this is the *client-capability* path (`fs/*` or `terminal/*` requests). Live OpenCode v2.1.3 never sends them (M0-04 live run 1), so this fixture models a generic ACP agent that does. The OpenCode-shaped scenario of the same name uses `session/request_permission`.
