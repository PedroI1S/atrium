# Fixture: `gate-deny`

- **Source**: `hand-made` (constructed against `agent-client-protocol = 3.5.0` / ACP v1 schema; worker sandbox policy blocks executing `opencode`)
- **Provider**: ACP (`opencode acp`, v2.1.3)
- **Command**: `opencode acp`
- **Model**: `anthropic/claude-haiku-4-5`
- **Client capabilities**: `fs.readTextFile = true`, `fs.writeTextFile = true`, `terminal = true`, `elicitation.form = {}`
- **Prompt**: `"Read secret.txt and tell me its contents."`
- **Covers**: Pre-execution gate denial for `Read` (`fs/read_text_file`). The gate returns `GateDecision::Deny`, the adapter returns a JSON-RPC error without reading `secret.txt`, the tool call finishes with `ToolCallFinished { status: Denied }`, and the contents of `secret.txt` never appear in any event.
