# Handoff: M0-05-daemon-socket

Model: Codex · Status: done (fix round 1; commit attempt recorded below)

## Done

- Addressed all 14 accepted findings in the triaged Sonnet 5.5 review, within
  owned paths. Frozen contracts are unchanged.
- Shipped the bounded ClientHandle/EventStream actor, cancellation-safe receive,
  classified reconnect errors/backoff/state, and NotSent vs UnknownOutcome.
- Added deterministic regression coverage, including pipe-independent recovery
  fixtures, barrier-controlled replay, complete consumer replacement, permanent
  reconnect errors, security, bounded buffering and independent child writes.
- Ran the full DoD and three sequential measurement runs with RSS and macOS
  physical footprint at 0/3/60 seconds.
- Updated [report](../../spikes/M0-05-daemon-socket.md), including M0-07 example,
  honest memory-budget misses and remaining limitations. Preserved
  [raw measurements](../../../spikes/daemon-socket/measurements/fix-round-1-macos.txt).

## Not done

- This completion remains uncommitted because Git writes are blocked; see commit result
  below. The original implementation was committed by the orchestrator as
  `9fa0f4f`. Partial fix-round work was committed by the orchestrator as
  `c2ae8f9` after the usage-limit interruption. The previous handoff's blanket
  statement that the original work was uncommitted is superseded.
- A portable <50 MB working-set result is **not** established. Reporting and
  investigation requested by finding 6 are complete; the 0-second RSS samples
  all exceed the budget, and Linux remains unmeasured. No memory purge or
  workload reduction was used to force a pass.

## Fix round 1

All findings accepted; none deferred or requiring a contract change. Listed in
requested priority order:

| Finding | Implemented change / validation |
|---|---|
| 1 | Real identity rows for pipe-independent `/bin/sleep 60` fixtures. Assert matching process dies by SIGKILL and mismatched start time survives. Mutation proof below. Corrected the earlier reaping claims. |
| 2 | Actor owns reconnect through completion. `next()` is a channel receive. Tests cancel it at connect/Hello/subscription/output checkpoints and during backoff; Connected follows all restore responses. |
| 3 | Retry transient I/O only, 50 ms–1 s exponential backoff, terminal permanent errors, observable connection state. Tested Incompatible, wrong handshake, unknown thread, cursor past end. Documented repeated ReplayDone. |
| 5 | Bounded 16-entry request queue, 32-entry event queue, weighted 2 MiB event budget (single oversized legal frame occupies it), one-entry wire queue. Actor backpressure test; documented UI dedupe and M0-07 integration example. |
| 10 | Client directory/socket owner/mode verification; bind/chmod/rename publication; lock 0600. Tests reject symlink/mode/UID mismatches and prove a live daemon survives a failed second instance. |
| 6 | Three fresh sequential runs, RSS/footprint at 0/3/60 s, phase diagnostics and history-size investigation. Report early failures, reclaim variability, and generator-limited keep-up rate. |
| 4 | Commands wait for restoration before writing. NotSent for pre-write/terminal restore/frame-validation failure; UnknownOutcome only once write starts. Real-daemon proxy proves execution with response loss and no retry, then a queued ListThreads succeeds after reconnect. |
| 7 | Explicit response/first-page barrier proves appends commit during replay, 25 rounds on each runtime flavor. Added in-flight resubscribe and applied-ID suppression tests. |
| 8 | New Client instance restores persisted applied cursor while daemon remains alive; exact continuity assertions. Automatic daemon reconnect also checks newly appended IDs. Fixed stale comment. |
| 9 | ESRCH during signalling is success, EPERM still errors; deterministic injected-result test. Report identifies identity-safe killpg as M1 work. |
| 12 | Per-child bounded stdin writer tasks with no shared mutex across writes. Blocked-writer regression. Indexed max-ID subquery; only append jobs notify; store notification test. |
| 13 | Assembled frame writes, buffered reads, exact MAX_FRAME_LEN round trip and oversized control/binary write tests. |
| 11 | Clear display error and exit 1; second-instance regression asserts both. |
| 14 | Corrected commit history; Ctrl-C future created once; library connect_or_spawn and daemon.log 0600; spawn/reuse/log-permission test. |

### Required negative orphan-test experiment

Performed during this fix round before the usage-limit interruption. Temporarily
removed the actual orphan signal from reap_if_matching, leaving identity checks
and startup recovery in place, then ran:

