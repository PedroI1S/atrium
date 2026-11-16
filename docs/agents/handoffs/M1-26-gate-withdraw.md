# M1-26-gate-withdraw handoff

Task: withdrawn requests free their ledger slot (P2, D-111, CR-18).
Branch: `agent/hydrogen/M1-26-gate-withdraw` (not pushed, merged, or rebased).
`docs/agents/m1-must-fix.md` (P2) left untouched per the brief.

## 1. API

- **`crates/atrium-agents/src/lib.rs` (`ToolGate::withdraw`)**
  ```rust
  fn withdraw<'a>(&'a self, _ticket: &'a ApprovalTicket) -> BoxFuture<'a, ()> {
      Box::pin(async {})
  }
  ```
  Note: the brief named `crates/atrium-core/src/lib.rs` for `ToolGate`, while the trait actually lives in `crates/atrium-agents/src/lib.rs` (and `atrium-agents` does not depend on `atrium-core`). A default no-op body keeps existing test/probe `ToolGate` mocks compiling without editing unowned files (`crates/atrium-daemon/src/commands.rs`, `examples/*`, `atrium-core` adversarial tests).

- **`crates/atrium-core/src/policy/gate.rs` (`SessionGate::withdraw` and `impl ToolGate for SessionGate`)**
  - `pub async fn withdraw(&self, ticket: &ApprovalTicket)` added at the end of `impl SessionGate`:
    1. Builds `RequestKey { thread, incarnation, request_id: ticket.request_id.clone() }`.
    2. Removes `ticket.request_id` from `self.opened` so `redeem` can never succeed.
    3. Calls `let _ = self.ledger.withdraw(&key).await;`, transitioning `pending | approved -> cancelled` (which frees its `MAX_PENDING_REQUESTS` slot in `requests` and keeps final states `redeemed | denied | answered | cancelled | expired` or unknown keys as idempotent no-ops without returning an error).
  - `impl ToolGate for SessionGate` delegates `withdraw` to `SessionGate::withdraw`.

## 2. Where each adapter calls `withdraw`

Both adapters queue withdrawn tickets onto `pending_withdrawals: Vec<ApprovalTicket>` and drain them asynchronously with `gate.withdraw(&ticket).await` inside a panic- and timeout-bounded `guarded` task before flushing pending events and before `SessionEnded` (`flush_final` / loop exit):

