# Review: M1-05-daemon (Codex)
Reviewer: GPT-6.1 Sol via Codex (read-only, sources inlined) · 2026-11-11

## Verdict

**rework**: demonstrated failures leave incarnations open, lose user text, violate unsubscribe guarantees, and allow shutdown to finish while a session task retains the daemon indefinitely.

This is a hand trace of the supplied sources. I could not run tests. Integration test bodies and several delegated implementations were not supplied; their names are not evidence of their assertions.

Notation below: `Command(r, C)` means `ClientMessage::Command { request_id: r, command: C }`. Connections have completed `Hello { protocol_version: 1, client_name: "review", role: Dev }` against a debug daemon with `dev_socket = true`. `T` is the returned thread ID; `R` is an actual ledger-issued request ID. Adapter envelopes use incarnation 1, the stated sequence number, and increasing `at_ms`.

## Findings

1. [major] **The first user-message send can retain the daemon indefinitely**
   - Where: `crates/atrium-daemon/src/sessions.rs:612` (`run`), `sessions.rs:702` (`stop_all`), `lib.rs:398` (`Daemon::stopped`).
   - Sequence:
     1. Send `Command(1, StartThread { agent: Native { provider: Claude, model: None, effort: None, preset: Home }, message: Some("go") })`.
     2. The scripted adapter returns a `Session` whose command channel already contains `COMMAND_BUFFER` `Interrupt` commands. Retain its command receiver without draining it, and retain its event sender.
     3. `run` installs the command sender, then waits at line 612 to send `"go"` into the full channel.
     4. Request forced daemon shutdown.
     5. `Live::stop` cannot enqueue `Shutdown`. The task never reaches `pump`, so neither its stop watcher nor `STOP_PATIENCE` runs.
     6. `stop_all` times out; `Daemon::stopped` exhausts its unwrap retries and returns.
   - Result: the session task remains blocked, holds `Arc<Inner>`, keeps the instance lock, and stores no closing events. The brief requires stopping sessions and storing their closing events on daemon shutdown.
   - Fix: make the initial send bounded and cancellable by `live.stop`; route cancellation or send failure through the existing incarnation cleanup path.

2. [major] **Recovery skips unfinished incarnations in sticky states and archived threads**
   - Where: `crates/atrium-daemon/src/upkeep.rs:66` (`recover`), especially lines 81–82.
   - Sequence:
     1. Start `Fake { tokens_per_second: 1000, chunk_chars: 4, duration_ms: 60000, tool_call_every_ms: 0 }`.
     2. Wait until its `TurnStarted` and an assistant `MessageDelta` are stored. It has an open turn and message, with no recorded process.
     3. Send `Command(2, ThreadCommand { thread_id: T, command: Shutdown })`.
     4. Let `Signal::Stopped` commit, producing `Ended`, then kill the daemon before the pump stores closing events.
     5. Restart. `live_processes` contains no entry for this Fake session. The second recovery pass sees `Ended` and skips it.
     6. Restart again: it is skipped again.
     7. An alternative is `ArchiveThread { thread_id: T }` followed by the crash after archive commits: `Store::threads` excludes it entirely.
   - Result: open messages and turns remain without `SessionEnded`; requests, if present, are expired globally without their required closing events. Contract §5 requires closing every previously live incarnation before accepting clients, and §10 does not exempt archived or sticky-state threads.
   - Fix: enumerate unfinished incarnations independently of thread visibility and attention state. The supplied frozen Store interface lacks a complete enumeration for this; escalate that interface gap as required by brief line 51.

3. [major] **Failed recovery closure is treated as successful recovery**
   - Where: `crates/atrium-daemon/src/upkeep.rs:59` (`recover`), `sessions.rs:448` (`close_incarnation`).
   - Sequence:
     1. A previous daemon leaves incarnation 1 with a recorded process, an open turn, and no `SessionEnded`.
     2. Restart; `kill_recorded` returns `Verified`.
     3. Recovery stores `Signal::Recovered { cleanup: Verified }`.
     4. Inject a one-shot `StoreError::Sqlite("transient")` into the closing batch append.
     5. `close_incarnation` returns `false`; recovery ignores it, clears the process record, expires the ledger, and accepts clients.
     6. Restart again. There is no process record, and the thread is now `Interrupted`; finding 2's state filter skips it.
   - Result: missing closing events become permanent after a transient error. The startup ordering promised by contract §5 and brief line 23 has not completed.
   - Fix: return a distinguishable error from `close_incarnation` and propagate it during recovery. Do not clear the recovery record or accept clients until closure succeeds. Runtime callers at `sessions.rs:655` also need to handle closure failure.

