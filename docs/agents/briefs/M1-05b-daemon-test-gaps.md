# M1-05b-daemon-test-gaps: the tests M1-05's late-fix review found missing

Model: Gemini 4 Hydrogen (`gemini-4-hydrogen-medium`, `scripts/agent-run agy`; D-074) · Reviewer: Sonnet 5.5 (read-only) · Depends on: M1-05 (merged)
Go: D-075 (Pedro, 2026-11-14) · Branch: `agent/hydrogen/M1-05b-daemon-test-gaps` · Worktree: `../atrium-wt/M1-05b-daemon-test-gaps`

## Goal
Close the test gaps of `docs/agents/reviews/M1-05-daemon-late-fixes.md` (findings 1 and 4) so the daemon's full-storage and writer rules are pinned by tests that would fail on the old code. Remove the duplicate select arm (finding 5). No other behaviour change.

## Context
- `docs/agents/reviews/M1-05-daemon-late-fixes.md`: read findings 1, 4 and 5 in full. They name the files, lines and the exact cases.
- `docs/agents/handoffs/M1-05-daemon.md` ("Flash review", "Fixed afterwards") and the existing tests in `crates/atrium-daemon/tests/` (`sessions.rs`, `socket.rs`, `common/`).
- `docs/contracts/m1.md` §5, §6.1 and §10.

## Owned paths
- New test files: `crates/atrium-daemon/tests/gaps_*.rs` (import the existing helpers with `#[path = "common/mod.rs"] mod common;`).
- Test code only inside the existing `#[cfg(test)]` modules of `crates/atrium-daemon/src/conn.rs`, `src/upkeep.rs` and `src/sessions.rs`, if a test needs private functions.
- One code change: delete the duplicate `stop.wait_for` select arm in `src/sessions.rs`'s first-message task (finding 5).
- `docs/agents/handoffs/M1-05b-daemon-test-gaps.md`

## What to do
1. **Finding 1, the full-storage test.** A new test, alongside the old one:
   - store a `MessageStarted` and a delta (and, in a second case, a `ToolCallStarted`) before space runs out;
   - then send one batch holding more deltas, the `MessageCompleted` (or `ToolCallCompleted`), `TurnCompleted` and `SessionEnded { exit_code: Some(0) }`, which the full store refuses;
   - assert what the daemon stores in its place: the `MessageCompleted` (`Streamed`) or the closed tool call, `TurnCompleted { outcome: Interrupted }` and `SessionEnded` with the "storage is full" reason; that nothing is left open in the incarnation; and that `seq` is contiguous.
   - It must fail on the code before `a27d432` (check by reading that diff; say in the handoff why it would fail).
2. **Finding 4, the writer.**
   - A `write_loop` test with a peer that reads slowly, so a frame is written in parts. The subscription ends while the frame is half sent. The peer must receive the whole frame, never a cut one, and nothing of that subscription after it.
   - A test that `first` is true only before any byte of a frame has been written.
   - If the snapshot ordering (`upkeep.rs`, the `publish` lock around the snapshot) can be tested without timing tricks, test that a turn that starts during a snapshot is either in it or stored after it. Otherwise say why not in the handoff.
3. **Finding 5.** Delete the duplicate arm; the tests still pass.

## Definition of Done
- [ ] The new tests pass, and each new test binary passes 20 times in a row (`for i in 1 2 … 20`-style loops are fine with `cargo test`; no shell substitutions, the worker policy refuses them).
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test -p atrium-daemon`.
- [ ] Handoff: each finding, the test names, why each would fail on the old code, the outputs.

## Rules
Work only in your worktree. Tests use temp folders; no real home paths in any file (the worker policy refuses path-like text such as `~/…` or `../` in file contents; you shouldn't need any). Commit on your branch only; never push, merge or rebase. Stop every daemon a test starts.

## Escalate if
A gap can't be tested without changing daemon code beyond finding 5: leave it, and say what change would make it testable.
