# Fixture: `multi-turn`

- **Source**: `hand-made` (constructed against `agent-client-protocol = 3.5.0` / ACP v1 schema; worker sandbox policy blocks executing `opencode`)
- **Provider**: ACP (`opencode acp`, v2.1.3)
- **Command**: `opencode acp`
- **Model**: `anthropic/claude-haiku-4-5`
- **Client capabilities**: `fs.readTextFile = true`, `fs.writeTextFile = true`, `terminal = true`, `elicitation.form = {}`
- **Prompts**:
  1. `"Remember the codeword: cobalt."`
  2. `"What was the codeword?"`
- **Covers**: Two sequential turns (`TurnStarted { turn: 1 }` → `TurnCompleted { turn: 1 }` → `TurnStarted { turn: 2 }` → `TurnCompleted { turn: 2 }`) in a single ACP session, plus `agent_thought_chunk` (`MessageKind::Reasoning`) and `plan` (`PlanUpdated`).
