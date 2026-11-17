# M1-31-credential-folder-search: a recursive search of a folder holding a credential is blocked (S8)

Model: Gemini 4 Hydrogen (`gemini-4-hydrogen-high`, `scripts/agent-run agy`) · Reviewer: Claude Sonnet 5.5 (read-only, cross-provider) · Depends on: nothing open (runs on `dev` after M1-18)
Go: D-111; D-119 (Pedro: the batch split into M1-27 to M1-32) · Branch: `agent/hydrogen/M1-31-credential-folder-search` · Worktree: `../atrium-wt/M1-31-credential-folder-search`

## Goal
S8 (M1-13 finding 5): `rg x ~/.aws` (a recursive search of a folder that holds a hard-denied credential) only asks. The finding's test expects a block. Make a recursive search whose root contains a hard-denied path a hard deny, the same as naming that path.

## Context
- The ignored test `finding_5_a_recursive_search_of_a_folder_with_a_credential_file_is_blocked` (`crates/atrium-core/tests/adversarial_policy.rs` ~1065); M1-13's handoff for the finding.
- `crates/atrium-core/src/policy/deny.rs`: `Sensitive::check` and `search_glob_hit` (~1175–1520); the comment near ~545–560 on CR-12 stores in recursive searches (those stay Ask: D-082).

## Owned paths
- `deny.rs`: `check` through `search_glob_hit` (~1175–1520) only. Don't touch the store lists (~440–470), `probes`…`is_match` (~560–815), `PRIV` (~821) or the glob normalizer (~1518 on).
- In `adversarial_policy.rs`: only remove the `#[ignore]` at ~1065.
- `crates/atrium-core/tests/M1-31-credential-folder-search.rs`; the handoff.

## What to do
1. A recursive search (rg, grep -r, find, fd, ag, ack, git grep with a pathspec, and the `Paths { recursive: true }` subject) whose root is a folder that contains a hard-denied path (`~/.aws`, `~/.ssh`, `~/.gnupg`, …, and their ancestors up to but not including `~` itself) is a hard deny. A search of `~` itself keeps `dev`'s verdict (it would make every home-wide search a hard deny). CR-12 stores keep asking (D-082).
2. Tests: each tool and spelling on each hard-denied folder, relative and home-anchored; non-recursive reads of the folder listing (`ls ~/.aws`) keep `dev`'s verdict; ordinary recursive searches (`rg x src`, `rg x ~/proj`, `rg x ~`) keep `dev`'s verdict. Un-ignore finding 5.

## Added scope (D-121: reopened known limit, done in this task's next round)
Owned paths widen to the glob normalizer in `deny.rs` (`apply_dotdot`, `normalize_glob_variants` and their helpers, ~1518–2075).
- **K10:** search globs with 400 or more `*/` segments followed by as many `..` take a few seconds to evaluate (cost roughly quadratic until the variant cap trips). Make the normalizer's cost linear or bounded per glob (for example, a work budget that fails closed to Ask once spent, or collapsing runs of `wildcard/..` in one step), so any glob up to the input size limit evaluates in well under 50 ms, with the same verdicts.

## Rules
- Work only in your worktree; commit on your branch only. Never push, merge or rebase. Never run a real provider.
- Five other tasks (M1-27 to M1-32) change the policy code at the same time. Stay inside your owned paths and the line ranges named there. If the fix needs a change elsewhere, stop that part and say so in the handoff: don't make it.
- New tests go in your own new test file, `crates/atrium-core/tests/M1-31-credential-folder-search.rs`. In existing test files, only remove the `#[ignore]` lines named in your brief.
- The policy rules: no line more permissive than `dev`; no new hard deny on an ordinary line that `dev` allows or asks about. Fail closed where you can't read something fully.
- Don't create extra target directories or copies of the repo (the disk is small). Remove scratch output you create.

## Definition of Done
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast`.
- [ ] `cargo test -p atrium-core` 3 times in a loop, with the times of `adversarial_fuzz` and the `policy` binary (no slowdown).
- [ ] Show your new tests (and the un-ignored ones) fail on `dev`'s code and pass now: check out `dev`'s versions of the src files you changed, run, restore.
- [ ] `git diff dev --stat` touches only your owned paths.
- [ ] Handoff `docs/agents/handoffs/M1-31-credential-folder-search.md`: what changed and where, a verdict table (dev vs now) for every line you tested, ordinary-line checks, the outputs, anything escalated.

## Added scope (D-120, orchestrator, 2026-11-16): N9
`adversarial_regression.rs` `round_5_blocker_1_searches_and_walkers_reaching_a_folder_linked_into_ssh` fails on Linux on `dev` too. Its fixture lives under `std::env::temp_dir()`, which is `/tmp` on Linux, so the "ordinary" lines `cd /tmp && rg x` and `find /tmp -name x` really reach the fake `.ssh`. Change only those two lines of the test: point them at a folder outside the fixture (a fresh empty temp folder, made the same way). Don't weaken anything else.

## Added scope (D-120, orchestrator, 2026-11-17): N11
On Linux, `dev` itself fails two timing tests even on a quiet machine:
- `adversarial_fuzz::random_hostile_lines_never_panic_and_nothing_sensitive_is_allowed`: 221 s, against a limit of 60 s;
- `adversarial_m1_18::fix4_finding3_glob_escape_total_variant_cap_performance`: 322 ms, against 150 ms.

Both pass on macOS. Find where the time goes: most likely the glob normalizer or the variant expansion, which this task owns. Bound it so both pass on Linux with their current limits. Don't raise the limits. If the time is outside your paths, name the function in the handoff.
