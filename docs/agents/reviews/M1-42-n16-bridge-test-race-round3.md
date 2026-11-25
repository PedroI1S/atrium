# M1-42-n16-bridge-test-race: review round 3

Reviewer: Claude Sonnet 5.5, read-only (D-162). Reviewed: round 4 (`01b27896`).

Orchestrator's verification (macOS): fmt and clippy clean; `bridge::` ×300 with 0 failures (23 tests). Workspace: 892 passed and 1 failed. The failure is N17, an `atrium-host` timing flake under load that isn't in M1-42's code (0 failures in 30 reruns on `dev`).

## Verdict
**Request changes.** The fake matches `Hub::subscribe_inbox`, and every pushing test waits first.
- `live` and `answered` update with no receiver, under the `peers` lock.
- A repeated `SubscribeInbox` raises only `answered`.
- The handoff's revert snippet type-checks, and the regression property holds.

Rounds 1 and 2 are closed, except round 2's finding 3, which is only partly closed.

## Findings
1. **Minor: `stale_peer_disconnect_requires_wait_inbox_answered_for_new_connection` doesn't create a stale peer.**
   - **Why:** nothing waits for A's disconnect handler, so A may already be gone when B connects, and the test passes with either helper. There is also no `live` or `peers` assertion while B is gated. If B registered early, `before-b` would be wiped by the late empty answer, and `["after-b"]` would still hold.
   - **Fix:** build the stale peer deterministically, as `push_prunes_dead_peer_and_drops_live_count` does.
     - Insert `(u64::MAX, tx)` into `peers` with its receiver kept alive, and bump `live`.
     - Gate B's answer.
     - Assert that `inbox_subscribers(1)` polls Ready, `wait_inbox_answered(mark + 1)` polls Pending, and `peers.len() == 1`.
     - Push `before-b`, release the gate, wait with `mark + 1`, assert `peers.len() == 2`, and keep the `after-b` check.
     - Update the handoff's wording to match.

## Test gaps
- Only finding 1's.
