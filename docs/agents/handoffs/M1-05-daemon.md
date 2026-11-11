# Handoff: M1-05-daemon

Model: Claude Fable 5 (the orchestrator, inline; D-055, D-057) · Status: **partial** — written, tested, reviewed twice by Codex (rework both times) and fixed after each; **not merged**: the Gemini 3.8 Flash review D-056 asks for could not be run (see "Next step")

## Next step
1. **The driver decides who reviews.** `agy` is not logged in inside the container this session ran in (it asks for a Google sign-in), so the Flash review of D-056 was not run. Either sign in and run it:
   ```
   git worktree add --detach ../atrium-wt/M1-05-review agent/fable/M1-05-daemon   # already exists on rodrigo's machine
   scripts/agent-run agy gemini-3.8-flash-medium M1-05-review .orchestration/prompts/M1-05-review.md
   ```
   or accept the Codex reviews below in its place (they are cross-provider, as PLAN §8.2 asks). Either way the fixes of round 2 have not been reviewed by anyone. The prompt is on rodrigo's machine only; it asks for an exact failing sequence for every claim, area by area.
2. Fix what the review finds, then merge `agent/fable/M1-05-daemon` into `dev`, update the status row, remove the two worktrees.
3. Decide CR-08, CR-09 and CR-10 (`docs/agents/change-requests.md`), and the notification question under "Not done".

## Review (2026-11-11)
GPT-6.1 Sol through Codex, read-only, with the sources inlined in the prompt because Codex's own sandbox doesn't start inside the container: **rework**, 13 findings (`docs/agents/reviews/M1-05-daemon-codex.md`). It could not run the tests and did not see the test files. A second round on the fixes (`M1-05-daemon-codex-round2.md`) was **rework** again; see "Round 2" below.

Fixed on the branch:
| Finding | Fix |
|---|---|
| 1. The first user message could block the session's task for ever | It is handed over beside the pump, for at most 5 s; if it isn't taken the session is stopped and closed with an error. Test: `a_session_that_takes_no_commands_can_still_be_stopped`. |
| 2. Recovery skipped unfinished incarnations in a settled state | Recovery closes every listed thread whose current incarnation has no `SessionEnded`, whatever its state. Test: the crash test's thread that the user had stopped. Archived threads: CR-10. |
| 3. A closure that failed in recovery counted as done | `close_incarnation` returns the error; recovery fails the start, and clears process records only at its end. At runtime a failed closure is logged and the next start closes it. |
| 5. A queued `SubscriptionLagged` outlived an `Unsubscribe` | The notice is tied to its thread and removed when the thread is subscribed to again or unsubscribed from. Test: `conn::a_queued_lag_notice_goes_with_its_subscription`. |
| 6. `ShutdownDaemon`'s answer could be lost | The stop is requested by the connection's reader after the answer is queued. |
| 7. A resolution could reach the session after its turn's end was stored | A per-session lock is held while events that end requests are stored and the ledger follows, and while a resolution is checked and passed on. Not tested: the window can't be hit on purpose. |
| 8. A refused batch left a gap in the sequence numbers | Closing events continue from the last stored event. Tested in `an_event_the_store_refuses_fails_the_session`. |
| 9. The synthetic agent cut user messages at 64 KiB | Longer ones are deltas and a `Streamed` completion. Tested. |
| 10. No limit on slow commands | Four per connection run beside the reader; more run in it, so the connection waits. Not tested. |
| 12. A new inbox subscriber got stale summaries | Threads with a session are refreshed before the answer. Test: `a_new_inbox_subscriber_gets_current_summaries`. |
| 13. A failed setting write left the login service changed | The setting is stored first; if the service can't be changed, the setting is put back. Not tested (tests never touch the service). |

Not fixed in round 1:
- **4. A thread already stopped can't become "cleanup unverified"** (sticky states in `attention::next_state`). It needs a contract change: **CR-09**.
- **5, first half** (an event the writer had already taken is written after an `Unsubscribe`). Declined: `Unsubscribe` has no answer, so a client can't tell this from an event sent just before it; what a client can observe is a later response, and the frame is written before any.
- **11. A snapshot can start just before a turn does.** Declined: the check is "no turn running now"; a snapshot is one read transaction and doesn't hold the turn up.

### Round 2
The reviewer confirmed findings 1, 6, 7, 9 and 12 as fixed, accepted the escalations (CR-09, CR-10), and did **not** accept the two declines (5 first half, 11). Those two stay declined and are **the driver's to settle**: closing them means either making `Unsubscribe` wait for a frame the writer has already taken (which lets a client that doesn't read block its own commands), or holding all appends during a snapshot.

