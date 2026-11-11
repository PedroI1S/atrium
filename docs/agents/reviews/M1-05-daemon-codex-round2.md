# Review: M1-05-daemon, round 2 (Codex)
Reviewer: GPT-6.1 Sol via Codex (read-only, sources inlined) · 2026-11-11

## Verdict
**rework**: the fixes address several original sequences, but introduce unordered initial-message delivery and leave demonstrated failures in runtime closure, command lifetime, sequence continuity, and settings consistency.

## Round 1 findings

1. **fixed** — With the original full, undrained command channel, the initial send now times out after five seconds while `pump` independently observes shutdown and its deadline (`sessions.rs:627–643`). The original indefinite retention is gone; new delivery defects are below.
2. **not fixed completely; escalation accepted** — The original stopped Fake thread is now closed regardless of its `Ended` state (`upkeep.rs:60–70`). The archived Fake variant still has neither a process record nor a listed summary and remains undiscovered. CR-10 appropriately escalates the frozen enumeration gap; it does not make that sequence fixed. Archived threads with current process records are covered by lines 48–56.
3. **fixed wrongly** — In the original recovery sequence, the failed closing append now propagates through `close` and prevents startup and record clearing (`upkeep.rs:56,72–75,97–99`). Runtime closure failure, however, still proceeds to clearing the process and removing the session (`sessions.rs:697–720`), allowing permanent abandonment as shown below.
4. **not fixed; escalation accepted** — User shutdown still establishes sticky `Ended`; subsequent `Recovered { Unverified }` is ignored at `attention/mod.rs:54–59`. CR-09 is justified because the frozen sticky-state and cleanup-reporting requirements conflict. The unresolved behavior remains a contract violation pending that decision.
5. **not fixed; decline not accepted** — Sequence B is fixed: queued lag notices are tagged and purged (`conn.rs:126–132,327–332,394–399`). Sequence A is unchanged: writer takes E1 at line 457 → unsubscribe processes at line 327 → writer begins E1 at line 466. `wire.rs:62–65` explicitly uses processing unsubscribe as the boundary; absence of an acknowledgment does not repeal it.
6. **fixed** — The original pause before `answer` no longer exposes stopping: `ShutdownDaemon` only returns success, and `handle` queues its answer before publishing `Stopping` (`commands.rs:317–319`, `server.rs:296–303`).
7. **fixed** — In the original TurnCompleted sequence, `store_batch` now holds `Live::requests` through append, publication, and expiry; AnswerQuestion takes that same lock before validation and forwarding (`sessions.rs:316–345`, `commands.rs:237–249`). It cannot enter the demonstrated expiry window.
8. **fixed wrongly** — The original oversized sequence-4 batch now produces synthetic Error at committed sequence 4 (`sessions.rs:391–398`). Read-only rejection followed by adapter closing events still creates the analogous gap identified in round 1; see below.
9. **fixed** — The original 65,537-byte message becomes bounded deltas plus a streamed completion containing the whole message’s character count and SHA-256 (`fake.rs:191–209`).
10. **fixed wrongly** — The original single-connection flood stops admitting asynchronous work after four slots; the fifth command occupies the reader (`server.rs:283–297`). Disconnecting releases connection admission while those detached workers continue, bypassing the intended bound and retaining the daemon.
11. **not fixed; decline not accepted** — The original interleaving remains: check zero turns at `upkeep.rs:171` → start and store TurnStarted → call snapshot at line 180. “No turn running now” is evaluated before snapshot admission, and a read transaction does not establish the requested scheduling exclusion.
12. **fixed** — In the original sequence, E5 exists before SubscribeInbox begins; the new refresh reads it before the hub constructs the initial response (`server.rs:270–274`). Concurrent snapshot consistency remains unproven, as noted below.
13. **fixed wrongly** — In the original storage-refusal sequence, persistence now fails before any service change (`commands.rs:570–574`). Concurrent setters and crashes between persistence and service modification create new persistent mismatches.

## New findings

Paths below are relative to `crates/atrium-daemon/src/`.

1. [major] **Another command can overtake the initial user message** — **Where:** `sessions.rs:617–635`.
   **Sequence:** Start a native thread with `"first"`. Let the adapter return an empty command channel. `run` exposes its sender at line 617, then pause before the spawned initial-message task executes. Another authenticated client sends `ThreadCommand::SendUserMessage { text: "second" }`; `Live::send` enqueues it and returns success. Resume the spawned task: it enqueues `"first"`.
   **Result:** The adapter receives `"second"` before `"first"`, contradicting the promised first-command ordering in handoff line 124 and the launch semantics.
   **Fix:** Establish initial-message ordering before exposing the sender. Keep event pumping concurrent with a blocked initial handoff, but queue or gate later commands behind it.

