# Handoff: M1-24-bridge-batching — status: **done**

Worker: Gemini 4 Hydrogen (high effort) · Branch: `agent/hydrogen/M1-24-bridge-batching`

## Done

1. **Batching in `app/src-tauri/src/bridge.rs`:**
   - Added `MAX_BATCH_DELIVERIES = 512` and `MAX_BATCH_BYTES = 256 * 1024` (256 KiB).
   - Added `batch_frame(&[Outgoing]) -> String`, which returns `frame(seq, json)` (`{"seq":...,"delivery":...}`) when there is a single item and `{"batch":[{"seq":...,"delivery":...},...]}` when two or more items are queued, embedding each `Outgoing::json` verbatim without re-encoding.
   - Updated `forward(window, sink)` to await the first delivery via `window.recv().await`, then non-blockingly drain any already-queued deliveries via `try_recv_now` (`std::task::Waker::noop()` polling `window.recv()` once) up to `MAX_BATCH_DELIVERIES` and `MAX_BATCH_BYTES`.
   - No coalescing timer is used: a lone delivery goes out immediately with 0 ms added latency.
   - If adding the next drained item would exceed `MAX_BATCH_BYTES`, it is held in `carry: Option<Outgoing>` and sent as the start of the immediately following iteration so multi-delivery frames never exceed 256 KiB, order/`seq` are preserved, and an oversized single delivery (> 256 KiB) still passes through intact.
   - If `try_recv_now` observes `Poll::Ready(None)` (window replaced/closed or client shut down), the already-popped batch is delivered before exiting.
   - `app/src-tauri/src/lib.rs` (`bridge_attach`) needed no changes because it already passes the frame string straight to `channel.send(InvokeResponseBody::Json(frame))`.

2. **Receive path in `app/src/daemon/client.ts`:**
   - Defined `ChannelFrame = Frame | { batch: Frame[] }` locally (leaving `app/src/daemon/types.ts` untouched per owned paths).
   - Added `receiveFrame(incoming: ChannelFrame)` and wired `channel.onmessage = receiveFrame` in `attach()`.
   - Each item in a batch runs `timed(\`deliver:${item.delivery.type}\`, () => apply(item.delivery))` in order, advances `applied = item.seq`, and calls `scheduleAck()` once at the end of the frame (same 16 ms ack cadence and `bridge_ack` payload).
   - Multi-item batches run inside Solid's `batch(...)` so `setDaemon` updates flush once per batch rather than once per item.
   - If an item inside a batch throws, subsequent items in the batch still apply and advance `applied` before `scheduleAck()` runs and the first error is rethrown.

3. **Tests:**
   - Rust (`app/src-tauri/src/bridge.rs`): updated `Scripted` to support server-pushed `ServerMessage` bursts and added `unpack` for existing tests, plus:
     - `batch_frame_packs_multiple_deliveries_and_preserves_lone_frame`
     - `batching_keeps_order_and_sequence_numbers` (1,200 deliveries arrive in order with consecutive `seq` in <= 4 frames)
     - `a_lone_delivery_is_not_delayed` (unbatched `{"seq":...,"delivery":...}` shape, < 50 ms)
     - `batch_bounds_are_respected_for_count_and_bytes` (612 deliveries capped at 512 per batch; 30 KiB deliveries capped at `<= 256 KiB` per multi-delivery batch; 300 KiB oversized lone delivery still forwarded intact followed by tail delivery)
     - `a_gone_sink_ends_forwarding_mid_stream` (sink returning `false` drops `Window` and closes it in `Client`)
     - `micro_measurement_frames_per_10000_deliveries`
   - TypeScript (`app/src/daemon/client.test.ts`):
     - batch applies in order, records per-delivery `timed` counters, and acknowledges the last `seq` after 16 ms
     - mix of batch and single frames (plus empty `{ batch: [] }`) applies in order and acknowledges the last `seq`
     - error on one item in a batch still applies later items and acknowledges the last `seq`

## Micro-measurement (without GUI)

From `cargo test -p atrium-app -- bridge --nocapture` (`bridge::tests::micro_measurement_frames_per_10000_deliveries`):
- Workload: 10,000 queued `ThreadUpdated` deliveries forwarded through `Client` -> `forward` -> `Sink`
- **Before (unbatched):** `10,000` frames
- **After (batched, max 512 / 256 KiB):** `20` frames (500x reduction; `max_batch_len=512`, `max_frame_bytes=221707`, `elapsed=254.409292ms` including test JSON parsing)

## What the orchestrator should check with `stream_bench.py`

Run (under the GUI lock, e.g. on macOS):
```
/usr/bin/lockf -k /tmp/atrium-gui-measure.lock python3 app/scripts/stream_bench.py --runs 3
```
Check:
1. Frame gate (`p99 <= 33 ms`, `<= 1 %` skipped/long frames) and `frameWork` `busy` time per frame (previously ~5 ms/frame from 10,000 `webview.eval` calls/s).
2. Whether live streaming across 10 threads (`fake.rs` ticks every 5 ms per thread) coalesces enough deliveries per `Channel::send` without a coalescing timer. Because `forward` drains only what is already queued and `Channel::send` posts asynchronously to the webview main thread, if `stream_bench.py` still shows small batch sizes and high `busy` ms/frame, a follow-up <= 4 ms pacing window (triggered only while streaming/busy, keeping 0 ms delay on lone idle deliveries) should be evaluated.

## Definition of Done commands

All run after the final code edit:

- `cargo fmt --all --check` -> exit 0 (no output).
- `cargo clippy --workspace --all-targets -- -D warnings` -> exit 0 (`Finished dev profile`).
- `cargo test --workspace --no-fail-fast` -> exit 0 (all workspace crates passed; `atrium-app` ran 51 lib tests + 2 `delivery_shapes` tests, 0 failed; `atrium-host` `adversarial_supervise` had 7 passed, 1 macOS-ignored `kill_recorded_setsid_grandchild_and_unreaped_zombie_leader_boundaries` — note an earlier run under parallel load failed `adversarial_supervise`, then both standalone `cargo test -p atrium-host --test adversarial_supervise` and the subsequent full `cargo test --workspace --no-fail-fast` exited 0).
- `pnpm --dir app typecheck` -> exit 0 (`tsc --noEmit`).
- `pnpm --dir app test` -> exit 0 (`Test Files 15 passed (15)`, `Tests 267 passed (267)`).
- `pnpm --dir app build` -> exit 0 (`dist/assets/index-CwABbGv9.js 273.79 kB | gzip: 90.63 kB`, built in 1.72 s).
