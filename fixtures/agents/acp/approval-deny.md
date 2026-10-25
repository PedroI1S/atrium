# Fixture: `approval-deny`

- **Source**: `hand-made` (constructed against `agent-client-protocol = 3.5.0` / ACP v1 schema; worker sandbox policy blocks executing `opencode`)
- **Provider**: ACP (`opencode acp`, v2.1.3)
- **Command**: `opencode acp`
- **Model**: `anthropic/claude-haiku-4-5`
- **Client capabilities**: `fs.readTextFile = true`, `fs.writeTextFile = true`, `terminal = true`, `elicitation.form = {}`
- **Prompt**: `"Run \`rm -rf build\`."`
- **Covers**: `session/request_permission` routed to `SessionConfig::gate` returning `GateDecision::Ask` → emits `ApprovalRequested` → resolved with `SessionCommand::ResolveApproval { decision: Deny }` → adapter selects `reject_once` option → emits `RequestResolved { outcome: Denied }` and `ToolCallFinished { status: Denied }`.
