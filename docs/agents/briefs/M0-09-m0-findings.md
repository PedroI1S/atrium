# M0-09-m0-findings: cross-adapter consistency tests and the M0 findings report

Model: Gemini 3.8 Flash · Reviewer: spot-check by the orchestrator (Opus 5.5) · Depends on: M0-01…M0-04 merged; the other spike reports for part 2 · Wave 3
Branch: `agent/flash/M0-09-m0-findings` · Worktree: `../atrium-wt/M0-09-m0-findings`

## Goal
1. Check that the four adapters normalize the *same* scenarios the *same* way.
2. Consolidate all M0 spike reports into one findings document for the M0 gate review.

## Context
- `docs/contracts/m0.md` (scenarios, §3), `crates/atrium-protocol/src/agent.rs` (mapping rules in the module docs).
- Inputs: `fixtures/agents/*/`, `docs/spikes/M0-0*.md`, `docs/agents/handoffs/M0-0*.md`.

## Owned paths
- `crates/atrium-agents/tests/consistency.rs` (new file only; the rest of the crate is contract)
- `docs/spikes/M0-findings.md`
- `docs/agents/handoffs/M0-09-m0-findings.md`

## Contract to implement (do not modify)
- `consistency.rs` loads every scenario's `.events.json` across providers and checks the invariants in the `atrium_protocol::agent` docs:
  - `SessionStarted` comes first and `SessionEnded` last;
  - every `ToolCallStarted` has exactly one `ToolCallFinished`;
  - deltas are followed by one `MessageCompleted` whose text equals their concatenation;
  - every `ApprovalRequested` in `approval-*` scenarios is followed by a `ToolCallFinished` with the right status;
  - `TurnStarted`/`TurnCompleted` pair up and `turn` increases.

  It reads the golden files only, so it needs no dependency on the spike crates.
- `M0-findings.md`: one table of measurements vs PLAN §4 budgets (pass/fail/not measured), per-spike recommendation, a list of every `Unmapped` record type per provider, security findings, and open questions for the driver. Quote numbers; don't re-derive them.

## Definition of Done
- [ ] `cargo fmt --check` · `cargo clippy -p atrium-agents --all-targets -- -D warnings` · `cargo test -p atrium-agents`
- [ ] Violations found by the consistency test are listed in the report per adapter, not fixed. You don't own the adapters.
- [ ] Handoff written.

## Out of scope
Changing adapters, fixtures or golden files. Making recommendations the spike reports don't support.

## Escalate if
- A spike report has no measurements where the brief required them. List it as "not measured"; don't estimate.
