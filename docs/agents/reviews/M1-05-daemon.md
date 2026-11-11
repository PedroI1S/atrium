# Review: M1-05-daemon
Reviewer: Gemini 3.8 Flash (read-only) · 2026-11-11

## Verdict
**approve with changes**: the architecture, socket protocol, two-lane writer, replay boundaries, and ledger integration are solid and verified, but mixed-batch read-only handling can leave open items unclosed, user-stop/read-only signals mask unverified process cleanup, and contract escalations CR-09/CR-10 require driver resolution.

---

## Findings

1. [major] **Unverified process cleanup is masked if the thread was stopped by the user or interrupted by full storage**
   - Where: `crates/atrium-daemon/src/sessions.rs:723-728` (`run`), `crates/atrium-daemon/src/commands.rs:489` (`thread_command`), `crates/atrium-daemon/src/upkeep.rs:131` (`watch_storage`), interacting with `crates/atrium-core/src/attention/mod.rs:54-60` (`next_state`).
   - Sequence 1:
     1. Start a native session on thread $T$ with recorded process identity $P$.
     2. Client sends `Command(r, ThreadCommand { thread_id: T, command: Shutdown })`.
     3. `thread_command` commits `Signal::Stopped` to the store, which transitions the thread to `ThreadState::Ended` (a sticky state).
     4. The adapter's event stream terminates without emitting `SessionEnded`, leaving child processes running in $P$'s process group.
     5. In `run`, `supervise::kill_recorded(&P)` returns `Cleanup::Unverified` (the group leader exited, but descendants remain and cannot safely be killed).
     6. `run` stores `Signal::Recovered { cleanup: Unverified }` at line 726.
     7. `attention::next_state` evaluates the signal, but lines 55–60 return `current.clone()` because `Ended` is sticky.
     8. Thread $T$ remains `Ended` (stopped).
   - Sequence 2:
     1. Native thread $T$ is actively running turns (`Running`).
     2. Storage falls below `MIN_FREE_DISK_BYTES`.
     3. `watch_storage` signals `Signal::StorageReadOnly` (`upkeep.rs:131`), transitioning $T$ to sticky `ThreadState::Interrupted { cleanup: Cleanup::Verified }`.
     4. The session stream terminates without `SessionEnded`, and `kill_recorded` returns `Cleanup::Unverified`.
     5. `run` stores `Signal::Recovered { cleanup: Unverified }`.
     6. `attention::next_state` retains `Interrupted { cleanup: Verified }`.
   - Result: In Sequence 1, the thread is displayed to the user as stopped (`Ended`), violating Contract §10: *"When identity can't be verified, the thread becomes Interrupted { cleanup: Unverified } and says so; it is never shown as stopped."* In Sequence 2, it is falsely reported as having verified cleanup.
   - Fix: Requires deciding CR-09 to permit `Signal::Recovered { cleanup: Unverified }` to escalate any existing state in `crates/atrium-core/src/attention/mod.rs`. In addition, the daemon should avoid committing premature sticky terminal states before the process lifecycle check completes.

