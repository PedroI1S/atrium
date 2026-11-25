# M1-44 review round 1 — Claude Sonnet 5.5 (agy, read-only, D-169)

Branch at `d9f9fa08`. Orchestrator's verification: fmt/clippy clean; workspace 933/0/7; `claude_` tests in M1-28-followups ×200 quiet and ×100 under load (up to 54): 0 failures.

## Verdict

**Request changes** (low-severity items only). I found no product bug. The cause analysis fits the code, the seam leaves production behaviour unchanged, and no assertion was loosened. The brief's suspected ordering bug is ruled out. Under D-120 every finding blocks approval, though. The main one is that nothing deterministically pins the late-ticket path. That path is both the stated cause and the product behaviour the fix leans on, and it is currently covered only by an absence of failures. The rest are small gaps in coverage, documentation and API shape.

I only read files and ran grep and `git diff`. I could not run tests, so the pass counts are the orchestrator's.

## Findings

**1. Medium: the late-ticket path has no deterministic test.**
- **Where:** `spawn_question` (`crates/atrium-agents/src/claude/adapter.rs:1558-1620`) and the tests in `M1-28-followups.rs`.
- **What I checked:**
  - Ordering is not a bug. `question_ticket` (`gate.rs:1770`) awaits `Ledger::open`, which awaits `store.open_question` (`ledger.rs:101`). Only then does the adapter send `Internal::Question`, and `on_internal` (`adapter.rs:1742-1748`) feeds `QuestionTicket`, which emits `QuestionAsked`. So `QuestionAsked` follows the commit.
  - `Ledger::withdraw` (`ledger.rs:292-305`) awaits the commit of `UPDATE … 'cancelled'`.
  - So `open_count == 1` right after a withdraw needs a second pending row. The only code that can create one is a ticket abandoned by `timeout(deadline, res_rx)` that later commits. The mechanism in the handoff matches the code.
- **Is the late row always cleaned up in production?** Yes. When the timeout fires, `res_rx` is dropped. The inner task has already called `guard.set_ticket(t)`, so `let _ = res_tx.send((t, guard))` fails and drops the guard. `OrphanGuard::drop` (`adapter.rs:458-476`) then queues `(ticket, permit)` on `withdraw_tx`. The actor loop drains that channel at the top of every iteration, through `Ev::Withdraw` and `drain_withdrawals`.
  - If `withdraw_rx` is closed or gone, `drop` falls back to a direct `handle.spawn(gate.withdraw)`.
  - If the value was sent into the oneshot after the timeout polled but before the receiver dropped, the receiver's drop frees it, which also drops the guard.
  - The permit and in-flight slot are held until the withdrawal finishes. With the default 20 s `gate_deadline` there is no orphan leak, so nothing needs a product fix.
- **Gap:** no test exercises this. The existing tests pass `Duration::from_millis(150)` or `from_secs(5)` and never make `question_ticket` late. The handoff says "consistent with the code", and the single historical failure has no trace. Nothing in the diff reproduces the old failure, and nothing pins the cleanup.
- **Failing case:** someone later changes `OrphanGuard` or the `res_tx.send` handling so that a late `Some(ticket)` is dropped without being queued. Every test stays green, because production no longer hits the 150 ms path, and an open row leaks.
- **Fix:** add a unit test in `claude::adapter::tests` with no sleeps.
  - Use a gate whose `question_ticket` waits on a `Notify` or watch and then returns `Some(ApprovalTicket::new("L1", false))`.
  - Use a small `gate_deadline` such as 20 ms.
  - Feed an `AskUserQuestion` and wait for the deny on the provider pipe.
  - Assert there is no `QuestionAsked`.
  - Release the gate and assert `withdraw` is called once for `L1` with the permit held until it finishes.
  - The same test shows the old test setup did produce an orphan, which confirms the cause.

**2. Low: the seam adds a second field to carry the same value.**
- **Where:** `adapter.rs:135`, `adapter.rs:299` and the new `Actor::withdraw_timeout` (line 577), which `Actor::new` sets to `None` (line 664).
- **Failing case:** a future second construction site of `Actor` silently ignores the override, so `withdraw_timeout` is not applied.
- **Which shape is better:** `ClaudeOptions::withdraw_timeout: Option<Duration>` is better. It matches `CodexOptions` (`codex/adapter.rs:65`, used at line 2121), it needs no `Adapter`→`Actor` plumbing because `options` is already cloned into the actor, and it adds no hidden public method.
- **Why the hidden method is still acceptable here:** `ClaudeOptions` has public fields and is not `#[non_exhaustive]`. Adding a field is a public-type change, which the brief's "no public signature changed" rule can be read to forbid. The `#[doc(hidden)]` method is the conservative reading and stays within the owned paths. I'd keep it for this task and record the options-field alignment as a follow-up or change request.
- **Fix:** either record that follow-up in the handoff, or move the override into `options` and drop the duplicate fields.

