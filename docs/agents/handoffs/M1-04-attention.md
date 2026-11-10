# Handoff: M1-04-attention

Model: Sonnet 5.5 · Status: done

## Done
- `attention::next_state`, `rank_inbox`, `should_notify` implemented in `crates/atrium-core/src/attention/mod.rs` (pure, no public signature changed).
- All 8 tests in `crates/atrium-core/tests/attention.rs` un-ignored and passing. The transition test is a table of 33 inputs (after review fixes) (every `AgentEvent` variant except the cases listed below, plus every `Signal`) x 10 states (Starting, Running, NeedsApproval, NeedsInput, Idle, Paused, Failed, Interrupted Verified, Interrupted Unverified, Ended), 320 cells; mismatches are collected and reported at once, nothing is printed on success.
- `rank_inbox`: reason from state, else `Reply` when unread; sorted by reason, then `attention_since_ms` (falls back to `updated_at_ms` if `None`), then thread id for determinism; storage-full item first ("Storage is full", no thread).
- `should_notify`: only on a change of state kind into NeedsApproval, NeedsInput or Failed; never focused; at least `NOTIFY_MIN_INTERVAL` since the last (a clock that went backwards doesn't notify).

## Not done
Nothing. No change requests.

## Definition of Done results
- `cargo test -p atrium-core --test attention`: 8 passed, 0 ignored.
- `cargo test -p atrium-core --lib`: ok (0 tests).
- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.

## Transition table
Rows: input. Columns: state before. `=` unchanged. Pending is the count after the input (for inputs that don't change requests, what the starting state implies: 1 approval in NeedsApproval, 1 question in NeedsInput).

| Input | Starting | Running | NeedsApproval | NeedsInput | Idle | Paused | Failed | Interrupted(V) | Interrupted(U) | Ended |
|---|---|---|---|---|---|---|---|---|---|---|
| Signal::Starting | Starting | Starting | Starting | Starting | Starting | Starting | Starting | Starting | Starting | Starting |
| Signal::Stopped | Ended | Ended | Ended | Ended | Ended | Ended | = | = | = | = |
| Signal::Exited(Some(0)) | Ended | Ended | Ended | Ended | Ended | = | = | = | = | = |
| Signal::Exited(Some(1)) / (None) | Int(V) | Int(V) | Int(V) | Int(V) | Int(V) | = | = | = | = | = |
| Signal::Recovered(Unverified) | Int(U) | Int(U) | Int(U) | Int(U) | Int(U) | Int(U) | = | = | = | = |
| Signal::Recovered(Verified) / StorageReadOnly | Int(V) | Int(V) | Int(V) | Int(V) | Int(V) | Int(V) | = | = | = | = |
| SessionStarted | Idle | = | = | = | = | = | = | = | = | = |
| TurnStarted | Running | Running | NeedsApproval | NeedsInput | Running | Running | = | = | = | = |
| TurnCompleted | Starting | Idle | NeedsApproval | NeedsInput | Idle | = | = | = | = | = |
| ApprovalRequested (pending 1,0) | NeedsApproval | NeedsApproval | NeedsApproval | NeedsApproval | NeedsApproval | NeedsApproval | = | = | = | = |
| QuestionAsked (0,1) | NeedsInput | NeedsInput | NeedsInput | NeedsInput | NeedsInput | NeedsInput | = | = | = | = |
| QuestionAsked (1,1) | NeedsApproval | NeedsApproval | NeedsApproval | NeedsApproval | NeedsApproval | NeedsApproval | = | = | = | = |
| RequestResolved (0,0) | Starting | Running | Running | Running | Idle | = | = | = | = | = |
| RequestResolved (0,1) | NeedsInput | NeedsInput | NeedsInput | NeedsInput | NeedsInput | NeedsInput | = | = | = | = |
| RateLimited | Paused | Paused | Paused | Paused | Paused | Paused | = | = | = | = |
| Error fatal | Failed | Failed | Failed | Failed | Failed | Failed | = | = | = | = |
| Error non-fatal, EnforcementChanged, MessageDelta/Completed, ToolCall*, PlanUpdated, ContextUpdated, QuotaUpdated, Unmapped | = | = | = | = | = | = | = | = | = | = |
| SessionEnded(Some(0)) | Ended | Ended | Ended | Ended | Ended | = | = | = | = | = |
| SessionEnded(Some(n != 0) / None) | Int(V) | Int(V) | Int(V) | Int(V) | Int(V) | = | = | = | = | = |

(`Usage` is in the table test in the state-neutral group.)

## Gotchas
- `Signal::Stopped` is `Ended` from every non-sticky state (Paused included); `Failed`, `Interrupted` and `Ended` are only left by `Signal::Starting` (see Review fixes).
- `NeedsApproval` and `NeedsInput` count as "a turn is open", so clearing them returns to `Running`. A request opened while `Idle` and then cleared returns to `Idle`, not `Running`.
- `Starting` and `Paused` are kept by events that don't change anything; `SessionStarted` moves `Starting` to `Idle`; `TurnStarted` leaves `Paused`.
- A fatal `Error` arriving while `Paused` or while requests are pending becomes `Failed`.
- `Recovered`/`StorageReadOnly` on a `Failed`, `Ended` or already-`Interrupted` thread keep the current state (sticky), so an Unverified cleanup is never overwritten by a later Verified one.
- Reply reason: any thread with `unread` and no stronger reason (including `Ended`, `Idle`, `Running`) appears as `Reply`.

## Follow-ups
- M1-02 must pass the pending counts after applying the event, and set `attention_since_ms` itself.

## Review fixes
1. `Signal::Stopped` no longer leaves `Failed`, `Ended` or `Interrupted` (stickiness is checked first; only `Signal::Starting` escapes). It is still `Ended` from Starting, Running, NeedsApproval, NeedsInput, Idle and Paused. Table row and `user_stop_ends_as_ended_whatever_the_exit_code` updated, with an explicit assertion that a stop on `Interrupted { Unverified }` keeps it.
2. The dead `Signal::Starting | Signal::Stopped => current` arm is gone; the match arms now map those two signals directly.
3. `AgentEvent::Usage` added to the neutral inputs of the table test (33 inputs x 10 states).

Results after the fixes: `cargo fmt --all --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test -p atrium-core --test attention` 8 passed; `--lib` ok.
