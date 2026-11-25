# Handoff: M1-44-n18-delivered-question-withdraw-flake

Branch: `agent/neon/M1-44-n18-delivered-question-withdraw-flake`

## Summary

- **Classification:** Not a product question-record ordering bug; caused by `ClaudeAdapter` coupling `drain_withdrawals` to `ClaudeOptions::gate_deadline`, which forced the Claude withdrawal-bound tests to shrink `gate_deadline` to 150 ms for real SQLite `question_ticket` / `check` calls.
- **Fix:** Added a `#[doc(hidden)]` `ClaudeAdapter::with_withdraw_timeout(Duration)` test seam (matching `CodexOptions::withdraw_timeout` without changing `ClaudeOptions` or any public signature/contract), used it in `Actor::drain_withdrawals`, and updated the three Claude tests in `crates/atrium-agents/tests/M1-28-followups.rs` that previously shrank `gate_deadline` to 150 ms solely to bound `drain_withdrawals`.
- **Checks run (D-123):** `cargo fmt --all` and `cargo check -p atrium-agents --tests --examples` are clean with 0 warnings. No test suites were run by the worker.

## Cause

### What is proven from the code

1. **No product ordering bug between `QuestionAsked` and the ledger insert:**
   - In `ClaudeAdapter::spawn_question` (`crates/atrium-agents/src/claude/adapter.rs:1564-1590`), the inner task awaits `gate.question_ticket(turn).await` before sending `Internal::Question { ticket: Some(t), .. }`.
   - `SessionGate::question_ticket` (`crates/atrium-core/src/policy/gate.rs:1770-1777`) awaits `Ledger::open_question` (`crates/atrium-core/src/policy/ledger.rs:70-109`), which awaits `Requests::open_question` (`crates/atrium-core/src/store/requests.rs:207-211`) -> `Shared::call` (`crates/atrium-core/src/store/mod.rs:102-121`) -> `Writer::open_row` (`crates/atrium-core/src/store/requests.rs:42-89`). `Writer::open_row` executes `INSERT INTO requests ... status = 'pending'` and `tx.commit()` on the single writer thread before replying on its oneshot channel.
   - `ClaudeNormalizer::can_use` (`crates/atrium-agents/src/claude/normalizer.rs:956-960`) holds `AskUserQuestion` with `emitted: false` until `Actor::on_internal` feeds `AdapterInput::QuestionTicket { ticket: Some(t), .. }` (`claude/adapter.rs:1728-1736`, `claude/normalizer.rs:1025-1056`), which emits `AgentEvent::QuestionAsked`.
   - `CodexAdapter` follows the same strict order (`crates/atrium-agents/src/codex/adapter.rs:1633-1655`, `1835-1851`; `codex/normalizer.rs:648-678`).

2. **No stale visibility between `ledger.withdraw` and `open_count`, and no re-opening path:**
   - `Ledger::withdraw` (`crates/atrium-core/src/policy/ledger.rs:292-305`) awaits `Requests::transition` (`crates/atrium-core/src/store/requests.rs:213-223`) -> `Writer::req_transition` (`store/requests.rs:122-135`), which commits `UPDATE requests SET status = 'cancelled'` via `tx.commit()` before returning `Ok(Some("pending"))`.
   - `Requests::open_count` (`store/requests.rs:225-237`) starts a fresh SQLite read transaction on a pooled read-only connection in WAL mode after `tx.commit()` has returned, so it cannot still count the row that `ledger.withdraw(&key).await.unwrap()` just cancelled.
   - No code path transitions a cancelled/resolved row back to `'pending'` or `'approved'`, and in this test only `spawn_question` -> `open_question` inserts into `requests`.
   - Therefore, for `open_count` at line 2327 to read **1** immediately after `ledger.withdraw(&request_id).await.unwrap()`, a **different** row `t_other != request_id` must exist in `requests` with `status = 'pending'`.

