# M0-01-claude-adapter: Claude Code stream-json + control protocol client

Model: Sonnet 5.5 · Reviewer: GPT-6.1 Sol · Depends on: none · Wave 1
Branch: `agent/sonnet/M0-01-claude-adapter` · Worktree: `../atrium-wt/M0-01-claude-adapter`

## Goal
A Rust `AgentAdapter` for Claude Code that runs it headless and maps everything into Atrium's normalized events, with **every permission decision coming back to Atrium** (no auto-approval inside the CLI). Prove it on live recorded fixtures.

## Context
- PLAN §3.2 (adapters, Claude bullet), §3.3 Home policy, §3.12 approvals, §6 risk "Claude stream-json control protocol is not a stable public API".
- Contracts: `docs/contracts/m0.md`, `crates/atrium-agents/src/lib.rs`, `crates/atrium-protocol/src/agent.rs`.
- Installed: `claude` 2.1.308. Relevant flags (see `claude --help`): `-p --input-format stream-json --output-format stream-json --verbose`, `--include-partial-messages`, `--permission-prompts host` (or `--permission-prompt-tool`), `--mcp-config` + `--strict-mcp-config`, `--resume`, `--model`, `--permission-mode`, `--allowedTools`.

## Owned paths
- `spikes/agent-claude/**`
- `fixtures/agents/claude/**`
- `docs/spikes/M0-01-claude-adapter.md`
- `docs/agents/handoffs/M0-01-claude-adapter.md`

## Contract to implement (do not modify)
- `impl AgentAdapter for ClaudeAdapter` and `impl Normalizer for ClaudeNormalizer` (`atrium-agents`).
- Mapping as documented in `atrium_protocol::agent`, including:
  - partial messages → `MessageDelta`; thinking → `MessageKind::Reasoning`;
  - tool_use / tool_result → the `ToolCall*` triple; tool name → `ToolKind`;
  - `AskUserQuestion` → `QuestionAsked` with structured options; `TodoWrite` → `PlanUpdated`;
  - `result` → `Usage` + `TurnCompleted`; quota/usage-window records → `QuotaUpdated`; an actual refusal → `RateLimited`;
  - `SessionCommand::Interrupt` → the control-protocol interrupt.
- **Policy enforcement (`docs/contracts/m0.md` §3.1)**: every tool call, including Read/Grep/Glob and MCP tools, goes through `SessionConfig::gate` before it runs. Candidates: a per-session `PreToolUse` hook (generated settings passed with `--settings`, never the user's own settings) bridged back to the adapter, and/or `can_use_tool` / `--permission-prompts host` with nothing pre-allowed. Find the combination that reaches `AllToolCalls`, and show it with the `gate-deny` fixture. `Ask` → `ApprovalRequested`, then `RequestResolved`.
- `PermissionPolicy`: `Ask` and `ReadOnly` → Claude's normal permission mode, with everything routed to the gate. `FullAccess` → `--permission-mode bypassPermissions` (the user's choice, allowed on the host, D-011); calls still pass the gate where it can see them.
- Per-session MCP: `SessionConfig::mcp_servers` → a generated `--mcp-config` file + `--strict-mcp-config`. Never touch the user's global config.
- Live inputs go through `AdapterInput` and the same `Normalizer` as replays.

## Definition of Done
- [ ] All fixture scenarios from `docs/contracts/m0.md` §3.3, recorded live with the cheapest model (`--model haiku`) in a scratch dir inside your worktree, sanitized, each with a golden test.
- [ ] The saturation test (§3.2).
- [ ] `cargo run -p spike-agent-claude --example record -- <scenario>` re-records a scenario; `--example chat` runs an interactive session, printing events as JSONL and reading gate decisions and approvals from stdin.
- [ ] `cargo fmt --check` · `cargo clippy -p spike-agent-claude --all-targets -- -D warnings` · `cargo test -p spike-agent-claude`
- [ ] Spike report `docs/spikes/M0-01-claude-adapter.md`: the enforcement matrix; time from spawn to `SessionStarted`; which records stayed `Unmapped`; how stable the control protocol looks; whether the `claude-code-acp` fallback is needed.
- [ ] Handoff written.

## Out of scope
The daemon, storage, UI, the policy engine itself (just surface approvals), other providers.

## Escalate if
- No combination lets the gate see a tool class before it runs (e.g. Read or Bash slips through). Stop and report; that's a security finding.
- You need a contract change → `docs/agents/change-requests.md` and stop.
