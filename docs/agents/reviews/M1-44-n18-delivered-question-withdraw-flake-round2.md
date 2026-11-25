# M1-44 review round 2 — Claude Sonnet 5.5 (Claude Code subagent on the driver's plan, read-only; D-170 fallback)

Branch at `b4ef7066`. Orchestrator's verification: fmt/clippy clean; workspace 935/0/7; `claude_` tests in M1-28-followups ×200 quiet and ×100 under load (up to 46): 0 failures.

## Verdict

**Request changes** (one Low; no product bug). All six round-1 findings are fixed. The cause holds: `QuestionAsked` comes only after `question_ticket` commits the row, and `ledger.withdraw` commits before it returns, so the only source of `open_count == 1` is a ticket abandoned at the test's 150 ms `gate_deadline`. In production a late ticket is always cleaned up: `OrphanGuard::drop` (adapter.rs ~458-476) queues `(ticket, permit)` on `withdraw_tx`, or spawns `gate.withdraw` directly if the channel is closed; the actor drains it every loop iteration (~1017) and at both shutdown points (~1104, ~1141), holding the permit and in-flight slot until the withdrawal ends. Production behaviour is unchanged: the override applies only in `drain_withdrawals`.

## Findings

1. **Low: the fallback test's 1 s wall-clock bound can flake under load.**
   - **Where:** `withdraw_timeout_bounds_drain_without_shrinking_gate_deadline`, part 2 (adapter.rs, `elapsed < Duration::from_secs(1)`), beside an outer `timeout(2 s)`.
   - **Failing case:** under 2×ncpu CPU hogs, a stalled scheduler delays the 20 ms `timeout_at` wake past 1 s, and the test fails ("fallback stayed blocked too long") although nothing is wrong. That is the flake class this task removes.
   - **Fix:** drop the wall-clock bound, and prove the fallback deterministically: distinctive values for `gate_deadline` and `withdraw_timeout`, with `tokio::time::pause()` and `advance`, asserting the drain detaches exactly at `gate_deadline` when there's no override.

## Round-1 findings, checked

- **1. Late-ticket test** (`late_question_ticket_denies_without_event_and_withdraws_orphan`): fixed and deterministic. Every wait is on a watch channel; the only timer is the 20 ms `gate_deadline`, which can fire only while the ticket is blocked; the `timeout(2 s)` calls are failure bounds.
- **2. Single construction site:** fixed. `Actor::new` takes `withdraw_timeout`; the only call sites are `start` and a test helper passing `None`. The change request (move it into `ClaudeOptions`) is in the handoff.
- **3. Shutdown cap and hook timeout:** fixed in the handoff; the `< 450 ms` bound holds because the in-flight count is 0 at shutdown.
- **4. Fallback and propagation:** fixed. `with_withdraw_timeout_propagates_through_start` goes through `AgentAdapter::start` with a 60 s `gate_deadline` and a 20 ms override.
- **5. Call-site comments:** fixed at all three sites.
- **6. UI visibility:** confirmed. `event_opened` defaults to 0 (`0001_init.sql:114`), is set to 1 only when `QuestionAsked` is stored (`writer.rs:891-893`), and the attention and open-items queries filter on it (`writer.rs:159`, `652`). **Wording correction for the handoff:** `open_count` (`requests.rs:225-237`) counts by `status IN ('pending','approved')` and does not filter on `event_opened`.

The seam: one `#[doc(hidden)] pub` method, no `ClaudeOptions` change, no protocol or `atrium-core` change. A `ClaudeOptions::withdraw_timeout` field would be the better shape, but it's a public type change, so the hidden method plus the change request is right for this task. No assertion in the three changed integration tests was loosened.

## Test gaps

- No end-to-end test of the original failure (a late ticket against the real ledger plus a blocked withdraw, checking `open_count`). The mock-gate unit test pins the cleanup path; not a blocker.
- Fallback timing: finding 1.
- N18 is still open in `m1-must-fix.md`; the orchestrator closes it at merge.
