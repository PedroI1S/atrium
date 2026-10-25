# M0-05-daemon-socket: daemon ↔ client over a unix socket, with replay

Model: GPT-6.1 Sol · Reviewer: Sonnet 5.5 · Depends on: none · Wave 1
Branch: `agent/sol/M0-05-daemon-socket` · Worktree: `../atrium-wt/M0-05-daemon-socket`

## Goal
Prove the process model in PLAN §3.1: a background daemon owns the agent processes and an event store, clients come and go, a reconnecting client gets exactly the events it missed, and agents keep running while no client is attached.

## Context
- PLAN §3.1 (daemon and client), §3.3 (storage: SQLite WAL, append-only events, single writer), §4 budgets (reconnect with full state < 100 ms, daemon idle RSS < 50 MB).
- Contract: `crates/atrium-protocol/src/wire.rs` (framing, handshake, subscribe/replay, commands, the `Fake` agent), `crates/atrium-protocol/src/agent.rs`.
- M0-07 (stream perf, wave 2) builds a Tauri client on top of your client library, so keep the client API small and documented.

## Owned paths
- `spikes/daemon-socket/**`
- `docs/spikes/M0-05-daemon-socket.md`
- `docs/agents/handoffs/M0-05-daemon-socket.md`

## Contract to implement (do not modify)
- **Library** (`spike_daemon_socket`): a frame codec (async read/write per `wire.rs`, rejecting oversized frames, unknown kinds and invalid JSON), and a `Client` with `connect`, `hello`, `subscribe(thread, after)`, `command(DaemonCommand) -> Result<CommandResult, UnknownOutcome>`, and automatic reconnect.
  - **Cursor** (per `wire.rs`): the consumer calls `client.applied(thread, event_id)` after applying an event; reconnect resubscribes from the last *applied* id, and the subscription stream drops ids at or below it.
  - **No automatic command retry**: a connection lost before a response returns `UnknownOutcome`.
- **`spike-daemon` bin**:
  - Socket and lock file in a directory passed by `--dir`, default `$TMPDIR/atrium-spike`. Mind the ~104-byte `sun_path` limit on macOS.
  - **Socket security** (per `wire.rs`): create the directory 0700 (or verify an existing one is a real directory, not a symlink, owned by the user, mode 0700; refuse otherwise), socket 0600, take the lock before removing a stale socket.
  - Single instance: a second daemon exits with a clear message.
  - Store: `rusqlite` (WAL) with an append-only `events(id INTEGER PRIMARY KEY, thread_id, envelope_json)` table, written by one writer task. `EventId` = the row id.
  - `Subscribe` handling per `wire.rs`: a response with `replay_until`, errors for unknown threads and cursors past the end, re-subscribe replaces.
  - Runs `AgentLaunch::Fake` agents as **child processes** (`spike-fake-agent` bin, emitting `AgentEventEnvelope` NDJSON on stdout). `Native` returns `CommandResult::Err` in M0.
  - Records each agent's **PID and process start time** (plus the executable path). On startup it reaps orphans left by a crashed daemon only when all three still match; an ambiguous process is left alone and logged. It appends the closing events + `SessionEnded` to those threads (PLAN: `Interrupted`).
- **`spike-client` bin**: starts the daemon detached if it isn't running (no `unsafe`: use `CommandExt::process_group`), starts a fake thread, follows it, and exits on Ctrl-C while the agent keeps running.

## Definition of Done
- [ ] Tests:
  - frame round-trip; rejection of oversized frames, unknown kinds and invalid JSON;
  - replay boundary: events appended *during* a subscribe's replay are delivered exactly once, in order (run it many times, or as a property test);
  - reconnect in **both windows**: (a) events read off the socket but not yet applied when the client dies, (b) a command sent and executed but its response lost. Assert no gap, no duplicate applied, and `UnknownOutcome` for (b);
  - subscribe errors: unknown thread, cursor past the end, re-subscribe;
  - socket security: refuses a symlinked dir, a dir owned by someone else (simulate by mode/owner check), a wrong mode; socket file is 0600;
  - single instance;
  - orphan reaping after `kill -9` of the daemon, and a **PID-identity mismatch** (stored start time differs) that must not be signalled.
- [ ] Measurements in the report, each with the command that reproduces it. Fixed parameters: 100k stored events in one thread, each a `MessageDelta` of 200 characters.
  - reconnect-to-caught-up time with the cursor **1,000 events behind** (budget < 100 ms) and with **no cursor** (full replay; reported, no budget);
  - sustained event throughput through one client;
  - daemon idle RSS after the 100k-event run and 60 s idle (budget < 50 MB).
- [ ] `cargo fmt --check` · `cargo clippy -p spike-daemon-socket --all-targets -- -D warnings` · `cargo test -p spike-daemon-socket`
- [ ] Spike report `docs/spikes/M0-05-daemon-socket.md`, plus a short "how M0-07 uses the client" section.
- [ ] Handoff written.

## Out of scope
Tauri, real agent adapters, launchd/systemd units, remote transports, binary stream frames beyond the codec (just test that they round-trip).

## Escalate if
- The replay/live boundary can't be made exactly-once without a global lock on the writer. Explain the trade-off in the report, then stop.
- You need a contract change → `docs/agents/change-requests.md` and stop.
