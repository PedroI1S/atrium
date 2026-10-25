# M0-05: daemon socket, persistence and reconnect

Fix round 1 implements all fourteen accepted review findings. The frozen wire
contract is unchanged. The pull-based replay design remains: one WAL writer,
short indexed reads, and a watch channel used only to wake subscribers. The
current suite has 25 passing tests. Reconnect latency remains below 100 ms in
the measured runs. Memory exceeds 50 MB shortly after the live run; a later
macOS reclaim makes the 60-second sample much smaller. That is not evidence of
a portable steady-state memory budget pass.

## Architecture

`spike-daemon` validates the private directory (real directory, current UID,
0700), takes a 0600 lock before stale-socket cleanup, binds a staging socket,
sets it to 0600, then renames it to the public socket path. Clients verify the
directory and socket ownership/mode before connecting, including on reconnect.
A second daemon prints a clear error and exits 1 without disturbing the first.

One dedicated writer thread owns the rusqlite WAL connection and a bounded job
queue. Events are appended with global SQLite row IDs. Only successful event
appends notify subscribers; metadata/read jobs do not. ListThreads uses an
indexed correlated `max(id)` subquery for each thread instead of joining all
historical events. Each connection reads at most 128 rows per page and finishes
its read statement before awaiting socket output.

Subscribe snapshots `replay_until`, sends the response, pages through that
boundary, emits ReplayDone, then reads higher IDs. The notification receiver
exists before the snapshot. Commits during replay or between an empty query and
waiting are retained in SQLite and cannot be lost by coalesced notifications.
Re-subscription replaces per-thread delivery state. No global writer lock is
needed during replay.

Fake agents are real child processes with a startup barrier: PID, native start
time and executable path are committed before they emit. Each child has its
own bounded stdin writer task; the children-map mutex is released before any
write. A stalled child cannot hold other children's stdin locks. On restart,
identity mismatch/unavailability fails closed. A matched orphan is signalled;
ESRCH means it exited between inspection and signalling and does not abort
startup. Open lifecycle objects are closed in message/tool/request/turn order,
then SessionEnded. Native launches return CommandResult::Err.

The frame codec assembles one buffer per write and uses buffered readers in
both connection implementations. It rejects invalid lengths/kinds/JSON and
short binary payloads. The exact MAX_FRAME_LEN limit is accepted; oversized
control and binary writes fail before writing bytes.

## ClientHandle and M0-07

`Client` is now a convenience facade over an actor. `into_parts()` gives a
cloneable `ClientHandle` and one `EventStream`. The actor owns socket I/O,
requests, reconnect and resubscription. Dropping `next()` only drops a channel
receive; it cannot interrupt a handshake or leave a partially restored client.
Connected is published only after every restore subscribe succeeds.

The handle offers `command`, `subscribe`, `reconnect`, `applied` and a watch
receiver from `connection_state`. The request queue holds 16 entries; the event
channel holds 32, with a 2 MiB serialized-byte permit budget. A single legal
frame larger than that budget occupies all permits (wire maximum 16 MiB).
There is also a one-entry network reader channel, one pending decoded delivery,
and the reader's in-progress frame. These are bounded transport buffers, not a
claim that all Rust allocations or consumer memory fit in 2 MiB. Subscription
state grows with the number of subscribed threads. Caller-owned command values
are checked against the frame limit before the socket write.

Drain the event stream concurrently with requests. A slow consumer intentionally
backpressures socket reads, so a command response behind replay events may wait
for the consumer. There is no unbounded command-wait event deque. Cancelling a
request waiter does not retract a request already accepted by the actor.

Transient connection I/O errors retry with exponential delay (50 ms up to 1 s).
Bad framing, incompatible/bad handshakes, restore-subscribe errors, and permission
errors are terminal. Ordinary subscribe rejections return CommandResult::Err
and keep the connection usable. ConnectionState exposes Reconnecting, Connected, Failed and Closed.
ReplayDone repeats after **every** reconnect. Commands queued while disconnected
wait for restoration. Failures before a command write starts are
CommandError::NotSent; failures after it starts are UnknownOutcome. Commands are
never automatically retried. Frame validation failures also return NotSent and
leave the connection usable.

