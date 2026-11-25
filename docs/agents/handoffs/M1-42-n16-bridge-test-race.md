# Handoff: M1-42-n16-bridge-test-race (N16)

- **Branch:** `agent/neon/M1-42-n16-bridge-test-race`
- **Owned paths touched:**
  - `app/src-tauri/src/bridge.rs` (`#[cfg(test)] mod tests` only; no production code touched)
  - `docs/agents/handoffs/M1-42-n16-bridge-test-race.md`
- **Product check:** Confirmed the real daemon (`crates/atrium-daemon/src/hub.rs`, `Hub::subscribe_inbox`) queues the `SubscribeInbox` response and inserts the connection into `self.inbox` under one mutex, so the product cannot lose an update this way. No changes were made to `atrium-client`, `atrium-daemon`, or non-test code in `bridge.rs`.

## Done

1. **Step 1 — Keep daemon `SubscribeInbox` ordering in `Scripted`** (`app/src-tauri/src/bridge.rs`, `mod tests`):
   - Removed eager registration of connections into `peers` on `listener.accept()`.
   - Tracked subscriber state in `watch::Sender<InboxSubscribers>` where `InboxSubscribers { live: usize, answered: u64 }` records both live push targets (`peers.len()`) and the monotonic count of `SubscribeInbox` requests answered.
   - In the `ClientMessage::SubscribeInbox` arm, `Scripted` acquires `peers_conn.lock()`, queues `ServerMessage::Response` onto `tx`, registers `(conn_id, tx.clone())` in `peers` once (`if !subscribed`), and updates `InboxSubscribers { live, answered }` via `send_modify` (which updates stored state even with 0 receivers) — all under the same lock, matching `Hub::subscribe_inbox`.
   - On connection close, subscribed connections are removed from `peers` by `conn_id` and `counts.live` is updated via `send_modify`. `Scripted::push` also updates `counts.live` via `send_if_modified` if `retain` drops a closed sender.
2. **Step 2 — Wait for inbox subscribers before pushing**:
   - Added `Scripted::inbox_subscribers(&self, n: usize)` (`counts.live >= n`, 5 s timeout) for single-connection tests starting from 0 peers, plus `live_inbox_subscribers()`, `inbox_answered_mark()`, and `wait_inbox_answered(target)` (`counts.answered >= target`, 5 s timeout) for multi-connection / reconnect scenarios where a stale peer may already satisfy `live >= n`.
   - Called `daemon.inbox_subscribers(1).await` between connecting and the first `daemon.push` in every test that calls `daemon.push`:
     - `batching_keeps_order_and_sequence_numbers`
     - `a_lone_delivery_is_not_delayed`
     - `batch_bounds_are_respected_for_count_and_bytes`
     - `a_gone_sink_ends_forwarding_mid_stream`
     - `micro_measurement_frames_per_10000_deliveries`
   - Verified no other tests in `bridge.rs` call `daemon.push`.