2. [major] **Initial-message cancellation silently loses text, and delivery can occur after stored session end** — **Where:** `sessions.rs:627–643,658–661`.
   **Sequence A:** Start with a nonempty initial message. Leave the spawned task unpolled. The scripted adapter emits SessionStarted and closes its event channel while retaining its command receiver. The pump reaches EOF; `run` aborts the initial-message task. Its join returns cancellation, so `matches!(first.await, Ok(false))` is false. Cleanup stores SessionEnded without the missing-message Error.
   **Sequence B:** Again leave the initial task unpolled. The adapter emits SessionStarted and SessionEnded. Pause the pump after storing SessionEnded, while its ledger expiry is awaiting completion (`sessions.rs:322–345`). Run the initial task: its send succeeds into the retained command receiver after SessionEnded is already stored.
   **Result:** An accepted initial message can disappear without a recorded delivery failure, or reach a session whose end is already in history.
   **Fix:** Give the initial handoff an explicit outcome and coordinate cancellation with terminal-event storage. Treat an aborted unsent handoff as lost; prevent it from sending after terminal storage begins.

3. [major] **A failed runtime closure can be abandoned permanently by resume** — **Where:** `sessions.rs:688–720`; `commands.rs:402–438`.
   **Sequence:** A native incarnation stores SessionStarted, its native session ID, TurnStarted, and an assistant delta. Its event stream ends unexpectedly. Inject a one-shot failure into the synthetic closing append. `run` logs the failure, clears the process record at line 708, signals exit, and removes the session. Send ResumeThread: incarnation 2 starts successfully. Restart the daemon later.
   **Result:** Incarnation 1 still has open items, but recovery enumerates current incarnations and remaining process records. Its cleared record and replaced incarnation make those closing events permanently unreachable.
   **Fix:** Retain failed-closure recovery information and refuse replacement until closure succeeds. Logging must not be followed by retiring the only recovery marker.

4. [major] **Read-only filtering still commits sequence gaps** — **Where:** `sessions.rs:516–521,536–547`.
   **Sequence:** Store sequences 0–3, including an open assistant message and turn. Storage becomes read-only. The adapter emits a nonclosing delta at sequence 4; its batch is rejected. The daemon sets `closing_only` and requests shutdown. The adapter subsequently emits MessageCompleted at 5, TurnCompleted at 6, and SessionEnded at 7. These closing events are accepted with their original sequence numbers.
   **Result:** Stored history is `0,1,2,3,5,6,7`. Because SessionEnded was stored, synthetic closure never repairs the gap.
   **Fix:** Preserve a committed sequence position across rejected or filtered input. Renumber the accepted continuation, or discard that continuation and synthesize closure from committed projections.

5. [major] **Detached slow commands bypass connection bounds and outlive daemon shutdown** — **Where:** `server.rs:283–294,238–240`; `commands.rs:504`; `lib.rs:387–398`.
   **Sequence:** Register a probe that stays pending until its 15-second timeout. On connection A, send four probes and let their workers start. Disconnect A; its connection task exits and releases its connection permit, but the spawned workers retain `Inner`, `Conn`, and their permits. Repeat with new connections: each gets four fresh slots. Separately, immediately request forced shutdown from another connection with no sessions running.
   **Result:** The 32-connection limit does not bound outstanding work across disconnected clients. Shutdown can finish its connections and exhaust `Daemon::stopped`’s two-second unwrap loop while probe workers still retain the daemon and instance lock.
   **Fix:** Track workers under connection/daemon ownership, cancel appropriate work on disconnect, and await termination during shutdown. Use a shared admission bound that disconnected workers continue to consume.

6. [major] **Concurrent keep-running changes can both succeed with contradictory final state** — **Where:** `commands.rs:562–585`.
   **Sequence:** Initially the setting is false and the service absent. Client A sets true: persistence succeeds, then pause before installation. Client B sets false: it reads true, persists false, removes the absent service, and returns success. Resume A: installation succeeds and A returns success.
   **Result:** GetSettings returns false while the service is installed. Both callers received success.
   **Fix:** Serialize the complete read/persist/service/compensation operation for this setting.

   There is also a crash variant: persist true, kill the daemon before line 577, and restart. Startup contains no service reconciliation, so true remains stored while the service remains absent. Persisted desired state needs reconciliation after restart.

