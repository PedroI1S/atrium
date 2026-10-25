# Fixture: `resume`

- **Source**: `hand-made` (constructed against `agent-client-protocol = 3.5.0` / ACP v1 schema; worker sandbox policy blocks executing `opencode`)
- **Provider**: ACP (`opencode acp`, v2.1.3)
- **Command**: `opencode acp`
- **Model**: `anthropic/claude-haiku-4-5`
- **Client capabilities**: `fs.readTextFile = true`, `fs.writeTextFile = true`, `terminal = true`, `elicitation.form = {}`
- **Resumed Session ID**: `ses_multi_01`
- **Prompt**: `"Confirm the codeword from our resumed session."`
- **Covers**: Resuming an existing ACP session in a new process via `session/load` with `SessionConfig::resume = Some("ses_multi_01")` → emits `SessionStarted { native_session_id: "ses_multi_01", .. }` followed by the new turn.
