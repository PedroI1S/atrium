# M1-44-n18-delivered-question-withdraw-flake: fix the blocked-withdrawals test flake (N18)

Model: Gemini 4 Neon (`gemini-4-neon-medium`, `scripts/agent-run agy`, D-158) · Reviewer: Claude Sonnet 5.5, read-only (D-162)
Go: Pedro approved the brief (D-167); **the start needs the driver's go**, once agy quota is back · Branch: `agent/neon/M1-44-n18-delivered-question-withdraw-flake`, cut from `dev` · Worktree: `../atrium-wt/M1-44-n18-delivered-question-withdraw-flake`

## The flake (N18 in `docs/agents/m1-must-fix.md`)
`crates/atrium-agents/tests/M1-28-followups.rs`, `claude_delivered_question_blocked_withdrawals_stay_bounded_and_complete` (from line 2222), failed once on macOS:
- **The failure:** `assert_eq!(store.requests().open_count(&thread, inc).await.unwrap(), 0)` at line 2327 got **1**.
- **When:** M1-42 round 5's verification, 2026-11-24, at load average ~5.
- **Since then:** it passed 30 of 30 reruns on `dev` at load ~3.

**What the test does:**
- A scripted fake Claude asks `MAX_PENDING_REQUESTS` questions one at a time. Each one is emitted only after the previous one is denied.
- For each question, the test waits for `AgentEvent::QuestionAsked` (`wait_for_question`, line 2198) and calls `ledger.withdraw(...)` for that request. That models the daemon freeing the slot before `DeclineQuestion`.
- It then asserts that the store has **0** open requests, and sends `DeclineQuestion`.
- Throughout, `BlockedDeliveredWithdrawGate` (line 2075) keeps the adapter's own `gate.withdraw()` blocked.

**What's suspected, unconfirmed:** when the test reads the store, one request is still open. Either:
- **(a)** the adapter emits `QuestionAsked` before the question's ledger record is written, so the test's `withdraw` runs first and the record lands after it. That would be an **ordering bug in the product**: a question shown before it is recorded. Or:
- **(b)** something else writes or re-opens a record concurrently: the detached withdrawal path, a recovery path, or the store's write being asynchronous with respect to `withdraw` returning.

## Goal
1. **Find the cause,** with evidence from the code: the Claude adapter's question delivery, `SessionGate`'s question ledger calls, `RequestLedger::withdraw` and the store's `open_count`. Say in the handoff which ordering lets `open_count` read 1, and whether it can happen in the real daemon or only in this test.
2. **If it is a product ordering bug** (for example, `QuestionAsked` reaches the session before the record is durable), fix it in the product: the record is written before the event is emitted, or `withdraw` waits for an in-flight insert. Add a test that pins the ordering deterministically, using gates or notifies, never sleeps. The M1-33 behaviour this test pins (detached withdrawals stay bounded and complete) must keep holding.
3. **If it is test-only,** make the test wait for the right condition: for example the record's existence, through a notify or a store query. It must not weaken what it pins.
4. **Rules for either case:**
   - Don't raise timeouts, loosen assertions or add retries.
   - Check the sibling tests in `M1-28-followups.rs` that use the same pattern (`wait_for_question`, then `ledger.withdraw`, then `open_count`), Codex variants included, and fix them the same way.

## Owned paths
- `crates/atrium-agents/tests/M1-28-followups.rs`
- If goal 2 applies, the product files involved, listed in the handoff:
  - `crates/atrium-agents/src/**` (the Claude adapter's question path, plus the Codex adapter's if the same ordering applies);
  - `crates/atrium-core/src/policy/gate.rs` and `crates/atrium-core/src/ledger*` (only the question-record ordering).
- `docs/agents/handoffs/M1-44-n18-delivered-question-withdraw-flake.md`
- **No protocol change.** If a fix needs one, or needs a public signature changed, don't make it. Write a change request in the handoff.

## Rules
- Follow `docs/agents/README.md`. Under D-123, run only `cargo fmt --all` and `cargo check -p <crate> --tests` on the crates you change. No `cargo test`; the orchestrator runs it.
- macOS and Linux are both supported, so every `cfg` builds on both.
- Commit on your branch only. Never push, merge, rebase or switch branches.

## Definition of Done (the orchestrator's verification)
- [ ] fmt, workspace clippy and `cargo test --workspace` are clean on macOS.
- [ ] `M1-28-followups` ×200 has 0 failures, and ×100 under CPU load (2× ncpu `yes`) has 0 failures.
- [ ] If goal 2 applies: with the fix reverted, the new ordering test fails.
- [ ] Sonnet's review approves with no findings (D-120).
