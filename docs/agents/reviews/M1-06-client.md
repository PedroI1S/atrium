# Review: M1-06-client
Reviewer: Gemini 3.8 Flash (read-only) · 2026-11-10
## Verdict
**approve with changes**: The per-window queue isolation, stream folding, and reconnection mechanics are solid, but a protocol violation in `ReplayDone` causes a permanent silent freeze of window deliveries, and socket peer credentials are not verified on the connected stream.

## Findings
1. [major] **Silent permanent freeze of thread view on mismatched `ReplayDone.last`**
   - Where: `crates/atrium-client/src/state.rs:474-481` (`State::on_message`).
   - Sequence:
     1. Window "main" opens and views thread "t": `view("main", 1, ThreadId("t"))`.
     2. Client connects and sends `ClientMessage::Subscribe { request_id: 1, thread_id: "t", after: None }`. `sub.phase` becomes `Phase::Subscribing { request: 1, resumed: false }`. Window has `View::Stale`.
     3. Daemon answers with `ServerMessage::Response { request_id: 1, result: CommandResult::Ok { value: json!({ "replay_until": 10 }) } }`. `sub.phase` transitions to `Phase::Replaying { until: Some(EventId(10)) }`.
     4. Daemon sends replay events 1..=10 via `ServerMessage::Event`. Client appends them to cache; window remains `View::Stale`.
     5. Daemon sends `ServerMessage::ReplayDone { thread_id: ThreadId("t"), last: Some(EventId(9)) }` (or any `last != until`).
     6. In `state.rs:476`, `matches!(sub.phase, Phase::Replaying { until } if until == last)` evaluates to `false`.
   - Result: The client does not transition `sub.phase` to `Phase::Live`, does not call `pump_viewers`, does not return an error, and silently swallows the frame returning `Ok(())`. The thread remains permanently stuck in `Phase::Replaying`. In `pump()`, `Phase::Replaying` hits `continue`, so the window is never sent `Delivery::Snapshot`. Subsequent live events from the daemon are appended to cache, but `on_event` line 531 checks `window.threads.get(&thread_id) == Some(&View::Live)`; since the view is `View::Stale`, all live deliveries to the window are suppressed. The window is permanently frozen. Under `docs/contracts/m1.md` §6.2, `crates/atrium-protocol/src/wire.rs:373-378`, and `docs/agents/handoffs/M1-06-client.md` line 52 ("`ReplayDone.last` must equal it. The client only goes live on a `ReplayDone` that matches"), a mismatched `ReplayDone` is a daemon protocol violation and must fail the connection or thread, not be silently ignored.
   - Fix: In `state.rs:474-481`, if `sub.phase` is `Phase::Replaying { until }` and `until != last`, return `Err(format!("daemon broke protocol: ReplayDone last ({last:?}) does not match replay_until ({until:?})"))`.

2. [major] **Socket check TOCTOU window and missing peer credential check on connected stream**
   - Where: `crates/atrium-client/src/connection.rs:134-148, 293-326` (`connect`, `verify_socket`).
   - Sequence:
     1. `connect()` calls `verify_socket(&options.socket)`.
     2. `verify_socket` performs `std::fs::symlink_metadata` on the parent folder and socket file, verifying UID and 0700/0600 mode permissions.
     3. Between `verify_socket` returning and `within(UnixStream::connect(&options.socket))` executing, the socket is unlinked or replaced by another process/daemon.
     4. `UnixStream::connect` connects to the replacement socket.
     5. `connect()` immediately proceeds to send `ClientMessage::Hello` without checking the connected peer's credentials.
   - Result: The security check is purely filesystem-stat based and vulnerable to a TOCTOU race. Under `docs/contracts/m1.md` §4 and `crates/atrium-protocol/src/wire.rs:21-43` ("Socket security" and "Roles and authority"), communication must be strictly restricted to the user's authentic daemon. While file permissions guard against other users, verifying the peer on the connected file descriptor via `SO_PEERCRED` / `LOCAL_PEERCRED` prevents connecting to an unexpected socket or swapped descriptor.
   - Fix: After `UnixStream::connect` succeeds, inspect peer credentials on the connected socket using `rustix::net::sockopt::get_socket_peercred` (or `UnixStream::peer_cred`) and assert that the peer UID matches `rustix::process::geteuid()`.

