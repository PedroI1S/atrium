## Verdict

**Rework required.** Reviewed `git diff a1f0174...HEAD` at `0d41b7b`. P0’s interface is unchanged, and nothing is wired into SessionGate.

Both reported test failures have incorrect expectations. Additional implementation defects remain, so this does not meet D-120’s approval gate. No files were modified.

## Findings

1. **Major — Exhaustion drops a fully scanned unit.**  
   [lex.rs:1103](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex.rs:1103) moves `cur` into `finished` before charging the unit allocation/work. If that charge fails, `finished` is dropped and the exhaustion handler sees an empty `cur`.  
   **Failing case:** `lex_exact("sudo id", 0, None, &mut ComputeBudget::new(16, 10000))` scans both words, then returns Exhausted without retaining the unit. The best-effort flush has the same move-before-charge defect at [best_effort.rs:522](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex/best_effort.rs:522).  
   **Fix:** charge before removing the pending builder, or restore/preserve it on failure. Preserve its reasons as well as its words (§8).

2. **Major — Significant allocations bypass the shared budget.**  
   [best_effort.rs:2462](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex/best_effort.rs:2462) grows brace stacks without allocation charges; the lower-bound pass also grows its stack and pair vector uncharged.  
   **Failing case:** `"{" × 10000 + "x" + "}" × 10000`, with `ComputeBudget::new(1_000_000, 200_100)`, fits the recorded allocation charges and succeeds, although these auxiliary vectors alone allocate substantially more than that budget. Cold memo insertion also clones the complete result and input without charging at [best_effort.rs:130](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex/best_effort.rs:130).  
   **Fix:** charge auxiliary buffers, capacity growth, cache copies and their scanning/copying work before performing them. Skip optional cache insertion when unaffordable. Audit the other uncharged vector allocations, including the unit-index mapping (§8).

3. **Major — MAX_WRAP is enforced one wrapper too late.**  
   [units.rs:459](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/units.rs:459) checks the limit only after recognizing another wrapper.  
   **Failing case:** `"command " × 8 + "cat a"` removes eight wrappers and returns no Unread reason. §4 and P0’s `MaxWrapReached` definition say **reaching** eight makes the unit Unread. The test at [units/tests.rs:287](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/units/tests.rs:287) explicitly asserts the incorrect boundary.  
   **Fix:** emit `MaxWrapReached` upon reaching eight removals; change the boundary tests to seven versus eight and correct the handoff.

4. **Major — Cached units reuse pipeline IDs from separate pipelines.**  
   [best_effort.rs:116](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex/best_effort.rs:116) refreshes unit indices but neither remaps pipeline IDs nor advances `state.next_pipe_id`. The recursion-cache hit has the same problem at line 497.  
   **Failing case:** warm the memo with `curl x | cat`, then lex `["curl x | cat", "echo ok | sh"]`. The cached first pipeline and newly parsed second pipeline both receive `PipelineId(1)`. Repeating a cached nested script containing a pipeline also aliases separate executions.  
   **Fix:** assign fresh pipeline IDs consistently on both cache-hit paths and advance the shared counter. Otherwise the record incorrectly associates unrelated stages for later best-effort matching (§2; P0’s pipeline model).

5. **Minor — Command folding differs from P0’s Unicode lowercase rule.**  
   [lex.rs:1545](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex.rs:1545) lowercases individual characters. P0 specifies `str::to_lowercase()`, matching the carried CR-19 implementation.  
   **Failing case:** folding the quoted command word `'ΟΣ'` produces `οσ`; string lowercase produces `ος` through contextual final-sigma handling.  
   **Fix:** preserve string-level lowercase semantics, with budgeted allocation and the listed FoldRules, without adding normalization (§4).

6. **Minor — Valid quoting inside a glob class is rejected.**  
   [lex.rs:537](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex.rs:537) unconditionally reports `UnsupportedQuoteOrEscape` upon encountering quotes or escapes inside a class.  
   **Failing case:** `cat /usr/bi[n'n']` uses an allowed non-negated class and valid single quoting. It is accepted consistently by sh, bash and zsh, but P1 marks it Unread. §3 does not exclude quotes inside classes.  
   **Fix:** process permitted quotes and escapes while retaining class state and byte provenance; do not introduce additional exclusions beyond §3.

7. **Minor — Empty quotes lose their effect on keyword recognition.**  
   [lex.rs:1401](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex.rs:1401) infers lexical quoting solely from emitted-byte provenance. Empty quotes emit no bytes.  
   **Failing case:** `''if foo` receives `ReservedWordOrConstruct("if")`, although all three shells parse `if` here as a command word, not a keyword. All three pass the corresponding syntax check.  
   **Fix:** retain token-level “contained quoting” information during scanning and use it for keyword and assignment recognition (§3).

8. **Minor — The reported brace failure is a test defect.**  
   [lex/tests.rs:798](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex/tests.rs:798) incorrectly expects `None` for `"{,$}".repeat(9)`.  
   **Failing case:** the implementation returns `Some`, correctly preserving M1-31 review-1 finding 10: an expansion can introduce `$` immediately before a subsequent brace, suppressing its expansion. The groups therefore are not independent, and the naïve 512-result bound is invalid. The original carried test expects FullAccess Allow.  
   **Fix:** expect successful bounded expansion; retain a separate genuinely overflowing case. Correct the opposite assertion in [handoff:125](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/docs/agents/handoffs/M1-37-rev4-p1-command-model.md:125) and cite the applicable §10/§11 disposition.

9. **Minor — The reported linearity failure is a test defect.**  
   [lex/tests.rs:1000](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex/tests.rs:1000) assumes every below-MAX_BYTES input succeeds with the default allocation budget. §8 makes no such promise.  
   **Failing case:** `echo ` followed by the test’s large repeated `$(` suffix correctly returns `Exhausted(AllocationBudget)`. Its allocation growth is linear: nesting is bounded by MAX_DEPTH, with a fixed number of copies per level. Its constant factor exceeds the default allocation allowance.  
   **Fix:** measure compute **and allocation** scaling with sufficiently generous explicit budgets, then separately assert Exhausted under default limits and preservation of earlier units. Do not enlarge P0’s frozen limits merely to satisfy this test.

## Test gaps

The requested §10 rows and broad boundary tests are present, but they miss the failing cases above. Add precise exhaustion-boundary tests, auxiliary-allocation accounting tests, cold/warm cache equivalence across multiple texts and sibling scripts, contextual Unicode folding, mixed quoted classes and empty-quote tokens.

For the round-2 sed decision, `sed -f -` and `sed --file=-` can execute an `e` command supplied by a heredoc; excluding all ConditionalRunner rows omits that body. However, dev’s stdin-script checks also omit sed, and §2 explicitly says misses on Unread parts are not findings. I therefore do **not** count this omission as a contract defect. A regression documenting this case would make the decision’s limits explicit.

The M1-27/M1-31 disposition lists and the list of rewritten scans are present. Correct the N11 disposition above and supply the required §10/§11 references for retained entries currently lacking them.

The two deterministic failures are evidenced by the supplied orchestrator verification log; this review did not build or rerun the Rust suite.