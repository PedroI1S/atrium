# Review: M1-15-store-launch-recovery (commit cc8978d)

Reviewer: Claude Opus 5.5, read-only (D-090, D-094). Author: Gemini 4 Hydrogen `high`. Base `5175a92`. Everything ran in one scratch clone of cc8978d (deleted afterwards); M1-13's daemon adversarial tests were copied in from `ccb1b60` for one run only.

## Verdict: approve with changes

CR-08, CR-09, CR-10 and finding 12 are implemented as approved, the signatures match, the wire is unchanged, and the N1 design is sound: it can't run two sessions for one thread, and a resume's wait is bounded by the old session's own stop patience. One regression must be fixed first: the rewritten `upkeep::recover` no longer closes a listed thread whose current incarnation has **no events** and whose state is not `Starting` (a thread the user stopped while it was still starting, then the daemon died). Such an incarnation is left without a `SessionEnded`, and when its recorded process can't be verified the thread stays `Ended`, which is exactly what CR-09 and contract §10 forbid. The base code closed it. A reviewer probe test fails on cc8978d and passes with the base `upkeep.rs`.

## 1. Scope: OK
- Every changed file is in the brief's owned paths: `atrium-core` `lib.rs` (the two methods only), `store/{mod,reader,writer}.rs`, `attention/mod.rs`, `tests/{attention,store}.rs`; `atrium-daemon` `src/{commands,sessions,upkeep}.rs`, `tests/{recovery,sessions,gaps_snapshot}.rs`; the handoff; the contract.
- No `policy/**`, no schema, no migration, no `adversarial_*.rs`, no `atrium-host`.
- `gaps_snapshot.rs`'s `GatedStore` (the only other `Store` implementer) only delegates the two new methods.
- Contract: two sentences, one marked CR-08 (§6.2 `ResumeThread`, m1.md:117) and one marked CR-10 (§10 Recovery, m1.md:161). Nothing else changed. §8's CR-09 sentence was already there (rev 3.1).
- **[minor] writer.rs:980:** `SessionEnded` now also sets `sessions.process = NULL`. It is in an owned path and is correct (see §5), but it is a store behaviour that neither CR-10 nor the contract sentence names; say it in the CR-10 sentence or `Store::set_process`/`live_processes` docs.

## 2. CR-09: OK
- `next_state` (attention/mod.rs:54-71): `Recovered { Unverified }` moves `Ended` and `Interrupted { Verified }` to `Interrupted { Unverified }`; `Failed` falls through to the sticky check and keeps its message; `Interrupted { Unverified }` stays. Exactly CR-09.
- Table row `"UUUUUU=U=U"` (attention.rs:148) against `STATES = "SRAQIPFXUE"`: F `=`, X→U, U `=`, E→U. Right. Plus a dedicated test with a `Failed` message.
- Daemon test `a_stopped_session_with_unverified_cleanup_becomes_interrupted` (sessions.rs:1566): user stop → `Ended`, the stream dies, the zombie `sleep` gives `Unverified`, the thread ends `Interrupted { Unverified }` with `session_ended` stored and a `CleanupUnverified` inbox item. Without the `next_state` change it would stay `Ended`, so it pins CR-09 end to end.
- **M1-05's order is still right** (signal before closing events, sessions.rs:840-845 and upkeep.rs `close`). With CR-09, `Ended` and `Interrupted { Verified }` no longer depend on it, but `Failed` still does: signal first gives `Interrupted { Unverified }` from a live state, closing first would let the fatal `Error` settle `Failed`. Keeping it is the safer choice; the handoff says so.
- **[nit] attention/mod.rs:29-31:** `next_state`'s doc still says `Ended` and `Interrupted` are sticky with no exception; add the CR-09 line.

## 3. CR-08: OK
- `Store::launch(&ThreadId) -> BoxFuture<'_, StoreResult<Option<NewThread>>>` (lib.rs:197): the approved signature in the trait's async form.
- `reader::launch` (reader.rs:124-163) reads `provider, model, effort, preset, location, title` from `threads`; `title` is the current one (rename updates the column).
- `resume_thread` (commands.rs:498-517) builds `Launch` from it, so `session_config` (`SessionConfig.effort`, `permissions(preset)`) and `session_gate` (`SessionGate` preset) both get the stored values. The Home/`None` workaround and its comment are gone.
- Tests: `launch_reads_stored_launch_fields_and_current_title` (unknown → `None`, all fields, title after rename); `resume_keeps_effort_and_non_home_preset` checks model, effort, `PermissionPolicy::FullAccess` and the gate's decision on both incarnations.
- **[nit] commands.rs:472-478:** the adapter is still picked from `summary.provider` while `Launch.provider` comes from `stored.provider`. They can't differ today; use one source.