M0-07 can use the following shape (ordinary error handling omitted):

```rust,ignore
let client = Client::connect_or_spawn(dir, daemon_exe, "m0-07").await?;
let (handle, mut events) = client.into_parts();
let statuses = handle.connection_state();
let render_handle = handle.clone();
let rendering = tokio::spawn(async move {
    while let Ok(message) = events.next().await {
        if let ServerMessage::Event { event } = message {
            // This check belongs in the UI/webview too, before changing state.
            if event.id <= last_ui_applied(&event.thread_id) { continue; }
            apply_to_ui_and_wait_for_ack(&event).await;
            render_handle.applied(&event.thread_id, event.id);
        }
        // ReplayDone is a per-reconnect catch-up marker, not a one-time event.
    }
});
handle.subscribe(thread, persisted_cursor).await?;
let result = handle.command(DaemonCommand::ListThreads).await?;
```

The UI must dedupe `id <= last_ui_applied`, including across delayed webview ACKs.
Persist the applied cursor with the corresponding UI state for app restarts.
Keep a handle alive while consuming. Drop the stream to stop the actor. Use
ListThreads/replayed state after UnknownOutcome before deciding to send a new
command. `connect_or_spawn` starts a detached process group only for an absent or
refused endpoint, waits for readiness, and captures stderr in `dir/daemon.log`
(0600). The demonstration CLI creates its Ctrl-C future once and detaches without
stopping the agent. Build all three binaries next to one another:

```sh
cargo build -p spike-daemon-socket --bins
target/debug/spike-client --dir /tmp/atrium-demo
```

## Fix round 1

Review: `docs/agents/reviews/M0-05-daemon-socket.md` in the main checkout. All
findings are addressed; none required changing a frozen contract.

| Finding | Change and evidence |
|---|---|
| 1 | Replaced the EPIPE-prone fake in the recovery test with pipe-independent `/bin/sleep 60` processes and real persisted identity rows. The matching process must terminate by SIGKILL; the start-time mismatch survives. With the kill temporarily disabled, this test failed with `recovery did not kill the pipe-independent orphan: Elapsed(())`; restoring it made the test pass. The original fake-agent test did not prove daemon-issued reaping. |
| 2 | Reconnect runs in the actor. Test checkpoints pause at connect, Hello write/read, each resubscribe write/read, output budget/channel waits, and retry backoff. At each checkpoint a polled `next()` is dropped; restoration still completes and Connected is withheld until all subscriptions succeed. |
| 3 | Classify retryable I/O separately, use capped exponential backoff, return permanent handshake/protocol/subscribe errors, and expose a watch state. Four permanent-error scenarios are tested. Repeated ReplayDone is documented above. |
| 5 | Ship bounded ClientHandle/EventStream, applied-cursor filtering, and generation filtering for stale queued deliveries. A consumer-pressure test queues a command behind events and completes it while draining; UI dedupe guidance and an integration example are above. |
| 10 | Validate client-side directory/socket ownership and mode; publish via bind/chmod/rename; lock 0600. Tests reject symlinks, wrong mode and simulated wrong UID, and prove the original daemon remains usable after a failed second start. |
| 6 | Three sequential runs sample RSS and macOS phys_footprint at 0/3/60 seconds. Diagnostics count active client SQLite readers and report lifecycle scan size. Early budget failures and reclaim dependence are reported below. The live number is a generator-limited 10k/s keep-up exercise, not maximum throughput. |
| 4 | Wait for reconnect before dequeuing/writing commands. The real-daemon proxy drops a successful StartThread response, pauses reconnect, queues ListThreads, then verifies successful delivery and exactly one thread. Terminal reconnect returns NotSent for that queued command. Oversized command validation also returns NotSent. |
| 7 | A response/first-page barrier forces appends to commit during replay. Assert the old response boundary and new DB tail before releasing it; run 25 rounds on each of current-thread and multi-thread runtimes. Added in-flight re-subscribe and <= applied-cursor tests. |
| 8 | Added complete Client replacement with only a persisted consumer cursor; assert every ID exactly once. Automatic reconnect now verifies new event continuity as well as the repeated ReplayDone marker. Removed the stale failed-command comment. |
| 9 | ESRCH at signalling is success, independently tested; other signal errors still propagate. Process-group cleanup (`killpg`) remains an M1 follow-up, as requested. |
| 12 | Independent bounded stdin writers with short map-lock ownership; a deliberately blocked writer cannot block another. Indexed max-ID subquery for ListThreads. Store test proves read/metadata jobs do not notify subscribers. |
| 13 | One assembled frame write and BufReader reads. Exact maximum-length binary round trip and binary/control write-side oversize tests. |
| 11 | Human-readable `spike-daemon: ...` error and exit code 1; second-instance test asserts both. |
| 14 | Commit history corrected in the handoff; one Ctrl-C future; shared library connect_or_spawn; private daemon log. Integration test starts a daemon, reconnects without spawning, and verifies log permissions. |