4. [major] **An earlier sticky state hides an unverified process cleanup**
   - Where: `crates/atrium-daemon/src/commands.rs:461` (`thread_command`), `sessions.rs:649` (`run`), `crates/atrium-core/src/attention/mod.rs:54` (`next_state`).
   - Sequence:
     1. Start a native session and record process identity `P`.
     2. Send `Command(2, ThreadCommand { thread_id: T, command: Shutdown })`; `Signal::Stopped` makes the thread `Ended`.
     3. The adapter's event stream ends without `SessionEnded`.
     4. Arrange the documented `Unverified` case: `P`'s leader is gone but its process group remains nonempty, so `kill_recorded(P)` cannot safely kill the group.
     5. `run` sends `Recovered { cleanup: Unverified }`, then stores closing events.
   - Result: `next_state` ignores recovery because `Ended` is sticky. The client sees `Ended` while cleanup is unverified, contrary to contract §10's “never shown as stopped” rule. Starting from `StorageReadOnly` produces the same defect with sticky `Interrupted { Verified }`. A crash after the user-stop signal produces the same problem in recovery.
   - Fix: allow cleanup uncertainty to escalate an already sticky stopped state. Merely moving the recovery signal before synthetic closing events does not fix this sequence. The frozen attention rules need an explicit contract change or an independently exposed cleanup status.

5. [major] **Unsubscribe does not revoke an already-taken event or a queued lag notification**
   - Where: `crates/atrium-daemon/src/conn.rs:303` (`unsubscribe`), `conn.rs:369` (`push_live`), `conn.rs:431` (`write_loop`).
   - Sequence A:
     1. Subscribe to `T`; queue event `E1`.
     2. The writer takes `E1` at line 431. Pause it before `write_all` writes any bytes.
     3. Process `Unsubscribe { thread_id: T }`.
     4. Resume the writer: it sends `E1`.
   - Sequence B:
     1. Make `T` live and fill the event lane.
     2. Publish an event for `T` that does not fit. `push_live` removes the subscription and queues `SubscriptionLagged` as an untagged `Coalesce::Keep` control frame.
     3. Before the writer takes that control frame, process `Unsubscribe { thread_id: T }`.
     4. The writer sends the old `SubscriptionLagged`.
   - Result: frames from the earlier subscription are sent after unsubscribe was processed. Both violate wire.rs lines 61–65 and D-058.
   - Fix: retain subscription identity on lag notifications and purge them on unsubscribe. Serialize unsubscribe processing with writes so a frame already removed from the queue cannot begin transmission afterward; queue removal alone is insufficient.

6. [major] **Shutdown can close its connection before queuing its response**
   - Where: `crates/atrium-daemon/src/commands.rs:306` (`ShutdownDaemon`), `server.rs:284` (`handle`), `server.rs:87` (`serve`).
   - Sequence:
     1. An otherwise idle authenticated connection sends `Command(7, ShutdownDaemon { force: true })`.
     2. Worker A executes `request_stop(true)` at commands.rs line 306. Pause it before `handle` calls `answer`.
     3. Worker B observes `Stopping`, runs `finish_all`, and wakes the writer.
     4. The writer finds the finishing connection's control lane empty, closes it, and calls `conn.close`.
     5. Resume worker A; `answer` attempts to queue response 7, but `control_frame` discards it because the connection is closed.
   - Result: an accepted shutdown receives no response. This contradicts wire.rs lines 67–68 and the explicit shutdown-response promise at commands.rs line 305.
   - Fix: queue the successful shutdown response before publishing the stopping phase, or introduce a response barrier that finishing must await.