2. [major] **Read-only storage rejection drops closing events inside a mixed batch, committing an incomplete session**
   - Where: `crates/atrium-daemon/src/sessions.rs:520-565` (`pump`).
   - Sequence:
     1. Native thread $T$ stores sequence 0 (`SessionStarted`), sequence 1 (`TurnStarted { turn: 1 }`), and sequence 2 (`MessageDelta { message_id: "m1", ... }`). In SQLite, message `"m1"` and turn 1 are registered in `open_items`.
     2. Storage space falls below `MIN_FREE_DISK_BYTES` (storage becomes read-only).
     3. The adapter sends a batch containing both non-closing and closing events:
        - `seq 3`: non-closing `MessageDelta { message_id: "m1", text: "more" }`
        - `seq 4`: closing `MessageCompleted { message_id: "m1", ... }`
     4. `pump` calls `store_batch(thread, incarnation, batch, Some(live))` at line 530.
     5. In `writer.rs:420`, because not all events are closing, `self.writable()?` fails with `StoreError::ReadOnly`.
     6. `store_batch` fails. `pump` enters `Err(StoreError::ReadOnly)` (line 548), sets `closing_only = true`, sends `Signal::StorageReadOnly`, and signals `live.stop()`. The entire batch (including `seq 4` `MessageCompleted`) is dropped.
     7. In response to `live.stop()`, the adapter completes its shutdown and emits:
        - `seq 5`: `TurnCompleted { turn: 1, outcome: Interrupted, ... }`
        - `seq 6`: `SessionEnded { exit_code: Some(0), ... }`
     8. `pump` receives this batch (line 488). Because `closing_only` is true, `batch.retain` keeps both events and renumbers them starting from `next_seq = 3`: `TurnCompleted` becomes `seq 3`, `SessionEnded` becomes `seq 4`.
     9. `store_batch` succeeds because both events are closing. `pump` records `out.ended = Some(Some(0))` (line 538) and breaks.
     10. In `run` (line 705), because `pumped.ended` is `Some`, `close_incarnation` is never called.
   - Result: In SQLite, `open_messages` still holds `"m1"`. `Store::open_items(T, 1)` permanently reports `"m1"` as open. Any subsequent `ExportThread` on thread $T$ fails or produces corrupted transcripts because `"m1"` has deltas but no `MessageCompleted`. Stored history has committed `SessionEnded` while open items remain unclosed, violating Contract §6.1, §7, and §10.
   - Fix: When a batch fails with `StoreError::ReadOnly`, filter that batch for any closing events (`is_closing`), renumber them, and store them immediately; or, upon transitioning to `closing_only`, ignore subsequent adapter output and let `close_incarnation` synthesize closing events from `Store::open_items`.

3. [minor] **Lost initial user message is silently dropped if the adapter exits cleanly before receiving it**
   - Where: `crates/atrium-daemon/src/sessions.rs:698-702` (`run`).
   - Sequence:
     1. Client sends `StartThread { agent: Native, message: Some("process prompt") }`.
     2. Native adapter process starts, but its command receiver is unpolled or stalled.
     3. The background task `first` waits up to `COMMAND_PATIENCE` (5 seconds) to send the message.
     4. The adapter immediately emits `SessionStarted` followed by `SessionEnded { exit_code: Some(0) }`.
     5. `pump` processes and stores `SessionEnded`, returning `pumped.ended = Some(Some(0))`.
     6. `first.abort()` is called at line 682, and `first_message_lost` evaluates to `true`.
     7. At line 698, `if first_message_lost && pumped.ended.is_none()` evaluates to `false` because `pumped.ended` is `Some`.
     8. The thread terminates with `Signal::Exited { code: Some(0) }` (ThreadState `Ended`).
   - Result: The user's initial message was accepted by `StartThread` but was never dispatched to the agent, never stored in the transcript, and no error was recorded. The thread terminates cleanly as `Ended` without notifying the user that their initial prompt was dropped.
   - Fix: If `first_message_lost` is true, record the failure (e.g. log an explicit warning or mark the thread attention detail) even if `pumped.ended` is `Some`.

4. [minor] **Archived unfinished incarnations without a recorded process cannot be discovered during recovery**
   - Where: `crates/atrium-daemon/src/upkeep.rs:60` (`recover`).
   - Sequence:
     1. A session without a recorded process (`AgentLaunch::Fake`, or a native session whose process was not recorded or already exited) is running turns on thread $T$.
     2. Client issues `ArchiveThread { thread_id: T }`. Thread $T$ is marked `archived = 1`.
     3. Daemon crashes (e.g., `SIGKILL` or power loss) before the session pump stores closing events.
     4. On reboot, `upkeep::recover` runs.
     5. Pass 1 checks `inner.store.live_processes()`, which does not contain thread $T$.
     6. Pass 2 iterates `inner.store.threads()`, which explicitly queries `WHERE t.archived = 0`, omitting thread $T$.
   - Result: Thread $T$'s unfinished incarnation is skipped during recovery. Its open turns and messages remain open in the store, violating Contract §5 and §10.
   - Fix: Requires CR-10 to expand the `Store` trait with a mechanism to query all unfinished incarnations regardless of thread archive status.

---

## Suspected, not shown

