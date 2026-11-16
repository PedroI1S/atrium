# M1-30-tree-wildcards: a wildcard in a credential or Atrium folder name counts as touching it (S5)

Model: Gemini 4 Hydrogen (`gemini-4-hydrogen-high`, `scripts/agent-run agy`) · Reviewer: Claude Sonnet 5.5 (read-only, cross-provider) · Depends on: nothing open (runs on `dev` after M1-18)
Go: D-111; D-119 (Pedro: the batch split into M1-27 to M1-32) · Branch: `agent/hydrogen/M1-30-tree-wildcards` · Worktree: `../atrium-wt/M1-30-tree-wildcards`

## Goal
S5 (M1-13 finding 11): a wildcard in the name of a credential tree or Atrium folder (`~/.g*/…`, `~/Library/Application Support/atr*/…`, `~/.s?h/id_rsa`, `~/.[s]sh`) must count as touching it, the same as the plain name.

## Context
- The ignored tests `finding_11_a_glob_inside_atriums_folder_names_is_blocked` (`crates/atrium-core/tests/adversarial_gate.rs` ~582) and `random_spellings_of_sensitive_paths_are_always_hard_denied` (`adversarial_fuzz.rs` ~422).
- `crates/atrium-core/src/policy/deny.rs`: `Sensitive::probes`, `glob_hit`, `ask_probes`, `raw_hit` and the pattern type below them (~560–815); `ask_roots`/`sensitive_roots` (~2151–2182).

## Owned paths
- `deny.rs`: lines ~560–815 (`probes` … the pattern type's `is_match`) and `ask_roots`/`sensitive_roots` at the end. Don't touch the store lists (~440–470), `PRIV` (~821), `check`/`search_glob_hit` (~1175–1520) or the glob normalizer (~1518 on): other tasks own them.
- In `adversarial_gate.rs` and `adversarial_fuzz.rs`: only remove the `#[ignore]` lines at ~582 and ~422.
- `crates/atrium-core/tests/M1-30-tree-wildcards.rs`; the handoff.

## What to do
1. When a path component of a word or glob is a pattern, test it against the names of the hard-denied and ask trees (and Atrium's folders) as a pattern, not as a literal: if it could match a sensitive name, treat the path as touching that tree, with the same verdict as the plain name.
2. Tests: `*`, `?`, classes and braces in each component of each tree name, home-anchored and relative, in reads, searches and command words; ordinary globs (`~/.config/*/settings.json`, `~/Documents/*.md`, `src/*.rs`) keep `dev`'s verdict. Un-ignore both finding-11 tests.

## Rules
- Work only in your worktree; commit on your branch only. Never push, merge or rebase. Never run a real provider.
- Five other tasks (M1-27 to M1-32) change the policy code at the same time. Stay inside your owned paths and the line ranges named there. If the fix needs a change elsewhere, stop that part and say so in the handoff: don't make it.
- New tests go in your own new test file, `crates/atrium-core/tests/M1-30-tree-wildcards.rs`. In existing test files, only remove the `#[ignore]` lines named in your brief.
- The policy rules: no line more permissive than `dev`; no new hard deny on an ordinary line that `dev` allows or asks about. Fail closed where you can't read something fully.
- Don't create extra target directories or copies of the repo (the disk is small). Remove scratch output you create.

## Definition of Done
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast`.
- [ ] `cargo test -p atrium-core` 3 times in a loop, with the times of `adversarial_fuzz` and the `policy` binary (no slowdown).
- [ ] Show your new tests (and the un-ignored ones) fail on `dev`'s code and pass now: check out `dev`'s versions of the src files you changed, run, restore.
- [ ] `git diff dev --stat` touches only your owned paths.
- [ ] Handoff `docs/agents/handoffs/M1-30-tree-wildcards.md`: what changed and where, a verdict table (dev vs now) for every line you tested, ordinary-line checks, the outputs, anything escalated.
