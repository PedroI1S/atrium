## Verdict

**Rework required.** Reviewed `git diff a1f0174...HEAD` at `b44f08e`.

The nine original reproductions are addressed by inspection. New cases remain below, including two incorrect test expectations. P0’s frozen interface is unchanged, and nothing is wired into SessionGate.

No files were modified or test suites rerun; test-failure evidence comes from the supplied verification log.

## Findings

1. **Major — Best-effort assignment detection performs uncharged quadratic work.**  
   [best_effort.rs:1189](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex/best_effort.rs:1189) calls `is_posix_name` on the accumulated word at every `=` until one qualifies. An invalid prefix leaves `assign_eq` unset.

   **Failing case:** lex `"echo " + "a".repeat(60_000) + "-" + "=".repeat(60_000)`. Each of 60,000 equals signs scans the same 60,000-character prefix. That is approximately 3.6 billion uncharged byte inspections for a 120,006-byte input. The recorded charges remain linear, defeating both §8’s complexity requirement and compute limit.

   **Fix:** track assignment-name validity incrementally and consider the first unquoted equals sign once. Charge every scan before performing it.

2. **Major — Exact-word buffer allocations still bypass size accounting.**  
   [lex.rs:273](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex.rs:273) charges two allocation bytes per emitted byte, then calls `Word::push`. That method grows two vectors geometrically; their allocated capacities exceed their lengths. Neither capacity growth nor the associated copying is charged.

   **Failing case:** on this 64-bit target, lex `"x"` with an allocation budget of  
   `2 + 4 * size_of::<Word>() + 4 * size_of::<RawCommandUnit>()`.  
   Lexing completes without Exhausted, but the word’s two one-byte vectors allocate eight bytes each, exceeding the word’s two-byte allowance by fourteen bytes.

   **Fix:** build the byte and provenance buffers through budgeted capacity growth, charging copying before reallocation, then construct `Word` using P0’s existing `from_parts` interface. This is a separate remaining allocation path beyond round 1’s repaired auxiliary buffers (§8).

3. **Major — The carried `find -exec` scan omits nested `-c` scripts.**  
   [best_effort.rs:1615](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex/best_effort.rs:1615) unwraps extracted commands and appends their wrapper payloads, but never applies ordinary code-payload extraction to their effective or alternative readings.

   **Failing case:** `find . -exec sh -c 'sudo id' \;` produces a derived `["sh", "-c", "sudo id"]` unit but no derived `["sudo", "id"]` unit. The carried dev implementation calls `code_texts_bounded` for this extracted command.

   **Fix:** process extracted readings through the same bounded payload extraction and recursion used for ordinary units, respecting depth and unit caps. This concerns the explicitly retained scan and required `-c` units (§2, brief, §12), rather than a general guarantee against misses on Unread code.

4. **Minor — Empty quotes still disappear during HOME assignment detection.**  
   [units.rs:923](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/units.rs:923) uses `leading_assignment_name`, which reconstructs assignment syntax solely from emitted-byte provenance.

   **Failing case:** `build_units("''HOME=/tmp; cat ~/x", …)` emits `HomeSetEarlier` for the second unit. The first word is a command name containing empty quoting, not an assignment; the repaired exact lexer correctly emits no Assignment reason.

   **Fix:** carry the lexer’s token-level assignment classification into HOME tracking through private metadata, without changing P0’s interface. Do not reconstruct assignment syntax from decoded bytes (§3).

5. **Minor — The reported glob-class failure is a test defect.**  
   [lex/tests.rs:722](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex/tests.rs:722) incorrectly calls `cat /usr/bi[n\]]` unclosed.

   **Failing case:** `\]` supplies a literal class member; the following unquoted `]` closes the class. Returning no UnreadReason is correct under §3 and preserves escape provenance.

   **Fix:** expect acceptance and verify the escaped byte’s provenance. Use `r"cat /usr/bi[n\]"` as the genuinely unclosed case.

6. **Minor — The reported allocation-linearity failure is a test defect.**  
   [lex/tests.rs:1473](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex/tests.rs:1473) requires allocation charges to grow by at least 75% of the input-length ratio. §8 requires a linear upper bound, not proportional growth with negligible fixed overhead.

   **Failing case:** the reported charges, 38,016 and 279,680, fit exactly `A(n) = 2n + 17,536` for these fixtures. The fixed 64-unit metadata overhead explains the 7.36 ratio despite a 12.80 length ratio.

   **Fix:** test an affine bound or remove the proportional lower bound. Keep actual allocation-accounting checks separate. Inputs below MAX_BYTES may legitimately exhaust the allocation budget; MAX_BYTES does not guarantee successful analysis.

## Test gaps

Add regressions for repeated equals signs after long invalid assignment prefixes, actual word-buffer capacity accounting, nested `find -exec sh -c` payloads, and empty-quoted HOME names through `build_units`.

The sed stdin-script limitation now has explicit coverage and remains outside §2’s promise, consistent with the round-1 disposition.