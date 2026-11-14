# Handoff: M1-05c-snapshot-seam

Model: Gemini 4 Hydrogen (Medium) · Branch: `agent/hydrogen/M1-05c-snapshot-seam`

## Done
1. **Store wrapper seam (`crates/atrium-daemon/src/lib.rs`)**
   - Added `pub store_wrapper: Option<Arc<dyn Fn(Arc<dyn Store>) -> Arc<dyn Store> + Send + Sync>>` to `Options` (with `#[allow(clippy::type_complexity)]` for workspace clippy, defaulting to `None` in `Options::new`).
   - In `Daemon::start`, wrapped the opened `SqliteStore` via `options.store_wrapper` before `Ledger::new`, `Inner`, or `upkeep::recover` uses it.

2. **Snapshot-ordering tests (`crates/atrium-daemon/tests/gaps_snapshot.rs`)**
   - Implemented `GatedStore` delegating all `atrium_core::Store` methods to the inner store, with `SnapshotGate` signals on `snapshot()`, `append()`, and `storage_status()`.
   - `turn_started_during_snapshot_waits_for_snapshot_to_finish` (Case A): waits for `snapshot()` to start and hold `publish`, launches a Home thread with `AgentLaunch::Fake`, verifies via a 200 ms bounded timeout that `TurnStarted` is not stored while `snapshot()` is held, releases `snapshot()`, and verifies `TurnStarted` is stored and a snapshot file exists in `snapshots_dir`.
   - `snapshot_is_not_called_while_a_turn_is_running` (Case B): starts a long-running fake turn (`5_000 ms`), waits until `ThreadState::Running` and at least two `storage_status()` checks occur, asserts `snapshot()` call count is `0`, interrupts the turn, and stops the daemon.

3. **M1-05b test nits (`crates/atrium-daemon/src/conn.rs`)**
   - Added `assert!(received.len() < large.len());` in `write_loop_finishes_a_started_frame_and_drops_later_ones_when_subscription_ends` before ending/replacing the subscription.
   - Added a comment clarifying that ending or replacing the subscription purges queued later frames while the in-flight frame still finishes.

## Mutation check (reverted, not committed)
Temporarily removed `let _appends_held = inner.publish.lock().await;` in `crates/atrium-daemon/src/upkeep.rs` (`snapshot_daily`) and ran `cargo test -p atrium-daemon --test gaps_snapshot`:

```
running 2 tests
test turn_started_during_snapshot_waits_for_snapshot_to_finish ... FAILED
test snapshot_is_not_called_while_a_turn_is_running ... ok

failures:

---- turn_started_during_snapshot_waits_for_snapshot_to_finish stdout ----

thread 'turn_started_during_snapshot_waits_for_snapshot_to_finish' (38824801) panicked at crates/atrium-daemon/tests/gaps_snapshot.rs:229:5:
TurnStarted was stored while snapshot was held
```

Restored `upkeep.rs` immediately after.

## Definition of Done outputs
- `cargo fmt --all --check`: exited 0 (clean).
- `cargo clippy --workspace --all-targets -- -D warnings`: exited 0 (clean).
- `cargo test -p atrium-daemon`: exited 0 — all passed (`lib`: 15, `client`: 1, `gaps_full_storage`: 2, `gaps_snapshot`: 2, `recovery`: 3, `sessions`: 19, `socket`: 9).
- 20x determinism runs:
  - `cargo test -p atrium-daemon --test gaps_snapshot -q` × 20: 20/20 passed (2 passed each run).
  - `cargo test -p atrium-daemon --lib conn::tests -q` × 20: 20/20 passed (11 passed each run).
- `git diff dev -- crates/atrium-daemon/src`: only `crates/atrium-daemon/src/lib.rs` (the `Options::store_wrapper` field, `None` default, and `Daemon::start` wrap) and `crates/atrium-daemon/src/conn.rs` (the two test nits).
