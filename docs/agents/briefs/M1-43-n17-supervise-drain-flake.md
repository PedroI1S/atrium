# M1-43-n17-supervise-drain-flake: fix the `kill_recorded` drain test flake (N17)

Model: Gemini 4 Neon (`gemini-4-neon-medium`, `scripts/agent-run agy`, D-158) · Reviewer: Claude Sonnet 5.5, read-only (D-162)
Go: needs the driver's go · Branch: `agent/neon/M1-43-n17-supervise-drain-flake`, cut from `dev` · Worktree: `../atrium-wt/M1-43-n17-supervise-drain-flake`

## The flake (N17 in `docs/agents/m1-must-fix.md`)
`crates/atrium-host/tests/supervise.rs`, `kill_recorded_returns_verified_when_gone_leader_group_drains_within_timeout`, failed once on macOS:
- **The failure:** `assert_eq!(cleanup, Cleanup::Verified)` at line 369 got `Unverified`.
- **When:** M1-42's verification, 2026-11-24, at load average ~6.6, while a worker was building.
- **Since then:** it passed 30 of 30 reruns on `dev` at load ~3.

**What the test does:**
- `spawn_gone_leader_with_holding_member` spawns a leader, which is this test binary re-executed. The leader spawns a group member, which is the test binary again, holding the leader's stdin.
- The test reaps only the leader.
- A thread drops stdin after 150 ms.
- `kill_recorded` sees the leader gone (`identify` → `Ok(None)`). It waits up to `GROUP_DRAIN_TIMEOUT` (2 s, `src/supervise.rs:46`) in `wait_for_group_drain_sync`, then returns `Verified` only if `group_is_empty(pgid)`.

**What's suspected, unconfirmed:** under load the member doesn't finish exiting within 2 s of the 150 ms release. Several things could cause that:
- it is the full test binary, so its start and exit are slow;
- it was reparented to launchd/init after the leader was reaped, so its reap depends on another process;
- `group_is_empty` may count a zombie, or ESRCH/EPERM may be read differently.

## Goal
1. **Find the cause.** Read `wait_for_group_drain_sync`, `group_is_empty`, the helper and the test. Say in the handoff, with evidence from the code, which step can exceed the timeout under load. Is it the member's exit, the zombie reap, or how the group check reads it? If a zombie member counts as "not empty", say so: that is a product question as well as a test one.
2. **Make the test deterministic without weakening what it pins.** "A gone leader whose group drains in time gives `Verified`, and the member and group are gone afterwards." The preferred fixes are test-side:
   - a lightweight member (for example `sh -c 'read _'` or `cat`, holding stdin) instead of a second test-binary process;
   - waiting for a readiness signal that is already in place before starting the clock;
   - or anything else that removes the dependence on scheduler timing.
3. **Rules for the fix:**
   - **Don't raise `GROUP_DRAIN_TIMEOUT`,** don't loosen the assertion, and don't add retries.
   - If the product code is wrong, for example a zombie counted as alive, fix it in `src/supervise.rs` with a test that pins it. Keep the sibling tests' meaning, above all `kill_recorded_returns_unverified_and_leaves_member_alive_when_drain_times_out`.
   - If the only real fix would change a public signature, don't make it. Write a change request in the handoff.
4. **Check the sibling tests** in `supervise.rs` that use the same helper for the same timing dependence, and fix them the same way.

## Owned paths
- `crates/atrium-host/tests/supervise.rs`
- `crates/atrium-host/src/supervise.rs` (only if goal 3 applies)
- `docs/agents/handoffs/M1-43-n17-supervise-drain-flake.md`

## Rules
- Follow `docs/agents/README.md`. Under D-123, run only `cargo fmt --all` and `cargo check -p atrium-host --tests`. No `cargo test`; the orchestrator runs it.
- macOS and Linux are both supported, so every `cfg` builds on both.
- Commit on your branch only. Never push, merge, rebase or switch branches.

## Definition of Done (the orchestrator's verification)
- [ ] fmt, workspace clippy and `cargo test --workspace` are clean on macOS.
- [ ] The `supervise` tests ×200 have 0 failures, including under load: a parallel `cargo build` of another worktree, or `stress`-style CPU load.
- [ ] With the fix reverted, the handoff's repro makes the original failure more likely, or the handoff explains why it can't be reproduced on demand.
- [ ] Sonnet's review approves with no findings (D-120).
