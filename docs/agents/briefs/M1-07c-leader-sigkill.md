# M1-07c-leader-sigkill: kill a provider leader that left its process group (N8)

Model: Claude Opus 5.5 (Claude subagent; D-076: process-signal and possibly `unsafe` work in `atrium-host`) · Reviewer: Gemini 4 Hydrogen `high` (read-only, cross-provider) · Depends on: M1-07b (merged)
Go: D-091 (Pedro, 2026-11-15) · Branch: `agent/opus/M1-07c-leader-sigkill` · Worktree: `../atrium-wt/M1-07c-leader-sigkill`

## Goal
Close N8 (`m1-must-fix.md`; `reviews/M1-07b-proc-drop-linux.md`, finding 1): a provider leader that leaves its own process group (`setpgid`/`setsid` after `spawn_group`) is never sent `SIGKILL`, so `terminate` and `terminate_blocking` wait until it exits on its own, and a synchronous `Drop` blocks its caller.

## What to do
1. In `crates/atrium-host/src/supervise.rs`, in both `terminate` and `terminate_blocking`: after the group kill, also `SIGKILL` the leader by pid while it is still an unreaped child (its pid can't be reused until it is reaped, the same argument the group kill already relies on). Keep the macOS `EPERM` zombie handling exactly as it is.
2. Prefer an existing safe signal helper; if a new `unsafe` block is unavoidable, give it a `// SAFETY:` comment and say so in the handoff (D-053: the review then audits it).
3. The review's nits: the `ECHILD` arm that can never succeed, `let _ = pid`, and the doc comments giving the blocking bounds.
4. Tests in `crates/atrium-host/tests/supervise.rs`, real processes, both platforms' code paths where they differ:
   - a leader that calls `setsid` (or `setpgid(0,0)`) and then sleeps: both `terminate` and `terminate_blocking` return promptly with `killed == true`, and the leader and the old group are gone;
   - the review's missing cases: a group holding only the zombie leader (the macOS `EPERM` path), and `grace > 0` with a leader that ignores `SIGTERM` (`killed == true` after the grace).
   Check that the first test fails (hangs or times out, bounded) on `dev`'s code; say how in the handoff.
5. Run the touched test binaries 20 times in a row on this Mac.

## Owned paths
`crates/atrium-host/src/supervise.rs`, `crates/atrium-host/tests/supervise.rs`, `docs/agents/handoffs/M1-07c-leader-sigkill.md`.

## Definition of Done
`cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test -p atrium-host -p atrium-agents`; the 20-run loops; the handoff with outputs.

## Rules
Commit on your branch only; never push, merge or rebase. Never signal anything you didn't spawn in the test. Other workers build on this Mac: one cargo command at a time.
