# M1-42-n16-bridge-test-race: review round 2

Reviewer: Claude Sonnet 5.5, read-only (D-162). Reviewed: round 3 (`d00e0116`).

Orchestrator's verification (macOS): fmt and clippy clean; workspace 891/0/7; `bridge::` ×300 with 0 failures.

## Verdict
**Request changes.** The fake matches `Hub::subscribe_inbox`, all subscriber-state updates are under the `peers` lock, and `send_modify` stores the value with no receiver. Round 1's three findings and its disconnect, pruning and two-client gaps are closed. The regression property holds by reading. There is no correctness defect. Three minor findings go back under D-120.

## Findings
1. **Minor: the handoff's "Done" and "Round 2" sections are stale.** "Done" still describes `watch::Sender<usize>` and `count >= n`. Round 2 still describes the removed `< 500ms` assertion. Round 2's revert step 3 uses `subscribers_conn.send_replace(peers.len())`, which no longer type-checks. Fix: mark them superseded, and update "Done" to the `InboxSubscribers { live, answered }` design.
2. **Minor: `inbox_subscribers_returns_immediately_if_already_subscribed` has a misleading name and no cleanup.** Fix: rename it, for example `subscriber_count_is_stored_without_a_watcher`, and shut the bridge down at the end like the sibling tests.
3. **Minor: the second-connection test doesn't exercise a stale peer.** A is never disconnected, so the test would also pass with `inbox_subscribers(1)` for B, and the 200 ms delay is redundant next to the gate. Fix: add a variant where A disconnects, then B connects with its answer gated. Show that `inbox_subscribers(1)` alone isn't enough and `wait_inbox_answered(mark + 1)` is. Drop the redundant delay, or comment why it's there.

## Test gaps
- The stale-peer scenario (finding 3).
- A repeated `SubscribeInbox` on one connection: `answered` goes up, `live` doesn't (the `!subscribed` branch).
