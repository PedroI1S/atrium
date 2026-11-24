## Verdict

**Rework required.** Reviewed `git diff 4f18a59...HEAD` at `a57b54f`.

Round 6’s finding is closed by inspection. P0’s frozen interface and SessionGate remain unchanged. One new finding remains.

No files were modified or tests rerun. Verification shows fmt, clippy and workspace tests passing (`rc=0`); repeated core runs remain underway.

## Findings

1. **Major — Best-effort entry charging lets Exhausted override MAX_BYTES.**

   [best_effort.rs:101](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex/best_effort.rs:101) performs a fallible budget charge before checking the total input size.

   **Failing case, derived by inspection:**

   ```rust
   let over = "a".repeat(limits::MAX_BYTES + 1);
   lex_best_effort(
       &[&over],
       &mut ComputeBudget::new(0, usize::MAX),
   )
   ```

   This returns `Err(Exhausted(ComputeBudget))` immediately, bypassing the size check. The required result is `Err(TooLarge(Cap::Bytes))`: §8 preserves size-limit Deny, and §1 places TooLarge above Exhausted. The public `lex_best_effort` wrapper performs no earlier size validation.

   **Fix:** calculate and validate the total string lengths before the fallible entry charge. This uses length metadata without scanning input bytes. Preserve the saturating sum and apply the ordering through both best-effort entry points.

## Test gaps

Add combined MAX_BYTES-and-exhaustion regressions for `lex_best_effort` and `lex_best_effort_into`, covering one oversized string and multiple strings whose combined size exceeds the cap. Existing byte-cap tests use default budgets; round 8’s combined tests cover only MAX_UNITS.