7. [major] **A resolution can reach the session after its turn's closing event was stored**
   - Where: `crates/atrium-daemon/src/sessions.rs:305` (`store_batch`), `commands.rs:230` (`AnswerQuestion`).
   - Sequence:
     1. In incarnation 1, start turn 1. The adapter obtains question ticket `R` through `gate.question_ticket(Some(1))`; its ledger row is pending.
     2. The adapter emits `TurnCompleted { turn: 1, outcome: Completed, usage: None }`.
     3. Let its append and publish finish. Pause `store_batch` after line 305, before `expire_turn` at line 322.
     4. On another worker, process `Command(8, AnswerQuestion { thread_id: T, incarnation: 1, request_id: R, answers: [] })`.
     5. The ledger still sees a pending, current-incarnation row; it accepts it. `session_of` finds the same live session and forwards `AnswerQuestion`.
     6. Resume the expiry task.
   - Result: a request whose turn has already ended in stored history reaches the session and receives `Ok`. Contract §5 says entries expire when the turn ends. The same window exists between stored cancellation/session-end events and their ledger updates.
   - Fix: serialize stored terminal events and their ledger expiry with resolution acceptance and forwarding. The append/publish lock currently ends too early and resolutions do not participate in it.

8. [major] **Rejected batches create gaps in stored sequence numbers**
   - Where: `crates/atrium-daemon/src/sessions.rs:491` (`pump`), `sessions.rs:377` (`close_incarnation`).
   - Sequence:
     1. Store sequence 0 `SessionStarted`, 1 `TurnStarted { turn: 1 }`, 2 user `MessageCompleted`, and 3 `MessageDelta { message_id: "a1", kind: Assistant, text: "a" }`.
     2. Emit sequence 4, another delta for `"a1"`, with text of `MAX_EVENT_BYTES + 1` bytes.
     3. `pump` advances `out.next_seq` to 5 before the append.
     4. The store rejects the whole batch with `EventTooLarge`.
     5. Cleanup synthesizes `Error` at sequence 5, then message completion, turn completion, and session end at subsequent numbers.
   - Result: stored sequences are `0,1,2,3,5,…`; sequence 4 never existed in the store. The synthetic continuation uses the rejected input's sequence rather than the last committed sequence. This breaks the requested continuous-sequence invariant. Read-only rejection and subsequent filtering can create analogous gaps.
   - Fix: base daemon-generated closing sequence numbers on the last committed event. Track received and committed sequence positions separately.

9. [major] **Fake silently truncates user messages**
   - Where: `crates/atrium-daemon/src/fake.rs:182` (`Agent::turn`).
   - Sequence:
     1. Send `Command(1, StartThread { agent: Fake { tokens_per_second: 0, chunk_chars: 1, duration_ms: 0, tool_call_every_ms: 0 }, message: Some("x" repeated 65537 times) })`. This fits the client-frame limit.
     2. Fake emits `SessionStarted`, `TurnStarted`, and one inline user completion containing only 65536 characters.
     3. Replay or export the thread.
   - Result: the final character is permanently absent. Contract §6.1 requires larger completion-only user messages to be emitted as deltas followed by a streamed completion; it explicitly requires the whole conversation to survive replay and export.
   - Fix: split long user messages into bounded deltas and emit a matching streamed completion instead of calling `cut`.

10. [major] **Slow commands have no limit on outstanding work**
    - Where: `crates/atrium-daemon/src/server.rs:276` (`handle`), `commands.rs:492` (`probe`), `commands.rs:695` (`export`).
    - Sequence:
      1. Register an adapter whose `probe` remains pending until its timeout.
      2. On one authenticated connection, send `Command(r, ProbeProviders { preset: Home })` for successive request IDs, before the first 15-second probe timeout.
      3. Each request spawns a separate task. No response has completed, so the control-lane cap has not engaged.
      4. For any chosen outstanding count `N`, the admission code contains no count or byte check that rejects request `N+1`. Export requests similarly enqueue independent blocking jobs.
    - Result: connection and lane quotas do not bound retained command tasks or export work. A client can exhaust resources without filling either outbound lane, contrary to the brief's resource-exhaustion and bounded-memory requirements.
    - Fix: enforce a daemon-wide and/or per-connection quota before spawning slow commands; release permits on completion and cancel appropriate work when its connection closes.