3. [minor] **`ToolCallUpdated` byte accounting inflates deltas and undercounts replacements**
   - Where: `crates/atrium-client/src/cache.rs:172-185` (`ThreadCache::append`).
   - Sequence A (Overcounting):
     1. Provider streams command output via 1,000 `ToolCallUpdated` chunks of 10 bytes (`output_delta: Some(...)`, `output: None`).
     2. Each incoming frame is ~200 bytes of JSON (`bytes = 200`).
     3. Line 177: `let merged = if replaced { bytes } else { entry.bytes + bytes };`.
     4. `entry.bytes` adds 200 bytes per delta chunk, reaching 200,000 bytes for 10 KB of text (20x overestimate), prematurely forcing segment eviction in `trim()`. (In contrast, `MessageDelta` line 152 correctly adds only `text.len()`).
   - Sequence B (Undercounting):
     1. Provider emits `ToolCallUpdated` with a 50 KB `input` payload. `entry.bytes` = 50,000.
     2. Provider emits subsequent `ToolCallUpdated` with `output: Some("ok")` (wire size ~200 bytes).
     3. `merge_update` preserves `input` on `into`, but sets `replaced = true`.
     4. Line 177: `merged = bytes` (~200 bytes). `entry.bytes` drops from 50,000 to 200 bytes and `self.bytes` decreases by ~50 KB despite the 50 KB `input` staying in memory, bypassing the 64 MiB cap.
   - Result: `self.bytes` drifts from actual cached memory occupancy.
   - Fix: For delta updates (`output_delta`), increment `entry.bytes` and `self.bytes` by `delta.len()`. When whole fields (`input`, `output`, `title`) are updated, compute the actual differential size rather than substituting the raw wire size of the update frame.

4. [minor] **Unbounded command wait when daemon stays connected but unresponsive**
   - Where: `crates/atrium-client/src/lib.rs:160-164` (`Client::command`).
   - Sequence:
     1. Client connects to daemon and handshakes successfully.
     2. Client calls `client.command(DaemonCommand::ListThreads)`.
     3. Command is encoded and queued; `PendingCommand` is saved in `state.commands`.
     4. Daemon socket stays open and reader loop stays idle, but daemon handler hangs or fails to respond to `request_id`.
     5. `Client::command` calls `answer.await`.
   - Result: Unlike handshake I/O (which is guarded by `IO_TIMEOUT = 5s`), `Client::command` has no timeout. Because the connection does not drop, `answer.await` hangs forever.
   - Fix: Provide a timeout option in `ClientOptions` or apply a sane default timeout (e.g., 30s) so unresponded commands fail with a timeout error rather than hanging callers indefinitely.

## Suspected, not shown
- **Unbounded cache growth when an item never closes within an incarnation**:
  If a tool call or approval request is opened (`ToolCallStarted`, `ApprovalRequested`) and never finished (`ToolCallFinished`, `RequestResolved`), `self.open.idle()` remains `false` forever. All subsequent events have `boundary = false`. When `self.bytes > max_bytes` (64 MiB), `trim()` searches for `self.entries.iter().skip(1).position(|e| e.boundary)`, which yields `None`, so `trim()` cannot evict any entries in the unclosed segment. What stopped this from being classified as a defect is that the design explicitly requires whole-segment eviction so views never begin mid-item, and line 244 explicitly notes: `"The last segment stays whatever its size: it may still be growing."`

## Tried and held
- **Windows: Acknowledgements of `seq` never delivered, duplicate, or out of order**:
  Sequence: `ack(999)` with 3 items in flight; duplicate `ack(2)` followed by `ack(2)`; out-of-order `ack(4)` followed by `ack(2)`.
  Held: In `state.rs:811-817`, `in_flight` pops only while `oldest <= seq`. Pop decrements `in_flight_bytes` by exact item bytes; queue becomes empty when `oldest > seq`. No underflow, negative value, or corruption occurs.
- **Windows: Deliveries exceeding `MAX_WINDOW_QUEUE_BYTES`**:
  Sequence: Snapshot of 20 MiB delivered when `MAX_WINDOW_QUEUE_BYTES` is 16 MiB.
  Held: In `state.rs:142`, `used == 0 || used.saturating_add(bytes) <= max` allows oversized deliveries when `used == 0`. The window takes and acknowledges the oversized delivery, returning `used` to 0 for subsequent deliveries.
- **Windows: Hung window delaying active window or command**:
  Sequence: Window "hung" never acknowledges while daemon sends 40 MiB of deltas to shared thread; window "main" receives live events; command `ListThreads` invoked.
  Held: Hung window hits bound and `fall_behind()` clears waiting queue and marks views stale without blocking. Window "main" receives all 40 MiB in order and commands execute without delay.
- **Windows: Window closed with deliveries in flight**:
  Sequence: Window takes deliveries 1..5 (`in_flight_bytes > 0`), then window drops or calls `close_window`.
  Held: `close_window` drops `WindowState` and view registrations cleanly. Subsequent takes return `Take::Closed` and no bytes leak.
- **Windows: Window opened with existing label**:
  Sequence: `open_window("main")` followed by `open_window("main")`.
  Held: Old generation window receives `Take::Closed` on next recv. New generation window starts fresh queue at `seq: 0`.
