# M0-03-agy-adapter: Antigravity (`agy`) stream-json client

Model: Gemini 4 Argon · Reviewer: Sonnet 5.5 · Depends on: none · Wave 1
Branch: `agent/argon/M0-03-agy-adapter` · Worktree: `../atrium-wt/M0-03-agy-adapter`

## Goal
A Rust `AgentAdapter` for `agy` and answers to the three open `agy` questions in PLAN §3.2: multi-turn over `--input-format stream-json`, per-session MCP injection, and **routing approvals back to Atrium**.

## Context
- PLAN §3.2 (Antigravity bullet: event stream `init | step_update | result`, approvals via skip-permissions + a `PreToolUse` hook), §6 risk row for `agy`, D-002 and D-006 in `docs/agents/decisions.md`.
- Contracts: `docs/contracts/m0.md`, `crates/atrium-agents/src/lib.rs`, `crates/atrium-protocol/src/agent.rs`.
- Real data to start from: `fixtures/agents/agy/m0-00-smoke.jsonl` (see its `.md`). The installed CLI is now `agy` 1.4.7; `agy --help` lists the flags.
- Hook contract as observed: JSON on stdin (`toolCall.name`, `toolCall.args`, `workspacePaths`), `{"decision": "allow"|"deny", "reason"}` on stdout. Under `--dangerously-skip-permissions`, `deny` is enforced and anything else runs. `scripts/agy-policy.py` is a working example (read it, don't edit it).

## Owned paths
- `spikes/agent-agy/**`
- `fixtures/agents/agy/**`
- `docs/spikes/M0-03-agy-adapter.md`
- `docs/agents/handoffs/M0-03-agy-adapter.md`

## Contract to implement (do not modify)
- `impl AgentAdapter for AgyAdapter`, `impl Normalizer for AgyNormalizer`. `step_update` → `ToolCall*` / messages by `step_type` and `state`; `result` → `Usage` + `TurnCompleted`; `denied_actions` → `ToolCallFinished { status: denied }`.
- **Gate bridge** (`docs/contracts/m0.md` §3.1): per session, the adapter writes a hook config (matcher `"*"`) into a session-private directory (mode 0700). The config points at a small helper (a bin in this crate, `spike-agy-hook`) that connects back to the adapter over a unix socket in that directory and sends the tool call. The adapter asks `SessionConfig::gate`: `Allow` / `Deny` answer at once; `Ask` emits `ApprovalRequested` and the helper **blocks until** `ResolveApproval`, then `RequestResolved`. Hook traffic is recorded as `AdapterInput::HookRequest/HookResponse` and gate decisions as `AdapterInput::Gate`. Find out and report:
  - Does `agy` wait for a slow hook? Is there a timeout? What happens when it fires?
  - If it can't block, implement the PLAN fallback: deny with a reason, surface the approval, retry the step after `Allow`. A retry grant is bound to **session + tool name + canonical arguments** and is used **once**; a retried call with different arguments needs a new decision.
- **Fail closed**: the helper answers `deny` on every error it can see (socket missing, adapter gone, bad reply, its own timeout shorter than `agy`'s). What `agy` does when the helper itself can't run is tested live by the orchestrator (below).
- **Per-session MCP**: find a way to give one session its own MCP servers without `agy mcp add` (that's global). Candidates: a generated config dir via env var or flag, `--add-dir` with a project config, a plugin dir. **Never modify `~/.gemini`**. Report what works.
- **Multi-turn**: the `--input-format stream-json` stdin message shape, and whether a session survives several turns.
- `PermissionPolicy`: `Ask` and `ReadOnly` both run every call through the gate bridge; the gate, not the adapter, decides what reads are allowed. `--mode plan` for `ReadOnly` if it helps. `FullAccess` → `--mode auto` (the user's choice, allowed on the host, D-011); the gate bridge stays in place.

## Definition of Done
- [ ] A golden test over `m0-00-smoke.jsonl`, plus hand-made fixtures (from the observed format) for every scenario in `docs/contracts/m0.md` §3.3, marked `hand-made` in their `.md`.
- [ ] The saturation test (§3.2).
- [ ] `cargo run -p spike-agent-agy --example record -- <scenario>` (records live and sanitizes) and `--example chat`.
- [ ] Unit tests for the bridge with a fake hook client: allow, deny, ask → allow/deny, adapter dropped mid-wait, malformed request, socket missing. **All error cases must deny.** Retry-grant tests: same args allowed once; changed args or a second use need a new decision.
- [ ] A live **hook-failure harness** (`--example hook-failures`) for the orchestrator to run against the installed `agy`: helper binary missing, helper exits non-zero, helper prints invalid JSON, helper sleeps past `agy`'s hook timeout, hook config unreadable. For each, it reports whether the tool ran. Any case where it runs blocks `agy` use on the host until fixed (orchestrator decides).
- [ ] `cargo fmt --check` · `cargo clippy -p spike-agent-agy --all-targets -- -D warnings` · `cargo test -p spike-agent-agy`
- [ ] Spike report `docs/spikes/M0-03-agy-adapter.md`: the enforcement matrix, answers to the three questions above, and what still needs a live run.
- [ ] Handoff written.

**Live runs**: your worker policy doesn't allow running `agy` itself, and that's intended. Build the `record`/`chat` harness so the orchestrator can run it during verification; list in the handoff exactly which commands to run.

## Out of scope
The daemon, the UI, the real policy engine (the bridge just surfaces approvals), other providers. Editing `scripts/agy-policy.py` or anything under `.agents/`.

## Escalate if
- The hook bridge can be bypassed: a tool runs without the hook being called under `Ask`. That's a security finding; report it and stop.
- You need a contract change → `docs/agents/change-requests.md` and stop.