11. [minor] **A snapshot can begin while a turn is running**
    - Where: `crates/atrium-daemon/src/upkeep.rs:177` (`snapshot_daily`).
    - Sequence:
      1. A snapshot is due and `running_turns()` returns zero.
      2. Pause upkeep after this check.
      3. Start a Fake thread and let its pump observe and store `TurnStarted`.
      4. Resume upkeep; it calls `store.snapshot` while the turn remains open.
    - Result: the snapshot admission check races turn admission, violating contract §7 and brief line 31.
    - Fix: coordinate snapshot admission with starting turns. A second unsynchronized check merely moves the race.

12. [minor] **SubscribeInbox's initial summaries can already be stale**
    - Where: `crates/atrium-daemon/src/sessions.rs:182` (`shows_in_summary`), `hub.rs:99` (`subscribe_inbox`).
    - Sequence:
      1. Store the first assistant delta, event `E4`; it sets unread and refreshes the hub.
      2. Store a second assistant delta, `E5`, in a separate batch. Since unread is already true, the hub is not refreshed.
      3. Send `SubscribeInbox { request_id: 9 }`; its returned summary has `last_event = E4`.
      4. Send `Command(10, GetThread { thread_id: T })` before another append; its summary has `last_event = E5`.
    - Result: the initial inbox response does not contain current summaries as promised by wire.rs lines 69–73. Avoiding a broadcast per text batch does not require returning an outdated initial snapshot.
    - Fix: obtain current summaries for initial inbox subscriptions, while coordinating registration with subsequent hub updates. Keep the suppression of per-delta broadcasts.

13. [minor] **A failed setting write leaves the login service changed**
    - Where: `crates/atrium-daemon/src/commands.rs:549` (`set_setting`), followed by line 568.
    - Sequence:
      1. With `manage_service = true`, the stored setting is `keep_agents_running = false` and the login service is absent.
      2. Send `Command(11, SetSetting { key: "keep_agents_running", value: true })`.
      3. Let `install_keep_running` succeed.
      4. Before the setting transaction, storage becomes read-only; `set_setting` returns `ReadOnly`.
      5. Read `GetSettings`.
    - Result: the client receives `StorageReadOnly` and reads `false`, but the service is installed and changes next-login behavior. Contract §10 couples this setting to installing/removing the service; there is no reconciliation here.
    - Fix: preserve the previous service state and compensate on persistence failure, or persist and reconcile an explicit desired service state. Report compensation failure explicitly.

## Suspected, not shown

- **Recovery of an older recorded incarnation:** upkeep.rs lines 47–49 kill/check its process, then clear its record without closing its open items. The omission is visible, but I could not establish a normal successful daemon path that creates this state independently of an earlier failed closure.
- **Native launch validation:** commands.rs lines 326–332 check adapter registration but show no model-catalogue or provider-enabled validation required by `AgentLaunch::Native`'s wire comment. The adapter/provider interfaces and settings schema needed to construct the exact invalid-model sequence were not supplied.
- **Export path safety:** relative paths are rejected locally. Existing files, directories, and symlink traversal depend on `history::create_export_file`, whose implementation was not supplied. I cannot certify those cases from its name or contract alone.
- **Export cleanup guarantees:** ordinary returned export/read/flush errors reach `remove_file`, but its error is ignored. The supplied sources do not establish the filesystem conditions needed to guarantee removal after directory changes or permission failures.
- **Storage recovery while idle:** whether the status can return from read-only without another write depends on the omitted `SqliteStore::storage_status` implementation. The daemon polls it correctly; that alone does not prove it refreshes free-space information.
- **App identity mutation:** another same-user process can rewrite the already-open inode or change its permissions after `fstat`. Path replacement does not redirect the open descriptor. This is within the expressly acknowledged same-user threat limit; I did not establish a separate forbidden authority escalation.

## Tried and held

These are hand-traced sequences, not executed tests.