Fixed after round 2:
| Finding | Fix |
|---|---|
| Round 1's 3, new 3: a closure that failed at runtime was lost once the thread was resumed | The process record is kept when a closure fails, and `ResumeThread` closes the incarnation before first, refusing the resume if that fails. Not tested (needs a store that fails once). |
| Round 1's 8, new 4: closing events accepted while storage is full kept the adapter's numbers | Once a batch was refused for full storage, the events kept are renumbered from the last stored one. Tested in `full_storage_interrupts_running_turns_and_refuses_new_ones`. |
| Round 1's 10, new 5: slow commands outlived their connection and the daemon's stop | The limit is daemon-wide (8), and a slow command is given up when its connection goes or the daemon stops. An export that was given up still finishes its file. Not tested. |
| Round 1's 13, new 6: two changes of the keep-running setting at once; a daemon killed between the setting and the service | One change at a time; at start the service is made to say what a stored setting says. Not tested. |
| New 1: another client's command could reach the session before the thread's first message | The session's command sender isn't given out until the first message was handed over or given up. Until then other commands are `NotRunning`. Not tested (a race). |
| New 2, first half: a first message given up because the session ended wasn't recorded | It is recorded as an error when the daemon closes the incarnation. |
| New 7: a read that failed in recovery could turn a finished `Paused` thread into `Interrupted` | The reads return their errors and the start fails. |
| New 9: the synthetic agent dropped messages past its queue without a word | It emits `Error { fatal: false }` for each, as `atrium-agents` asks of adapters. Test: `the_synthetic_agent_says_when_it_does_not_queue_a_message`. |

Not fixed after round 2:
- **New 2, second half:** a first message can still be put in the command channel of a session whose `SessionEnded` was just stored. Nothing reads that channel any more, so it has no effect.
- **New 8:** a process record whose incarnation ended, left by a daemon killed before it cleared the record, stays in the store. `Store::live_processes` leaves ended incarnations out, so nothing lists it and nothing uses it. Folded into CR-10.
- A second inbox subscriber's refresh and its answer are not one snapshot ("Suspected, not shown").

## Done
- New crate `crates/atrium-daemon`: a library and the `atriumd` binary, added to the workspace. No `unsafe`. New dependencies are ones already in `Cargo.lock` (`sha2`, `libc`, `rustix`; `tempfile` and `atrium-client` for tests).
- 12 unit tests and 32 tests over a real socket in a temporary home (`tests/{socket,sessions,recovery,client}.rs`), with a scripted adapter the test drives event by event.
- A measurement harness for contract §14 (`examples/perf.rs`) and a `perf` profile in the root `Cargo.toml`.

## How it is built
| File | What it holds |
|---|---|
| `lib.rs` | `Daemon`, `Options`, the shared state (`Inner`), start and stop. |
| `server.rs` | The socket, the handshake and roles, each connection's reader, each subscription's feeder. |
| `conn.rs` | One connection's two outgoing lanes and its subscriptions. |
| `hub.rs` | Who is told what: live subscribers, inbox subscribers, the thread list, focus, notifications. |
| `sessions.rs` | Starting an incarnation, pumping its events into the store, closing what it left open. |
| `commands.rs` | Every `DaemonCommand`. |
| `upkeep.rs` | Recovery at start, the storage watch, daily snapshots, `CommandNotifier`. |
| `fake.rs` | The synthetic agent behind `AgentLaunch::Fake`. |
| `main.rs` | `atriumd [--dev-socket]`, `SIGTERM` and `SIGINT`. |

**The replay/live boundary.** An append and the publishing of what it stored happen under one lock (`Inner::publish`). A subscription is fed from the store by its own task, which waits for room in the connection's event lane and so is never dropped for being slow. It goes live under the same lock, after a read that finds nothing more. So every event reaches a subscriber once, from the store or from the hub. `ReplayDone` is queued as soon as the feeder passes `replay_until`, never while holding the lock.

**Lanes.** The control lane is written before the event lane. Queued `ThreadUpdated` and `InboxUpdated` frames are replaced by newer ones, so a client that reads slowly gets the latest, not a backlog. A live event that doesn't fit the event lane drops that subscription with `SubscriptionLagged`. Every queued event frame carries its subscription's id, and replacing or ending a subscription removes its frames, which is what makes D-058's rule hold.