3. **How a second `'pending'` row is created in `claude_delivered_question_blocked_withdrawals_stay_bounded_and_complete`:**
   - Unlike `CodexAdapter` (which has `GATE_TIMEOUT = 300s` for `question_ticket` and a separate `CodexOptions::withdraw_timeout` for `drain_withdrawals`), `ClaudeAdapter` used `ClaudeOptions::gate_deadline` for both `spawn_question` (`claude/adapter.rs:1558, 1570`) and `drain_withdrawals` (`claude/adapter.rs:1167`).
   - Because the test runs `MAX_PENDING_REQUESTS` (32) sequential `DeclineQuestion` -> `drain_withdrawals` cycles while `BlockedDeliveredWithdrawGate::withdraw` blocks every withdrawal, it passed `claude_opts(Duration::from_millis(150))` (line 2303) so `drain_withdrawals` would detach in 150 ms.
   - That also set `spawn_question`'s `timeout(deadline, res_rx)` to **150 ms** for all 32+1 questions, each performing 2 `spawn_blocking` SQLite reads (`check_incarnation`, `open_count`) plus 1 writer-thread SQLite `INSERT` + `tx.commit()` on a `worker_threads = 2` tokio runtime.
   - If any question `q_k` (`0 <= k < 31`) takes > 150 ms:
     1. `spawn_question`'s `timeout(150ms, res_rx)` expires, drops `res_rx`, and sends `Internal::Question { native: "q_k", ticket: None, failed: true }`.
     2. `Actor::on_internal` feeds `QuestionTicket { ticket: None }` (emitting no `QuestionAsked`) and calls `deny_can_use("q_k", "Atrium declined the question")`.
     3. `fake-claude-delivered-withdraw.py` (lines 2281-2284) treats any `behavior == "deny"` for `q_k` as the test's decline and immediately emits `q_{k+1}`.
     4. Meanwhile, `q_k`'s detached inner task finishes `gate.question_ticket(turn).await`, committing row `t_k` (`status = 'pending'`) in SQLite. Its `res_tx.send` fails because `res_rx` was dropped, so `OrphanGuard::drop` queues `(t_k, permit_k)` on `withdraw_tx`.
     5. `BlockedDeliveredWithdrawGate::withdraw` blocks `t_k`'s withdrawal (just like every other `gate.withdraw()` call until `release_withdraw`), leaving `t_k` `'pending'` in SQLite.
     6. `q_{k+1}` completes within 150 ms, commits `t_{k+1}`, and emits `AgentEvent::QuestionAsked { request_id: t_{k+1} }`.
     7. The test's `wait_for_question` (still on iteration `k`) receives `t_{k+1}`, calls `ledger.withdraw(t_{k+1}).await.unwrap()`, and asserts `open_count == 0` at line 2327 — reading **1** (the orphaned `t_k`).

4. **Real daemon impact:**
   - In the real daemon, `ClaudeOptions::default().gate_deadline` is 20 s and `SessionGate::withdraw` is not artificially blocked; if a `question_ticket` ever timed out after 20 s and committed late, `OrphanGuard::drop` runs `SessionGate::withdraw` unblocked and cancels the orphaned row.

### What is only consistent with the code

- The single historical failure on 2026-11-24 (M1-42 round 5 verification, load ~5) did not record per-question timings or a dump of the `requests` table, so we cannot directly observe from that run's logs that a `question_ticket` call exceeded 150 ms. That mechanism is the only code path that can produce `open_count == 1` at line 2327 and matches all observed facts (Claude-only, occurred once at load ~5, passed 30/30 on `dev` at load ~3).

## Fix