- **Replay boundary:** append before the feeder's locked second read → feeder reads it; append after `go_live` → hub publishes it; the shared publish lock prevents a missed or duplicate boundary event.
- **Replay marker:** a page containing events both at/below and above `replay_until` queues `ReplayDone` immediately before the first above-boundary event; an empty page queues it after all preceding events.
- **Successful resubscribe:** an old feeder waiting for room wakes after replacement, finds its sub ID absent, and cannot enqueue more frames.
- **Old live registration:** a publish against an old hub sub ID returns `Gone`; it cannot target the replacement subscription.
- **Already-taken frame plus successful Subscribe:** the single writer finishes that frame before taking the new response, so it cannot deliver that frame after the response.
- **Lag plus successful Subscribe:** lag insertion and subscription removal share the outbox lock; a lag notification queued before replacement precedes its response in the control FIFO.
- **Byte accounting:** feed/push → take, feed → replace/unsubscribe, and feed → close/finish each remove exactly the bytes previously added under the same mutex; no underflow or drift appeared.
- **Oversized lane item:** with a deliberately smaller configured lane, an empty lane accepts one oversized item, for both store and live delivery. This is explicitly tested. Production's 32 MiB lane exceeds the permitted server-frame size.
- **`last_sent`:** taking an event updates `written` early, but the same writer must complete that event before sending a later lag frame; a failed event write prevents that lag frame from being sent.
- **Control priority:** queued responses precede queued event frames. An event already being written can delay a response by one frame; the writer cannot interleave framed messages.
- **Thread removal:** queued update → archive → delayed refresh replaces the update with removal; `thread_changed` cannot reinsert a removed thread.
- **Reader closure:** `closed()` registers its notification before checking the flag; close notification cannot be lost in that interval.
- **Role admission:** pre-Hello commands, repeated Hello, Agent, Dev without the flag, and Dev with debug assertions disabled are refused; every Welcome passes `grant`.
- **Hello deadline:** sending the header and payload one byte at a time still shares the single five-second timeout around `read_client`.
- **Connection quota:** 32 silent accepted connections consume all permits before Hello; another connection receives the refusal path.
- **Malformed frames:** lengths 0, 1, and greater than 1 MiB; unknown kind; Stream; and invalid JSON all terminate the connection without an indexing panic.
- **Identity read:** final-component symlink → `O_NOFOLLOW` failure; nonregular file, wrong owner, or group/other permissions → refusal; reads are capped at 64 KiB. Oversized files are prefix-read rather than explicitly rejected.
- **Normal synthetic closure:** stream end after valid stored events builds completions from open-item projections in message → tool → request → turn → session order.
- **Adapter startup failure:** failed `start` produces fatal Error then SessionEnded; no established-session events are invented.
- **Ledger routing:** commands embedded in `ThreadCommand` cannot bypass the ledger; direct approval/question commands perform ledger validation before finding or sending to the session.
- **Concurrent resumes:** two `ResumeThread` requests race into `launch`; the sessions-map insertion admits only one. A resume during old-task cleanup remains refused until that task removes itself.
- **Process record ordering:** an on_spawn recording task waiting behind `Process::over` cannot write after `over` becomes true; normal cleanup then clears the record.
- **Successful repeated recovery:** an already stored SessionEnded prevents another synthetic closure; a cleared process record is absent on the next pass.
- **Storage watch:** a sustained read-only state adds the inbox item, stops sessions whose turn flag is true, and removes the item after observing healthy status; new-work commands check storage status directly.

Command-by-command checks, excluding the findings above:

| Command | Hand-traced behavior |
|---|---|
| StartThread | Returns starting summary; absent adapter → Invalid; draining → Invalid; read-only → StorageReadOnly. |
| ListThreads | Reads Store's unarchived, newest-first list. |
| ProbeProviders | Returns provider-sorted probes; timeout produces an error entry. |
| RenameThread | Null on success; unknown/archived → UnknownThread; empty title → Invalid. |
| GetThread | Returns persisted summary, including an archived thread; absent → UnknownThread. |
| ThreadCommand | Null after accepted send/stop; unknown/archived → UnknownThread; absent session → NotRunning; embedded resolutions → Invalid. |
| ResolveApproval | Returns rule ID object; ledger refusal mapping precedes session routing. |
| AnswerQuestion | Returns null; follows the same ledger-first routing. |
| ResumeThread | Returns starting summary; unknown/archived → UnknownThread; missing native ID, Fake ID, or adapter → Invalid. |
| ArchiveThread | Signals/stops a running session, archives, broadcasts removal, returns null; unknown/archived → UnknownThread. |
| MarkRead | Persists and refreshes; unknown/archived → UnknownThread. |
| SetFocus | Records the optional ID and returns null; no existence check is specified by its wire comment. |
| ListInbox | Returns the hub's ranked inbox. |
| ListRules | Returns persisted rules. |
| CreateRule | Assigns ID, Settings origin, and time; empty command argv → Invalid. |
| DeleteRule | Delegates deletion, maps store failure, otherwise null. |
| ExportThread | Starts paging from None; relative path → Invalid; absent thread → UnknownThread; archived thread remains exportable. |
| GetSettings | Returns a JSON object of stored settings. |
| SetSetting | Empty/oversized key or nonboolean keep-running value → Invalid; ordinary keys are persisted unchanged. |
| ShutdownDaemon | Running turns plus force=false → TurnsRunning without changing phase; force=true proceeds to stopping. During draining, the refusal leaves draining intact. |