- **Claude (`crates/atrium-agents/src/claude/adapter.rs`)**
  - `forget_native` (`control_cancel_request`): withdraws matching entries from `approvals`, `redeeming` (in-flight `spawn_redeem` approvals), and `questions`.
  - `on_can_use`: if `take_hook_ticket(&key)` misses for a `tool_use_id` (e.g. `hook-ask-changed` or oversize `can_use_tool`), withdraws any earlier `hook_asked` tickets for that `tool_use_id`.
  - `on_user`: when a `tool_result` arrives for `tool_use_id`, withdraws any unmatched `hook_asked` tickets for that `tool_use_id`.
  - `clear_requests` (`result` / turn end, and `finish` / session end): drains and withdraws all remaining `hook_asked`, `approvals`, `redeeming`, and `questions`.
  - `on_internal` / `on_gate_decision`: withdraws any `GateDecision::Ask { ticket }` or `Some(question_ticket)` whose request already left `live` or whose `proc` is `None`, and any `Internal::Redeem` whose request is no longer live or did not return `GateDecision::Allow`.
  - `on_command` (`DeclineQuestion`): queues `withdraw` for the declined question ticket (idempotent with the daemon's `ledger.withdraw` and fixes standalone adapter runs).
  - `finish` and `run` exit: calls `drain_internal_withdrawals()` on `internal_rx` and `drain_withdrawals().await` before flushing closing events.

- **Codex (`crates/atrium-agents/src/codex/adapter.rs`)**
  - `PendingQuestion` now carries `ticket: Option<ApprovalTicket>`.
  - `forget_native` (`serverRequest/resolved`): withdraws matching entries from `approvals` (including `redeeming:{native}`) and `questions`.
  - `clear_requests` (`turn/completed`, `discard`, and `Driver::run` exit): drains and withdraws all remaining `approvals` and `questions`.
  - `on_internal`: withdraws any `GateDecision::Ask { ticket }` or `Some(question_ticket)` when `!live`, `meta.is_none()`, or declined for `PENDING_LIMIT` saturation, and any `Internal::Redeem` that is no longer live, missing metadata, or not `GateDecision::Allow`.
  - `cancel_overflow_request`: withdraws any ticket removed due to `PENDING_LIMIT` saturation.
  - `on_command` (`DeclineQuestion`): withdraws `pending.ticket` if present.
  - `Driver::run` exit: calls `clear_requests()`, `drain_internal_withdrawals()`, and `drain_withdrawals().await` before `flush_final().await`, and `guarded` now aborts its inner spawned task on drop.

Note on `RequestResolved { cancelled }`: when `ApprovalRequested` or `QuestionAsked` was already emitted, the normalizer still emits `AgentEvent::RequestResolved { outcome: RequestOutcome::Cancelled }` (which the daemon stores and feeds to `ledger.withdraw`, idempotently). When `ApprovalRequested`/`QuestionAsked` was never emitted (e.g. an unmatched hook `Ask` or pre-emission cancellation/saturation), `RequestResolved` is not emitted (per `consistency.rs` and contract §5), and `ToolGate::withdraw` frees the ledger slot directly.

## 3. Contract wording (`docs/contracts/m1.md`)

Added the **Withdraw (D-111, CR-18)** bullet in `docs/contracts/m1.md` §5 (`Approvals (G3, D-046)`), where `ToolGate::question_ticket`, `ToolGate::redeem`, and ledger `withdraw` are specified (CR-18's brief text referenced §3/§7, while approval lifecycle and `ToolGate` live in §5):

> - **Withdraw (D-111, CR-18).** `ToolGate::withdraw(ticket)` (implemented by `SessionGate::withdraw`) resolves an open approval or question ticket as `cancelled`, removes it from the gate's open-ticket set so it can never be redeemed, and frees its `MAX_PENDING_REQUESTS` slot; it is idempotent (resolved, redeemed, or unknown tickets are a no-op and never fail the turn). Adapters call it whenever a request is withdrawn (provider cancellation, turn end, interrupt/shutdown, saturation decline, or an `Ask` never followed by `can_use_tool`), while emitted `ApprovalRequested`/`QuestionAsked` requests still emit `RequestResolved { outcome: cancelled }` as above.

## 4. Verification & Definition of Done outputs

### Core test failing on `dev` (before implementing `withdraw`)

Command: `cargo test -p atrium-core --test gate_withdraw` (run on `dev` baseline before editing `gate.rs`/`lib.rs`):

```text
error[E0599]: no method named `withdraw` found for struct `Arc<dyn ToolGate>` in the current scope
   --> crates/atrium-core/tests/gate_withdraw.rs:141:10
...
error[E0599]: no method named `withdraw` found for struct `Arc<SessionGate>` in the current scope
   --> crates/atrium-core/tests/gate_withdraw.rs:181:10
...
error: could not compile `atrium-core` (test "gate_withdraw") due to 10 previous errors
```

### Targeted new tests (after final edits)

- `cargo test -p atrium-core --test gate_withdraw`: **3 passed; 0 failed** (`withdrawn_tickets_free_slots_against_max_pending_requests`, `withdrawn_ticket_cannot_be_redeemed_and_never_becomes_approval`, `withdraw_is_idempotent_across_resolved_redeemed_and_unknown_tickets`).
- `cargo test -p atrium-agents --test gate_withdraw`: **3 passed; 0 failed** (`claude_adapter_withdraws_cancelled_and_turn_ended_requests`, `claude_adapter_withdraws_unmatched_hook_ask_when_input_changes`, `codex_adapter_withdraws_interrupted_approval_and_question_and_saturation`).
- `cargo test -p atrium-agents --lib`: **36 passed; 0 failed** (includes new unit test `claude::adapter::tests::withdrawing_hook_ask_and_redeeming_tickets_queues_gate_withdraw`).

### `git diff dev -- crates/atrium-core/src/policy/gate.rs`

```diff
diff --git a/crates/atrium-core/src/policy/gate.rs b/crates/atrium-core/src/policy/gate.rs
index 049c3153..753241e6 100644
--- a/crates/atrium-core/src/policy/gate.rs
+++ b/crates/atrium-core/src/policy/gate.rs
@@ -1346,6 +1346,21 @@ impl SessionGate {
         }
         Ok(GateDecision::Allow)
     }
+
+    /// Withdraws an open ticket as cancelled, freeing its ledger slot and
+    /// preventing future redemption. Idempotent: resolved, redeemed, or
+    /// unknown tickets are a no-op and never fail the turn.
+    pub async fn withdraw(&self, ticket: &ApprovalTicket) {
+        let key = RequestKey {
+            thread: self.config.thread.clone(),
+            incarnation: self.config.incarnation,
+            request_id: ticket.request_id.clone(),
+        };
+        if let Ok(mut m) = self.opened.lock() {
+            m.remove(&ticket.request_id);
+        }
+        let _ = self.ledger.withdraw(&key).await;
+    }
 }
 
 impl ToolGate for SessionGate {
@@ -1377,4 +1392,8 @@ impl ToolGate for SessionGate {
                 .ok()
         })
     }
+
+    fn withdraw<'a>(&'a self, ticket: &'a ApprovalTicket) -> BoxFuture<'a, ()> {
+        Box::pin(self.withdraw(ticket))
+    }
 }
```

### Workspace Definition of Done commands

- `cargo fmt --all --check`: exited `0` (clean).
- `cargo clippy --workspace --all-targets -- -D warnings`: exited `0` (`Finished dev profile ... in 2m 10s`, 0 warnings).
- `cargo test --workspace --no-fail-fast`: exited `0` (all workspace unit, integration, and doc tests passed; only the 7 pre-existing `#[ignore]`d M1-13 tests on `dev` were ignored).
