# M1-44 review round 3 — Claude Sonnet 5.5 (agy, read-only, D-169)

Branch at `2da08730`. Orchestrator's verification: fmt/clippy clean; workspace 935/0/7; `claude_` tests in M1-28-followups ×200 quiet and ×100 under load: 0 failures.

## Verdict

**Request changes.** The round-2 finding is closed. I found no product bug, no loosened assertion and no change outside `atrium-agents`. The remaining findings are Low or Info, but D-120 and the brief's Definition of Done ("approves with no findings") count every finding.

- **Paused-clock test:** `withdraw_timeout_bounds_drain_without_shrinking_gate_deadline` is fixed and does not pass trivially.
- **Handoff:** the `open_count` wording is corrected.
- **Cause and seam:** the cause holds up against the code, and the default production path is unchanged.
- **Still open:**
  - `Cargo.toml` is outside the brief's owned paths.
  - Two tests in the diff still have wall-clock bounds a stall can break.
  - Some handoff line references are stale.

I read the code and the diff. I did not run anything, and I rely on your verification numbers for runtime behaviour.

## Findings

**1. Low – `crates/atrium-agents/Cargo.toml:27` is outside the brief's owned paths.**
- The brief owns `M1-28-followups.rs`, `crates/atrium-agents/src/**`, the core ledger/gate files and the handoff. `Cargo.toml` is not on that list.
- The handoff lists it under "Files Changed" as "dev-dependencies only" but never marks it as a deviation or asks for sign-off.
- `tokio/test-util` was not enabled anywhere else in the workspace (grep over all `Cargo.toml` files). `start_paused` cannot work without it.
- The change is benign, assuming resolver 2 (default for edition 2021+): it only affects test builds, and `Cargo.lock` is unchanged.
- **Fix:** record it in the handoff as a deviation, and have the orchestrator explicitly accept it at merge. Alternatively drop the dependency and give up the paused clock, which is worse.

**2. Low – `claude_hung_withdrawals_finish_within_one_deadline` still has a wall-clock upper bound a stall can break** (`M1-28-followups.rs:529-532`).
- The bound is `elapsed < 450 ms`, measured over a real `shutdown(session)`: process stop, reap, and a 150 ms drain.
- Failing case: a 300 ms scheduler stall during child stop or reap on a loaded macOS host. Elapsed becomes about 500 ms and the test fails, even though one deadline was respected.
- The ×100 run under 2×ncpu load passing shows the risk is small, not zero.
- The ratio it pins is 1 deadline versus 5 deadlines, 150 ms versus 750 ms. The 300 ms margin is narrow.
- A deterministic version is not possible here: the child is a real process, so the clock cannot be paused.
- **Fix:** scale the pair so the ratio is kept but the margin grows. For example use `with_withdraw_timeout(1 s)` with a bound `< 3 s`; the sequential bug would take 5 s. That is not a loosening, since the 1-versus-5 distinction stays.

**3. Low – `with_withdraw_timeout_propagates_through_start` has wall-clock failure bounds that are too tight** (`claude/adapter.rs:2700`, `2718`, `2728`).
- The bounds are 5 s for `QuestionAsked`, 2 s for `withdraw_started`, and 5 s for `SessionEnded`.
- The first covers a real Python spawn plus `claude --version`. `VERSION_TIMEOUT` is itself 5 s (`adapter.rs:93`), and the sibling helper `wait_for_question` allows 15 s for the same wait.
- Failing case: a stall over 5 s during startup under heavy load. The test fails spuriously on a wait that has nothing to do with propagation.
- The discriminating condition is a 60 s `gate_deadline` versus 20 ms, so the bounds can be widened for free.
- **Fix:** raise `gate_deadline` to 600 s and all three bounds to 30–60 s. A real propagation failure still fails, and a pass costs about 20 ms either way.

