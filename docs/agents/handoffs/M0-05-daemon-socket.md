# Handoff: M0-05-daemon-socket

Model: GPT-6.1 Sol · Status: done

## Done

- Implemented the codec, applied-cursor Client with automatic reconnect and
  UnknownOutcome, secure single-instance WAL daemon, fake child process and
  detached demonstration client within the owned spike paths.
- Implemented replay/live boundary without a global writer lock, subscribe
  responses/errors/replacement, PID/start/executable identity persistence,
  fail-closed orphan handling and closing lifecycle events.
- Added eleven tests covering all brief scenarios and a reproducible benchmark.
- Report: [M0-05-daemon-socket](../../spikes/M0-05-daemon-socket.md), including
  measurements, risks, reproduction commands and M0-07 integration instructions.

## Not done

- Commits: sandbox blocks writes to the worktree's external Git metadata.
  `git add spikes/daemon-socket` and `git commit -m 'Implement M0 daemon socket
  replay and reconnect spike'` both exited 128:

  ```text
  fatal: Unable to create '/Users/pedro/Documents/Harness/.git/worktrees/M0-05-daemon-socket/index.lock': Operation not permitted
  ```

  All deliverables remain uncommitted for the orchestrator. No workaround,
  push, merge, rebase, main/dev change or contract change was performed.

## Definition of Done results

Commands ran from the assigned worktree on `agent/sol/M0-05-daemon-socket`.
Cargo builds used `CARGO_HOME=/tmp/atrium-m005-cargo` because the global cache
cannot accept downloaded packages in this sandbox.

```text
cargo fmt --check
exit 0, no formatting differences

cargo clippy -p spike-daemon-socket --all-targets -- -D warnings
exit 0, Finished dev profile; no warnings

cargo test -p spike-daemon-socket
exit 0
integration tests: 11 passed; 0 failed; 0 ignored
library/binary unit targets and doc-tests: passed (0 tests)

cargo build -p spike-daemon-socket --release --bins --example measure
exit 0, Finished release profile

target/release/examples/measure target/release/spike-daemon 60
exit 0
cursor 99,000: 1,000 events, connect-to-caught-up 8.737 ms (<100 ms: PASS)
no cursor: 100,000 events, connect-to-caught-up 339.988 ms
stored sustained delivery: 294,127.7 events/s
live: 49,878 deltas / 5.238 s = 9,522.4 deltas/s
60 s idle RSS: 28,835,840 bytes = 27.500 MiB (<50 MB: PASS)
```

The replay/live race test runs 25 concurrent-append rounds. Lost command
responses are tested through a proxy to the real daemon, asserting one executed
thread and UnknownOutcome. Orphan tests kill the daemon with SIGKILL, assert
identity-matched cleanup, and leave a start-time-mismatched process unsignalled.
UID mismatch is simulated through the production metadata validator.

## Gotchas

- Git metadata permissions prevent staging/committing, including this handoff.
- Root Cargo.lock resolution generated during builds is restored to its original
  contents; the orchestrator must update it at integration. Added dependencies
  live only in the spike's Cargo.toml.
- Native process inspection works here; shell `ps` and CPU-brand sysctl are
  blocked by the sandbox. Measurements identify Darwin 27.0.0 / arm64 without
  claiming an unavailable exact CPU model.
- Fake agents must be built next to spike-daemon. Use short --dir paths on macOS.
- Client next() reconnects automatically; consumer application advances the
  cursor. Commands never retry. See report for mutable-client ownership and
  cancellation notes.

## Follow-ups

- Orchestrator: review, resolve root Cargo.lock and commit these owned files.
- M0-07: use the documented Client API and acknowledge after applying UI state.
- M1: persistent open-object projections, finer native process identity, bounded
  byte buffering, request deadlines/backoff, reader pool and Linux validation.
