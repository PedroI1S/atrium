# M1-42-n16-bridge-test-race: the bridge tests' fake daemon pushes before it answers `SubscribeInbox` (N16)

Model: Gemini 4 Neon (`gemini-4-neon-medium`, `scripts/agent-run agy`, D-158) · Reviewer: Claude Sonnet 5.5, read-only (D-162; Codex when its usage is back) · Depends on: nothing open
Go: needs the driver's go · Branch: `agent/neon/M1-42-n16-bridge-test-race` · Worktree: `../atrium-wt/M1-42-n16-bridge-test-race`

## Goal
Make `app/src-tauri/src/bridge.rs`'s tests deterministic by fixing the race the orchestrator found. The evidence is N16 in `docs/agents/m1-must-fix.md`. In short:
- **The fake pushes too early.** `Scripted` (the fake daemon in `bridge.rs`'s test module) adds each connection to `peers` as soon as it connects, and `Scripted::push` sends to every peer. The tests call `push` right after `client_connected`, which waits only for `ConnectionState::Connected`.
- **What happens then.** If the fake hasn't answered `SubscribeInbox` yet, the updates arrive before the answer. The answer's empty `threads` list then replaces them in the client (`crates/atrium-client/src/state.rs` ~501, ~567, ~678). The window gets an empty inbox, and the test waits forever.
- **The fix belongs in the fake, not in the client.** The real daemon queues the answer and registers the connection for updates under one lock (`crates/atrium-daemon/src/hub.rs` `subscribe_inbox`), and the fake must keep the same rule.

## What to do
1. **Make `Scripted` keep the daemon's ordering.** A connection becomes a push target only once its `SubscribeInbox` answer has been queued on that connection's writer, in that order. Before that, `push` must not send to it.
2. **Add a wait so a push is never lost.** For example, `Scripted::inbox_subscribers(n).await` waits, with a 5 s timeout, until `n` connections have had their `SubscribeInbox` answered. Use it in every test that pushes, between connecting and the first `push`. Every test that calls `daemon.push` must call it; today that's `batching_keeps_order_and_sequence_numbers`, `a_lone_delivery_is_not_delayed`, `batch_bounds_are_respected_for_count_and_bytes`, `a_gone_sink_ends_forwarding_mid_stream` and `micro_measurement_frames_per_10000_deliveries`. Check for others.
3. **Add a permanent regression test.** It needs a test-only option on `Scripted`, such as `Scripted::start_with(answer, Options { inbox_delay: Duration })`, that waits before answering `SubscribeInbox`.
   - Run the push-then-receive tests' logic, or a test of its own, with a 200 ms delay. It must pass.
   - Without step 1, the same test must fail. The orchestrator checks that by reverting step 1.
4. **No change to the client, the bridge's non-test code, or the daemon.** If you find that the product can lose an update this way, stop and write it in the handoff.

## Owned paths
- `app/src-tauri/src/bridge.rs`, **the `#[cfg(test)] mod tests` only**.
- The handoff, `docs/agents/handoffs/M1-42-n16-bridge-test-race.md`.

## Rules
- Follow the worker rules in `docs/agents/README.md`. Under D-123, run only `cargo fmt --all` and `cargo check -p atrium-app --tests`. No `cargo test`, `cargo clippy` or `pnpm`: the orchestrator runs the tests.
- Never raise a timeout or retry to hide the race, and never weaken an assertion.

## Definition of Done (the orchestrator's verification)
- [ ] fmt, workspace clippy and `cargo test --workspace --no-fail-fast` are clean.
- [ ] `bridge::` runs 300 times in a row with no failure.
- [ ] The regression test fails with step 1 reverted.
- [ ] The review approves with no findings (D-120).