**4. Info – the seam shape: prefer `ClaudeOptions::withdraw_timeout`, as in Codex.**
- `CodexOptions::withdraw_timeout: Option<Duration>` (`codex/adapter.rs:65`) is the existing pattern. It needs one field, one read in `drain_withdrawals`, and no `Actor::new` signature change.
- The hidden-builder route adds a field on `ClaudeAdapter`, a field on `Actor`, a new `Actor::new` parameter, and a `#[doc(hidden)] pub fn` that any dependent crate such as the daemon can still call. Integration tests need `pub`, so `cfg(test)` is not an option.
- Every in-repo `ClaudeOptions { .. }` literal I checked uses `..ClaudeOptions::default()` or `..opts()`, so a new field would not break them. The one literal in `examples/claude-record.rs` was only partly visible in the grep context, but the handoff says it compiles unchanged.
- Within the brief ("no public signature change"), a hidden builder is defensible, because adding a field to a non-`#[non_exhaustive]` public struct is arguably an API change. Keeping it is acceptable.
- **Action:** make sure the change request in handoff Round 2 item 2 is logged somewhere that outlives the handoff, such as `m1-must-fix.md`, at merge.
- Default behaviour is unchanged: `self.withdraw_timeout.unwrap_or(self.options.gate_deadline)` at `adapter.rs:1187-1188`. The old line was `self.options.gate_deadline`. `ClaudeAdapter::new` and `Default` give `None`, and the daemon uses `ClaudeOptions::default()`.

**5. Info – the handoff has stale and mixed line references.**
- Handoff line 28 cites `claude/adapter.rs:1558, 1570` and `1167`. Those are `dev` line numbers. On HEAD, `1558` is a `}` in `take_hook_ticket` and `1167` is inside `wait_for_in_flight_gates`. The correct HEAD lines are `1579` and `1591` (`deadline` and `timeout(deadline, res_rx)`) and `1187-1188`.
- Other citations are HEAD-based, for example `1564-1590` and `1152`.
- Round 2 item 4 (handoff lines 119-121) still describes the superseded wall-clock fallback test ("≥ 15 ms and < 1 s"). Round 3 replaced it, and nothing marks the old text as superseded.
- **Fix:** use HEAD line numbers throughout and mark Round 2 item 4 as superseded by Round 3.

**6. Info – the N18 text in `m1-must-fix.md` will go stale.**
- It cites "line 2327"; the assert is now at `M1-28-followups.rs:2339`.
- The orchestrator closes N18 at merge. Update the line reference or close it by test name rather than line number.

### What I checked and found sound

**Paused-clock test.**
- The test is `start_paused` with no wall-clock bound. After the first poll, nothing awaits except `advance`, so there is no auto-advance. Auto-advance needs an idle runtime, and `yield_now` keeps the runtime busy.
- Detaching is judged by a single non-blocking poll through a biased `select!` against `ready(())`. If the override were ignored, the poll at 11 s would still be pending and the test would panic.
- **Override case:**
  - `gate_deadline` is 73 s and the override is 11 s.
  - The drain is pending at 10 s with `started == 1`.
  - It detaches at 11 s, and `elapsed == 11 s` exactly.
- **Fallback case:**
  - Here `withdraw_timeout = None`.
  - The drain is pending at 72 s. That is past 11 s, so an implementation that used the override would already have detached and would panic.
  - It detaches at 73 s, with `elapsed == 73 s` exactly.
- The detached withdrawals are released with `release_tx` and `release_fb_tx`, so nothing leaks.

**The test sets `a.withdraw_timeout` directly.** The plumbing from `with_withdraw_timeout` through `start` to `Actor::new` is covered only by the propagation test, which is wall-clock (finding 3). The builder state itself is asserted at lines 2496-2497.

**`late_question_ticket_denies_without_event_and_withdraws_orphan`.**
- It uses `start_paused`, and every timeout in it is a failure bound on the mock clock.
- Auto-advance orders timers by deadline: the 20 ms gate deadline fires before the 500 ms `ALIVE_POLL` tick and the 2 s bounds. A passing run also shows the actor does not wait on blocking-pool work during shutdown.

