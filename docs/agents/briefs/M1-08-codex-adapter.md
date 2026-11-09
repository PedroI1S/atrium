# M1-08-codex-adapter: Codex adapter (production, fixtures)

Model: Gemini 4 Argon · Reviewer: Sonnet 5.5 · Depends on: contract (M1 rev 1); M1-10 for saturation tests
Branch: `agent/argon/M1-08-codex-adapter` · Worktree: `../atrium-wt/M1-08-codex-adapter`

## Goal
Fold the M0-02 spike into `atrium-agents` as the production Codex adapter on the M1 contract, against the recorded fixtures. Codex can't run live on this shift (D-047): the live checks are a separate task on rodrigo's shift.

## Context
- `docs/contracts/m1.md` §3, §5, §6.1, §10. M0 contract §3 (fixtures and scenarios).
- `crates/atrium-agents/src/lib.rs` (rev 3), `crates/atrium-protocol/src/{agent,policy,limits}.rs`.
- The spike: `spikes/agent-codex/`, report `docs/spikes/M0-02-codex-adapter.md`, fixtures `fixtures/agents/codex/`.
- `m1-must-fix.md` X1–X3 (live, not yours) and **X6**: 12 startup-failure goldens emit `Unmapped` before the session (only `Error { fatal }` + `SessionEnded` are allowed now, with diagnostics in `Error::details`), and the hand-made `interrupt` golden has an inverted closing order.
- Decisions: D-044, D-046, D-047.

## Owned paths
- `crates/atrium-agents/src/codex/**` (declare `pub mod codex;` in a one-line change to `lib.rs`, the only edit allowed there)
- `crates/atrium-agents/tests/codex*.rs`, `crates/atrium-agents/examples/codex-*.rs`
- `crates/atrium-agents/Cargo.toml`: dependency lines only, with `# M1-08`
- `fixtures/agents/codex/**`
- `docs/agents/handoffs/M1-08-codex-adapter.md`

## Contract to implement
- `codex::CodexAdapter: AgentAdapter`, `codex::CodexNormalizer: Normalizer`, over `codex app-server`.
- Descriptor: Codex is provider-gated, so classes it doesn't route through approval requests are `ProviderGated`, and classes not verified live are `Unknown` (both make it Partial). Record `~/.codex/config.toml` and any profile as `ConfigSource`s. `tested_versions` = the version the fixtures were recorded with.
- Approvals through tickets and `gate.redeem` before answering the app-server's approval request (same rules as the Claude brief). Home threads use `sandbox=read-only`, `approval_policy=on-request`.
- `MessageBody`, `ContextUpdated`, `Error::details`, output caps and record limits; closing through a bounded terminal tail; shutdown via `atrium_host::supervise`.
- Re-normalize the existing recorded inputs into rev-3 goldens. Where a fixture is hand-made, keep it marked hand-made.

## Definition of Done
- [ ] All Codex goldens regenerated; the X6 violations are gone (run `cargo test -p atrium-agents --test consistency -- --ignored` and show it reports no Codex violations).
- [ ] Saturation test as in the M0 contract §3.2, with process-group cleanup (`#[ignore = "needs M1-10"]` if `supervise` isn't merged yet; say so).
- [ ] A live harness (`cargo run -p atrium-agents --example codex-record -- <scenario>`) ready for rodrigo's shift, with a short `fixtures/agents/codex/LIVE.md` listing what to record for X1–X3.
- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test -p atrium-agents`
- [ ] Handoff with outputs and the enforcement matrix, marking every row that is unverified until the live run.

## Out of scope
Live Codex runs (no login on this shift: don't try to log in), the Claude adapter, the policy engine, the daemon.

## Escalate if
A fixture can't be mapped to the rev-3 contract; two fix rounds without passing.
