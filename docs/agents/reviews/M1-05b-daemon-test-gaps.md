# Review: M1-05b-daemon-test-gaps (commit 36208e1 on 8677018)

Reviewer: Claude Sonnet 5.5, read-only (D-075). Author: Gemini 4 Hydrogen `medium`. Experiments ran in a scratch clone with its own target folder.

## Verdict: approve with changes

The tests are real, and each catches the regression it targets, shown by mutation. The changes are minor: some reasoning in the handoff is imprecise, and the snapshot "needs a seam" claim is too pessimistic. Nothing blocks the merge.

## 1. Scope: OK
Three files, all owned:
- `crates/atrium-daemon/tests/gaps_full_storage.rs` (new);
- test code inside the existing `#[cfg(test)] mod tests` of `crates/atrium-daemon/src/conn.rs` (+122, after line 748);
- the handoff.

No production code changed.

## 2. Finding 1 (full storage): OK, mutation confirmed

| Test | Stored events asserted |
|---|---|
| `full_storage_closes_open_message_from_store_not_refused_batch` | `MessageCompleted` (`Streamed`, chars 5, deltas 1, sha256 of "hello"), `TurnCompleted { Interrupted }`, `SessionEnded { exit_code: None, reason: Some("storage is full") }` |
| `full_storage_closes_open_tool_call_from_store_not_refused_batch` | `ToolCallFinished { Cancelled, output: None }`, `TurnCompleted { Interrupted }`, the same `SessionEnded` |

- **Seq:** asserted contiguous in both tests.
- **Inbox:** the `StorageFull` item is checked, and so is its clearing.
- **"Nothing open":** shown indirectly, by the closers and by a resume that works once space returns. It is not asserted directly.

**Mutation:** `git show a27d432 -- crates/atrium-daemon/src/sessions.rs | git apply -R` applies cleanly, and both tests then fail.
- Message test (`gaps_full_storage.rs:89`): got `[session_started, turn_started, message_delta, session_ended]`.
- Tool test (`:208`): got `[…, tool_call_started, session_ended]`.

The old code stored the adapter's own `SessionEnded` and never ran `close_incarnation`. The handoff's explanation matches what the reviewer saw.

## 3. Finding 4 (writer): OK, mutation confirmed
**`write_loop_finishes_a_started_frame_and_drops_later_ones_when_subscription_ends` (conn.rs:753)**
- The frame is partial by construction, not by timing: a 4 MiB frame against the UnixStream buffer, with 1024 bytes read first.
- It covers both `unsubscribe` and replacement.

**`write_some_treats_first_as_true_only_before_any_byte_is_written` (conn.rs:802)**
- A meaningful test. It checks a `WouldBlock` with no bytes written, then 40 of 100 bytes written, then the remainder finished after an unsubscribe.

**Mutations:**

| Mutation | Result |
|---|---|
| Revert `a27d432`'s `sessions.rs` change | both full-storage tests FAIL |
| `write_loop` passes `first = true` always | `write_loop_finishes_a_started_frame…` FAILS (conn.rs:796) |
| `write_some` sets `written` without `written > 0` | `write_some_treats_first_as_true…` FAILS (conn.rs:831) |
| `write_some` subscription check unconditional | both new conn tests FAIL |

**Determinism:** `conn::tests` and `gaps_full_storage` each ran 20 times with 0 failures. `gaps_full_storage` takes about 4 s per run.

- **[nit] conn.rs:796-798, the assertion "nothing of that subscription after it":** it holds, but because `unsubscribe`/`subscribe` and `finish()` purge queued events (conn.rs:213-216), not because of `write_some`'s check. It would pass with no origin check at all. The taken-but-unwritten case is covered by the second test and by `a_frame_the_writer_holds_is_not_sent_once_its_subscription_ended`. Reword the comment, or end the variants by closing the peer instead of calling `finish()`.
- **[nit] conn.rs:775:** add `assert!(received.len() < large.len())` before ending the subscription, so the "frame still incomplete" precondition is explicit.

## 4. Finding 5: confirmed absent
`src/sessions.rs:640` is the only `stop.wait_for` arm in the first-message `select!` (lines 631-642). `git log -G'stop.wait_for'` shows the duplicate went away in round 2 (946e8f7). Not changing the code was correct.

## 5. Snapshot ordering
- **[minor] The handoff's claim is imprecise.** `Inner.store` is `Arc<dyn Store>` (lib.rs:180) and `Store::snapshot` is a trait method (atrium-core lib.rs:214). A delegating store whose `snapshot()` waits on a barrier would test it without timing tricks.
- **What is missing is injection:** `Daemon::start` builds the `SqliteStore` itself (lib.rs:293-295). A `#[cfg(test)]`-only hook would be enough, e.g. `Options.store_wrapper: Option<Box<dyn Fn(Arc<dyn Store>) -> Arc<dyn Store>>>`.
- **The test it would allow:** hold the snapshot on the barrier, start a turn, release, and assert that either the turn was not visible to the snapshot or its `TurnStarted` was stored after the release.
- **Next step:** this is production code, so it was rightly left out of M1-05b. A small follow-up task can add the hook and the test.

## 6. Toolchain (scratch copy of 36208e1)
- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test -p atrium-daemon`: all pass. Counts: lib 15, client 1, gaps_full_storage 2, recovery 3, sessions 19, socket 9.

## Other nits
- The handoff's "why it would fail" for the `first` test (9891cc7, `take()` setting `written` early) was verified by mutation only, not by checking out the old commit.
- The handoff doesn't mention the mutation checks.
