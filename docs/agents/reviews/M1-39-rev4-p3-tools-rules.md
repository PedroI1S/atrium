## Verdict

**Request changes.** No files were modified.

CR-P3-1, CR-P3-2 and CR-P3-3 leave concrete defects, detailed below. **CR-P3-4 does not:** its augmentation covers the five CR-17 entries, including their relevant ancestors.

## Findings

1. **High — Rule coverage cannot account for P1/P2 Unread outcomes (CR-P3-3).**  
   [v4/rules.rs:517](/home/rodrigo/atrium-wt/M1-39-rev4-p3-tools-rules/crates/atrium-core/src/policy/v4/rules.rs:517) checks only `ToolEffects::unread`. For `cat a` with an Unread canonical target, a Thread Allow prefix `["cat"]` acquires coverage and suppresses a Global Deny `["cat"]`. The same gap affects unit-level P1/P2 reasons and Exhausted outcomes. **Fix:** extend `RuleEvalContext` with prior `PartResult`s or equivalent per-unit outcome information, and use it before granting Allow coverage or suppressing Deny.

2. **High — Partial coverage incorrectly suppresses lower-ranked Deny rules.**  
   [v4/rules.rs:917](/home/rodrigo/atrium-wt/M1-39-rev4-p3-tools-rules/crates/atrium-core/src/policy/v4/rules.rs:917) skips covered units when matching Deny. With Thread Allow `["cat"]`, Global Deny `["cat"]`, and `cat a && ls b`, P3 returns no Deny. `dev` denies because the higher rank has not covered the whole call. The new test at `rules/tests.rs:480` asserts the incorrect result. **Fix:** retain call-level suppression; while the call remains uncovered, match lower-ranked Deny rules against every exact and best-effort unit.

3. **High — A matched Deny can be lost to subsequent allocation exhaustion.**  
   [v4/rules.rs:577](/home/rodrigo/atrium-wt/M1-39-rev4-p3-tools-rules/crates/atrium-core/src/policy/v4/rules.rs:577) performs fallible reason and ID construction after finding the winning Deny. For a non-command Read call, one Global Deny `Any`, no units, and `ComputeBudget::new(12, 1_048_576)`, matching succeeds but reason construction returns Exhausted. The Deny disappears, violating §1 and §8. **Fix:** reserve and charge the result’s storage before matching; committing an established Deny must not fail.

4. **Medium — Git selection discards previously found results on exhaustion.**  
   [v4/tools/git.rs:81](/home/rodrigo/atrium-wt/M1-39-rev4-p3-tools-rules/crates/atrium-core/src/policy/v4/tools/git.rs:81) propagates exhaustion while its results remain in a local vector. For `git show -- ':!x' y`, a compute budget of eight units permits the first StoreHit, then fails on `y`; the caller receives only `Err(Exhausted)`. **Fix:** amend `check_git_selection` to write through a reserved result sink, or return partial results together with exhaustion. §8 requires retaining the StoreHit.

5. **High — BSD/macOS `sed -i` can hide a listed trigger.**  
   [v4/tools.rs:1880](/home/rodrigo/atrium-wt/M1-39-rev4-p3-tools-rules/crates/atrium-core/src/policy/v4/tools.rs:1880) selects the first positional word unless it is empty. In `sed -i p 'w out' input`, BSD/macOS consumes `p` as the backup suffix and executes `w out`; P3 inspects `p` and emits no Unread reason. An exact Allow can consequently cover the unit. **Fix:** inspect every applicable script interpretation, or classify ambiguous bare `-i` forms as Unread.

6. **Medium — Deny path scopes use “any target” instead of the existing “every target” condition.**  
   [v4/rules.rs:856](/home/rodrigo/atrium-wt/M1-39-rev4-p3-tools-rules/crates/atrium-core/src/policy/v4/rules.rs:856) reuses the existential PathGlob matcher for `path_scope`. A Deny `Any` scoped to `/proj/**` matches a call touching both `/proj/a` and `/elsewhere/b`; it also ignores an additional Unread target. That changes `dev`’s scope semantics and contradicts §7’s Unread-target condition. **Fix:** require all canonical targets to satisfy the scope and reject unresolved targets. Keep existential matching only for the Deny `PathGlob` matcher itself.

7. **Medium — The daemon’s validator mirror already differs from core (CR-P3-1).**  
   [commands.rs:673](/home/rodrigo/atrium-wt/M1-39-rev4-p3-tools-rules/crates/atrium-daemon/src/commands.rs:673) does not call the required core validator. For an Allow prefix `["sed","z"]`, core accepts the safe script, while the daemon’s separate parser rejects it; listing a stored copy likewise marks it invalid although core accepts it. **Fix:** expose a public core validation entry point through `policy/mod.rs`, including a displayable error, and remove the daemon mirror.

