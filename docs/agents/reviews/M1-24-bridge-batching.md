# Review: M1-24-bridge-batching (ad85f9ad, 9f086db0)

Reviewer: Claude Sonnet 5.5, read-only (cross-provider). Author: Gemini 4 Hydrogen `high`. Saved by the orchestrator from the reviewer's report. The orchestrator applied finding 1 in `98777808`.

## Verdict: **approve** (two optional minor changes, a few nits)

No blocker or major issue. The reviewer could not reproduce the worker's 47-minute test hang, and found no path in the final `forward` that can hang. The only hang risk was in the tests (finding 1).

## Findings
1. **Minor (test hygiene): four `forward` task joins had no timeout** (`bridge.rs:830, 924, 976, 1072`). They end only if the client's `Shutdown` handling closes the window. This was the only place a test could block forever, so it is the most likely cause of the hang. *Fixed by the orchestrator, `98777808`: each join now has a 5 s `tokio::time::timeout`.*
2. **Minor: frames of 8 KiB or more go through Tauri's fetch path, not eval** (`tauri-2.12.5/src/ipc/channel.rs`, `MAX_JSON_DIRECT_EXECUTE_THRESHOLD = 8192`). A busy batch (~220 KiB) costs one `eval`, one IPC fetch and one `JSON.parse`: still far cheaper than 500 evals, but a round trip, and ordering relies on Tauri's index reordering (already relied on for snapshots). No code change; noted here instead of the handoff.
3. **Minor (risk): one frame can carry up to 512 deliveries or 256 KiB, applied synchronously.** Branch run 3 (load ~7): p99 23 ms, max 52 ms, busy 4.5 ms/frame with 450–520 deliveries per frame. Within §14, but it is the tail to watch. If the gate gets tight, lower `MAX_BATCH_DELIVERIES` to 128–256 (one constant; the tests use it).
4. **Minor: only the first error in a batch is rethrown** (`client.ts` `receiveFrame`); later errors are swallowed without a log. Items after a failure still apply and the last seq is acknowledged, which is better than before. Fix: `console.error` the extra errors.
5. **Minor/nit: `deliver:*` timings aren't comparable between dev and branch.** Inside Solid's `batch()` effects run after the last item, outside `timed`, so part of the drop is moved work. Compare busy ms/frame and frame percentiles instead (unchanged).
6. **Nit:** the handoff says one acknowledgement per animation frame; `client.ts` uses its existing 16 ms timer. The added `clearTimeout(ackTimer)` on reattach is harmless.
7. **Nit:** `forward` never yields while the queue stays non-empty. Unchanged from before and bounded by the client's queue limit.
8. **Nit:** `a_lone_delivery_is_not_delayed` (< 50 ms) and `batching_keeps_order_and_sequence_numbers` (≤ 4 frames for 1,200) are timing assertions; both passed 40× under CPU load, but could flake on a very loaded CI machine.

## Checked and OK
- **Order and seq:** the batch is built from `recv` in order; `carry` keeps order across frames; tests assert +1 steps over 1,200 and 10,000 deliveries.
- **A gone sink ends forwarding** (`a_gone_sink_ends_forwarding_mid_stream`, exactly 2 deliveries). Replacing a window aborts the old task; at most one last batch reaches the old sink.
- **Acknowledgements** are cumulative in `state.rs`; acknowledging a batch's last seq frees it all; back-pressure unchanged (16 MiB limit vs ≤ 256 KiB batch).
- **Cancellation:** `try_recv_now` polls `recv()` once with a noop waker and drops it, which is safe because `Window::recv` is documented cancellation-safe. It depends on that property; a real `try_recv` in `atrium-client` would be sturdier (out of scope).
- **Latency:** a lone delivery is sent at once in the old shape; the byte bound is exact; an oversized first item is still sent.
- **`client.ts`:** both frame shapes accepted; items apply in order; malformed input fails as before; `solidBatch` is safe on Solid 1.9.17.
- **Memory drop** (web content peak 727–752 → 518–528 MiB): supported, not proven. Far fewer script evals and less signal churn mean less short-lived garbage; the numbers are 30 s peaks, so GC timing plays a part.

## Benchmark (orchestrator, macOS, perf build, GUI lock held)
10 threads × 1k tok/s × 30 s.

| | run | fps | p50/p95/p99/max ms | long frames | busy ms/frame | web content peak MiB | app+webviews peak MiB |
|---|---|---|---|---|---|---|---|
| dev | 1 | 59.9 | 17/18/18/38 | 2 (0.1 %) | 1.9 | 727 | 869 |
| dev | 2 | 60.0 | 17/18/18/19 | 0 | 1.8 | 752 | 873 |
| dev | 3 | 60.0 | 17/18/18/20 | 0 | 1.9 | 742 | 874 |
| branch | 1 | 60.0 | 17/18/18/18 | 0 | 1.8 | 518 | 657 |
| branch | 2 | 59.9 | 17/18/18/29 | 1 (0.1 %) | 1.9 | 528 | 709 |
| branch | 3 | 59.9 | 17/18/23/52 | 10 (0.6 %) | 4.5 | 524 | 654 |

Load average 3.6–4.6 for dev, 4.6–6.9 for the branch. All runs pass §14.

## What the reviewer ran
In the worktree, read-only: `cargo test -p atrium-app --lib -- bridge` 6× (16/16), then 40× under 8 CPU-spin processes and 5× with `--test-threads=1` (no failures or hangs); `pnpm --dir app test` 267/267; clippy clean. No GUI scripts, no real provider. After the fix the orchestrator re-ran the bridge tests (16/16), fmt and clippy.