## 4. CR-10 and finding 12
- `Store::unfinished_incarnations() -> BoxFuture<'_, StoreResult<Vec<(ThreadId, u32)>>>` (lib.rs:209): approved signature. The SQL (reader.rs:185-213) joins `threads` to `sessions` on the **current** incarnation, needs ≥1 event and no `session_ended` event, ignores `archived`. Matches CR-10 ("every thread's current incarnation that has events and no `SessionEnded`, archived or not, from the sessions table").
- `recover` (upkeep.rs:36-92): kills every `live_processes` record first, closes every `unfinished_incarnations` entry (archived included) with the kill's result or, without one, a cleanup derived from the stored state; clears all records only after every closure succeeded. A failed closure fails the start and keeps the records.
- **Finding 12 is fixed:** `Signal::Recovered` sets `ended_ms` (writer.rs:525-528), so on the next start `live_processes` skips the session, but `unfinished_incarnations` still lists it (no `SessionEnded`), it is closed, and its record is cleared (upkeep.rs:63-67). The retried closure takes its cleanup from the state the first attempt's signal left (`Interrupted { cleanup }`), so an `Unverified` is kept. Test `failed_recovery_closure_retries_and_clears_process_record` pins it, including the record count before and after.
- Archived case: `an_archived_unfinished_thread_is_closed_on_next_start` covers a thread archived mid-turn (no process record) and a store-built archived thread with an open tool call; both end with the full closing sequence and no record left.
- **[major] upkeep.rs:72-84 — regression: a listed incarnation with no events and a non-`Starting` state is never closed.**
  - Before, the second loop closed every listed thread with `incarnation > 0` and no `SessionEnded` (`Starting` → `Unverified`, else `Verified`). Now `unfinished_incarnations` skips zero-event incarnations (as approved) and the fallback loop only closes `Starting` ones.
  - Reachable: the user stops a thread while its provider is still starting (`Signal::Stopped` → `Ended`, no event stored yet, the process may already be recorded by `on_spawn`), then the daemon dies. Same for `StorageReadOnly` during a start.
  - Effect: the process is killed (or not) and the record cleared, but no `Recovered` signal and no `SessionEnded` are stored. With an unverifiable process the thread stays **`Ended`**, which breaks CR-09 and §10 ("never shown as stopped"); in every case the incarnation has no `SessionEnded`.
  - Evidence: a probe test (store-built: thread, incarnation 1, `Starting`, a recorded process whose start time doesn't match a live `sleep`, `Stopped`; then boot) gives `state=Ended history=[]` on cc8978d. With only `upkeep.rs` reverted to `5175a92` it passes: `session_ended` and `Interrupted { Unverified }`.
  - Fix: in the fallback loop close every listed `incarnation > 0` not in `handled` (as before), with the kill's result when there is one, else `Unverified` for `Starting` and `Verified` otherwise. `close` already skips an incarnation whose `SessionEnded` is stored. Add the probe as a recovery test.
- **[minor]** An archived thread whose current incarnation has no events (archived while starting, then a crash) is still never closed. That is what CR-10 approved ("has events"), so it is not a defect of this task; note it in the handoff as the remaining gap.
- **[minor] upkeep.rs:22-33:** the doc comment still says "every listed thread"; it should say every unfinished incarnation (archived included) plus the zero-event fallback.
- **[minor]** No test for "a process record whose incarnation already ended is cleared" (brief, step 2). The writer change prevents new ones (store test checks `process` is NULL after `SessionEnded`), but a record left by an older build (ended, `process` set) is listed by neither method and stays forever. Harmless (nothing reads it); say so in the handoff.

## 5. The writer change (`SessionEnded` sets `process = NULL`): correct, transactional, no recovery path lost
- It is in the same `UPDATE sessions` statement inside `apply`'s transaction (writer.rs:977-985), so the event and the cleared record commit together.
- Crash after `SessionEnded` is stored but before the group is killed: nothing changes. `live_processes` already required `ended_ms IS NULL`, and the same `SessionEnded` sets `ended_ms`, so before this change such a record was invisible to recovery too. In the daemon's own path the kill comes first: `run()` calls `kill_recorded` before `close_incarnation` stores `SessionEnded` (sessions.rs:826-855), and recovery's `close` stores it after `kill_recorded` as well. When the adapter stores `SessionEnded` itself, the daemon never killed anything afterwards, before or after this change (`run()` only checks the group when `pumped.ended` is `None`); cleanup there is the adapter's (contract §10: soft stop, `SIGKILL` the group, reap).
- It also fixes a small leak: `run()`'s failed-closure path keeps the record for "a resume or the next start"; a resume's `close_incarnation` now clears it.

## 6. N1: sound
- **Never two sessions for one thread.** `launch` inserts only when the map has no entry (sessions.rs:241-263); a terminal entry is waited on, a live one refused. `SessionGuard` removes only its own `Arc` (ptr_eq) and then sets `done`. Waiters re-check in a loop under the map lock, so of two concurrent resumes one inserts and the other sees a non-terminal session and gets `Invalid` (tested).
- **No lost wake-up.** `finished()` subscribes to a `watch` and `wait_for` checks the current value first.
- **The wait is bounded, but only by construction.** A session is marked terminal only on paths that end it: `Live::stop` (pump deadline `STOP_PATIENCE` = 15 s), a stored `SessionEnded` (pump breaks), a fatal `Error` (now followed by `stop()`), full storage (`stop()`), the pump's exit. After the pump, `run()` does at most a 2 s `kill_recorded` drain and a few store calls. Worst case a `ResumeThread` answers after ~17 s instead of `Invalid` at once.
- `launch`'s early error path calls `finish()` too, and `SessionGuard` covers panics inside `run()`.
- **[minor] sessions.rs:404-406:** `stop()` after a stored fatal `Error` is new behaviour: a session that reports a fatal error is now shut down even if the adapter meant to go on. It is what makes "terminal" mean "ending", so it's reasonable, but it isn't in the brief or the handoff's list of behaviour changes; say so.
- **[minor] sessions.rs:217-231, 258:** the wait has no timeout of its own; a store call that hangs in the old session's teardown hangs the resume. A `timeout(STOP_PATIENCE + …)` that answers `Invalid` would bound it independently.
- **[nit] upkeep.rs:142-155:** `watch_storage` publishes `Interrupted` before `live.stop()` marks it terminal, so the N1 window remains there. Not reachable in practice (a resume is refused while storage is full by `can_start_turns`).
- The 201-round test covers `Ended` (exit 0), `Interrupted` (exit 1), `Failed` (start failure) and a user stop with the provider finishing asynchronously, each followed by an immediate resume, and the concurrent double resume.

## 7. N2 and N3
- **N2: the fix is sound for the race it fixes.** The test now waits for each `ThreadUpdated` on the watcher's own stream instead of draining `aside` behind a `ListInbox` barrier, which didn't order the two lanes.
  - **[minor] sessions.rs:839-843:** the "39 more deltas send no update" check still drains `aside` after a `ListInbox` barrier. It can no longer fail spuriously, but a late spurious update would be missed, so the assertion is weaker than before. Acceptable; say so.
  - **[minor] sessions.rs:858-863:** the MarkRead/Rename section still relies on the barrier to have the `ThreadUpdated` in `aside`. The update is queued before the watcher's `ListInbox` is sent, so it is unlikely to lose, but it is the same pattern; it passed 30 of 30 here.
- **N3: the analysis is right in substance but has no new evidence.** The handoff repeats Argon's reading (an unreaped zombie keeps the group non-empty past the 2 s drain; Linux `posix_spawn`/vfork exe race) and adds `rig.stop` calls. Reading the code: the daemon is not early, its `Unverified` is the correct answer while a zombie is in the group; the sessions test is the one that assumes its own reaper task runs within 2 s. So N3 is test-side timing plus `atrium-host`, outside this task. No N3 failure in 30 runs here.
  - **Relation to N10:** `kill_recorded` doesn't use `terminate`, so N10's `terminate` fix won't touch it, but it has the same macOS shape: `group_is_empty` counts only `ESRCH` as empty, and `signal_group(SIGKILL)` returning `EPERM` gives `Unverified` at once (supervise.rs:261-266, 311-315 on dev). On XNU a group whose leader is exiting or a zombie answers `EPERM`, so the macOS form of N3 is the N10 race in `kill_recorded`. Whoever fixes N10 should look at `kill_recorded` too.

## 8. M1-13's daemon adversarial tests against cc8978d
Copied from `ccb1b60` (`adversarial_recovery.rs`, `adversarial_socket.rs`; they need nothing else, `common/mod.rs` is unchanged on M1-13), 4 runs:
- `adversarial_recovery`: 3 of 3 pass (`crash_child`, `recovery_with_mismatched_recorded_start_time_is_unverified_and_spares_process`, `failed_closure_in_recovery_fails_start_and_next_start_closes_it`). The last one is finding 12's three-stage scenario; it passes, and its retry expectations match the new code.
- `adversarial_socket`: 7 of 7 pass.
- No failure, so no assertion pinned a removed workaround.

## 9. Toolchain (scratch clone of cc8978d)
- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace --no-fail-fast`: 511 passed, 0 failed, 1 ignored (the pre-existing `consistency` one).
- `scripts/check-protocol-ts`: `protocol.ts is up to date` (wire unchanged).
- `cargo test -p atrium-daemon` 30 runs in a row: **30 of 30 passed** (no failure of `a_stale_approval_is_expired_and_never_reaches_the_session`, N2's or N3's tests; no daemon or `sleep` left running).
- `resume_right_after_final_state_in_a_tight_loop` (201 rounds): 40 of 40 passed (30 inside the loop above, 10 more on its own, 0.26-0.94 s each).

## 10. Trial merge of dev (`7ef8dcc`) into cc8978d
- **One conflict, text only:** `docs/contracts/m1.md` §6.2. M1-15 rewrote the `ResumeThread` bullet (CR-08); dev changed the next bullet, `DeclineQuestion` (erratum D-091). Resolution: M1-15's `ResumeThread` sentence plus dev's `DeclineQuestion` sentence. The orchestrator must resolve it the same way at merge.
- No code conflict (dev's `atrium-host/supervise.rs` and Codex adapter don't touch M1-15's files).
- On the merged tree: fmt clean, clippy clean, `cargo test --workspace --no-fail-fast` 572 passed, 0 failed, 0 ignored; `scripts/check-protocol-ts` up to date.

## Mutation results (3 runs each)

| Mutation | Tests that fail | Result |
|---|---|---|
| Recovery clears each process record right after the kill, before the closures (upkeep.rs, first loop) | `recovery::failed_recovery_closure_retries_and_clears_process_record` (recovery.rs:298, record count) and M1-13's `adversarial_recovery::failed_closure_in_recovery_fails_start_and_next_start_closes_it` (:239), 3 of 3 | caught |
| `unfinished_incarnations` skips archived threads (`AND t.archived = 0`) | `store::unfinished_incarnations_lists_open_current_incarnations_archived_or_not` (store.rs:2069) and `recovery::an_archived_unfinished_thread_is_closed_on_next_start` (recovery.rs:406), 3 of 3 | caught |
| `resume_thread` ignores the stored preset (`Preset::Home`) | `sessions::resume_keeps_effort_and_non_home_preset` (sessions.rs:1652), 3 of 3 | caught |

Extra probe (not a mutation): the stopped-while-starting crash case of §4 fails on cc8978d and passes with the base `upkeep.rs`.

## Findings
- **[major] upkeep.rs:72-84:** zero-event, non-`Starting` listed incarnations are no longer closed after a crash; an unverified process then leaves the thread `Ended` (CR-09/§10 regression). §4.
- **[minor] writer.rs:980:** `SessionEnded` clearing `process` isn't documented. §1, §5.
- **[minor] upkeep.rs:22-33:** stale `recover` doc. §4.
- **[minor]** No test/handoff note for a pre-existing ended-but-recorded process (older builds). §4.
- **[minor]** Archived zero-event incarnation still not closed (as CR-10 approved); note it. §4.
- **[minor] sessions.rs:404-406:** fatal `Error` now stops the session; undocumented behaviour change. §6.
- **[minor] sessions.rs:217-231, 258:** resume wait has no timeout of its own. §6.
- **[minor] sessions.rs:839-843, 858-863:** N2 test keeps two barrier-based checks (one weaker, one same pattern). §7.
- **[minor]** N3: no new evidence; the macOS form is N10's `EPERM` race in `kill_recorded`. §7.
- **[nit] attention/mod.rs:29-31:** `next_state` doc lacks the CR-09 exception. §2.
- **[nit] commands.rs:472-478:** adapter chosen from `summary.provider`, launch from `stored.provider`. §3.
- **[nit] upkeep.rs:142-155:** `watch_storage` publishes `Interrupted` before marking terminal. §6.

## Asked before merge
1. Fix the zero-event regression in `recover` (close every listed unfinished `incarnation > 0`, as before) and add the stopped-while-starting crash test (state `Interrupted { Unverified }` with an unverifiable process, `session_ended` stored).
2. Document `SessionEnded` clearing `process`, the fatal-`Error` stop, and the archived zero-event gap (handoff; the CR-10 contract sentence for the first).
3. Fix the `recover` and `next_state` doc comments.