## Validation

```sh
cargo fmt --check
cargo clippy -p spike-daemon-socket --all-targets -- -D warnings
cargo test -p spike-daemon-socket
```

All pass: 5 library tests and 20 integration tests, zero failed. All binary and
doc-test targets also pass. New tests use readiness lines, oneshot channels,
semaphores, wire responses, or durable-state conditions as synchronization.
Timeouts bound failures; test sleeps are not used as synchronization. The
production reconnect timer is exercised through a checkpoint.

This sandbox uses `CARGO_HOME=/tmp/atrium-m005-cargo` for a writable Cargo cache;
prefix Cargo commands with that assignment here. macOS phys_footprint uses the
safe `libproc` wrapper, whose build requires the native Clang tooling available
on this machine. No unsafe code was added to the spike.

## Measurements

Reproduce the complete three-run measurement, including diagnostic output:

```sh
cargo build -p spike-daemon-socket --release --bins --example measure
target/release/examples/measure target/release/spike-daemon 3
```

The last argument is now **run count**, not idle seconds. Every run samples at
0, 3 and 60 seconds. Each uses a fresh daemon/database with exactly 100,000
stored MessageDelta events in one thread, each 200 ASCII characters. Fixture
seeding occurs outside the measured daemon. Connect-to-caught-up starts before
connect/Hello and ends at ReplayDone after each event has been acknowledged.
Stored throughput is 100,000 divided by full-replay duration. Runs are sequential,
optimized Rust 1.99.1, Darwin 27.0.0 / arm64, WAL synchronous=NORMAL, local socket.
These are individual samples, not percentile estimates.

| Run | 1,000-behind catch-up | Full replay | Stored events/s | Generator deltas / elapsed | Observed live delivery/s |
|---|---:|---:|---:|---:|---:|
| 1 | 10.259 ms | 340.060 ms | 294,066.1 | 49,988 / 5.247 s | 9,526.2 |
| 2 | 3.454 ms | 335.905 ms | 297,702.9 | 49,984 / 5.074 s | 9,850.5 |
| 3 | 3.676 ms | 334.229 ms | 299,196.1 | 49,986 / 5.063 s | 9,872.4 |

All three 1,000-behind measurements pass the <100 ms budget. Full replay has no
budget in this brief.

| Run | Idle sample | RSS (MiB) | phys_footprint (MiB) | RSS <50,000,000 bytes? |
|---|---:|---:|---:|---|
| 1 | 0 s | 89.125 | 83.251 | **FAIL** |
| 1 | 3 s | 89.125 | 83.251 | **FAIL** |
| 1 | 60 s | 23.125 | 4.751 | pass |
| 2 | 0 s | 89.016 | 83.110 | **FAIL** |
| 2 | 3 s | 60.375 | 54.454 | **FAIL** |
| 2 | 60 s | 9.250 | 4.813 | pass |
| 3 | 0 s | 89.047 | 83.094 | **FAIL** |
| 3 | 3 s | 47.562 | 28.344 | pass |
| 3 | 60 s | 9.344 | 4.719 | pass |

