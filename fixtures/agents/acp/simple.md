# Fixture: `simple`

- **Source**: `hand-made` (constructed against `agent-client-protocol = 3.5.0` / ACP v1 schema; worker sandbox policy blocks executing `opencode`)
- **Provider**: ACP (`opencode acp`, v2.1.3)
- **Command**: `opencode acp`
- **Model**: `anthropic/claude-haiku-4-5`
- **Client capabilities**: `fs.readTextFile = true`, `fs.writeTextFile = true`, `terminal = true`, `elicitation.form = {}`
- **Prompt**: `"What is 2 + 2? Reply with just the number."`
- **Covers**: Handshake (`initialize` → `session/new`), single-turn prompt (`session/prompt`), streaming assistant text (`session/update` with `agent_message_chunk`), turn completion with token usage (`stopReason: "end_turn"`), and clean shutdown (`SessionEnded`).