8. **Medium — The app retains per-tool guards removed by D-141.**  
   [rule-form.ts:543](/home/rodrigo/atrium-wt/M1-39-rev4-p3-tools-rules/app/src/chat/rule-form.ts:543) still models `deno`, `bun`, PHP, Lua, PowerShell and other tools outside the frozen table. For example, the app rejects—and `checkStoredRule` marks invalid—Allow prefix `["deno"]`; core and daemon accept it as an unknown tool. Carried tests at `rule-form.test.ts:345` preserve that old expectation. **Fix:** align validation with the six row kinds and rewrite or drop every conflicting carried test, recording its disposition under §12.

9. **Medium — Stored invalid rules are never marked in the rendered rules list (CR-P3-2).**  
   [rule-form.ts:659](/home/rodrigo/atrium-wt/M1-39-rev4-p3-tools-rules/app/src/chat/rule-form.ts:659) adds annotation helpers with no production consumer. Seed Allow prefix `["awk"]`: the daemon returns `invalid_reason`, but Settings still loads `Rule[]` and renders the ordinary Allow label and ready indicator. **Fix:** make validity part of the listing interface consumed by Settings—using `ListedRule[]` or a defaulted protocol field with its TypeScript mirror—and render the invalid state and reason. The flattened wire workaround alone does not fulfill §11 item 5.

10. **High — Root pathspec selections escape CR-12 checks.**  
    [v4/tools/git.rs:425](/home/rodrigo/atrium-wt/M1-39-rev4-p3-tools-rules/crates/atrium-core/src/policy/v4/tools/git.rs:425) normalizes `.` to no components and returns false. Thus `git show HEAD -- .` produces no StoreHit although that explicit pathspec is an ancestor of credential-shaped names. The special `:` pathspec also falls through without a hit. **Fix:** recognize root ancestor selections and the lone-colon pathspec in pathspec context, while keeping revision-object parsing separate.

11. **Medium — `--noglob-pathspecs` incorrectly leaves wildcards active.**  
    [v4/tools/git.rs:65](/home/rodrigo/atrium-wt/M1-39-rev4-p3-tools-rules/crates/atrium-core/src/policy/v4/tools/git.rs:65) sets the wildcard-disabling state to false for this option. `git --noglob-pathspecs status -- '*.rs'` consequently gets `GitWildcardPathspec`, preventing an otherwise covering exact rule. Those wildcard characters are literal unless explicit glob magic enables them. **Fix:** track default wildcard interpretation separately from whether pathspec magic is enabled.

12. **Medium — Program pinning rescans words before charging.**  
    [v4/tools.rs:1168](/home/rodrigo/atrium-wt/M1-39-rev4-p3-tools-rules/crates/atrium-core/src/policy/v4/tools.rs:1168) calls `Word::as_str()`, which validates UTF-8, before the subsequent clone charge. With `python` and a long named script, a budget exhausted immediately before this iteration still permits scanning the entire script word. The earlier charged validation already consumed its own scan allowance. **Fix:** retain the validated string views or call `word_as_str_budgeted` before each additional scan.

13. **Medium — The search test invents a supported flag.**  
    [v4/tools/tests.rs:438](/home/rodrigo/atrium-wt/M1-39-rev4-p3-tools-rules/crates/atrium-core/src/policy/v4/tools/tests.rs:438) expects `rg -L --no-follow needle src` to disable following and remain Home-eligible. `--no-follow` is absent from the frozen table, so §6 requires UnknownFlag/Unread. This test fails in verification. **Fix:** assert the table’s Unread result and remove the unsupported toggle expectation.

14. **Medium — The stored-invalid-rule fixture fails before exercising listing.**  
    [create_rule.rs:263](/home/rodrigo/atrium-wt/M1-39-rev4-p3-tools-rules/crates/atrium-daemon/tests/create_rule.rs:263) creates the data directory with default permissions. Under umask 022 it is 0755, and daemon startup rejects it because `secure_dir` requires 0700. Verification reports this startup failure. **Fix:** create the fixture directory with `lifecycle::secure_dir` before opening the store.

## Test gaps

- Add P1/P2 outcome-aware coverage tests, including competing higher-ranked Allow and lower-ranked Deny rules.
- Add exhaustion tests **after** a Deny or Git StoreHit has been established, plus charge-before-rescan boundaries.
- Add daemon/core/app validation parity cases and a rendered invalid-rule-list test.
- Add the BSD `sed -i`, root pathspec, `--noglob-pathspecs`, and mixed/Unread path-scope cases above.

The supplied verification output reports fmt and clippy passing, but **workspace tests return 101**, with the search expectation and daemon fixture failures above. No tests were run during this read-only review.