- **Stale summary race on concurrent SubscribeInbox and refresh:** `SubscribeInbox` refreshes streaming sessions prior to answering (`server.rs:270–274`), then acquires the `Hub` mutex to read the current summary list. A concurrent store write occurring between `refresh_thread` and `subscribe_inbox` could momentarily serve a summary older than the store's latest commit, but subsequent `ThreadUpdated` broadcasts immediately rectify the view. No sequence was found that produces permanent inconsistency.
- **Export directory permission revocation:** If directory permissions are altered while `history::export_thread` is executing, `std::fs::remove_file` at `commands.rs:748` could fail to delete the partial file. This depends on external OS filesystem intervention and represents standard best-effort cleanup.

---

## Tried and held

- **Replay/live boundary interleaving:** An append and its live broadcast execute under `Inner::publish`; `feed` checks stored events and transitions to `go_live` under that same lock; no boundary event is dropped or duplicated.
- **ReplayDone sequence boundary:** A page containing events both below/at and above `replay_until` queues `ReplayDone` immediately prior to the first event exceeding `replay_until`; empty pages queue it after all stored events.
- **Old feeder room wake-up:** When a subscription is replaced or unsubscribed while its feeder waits for lane space, `room.notify_waiters()` wakes the feeder, which observes `subs.contains_key(&sub) == false` and exits immediately.
- **Concurrent Live registration:** `Hub::go_live` retains only the newest `(Conn, SubId)` per thread, preventing stale subscriptions from receiving pushed events.
- **Event lane byte accounting:** Enqueueing, dequeueing, retention cleanup, and connection finish/close adjust `event_bytes` and `control_bytes` under the outbox mutex without underflow, drift, or unbounded growth.
- **Oversized lane item delivery:** When `event_bytes == 0`, `has_room` accepts one event exceeding `max_event_bytes`, allowing both store replays and live oversized events to be written.
- **SubscriptionLagged last_sent precision:** `take()` records `sub.written = Some(id)` as the event frame is popped, and writes control frames before any subsequent event frame, ensuring `last_sent` matches the client's actual received stream.
- **Control lane priority:** Responses and coalesced updates precede event frames in `conn.take()`. An event frame currently in transmission can delay a response by at most one frame.
- **Coalescing order integrity:** `Coalesce::Thread(id)` replaces existing queued updates. When a thread is removed, `Hub::threads.remove(thread)` prevents any future `thread_changed` from queuing updates for that thread.
- **Connection termination guarantees:** `server::serve` bounds connection drain by `FINISH_PATIENCE` (2s) and aborts remaining tasks via `JoinSet::shutdown()`; `write_loop` bounds stalled writes by `WRITE_STALL` (30s).
- **Client authentication and role checks:** Pre-`Hello` messages, duplicate `Hello`, unrecognized frame kinds, frames over 1 MiB, `Agent` roles, and unverified `App`/`Dev` connections are rejected without sending `Welcome`.
- **`Hello` timeout enforcement:** Single 5-second `tokio::time::timeout` wraps the full `read_client` frame assembly, protecting against byte-at-a-time dribble attacks.
- **Connection slot limits:** `MAX_CONNECTIONS` permits are acquired immediately upon TCP/Unix accept before reading `Hello`, bounding unauthenticated sockets.
- **Ledger resolution routing:** `ResolveApproval` and `AnswerQuestion` validate against the approval ledger under `live.requests` before dispatching to the session command channel; stale or already resolved requests receive `ErrorCode::Expired`.
- **Concurrent ResumeThread serialization:** `Inner::launch` synchronizes on `Inner::sessions`; simultaneous resumes for the same thread result in the second caller receiving `ErrorCode::Invalid`. Resuming while an older session task terminates is blocked until the old task removes itself.
- **Process spawn race protection:** `session_config`'s `on_spawn` listener synchronizes against `Process::over`; if the incarnation terminates before the process ID arrives, `*over == true` prevents post-closure recording.
- **Storage watch polling:** The store's read-only status is polled every 2 seconds; sustained read-only triggers session stops and inbox full alerts, and clears them automatically when storage recovers.
- **All 20 `DaemonCommand` variants:** Verified against doc comments in `wire.rs` for authority, parameters, return types, and error code mappings (`Expired`, `UnknownThread`, `NotRunning`, `StorageReadOnly`, `TurnsRunning`, `Invalid`, `Internal`).

---

## Test gaps

