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
- **Library** (`spike_daemon_socket`): a frame codec (async read/write per `wire.rs`, rejecting frames over `MAX_FRAME_LEN`), and a `Client` with `connect`, `hello`, `subscribe(thread, after) -> Stream<ServerMessage>`, `command(DaemonCommand) -> CommandResult`, and automatic reconnect that resubscribes from the last seen `EventId`.
- **`spike-daemon` bin**:
  - Socket and lock file in a directory passed by `--dir`, default `$TMPDIR/atrium-spike`. Mind the ~104-byte `sun_path` limit on macOS.
  - Single instance: a second daemon exits with a clear message.
  - Store: `rusqlite` (WAL) with an append-only `events(id INTEGER PRIMARY KEY, thread_id, envelope_json)` table, written by one writer task. `EventId` = the row id.
  - Runs `AgentLaunch::Fake` agents as **child processes** (`spike-fake-agent` bin, emitting `AgentEventEnvelope` NDJSON on stdout). `Native` returns `CommandResult::Err` in M0.
  - Records agent PIDs. On startup it reaps orphans left by a crashed daemon and appends an `Error { fatal: true }` + `SessionEnded` to their threads (PLAN: `Interrupted`).
- **`spike-client` bin**: starts the daemon detached if it isn't running (no `unsafe`: use `CommandExt::process_group`), starts a fake thread, follows it, and exits on Ctrl-C while the agent keeps running.

## Definition of Done
- [ ] Tests:
  - frame round-trip and oversized-frame rejection;
  - replay boundary: events appended *during* a subscribe's replay are delivered exactly once, in order (run it many times, or as a property test);
  - reconnect: kill the client mid-stream, reconnect with `after`, assert there's no gap and no duplicate;
  - single instance;
  - orphan reaping after `kill -9` of the daemon.
- [ ] Measurements in the report, each with the command that reproduces it:
  - reconnect-to-caught-up time with 100k stored events (budget < 100 ms);
  - sustained event throughput through one client;
  - daemon idle RSS (budget < 50 MB).
- [ ] `cargo fmt --check` · `cargo clippy -p spike-daemon-socket --all-targets -- -D warnings` · `cargo test -p spike-daemon-socket`
- [ ] Spike report `docs/spikes/M0-05-daemon-socket.md`, plus a short "how M0-07 uses the client" section.
- [ ] Handoff written.

## Out of scope
Tauri, real agent adapters, launchd/systemd units, remote transports, binary stream frames beyond the codec (just test that they round-trip).

## Escalate if
- The replay/live boundary can't be made exactly-once without a global lock on the writer. Explain the trade-off in the report, then stop.
- You need a contract change → `docs/agents/change-requests.md` and stop.
