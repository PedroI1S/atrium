# M1-42-n16-bridge-test-race: review round 4

Reviewer: Claude Sonnet 5.5, read-only (D-162). Reviewed: round 5 (`1e59e5eb`).

Orchestrator's verification (macOS): fmt and clippy clean; `bridge::` ×300 with 0 failures (23 tests). Workspace: 931 passed and 1 failed. The failure is N18, an `atrium-agents` load flake that isn't in M1-42's code (0 failures in 30 reruns on `dev`).

## Verdict
**Approve.** No findings. Review 3's finding is closed: `stale_peer_requires_wait_inbox_answered_for_new_connection` now builds the stale peer deterministically. While B is gated, it checks that `inbox_subscribers(1)` is Ready, `mark + 1` is Pending and `peers.len() == 1`, and after the release that `peers.len() == 2`.
- **Ordering:** the fake copies `Hub::subscribe_inbox`, and every `push` site waits first.
- **Counts:** `send_modify` and `send_if_modified` work with no receivers, and the disconnect, prune and repeated-subscribe cases are all pinned.
- **Regression:** the property holds, and the handoff's revert snippet type-checks.
- **Scope:** only `mod tests` in `bridge.rs` and the handoff changed. No timeout was raised and no assertion was weakened.

## Findings
None.

## Test gaps
None required. Optional: a `Control::Reconnect` variant of the second-connection test. The fake's per-connection logic is the same either way.
