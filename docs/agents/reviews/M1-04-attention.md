# Review: M1-04-attention
Reviewer: Gemini 3.8 Flash (read-only) · 2026-11-09

## Verdict
**approve with changes** — The implementation of inbox ranking, notification policy, and live state transitions is clean, pure, and well-tested, but `Signal::Stopped` breaks the stickiness of `Failed` and `Interrupted` contrary to the contract and doc comment.

## Findings

1. [major] **`Signal::Stopped` breaks stickiness of `Failed` and `Interrupted`**
   - **Where**: `crates/atrium-core/src/attention/mod.rs:49-55`
   - **Problem**: `next_state` immediately returns `ThreadState::Ended` on `Signal::Stopped` before checking the stickiness of `Failed`, `Ended`, and `Interrupted`. The doc comment does not support this: bullet 1 explicitly specifies that `Signal::Starting` applies *"from any state (a new incarnation)"*, whereas bullet 3 states: *"Sticky until the next `Signal::Starting`: `Failed` (after a fatal `Error`), `Ended` and `Interrupted`"*. The doc comment names only `Signal::Starting` as the exit condition for those states. In addition, transitioning `Interrupted { cleanup: Unverified }` to `Ended` on `Signal::Stopped` violates contract §10 (*"When identity can't be verified, the thread becomes `Interrupted { cleanup: Unverified }` and says so; it is never shown as stopped"*), and transitioning `Failed` to `Ended` silently discards the error message on the thread state without starting a new incarnation.
   - **Fix**: Move the stickiness check for `Failed`, `Ended`, and `Interrupted` above `Signal::Stopped`, so only `Signal::Starting` escapes them. Update `crates/atrium-core/tests/attention.rs`: change the expected transition string for `Signal::Stopped` in the table test from `"EEEEEEEEEE"` to `"EEEEE====="`, and adjust `user_stop_ends_as_ended_whatever_the_exit_code` to assert that terminal states remain sticky.

2. [minor] **Unreachable dead match arm for `Signal::Starting | Signal::Stopped`**
   - **Where**: `crates/atrium-core/src/attention/mod.rs:72`
   - **Problem**: Lines 49-55 already return early when `input` is `AttentionInput::Signal(Signal::Starting | Signal::Stopped)`. Consequently, the match arm `AttentionInput::Signal(Signal::Starting | Signal::Stopped) => current.clone()` at line 72 is dead code that can never execute.
   - **Fix**: Remove line 72 from the `match input` expression.

3. [nit] **`AgentEvent::Usage` omitted from table test**
   - **Where**: `crates/atrium-core/tests/attention.rs:308-372`
   - **Problem**: The Definition of Done requires the transition test to cover every state with every input. `AgentEvent::Usage` is missing from the `neutral` list in `tests/attention.rs` (acknowledged in the handoff).
   - **Fix**: Add `AgentEvent::Usage` with a dummy `Usage` struct to the `neutral` list in `table()`.
