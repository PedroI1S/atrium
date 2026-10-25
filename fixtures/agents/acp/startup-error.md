# Fixture: `startup-error`

- **Source**: `hand-made` (constructed against `agent-client-protocol = 3.5.0` / ACP v1 schema; worker sandbox policy blocks executing `opencode`)
- **Provider**: ACP (`opencode acp`, v2.1.3)
- **Command**: `opencode acp`
- **Model**: `invalid-provider/nonexistent-model`
- **Client capabilities**: `fs.readTextFile = true`, `fs.writeTextFile = true`, `terminal = true`, `elicitation.form = {}`
- **Prompt**: *(none — fails during session creation)*
- **Covers**: Pre-session failure lifecycle (`Error { fatal: true }` followed by `SessionEnded { exit_code: Some(1), .. }`, with **no** `SessionStarted` event).
