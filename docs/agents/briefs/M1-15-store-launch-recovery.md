# M1-15-store-launch-recovery: CR-08 and CR-10 (two `Store` methods, resume and recovery), CR-09, and the resume race N1

Model: Gemini 4 Hydrogen (`gemini-4-hydrogen-high`, `scripts/agent-run agy`; D-074, D-076, D-090; was Sonnet 5.5) · Reviewer: Opus 5.5 (read-only) · Depends on: M1-02, M1-05, M1-16 (merged)
Go: D-067 (rodrigo approved CR-08 and CR-10), D-082 (Pedro approved CR-09), D-090 (Pedro: start now, ahead of M1-13's merge) · Branch: `agent/hydrogen/M1-15-store-launch-recovery`

**Started before M1-13's merge (D-090).** M1-13's adversarial tests (`crates/atrium-daemon/tests/adversarial_*.rs`, its recovery tests and finding 12's test) are **not on `dev` yet**: they are on `agent/sonnet/M1-13-adversarial`. Read them there with `git show agent/sonnet/M1-13-adversarial:<path>` if you need them, but don't copy them in. The orchestrator runs them against your branch when M1-13 merges. Don't touch `crates/atrium-core/src/policy/**` (M1-13 is changing it). · Worktree: `../atrium-wt/M1-15-store-launch-recovery`

## Goal
Apply the three approved change requests: a resumed thread keeps the effort and preset it was started with (CR-08); the daemon's recovery finds every unfinished incarnation, archived threads included (CR-10); a thread whose cleanup can't be verified is never shown as stopped (CR-09).

## Context
- `docs/agents/change-requests.md`: CR-08, CR-09 and CR-10 (read all three in full; they say what M1-05 does today as a workaround).
- D-067, D-082 and D-090 in `docs/agents/decisions.md`; contract §8 (the sentence marked "rev 3.1, CR-09").
- `docs/contracts/m1.md` §5 and §10 (lifecycle, recovery), §7 (the store).
- `crates/atrium-core/src/lib.rs` (the `Store` trait), `crates/atrium-core/src/store/**` (`SqliteStore`, its reader and writer), `crates/atrium-daemon/src/upkeep.rs` (`recover`), and where the daemon resumes a thread (`ResumeThread`, the `SessionConfig` and `SessionGate` it builds).
- `docs/agents/handoffs/M1-05-daemon.md` ("open": CR-08, CR-10), `docs/agents/reviews/M1-05-daemon-codex*.md` (findings 2 and 8).
- `docs/agents/reviews/M1-14a-linux-host.md`, the last section ("The daemon's flaky tests (N1–N3): causes"): Argon's reading of N1 (the window runs from the moment `pump` stores `SessionEnded` to the end of `run()`, three store round-trips later), of N2, and of N3 (a killed leader stays a zombie in its process group until its parent reaps it, so `group_is_empty` is false and `kill_recorded` answers `Unverified`). Check each against the code before you rely on it.
- `docs/agents/handoffs/M1-13-adversarial.md`, finding 12: `upkeep::recover()` records `Signal::Recovered` before `close_incarnation`; if the closure fails, `live_processes()` skips that session on the next start and `sessions.process` stays set.

## Owned paths
- `crates/atrium-core/src/lib.rs`: the two new trait methods only, exactly as approved:
  - `Store::launch(&ThreadId) -> StoreResult<Option<NewThread>>` (the row as it was created, with the current title);
  - `Store::unfinished_incarnations() -> StoreResult<Vec<(ThreadId, u32)>>` (every thread's current incarnation that has events and no `SessionEnded`, archived or not).
- `crates/atrium-core/src/store/**` and `crates/atrium-core/tests/store*.rs`: their implementation and tests.
- `crates/atrium-core/src/attention/**` and its tests (`crates/atrium-core/tests/attention*.rs` if they exist): CR-09 only.
- `crates/atrium-daemon/src/upkeep.rs` and the resume path in `crates/atrium-daemon/src/**`: use the two methods, remove the workarounds.
- `crates/atrium-daemon/tests/**` except `adversarial_*.rs` (M1-13's; read them, and if one must change because the behaviour it pins was the workaround, say so in the handoff and change only that assertion).
- Every other implementer of `Store` in the workspace (test doubles): the two methods, nothing else.
- `docs/contracts/m1.md`: only the sentences that describe the two methods and the resume and recovery behaviour, marked as CR-08/CR-10 (D-067).
- `docs/agents/handoffs/M1-15-store-launch-recovery.md`

## What to do
0. **CR-09.** In `attention::next_state`, `Signal::Recovered { cleanup: Unverified }` moves `Ended` and `Interrupted { cleanup: Verified }` to `Interrupted { cleanup: Unverified }` (`Failed` keeps its message and stays `Failed`). Add the rows to the transition table test. Then check the daemon: where M1-05 sends the `Unverified` signal before the closing events as a workaround (CR-09's text), keep the order if it is still right and say so; add a daemon test where a user stop is followed by an unverified cleanup and the thread ends `Interrupted { Unverified }`.
1. **CR-08.** Implement `Store::launch`. On `ResumeThread`, build the new incarnation's `SessionConfig` and `SessionGate` from the stored effort and preset instead of the Home preset with no effort. Tests: a thread started with an effort level and a non-Home preset resumes with both; an unknown thread gives `None`; the title returned is the current one.
2. **CR-10.** Implement `Store::unfinished_incarnations`. `upkeep::recover` uses it to close every unfinished incarnation, archived threads included, instead of reading each listed thread's last event. Tests: a daemon killed between "stop the session" and "archive" leaves an archived thread; the next start closes its open messages, tool calls and turn and stores `SessionEnded`; a process record whose incarnation already ended is cleared.
3. **Finding 12.** With the new method, make a failed closure in recovery retried on the next start without leaving `sessions.process` set for an ended session. If that needs a change outside the owned paths, stop and report it instead.
4. **N1 (`m1-must-fix.md`, from M1-14a): a resume right after `Ended` is refused.** `sessions.rs` `run()` publishes the final state before it removes the thread from `Inner::sessions`, and `launch()` refuses a thread still in the map. Fix the order (or let `launch` take over a finished session) so that a client that sees `Ended`, `Failed` or `Interrupted` can resume at once, without ever running two sessions for one thread. Test: resume in a tight loop right after the final state, 200 rounds. Then run `cargo test -p atrium-daemon` 30 times in a row on the host: `a_stale_approval_is_expired_and_never_reaches_the_session` must no longer fail.
5. **N2 and N3:** fix the inbox test's ordering race (N2, test-side). For N3, establish whether the daemon's drain or the test is early when `kill_recorded` meets a zombie `sleep`, with evidence; fix it if the cause is in your paths, otherwise report it.
6. No schema migration unless one is truly needed; if it is, follow M1-02's migration rules and say why in the handoff.

## Definition of Done
- [ ] The two methods with tests in `atrium-core`; the resume and recovery tests in `atrium-daemon`.
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test -p atrium-core -p atrium-daemon -p atrium-client`; `scripts/check-protocol-ts` (the wire must not have changed).
- [ ] The contract's wording updated, and nothing else in the contract.
- [ ] Handoff with outputs, and what the removed workarounds were.

## Rules
- macOS host, `agy` worker policy: `cargo`, `git status/diff/log/add/commit/show`, `rg`, `grep`, `ls`, `cat` and `scripts/check-protocol-ts` are fine; shell substitutions, `~`, `../` and shell loops are refused. For the 30-run and 200-round checks, write the loop inside a test (or say in the handoff that the orchestrator must run it).
- No test data in shell heredocs (worker rule 6). Stop every daemon you start. Commit on your branch only; never push, merge or rebase.

## Out of scope
CR-12 (a separate policy task), the wire protocol, the app, anything in `policy/**`.

## Escalate if
A method's signature has to differ from the approved one; the wire or the schema has to change; an M1-13 adversarial test fails for a reason that isn't the removed workaround.
