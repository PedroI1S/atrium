# M1-04-attention: thread state machine, inbox ranking, notification policy

Model: Sonnet 5.5 · Reviewer: Gemini 3.8 Flash · Depends on: contract (M1 rev 2)
Branch: `agent/sonnet/M1-04-attention` · Worktree: `../atrium-wt/M1-04-attention`

## Goal
Implement the pure functions that decide a thread's state, rank the inbox and decide when to notify. Everything that shows the user "this needs you" depends on them.

## Context
- `docs/contracts/m1.md` §8. PLAN §3.4.
- `crates/atrium-core/src/attention/mod.rs` (signatures and the precedence rules in the doc comments), `crates/atrium-core/src/lib.rs` (`Signal`).
- `crates/atrium-protocol/src/thread.rs` (`ThreadState`, `Cleanup`, `ThreadSummary` with `attention_since_ms`/`attention_detail`, `InboxReason` order, `InboxItem`), `crates/atrium-protocol/src/agent.rs` (events and lifecycles).

## Owned paths
- `crates/atrium-core/src/attention/**`
- `crates/atrium-core/tests/attention.rs`
- `docs/agents/handoffs/M1-04-attention.md`

## Contract to implement
- `attention::next_state(current, pending, input)`: every `ThreadState` × every relevant `AgentEvent` and `Signal`. `pending` is the count after the input. `NeedsApproval` beats `NeedsInput`; both return to `Running` when cleared during a turn. `Failed` and `Paused` are sticky through the closing events and exit; after `Signal::Stopped` the exit is `Ended`; an exit with a turn open or a non-zero code is `Interrupted { cleanup: Verified }`; `Signal::Recovered { cleanup }` is `Interrupted { cleanup }`.
- `attention::rank_inbox(threads, storage_full_since_ms)`: one item per thread that needs the user, reason from its state and `unread`, ranked by `InboxReason` order then `attention_since_ms` (oldest first), detail from `attention_detail`. A storage-full item first when given.
- `attention::should_notify(previous, next, focused, last_notified_ms, now_ms)`: only for transitions into `NeedsApproval`, `NeedsInput` and `Failed`; never when focused; at most once per `limits::NOTIFY_MIN_INTERVAL` per thread.

## Definition of Done
- [ ] Every test in `crates/atrium-core/tests/attention.rs` un-ignored and passing; the transition test is a table covering every state with every input, and prints nothing unexpected.
- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test -p atrium-core --test attention` and `--lib`
- [ ] Handoff with outputs and the transition table as a Markdown table.

## Out of scope
Storing anything (M1-02 calls `next_state` on every append), posting notifications (the daemon, M1-05), UI.

## Escalate if
A transition can't be decided from the inputs the signature gives: change request, and stop that part. Two fix rounds without passing.
