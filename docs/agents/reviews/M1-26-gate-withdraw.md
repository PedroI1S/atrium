# Review: M1-26-gate-withdraw (80af910e, b3522315, bb93f2d0; P2, CR-18)

Reviewer: Claude Sonnet 5.5, read-only (cross-provider). Author: Gemini 4 Hydrogen `high`. Saved by the orchestrator from the reviewer's report. The orchestrator checked that `gate.rs` is add-only and that the branch merges cleanly with dev and with M1-18.

## Verdict: **approve** (two minors, two nits, none blocking)

## Safety
- **Withdraw only cancels.** `SessionGate::withdraw` (`gate.rs:1351-1363`) drops the ticket from `opened` and calls `ledger.withdraw`, a compare-and-set from `pending|approved` to `cancelled` (`ledger.rs:284-297`). Nothing in it can approve.
- **A withdrawn ticket is never redeemed:** `do_redeem` fails with "unknown approval" and the ledger refuses a `cancelled` row; tested for pending and approved-but-unredeemed tickets.
- **Idempotent:** redeemed, denied, answered, cancelled and unknown tickets refuse, and `withdraw` discards the refusal; tested.
- **Races resolve one way:** one compare-and-set wins; a withdraw racing a redeem makes the redeem fail, which denies the call.
- **No still-pending ticket is withdrawn in Claude's hook sequence:** a later `can_use_tool` with the same call and input takes the hook ticket first (`take_hook_ticket`, `adapter.rs:1247`). `withdraw_hook_for_call` runs only after a miss (input changed), on oversize arguments, or on a `tool_result`, where the ticket really is orphaned.
- **The app sees cancellations** through the existing path: the daemon calls `ledger.withdraw` for every stored `RequestResolved { Cancelled | Expired }` (`sessions.rs` ~410-420). A ticket withdrawn before `ApprovalRequested` has no card.

## Call sites
- Claude: `control_cancel_request`, turn end, process exit, unmatched hook Ask (miss, oversize, `tool_result`), late Ask results, non-Allow redeems, `DeclineQuestion`. Interrupt ends in `result` or exit.
- Codex: `serverRequest/resolved`, `turn/completed`, discard, driver exit, not-live and no-metadata cases, saturation decline, non-Allow redeem, `DeclineQuestion`, `redeeming:{native}`.
- No missed path found. Withdrawals drain at the top of the actor loop and before `SessionEnded`, a few ms late: acceptable.
- `ProbeGate` keeps the default no-op, but never opens tickets.

## Tests
Core: fill `MAX_PENDING_REQUESTS`, Deny at the limit, withdraw frees 2 slots, reopen works, the limit bites again. A no-op `withdraw` would fail the slot-count and `Expired` assertions. Adapters: real ledger through the fake providers, `asked == withdrawn`, `open_count == 0`, for Claude cancel, turn end and hook Ask with changed input; Codex interrupt with an approval and with a question; Codex saturation.

## Findings
1. **Minor:** withdrawals are awaited one at a time under `gate_deadline`/`REDEEM_TIMEOUT` (`claude/adapter.rs` ~993-1003, `codex/adapter.rs` ~1869-1876); a hung gate could stall the actor for N × the deadline. Fix: one `JoinSet` with one overall deadline, or detached tasks.
2. **Minor:** `clear_requests` aborts in-flight gate checks before draining (`claude/adapter.rs:1043-1047`), so a check that already opened a ticket can leak it until turn expiry, as before this change. A handoff note is enough.
3. **Nit:** Codex `guarded` now aborts its inner task on drop, which changes every gate check and redeem timeout (the safe direction, matching Claude). Flag it in the handoff as intentional.
4. **Nit:** the Claude `tool_result`-without-`can_use_tool` withdrawal has only a unit test, not a fake-provider test.

## What the reviewer ran
`cargo test -p atrium-core --test gate_withdraw` (3 passed); `cargo test -p atrium-agents --test gate_withdraw` (3 passed); `cargo test -p atrium-agents` (all suites ok); clippy on both crates clean.