- **Subscriptions: Late response to earlier `Subscribe`**:
  Sequence: Request 1 `Subscribe` sent; subscription replaced by Request 2 `Subscribe`; Response for Request 1 arrives.
  Held: `state.rs:595` checks `current != request`, cleanly ignoring the stale response.
- **Subscriptions: View opened and closed before `Subscribe` response arrives**:
  Sequence: `view("t")` sends Subscribe 1; `unview("t")` removes viewer and sends `Unsubscribe`; Response for Subscribe 1 arrives.
  Held: `state.known.subs` no longer contains thread "t", so `on_subscribed` returns `Ok(())` and discards response.
- **Subscriptions: Last viewer leaves while subscribe in flight, then new viewer arrives before answer**:
  Sequence: Viewer 1 leaves (triggering Unsubscribe); Viewer 2 views "t" (triggering Subscribe 2); Response for Subscribe 1 arrives.
  Held: Request 1 response is ignored due to `current != request` (current is 2). Viewer 2 receives replay once Subscribe 2 response arrives.
- **Subscriptions: Reload loop after rejected cursor**:
  Sequence: Subscribe with `after: Some(EventId(10))` rejected with `ErrorCode::Invalid`; client resets cache and sends `after: None`; daemon rejects second subscribe with `ErrorCode::Internal`.
  Held: Second subscribe has `resumed: false`, so it transitions to `Phase::Failed`, notifies viewers with `Delivery::ThreadFailed`, and does not loop.
- **Subscriptions: Lagged subscription while replaying**:
  Sequence: Thread in `Phase::Replaying`; daemon sends `SubscriptionLagged`.
  Held: Resubscribes from `cache.last()`. Any additional `SubscriptionLagged` while `Subscribing` is ignored because phase is not `Replaying` or `Live`.
- **Cache: Text deltas interleaved between two open messages**:
  Sequence: Message "m1" delta "A", message "m2" delta "X", "m1" delta "B", "m2" delta "Y".
  Held: `self.open.messages` tracks entry indices independently. "m1" entry folds to "AB" and "m2" entry folds to "XY" without interleaving errors.
- **Cache: Cache evicted down to single segment**:
  Sequence: Cache trimmed so that only the newest segment remains. Reconnect occurs.
  Held: `trim()` preserves the active segment; `self.last` is a scalar `Option<EventId>` that remains intact and supplies the correct cursor for resubscription.
- **Cache: `page_before` edge cases**:
  Sequence: `page_before` with `EventId(1)` (first event), `EventId(0)` (before first), and non-existent IDs.
  Held: `partition_point` correctly identifies boundary; returns empty page with `has_older: false` and `truncated: true` when before first cached event.
- **Connection: Start hook concurrency**:
  Sequence: Daemon missing on startup; reconnection loop runs.
  Held: `run` sequentially awaits `spawn_blocking(start)` inside the single connection task loop. Next attempt does not begin until hook completes. Hook is not run on `Incompatible`, `Failed`, or protocol violations.
- **Connection: Frame length bounds**:
  Sequence: Client reads frame with length 0, length > 16 MiB, unknown kind, or unannounced stream.
  Held: `read_control` returns `InvalidData`, terminating session with `ConnectionState::Failed` without reconnecting.
- **Connection: Panic in read/write tasks**:
  Sequence: Simulated panic in `read_loop` or `write_loop`.
  Held: `session` catches task join error via `tokio::select!`, treats as `Ended::Lost`, drains pending commands with `CommandError::UnknownOutcome`, and triggers reconnect.

## Test gaps
1. **Missing test: Mismatched `ReplayDone.last`**:
   No test in `state::tests` or `tests/socket.rs` sends a `ReplayDone` where `last != replay_until`. This allows Finding 1 (silent freeze) to go undetected.
2. **Missing test: Socket descriptor peer UID verification**:
   No test verifies that `connect` validates peer credentials (`SO_PEERCRED` / `LOCAL_PEERCRED`) on the connected socket stream.
3. **Missing test: `ToolCallUpdated` byte folding accounting**:
   `cache::tests::deltas_and_tool_updates_fold_and_usage_keeps_the_latest` tests field values but does not assert `cache.bytes()` after multiple `ToolCallUpdated` deltas or replacements, masking Finding 3.
4. **Missing test: `Client::command` timeout / hung daemon**:
   No socket test verifies behavior when a command is dispatched to a connected daemon that never replies.
5. **Mirroring / Ineffective test assertions**:
   `state::tests::what_the_daemon_must_not_send_is_a_violation` (lines 1665-1677) feeds late `ok`, `event`, `done`, and `SubscriptionLagged` frames into `rig.daemon(...)` after an `unview`, but contains no assertions verifying that the state remained intact and no deliveries were queued.
