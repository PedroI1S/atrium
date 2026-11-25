# M1-42-n16-bridge-test-race: review round 1

Reviewer: Claude Sonnet 5.5, read-only (D-162). Reviewed: rounds 1 and 2 (`968f4a10`, `e2d8cca7`).

Orchestrator's verification (macOS): fmt and clippy clean; workspace 888/0/7; `bridge::` ×300 with 0 failures. With step 1 reverted, `delayed_subscribe_inbox_does_not_lose_pushed_updates` fails at `next_raw` ("a delivery arrives").

## Verdict
**Request changes.** The core fix is correct and matches `Hub::subscribe_inbox`: the answer is queued and the peer registered under one lock, with no `.await` while it is held. Every `push` is preceded by `inbox_subscribers(1)`, and `send_replace` is right. Only the test module and the handoff changed. Under D-120 the three minor findings and the test gaps go back as rework.

## Findings
1. **Minor: `inbox_subscribers(n)` is `>= n`, so a dead connection that hasn't been pruned can satisfy it.** In a re-attach on a new connection, the stale peer counts, the wait returns early, and the push reaches only the stale peer. That is N16 again, latent. Fix: make the wait robust, for example a monotonic "subscriptions answered" counter and a wait for one more than before, or document and test the contract.
2. **Minor: the `elapsed() < 500ms` assertion in `inbox_subscribers_returns_immediately_if_already_subscribed` catches nothing.** A regression panics at the 5 s timeout first, so the only way it fails is a scheduler stall, which makes it a new flake source. Fix: drop it.
3. **Minor: the `receiver_count() == 0` precondition needs a comment** saying why it is asserted.

## Test gaps
- The disconnect path's count update, with no receiver alive.
- `push`'s `send_if_modified` pruning when a peer's writer is gone.
- Several connections: two clients, and a re-attached window on a new connection with a delayed answer, where the push reaches B only after B's answer.
- The handoff's round 1 revert snippet still shows `send`. Mark it superseded.