3. **Step 3 — Permanent regression test and unit coverage**:
   - Added `ScriptedOptions { inbox_delay: Duration, inbox_gate: Option<Arc<tokio::sync::Notify>> }` and `Scripted::start_with(answer, options)` (with `Scripted::start` delegating to `ScriptedOptions::default()`).
   - Added `delayed_subscribe_inbox_does_not_lose_pushed_updates` (200 ms `inbox_delay`, 100 pushed updates, all arrive in order with rising sequence numbers).
   - Added unit tests covering 0-receiver updates (`subscriber_count_is_stored_without_a_watcher`), disconnect cleanup (`disconnect_updates_live_subscriber_count_without_watcher`), dead-peer pruning in `push` (`push_prunes_dead_peer_and_drops_live_count`), two-connection gating (`second_connection_with_delayed_subscribe_inbox_receives_only_post_answer_pushes`), deterministic stale-peer injection + gated reconnect (`stale_peer_requires_wait_inbox_answered_for_new_connection`, proving `inbox_subscribers(1)` is `Ready` while `wait_inbox_answered(mark + 1)` stays `Pending` and `peers.len()` excludes B until B's gate opens), and repeated `SubscribeInbox` on one connection (`repeated_subscribe_inbox_on_same_connection_raises_answered_without_duplicating_peer`).

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

### 3. Verify the regression test fails with Step 1 reverted (superseded by Round 2)

> **Superseded by Round 2** (Round 1 used `watch::Sender::send`, which drops updates when no receiver is subscribed; use the revert instructions in Round 3 below).

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

## Round 2 (superseded by Round 3)

> **Superseded by Round 3**: `watch::Sender<usize>` and `send_replace` were replaced by `watch::Sender<InboxSubscribers>` with `send_modify`; the `< 500 ms` wall-clock assertion was removed; the test was renamed in Round 4; and the revert snippet below no longer type-checks against current code — use the revert instructions in Round 3 / Round 4 below.

### Fix

`tokio::sync::watch::Sender::send` returns `Err(SendError)` **without updating the stored value** when there are no active receivers. Because `Scripted::start_with` drops the initial receiver (`let (subscribers, _) = tokio::sync::watch::channel(0usize);`), any `SubscribeInbox` or disconnect that completed before a test called `inbox_subscribers` left the stored count at `0`, causing `inbox_subscribers(1)` to time out after 5 s.

Checked every place the subscriber count changes in `app/src-tauri/src/bridge.rs` (`mod tests`):
- **Register (`SubscribeInbox` arm):** changed `subscribers_conn.send(peers.len())` to `subscribers_conn.send_replace(peers.len())`, which always updates the stored value even with 0 receivers.
- **Disconnect:** changed `subscribers_conn.send(peers.len())` to `subscribers_conn.send_replace(peers.len())`.
- **`Scripted::push`:** verified `self.subscribers.send_if_modified(...)` already updates the stored value unconditionally even with 0 receivers.

### New test

Added `inbox_subscribers_returns_immediately_if_already_subscribed` in `app/src-tauri/src/bridge.rs`:
- Connects a `Bridge`, opens `"main"`, enables `show_inbox(true)`, waits for the initial `"inbox"` snapshot, and runs `flush_barrier` on the same connection so `SubscribeInbox` has completely finished updating `subscribers` before anyone watches it.
- Asserts `daemon.subscribers.receiver_count() == 0`.
- Calls `daemon.inbox_subscribers(1).await` and asserts it returns immediately (`< 500 ms`).

### Checks run by worker (D-123)

- `cargo fmt --all` — clean
- `cargo check -p atrium-app --tests` — clean, 0 warnings

### Updated verification commands (fish-compatible)

#### 1. Format, clippy, and full workspace test suite

```fish
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
```

#### 2. Run `bridge::` tests 300 times in a row

```fish
for i in (seq 1 300)
    cargo test -p atrium-app bridge:: --quiet; or begin; echo "FAILED on run $i"; break; end
end
```

#### 3. Verify the regression test fails with Step 1 reverted (superseded by Round 3)

> **Superseded by Round 3** (`send_replace` no longer type-checks against `watch::Sender<InboxSubscribers>`; use the revert instructions in Round 3 / Round 4 below).

Temporarily revert Step 1 in `app/src-tauri/src/bridge.rs` inside `Scripted::start_with`:

1. Right after `let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();` and `let mut subscribed = false;`, restore eager peer registration on accept **using `send_replace`**:
   ```rust
   {
       let mut peers = peers_conn.lock().unwrap_or_else(PoisonError::into_inner);
       peers.push((conn_id, tx.clone()));
       subscribed = true;
       let _ = subscribers_conn.send_replace(peers.len());
   }
   ```
2. In the `ClientMessage::SubscribeInbox { request_id }` arm, keep the `inbox_delay` sleep and return `Some(ServerMessage::Response { ... })` directly (without locking `peers_conn` or registering there):
   ```rust
   ClientMessage::SubscribeInbox { request_id } => {
       if !options.inbox_delay.is_zero() {
           tokio::time::sleep(options.inbox_delay).await;
       }
       Some(ServerMessage::Response {
           request_id,
           result: CommandResult::Ok {
               value: serde_json::json!({"threads": [], "inbox": []}),
           },
       })
   }
   ```

Then run the regression test:

```fish
cargo test -p atrium-app bridge::tests::delayed_subscribe_inbox_does_not_lose_pushed_updates -- --exact
```

Expected failure: `inbox_subscribers(1)` returns immediately on accept (does **not** hang), `daemon.push` sends the 100 `ThreadUpdated` messages before the 200 ms `SubscribeInbox` answer arrives, the delayed answer clears `known.threads`, and the test panics in `next_raw` with `"a delivery arrives"` (not `"connections subscribe to the inbox"`).

Restore the working tree afterwards:

```fish
git checkout -- app/src-tauri/src/bridge.rs
```

## Round 3

### Changes (test module only: `app/src-tauri/src/bridge.rs`, `#[cfg(test)] mod tests`)

1. **Finding 1 — Monotonic `SubscribeInbox` answer counter alongside live peer count:**
   - Replaced `watch::Sender<usize>` with `watch::Sender<InboxSubscribers>` where `InboxSubscribers { live: usize, answered: u64 }` tracks both the live registered push targets (`peers.len()`) and the monotonic count of `SubscribeInbox` requests whose response has been queued under the same `peers` lock.
   - Updated `SubscribeInbox` and disconnect paths to use `send_modify` (which updates the stored state even with 0 receivers), and `Scripted::push` to update `counts.live` via `send_if_modified`.
   - Kept `Scripted::inbox_subscribers(n)` (`live >= n`, 5 s timeout) for single-connection tests starting from 0 peers, and added `live_inbox_subscribers()`, `inbox_answered_mark()`, and `wait_inbox_answered(target)` (`answered >= target`, 5 s timeout), documenting exactly what each helper promises.
   - Added an optional `inbox_gate: Option<Arc<tokio::sync::Notify>>` to `ScriptedOptions` so multi-connection tests can hold a new connection's `SubscribeInbox` answer deterministically without wall-clock races.
2. **Finding 2 — Removed wall-clock timing assertion:**
   - Dropped `elapsed() < Duration::from_millis(500)` from `inbox_subscribers_returns_immediately_if_already_subscribed` (the helper's own 5 s timeout catches a broken 0-receiver update without flaking under load).
3. **Finding 3 — Documented `receiver_count() == 0` assertion:**
   - Added a comment explaining that `watch::Sender::send` drops updates when `receiver_count() == 0`, so asserting zero receivers verifies the 0-receiver update path (`send_modify`).

### New tests

1. `disconnect_updates_live_subscriber_count_without_watcher`:
   - Subscribes a client (`live == 1`, `answered == 1`), verifies `receiver_count() == 0`, drops the client while keeping `receiver_count() == 0`, and polls `live_inbox_subscribers()` until it reaches `0` (with `answered` staying `1`).
2. `push_prunes_dead_peer_and_drops_live_count`:
   - Subscribes a live client, injects a peer whose writer receiver (`rx`) is already dropped (`live == 2`, `receiver_count() == 0`), calls `daemon.push(...)`, and asserts the dead peer is pruned from `peers`, `live` drops to `1` (`answered` stays `1`), and the live client still receives the pushed update.
3. `second_connection_with_delayed_subscribe_inbox_receives_only_post_answer_pushes`:
   - Connects client A (`live == 1`, `mark = inbox_answered_mark() == 1`), connects client B (re-attached window on a new connection) with a gated + 200 ms delayed `SubscribeInbox` answer, pushes `"before-b"` while B's answer is pending (asserting `live == 1`, `answered == mark`), releases B and waits via `wait_inbox_answered(mark + 1)` (`live == 2`, `answered == 2`), then pushes `"after-b"`. Asserts A receives `["before-b", "after-b"]` and B receives its empty `inbox` snapshot followed only by `["after-b"]`.

### Checks run by worker (D-123)

- `cargo fmt --all` — clean
- `cargo check -p atrium-app --tests` — clean, 0 warnings

No `cargo test`, `cargo clippy`, or `pnpm` was run by the worker under D-123.

### Verification commands (fish-compatible)

#### 1. Format, clippy, and full workspace test suite

```fish
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
```

#### 2. Run `bridge::` tests 300 times in a row

```fish
for i in (seq 1 300)
    cargo test -p atrium-app bridge:: --quiet; or begin; echo "FAILED on run $i"; break; end
end
```

#### 3. Verify the regression test fails with Step 1 reverted (race itself, not `inbox_subscribers` hang)

Temporarily revert Step 1 in `app/src-tauri/src/bridge.rs` inside `Scripted::start_with`:

1. Right after `let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();` and `let mut subscribed = false;`, restore eager peer registration on accept using `send_modify`:
   ```rust
   {
       let mut peers = peers_conn.lock().unwrap_or_else(PoisonError::into_inner);
       peers.push((conn_id, tx.clone()));
       subscribed = true;
       subscribers_conn.send_modify(|counts| {
           counts.live = peers.len();
       });
   }
   ```
2. In the `ClientMessage::SubscribeInbox { request_id }` arm, keep the gate/delay and return `Some(ServerMessage::Response { ... })` directly (without locking `peers_conn` or registering there):
   ```rust
   ClientMessage::SubscribeInbox { request_id } => {
       if let Some(gate) = &options_conn.inbox_gate {
           gate.notified().await;
       }
       if !options_conn.inbox_delay.is_zero() {
           tokio::time::sleep(options_conn.inbox_delay).await;
       }
       Some(ServerMessage::Response {
           request_id,
           result: CommandResult::Ok {
               value: serde_json::json!({"threads": [], "inbox": []}),
           },
       })
   }
   ```

Then run the regression test:

```fish
cargo test -p atrium-app bridge::tests::delayed_subscribe_inbox_does_not_lose_pushed_updates -- --exact
```

Expected failure: `inbox_subscribers(1)` returns immediately on accept (does **not** hang), `daemon.push` sends the 100 `ThreadUpdated` messages before the 200 ms `SubscribeInbox` answer arrives, the delayed answer clears `known.threads`, and the test panics in `next_raw` with `"a delivery arrives"` (not `"connections subscribe to the inbox"`).

Restore the working tree afterwards:

```fish
git checkout -- app/src-tauri/src/bridge.rs
```

## Round 4

### Changes

1. **Handoff updated** (`docs/agents/handoffs/M1-42-n16-bridge-test-race.md`):
   - Updated "Done" to describe the `InboxSubscribers { live, answered }` design and current test suite.
   - Marked the entire "Round 2" section (including its `send_replace` revert step 3 and removed `< 500 ms` assertion) as superseded by Round 3.
   - Verified the current revert snippet (below and in Round 3) type-checks against today's `Scripted::start_with` (`InboxSubscribers`, `send_modify`, `options_conn`).
2. **Renamed and cleaned up 0-receiver test** (`app/src-tauri/src/bridge.rs`, `mod tests`):
   - Renamed `inbox_subscribers_returns_immediately_if_already_subscribed` to `subscriber_count_is_stored_without_a_watcher`.
   - Added `drop(handle); bridge.detach("main"); bridge.client().shutdown();` at the end, matching sibling tests.
3. **Stale-peer test + removed redundant delay** (`app/src-tauri/src/bridge.rs`, `mod tests`):
   - Dropped the redundant `inbox_delay: Duration::from_millis(200)` from `second_connection_with_delayed_subscribe_inbox_receives_only_post_answer_pushes` (the gate alone holds B deterministically).
   - Added `stale_peer_disconnect_requires_wait_inbox_answered_for_new_connection` (refined and renamed to `stale_peer_requires_wait_inbox_answered_for_new_connection` in Round 5 to inject a deterministic stale peer into `peers` and assert `inbox_subscribers(1)` is `Ready`, `wait_inbox_answered(mark + 1)` is `Pending`, and `peers.len()` rises from `1` to `2` across the gate).
4. **Repeated-`SubscribeInbox` test** (`app/src-tauri/src/bridge.rs`, `mod tests`):
   - Added `repeated_subscribe_inbox_on_same_connection_raises_answered_without_duplicating_peer`: sends two `SubscribeInbox` requests over a single connection, verifying both are answered, `answered` rises from `1` to `2`, `live` and `peers.len()` stay `1` (matching `Hub::subscribe_inbox`'s `HashMap::insert`), and a subsequent push is delivered once before a barrier response.

### Checks run by worker (D-123)

- `cargo fmt --all` — clean
- `cargo check -p atrium-app --tests` — clean, 0 warnings

No `cargo test`, `cargo clippy`, or `pnpm` was run by the worker under D-123.

### Verification commands (fish-compatible)

#### 1. Format, clippy, and full workspace test suite

```fish
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
```

#### 2. Run `bridge::` tests 300 times in a row

```fish
for i in (seq 1 300)
    cargo test -p atrium-app bridge:: --quiet; or begin; echo "FAILED on run $i"; break; end
end
```

#### 3. Verify the regression test fails with Step 1 reverted

Temporarily revert Step 1 in `app/src-tauri/src/bridge.rs` inside `Scripted::start_with`:

1. Right after `let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();` and `let mut subscribed = false;`, restore eager peer registration on accept:
   ```rust
   {
       let mut peers = peers_conn.lock().unwrap_or_else(PoisonError::into_inner);
       peers.push((conn_id, tx.clone()));
       subscribed = true;
       subscribers_conn.send_modify(|counts| {
           counts.live = peers.len();
       });
   }
   ```
2. In the `ClientMessage::SubscribeInbox { request_id }` arm, keep the gate/delay and return `Some(ServerMessage::Response { ... })` directly (without locking `peers_conn` or registering there):
   ```rust
   ClientMessage::SubscribeInbox { request_id } => {
       if let Some(gate) = &options_conn.inbox_gate {
           gate.notified().await;
       }
       if !options_conn.inbox_delay.is_zero() {
           tokio::time::sleep(options_conn.inbox_delay).await;
       }
       Some(ServerMessage::Response {
           request_id,
           result: CommandResult::Ok {
               value: serde_json::json!({"threads": [], "inbox": []}),
           },
       })
   }
   ```

Then run the regression test:

```fish
cargo test -p atrium-app bridge::tests::delayed_subscribe_inbox_does_not_lose_pushed_updates -- --exact
```

Expected failure: `inbox_subscribers(1)` returns immediately on accept, `daemon.push` sends the 100 `ThreadUpdated` messages before the 200 ms `SubscribeInbox` answer arrives, the delayed answer clears `known.threads`, and the test panics in `next_raw` with `"a delivery arrives"`.

Restore the working tree afterwards:

```fish
git checkout -- app/src-tauri/src/bridge.rs
```

## Round 5

### Changes

1. **Deterministic stale-peer test** (`app/src-tauri/src/bridge.rs`, `mod tests`):
   - Renamed `stale_peer_disconnect_requires_wait_inbox_answered_for_new_connection` to `stale_peer_requires_wait_inbox_answered_for_new_connection`.
   - After A's `SubscribeInbox` is answered (`mark = inbox_answered_mark() == 1`), waits for A's disconnect to drain `live` to `0`, then deterministically injects a stale peer `(u64::MAX, stale_tx)` into `peers` (keeping `stale_rx` alive so `push` does not prune it) and sets `counts.live = peers.len()` under the same `peers` lock.
   - Connects B with its `SubscribeInbox` answer gated. While B is gated, polls each future once with `Context::from_waker(Waker::noop())` (no sleeps or timing assertions) and pins:
     - `inbox_subscribers(1)` is `Poll::Ready` (satisfied by the stale peer);
     - `wait_inbox_answered(mark + 1)` is `Poll::Pending` (B is not answered yet);
     - `peers.len() == 1` (and `live == 1`, `answered == mark`), so B is not registered yet.
   - Pushes `"before-b"`, releases the gate, waits via `wait_inbox_answered(mark + 1)`, asserts `peers.len() == 2` (`live == 2`, `answered == mark + 1`), pushes `"after-b"`, and checks B receives its empty `inbox` snapshot followed only by `"after-b"`.
2. **Handoff updated** (`docs/agents/handoffs/M1-42-n16-bridge-test-race.md`):
   - Updated the Done and Round 4 entries to reflect the renamed test and what it now pins.

### Checks run by worker (D-123)

- `cargo fmt --all` — clean
- `cargo check -p atrium-app --tests` — clean, 0 warnings

No `cargo test`, `cargo clippy`, or `pnpm` was run by the worker under D-123.

### Verification commands (fish-compatible)

Same as Round 4:

```fish
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
for i in (seq 1 300)
    cargo test -p atrium-app bridge:: --quiet; or begin; echo "FAILED on run $i"; break; end
end
```
