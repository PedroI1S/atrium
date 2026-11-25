# M1-39-rev4-p3-tools-rules: review round 4

Reviewer: Claude Sonnet 5.5, read-only (D-162). Reviewed: round 4 (`549d5056`, `ed59e44d`).

Orchestrator's verification (macOS, the worktree's own `target/`): 924/0/7, 371 vitest, clippy and fmt OK, protocol up to date.

## Verdict
**Request changes.** Review 3's findings (`tar -s -x`, `gawk -Wsource=`, `git diff --unified --ext-diff`) and round 2's `git -Csrc` are closed under D-156.
- No hidden trigger, code or pin in core. No word is consumed as an option value outside the frozen table.
- `-L`/`--line-range` in `tools/git.rs` is fail-closed.
- §8 charging is right, the frozen interface is kept (plus D-153's `RuleEvalContext.outcomes`), and nothing is wired into `SessionGate`.
- Core and app agree everywhere except one place.

## Findings
1. **Low: the app's `isSedScriptSafe` accepts a non-ASCII sed delimiter that core rejects.**
   - Code: `app/src/chat/rule-form.ts`, `isSedScriptSafe` (`s` ~354, `y` ~371, `skipAddr` `\` ~316) and `crates/atrium-core/src/policy/v4/tools/sed.rs` (`s` ~103, `y` ~134, `skip_address` ~297).
   - Why: core scans UTF-8 bytes, so a delimiter like `€` leaves stray continuation bytes and the scan ends `Uncertain`. The app scans UTF-16 code units, so it accepts the same script.
   - Failing case: Allow, command prefix `sed 's€a€b€'`, `sed -n 's€a€b€p'`, `y€ab€cd€`, `\€re€p`. The app's `commandRisk` returns `null`, the form submits, and the daemon's `create_rule` answers `Invalid`. `checkStoredRule` disagrees with the daemon the same way.
   - Fix: make the app treat a non-ASCII `s`/`y`/`\` delimiter as unsafe, or scan `TextEncoder` bytes exactly like `sed.rs`.

## Test gaps
- Finding 1 in `rule-form.test.ts` (`commandRisk` and `checkStoredRule`), `v4/rules/tests.rs` (`validate_rule_matcher`) and `crates/atrium-daemon/tests/create_rule.rs`'s rejected-prefix table.
- A non-ASCII byte inside a regex or replacement (`s/é/x/`) with an ASCII delimiter, pinned as unaffected.
- Optional: a shared core/app fixture table of `(argv, valid/invalid)` cases.