7. [minor] **A recovery read failure changes a completed Paused thread** — **Where:** `sessions.rs:358–374`; `upkeep.rs:87–99`.
   **Sequence:** A thread stores RateLimited followed by its closing events and SessionEnded, preserving Paused as required. Restart with a one-shot error on the `last_event` read inside `incarnation_ended`. `last_stored` converts that error to None, so `close` sends Recovered, changing Paused to Interrupted. Its subsequent `close_incarnation` read succeeds, finds SessionEnded, and returns false.
   **Result:** Startup succeeds after altering a completed thread’s state solely because an existence check failed.
   **Fix:** Return and propagate read errors from `last_stored` and `incarnation_ended`; distinguish “absent” from “could not read.”

8. [minor] **Process records can become permanently excluded from cleanup** — **Where:** `sessions.rs:521–526,705–708`; `upkeep.rs:44,73–74`; `atrium-core/src/lib.rs:201–202`.
   **Sequence:** A recorded native incarnation stores SessionEnded. Kill the daemon before `run` clears its process record. On restart, `live_processes` excludes the incarnation because it has an end. The listed-thread pass sees SessionEnded and returns without adding its record to `records`.
   **Result:** No subsequent restart clears that stored process identity. The same condition arises when recovery closes one incarnation successfully and then fails while closing another before reaching its clearing loop.
   **Fix:** Provide a way to enumerate and retire completed process records, including recovery records whose closure committed during an earlier failed startup.

9. [major] **Fake still silently drops accepted queued user messages** — **Where:** `fake.rs:103–108`; `commands.rs:457`.
   **Sequence:** Start a long-running Fake turn. Send `MAX_QUEUED_MESSAGES` subsequent messages, scheduling each to be consumed into `Agent::queue` while the current turn remains active. Send one additional distinct message and let `on_command` consume it. Its channel send succeeds, so ThreadCommand returns success, but the queue-length condition discards its text.
   **Result:** The additional message never appears in a later turn, replay, or export, and the caller receives no rejection.
   **Fix:** Make queue admission observable to the command caller, or apply backpressure before consuming messages that cannot be retained.

## Suspected, not shown

- **Concurrent initial inbox snapshot:** refreshing live threads individually and then registering under the hub mutex does not establish one atomic store snapshot (`server.rs:270–274`). The original preexisting-E5 sequence is fixed; the supplied contract does not establish enough snapshot linearization detail to classify the remaining concurrent window confidently.
- **Permanent startup failure on old-record clearing:** `recover` clears noncurrent process records at `upkeep.rs:73–74`. Whether that operation can repeatedly reject an old incarnation depends on the omitted `set_process` implementation.
- **Closure after a failed last-event read:** swallowing that read error can restart synthetic sequence allocation at zero (`sessions.rs:391–398`). Whether duplicate sequences are accepted or cause another failure depends on omitted Store validation.

## Tried and held

- **Request-lock order:** terminal batches acquire requests → publish → hub, then refresh → hub. Resolution commands acquire requests but acquire neither publish nor refresh. No reverse edge or demonstrated deadlock appears in the supplied paths.
- **Resolution lifetime:** waiting for command-channel room while holding requests is bounded by five seconds. The guard can delay terminal storage but the shown channel wait cannot hold it forever.
- **Unprotected terminal batches:** the only supplied `close_incarnation(..., None)` caller is startup recovery, before clients or sessions exist. Runtime pump and synthetic closure pass `Some(live)`.
- **Initial-task lifetime:** abort is awaited; its cloned sender and Live are released. No independent indefinite task/channel retention was demonstrated.
- **Recovery exclusions:** completed Paused threads normally return before Recovered; listed incarnation-0 threads are skipped; archived threads with current recorded processes are closed.
- **Synthetic sequence baseline:** an older-incarnation last event is ignored, and an incarnation with no events starts synthetic closure at zero. Both are correct.
- **Lag notices:** removal subtracts exactly the removed frame’s bytes. Failed resubscribe admission preserves the notice; successful replacement and explicit unsubscribe legitimately remove it. No additional needed-notice loss was demonstrated.
- **Shutdown answer:** ShutdownDaemon is not classified slow, so the corrected answer-before-stop branch applies.
- **Fake user-message encoding:** multibyte cuts preserve character boundaries and progress; concatenated deltas equal the original bytes, so the hash matches. Empty text takes the inline branch without looping.
- **Validation:** no tools or tests were run. The 43 passing daemon tests are author-reported.