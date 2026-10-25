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
- `session/update` notifications: message and thought chunks → `MessageDelta` (`Assistant` / `Reasoning`), `tool_call` → `ToolCallStarted`, `tool_call_update` → `ToolCallUpdated` using the replacement fields (title, kind, input, content) and `ToolCallFinished` on a terminal status (ACP tool kinds map almost 1:1 to `ToolKind`), `plan` → `PlanUpdated`.
- `session/request_permission` → `SessionConfig::gate`. `Allow`/`Deny` answer at once with the agent's allow-once / reject option; `Ask` → `ApprovalRequested`, then `RequestResolved`. Atrium's "always" rules live in Atrium, not the agent.
- `Interrupt` → `session/cancel`; pending permission requests resolve as `cancelled`. Stop reason → `TurnOutcome`.
- **Policy enforcement (`docs/contracts/m0.md` §3.1)**: by default an ACP agent only asks about what it chooses to, so expect `ProviderGated`. Spike the alternative: advertise the client **fs** (`read_text_file` / `write_text_file`) and **terminal** capabilities, so OpenCode's file and command operations go through the adapter, where the gate sees every one before it runs. Report per tool class which path OpenCode actually uses, in the enforcement matrix.
- `PermissionPolicy`: `Ask`/`ReadOnly` as above (the gate decides); `FullAccess` → the agent's most permissive mode (the user's choice, allowed on the host, D-011); permission requests still pass the gate.
- Live inputs go through `AdapterInput` and the same `Normalizer` as replays.

## Definition of Done
- [ ] Hand-made fixtures (from the ACP schema in the crate) for every scenario in `docs/contracts/m0.md` §3.3, marked `hand-made` in their `.md`, each with a golden test.
- [ ] The saturation test (§3.2).
- [ ] `cargo run -p spike-agent-acp --example record -- <scenario>` (records live from `opencode acp`, sanitized; with and without the fs/terminal capabilities) and `--example chat`.
- [ ] `cargo fmt --check` · `cargo clippy -p spike-agent-acp --all-targets -- -D warnings` · `cargo test -p spike-agent-acp`
- [ ] Spike report `docs/spikes/M0-04-acp-adapter.md`: the enforcement matrix (with and without client capabilities), protocol version, which ACP features OpenCode supports (load/resume, plans, permissions), `Unmapped` records, and the crate version you pinned.
- [ ] Handoff written.

**Live runs**: your worker policy doesn't allow running `opencode`. Build the harness so the orchestrator can record the real fixtures during verification, and list the exact commands in the handoff.

## Out of scope
The daemon, the UI, the policy engine, other providers, implementing ACP *server* features.

## Escalate if
- With the fs/terminal capabilities advertised, OpenCode still writes files or runs commands without going through them or asking. That's a security finding; report it and stop.
- You need a contract change → `docs/agents/change-requests.md` and stop.
