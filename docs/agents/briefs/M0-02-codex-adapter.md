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
- Command-execution and file-change approval **server requests** go to `SessionConfig::gate` (`request_id` = the JSON-RPC id as a string). `Ask` → `ApprovalRequested`, answered from `ResolveApproval`; the server's request-resolved notification (including cleanup on interrupt) → `RequestResolved`.
- Agent message / reasoning deltas, command and patch items → `ToolCall*` (updates use the replacement fields), plan updates, token usage (per-turn running total, see `agent.rs`), rate-limit/quota notifications → `QuotaUpdated`; only an actual refusal → `RateLimited`.
- **Policy enforcement (`docs/contracts/m0.md` §3.1)**: find the strictest setting that sends *every* command, file change and MCP call to the client before it runs (e.g. `approval_policy` "untrusted" plus whatever covers commands Codex considers safe). If reads or "safe" commands still run unchecked, report `ProviderGated` and fill in the enforcement matrix honestly; don't hide it.
- `PermissionPolicy`: `ReadOnly` → `sandbox=read-only` + the strictest approval setting. `Ask` → workspace-write + the strictest approval setting. `FullAccess` → `danger-full-access` + no approvals, on the host or in a sandbox (the user's choice, D-011). Write down the exact settings in the report.
- Per-session MCP through per-process config overrides (`-c mcp_servers.<name>…`), never by editing `~/.codex/config.toml`.
- Live inputs (stdin requests/responses, stdout, gate decisions, commands) go through `AdapterInput` and the same `Normalizer` as replays.

## Definition of Done
- [ ] Every scenario from `docs/contracts/m0.md` §3.3, with a golden test each.
- [ ] The saturation test (§3.2).
- [ ] `cargo run -p spike-agent-codex --example record -- <scenario>` and `--example chat`, as in M0-01.
- [ ] `cargo fmt --check` · `cargo clippy -p spike-agent-codex --all-targets -- -D warnings` · `cargo test -p spike-agent-codex`
- [ ] Spike report `docs/spikes/M0-02-codex-adapter.md`: the enforcement matrix, protocol version/stability notes, time to first event, `Unmapped` records, what quota/rate-limit data is available.
- [ ] Handoff written.

**Live runs**: try recording with the cheapest model `model/list` reports. If your own sandbox stops `codex app-server` from running (e.g. it can't write `~/.codex`), **don't work around it**. Write fixtures by hand from the schema, mark them `hand-made` in their `.md`, and say so in the handoff; the orchestrator records the real ones during verification.

## Out of scope
The daemon, storage, UI, the policy engine, other providers. The `codex exec --json` format (that's only for the build workers).

## Escalate if
- Writes or non-read commands run without reaching the client under every setting. That's a security finding; report it and stop. (Unchecked reads are expected to be possible: report them in the matrix, don't stop.)
- You need a contract change → `docs/agents/change-requests.md` and stop.