## Test gaps

- **Tests were not run.** The handoff's formatting, clippy, workspace tests, client integration, and performance results remain author-reported.
- **D-058, in-flight and lagged frames:** `conn::a_replaced_subscription_leaves_nothing_queued` never starts a writer and never queues a lag notification. A mutation retaining untagged lag frames or allowing already-taken frames to write after unsubscribe survives this unit test. The socket replacement test's assertions are unavailable.
- **Whole user messages:** `the_synthetic_agent_runs_turns_and_stops` is the relevant integration test name, but its body is unavailable. The current 64 KiB truncation demonstrates that the DoD needs a long-user-message replay/export assertion.
- **Rejected-batch sequence continuity:** `an_event_the_store_refuses_fails_the_session` may check the fatal state without checking sequences; its body is unavailable. Assert the complete committed sequence list around rejection, rather than only Error/SessionEnded.
- **Recovery completeness:** `a_killed_daemon_is_recovered_at_the_next_start` must also exercise no-process Ended/Interrupted/Failed and archived unfinished incarnations, plus a failed closing append. A successful recorded-process recovery alone does not cover findings 2–3.
- **Unverified cleanup after prior signals:** the named process-cleanup tests need variants preceded by `Stopped` and `StorageReadOnly`; merely testing Unverified from a live state misses finding 4.
- **Shutdown response race:** the unit `finishing_writes_the_control_lane_and_drops_the_events` queues the response before calling `finish`. Moving stop notification before response enqueue survives that test—it is the current implementation.
- **Startup backpressure:** no supplied test body demonstrates stopping while the initial session-command send is blocked. Add a full retained command channel, then assert closure and lock release.
- **Snapshot exclusion:** `a_snapshot_is_made_once_a_day_when_no_turn_runs` needs a controlled interleaving between the zero-turn check and snapshot admission. A sequential idle/busy test would still pass.
- **Role deadline coverage:** the handoff explicitly says byte-at-a-time Hello is untested; the hand trace holds. Socket subscription quota coverage is also explicitly absent.
- **Outstanding work and settings compensation:** no named test establishes slow-command admission bounds or service/store failure reconciliation.

## The author's own decisions

- **Check an adapter's leftover process:** consistent with contract §10. Its claimed Unverified behavior is incomplete because of finding 4.
- **Recover Starting without a recorded process as Unverified:** conservative and consistent with the contract's uncertainty rule.
- **End the session after a non-full-storage batch failure:** consistent with `limits.rs` lines 37–40. Sequence continuity and failed synthetic closure remain defective.
- **Map ledger Store failure to Internal:** consistent with the frozen wire's specified Expired cases, which concern invalid/stale requests. It is an exception to the brief's broader wording “every ledger refusal,” and should be described as such.
- **Store-backed Get/List versus cached inbox summaries:** suppressing streaming broadcasts is required; returning stale initial inbox summaries conflicts with the current-snapshot promise, as shown in finding 12.
- **Title derivation and 120-character limit:** no frozen-contract contradiction found.
- **Keep-running boolean installs/removes service; other settings stored unchanged:** consistent with the contract's setting semantics, subject to finding 13's partial failure.
- **Close writes after 30 seconds:** no frozen-contract contradiction found. The implementation times out the entire `write_all`, however; it does not reset the deadline whenever progress occurs.