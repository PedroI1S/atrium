# M0-05: daemon socket, persistence and reconnect

Implemented in `spikes/daemon-socket`, with library `spike_daemon_socket` and
`spike-daemon`, `spike-client`, and `spike-fake-agent` binaries. The frozen M0
contract is unchanged. All eleven tests pass.

## What was built

The codec implements the big-endian length/kind framing and typed JSON control
messages, including opaque binary stream frames. Zero lengths, oversized frames,
unknown kinds, short stream payloads, invalid UTF-8/JSON and invalid typed messages
are errors. Connection handlers close on invalid incoming frames. Persistent
reader tasks prevent cancellation of a partial frame when another operation wins
an async select.

The daemon validates its private directory (real directory, current UID, exactly
0700), checks the portable socket path length, acquires the lock before removing
a stale socket, and sets the bound socket to 0600. A second instance fails with
an explicit lock error. The default directory is `$TMPDIR/atrium-spike`; pass a
short `--dir` when necessary on macOS.

A single dedicated writer thread owns the rusqlite WAL connection. Its bounded
job queue accepts event batches and thread metadata updates. Committed SQLite
row IDs are the global EventIds. Events are append-only in the implementation;
thread status and process identity are separate projections. No client owns an
agent: fake agents run as child processes with piped NDJSON output and stdin
commands, in their own process groups. Native launches return a protocol error.
The CLI starts a daemon in a separate process group when needed, follows a fake
thread, and handles Ctrl-C as a detach.

### Replay/live boundary

Each connection creates a watch receiver before accepting subscriptions. A
successful subscribe snapshots the thread's maximum committed ID, responds with
`replay_until`, and reads pages of 128 rows up to that ID. It sends ReplayDone
before reading any higher IDs. Live delivery uses the same indexed query with
the last sent ID as its exclusive lower bound. The watch channel only wakes
readers; it never carries event data. A notification coalesced while replay is
in progress is harmless because SQLite contains every committed row. A commit
between an empty query and waiting also leaves a pending watch change.

SQLite read statements finish before socket writes await. Replay never holds a
writer mutex or a long-lived SQLite snapshot. No global lock on the writer is
needed. Re-subscription replaces the connection's per-thread state. Unknown
threads and cursors beyond a thread's maximum ID receive error responses.

### Crash recovery

The child waits for a startup barrier until its PID, native process start time,
and executable path are committed. Identity comes from `sysinfo` native process
inspection. On restart, the daemon signals an orphan only if all three match;
unavailable or mismatched identity is logged and left alone. Persisted events
reconstruct open objects, which are closed in message → tool → request → turn
order, with Interrupted turns, then SessionEnded. Completed sessions do not get
a second ending. Normal children are waited/reaped by their owning task; after
a daemon SIGKILL, the old child is adopted by the OS, which performs the final
wait after the replacement daemon kills it.

## Validation

Run from the workspace root:

```sh
cargo fmt --check
cargo clippy -p spike-daemon-socket --all-targets -- -D warnings
cargo test -p spike-daemon-socket
```

Results: formatting clean, Clippy clean with warnings denied, eleven tests
passed, zero failed; binary and library unit/doc-test targets also passed.
The integration suite covers:

- Control/binary round trips and malformed-frame rejection.
- Twenty-five replay/live races with concurrent appends, exact ID ordering,
  exactly one ReplayDone, and no missing or duplicate events. The fixture and
  in-process daemon share the same writer actor.
- A read-but-unapplied event, transport loss across a daemon crash/restart,
  replay from the consumer's applied cursor, and no duplicate applied IDs.
- Automatic reconnect and resubscribe on socket EOF.
- A proxy forwarding a real StartThread command to the daemon, dropping its
  successful response, and checking UnknownOutcome and exactly one thread.
- Subscribe errors/replacement, handshake ordering/version mismatch, directory
  symlink/mode/UID validation (UID mismatch simulated through the same metadata
  validator), socket mode 0600 and a second process losing the instance lock.
- SIGKILL of a daemon with a running fake child, successful orphan cleanup and
  lifecycle closure, plus a persisted start-time mismatch whose unrelated live
  process survives recovery.
- Agents producing events and finishing while all clients are disconnected.
- Closing order, sequence continuity, idempotent ending and pre-session failure.

The global Cargo cache is read-only in this worker sandbox. Builds/checks used
`CARGO_HOME=/tmp/atrium-m005-cargo`, a writable temporary cache with links to
already available packages. Prefix the commands above with that assignment in
this session. Cargo's generated root lockfile changes are excluded from this
worker's deliverable; integration must resolve the additional dependencies.

## Measurements

