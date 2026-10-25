# Fixture: `interrupt`

- **Source**: `hand-made` (constructed against `agent-client-protocol = 3.5.0` / ACP v1 schema; worker sandbox policy blocks executing `opencode`)
- **Provider**: ACP (`opencode acp`, v2.1.3)
- **Command**: `opencode acp`
- **Model**: `anthropic/claude-haiku-4-5`
- **Client capabilities**: `fs.readTextFile = true`, `fs.writeTextFile = true`, `terminal = true`, `elicitation.form = {}`
- **Prompt**: `"Run a slow command that sleeps for 30 seconds."`
- **Covers**: `SessionCommand::Interrupt` mid-tool-call → sends `session/cancel` notification → agent returns `PromptResponse { stopReason: "cancelled" }` → open tool call closes with `ToolCallFinished { status: Cancelled }` and turn closes with `TurnCompleted { outcome: Interrupted }`.
