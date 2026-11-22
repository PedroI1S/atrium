# M1-34-n9-fixture: N9, the `/tmp` fixture in `round_5_blocker_1`

Extraction: the orchestrator (D-136), no writer · Reviewer: Codex GPT-6.1 Sol `high`, read-only (D-124) · Depends on: nothing open
Go: D-136 · Branch: `agent/orchestrator/M1-34-n9-fixture` · Worktree: `../atrium-wt/M1-34-n9-fixture`

## Goal
N9 (`m1-must-fix.md`): on Linux, `adversarial_regression::round_5_blocker_1_searches_and_walkers_reaching_a_folder_linked_into_ssh` fails on `dev` every run, because two of its "ordinary" lines search `/tmp`, which holds the fixture's own fake home. M1-31's commit `bce1a5a` points those lines at an empty folder outside the temp home instead; the assertions are unchanged.

## What it takes
`bce1a5a` from `agent/hydrogen/M1-31-credential-folder-search`, cherry-picked (it touches only `crates/atrium-core/tests/adversarial_regression.rs`).

## Definition of Done (the orchestrator's verification, D-123)
- [ ] fmt, workspace clippy, `cargo test --workspace --no-fail-fast`; `round_5_blocker_1` passes on Linux.
- [ ] `git diff dev --stat` is that one file.
- [ ] Codex review: approve with no findings (D-120).