### Product seam (`crates/atrium-agents/src/claude/adapter.rs`)
- Added a private `withdraw_timeout: Option<Duration>` field on `ClaudeAdapter` and `Actor`, plus `#[doc(hidden)] pub fn with_withdraw_timeout(mut self, timeout: Duration) -> Self` on `ClaudeAdapter`.
- `Actor::drain_withdrawals` now uses `self.withdraw_timeout.unwrap_or(self.options.gate_deadline)`.
- `ClaudeOptions`, `ClaudeAdapter::new`, all public contract traits/types, and default production behaviour (`withdraw_timeout = None` -> `options.gate_deadline`) are unchanged; `examples/claude-record.rs` and all other callers compile without modification.
- Added unit test `claude::adapter::tests::withdraw_timeout_bounds_drain_without_shrinking_gate_deadline` pinning that `withdraw_timeout` detaches a blocked withdrawal without reducing `options.gate_deadline`.

### Tests (`crates/atrium-agents/tests/M1-28-followups.rs`)
- `claude_delivered_question_blocked_withdrawals_stay_bounded_and_complete`: kept `ClaudeOptions::default().gate_deadline` (20 s) and set `.with_withdraw_timeout(Duration::from_millis(20))`, matching its Codex twin (`CodexOptions { withdraw_timeout: Some(Duration::from_millis(20)), .. }`). No timeouts were raised, no assertions loosened, no retries added.
- Checked all sibling tests in `crates/atrium-agents/tests/M1-28-followups.rs`:
  - `codex_delivered_question_blocked_withdrawals_stay_bounded_and_complete`: already separates `withdraw_timeout` (20 ms) from `GATE_TIMEOUT` (300 s); no change needed.
  - `claude_hung_withdrawals_finish_within_one_deadline`: had the same 150 ms `gate_deadline` coupling on 5 real `SessionGate::check` calls; updated to `ClaudeOptions::default().gate_deadline` + `.with_withdraw_timeout(deadline)` (150 ms).
  - `claude_delayed_withdrawal_past_deadline_keeps_permit_and_cancels_orphan`: had the same 150 ms `gate_deadline` coupling on real `SessionGate::check` calls; updated to `ClaudeOptions::default().gate_deadline` + `.with_withdraw_timeout(wait_deadline)` (150 ms).

## Files Changed

- `crates/atrium-agents/src/claude/adapter.rs`
- `crates/atrium-agents/tests/M1-28-followups.rs`
- `docs/agents/handoffs/M1-44-n18-delivered-question-withdraw-flake.md`

## Verification Commands (for the orchestrator)

All commands below are fish-safe (loops and background load wrapped in `bash -c`).

1. **Format, clippy, workspace tests:**
   ```sh
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```

2. **`M1-28-followups` ×200 (0 failures):**
   ```sh
   bash -c 'set -euo pipefail; for i in $(seq 1 200); do cargo test -p atrium-agents --test M1-28-followups -- --quiet || { echo "failed on run $i"; exit 1; }; done; echo "200/200 passed"'
   ```

3. **`M1-28-followups` ×100 under CPU load (2× ncpu `yes`, 0 failures):**
   ```sh
   bash -c 'set -euo pipefail; n=$(sysctl -n hw.ncpu 2>/dev/null || nproc); pids=(); trap "kill ${pids[*]} 2>/dev/null || true" EXIT; for _ in $(seq 1 $((n * 2))); do yes >/dev/null & pids+=($!); done; for i in $(seq 1 100); do cargo test -p atrium-agents --test M1-28-followups -- --quiet || { echo "failed on run $i"; exit 1; }; done; echo "100/100 passed under load"'
   ```

4. **Revert check for the `claude/adapter.rs` product seam:**
   - Temporarily revert the `drain_withdrawals` line in `crates/atrium-agents/src/claude/adapter.rs` from `self.withdraw_timeout.unwrap_or(self.options.gate_deadline)` back to `self.options.gate_deadline`, then run:
     ```sh
     cargo test -p atrium-agents --lib withdraw_timeout_bounds_drain_without_shrinking_gate_deadline -- --exact
     ```
   - Expected: fails after 2 s with `drain_withdrawals did not honor withdraw_timeout` (waiting on the 60 s `gate_deadline` instead of the 20 ms `withdraw_timeout`). Restore the line afterwards.