- **`MAX_SUBSCRIPTIONS_PER_CONNECTION` over real socket:** Tested only as a unit test in `conn.rs:subscriptions_are_limited`. A mutation removing the check at `server.rs:350` survives all integration tests in `tests/socket.rs`.
- **Socket test for slow command quota:** The daemon-wide limit of 8 slow commands (`MAX_SLOW_COMMANDS`) added in round 2 has no integration test spanning multiple concurrent probe/export calls over real sockets.
- **Keep-running service rollback failure:** In `commands.rs:set_setting`, the compensation path where service reconfiguration fails and setting reversion fails is not exercised by tests (since test environments disable `manage_service`).
- **Store read failure during replay:** When `store.events_after` fails in `feed`, `server.rs:broken` closes the connection. No test exercises this error path with an injected store failure.

---

## Evaluation of Earlier Reviews and Declined Findings

### Fixes made after Round 2
1. **Gated first user message (`sessions.rs:646–675`):** Retains `commands` until the initial message is accepted or times out. Commands from other clients receive `ErrorCode::NotRunning` while waiting. Verified.
2. **Renumbering while storage is full (`sessions.rs:520–527`):** Closing events kept during `closing_only` are renumbered starting from `next_seq`. Verified (subject to Finding 2 regarding mixed batches).
3. **Closure retried before resume (`commands.rs:431–442`):** `resume_thread` retries `close_incarnation` for unclosed previous incarnations before starting a new one. Verified.
4. **Daemon-wide slow-command semaphore (`server.rs:283–300`, `lib.rs:333`):** Capped at 8 permits across the daemon, with connections waiting in the reader once permits are exhausted, and workers cancelled on connection close. Verified.
5. **Keep-running service reconciliation (`commands.rs:563–658`):** Synchronized under `inner.service` mutex, with startup reconciliation in `commands::reconcile_service`. Verified.
6. **Synthetic agent queue limit handling (`fake.rs:108–162`):** Messages exceeding `MAX_QUEUED_MESSAGES` emit non-fatal `AgentEvent::Error` events. Tested and verified.

### The Two Declined Findings
- **Codex Finding 5 (first half) — Event written after Unsubscribe:**
  **Accept the author's decline.** `Unsubscribe` is a unidirectional control message with no wire acknowledgment. If a frame has already been removed from the outbound queue by the writer and is in the middle of being transferred via `write_all`, it cannot be revoked from the kernel socket buffer. Because `Unsubscribe` provides no response, the client cannot observe a causal ordering violation. Any subsequent command's response is queued on the control lane and will only be transmitted on the writer's subsequent cycle. D-058's guarantee that nothing from the earlier subscription follows a subsequent subscription response remains intact.
- **Codex Finding 11 — Snapshot admission race with turn start:**
  **Accept the author's decline.** Contract §7 and brief line 31 specify running snapshots once a day when no turn is running. In SQLite WAL mode, `VACUUM INTO` is an isolated read transaction that does not block active writers or appends. Evaluating `running_turns() == 0` prior to initiating the snapshot satisfies the scheduling requirement without introducing unnecessary global writer locks.

---

## The Author's Own Decisions

1. **Verify adapter processes with `supervise::kill_recorded`:** Complies with Contract §10. (Subject to Finding 1 regarding sticky-state masking).
2. **Recover unrecorded `Starting` threads as `Cleanup::Unverified`:** Conservative, sound, and consistent with the uncertainty principle of crash recovery.
3. **End sessions on non-storage-full store batch refusals:** Aligns with `limits.rs:37–41` to prevent divergent state histories.
4. **Map ledger `Store` errors to `Internal` and all other refusals to `Expired`:** Correctly implements the wire protocol distinction between stale requests and unexpected storage failures.
5. **Direct store reads for Get/List vs. Hub cached inbox summaries:** Adheres to the requirement that streaming text must not emit per-batch inbox broadcasts, while refreshing before `SubscribeInbox` ensures initial snapshot accuracy.
6. **Title generation (first line, spaces collapsed, max 120 chars):** Practical, clean, and does not contradict any frozen contract.
7. **`keep_agents_running` service management:** Follows Contract §10 specifications.
8. **30-second write timeout:** Reasonable safeguard against hung clients; client reconnects from its applied cursor.
