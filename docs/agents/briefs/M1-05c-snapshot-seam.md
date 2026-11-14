# M1-05c-snapshot-seam: a store hook for tests, and the snapshot-ordering test

Model: Gemini 4 Hydrogen (`gemini-4-hydrogen-medium`, `scripts/agent-run agy`; D-076) · Reviewer: Sonnet 5.5 (read-only) · Depends on: M1-05b (merged into `dev`, `0e75ec2`)
Go: D-077 (Pedro, 2026-11-14) · Branch: `agent/hydrogen/M1-05c-snapshot-seam` · Worktree: `../atrium-wt/M1-05c-snapshot-seam`

## Goal
Pin the snapshot-ordering rule of `crates/atrium-daemon/src/upkeep.rs` (`snapshot_daily`) with a test: a turn that starts while a snapshot is being made is either seen by the "no turn is running" check or stored after the snapshot. M1-05b couldn't test it without a hook (`docs/agents/reviews/M1-05b-daemon-test-gaps.md`, section 5). Also fix that review's two test nits.

## Context
- `docs/agents/reviews/M1-05b-daemon-test-gaps.md`: sections 3 (the nits) and 5 (the seam and the test).
- `docs/agents/reviews/M1-05-daemon-late-fixes.md`, finding 4 (the snapshot part).
- `crates/atrium-daemon/src/upkeep.rs` `snapshot_daily`; `src/lib.rs` `Options`, `Inner` and `Daemon::start`; `atrium_core::Store` (`crates/atrium-core/src/lib.rs`); `docs/contracts/m1.md` §7.
- `crates/atrium-daemon/tests/gaps_full_storage.rs` and `tests/common/` for how tests start a daemon.

## Owned paths
- `crates/atrium-daemon/src/lib.rs`: one new `Options` field and the line in `Daemon::start` that applies it. Nothing else.
- `crates/atrium-daemon/tests/gaps_snapshot.rs` (new; import the helpers with `#[path = "common/mod.rs"] mod common;`).
- The two tests in `crates/atrium-daemon/src/conn.rs`'s `mod tests` that M1-05b added (the nits only).
- `docs/agents/handoffs/M1-05c-snapshot-seam.md`

## What to do
1. **The hook.** Add a test seam to `Options`, written like the existing `connection_queue_bytes` and `snapshot_check_every` seams (doc comment says it is a test seam):
   `pub store_wrapper: Option<Arc<dyn Fn(Arc<dyn Store>) -> Arc<dyn Store> + Send + Sync>>`, `None` in `Options::new`. In `Daemon::start`, wrap the opened store with it before anything else uses the store (the ledger, recovery, `Inner`). With `None` the daemon behaves exactly as now.
2. **The test**, `tests/gaps_snapshot.rs`:
   - A delegating `Store` that forwards every method to the real store, except that `snapshot()` signals "started" and then waits on a release signal (a `tokio::sync::Notify` or a oneshot) before forwarding.
   - Start a daemon with the wrapper, a small `snapshot_check_every`, and no snapshot yet (so one is due).
   - **Case A, turn starts during the snapshot:** wait for "started", then send a user message on a Home thread with the fake agent. Check that the turn's `TurnStarted` is not stored yet while the snapshot is held (the append waits on `publish`). Release; check it is stored afterwards and the snapshot file exists.
   - **Case B, turn already running:** start a turn first (a fake agent that keeps the turn open), let the check run, and assert `snapshot()` is never called while the turn runs (the wrapper counts calls).
   - No sleeps as synchronisation, except bounded waits with a timeout that fail the test (e.g. "`TurnStarted` not stored within 200 ms" is acceptable only as the negative check in case A; say so in a comment).
   - Stop every daemon the test starts.
3. **The M1-05b nits** (`src/conn.rs` tests):
   - `write_loop_finishes_a_started_frame_…`: add `assert!(received.len() < large.len())` before the subscription ends, and either end the variants by closing the peer instead of calling `finish()`, or reword the comment so it doesn't claim the origin check is what drops the later frame.
4. **Show the test catches a regression:** in your worktree, temporarily move the `running_turns()` check in `snapshot_daily` above the `publish` lock (or drop the lock), run the test, see it fail, and restore the code. Don't commit the mutation; report the output in the handoff.

## Definition of Done
- [ ] `gaps_snapshot` and `conn::tests` each pass 20 times in a row.
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test -p atrium-daemon`.
- [ ] `git diff dev -- crates/atrium-daemon/src` shows only the hook (field, default, one line in `start`) and the two test edits.
- [ ] Handoff: the hook, the test names, the mutation result, the outputs.

## Rules
Work only in your worktree. Tests use temp folders; no real home paths in any file. Commit on your branch only; never push, merge or rebase. Shell substitutions, `~` and `../` are refused by the worker policy.

## Escalate if
The store can't be wrapped before the ledger and recovery use it without a larger change to `start`, or the test can't be made deterministic: stop and say what would be needed.
