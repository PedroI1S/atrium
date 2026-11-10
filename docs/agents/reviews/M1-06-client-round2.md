# Review: M1-06-client, round 2
Reviewer: Gemini 3.8 Flash (read-only) · 2026-11-10
## Verdict
**approve**: All round-1 findings are resolved or soundly declined, author-introduced improvements correctly address edge cases, and all verified sequences across subscription replays, cache segments, connection backoff, and reader shutdown hold.

## Round-1 findings
- Finding 1 (`ReplayDone.last` mismatch silent freeze): fixed.
- Finding 2 (connected stream peer UID verification): fixed.
- Finding 3 (`ToolCallUpdated` byte folding accounting): fixed.
- Finding 4 (command timeout in client): declined and accepted; commands are strictly one-shot and caller-side deadline cancellation via dropping `Client::command` is safe.

## Findings
None.

## Tried and held
- **1: Replay termination and boundary validation (`state.rs:475-496`)**:
  - *Resume with cursor where `replay_until` equals cursor*: Thread cache holds up to `EventId(5)`. Reconnect sends `Subscribe { after: Some(5) }`. Daemon has no new events and responds `Ok { replay_until: 5 }`. Replay stream emits 0 events and sends `ReplayDone { last: Some(5) }`. `until == last` (5 == 5) and `have < until` (5 < 5) evaluates to false. Transition to `Phase::Live` succeeds without error.
  - *Replay with `replay_until: null`*: Empty thread subscribes with `after: None`. Daemon responds `Ok { replay_until: null }`. Daemon sends `ReplayDone { last: None }`. `until` and `last` are both `None`; `until.is_some_and(...)` evaluates to false. View transitions to `Phase::Live` immediately.
  - *Resubscribe after `SubscriptionLagged`*: Thread receives `SubscriptionLagged`. Client resubscribes from `sub.cache.last()`. Delta events are replayed up to `replay_until`, followed by matching `ReplayDone`. Views transition to live with no dropped or duplicated events.
  - *Reload after rejected cursor*: Client resumes with invalid cursor; daemon answers `Err(ErrorCode::Invalid)`. Client resets cache (`sub.cache.reset()`), sends `Subscribe { after: None }`, replays full history from start up to `replay_until`, and activates live view upon matching `ReplayDone`.
  - *Repeated events at or below cursor during replay*: Daemon resends events `<= cursor`. `ThreadCache::append` identifies `stored.id <= last` and returns false without modifying cache. When subsequent events arrive, `cache.last()` advances to `replay_until` and matches `ReplayDone`.
  - *Illegal premature `ReplayDone`*: Daemon specifies `replay_until: 5` but sends events up to 4, then sends `ReplayDone { last: 5 }`. `until.is_some_and(|until| sub.cache.last().is_none_or(|have| have < until))` evaluates to true (`4 < 5`). Returns protocol violation error, terminating connection.
  - *Illegal mismatched `ReplayDone`*: Daemon specifies `replay_until: 5` and sends `ReplayDone { last: 4 }` or `ReplayDone { last: None }`. `until != last` evaluates to true, triggering immediate protocol violation error.

- **2: Connected stream peer UID verification (`connection.rs:188-204`)**:
  - `stream.peer_cred()` inspects peer credentials on the connected socket descriptor via `SO_PEERCRED` / `LOCAL_PEERCRED`.
  - Connecting to a socket replaced by an adversary or another user fails with `Ended::Refused(ConnectionState::Failed)`, closing the connection before `ClientMessage::Hello` is sent.

- **3: Tool call byte accounting and delta folding (`cache.rs:178-185, 420-498`)**:
  - *Streamed output chunks*: 1,000 delta updates with `output_delta: Some(...)` and `input: None` accumulate text length directly without wire framing overhead or running `json_len`.
  - *Full output replacement*: When `output: Some(whole)` replaces streaming deltas, `merge_update` returns `(grown, shrunk)` reflecting gained content (`whole.len() + text(more)`) and lost content (`text(output) + text(output_delta)`).
  - *Input payload updates*: `json_len` serializes via `serde_json::to_writer` into a zero-allocation `Count` writer only when `new_input.is_some()`.
  - *Invariants*: `ThreadCache::bytes()` strictly matches the sum of cached entry bytes; no integer underflow or memory drift occurs.

- **4: Command cancellation safety (`lib.rs:160-164`, `state.rs:432-436, 680-685`)**:
  - Dropping `client.command(...)` drops the `oneshot::Receiver`. When the daemon response arrives, `pending.reply.send(...)` safely fails without panicking.
  - Connection disconnect drains pending commands with `CommandError::UnknownOutcome`.

- **5: Segment-bounded folding of latest values (`cache.rs:77, 204-213, 239-253, 257-268`)**:
  - `Usage`, `ContextUpdated`, and `QuotaUpdated` fold only when `entry_number >= self.segment_start`.
  - A tail snapshot always includes the active segment, ensuring that a window opening mid-session receives the latest usage and quota metrics.
  - History paging via `page_before` returns intact historical segments without retroactively altered metrics.
  - Front-end cache eviction in `trim()` preserves the active segment at the tail; `segment_start` and `base` offsets remain consistent across evictions and `reset()`.
  - Entries per segment are bounded (at most one per slot per segment).

- **6: Early connection drop backoff (`connection.rs:49-51, 88-165, 272-315`)**:
  - First connection loss reconnects at once (`early == 1`).
  - Rapid repeated drops before `SETTLED` (2 s) trigger exponential backoff (`early > 1`).
  - Stable connections lasting $\ge 2$ s reset `early = 0`.
  - Calling `Client::reconnect()` during a backoff sleep breaks the sleep and reconnects immediately.
  - Calling `Client::reconnect()` during an active session resets `early = 0`.
  - Calling `Client::reconnect()` while connecting cancels the attempt and restarts immediately via biased `select!`.
  - During backoff sleep, windows observe `ConnectionState::Connecting { attempt: 1 }`.
  - Start daemon hook schedule (`attempt % START_EVERY == 1`) is preserved.

- **7: Reader task shutdown and abort safety (`connection.rs:284-315`)**:
  - The `reading` boolean flag ensures `read.await` is never polled after completing inside `select!`.
  - When connection termination is triggered by write failure, reconnect, or shutdown, `read.abort()` cancels the reader task and `read.await` blocks until the reader cleanly terminates.
  - `read_loop` has no blocking system calls and cancels at `read_control.await` without hanging, guaranteeing no stale connection frames are processed after a subsequent reconnect.
