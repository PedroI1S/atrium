# Review: M1-05-daemon, the four late fixes
Reviewer: Claude Sonnet 5.5 (read-only, D-060) · 2026-11-12

## Verdict
**approve**: all four changes are correct as far as I could trace them (no lost or duplicated event, no stuck incarnation, no deadlock or panic found); the findings are weak tests and one latency cost, none blocking.

`cargo test -p atrium-daemon` **passed on this Mac (macOS, Darwin)**: 13 unit, 1 client, 3 recovery, 19 sessions, 9 socket, 0 failed. `cargo clippy -p atrium-daemon --all-targets -- -D warnings`: clean.

## Changes reviewed
1. `9891cc7`, conn.rs: the writer checks under the outbox lock, before a frame's first bytes, that its subscription still exists (`write_some`); `last_sent` is set only once bytes are written (D-058, first half of old finding 5).
2. `9891cc7`, upkeep.rs: the daily snapshot is made with `Inner::publish` held, and "no turn running" is checked under it (old finding 11).
3. `a27d432`, sessions.rs: after a batch is refused for full storage the pump stores nothing more, waits for the session, and `close_incarnation` closes from `open_items` (Flash finding 2).
4. `a27d432`, sessions.rs: a first message lost to a session that ended by itself is logged (Flash finding 3).

## Findings

1. [minor] **The full-storage test does not distinguish the new rule from the old one**
   Where: crates/atrium-daemon/tests/sessions.rs:584-634.
   Problem: the refused batch is a lone delta. The session's later `turn_done` and `ended` then arrive. The test asserts the kinds `session_started, turn_started, turn_completed, session_ended` and contiguous `seq`. The code removed in `a27d432` (keep the closing events, renumber them) gives the same kinds and the same `seq`, so the test passes with either. The case Flash described (a refused batch that held the closing events of an open message, tool call or request, so a later `SessionEnded` closed the incarnation with that item still open) is not exercised. The test also does not check the `TurnCompleted` outcome (the daemon writes `Interrupted`; the adapter's would be `Completed`), nor `SessionEnded.reason`.
   Fix: before the space runs out, store a `MessageStarted`/delta (or a `ToolCallStarted`). Then send one batch holding more deltas plus the `MessageCompleted` and `turn_done` (or just send the closers after the refusal) and `ended(Some(0))`. Assert the daemon's `MessageCompleted` (`Streamed`), `TurnCompleted { Interrupted }` and `SessionEnded { reason: "storage is full" }`.

2. [minor] **After a refusal the adapter's own end is discarded, so its exit code and the turn's usage are lost**
   Where: crates/atrium-daemon/src/sessions.rs:507-512 (`closing_only` empties the batch, `out.ended` is not set) and :724-766 (`Signal::Exited { code: None }`, `kill_recorded` runs).
   Problem: the session ends cleanly with `exit_code: Some(0)` and a `TurnCompleted` carrying `usage`. Neither is stored. The incarnation ends with `exit_code: None` and the thread gets `Exited { code: None }`. This is a consequence of the chosen design and is not wrong, since the history stays closed and in order. It is also not mentioned in the handoff.
   Fix: nothing needed. Note it in the handoff, or in a later round let the pump keep the exit code from the dropped `SessionEnded` and pass it to the closing `SessionEnded` and the signal.

3. [minor] **The daily snapshot holds `publish` for the whole `VACUUM INTO`**
   Where: crates/atrium-daemon/src/upkeep.rs:181-188.
   Problem: confirmed by code, not measured. Every `store_batch` waits for the lock, so all sessions' appends stall while the snapshot runs, and so do subscription go-live (server.rs:406) and any batch that ends a request (it holds `Live::requests` while it waits, so `ResolveApproval` waits too). The snapshot itself already reads a consistent WAL snapshot on a reader connection (snapshot.rs:77-96), so the lock buys only the "turn that starts now" ordering. The handoff's store (839 MiB) makes the VACUUM seconds long, not milliseconds. If `ShutdownDaemon` arrives meanwhile, the sessions' closing events wait for it inside the 17 s stop patience. There is no deadlock: `publish` is always taken first, and the reads take a pool permit with no await on `publish` while holding it.
   Fix: acceptable as is for M1 (once a day, only when no turn is running). If it shows in the perf run, check `running_turns()` under the lock and run the VACUUM after releasing it, or stop the snapshot when the daemon stops. The reasoning in the comment ("a turn that starts now is seen here or stored after the snapshot") holds, because `pump` sets the turn flag before `store_batch` (sessions.rs:489-494).

4. [minor] **Neither the writer's wiring nor the partial-frame case is tested; the snapshot ordering and the log line are untested**
   Where: conn.rs test `a_frame_the_writer_holds_is_not_sent_once_its_subscription_ended`; upkeep.rs; sessions.rs:679-688.
   Problem: the conn test calls `write_some` directly with `first = true`. It checks the rule for the unsubscribe and re-subscribe cases (event 1 only reaches the peer), which is good. It does not go through `write_loop`, so `first = (written == 0)` and the "keep writing the rest of a frame already started even if its subscription then ends" case (which must not truncate a frame) have no test. I traced both and they are correct: `written` is 0 only before any bytes, and a `WouldBlock` retry re-checks the subscription. The snapshot change has no new test (the existing one only covers the old path), and the log line for a lost first message has none either; the handoff says the race is not testable.
   Fix: optional. A `write_loop` test with a peer that reads slowly would cover the partial frame. The first-message log needs a session that ends without taking its first message; the existing `a_session_that_takes_no_commands_can_still_be_stopped` setup nearly does that.

5. [nit] **Duplicate select arm**
   Where: crates/atrium-daemon/src/sessions.rs, in the spawned first-message task: two identical `_ = stop.wait_for(|stop| *stop) => First::Stopped` arms.
   Problem: harmless; the second can never be the one chosen differently.
   Fix: delete one.

Traced and found sound (no finding):
- Change 1: a frame already taken whose subscription ended is dropped; a started frame is finished, so the stream is never cut mid-frame. A lag notice computed after a take but before the first write names the previous event, and the held frame is then dropped because the lagged subscription is gone, so `last_sent` matches what was sent. The mutex is held only across a non-blocking `try_write`.
- Change 3: a refused batch holding `SessionEnded` leaves `out.ended = None` and breaks the pump, so the daemon closes from `open_items` (the closing events are accepted while read-only). Later batches are drained and dropped, so the adapter is not backpressured. `live.stop()` plus the 15 s deadline bound the wait. Ledger entries for requests never stored are expired by the stored `SessionEnded`.
- Change 4: with `ended` set, the pump still logs the loss only; the history already ends in `SessionEnded`, so nothing can be added.