**1. The cause.**
- `Ledger::open` commits the row before `question_ticket` returns. The chain is `open_question` → `Shared::call` → `Writer::open_row`, which runs `tx.commit()` on the writer thread (`store/requests.rs:88`) before the oneshot reply (`store/mod.rs:109-119`).
- The normalizer emits `QuestionAsked` only when `question_ticket` gets `Some` (`normalizer.rs:1037-1049`). The adapter feeds that only after `res_rx` receives the ticket (`adapter.rs:1591-1611`).
- `withdraw` is a CAS UPDATE with `tx.commit()` before it returns (`requests.rs:122-135`). `open_count` is a fresh read after that, so it cannot see a stale row.
- So `open_count == 1` right after a successful withdraw needs a *different* pending row. The only code that can make one is the late inner task, once `timeout(150 ms, res_rx)` has expired (`adapter.rs:1585-1624`). A blocked withdraw (`BlockedDeliveredWithdrawGate`) then leaves that row pending. The suspected product ordering bug is ruled out.
- The handoff correctly says the historical failure is only consistent with this mechanism, since no timings were captured.

**2. The late-ticket path in production.**
- After the deadline, `Internal::Question{ticket: None}` produces a deny with no `QuestionAsked` (`adapter.rs:1749-1758`).
- The inner task commits late. `res_tx.send` fails, and `OrphanGuard::drop` queues `(ticket, permit)` on `withdraw_tx` (`adapter.rs:458-475`).
- The actor reads that through `Ev::Withdraw` (`adapter.rs:1037`) and `drain_withdrawals` at the top of the loop (`adapter.rs:1017`).
- If the actor has already shut down, `close()` and a final drain run first (`adapter.rs:1101-1104`, `1139-1141`). After that, `send` fails and the direct `Handle::spawn` fallback withdraws. If the outer task is aborted, the result is dropped with `res_rx`, which drops the guard and also withdraws.
- With the real 20 s deadline and an unblocked `SessionGate::withdraw`, the row is cancelled. The permit and in-flight slot are held until then. The row has `event_opened = 0`, so it is invisible to the UI, as the handoff says.
- No orphaned open row path. No product fix is needed.

**4. The three Claude tests still pin the same properties.**
- `hung_withdrawals`: the drain bound is still 150 ms through the override. If the override were ignored, shutdown would wait 20 s and fail the 450 ms bound.
- `delayed_withdrawal`: the 150 ms bound is kept. The `>= 120 ms` check is a lower bound, so a stall cannot break it. Its fillers and `delayed_0` are now released only by the cancel files, not raced by a 150 ms timeout, which is more deterministic.
- `delivered_question`: 20 ms matches the Codex twin. 32 sequential declines cost at most about 0.64 s, down from about 4.8 s.
- No assertions were loosened, no retries added, and no timeouts raised.
- The 20 s `gate_deadline` also affects two other things, as the handoff notes: the `wait_for_in_flight_gates` cap (150 ms to 500 ms) and the `PreToolUse` hook timeout (10 s to 30 s). In `hung_withdrawals`, shutdown only pays the 500 ms if an in-flight check is stuck, because the wait is notify-driven. I take the handoff's argument that all five checks have settled by then as correct.

**5. Owned paths.** The diff is four files. Apart from `Cargo.toml` (finding 1), the rest are in `atrium-agents` or the handoff. Nothing in `atrium-core`: `git diff dev...HEAD --stat -- crates/atrium-core` is empty.

## Test gaps

1. **No integration test reproduces N18's mechanism with the real ledger.**
   - The mechanism is proven in pieces: a late ticket yields a deny with no event and an orphan withdraw (adapter unit test), and an orphan row blocks `open_count` (code reading).
   - A deterministic test would wrap the real `SessionGate` so `question_ticket` delays past a small `gate_deadline`. It would assert that `open_count` returns to 0 after the orphan withdraw and that `QuestionAsked` is never emitted. That pins the production invariant end to end.
2. **`delivered_question` asserts `open_count == 0` with no message** (`M1-28-followups.rs:2339`). If a stray row ever returns, for example after a late ticket past 20 s, the failure shows only `left: 1`. Add a message with the iteration and `gate.question_calls()`.
3. **The `with_withdraw_timeout` → `start` → `Actor::new` plumbing has no deterministic test.** It relies on the propagation test, which has the bounds in finding 3. Moving the override into `ClaudeOptions` (finding 4) would remove this plumbing and the need for that test.
4. **`OrphanGuard::drop`'s direct-spawn fallback** (send fails after the actor ends) **is not exercised by any test in this diff.** I assume an existing M1-33 test covers it, but I did not check.

