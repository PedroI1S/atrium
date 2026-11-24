## Verdict

**Rework required.** Reviewed `git diff 4f18a59...HEAD` at `1a1db5b`.

Round 4’s two findings are closed by inspection. P0’s frozen interface and SessionGate remain unchanged. Two new defects remain. No files were modified or Rust tests rerun.

## Findings

1. **Major — Empty heredoc delimiters expose stdin data as exact commands.**  
   [lex.rs:1485](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex.rs:1485) queues a heredoc only when its decoded delimiter is nonempty.

   **Failing case:** `lex_exact("cat <<''\nsudo id\n\n", …)` produces an exact `["sudo", "id"]` unit, although that line is heredoc data. The best-effort lexer correctly retains only the `cat` unit here. The invented exact unit supplies P5 with a false privilege-escalation candidate, violating §1’s precise-hit requirement and §3’s treatment of heredocs.

   **Fix:** distinguish a missing delimiter token from a valid empty quoted delimiter. Queue and skip bodies for `<<''`, `<<""` and their `<<-` forms.

2. **Minor — Valid shared bracket expressions are rejected.**  
   [lex.rs:1249](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex.rs:1249) always treats the first `]` as closing the class; [line 1297](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex.rs:1297) rejects an internal `[`.

   **Failing cases:** `cat []A]GENTS.md`, `cat [A[]GENTS.md` and `cat [[:upper:]]GENTS.md` receive Unread reasons. All three patterns match `AGENTS.md` under the available sh, bash and zsh. They contain valid non-negated bracket expressions permitted by §3; the contract does not exclude these forms.

   **Fix:** recognize an initial literal `]`, literal `[` members and shared POSIX named classes, preserving provenance and budget charging. Correct the test at [lex/tests.rs:192](/home/rodrigo/atrium-wt/M1-37-rev4-p1-command-model/crates/atrium-core/src/policy/v4/lex/tests.rs:192), which currently requires `[[:alpha:]]` to be rejected.

## Test gaps

Add regressions asserting that empty-delimiter heredoc bodies produce no exact command units, while commands after the terminator remain visible. Add positive coverage for the three bracket forms above.

The verification file still identifies `8b5a298`; its passing results do not verify round 6.