One local optimized run, Darwin 27.0.0 / arm64, Rust 1.99.1, SQLite WAL with
`synchronous=NORMAL`, no injected socket latency. These are individual samples,
not percentile claims. Seeded fixture: exactly 100,000 events in one thread;
every event is a MessageDelta containing 200 ASCII characters. Fixture creation
happens outside the measured daemon. The example asserts both replay counts and
acknowledges each delivered event with `applied`.

Reproduce every number below with:

```sh
cargo build -p spike-daemon-socket --release --bins --example measure
target/release/examples/measure target/release/spike-daemon 60
```

Use the Cargo cache assignment above if running inside this worker's sandbox.

| Measurement | Result | Method / budget |
|---|---:|---|
| Connect to caught up, cursor 99,000 | 8.737 ms | Monotonic timer before connect/Hello through ReplayDone after applying 1,000 events; <100 ms passes |
| Connect to caught up, no cursor | 339.988 ms | Same timer through all 100,000 events; no budget |
| Sustained stored-event delivery, one client | 294,127.7 events/s | 100,000 / full-replay elapsed time, including connection and application acknowledgment |
| Live child → WAL → one client | 9,522.4 deltas/s | 49,878 200-character deltas received in 5.238 s, through TurnCompleted; configured target 10,000/s, no tool calls |
| Daemon RSS after 60 s idle | 28,835,840 bytes (27.500 MiB) | Native process resident-memory reading via sysinfo; <50 MB passes |

The live run follows the 100k replay run and adds a second short-lived thread.
It uses Fake with 500,000 tokens/s, 200 characters/chunk, 5,000 ms duration and
zero tool interval. Its throughput includes the final complete-message event
and drain time in the denominator, but counts only deltas in the numerator.
RSS is sampled after both runs and a full 60-second idle wait; it excludes the
benchmark consumer and fake child. Stored throughput and live throughput measure
different paths and are reported separately.

## What broke and remaining risks

The sandbox refuses to execute `ps`; replacing shell process inspection with
native `sysinfo` inspection made identity tests and RSS measurement work here.
Git cannot create the external worktree metadata's index lock, so the worker
cannot stage or commit. This does not block code, tests or measurements.

This is a local process-model spike. Process start timestamps currently have
second granularity, and identity comparison plus signalling is not atomic with
PID reuse. M1 should use higher-resolution native identity and platform process
handles where available. Inspection errors fail closed today.

Recovery scans thread history and reconstructs message text, so recovery peak
memory/time grows with history; persist an open-object projection in M1. Normal
child ingestion currently commits one event per transaction, whereas fixture
seeding batches 1,000; batching live ingestion is an M1 throughput opportunity.
SQLite reads run synchronously on connection tasks with short pages; move them
to a dedicated reader pool for heavier concurrent clients. `synchronous=NORMAL`
is sufficient for process-crash recovery here; power-loss durability requires a
separate product decision.

The codec enforces the 16 MiB frame limit. Very long synthetic messages can
produce a complete-message event larger than that limit; M1 needs bounded
message sizes or a separate transfer strategy. Connection request buffering is
bounded, but events accumulated while a Client waits for a command response are
not byte-budgeted. There are no request deadlines. Automatic reconnect retries
at 50 ms and assumes the same compatible daemon/store remains available;
production needs cancellation, error classification and backoff. Linux behavior
has not been measured in this macOS run.

## Recommendation for M1

Keep the wire protocol, applied-cursor semantics, single WAL writer, and indexed
replay with notifications as wakeups. Both measured budgets pass locally. Add
persistent lifecycle projections, bounded byte budgets, explicit operation
cancellation/deadlines, stronger native process identity and Linux validation
before integrating real adapters.

## How M0-07 uses the client

Build the three binaries first when using the detached demonstration CLI:

```sh
cargo build -p spike-daemon-socket --bins
target/debug/spike-client --dir /tmp/atrium-demo
```

M0-07 can depend on `spike-daemon-socket` by path. `Client::connect(socket, name)`
performs Hello; `hello()` is idempotent on an established connection.
`command(DaemonCommand)` returns a wire CommandResult or UnknownOutcome.
`subscribe(thread, after)` returns the wire response with replay_until. Then
consume `next().await` for ServerMessage::Event and ReplayDone, applying events
before calling `applied(&thread, event.id)`. The same next loop reconnects and
re-subscribes from applied cursors. Persist cursors with the corresponding UI
state when surviving a complete app restart. Explicit `reconnect()` discards
unapplied transport buffers and retains applied cursors.

Use one owner task for the mutable client. `next()` is cancellation-safe; avoid
cancelling command/subscribe operations during writes. The rendering consumer
should acknowledge only after the state update is complete. After UnknownOutcome,
reconnect and use ListThreads or replayed state before deciding whether to issue
a new command. The client never retries commands automatically. Binary frames
are available through the codec; the high-level Client handles control messages.
