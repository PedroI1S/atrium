# M0-04-acp-adapter: Agent Client Protocol client (OpenCode)

Model: Gemini 4 Argon · Reviewer: Sonnet 5.5 · Depends on: none · Wave 1
Branch: `agent/argon/M0-04-acp-adapter` · Worktree: `../atrium-wt/M0-04-acp-adapter`

## Goal
One Rust `AgentAdapter` for any ACP agent, using Zed's `agent-client-protocol` crate on the client side, proven against OpenCode (`opencode acp`, v2.1.3 installed).

## Context
- PLAN §3.2 (ACP bullet: MCP servers passed in `session/new`), §3.12 (ACP enforcement via permission requests), §7 open question "which ACP agents to test first" (OpenCode is the answer for M0; Goose isn't installed).
- Contracts: `docs/contracts/m0.md`, `crates/atrium-agents/src/lib.rs`, `crates/atrium-protocol/src/agent.rs`.

## Owned paths
- `spikes/agent-acp/**`
- `fixtures/agents/acp/**`
- `docs/spikes/M0-04-acp-adapter.md`
- `docs/agents/handoffs/M0-04-acp-adapter.md`

## Contract to implement (do not modify)
- `impl AgentAdapter for AcpAdapter` (agent command + args configurable; default `opencode acp`), `impl Normalizer for AcpNormalizer`.
- `initialize` → `session/new` (cwd + `SessionConfig::mcp_servers`) or `session/load` for resume, when the agent supports it → `session/prompt`.
- `session/update` notifications: message and thought chunks → `MessageDelta` (`Assistant` / `Reasoning`), `tool_call` / `tool_call_update` → `ToolCall*` (ACP tool kinds map almost 1:1 to `ToolKind`), `plan` → `PlanUpdated`.
- `session/request_permission` → `ApprovalRequested`, answered from `ResolveApproval`: pick the agent's allow-once option or a reject option. Atrium's "always" rules live in Atrium, not the agent.
- `Interrupt` → `session/cancel`. Stop reason → `TurnOutcome`.
- The client-side fs/terminal capabilities: advertise none in M0 (the agent uses its own tools), and note in the report what we'd gain by offering them.

## Definition of Done
- [ ] Hand-made fixtures (from the ACP schema in the crate) for every scenario in `docs/contracts/m0.md` §3, marked `hand-made` in their `.md`, each with a golden test.
- [ ] `cargo run -p spike-agent-acp --example record -- <scenario>` (records live from `opencode acp`, sanitized) and `--example chat`.
- [ ] `cargo fmt --check` · `cargo clippy -p spike-agent-acp --all-targets -- -D warnings` · `cargo test -p spike-agent-acp`
- [ ] Spike report `docs/spikes/M0-04-acp-adapter.md`: protocol version, which ACP features OpenCode supports (load/resume, plans, permissions), `Unmapped` records, and the crate version you pinned.
- [ ] Handoff written.

**Live runs**: your worker policy doesn't allow running `opencode`. Build the harness so the orchestrator can record the real fixtures during verification, and list the exact commands in the handoff.

## Out of scope
The daemon, the UI, the policy engine, other providers, implementing ACP *server* features.

## Escalate if
- OpenCode runs tools that should need permission without sending `session/request_permission`. That's a security finding; report it and stop.
- You need a contract change → `docs/agents/change-requests.md` and stop.
