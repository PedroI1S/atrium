## Verdict

**Rework required.** Reviewed `git diff b50f726...HEAD` at `22ff130`.

The six round-2 reproductions are fixed, including both incorrect test expectations. P0’s frozen interface remains unchanged, and nothing is wired into SessionGate.

Three new §8 defects remain. No files were modified or tests rerun; the supplied verification log is clean.

## Findings

1. **Major — Allocation exhaustion drops a fully scanned command word.**  
   [lex.rs:1743](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex.rs:1743) passes the completed word by value into `vec_push_budgeted`. If capacity charging fails, that helper drops the word before the exhaustion handler can preserve it.

   **Failing case:** `lex_exact("sudo", 0, None, &mut ComputeBudget::new(1000, 16))`. The byte and provenance buffers consume 16 allocation bytes, and the entire word is scanned. Growing `cur.argv` then fails. The result contains Exhausted and **zero units**, losing the discovered command.

   **Fix:** reserve argv capacity before scanning the word, or retain/return the completed word when insertion fails so recovery preserves it. Audit equivalent insertions of completed words and redirects.

2. **Major — Exact wrapper validation performs uncharged operand scans.**  
   [units.rs:753](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/units.rs:753) scans the operand through `has_unquoted_glob` and digit validation without accepting or charging a budget. Signal and duration validation have the same problem.

   **Failing case:** first lex `"nice -n " + "0".repeat(60_000) + " cat"` with a generous budget, then pass its raw unit to `unwrap_unit` with `ComputeBudget::new(100, 10_000)`. Unwrapping succeeds after charging only **19 compute units**, despite scanning the 60,000-byte operand multiple times.

   **Fix:** pass the budget through validation helpers and charge each scan before performing it. Include the other variable-length scans in exact unwrapping and nested-shell classification.

3. **Major — Best-effort payload deduplication performs uncharged quadratic work.**  
   [best_effort.rs:1768](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex/best_effort.rs:1768) searches every previously collected string using `payloads.contains(&s)`. Neither comparison steps nor compared bytes are charged.

   **Failing case:** construct this 130,817-byte line:

   ```rust
   format!("sh {}", (0..255)
       .map(|i| format!("<(echo {}{i:04})", "a".repeat(500)))
       .collect::<Vec<_>>().join(" "))
   ```

   Collection performs **32,385 string comparisons**, each against a long common prefix. In a focused call to `collect_shell_proc_sub_payloads`, a 200,000-unit budget succeeds: cloning and vector growth charge about 130,000 units, while the comparisons inspect over 16 million common-prefix bytes uncharged. Increasing the number of payloads makes this work quadratic.

   **Fix:** use budgeted deduplication with linear processing, or explicitly bound and charge comparisons before performing them. Audit the other payload `.contains` calls.

## Test gaps

Add regressions for completed-word insertion failure, wrapper operand scans under tight budgets, and many distinct payloads sharing long prefixes. The existing charge-growth tests cannot detect work that never increments the charge count.