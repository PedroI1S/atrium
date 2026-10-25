# Fixture: `tool-read`

- **Source**: `hand-made` (constructed against `agent-client-protocol = 3.5.0` / ACP v1 schema; worker sandbox policy blocks executing `opencode`)
- **Provider**: ACP (`opencode acp`, v2.1.3)
- **Command**: `opencode acp`
- **Model**: `anthropic/claude-haiku-4-5`
- **Client capabilities**: `fs.readTextFile = true`, `fs.writeTextFile = true`, `terminal = true`, `elicitation.form = {}`
- **Prompt**: `"Read hello.txt and tell me what it says."`
- **Covers**: Tool call lifecycle (`tool_call` → `fs/read_text_file` client request → `GateDecision::Allow` → `tool_call_update` completed) mapped to `ToolCallStarted` → `ToolCallFinished { status: Succeeded }`.