**Sessions.** One task per session owns the adapter's event receiver. After each stored batch the ledger follows (contract §5). If the adapter doesn't end its own session (it failed to start, its stream stopped, it didn't stop within 15 s, or the store refused a batch), the daemon closes the incarnation from `Store::open_items`, in the contract's order.

**Stopping.** `SIGTERM` (or `Daemon::stop(false)`): the daemon keeps serving until no turn is running and refuses new turns meanwhile. `SIGINT`, `ShutdownDaemon`: connections get their queued answers and are closed, sessions are told to stop and their closing events are stored, then the socket and the lock go.

## Decisions I made that the brief didn't spell out
- **A process the adapter left behind is checked, not assumed gone.** When the daemon closes an incarnation itself, it runs `supervise::kill_recorded` on the recorded process. If the result is `Unverified`, the thread becomes `Interrupted { cleanup: Unverified }`. Before this, such a thread showed as stopped.
- **A thread that was `Starting` when the daemon died, with no recorded process, is recovered as `Unverified`.** The process may have been spawned and not yet recorded. This can show a "cleanup unverified" inbox item when nothing was ever started.
- **A batch the store refuses (other than for full storage) ends the session** with a fatal error, instead of carrying on with a hole in the history.
- **`ResolveApproval`: a store failure in the ledger is `Internal`.** Every other ledger refusal is `Expired`. The brief says "every ledger refusal"; this is the one exception.
- **`ListThreads` and `GetThread` read the store; the inbox stream uses the hub's copy.** The copy is refreshed only when something a summary shows changed, so `last_event` and `updated_at_ms` in a `ThreadUpdated` lag during streaming. That is what keeps streaming from producing an update per batch. The answer to `SubscribeInbox` is current.
- **Titles.** A thread's title is the first line of its first user message, spaces collapsed, at most 120 characters. A thread started without a message is "New thread" until its first message.
- **Settings.** `keep_agents_running` (a boolean) installs or removes the login service; every other key is stored as given.
- **A connection whose write makes no progress for 30 s is closed.** The client reconnects from its cursor.

## Not done, and why
- **Notifications use `notify-send` (Linux) and `osascript` (macOS) behind the `Notifier` trait, not `notify-rust`** as PLAN §3.4 names. It avoids a D-Bus dependency in the daemon. For the driver to confirm or reverse.
- **A release daemon accepts no client yet.** `App` needs an identity compiled in, which exists once the app is packaged (M1-09a). Debug builds read `Paths::app_identity`.
- **`ProbeProviders` returns one entry per registered adapter**, always with `enforcement: Some`. The probe returns a descriptor, not "the CLI isn't installed", so `enforcement: None` can't be produced here. For M1-07 and M1-08.
- **`MAX_SUBSCRIPTIONS_PER_CONNECTION` is tested at the unit level only** (`conn.rs`), not over a socket with 257 threads.
- **CR-08 is undecided**, so a resumed thread runs with the Home preset and no effort setting.
- **macOS:** nothing here was run on macOS. The code has no platform branches except `CommandNotifier`.

## Definition of Done results
Run on Linux (CachyOS, kernel 7.3), after the round-2 fixes:
- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace`: 266 passed, 0 failed, 1 ignored (`consistency_all_fixtures`, owned by M1-07 and M1-08).
- `cargo test -p atrium-daemon`: 44 passed (12 unit, 9 `socket`, 19 `sessions`, 3 `recovery`, 1 `client`), three runs in a row.

Each Definition of Done item and its test:
| Item | Test |
|---|---|
| Replay then live, events arriving during the replay | `socket::replay_then_live_delivers_every_event_once_and_in_order` (8 subscribers, 30,000 events) |
| Resubscribing while a replay is queued | `socket::nothing_of_a_replaced_subscription_follows_the_new_answer` |
| A client that doesn't read | `socket::a_reader_that_stops_lags_alone` |
| A stale approval | `sessions::a_stale_approval_is_expired_and_never_reaches_the_session` |
| A session that dies without closing events | `sessions::a_session_that_dies_is_closed_in_order` |
| A daemon killed with a live process | `recovery::a_killed_daemon_is_recovered_at_the_next_start` (a second process, killed with `SIGKILL`) |
| Role checks, `Hello` timeout, oversized frame, too many connections | `socket::a_role_is_granted_or_the_connection_is_refused`, `the_app_identity_file_must_be_private`, `a_silent_connection_is_closed_at_the_hello_deadline`, `a_frame_that_is_not_one_closes_the_connection`, `connections_are_limited` |
| `atrium-client` against the real daemon | `client::the_client_and_the_daemon_work_together` |

`App` is tested for real on Linux: the test binary connects as `App` and is verified through `SO_PEERPIDFD` and `/proc/<pid>/exe`.

### Contract §14
```
cargo build --profile perf -p atrium-daemon --bin atriumd --example perf
target/perf/examples/perf
```
The `perf` profile is `release` with debug assertions on, which is what lets the daemon accept the harness as a `Dev` client. One run, Linux, 8 cores, the store on tmpfs:

| Budget | Result |
|---|---|
| Reconnect, cursor 1,000 events behind, 20 runs (gate: p95 < 100 ms) | min 2.13 ms, median 2.76 ms, **p95 3.20 ms**, max 3.44 ms |
| Full replay of 100,000 events (reported) | 152 to 156 ms, about 650,000 events a second |
| Idle RSS, 10 samples from 60 s after a 3,000,000-token session was closed (gate: median < 50 MB) | **median 19.4 MiB**, max 19.4 MiB |
| Peak RSS during the load, and right after the close (reported) | 15.1 MiB, 15.3 MiB |

- A reconnect is timed from `connect` to `ReplayDone`, with the events counted and not decoded. The client's own share is in M1-06's handoff.
- The 3M-token session is 3,000,000 one-token deltas, as in the M0 measurements, stored at about 200,000 events a second (the synthetic agent's rate, not the store's limit). The store file was 839 MiB.
- The daemon's first daily snapshot (60 s after it starts) ran inside the idle window, which is why the idle figure is above the one right after the close.
- Measured before the review fixes. They add a lock per batch that ends a request and a read per live thread on `SubscribeInbox`; the numbers were not taken again.

## For M1-07 and M1-08 (the adapters)
- Register an adapter with `Options::adapters`. Until one is registered, `AgentLaunch::Native` for its provider is `Invalid`.
- The first command a session receives is `SendUserMessage` with the message the thread was started with, if there was one.
- End the session yourself (`SessionEnded` after the closing events). If you don't, the daemon closes the incarnation and runs `kill_recorded` on your process. A killed leader that nobody reaps still counts as a member of its group, so the result is then `Unverified`: reap the child.
- `on_spawn` is recorded in the store by a task; `Session::process` is recorded too.
- An event over `MAX_EVENT_BYTES` loses its whole batch (up to 256 events) and ends the session as failed.
- `SessionConfig::env` and `program` are empty; `mcp_servers` is empty in M1.
- A remembered rule is seen by the session's gate from its next check (tested).

## For M1-09a (the app shell)
- `Daemon::start(Options)`; `main.rs` shows the whole wiring. The dev app writes `Paths::app_identity` (mode 0600) with `peer::current_app_identity()` before it connects.
- After `ShutdownDaemon` the answer arrives and then the connection closes.
- `ExportThread` needs an absolute path. `Invalid` covers "something is already there", "the folder doesn't exist" and "the folder isn't yours"; the message says which.
- While the daemon is draining after `SIGTERM`, `StartThread`, `ResumeThread` and `SendUserMessage` are `Invalid`.
- A `SendUserMessage` while the thread is still `Starting` is `NotRunning`: the session takes commands once the adapter has started. The message a thread is started with is the exception (it is sent as soon as the session exists).

## For M1-13 (adversarial tests)
- D-058's rule is tested for one replaced subscription and one `Unsubscribe`. Not tested: many replacements in a row, a replacement while live.
- Not tested: a store read that fails during a replay (the connection is closed), a full control lane (8 MiB closes the connection), `Hello` sent a byte at a time across the deadline.
- `kill_recorded` returning `Unverified` at start-up for a *recorded* process is not tested (only the no-record case is).
- Not tested, from the review: a closure that fails in recovery (the start must fail and the next one must close it), the request lock, the slow-command limit, the keep-running setting when the service can't be changed.
- The Linux `App` path is tested with a development identity (`dev_ino` set), not with a root-owned packaged path.

## Gotchas
- A scripted or real session that ignores `Shutdown` holds the daemon's stop for 17 s (15 s of patience, then 2 s).
- `tempfile` creates folders 0755 on this machine; `secure_dir` refuses that for a runtime directory that already exists. The harness sets 0700 itself.
- `Signal::Recovered { Unverified }` must be sent before the closing events are stored. The `SessionEnded` among them makes the state sticky, and a later signal is ignored. The first version had this the wrong way round, in the session's end and in recovery.
