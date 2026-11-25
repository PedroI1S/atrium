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
     cargo test -p atrium-agents --lib with_withdraw_timeout_propagates_through_start -- --exact
     ```
   - Expected: both fail after their timeout (waiting on the 60 s `gate_deadline` instead of the 20 ms `withdraw_timeout`). Restore the line afterwards.

## Round 2

Addressed every finding and the open point from Sonnet's round 1 review (`.orchestration/review-input/M1-44-n18-delivered-question-withdraw-flake-round1.md`):

1. **Finding 1 (Medium) & Finding 6 (Info) — deterministic late-ticket test & proof of cause:**
   - Added `claude::adapter::tests::late_question_ticket_denies_without_event_and_withdraws_orphan` in `crates/atrium-agents/src/claude/adapter.rs` with zero sleeps (`tokio::sync::watch` barriers + `Semaphore::acquire_many`).
   - Uses a gate whose `question_ticket` blocks on a watch channel and then returns `Some(ApprovalTicket::new("L1", false))`, with `gate_deadline = 20 ms`.
   - Feeds `AskUserQuestion`, waits for the `control_response` deny on the provider pipe, asserts no `AgentEvent::QuestionAsked` is emitted, asserts both `gate_in_flight == 1` and `gate_permits` remain held after the timeout, releases the gate, and asserts `gate.withdraw` is called exactly once for `"L1"` while the permit remains held until `withdraw` completes and is released afterwards.
   - This deterministically pins the `OrphanGuard::drop` cleanup path that the fix relies on and proves that a slow `question_ticket` past `gate_deadline` produces a late ticket that must be withdrawn (explaining how the old 150 ms `gate_deadline` setup left a pending row when `BlockedDeliveredWithdrawGate` blocked withdrawals).

2. **Finding 2 (Low) — single `Actor` construction site & Change Request:**
   - Kept `#[doc(hidden)] ClaudeAdapter::with_withdraw_timeout` so `ClaudeOptions` (a public struct without `#[non_exhaustive]`) is not modified, respecting the brief's no-public-signature-change rule.
   - Changed `Actor::new` (`crates/atrium-agents/src/claude/adapter.rs`) — the single construction site for `Actor` — to accept `withdraw_timeout: Option<Duration>` directly as a parameter instead of post-construction field mutation, and documented this invariant on `Actor::new`. No caller can silently omit or lose the override.
   - **Change request:** The next time `ClaudeOptions` is allowed to change (contract / public API bump), move `withdraw_timeout: Option<Duration>` into `ClaudeOptions` (matching `CodexOptions::withdraw_timeout` in `crates/atrium-agents/src/codex/adapter.rs:65`) and remove `ClaudeAdapter::withdraw_timeout` and `ClaudeAdapter::with_withdraw_timeout`.

3. **Finding 3 (Low) — other effects of keeping `gate_deadline` at 20 s:**
   - Restoring `gate_deadline` from 150 ms to 20 s in the three `M1-28-followups.rs` tests also restores:
     - `wait_for_in_flight_gates`'s shutdown wait cap (`crates/atrium-agents/src/claude/adapter.rs:1152`, `gate_deadline.min(500 ms)`) from 150 ms to 500 ms;
     - the `PreToolUse` hook timeout (`crates/atrium-agents/src/claude/adapter.rs:814`, `gate_deadline + 10 s`) from 10 s to 30 s.
   - In `claude_hung_withdrawals_finish_within_one_deadline`, the `< 450 ms` shutdown bound remains safe because `wait_for_approvals(&mut session, 5).await` and `assert_eq!(ledger.gate.asked().len(), 5)` (`crates/atrium-agents/tests/M1-28-followups.rs:521-522`) already run before `shutdown(session).await` (line 525), ensuring all 5 gate checks have completed and `gate_in_flight == 0` when `wait_for_in_flight_gates` runs.

4. **Finding 4 (Low) — fallback & propagation unit tests:**
   - Extended `claude::adapter::tests::withdraw_timeout_bounds_drain_without_shrinking_gate_deadline` with a second case (`withdraw_timeout = None`, `gate_deadline = 20 ms`) asserting that `drain_withdrawals` falls back to `gate_deadline`, detaches at ~20 ms (`>= 15 ms` and `< 1 s`), and stays bounded.
   - Added `claude::adapter::tests::with_withdraw_timeout_propagates_through_start`, which calls `ClaudeAdapter::new(...).with_withdraw_timeout(20 ms).start(...)` through the public `AgentAdapter::start` path against a 60 s `gate_deadline` and verifies that a blocked withdrawal detaches promptly rather than blocking shutdown for 60 s.

5. **Finding 5 (Low) — comments at the three `M1-28-followups.rs` call sites:**
   - Added an explanatory comment at all three changed `ClaudeAdapter::new(claude_opts(ClaudeOptions::default().gate_deadline))` sites (`claude_hung_withdrawals_finish_within_one_deadline`, `claude_delayed_withdrawal_past_deadline_keeps_permit_and_cancels_orphan`, and `claude_delivered_question_blocked_withdrawals_stay_bounded_and_complete`) explaining that `gate_deadline` stays at its 20 s default because it also bounds real SQLite checks and tickets.
   - Left `docs/agents/m1-must-fix.md` untouched (orchestrator closes N18 at merge).

6. **Reviewer's open point — can a late, never-announced question reach the UI between `open_question` and `withdraw`?**
   - **No.** Checked the full path from `Ledger::open_question` to SQLite reads:
     - `Ledger::open_question` (`crates/atrium-core/src/policy/ledger.rs:134-142`) delegates to `Ledger::open` (`ledger.rs:70-109`), which calls `self.store.open_question(&key, turn)` (`ledger.rs:101`) and emits no events or notifications.
     - `Requests::open_question` (`crates/atrium-core/src/store/requests.rs:207-211`) -> `Writer::open_row` (`store/requests.rs:42-89`) inserts a `requests` row with `status = 'pending'` and leaves `event_opened` at its schema default `0` (`crates/atrium-core/schema/0001_init.sql:113-114`).
     - `event_opened = 1` and `cx.row.pq += 1` are set only when `AgentEvent::QuestionAsked` is recorded (`crates/atrium-core/src/store/writer.rs:885-906`).
     - Every UI/attention/recovery query on `requests` filters on `event_opened = 1 AND event_closed = 0`: `refresh_attention` (`crates/atrium-core/src/store/writer.rs:157-159`) and `open_items` (`crates/atrium-core/src/store/writer.rs:651-652`).
     - Only `Requests::open_count` (`crates/atrium-core/src/store/requests.rs:225-237`) counts `event_opened = 0` rows, solely for `MAX_PENDING_REQUESTS` capacity accounting in `Ledger::open`. Because a timed-out `question_ticket` never emits `AgentEvent::QuestionAsked`, it cannot surface in the UI or notifications before `OrphanGuard::drop` withdraws it.

7. **Checks run in Round 2 (D-123):**
   - `cargo fmt --all` and `cargo check -p atrium-agents --tests --examples` clean with 0 warnings. No test suites were run by the worker.
   - Additional targeted unit tests for the orchestrator to verify:
     ```sh
     cargo test -p atrium-agents --lib late_question_ticket_denies_without_event_and_withdraws_orphan -- --exact
     cargo test -p atrium-agents --lib withdraw_timeout_bounds_drain_without_shrinking_gate_deadline -- --exact
     cargo test -p atrium-agents --lib with_withdraw_timeout_propagates_through_start -- --exact
     ```
