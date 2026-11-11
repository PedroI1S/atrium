# Handoff: M1-05-daemon

Model: Claude Fable 5 (the orchestrator, inline; D-055, D-057) · Status: **written and tested; review pending**

## Done
- New crate `crates/atrium-daemon`: a library and the `atriumd` binary, added to the workspace. No `unsafe`. New dependencies are ones already in `Cargo.lock` (`sha2`, `libc`, `rustix`; `tempfile` and `atrium-client` for tests).
- 11 unit tests and 29 tests over a real socket in a temporary home (`tests/{socket,sessions,recovery,client}.rs`), with a scripted adapter the test drives event by event.
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
- **`ResolveApproval`: only a store failure in the ledger is `Internal`.** Every other ledger refusal is `Expired`, as the brief says.
- **`ListThreads` and `GetThread` read the store; the inbox stream uses the hub's copy.** The copy is refreshed only when something a summary shows changed, so its `last_event` and `updated_at_ms` lag during streaming. That is what keeps streaming from producing an update per batch.
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
Run on Linux (CachyOS, kernel 7.3), at `23a8a89`:
- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace`: see the numbers at the end of this file.
- `cargo test -p atrium-daemon`: 40 passed (11 unit, 9 `socket`, 16 `sessions`, 3 `recovery`, 1 `client`).

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
- Measured before the last two changes to how a session is closed, which don't touch these paths.

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

## For M1-13 (adversarial tests)
- D-058's rule is tested for one replaced subscription and one `Unsubscribe`. Not tested: many replacements in a row, a replacement while live.
- Not tested: a store read that fails during a replay (the connection is closed), a full control lane (8 MiB closes the connection), `Hello` sent a byte at a time across the deadline.
- `kill_recorded` returning `Unverified` at start-up for a *recorded* process is not tested (only the no-record case is).
- The Linux `App` path is tested with a development identity (`dev_ino` set), not with a root-owned packaged path.

## Gotchas
- A scripted or real session that ignores `Shutdown` holds the daemon's stop for 17 s (15 s of patience, then 2 s).
- `tempfile` creates folders 0755 on this machine; `secure_dir` refuses that for a runtime directory that already exists. The harness sets 0700 itself.
- `Signal::Recovered { Unverified }` must be sent before the closing events are stored. The `SessionEnded` among them makes the state sticky, and a later signal is ignored. The first version had this the wrong way round, in the session's end and in recovery.
