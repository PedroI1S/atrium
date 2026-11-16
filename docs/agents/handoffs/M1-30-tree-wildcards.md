# Handoff: M1-30-tree-wildcards (partial)

Status: **partial**. The Hydrogen worker (Gemini 4 Hydrogen, high) stopped on 2026-11-16 during its last verification loop, before committing. The orchestrator committed its uncommitted work as WIP (this commit) so nothing is lost.

## What's in the WIP commit

- `crates/atrium-core/src/policy/deny.rs`: +753/−42. S5's wildcard matching for credential and Atrium folder names, including a fix to the `**` tree-leaf rule the worker made late in the run.
- `crates/atrium-core/tests/M1-30-tree-wildcards.rs`: new test file.
- `crates/atrium-core/tests/adversarial_gate.rs`: un-ignores `finding_11_a_glob_inside_atriums_folder_names_is_blocked` (M1-13 finding 11).

## State of verification (unconfirmed)

From the worker's last messages: it had two failing tests (in `M1-30-tree-wildcards` and `adversarial_gate`), then fixed the `**` rule and reran `adversarial_m1_18`, `adversarial_regression` and `M1-30-tree-wildcards`. It was running fmt, clippy, the workspace tests and a 3× `cargo test -p atrium-core` timing loop (`adversarial_fuzz`, `policy`) when it stopped. **None of those results were recorded; treat the DoD as not run.**

## What's left

1. Read the brief **on `dev`** (`docs/agents/briefs/M1-30-tree-wildcards.md`); its "Added scope" (K8, D-121) was added after this branch was cut and is **not started**.
2. Check the `deny.rs` diff stays within the brief's owned functions and line ranges (D-119), and that +753 lines isn't bloat.
3. Finish S5 and K8, writing code and tests only (D-123).
4. The single verification task runs the DoD; then a Sonnet review with the 100% gate (D-120).