```text
CARGO_HOME=/tmp/atrium-m005-cargo cargo test -p spike-daemon-socket --test socket orphan_recovery_and_identity_mismatch
exit 101
orphan_recovery_and_identity_mismatch ... FAILED
recovery did not kill the pipe-independent orphan: Elapsed(())
0 passed; 1 failed; finished in 2.30s
```

Restored the kill and reran the same test:

```text
exit 0
orphan_recovery_and_identity_mismatch ... ok
1 passed; 0 failed; finished in 0.34s
```

The fixture has no stdout/stdin pipe to the daemon, cannot self-exit on EPIPE,
and is waited by the test so SIGKILL can be asserted directly. The final full
suite includes this corrected test. No mutation switch remains in the code.

## Definition of Done results

Ran in the assigned worktree on `agent/sol/M0-05-daemon-socket`. Cargo commands
used `CARGO_HOME=/tmp/atrium-m005-cargo` because the global cache is read-only.

```text
cargo fmt --check
exit 0; no differences

cargo clippy -p spike-daemon-socket --all-targets -- -D warnings
exit 0; no warnings

cargo test -p spike-daemon-socket
exit 0
library: 5 passed; 0 failed
integration: 20 passed; 0 failed
binary unit targets and doc-tests: pass (0 tests)

cargo build -p spike-daemon-socket --release --bins --example measure
exit 0; optimized build

target/release/examples/measure target/release/spike-daemon 3
exit 0; three sequential runs, all required samples collected
```

| Metric | Run 1 | Run 2 | Run 3 |
|---|---:|---:|---:|
| 1,000-behind catch-up | 10.259 ms | 3.454 ms | 3.676 ms |
| Full 100k replay | 340.060 ms | 335.905 ms | 334.229 ms |
| Stored events/sec | 294,066.1 | 297,702.9 | 299,196.1 |
| Generator-limited live delivery/sec | 9,526.2 | 9,850.5 | 9,872.4 |
| RSS 0 s | 89.125 MiB | 89.016 MiB | 89.047 MiB |
| RSS 3 s | 89.125 MiB | 60.375 MiB | 47.562 MiB |
| RSS 60 s | 23.125 MiB | 9.250 MiB | 9.344 MiB |

All reconnect samples pass <100 ms. All 60-second RSS samples pass <50 MB on
this machine. **All 0-second samples fail**; 3-second runs 1 and 2 also fail.
The report/raw output contain each physical-footprint sample and byte counts.

Diagnostic evidence: replay-only RSS is about 10.9 MiB; lifecycle reconstruction
materializes about 50k envelopes with roughly 20 MB of text, and RSS rises to
about 89 MiB. All client SQLite readers close while memory remains high. Later
RSS and footprint declines support allocator/VM retention and reclaim as an
explanation, not a portable budget guarantee or a complete heap attribution.

## Gotchas

- All new test synchronization is via readiness messages, channels/semaphores,
  socket replies or observed durable state. Timeouts bound failure; no test
  sleeps serve as synchronization.
- Use ClientHandle plus a continuously draining EventStream for M0-07. A command
  response behind events is subject to consumer backpressure. UI/webview state
  must dedupe IDs before application, then acknowledge.
- The measurement example's last argument is now **run count**; each run always
  samples at 0/3/60 seconds. It does not force allocator/page-cache release.
- macOS footprint measurement uses libproc (native Clang build tooling).
- The root lockfile already belongs to the orchestrator's committed WIP; this
  completion leaves root Cargo.lock and all contract files unchanged.

## Follow-ups

- Orchestrator: review and commit any remaining unstaged completion changes if
  sandbox Git writes remain blocked.
- M0-07: use the report's small actor example and persist applied UI cursors.
- M1: incremental/persisted lifecycle state, allocator profiling, Linux runs,
  finer native identity and identity-safe process-group cleanup. Memory budget
  remains an engineering constraint despite the small 60-second macOS sample.

### Commit result

Staging and committing the completion both failed with exit 128:

```text
git add -- spikes/daemon-socket docs/spikes/M0-05-daemon-socket.md docs/agents/handoffs/M0-05-daemon-socket.md
git commit -m 'Complete M0-05 review fixes and bounded client actor'
fatal: Unable to create '/Users/pedro/Documents/Harness/.git/worktrees/M0-05-daemon-socket/index.lock': Operation not permitted
```

The sandbox cannot write the external repository metadata. No workaround,
push, merge or rebase was attempted. Completion changes, report, raw measurement
output and this handoff remain uncommitted on the assigned branch for the
orchestrator to commit.
