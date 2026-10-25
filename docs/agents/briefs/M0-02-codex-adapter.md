# M0-02-codex-adapter: Codex app-server client

Model: GPT-6.1 Sol · Reviewer: Sonnet 5.5 · Depends on: none · Wave 1
Branch: `agent/sol/M0-02-codex-adapter` · Worktree: `../atrium-wt/M0-02-codex-adapter`

## Goal
A Rust `AgentAdapter` for Codex over `codex app-server` (JSON-RPC over stdio), mapping its notifications and server requests into Atrium's normalized events, with approvals routed back to Atrium.

## Context
- PLAN §3.2 (Codex bullet), §3.3 Home policy (`sandbox=read-only`, `approval_policy=on-request`), §3.11 rate limits.
- Contracts: `docs/contracts/m0.md`, `crates/atrium-agents/src/lib.rs`, `crates/atrium-protocol/src/agent.rs`.
- Installed: `codex` 0.163.0. Protocol reference: `codex app-server generate-json-schema --out spikes/agent-codex/schema` (about 4 MB; **git-ignore it** in `spikes/agent-codex/.gitignore`, don't commit it). `generate-ts` exists too.

## Owned paths
- `spikes/agent-codex/**`
- `fixtures/agents/codex/**`
- `docs/spikes/M0-02-codex-adapter.md`
- `docs/agents/handoffs/M0-02-codex-adapter.md`

## Contract to implement (do not modify)
- `impl AgentAdapter for CodexAdapter`, `impl Normalizer for CodexNormalizer`.
- Handshake (`initialize`), start or resume a thread, start turns, interrupt.
- Command-execution and file-change approval **server requests** → `ApprovalRequested` (`request_id` = the JSON-RPC id as a string), answered from `ResolveApproval`.
- Agent message / reasoning deltas, command and patch items → `ToolCall*`, plan updates, token usage, rate-limit notifications → `RateLimited`.
- `PermissionPolicy`: `ReadOnly` → `sandbox=read-only` + `approval_policy=on-request`. `Ask` → workspace-write with every escalation sent to Atrium. `FullAccess` → no sandbox, no approvals. Write down the exact settings you chose in the report.
- Per-session MCP through per-process config overrides (`-c mcp_servers.<name>…`), never by editing `~/.codex/config.toml`.

## Definition of Done
- [ ] Every scenario from `docs/contracts/m0.md` §3, with a golden test each.
- [ ] `cargo run -p spike-agent-codex --example record -- <scenario>` and `--example chat`, as in M0-01.
- [ ] `cargo fmt --check` · `cargo clippy -p spike-agent-codex --all-targets -- -D warnings` · `cargo test -p spike-agent-codex`
- [ ] Spike report `docs/spikes/M0-02-codex-adapter.md`: protocol version/stability notes, time to first event, `Unmapped` records, what rate-limit data is available.
- [ ] Handoff written.

**Live runs**: try recording with the cheapest model `model/list` reports. If your own sandbox stops `codex app-server` from running (e.g. it can't write `~/.codex`), **don't work around it**. Write fixtures by hand from the schema, mark them `hand-made` in their `.md`, and say so in the handoff; the orchestrator records the real ones during verification.

## Out of scope
The daemon, storage, UI, the policy engine, other providers. The `codex exec --json` format (that's only for the build workers).

## Escalate if
- An approval can't be routed to the client (Codex decides on its own under every setting). That's a security finding; report it and stop.
- You need a contract change → `docs/agents/change-requests.md` and stop.