**3. Low: shrinking `gate_deadline` also affected other code, and the handoff doesn't say so.**
- **Where:** `adapter.rs:1152` (`wait_for_in_flight_gates` cap = `gate_deadline.min(500 ms)`) and `adapter.rs:814` (hook timeout = `gate_deadline` + 10 s).
- **Effect:** in the three changed tests the shutdown wait cap goes from 150 ms to 500 ms, and the hook timeout from 10 s to 30 s.
- **Impact:**
  - `claude_hung_withdrawals_finish_within_one_deadline` (line 478) asserts `< 450 ms` on shutdown. In that test the five checks have already returned `Ask`, so `gate_in_flight` should be 0 and the wait is not entered. Any check still in flight would add up to 500 ms against the 450 ms bound.
  - So "does not wait much longer" depends on in-flight being 0 at shutdown. The handoff didn't call this out.
- **Fix:** add one sentence to the handoff, and ideally assert `ledger.gate.asked().len() == 5` before shutdown, which line 519 already does.

**4. Low: the seam's unit test doesn't pin the default or the propagation.**
- **Where:** `withdraw_timeout_bounds_drain_without_shrinking_gate_deadline` (`adapter.rs:2453`).
- **Gaps:**
  - It sets `a.withdraw_timeout` directly on the actor, so it never tests the `start` plumbing at `adapter.rs:299`. Only the integration tests cover that, indirectly.
  - It never tests that `None` falls back to `gate_deadline`, which is the "production unchanged" claim. The expression `unwrap_or(self.options.gate_deadline)` is correct by inspection, but nothing asserts it.
- **Failing case:** someone changes the fallback to a constant, or drops line 299. The unit test still passes.
- **Fix:** add a second case with `withdraw_timeout = None` and `gate_deadline = 20 ms`. It should detach at about 20 ms and stay bounded.

**5. Low: no comment says why `gate_deadline` must stay at its default in these tests.**
- **Where:** the three changed call sites in `M1-28-followups.rs` (lines 503, 1700, 2305).
- **Failing case:** a later cleanup "simplifies" `claude_opts(ClaudeOptions::default().gate_deadline)` back to `claude_opts(Duration::from_millis(150))`. The flake returns and `open_count == 1` is again reported with no hint of the cause.
- **Fix:** add a short comment at those sites, for example "keep `gate_deadline` large; it also bounds real SQLite tickets and checks".
- **Related:** N18 in `m1-must-fix.md` still says "routed to M1-44, not started". It should be closed after merge.

**6. Info: the cause remains inferred, and the load runs have no baseline.**
- The handoff is honest that the cause is "consistent with the code".
- The ×200 and ×100 runs under load, with 0 failures, are the evidence for the fix. We don't know how often the old code failed under the same load, so the runs don't show the old failure rate.
- **Fix:** if a cheap baseline is wanted, run the old test with `gate_deadline` at 150 ms under the same hogs. Finding 1's test covers this deterministically.

## Test gaps

- **Late `question_ticket`:** no test for a ticket arriving after the deadline (finding 1). It should check that there is no `QuestionAsked`, that a deny goes out, that the late row is withdrawn once, and that the permit is held until the withdrawal finishes.
- **No override:** nothing tests the no-override case of `drain_withdrawals` (finding 4).
- **Propagation:** nothing tests that `start` carries the override to the actor, except indirectly (finding 4).
- **Tests keep what they pin:** I found no weakening.
  - Test 1 (`claude_hung_withdrawals_finish_within_one_deadline`): the same `< 450 ms` and `withdrawn().len() == 5` assertions, and the 150 ms drain bound is unchanged.
  - Test 2 (`claude_delayed_withdrawal_past_deadline_keeps_permit_and_cancels_orphan`): the 150 ms drain bound is unchanged. The `>= 120 ms` elapsed assertion, the permit retention and the `open_count == 1` assertions are unchanged.
  - Test 3 (`claude_delivered_question_blocked_withdrawals_stay_bounded_and_complete`): the drain bound drops from 150 ms to 20 ms, which matches the Codex sibling. The assertions are unchanged: `withdraw_started == total`, `withdraw_finished == 0`, then all complete, and `question_calls == total`. The shorter bound can't make this test pass trivially, because every withdrawal is still blocked on the gate and has to complete after release.
  - Test 3 also gets faster: 32 cycles of 20 ms instead of 150 ms.
- **Codex sibling:** untouched, and it doesn't need a change. It already separates `withdraw_timeout` (20 ms) from `GATE_TIMEOUT` (300 s).
- **Owned paths:** the diff touches only `claude/adapter.rs`, `M1-28-followups.rs` and the handoff. There is no `atrium-core` change (empty `git diff --stat -- crates/atrium-core`) and no protocol change.
- **Not verified:** whether `Ledger::open_question` emits any notification or UI-visible row between commit and withdraw for a late, never-announced question. I didn't read that far, and it matters only if a late row can show in the UI.

