# M1-44 review round 4 — Claude Sonnet 5.5 (Claude Code subagent on the driver's plan, read-only; D-170 fallback, agy's Claude quota hit its weekly limit)

Branch at `8ead1879`. Orchestrator's verification: fmt/clippy clean; workspace 935/0/7; `claude_` tests in M1-28-followups ×200 quiet and ×100 under load (up to 39): 0 failures.

## Verdict

**Approve.** Round 4 answers every worker-owned item from review 3, and nothing new was found in the whole diff.

- **Finding 1 (`Cargo.toml`):** recorded correctly. `crates/atrium-agents/Cargo.toml:27` adds `tokio` with `test-util` under `[dev-dependencies]` only; the handoff's "Deviations from Owned Paths" entry gives the reason and the dev-only scope and says the driver signs off at merge. `resolver = "3"`, `Cargo.lock` unchanged.
- **Finding 2:** `claude_hung_withdrawals_finish_within_one_deadline` now uses a 1 s withdraw timeout and requires `900 ms <= elapsed < 3 s`. The spy's withdraw hangs forever, so ignoring the seam (20 s fallback), not waiting, or a sequential 5 × 1 s drain all fail. The stall margin is about 2 s, up from 300 ms. No assertion loosened.
- **Finding 3:** `with_withdraw_timeout_propagates_through_start` uses a 600 s `gate_deadline` and 30 s failure bounds; only a real propagation failure trips it.
- **Finding 5:** handoff line references match HEAD; Round 2 item 4 is marked superseded.
- **Test gap 2:** the `open_count == 0` assert reports `request_id`, `iter/total` and `gate.question_calls()`, in the Claude test and its Codex twin.

The cause holds against the code (no ordering bug; the second pending row comes only from a ticket abandoned at the old 150 ms deadline, kept pending by the blocked withdraw). The late-ticket path is safe in production (`OrphanGuard::drop` queues the withdraw, or spawns it directly at shutdown; the permit is held until it completes; the row never reaches the UI because `event_opened` stays 0), and `late_question_ticket_denies_without_event_and_withdraws_orphan` pins it deterministically. The hidden `with_withdraw_timeout` is within the brief; `ClaudeOptions::withdraw_timeout` would be the better long-term shape, covered by the handoff's change request. Only four files changed, no `atrium-core` or `Cargo.lock` change.

## Findings

None. Review 3's findings 4 and 6 belong to the orchestrator at merge.

## Test gaps

None blocking. The original flake was never reproduced under load; the proof is deterministic (the late-ticket test plus the paused-clock drain test), with the orchestrator's 0/200 quiet and 0/100 loaded runs as the empirical check.
