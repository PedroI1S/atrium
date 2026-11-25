# M1-43 review round 3 — Claude Sonnet 5.5 (Claude Code subagent on the driver's plan, read-only; D-170 fallback)

Branch at `b2426ff2`. Orchestrator's verification: fmt/clippy clean; workspace 932/0/7; `supervise` ×200 quiet 0 failures; ×100 under 2×ncpu load (up to 72) 0 failures, with every failing run's log kept (none).

## Verdict

**Approve.** All 9 round-2 Lows and the zombie test gap are fixed in `5157154b` and `b2426ff2`, with no new defects. Only `crates/atrium-host/tests/supervise.rs` and the handoff changed; no `src/` change, `GROUP_DRAIN_TIMEOUT` untouched, no retries. The member now joins the leader's group while the leader is alive, as the test's own child and reaped by the test, and is released by an explicit `bye\n` message. That removes the PID 1 reap hop, the macOS CLOEXEC EOF window and the heavyweight `current_exe` startup. The one loosened bound (the Verified test's `< GROUP_DRAIN_TIMEOUT + 3s`) is what round 2's finding 9 asked for, and pins nothing the `Verified`, `!pid_exists` and `!group_exists` assertions don't already pin.

The cause in the handoff matches `src/supervise.rs`: `group_is_empty` is true only on `ESRCH`; a zombie counts as "not empty" on both platforms (Linux `0`, macOS `EPERM`, which `kill(2)` can't tell apart from another UID's group that reused the pgid). That is fail-safe for `Verified`, so no product change.

## Findings

None.

Non-findings: the 50 ms release can elapse before `kill_recorded` starts polling under a long preemption; the test still passes with `Verified` but that run skips the loop. The `< 5 s` bounds are pure hang guards.

## Remaining wall-clock dependence (check 5)

None flaky by construction. The Verified test needs the releaser thread, the member's exit and the test's own reap to land within the 2 s window, so it fails only on a stall of about 1.9 s. The 5 s bounds, handshake timeout and zombie wait are hang guards. The sibling's `>= 2 s` lower bound is deterministic. The new zombie test (`kill_recorded_treats_unreaped_zombie_member_as_unverified_until_reaped`) is deterministic on both platforms, because only the test reaps the member, and it exercises the macOS `EPERM` gone-leader branch end to end.

## Test gaps

- No test of the handshake timeout's own failure message (a defensive guard).
- The Verified test's polling path isn't pinned deterministically; that would need a product hook showing `kill_recorded` has entered its loop.
- The other PID-1-reap tests (`supervise.rs` `:69`, `:119`, `:517`, `:615`, `:687`) still depend on init reaping within 2 s. The handoff lists them as out of scope; a failure there is not an N17 regression.
