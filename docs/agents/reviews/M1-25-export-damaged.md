# Review: M1-25-export-damaged (e69c44ff, bce51ac2; CR-16)

Reviewer: Claude Sonnet 5.5, read-only (cross-provider). Author: Gemini 4 Hydrogen `high`. Saved by the orchestrator from the reviewer's report. The orchestrator applied finding 2 in `0e57b6aa` and checked that 2 daemon tests fail on dev's code and pass with the fix.

## Verdict: **approve** (no blockers or majors)

1. **Privacy: clean.** A message that fails verification leaves the open list at its `MessageCompleted` (`export.rs`, `Fold::push`), so `close()` never writes it. Its text and `Inline` body are dropped inside `verify_message_reason`, which returns only a static reason. The placeholder carries head fields, `message_id`, `kind`, the event range and that reason. `message_id` (provider-controlled) goes through the redactor; the test uses `HOME/CANARY`. Tool output is keyed by call id, so nothing tied to a damaged message is pulled in. The "still open at session end" path covers messages that never completed, which aren't damaged under CR-16.
2. **Correctness.** Only verification failures count (delta count, chars, sha256, an inline body after deltas). I/O and store errors still fail the export, and the partial file is removed. The count saturates; 5 damaged messages give `damaged: 5` in both formats. Order is kept; healthy JSONL lines still deserialize as `StoredEvent`.
3. **Display** keeps §6.1's hard error (`reconstruct_message` → `Integrity`).
4. **API:** `{"bytes","damaged"}` is additive; `export_thread` keeps its signature; bindings regenerated; contract wording short and accurate.
5. **Tests** run and pass, and assert the right things.

## Findings
1. **Minor:** no test combines a damaged message with another still open at the end. The code is correct; nothing pins it. Add one case.
2. **Minor:** the damaged JSONL line is a new flat shape, not a `StoredEvent`. State it in §13. *Applied by the orchestrator, `0e57b6aa`.*
3. **Minor:** the daemon test covers only one damaged message; acceptable given the core tests.
4. **Nit:** `write_export`'s `integrity =>` arm is now unreachable in practice; keep it for exhaustiveness.

## What the reviewer ran
`cargo test -p atrium-core --test history` (27 passed); `cargo test -p atrium-daemon --test sessions` (23 passed); clippy on both crates clean; `git status` clean.
