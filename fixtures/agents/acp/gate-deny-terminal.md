# Fixture: `gate-deny-terminal`

- **Source**: `hand-made` (constructed against `agent-client-protocol = 3.5.0` / ACP v1 schema; worker sandbox policy blocks executing `opencode`)
- **Provider**: ACP (`opencode acp`, v2.1.3)
- **Command**: `opencode acp`
- **Model**: `anthropic/claude-haiku-4-5`
- **Client capabilities**: `fs.readTextFile = true`, `fs.writeTextFile = true`, `terminal = true`, `elicitation.form = {}`
- **Prompt**: `"Run \`cat secret.txt\` in the shell."`
- **Covers**: Pre-execution gate denial for `Execute` (`terminal/create`). The gate returns `GateDecision::Deny`, the adapter refuses to spawn the command and returns a JSON-RPC error, the tool call finishes with `ToolCallFinished { status: Denied }`, and `secret.txt` is never read.

- **Note (fix round 1)**: this is the *client-capability* path (`fs/*` or `terminal/*` requests). Live OpenCode v2.1.3 never sends them (M0-04 live run 1), so this fixture models a generic ACP agent that does. The OpenCode-shaped scenario of the same name uses `session/request_permission`.
