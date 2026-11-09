# M1-07-claude-adapter: Claude Code adapter (production)

Model: Sonnet 5.5 · Reviewer: Gemini 4 Argon · Depends on: contract (M1 rev 1); M1-10 for live and saturation tests
Branch: `agent/sonnet/M1-07-claude-adapter` · Worktree: `../atrium-wt/M1-07-claude-adapter`

## Goal
Fold the M0-01 spike into `atrium-agents` as the production Claude Code adapter, on the M1 contract, with every M1 must-fix item for Claude closed.

## Context
- `docs/contracts/m1.md` §3, §5, §6.1, §10. M0 contract §3 (fixtures and required scenarios) still applies.
- `crates/atrium-agents/src/lib.rs` (rev 3), `crates/atrium-protocol/src/{agent,policy,limits}.rs`.
- The spike: `spikes/agent-claude/` and its report `docs/spikes/M0-01-claude-adapter.md`.
- `docs/agents/reviews/M0-gate.md` §2 Claude row and §3 item 3; `m1-must-fix.md`: "Claude" line, process groups, credential-safe Debug, X6.
- Decisions: D-017 (Claude keeps its native settings; external hooks make enforcement Partial), D-044, D-046.

## Owned paths
- `crates/atrium-agents/src/claude/**` (declare `pub mod claude;` in a one-line change to `lib.rs`, the only edit allowed there)
- `crates/atrium-agents/tests/claude*.rs`, `crates/atrium-agents/examples/claude-*.rs`
- `crates/atrium-agents/Cargo.toml`: dependency lines only, with `# M1-07`
- `fixtures/agents/claude/**`
- `docs/agents/handoffs/M1-07-claude-adapter.md`

## Contract to implement
- `claude::ClaudeAdapter: AgentAdapter` and `claude::ClaudeNormalizer: Normalizer`.
- `probe` and the session descriptor (built only with `EnforcementDescriptor::evaluate`): inspect user, project, local and managed settings, plugin directories, and any `--settings` file. Plugin hooks and anything you can't parse are `inspected: false`. External hooks → `ExternalHooks` reason. `tested_versions` lists the versions your live fixtures were recorded with.
- Approvals: `ApprovalRequested.request_id` is the gate's ticket id. On `ResolveApproval { Allow }`, call `gate.redeem` immediately before answering `can_use_tool`; anything but `Allow` answers deny. Record each redeem as `AdapterInput::Redeem`.
- `MessageBody` (`Streamed` with chars/deltas/sha256; `Inline` only for short completion-only messages), `ContextUpdated` for context figures, `Error::details` for pre-session diagnostics, output caps and record limits from `limits.rs`.
- Shutdown: soft stop, then `atrium_host::supervise::terminate` on the process group. Closing events go through a bounded terminal tail (`TERMINAL_TAIL_EVENTS`) and are never dropped after 300 ms (`spikes/agent-claude/src/adapter.rs:696`).

## Definition of Done
- [ ] All nine M0 scenarios re-recorded **live** with the current `claude` CLI and the cheapest model, sanitized, `Sanitizer::is_clean` asserted; goldens regenerated on the rev-3 event shapes.
- [ ] New fixtures: `plugin-hook` (a plugin with a hook → Partial with the reason), `stale-approval` (an Allow that arrives after the turn ended is never executed).
- [ ] Saturation test (receiver never read): `Interrupt` and `Shutdown` work within `SHUTDOWN_GRACE`, the whole process group is gone, and `SessionEnded` is delivered once the receiver drains. If M1-10's `supervise` isn't merged yet, mark the tests that need it `#[ignore = "needs M1-10"]` and say so; the orchestrator runs them after rebasing.
- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test -p atrium-agents`
- [ ] Handoff with outputs, the enforcement matrix (M0 contract §3.1) for the tested version, and the descriptor printed for your own machine's setup (sanitized).

## Out of scope
The policy decisions (the gate is given to you; tests use a scripted gate), the daemon, the Codex adapter, MCP injection beyond keeping `SessionConfig::mcp_servers` working.

## Escalate if
The CLI's control protocol changed in a way the contract can't express; a must-fix item can't be closed as written; two fix rounds without passing.
