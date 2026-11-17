# M1-32-link-globs: globs that reach a credential through a symlink; `rg -L`'s false block (S11, K5)

Model: Gemini 4 Hydrogen (`gemini-4-hydrogen-high`, `scripts/agent-run agy`) · Reviewer: Claude Opus 5.5 (read-only, cross-provider; security) · Depends on: nothing open (runs on `dev` after M1-18)
Go: D-111; K5 reopened under D-121; D-119 (Pedro: the batch split into M1-27 to M1-32) · Branch: `agent/hydrogen/M1-32-link-globs` · Worktree: `../atrium-wt/M1-32-link-globs`

## Goal
Two symlink items, in the same code:
- S11 (M1-13 finding 10): a glob that reaches a credential only through a symlink (a link in the project pointing into `~/.ssh`, matched by `*/id_rsa`) isn't caught.
- K5: `rg -L pat` with no folder, run from a working folder that holds a link into `~/.ssh`, is a hard deny (`dev`: Ask/Allow/Allow). Make it ask like `dev`'s Home verdict instead (a hard deny there is a false block on an ordinary line).

## Context
- The ignored test `finding_10_a_glob_that_reaches_a_credential_through_a_link_is_blocked` (`crates/atrium-core/tests/adversarial_gate.rs` ~571); K5 in `docs/agents/m1-must-fix.md`.
- `crates/atrium-core/src/policy/gate.rs`: `reaches_tree_through_links` (~1379) and its callers (~320–350); the path resolver (~40–80).

## Owned paths
- `gate.rs`: `reaches_tree_through_links` and its helpers, and its callers (~320–350). Nothing in `shell.rs`, `deny.rs` or `engine.rs` (other tasks own them).
- In `adversarial_gate.rs`: only remove the `#[ignore]` at ~571.
- `crates/atrium-core/tests/M1-32-link-globs.rs`; the handoff.

## What to do
1. S11: when a glob or a recursive search could follow a symlink below its root, resolve the links it could reach (bounded, as the existing code does) and match the glob's remaining components against the link targets, so a credential reached only through a link gets the same verdict as naming it.
2. K5: `rg -L` (and `--follow`, `grep -R`, `find -L`) with no folder named, from a folder that holds a link into a sensitive tree: ask in Home (`dev`'s verdict), not a hard deny; keep the hard deny when the line names the credential.
3. Tests: one-hop and two-hop links, dangling links, links to ancestors of a tree, globs with `*`/`**`/classes, all tools with follow flags; ordinary searches in a project with links to other project folders keep `dev`'s verdict. Un-ignore finding 10.

## Rules
- Work only in your worktree; commit on your branch only. Never push, merge or rebase. Never run a real provider.
- Five other tasks (M1-27 to M1-32) change the policy code at the same time. Stay inside your owned paths and the line ranges named there. If the fix needs a change elsewhere, stop that part and say so in the handoff: don't make it.
- New tests go in your own new test file, `crates/atrium-core/tests/M1-32-link-globs.rs`. In existing test files, only remove the `#[ignore]` lines named in your brief.
- The policy rules: no line more permissive than `dev`; no new hard deny on an ordinary line that `dev` allows or asks about. Fail closed where you can't read something fully.
- Don't create extra target directories or copies of the repo (the disk is small). Remove scratch output you create.

## Definition of Done
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast`.
- [ ] `cargo test -p atrium-core` 3 times in a loop, with the times of `adversarial_fuzz` and the `policy` binary (no slowdown).
- [ ] Show your new tests (and the un-ignored ones) fail on `dev`'s code and pass now: check out `dev`'s versions of the src files you changed, run, restore.
- [ ] `git diff dev --stat` touches only your owned paths.
- [ ] Handoff `docs/agents/handoffs/M1-32-link-globs.md`: what changed and where, a verdict table (dev vs now) for every line you tested, ordinary-line checks, the outputs, anything escalated.

## Scope change (D-125, rodrigo, 2026-11-16)
K5 is closed as correct behaviour: the hard deny stays. This task's scope is S11 only.

## Added scope (D-120, orchestrator, 2026-11-17)
The Opus escalation left two items outside the owned range, and a review would flag them. The owned paths widen to:
- `gate.rs` `resolve_fs` (~84): resolve links in order before `..`, as the owned range now does;
- `alias_fix`: match hard-linked credentials by identity for glob matches too, not only for directly named paths.