[Raw output and diagnostics](../../spikes/daemon-socket/measurements/fix-round-1-macos.txt)
retain the byte counts; MiB above means 1,048,576 bytes. The pass/fail column
uses the stricter decimal 50 MB threshold. The 60-second samples pass **on this
machine**, while the immediate working set misses the budget in every run.

The live phase configures Fake for 10,000 deltas/s (500,000 tokens/s, 200 chars,
5,000 ms, no tools). Its observed delivery rate includes final-message/session
closure and drain time. This tests keeping up with that generator; it does not
identify maximum sustainable live throughput. The idle clock starts after the
session's durable `running=false` projection and client drop. RSS excludes the
benchmark consumer and child. Physical footprint is macOS accounting, not RSS;
its changes and reclamation policy must not be extrapolated to Linux.

### Memory investigation

After the 100k replay alone, daemon RSS is 10.875–10.984 MiB and footprint is
about 5.1–5.2 MiB. The live thread's lifecycle scan then materializes
49,989–49,993 envelopes, containing 19,993,600–19,995,200 bytes of message text
(deltas plus the completed message). During that scan, RSS is about 85.7 MiB;
after folding/releasing history it is about 89.0–89.1 MiB. The fold builds a
second concatenated message and temporarily overlaps the history vector,
complete-message payload, and serialization/frame buffers.

The diagnostic `reader_closed readers=0` entries confirm all client SQLite
connections are dropped while RSS remains roughly 89 MiB. The writer's cache
target is 2 MiB; client-reader page caches disappear with their connections.
The local lifecycle reader is scoped to close_thread and is gone after that
operation finishes. Consequently, retained client connections cannot explain
the idle 3-second excess. The source-level allocation lifetimes and the sharp
later decline in both RSS and footprint point to allocator/VM retention and
reclaim after the large transient allocations. This is an inference supported
by phase measurements, not an allocation-by-allocation heap profile. No claim
is made that every retained byte has been assigned to an allocator arena.

Physical footprint falls from approximately 83 MiB at 0 seconds to about
4.7–4.8 MiB at 60 seconds. The varying 3-second results show reclaim timing is
not stable even on this machine. A small final RSS sample does not establish
that live workload memory fits under 50 MB.

No allocator purge, forced memory pressure, cache flush, or shortened workload
was used to make a sample pass. The original report's 27.5 MiB at 60 seconds was
one historical sample and its general memory-budget conclusion is withdrawn.
Linux has not been measured; its allocator/OS may retain the early high RSS.

## Recommendation and remaining limits

Keep the WAL writer, indexed pull replay, and applied-cursor wire semantics.
The actor is the M0-07 integration surface. Reconnect latency passes locally,
but memory work is still required before claiming a portable <50 MB budget.
Persist open-object projections or fold history incrementally instead of
materializing it on session closure; profile live allocations/allocator arenas
before selecting a memory remedy.

Process start identity still has second granularity and inspection/signal has
a PID-reuse window; use finer native identity and process handles in M1. Only
the matched leader PID is killed; add identity-safe process-group cleanup for
agents that spawn descendants. SQLite reads still run synchronously in connection
tasks. Child ingestion commits one event per transaction; batching is a later
throughput option. Very long completed messages can exceed the 16 MiB wire
limit. Handshake, restore subscribe and writes have 5-second I/O deadlines;
ordinary command-response waiting can still be delayed by backpressure or a
nonresponsive daemon. Production needs explicit cancellation/deadline policy
and Linux validation. `synchronous=NORMAL` covers this process-crash spike, not
a power-loss durability guarantee.
