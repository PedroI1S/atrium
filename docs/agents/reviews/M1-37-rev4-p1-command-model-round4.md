## Verdict

**Rework required.** Reviewed `git diff 4f18a59...HEAD` at `8b5a298`.

Round 3’s three reproductions are fixed. P0’s frozen files and SessionGate remain unchanged. No files were modified or tests rerun. The supplied verification log shows fmt, clippy and workspace tests passing (`rc=0`).

The `unwrap_unit` change request is **not a concrete interface finding**: `build_units` unwraps in place and preserves its units. The by-value signature alone does not demonstrate lost information in a contract-defined caller.

## Findings

1. **Major — Best-effort unwrapping drops completed alternative readings on Exhausted.**  
   [best_effort.rs:2105](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex/best_effort.rs:2105) returns only `cur` on failure, dropping `alts`. Its caller then records an empty alternative list at [line 1249](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex/best_effort.rs:1249).

   **Failing case, derived by inspection on this 64-bit target:** call `lex_best_effort_into` on `"command -Z arg sudo id"` with `ComputeBudget::new(402, 100_000)`. Unwrapping completes and stores the alternative `["sudo", "id"]`, then exhausts on the final five-unit argv-shift charge. Recovery retains the raw argv but loses that completed alternative. This violates §8’s preservation requirement.

   **Fix:** return the partial alternative readings alongside the argv on exhaustion, and retain them in `push_converted`. Apply the same preservation rule to the helper’s other callers.

2. **Minor — Crontab line scanning happens before its budget charge.**  
   [best_effort.rs:2629](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex/best_effort.rs:2629) obtains each line through `text.lines()` before charging its length. Obtaining the line already scans for its terminator.

   **Failing case:** `crontab_commands_budgeted(&"x".repeat(60_000), &mut ComputeBudget::new(0, 0))` scans the entire 60,000-byte input before its first charge returns Exhausted. The result reports exhaustion, but the work has already happened, contrary to §8.

   **Fix:** charge the delimiter-search pass before constructing/advancing the line iterator, or use an incrementally charged scanner. Keep the subsequent parsing and copying charges separate.

## Test gaps

- The new best-effort sweeps check raw argv and redirects, but do not verify preservation of already-produced alternative readings.
- Add a charge-before-scan regression for crontab input; charge-growth upper bounds cannot detect work performed before charging.