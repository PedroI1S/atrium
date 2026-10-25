# Fixture: `approval-allow`

- **Source**: `hand-made` (constructed against `agent-client-protocol = 3.5.0` / ACP v1 schema; worker sandbox policy blocks executing `opencode`)
- **Provider**: ACP (`opencode acp`, v2.1.3)
- **Command**: `opencode acp`
- **Model**: `anthropic/claude-haiku-4-5`
- **Client capabilities**: `fs.readTextFile = true`, `fs.writeTextFile = true`, `terminal = true`, `elicitation.form = {}`
- **Prompt**: `"Run \`echo approved-ok\`."`
- **Covers**: `session/request_permission` routed to `SessionConfig::gate` returning `GateDecision::Ask` → emits `ApprovalRequested` → resolved with `SessionCommand::ResolveApproval { decision: Allow }` → adapter selects `allow_once` option → emits `RequestResolved { outcome: Approved }` → tool finishes `Succeeded`.
