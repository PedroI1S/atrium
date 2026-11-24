## Verdict

**Rework required.** Reviewed `git diff 4f18a59...HEAD` at `16baae3`.

Round 5’s heredoc finding is closed; its bracket-class finding is closed by D-146. P0’s interface and SessionGate remain unchanged. One new finding remains.

No files were modified or tests rerun. Verification reports fmt, clippy and workspace tests passing; repeated core runs are still underway.

## Findings

1. **Major — Exhaustion discards an already-discovered unit beyond MAX_UNITS instead of returning TooLarge.**

   [lex.rs:2245](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex.rs:2245) drops the pending unit when 512 units are already stored. The best-effort recovery has the same guard at [best_effort.rs:1214](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex/best_effort.rs:1214).

   **Failing case, derived by inspection:** call `lex_exact` on `"x;".repeat(512) + "sudo"` with `ComputeBudget::new(31_347, 128 * 1024 * 1024)`. The lexer stores all 512 preceding units and completely reads and stores `sudo` in the pending unit. Its subsequent reserved-word lookup exhausts the budget. Recovery discards `sudo` and returns 512 units plus Exhausted.

   The 513th unit is already established. Returning Exhausted instead of `TooLarge(Cap::Units)` violates §8’s structural-cap requirement and §1’s precedence, changing the prescribed Deny into Ask.

   **Fix:** when exhaustion recovery encounters a nonempty pending unit beyond the unit cap, return TooLarge. Apply the same rule to best-effort recovery.

## Test gaps

Add combined cap-and-exhaustion regressions for both lexers: 512 stored units plus a completed pending word must produce TooLarge when a subsequent charge fails. Existing cap tests use generous budgets; exhaustion sweeps stay below the cap.

## Notes for the driver

- **Slash members:** excluding unquoted `/` narrows D-146’s literal wording, because `/` is an allowed unquoted character. The exclusion has a sound intersection rationale: slash has special pathname semantics, and dev explicitly handles slash-containing classes through widening. Record the exclusion explicitly; its practical effect is additional Unread/Ask results.
- **Quoted `!` and `^`:** excluding these is consistent with D-146’s unqualified “no `!` or `^`.” They are literal bytes rather than shell negation, so `NegatedGlobClass` is an imprecise explanation. If “quoted or escaped byte” was intended to override that prohibition, the driver should clarify it explicitly. Neither choice is counted as a finding, as requested.