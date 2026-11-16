# M1-26-gate-withdraw: withdrawn requests free their ledger slot (P2, CR-18)

Model: Gemini 4 Hydrogen (`gemini-4-hydrogen-high`, `scripts/agent-run agy`; D-111) · Reviewer: Sonnet 5.5 (read-only, cross-provider) · Depends on: nothing open
Go: D-111, CR-18 (Pedro) · Branch: `agent/hydrogen/M1-26-gate-withdraw` · Worktree: `../atrium-wt/M1-26-gate-withdraw`

## Goal
P2 (`m1-must-fix.md`): a ticket opened by an `Ask` whose request is then withdrawn (cancelled, or never followed by `can_use_tool`) stays `pending` in the ledger until the turn ends and counts against `MAX_PENDING_REQUESTS`. `SessionGate` has no call to withdraw it. CR-18 adds one, and the adapters call it.

## Context
- `docs/agents/change-requests.md` CR-18; `docs/agents/m1-must-fix.md` P2; contract `docs/contracts/m1.md` §3 and §7 (gate, ledger, requests).
- `crates/atrium-core/src/policy/mod.rs` (`SessionGate`), `crates/atrium-core/src/policy/gate.rs` (`impl SessionGate`, around line 1166), `crates/atrium-core/src/policy/ledger.rs` (the ledger already has `withdraw`, line 284), `crates/atrium-core/src/lib.rs` (`ApprovalLedger::withdraw`, `ToolGate`).
- The adapters: `crates/atrium-agents/src/claude/*` (the permission prompt tool, hooks, `can_use_tool`) and `crates/atrium-agents/src/codex/*` (approvals, `MAX_PENDING_REQUESTS` at `adapter.rs:1254`).
- The daemon's `RequestResolved { cancelled }` path.

## Owned paths
- In `crates/atrium-core/src/policy/gate.rs`: only new code inside `impl SessionGate`, added at its end. Another task (M1-18) is rewriting the rest of `gate.rs`, `shell.rs`, `deny.rs` and `engine.rs`: don't touch them, or the merge will conflict.
- `crates/atrium-core/src/policy/mod.rs`, `ledger.rs`, `crates/atrium-core/src/lib.rs` (the `ToolGate` trait, if the call belongs there).
- `crates/atrium-agents/src/{claude,codex}/**` where a request is withdrawn.
- Tests in `crates/atrium-core/tests/` and `crates/atrium-agents/tests/` (new files preferred).
- Contract §3/§7: the CR-18 wording. `docs/agents/handoffs/M1-26-gate-withdraw.md`.

## What to do
1. Add `withdraw` to the gate (and to `ToolGate` if adapters reach the gate through it): it resolves an open ticket as cancelled, emits/returns what the daemon needs for `RequestResolved { cancelled }`, frees the slot, and is idempotent (withdrawing a resolved, redeemed or unknown ticket is a no-op, never an error that ends the turn). It never turns a ticket into an approval.
2. Call it from both adapters wherever a request is withdrawn: the provider cancels it, the turn is interrupted, or an `Ask` is never followed by the tool call (find how each adapter can tell).
3. Tests:
   - core: withdrawn tickets no longer count against `MAX_PENDING_REQUESTS` (open the limit, withdraw, open again); a withdrawn ticket can't be redeemed; withdraw is idempotent;
   - each adapter: a withdrawn request calls `withdraw` (with the existing fakes/fixtures; never a real provider).
   Show the core test fails on `dev`.

## Definition of Done
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast`.
- [ ] `git diff dev -- crates/atrium-core/src/policy/gate.rs` shows only lines added inside `impl SessionGate`.
- [ ] Handoff: the API, where each adapter calls it, the contract wording, the outputs; update P2 in nothing (the orchestrator does).

## Rules
- Work only in your worktree; commit on your branch only. Never push, merge or rebase. Never run a real provider.
