# Handoff: M1-42-n16-bridge-test-race (N16)

- **Branch:** `agent/neon/M1-42-n16-bridge-test-race`
- **Owned paths touched:**
  - `app/src-tauri/src/bridge.rs` (`#[cfg(test)] mod tests` only; no production code touched)
  - `docs/agents/handoffs/M1-42-n16-bridge-test-race.md`
- **Product check:** Confirmed the real daemon (`crates/atrium-daemon/src/hub.rs`, `Hub::subscribe_inbox`) queues the `SubscribeInbox` response and inserts the connection into `self.inbox` under one mutex, so the product cannot lose an update this way. No changes were made to `atrium-client`, `atrium-daemon`, or non-test code in `bridge.rs`.

## Done

1. **Step 1 — Keep daemon `SubscribeInbox` ordering in `Scripted`** (`app/src-tauri/src/bridge.rs`, `mod tests`):
   - Removed eager registration of connections into `peers` on `listener.accept()`.
   - In the `ClientMessage::SubscribeInbox` arm, `Scripted` now acquires `peers_conn.lock()`, queues `ServerMessage::Response` onto `tx`, and only then appends `(conn_id, tx.clone())` to `peers` and updates the `watch::Sender<usize>` subscriber count — all under the same lock, matching `Hub::subscribe_inbox`.
   - On connection close, subscribed connections are removed from `peers` by `conn_id` and the subscriber count is updated. `Scripted::push` also updates the subscriber count via `send_if_modified` if `retain` drops a closed sender.
2. **Step 2 — Wait for inbox subscribers before pushing**:
   - Added `Scripted::inbox_subscribers(&self, n: usize)` which waits (with a 5 s timeout) on `self.subscribers.subscribe().wait_for(|&count| count >= n)` until `n` connections have had their `SubscribeInbox` answered and been registered as push targets.
   - Called `daemon.inbox_subscribers(1).await` between connecting and the first `daemon.push` in every test that calls `daemon.push`:
     - `batching_keeps_order_and_sequence_numbers`
     - `a_lone_delivery_is_not_delayed`
     - `batch_bounds_are_respected_for_count_and_bytes`
     - `a_gone_sink_ends_forwarding_mid_stream`
     - `micro_measurement_frames_per_10000_deliveries`
   - Verified no other tests in `bridge.rs` call `daemon.push`.
3. **Step 3 — Permanent regression test**:
   - Added `ScriptedOptions { inbox_delay: Duration }` and `Scripted::start_with(answer, options)` (with `Scripted::start` delegating to `ScriptedOptions::default()`). When `!options.inbox_delay.is_zero()`, the `SubscribeInbox` handler sleeps `options.inbox_delay` before answering and registering the peer.
   - Added `delayed_subscribe_inbox_does_not_lose_pushed_updates`, which starts `Scripted::start_with(list_threads_ok, ScriptedOptions { inbox_delay: Duration::from_millis(200) })`, waits for `client_connected` and `daemon.inbox_subscribers(1).await`, pushes 100 `ThreadUpdated` messages, flushes with `flush_barrier`, and asserts all 100 updates arrive in order with rising sequence numbers.

## Checks run by worker (D-123)

- `cargo fmt --all` — clean
- `cargo check -p atrium-app --tests` — clean, 0 warnings

No `cargo test`, `cargo clippy`, or `pnpm` was run by the worker under D-123.

## Orchestrator verification commands

All commands below are fish-compatible.

### 1. Format, clippy, and full workspace test suite

```fish
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
```

### 2. Run `bridge::` tests 300 times in a row

```fish
for i in (seq 1 300)
    cargo test -p atrium-app bridge:: --quiet; or begin; echo "FAILED on run $i"; break; end
end
```

### 3. Verify the regression test fails with Step 1 reverted

Temporarily revert Step 1 in `app/src-tauri/src/bridge.rs` inside `Scripted::start_with`:

1. Right after `let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();`, restore eager peer registration on accept:
   ```rust
   {
       let mut peers = peers_conn.lock().unwrap_or_else(PoisonError::into_inner);
       peers.push((conn_id, tx.clone()));
       subscribed = true;
       let _ = subscribers_conn.send(peers.len());
   }
   ```
2. In the `ClientMessage::SubscribeInbox { request_id }` arm, keep the `inbox_delay` sleep and return `Some(ServerMessage::Response { ... })` directly (without registering the peer there).

Then run the regression test (it must fail on the 5 s `next_raw` timeout because `inbox_subscribers(1)` returns on accept and `push` sends before the 200 ms `SubscribeInbox` answer arrives and clears `known.threads`):

```fish
cargo test -p atrium-app bridge::tests::delayed_subscribe_inbox_does_not_lose_pushed_updates -- --exact
```

Restore the working tree afterwards:

```fish
git checkout -- app/src-tauri/src/bridge.rs
```

## Gotchas / follow-ups

None.
