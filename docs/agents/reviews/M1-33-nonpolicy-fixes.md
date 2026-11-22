# Review: M1-33-nonpolicy-fixes
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-21 · round 1

## Verdict

**rework**

The diff matches the brief’s scope. I found no dependency on policy changes left on the source branch, but six findings remain.

## Findings

1. **Major — `crates/atrium-agents/src/codex/adapter.rs:1440`, `crates/atrium-agents/src/claude/adapter.rs:1399`: cancelled checks accumulate detached tasks.**  
   The inner gate tasks survive cancellation and timeout without an admission bound. With a gate blocked before returning, repeatedly submitting and cancelling native requests removes the outer bookkeeping while retaining every inner task, payload and gate reference. The 500 ms shutdown wait leaves them running. **Fix:** bound outstanding inner calls, including cancelled calls still awaiting compensation; retain capacity until they settle and decline excess requests.

2. **Major — `app/src/chat/markdown.ts:616`: image brackets retain quadratic scanning.**  
   `"![".repeat(n) + "[x](https://e.co)".repeat(n)` scans all image openers for every successful link: image openers never become inactive and never trigger the early break. Read-only probes took approximately 0.47 seconds for 304 KB and 2.79 seconds for 608 KB. **Fix:** track active non-image openers separately so each is visited at most once.

3. **Major — `app/src/chat/markdown.ts:515`: distinct backtick lengths still rescan suffixes.**  
   Concatenating unmatched backtick runs of lengths 1 through 2,400, each separated by `x`, bypasses the failed-length cache because every length is unique. The 2.88 MB input took approximately three seconds to parse synchronously. **Fix:** index complete backtick runs by length in one pass, then look up matching closers without rescanning subsequent runs.

4. **Major — `app/src/chat/markdown.ts:390`: inline nesting can overflow the stack.**  
   `parseInline("*".repeat(12000) + "x" + "*".repeat(12000))` throws `RangeError: Maximum call stack size exceeded` in `mapText`. This also occurs on dev, but remains in the parser-hardening area covered by D-120. **Fix:** bound inline AST depth and flatten excess nesting, or make traversal and rendering safe for arbitrary depth.

5. **Minor — `app/src/chat/markdown.ts:292`: malformed text disables later valid links.**  
   For `"[x](<".repeat(1000) + ">X [ok](file:///tmp/a)"`, failed overlapping scans exhaust the shared budget. HEAD produces no link; dev correctly parses the final `[ok]` link. Every subsequent link-tail attempt returns immediately once the budget is exhausted. **Fix:** cache failed scan ranges or tokenize destinations so malformed prefixes cannot consume the parsing allowance for unrelated valid links.

6. **Minor — `crates/atrium-daemon/src/lib.rs:166`: login-shell timeout leaves descendants and reader threads running.**  
   A startup script executing `sleep 600; true` leaves the sleep process alive after `child.kill()` kills its shell. The descendant retains stdout, so the detached `read_to_end` thread also remains blocked. A shell that exits while a background child retains stdout has the same reader leak. **Fix:** supervise discovery in its own process group and make stdout collection cancellable, with cleanup on every return path.

## Test gaps

- Add regression coverage for findings 1–6, including repeated cancellation against a blocked gate and shell-descendant cleanup.
- The supplied verification log reports two workspace failures: `upgrade::tests::the_daemon_in_the_lock_file_gets_the_signal` and `round_5_blocker_1_searches_and_walkers_reaching_a_folder_linked_into_ssh`. Their paths are unchanged here; establish the dev baseline and obtain passing verification or a recorded driver exception.
- The supplied log contains no `pnpm -C app typecheck` or `pnpm -C app test` results.
- I ran read-only, in-memory parser probes; I did not run suites or modify files.