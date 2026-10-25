# Fixture: `question`

- **Source**: `hand-made` (constructed against `agent-client-protocol = 3.5.0` / ACP v1 `elicitation/create` schema; worker sandbox policy blocks executing `opencode`)
- **Provider**: ACP (`opencode acp`, v2.1.3)
- **Command**: `opencode acp`
- **Model**: `anthropic/claude-haiku-4-5`
- **Client capabilities**: `fs.readTextFile = true`, `fs.writeTextFile = true`, `terminal = true`, `elicitation.form = {}`
- **Prompt**: `"Scaffold a new module, but ask me which language to use first."`
- **Covers**: `elicitation/create` (`mode: "form"`) mapped to `QuestionAsked` with structured `QuestionOption`s, answered via `SessionCommand::AnswerQuestion`, emitting `RequestResolved { outcome: Answered }` and sending `CreateElicitationResponse { action: "accept" }` to the agent.
