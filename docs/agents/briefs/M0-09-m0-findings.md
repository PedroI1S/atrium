# M0-09-m0-findings: cross-adapter consistency tests and the M0 findings report

Model: Gemini 3.8 Flash · Reviewer: spot-check by the orchestrator (Opus 5.5) · Depends on: M0-01…M0-04 merged; the other spike reports for part 2 · Wave 3
Branch: `agent/flash/M0-09-m0-findings` · Worktree: `../atrium-wt/M0-09-m0-findings`

## Goal
1. Check that the four adapters normalize the *same* scenarios the *same* way.
2. Consolidate all M0 spike reports into one findings document for the M0 gate review.

## Context
- `docs/contracts/m0.md` (scenarios, §3), `crates/atrium-protocol/src/agent.rs` (mapping rules in the module docs).
- Inputs: `fixtures/agents/*/` (live and `handmade/` fixtures), `docs/spikes/M0-0*.md`, `docs/agents/handoffs/M0-0*.md`, and `docs/agents/reviews/M0-*.md` (cross-provider reviews and triage).
- **Added at dispatch (2026-10-26)**:
  - `docs/agents/decisions.md` (D-008 … D-029, including D-012/D-017/D-019 enforcement levels, D-018 spikes merged with must-fixes, D-024 UI direction, D-028 the per-window memory budget);
  - `docs/agents/m1-must-fix.md` (link to it; don't duplicate it);
  - `docs/design/ui-direction.md`;
  - CR-01 … CR-03 in `docs/agents/change-requests.md`.
- **M0-06b** (layout B, opacity, pop-outs) is still in its final fix round on another branch, so its report isn't in your worktree. Put an "M0-06b: pending final round" placeholder row in every table and section where it belongs; the orchestrator fills it in after merge. Don't try to read other worktrees.
- **The `agy` fixtures** come from `agy` 1.4.7. The consistency test reads golden `.events.json` files only.

## Owned paths
- `crates/atrium-agents/tests/consistency.rs` (new file only; the rest of the crate is contract)
- `docs/spikes/M0-findings.md`
- `docs/agents/handoffs/M0-09-m0-findings.md`

## Contract to implement (do not modify)
- `consistency.rs` loads every scenario's `.events.json` across providers and checks the lifecycles and mapping rules in the `atrium_protocol::agent` module docs. First it classifies the run:
  - **established session** (contains `SessionStarted`): `SessionStarted` first, `SessionEnded` last;
  - **pre-session failure** (no `SessionStarted`): only `Error { fatal: true }` then `SessionEnded`.

  Then, for established sessions:
  - every `ToolCallStarted` has exactly one `ToolCallFinished`, and no `ToolCallUpdated` comes after it;
  - deltas are followed by one `MessageCompleted` whose text equals their concatenation;
  - every `ApprovalRequested` / `QuestionAsked` has exactly one `RequestResolved`, and none is left open at `SessionEnded`;
  - in `approval-deny` and `gate-deny`, the gated call ends `denied`; in `gate-deny`, no event contains the secret file's contents;
  - in `interrupt`, the turn ends `interrupted` and open calls end `cancelled`;
  - `TurnStarted`/`TurnCompleted` pair up and `turn` increases; no `Usage` comes after its turn's `TurnCompleted`;
  - the closing order on exit matches the module docs.

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
