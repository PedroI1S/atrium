# M1-05b-daemon-test-gaps: handoff

Worker: Gemini 4 Hydrogen · Branch: `agent/hydrogen/M1-05b-daemon-test-gaps`

## Summary
Closed the testable gaps from `docs/agents/reviews/M1-05-daemon-late-fixes.md` (findings 1 and 4) and checked finding 5.

## Findings

### Finding 1 — Full-storage test (`crates/atrium-daemon/tests/gaps_full_storage.rs`)
- **Tests added:**
  - `full_storage_closes_open_message_from_store_not_refused_batch`
  - `full_storage_closes_open_tool_call_from_store_not_refused_batch`
- **What they test:**
  - Store a `MessageDelta` (opening a message) or `ToolCallStarted` (opening a tool call) before space runs out.
  - Trigger `StorageReadOnly` (`rig.free_bytes = 1024`), wait for `ThreadState::Interrupted { cleanup: Cleanup::Verified }` and `SessionCommand::Shutdown`.
  - Send a batch holding a further delta, `MessageCompleted` (or `ToolCallFinished { status: Succeeded }`), and `TurnCompleted` (refused by the full store), followed after 50 ms by `SessionEnded { exit_code: Some(0) }`.
  - Assert the daemon stores `MessageCompleted` (`Streamed { chars: 5, deltas: 1, .. }` for the pre-refusal delta only) or `ToolCallFinished { status: Cancelled }`, `TurnCompleted { outcome: TurnOutcome::Interrupted }`, and `SessionEnded { exit_code: None, reason: Some("storage is full") }`, with contiguous `seq` (`0..6`), and that the thread is cleanly closed and resumable once space returns.
- **Why each would fail on the code before `a27d432`:**
  - Before `a27d432`, `pump` set `closing_only = true` on `StoreError::ReadOnly`, dropped the refused batch (including `MessageCompleted` / `ToolCallFinished` and `TurnCompleted`), and then on the subsequent `SessionEnded` batch used `batch.retain(|e| is_closing(&e.event))` to keep and store the adapter's `SessionEnded { exit_code: Some(0), reason: None }`. Because `out.ended` became `Some(Some(0))`, `close_incarnation` never ran, leaving the message (or tool call) and turn open in the incarnation and `SessionEnded.reason` as `None` (instead of `"storage is full"`).

### Finding 4 — Writer partial-frame & `first` flag (`crates/atrium-daemon/src/conn.rs`), and snapshot ordering (`crates/atrium-daemon/src/upkeep.rs`)
- **Tests added in `crates/atrium-daemon/src/conn.rs` (`mod tests`):**
  - `write_loop_finishes_a_started_frame_and_drops_later_ones_when_subscription_ends`
    - Feeds a 4 MiB frame and a 1 KiB follow-up frame on a subscription, spawns `write_loop`, reads the first 1024 bytes on the peer (proving `written > 0` while the 4 MiB frame exceeds the `UnixStream` buffer and is only partially sent), then ends the subscription (`unsubscribe` and `subscribe` replacement in a loop) and calls `conn.finish()`. The peer reads the rest in 16 KiB chunks and asserts it receives the entire 4 MiB frame, never cut, and nothing of the second frame.
    - **Why it would fail on broken `first` wiring:** if `write_loop` passed `first = true` on subsequent `write_some` calls (instead of `written == 0`), ending the subscription mid-frame would cause `write_some` to return `Ok(None)` and truncate the frame.
  - `write_some_treats_first_as_true_only_before_any_byte_is_written`
    - Fills the `UnixStream` send buffer to force `WouldBlock` (`0` bytes written) on `write_some(..., first = true, ...)`: asserts `Ok(Some(0))` and that `sub.written` (`last_sent`) remains `None`; drains the filler, ends the subscription, and asserts a retry with `first = true` returns `Ok(None)` (frame dropped). Then on a second subscription, writes 40 of 100 bytes with `first = true`, asserts `sub.written == Some(EventId(2))`, ends the subscription, and writes the remaining 60 bytes with `first = false` (`Ok(Some(60))`), receiving all 100 bytes.
    - **Why it would fail on the old code (before `9891cc7`):** before `9891cc7`, `take()` set `sub.written` before any bytes were written (even if the socket hit `WouldBlock` and nothing was sent), and there was no `write_some` subscription check under the outbox lock.
- **Snapshot ordering (`crates/atrium-daemon/src/upkeep.rs`):**
  - **Not tested without timing tricks or daemon code changes.** `snapshot_daily` acquires `inner.publish.lock().await`, checks `inner.running_turns() > 0`, and calls `inner.store.snapshot(&dir).await` (`SqliteStore::snapshot` / `VACUUM INTO` in < 1 ms) with no hook or pause point inside `snapshot()`. Testing that a turn starting during a snapshot is either seen by `running_turns()` or stored after the snapshot would require either timing tricks (racing a turn start against `snapshot_check_every`) or adding a test seam (e.g., an optional `Store` wrapper or `on_snapshot` barrier hook in `Options`/`Inner` for tests).

### Finding 5 — Duplicate select arm (`crates/atrium-daemon/src/sessions.rs`)
- Checked `crates/atrium-daemon/src/sessions.rs` lines 632–641 (`first`-message `tokio::select!`): there is only a single `_ = stop.wait_for(|stop| *stop) => First::Stopped` arm on `HEAD`. No code change was needed.

## Definition of Done outputs

- `cargo fmt --all --check`: exited 0 (clean).
- `cargo clippy --workspace --all-targets -- -D warnings`: exited 0 (clean).
- `cargo test -p atrium-daemon --lib conn::tests --quiet` × 20 in a row: 20/20 passed (`11 passed; 0 failed` per run).
- `cargo test -p atrium-daemon --test gaps_full_storage --quiet` × 20 in a row: 20/20 passed (`2 passed; 0 failed` per run).
- `cargo test -p atrium-daemon`: exited 0:
  - `src/lib.rs` unit tests: 15 passed, 0 failed
  - `src/main.rs` unit tests: 0 passed, 0 failed
  - `tests/client.rs`: 1 passed, 0 failed
  - `tests/gaps_full_storage.rs`: 2 passed, 0 failed
  - `tests/recovery.rs`: 3 passed, 0 failed
  - `tests/sessions.rs`: 19 passed, 0 failed
  - `tests/socket.rs`: 9 passed, 0 failed
  - Doc-tests: 0 passed, 0 failed
