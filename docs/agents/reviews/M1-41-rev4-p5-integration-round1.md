# M1-41-rev4-p5-integration (P5): review round 1

Reviewer: Claude Sonnet 5.5, read-only, a Claude Code subagent on Pedro's plan (D-162, D-170). Branch `agent/neon/M1-41-rev4-p5-integration` at `ba778075` (base `5c4d154f`). Round 1 ran in three parts because of network and quota interruptions.

Orchestrator verification (macOS, `dev` 274ca5c0 merged with the branch):
- fmt OK; protocol OK; typecheck OK; vitest 373/373; app build OK;
- **clippy: 11 errors;**
- **workspace: 1038 passed, 52 failed, 1 ignored;**
- **suite timings slower than `dev`**: `adversarial_regression` 2.64 s → 11.30 s, `adversarial_fuzz` 15.63 s → 28.27 s, `adversarial_gate` 1.05 s → 2.74 s, `adversarial_m1_18` 2.95 s → 5.72 s, `policy` 4.89 s → 6.59 s;
- the M1 demo was not run, because the daemon's fake-approval tests fail.

The full failure list is in the worktree's `.orchestration/review-input/M1-41-verify-r1.md`. The orchestrator logged the slow timings as an extra finding (O1).

## Verdict

**Request changes.** Verification is red: 52 tests fail and clippy has 11 errors. By the reviewer's reading, the failures do not all share one cause:
- product defects: the daemon approval path, the best-effort pass, and one cwd rule in the exact model (findings 1 to 4, 8 and 9);
- migrated tests whose expectations are stale or uncited (findings 5 to 7);
- clippy and handoff accuracy (findings 10 to 12).

The evaluator's core looks sound:
- `decide` follows §1's six steps in order and §2's table per setting;
- the structural caps come before budgets (D-150);
- `build_protected_set` has its own `FsCaps` and `ComputeBudget`;
- the resolver-backed entry points are used (D-147);
- `RuleEvalContext::outcomes` is passed;
- `unread` is set and clears `remember_matcher`;
- `redeem` re-evaluates through rev 4;
- all 26 §10 rows plus the Unknown subject go through `SessionGate`, and none fails.

The reviewer ran nothing. Causes marked "likely" come from reading the code.

## Findings

1. **High: plain writes become Unread in the daemon (7 tests: `requests_fake.rs` ×6, `sessions.rs::always_creates_a_rule_with_the_approval`).**
   - Case: `echo synthetic` and `touch notes.txt` in Home. Expected: a rememberable Ask with `unread = None`. Actual: `rememberable = false`.
   - Likely cause:
     - the daemon's protected paths include `current_exe()` (`atrium-daemon/src/lib.rs:699`, under test `target/debug/deps/<bin>`);
     - `build_protected_set` step 5 `folded_spellings(.., write_only = true, ..)` lists every ancestor (`sensitive.rs:2687`);
     - a huge ancestor hits the 4,096-entry cap;
     - that gives `Listing::Failed`, a write-only `GapScope::AnyTarget` gap (`sensitive.rs:2714`), and every non-Read target becomes Unread.
   - This is the open-ended gap D-145 item 4 forbids, and it can affect real users.
   - Fix:
     - a listing cap on an ancestor must not give `AnyTarget`. Scope it to `Through(root)`, or fold ancestors by `stat`ing candidate names;
     - consider a separate cap for ancestor-fold discovery.
   - Test: protected paths under a directory with more than 4,096 entries; `touch x` in Home must be a rememberable Ask with `unread = None`.
2. **High: the best-effort pass lost `dev`'s payload decoding (about 17 tests).**
   - §2's footnote, §10 row 4 and §12 dropped only K7, K9, `$'…'` and the globbed `cd`. These tests weren't migrated, so `dev`'s HardDeny stands.
   - Missing:
     - echo and printf escape decoding, and `yes` (`dev`'s `unescape_one`, `unescape_to`, `format_printf` and `echo_printf_texts` were deleted, not ported; `lex/best_effort.rs:2838` only joins raw words);
     - process substitution, heredoc and stdin-reader sinks (shell, `make -f -`, `at`, `batch`, `chroot`, `script`, `source /dev/stdin`);
     - compound sinks (`if`, `while`, `until`, `for`, `case`, functions, `coproc`, `{ }` in a pipe);
     - substituted command words and wrappers (`s$(echo h) -c`, `env $(..)`, `nohup`, `time`, `nice -n 5`, `$(x)/bin/$Y`);
     - `script -t`/`-t0`/`--timing=` option forms;
     - `env -ZSfoo bash <(..)`;
     - `cd .ssh; ls | xargs cat`.
   - Tests: `finding_5_*`, `finding_7`, `finding_8`, `finding_9`, `finding_14`, `held_echo_and_printf_into_a_shell`, `leftover_command_substitution_in_text_bodies…`, `leftover_wrappers_shells_and_stdin_readers`, `review_round4_table`, `review_round5_table`, `round_4_same_shell_texts…`, `round_4_blocker_5…`, `round_5_blocker_3…`, `round_5_blocker_7…`, `round_6_review_inputs`, `round_6_item_4…`, `round_7_item_5…`, `round_7_item_6…`, `round_8_blocker_4…`, `k1_…`, `k4_find_exec_payloads`, `fix2_k1…`, `sensitive_inputs_are_hard_denied…`.
   - Fix: port the missing readings into `evaluate/best_effort.rs` and `lex/best_effort.rs`, with a decode-level unit test for each generator.
3. **High: the best-effort pass over-hits, with false HardDeny (7 tests).**
   - `cat proj_atrium/*` gives `Deny(atrium-self)`. `9dc64108`'s prefix probes match any name containing `atrium`.
   - `Glob ["~/proj"] "**/.."` and `**/a/b/../../..` give `credential-read`. The selector only reaches home itself, which S8 exempts (§5, §10 row 16), and §1 says over-approximation never hits.
   - `ag x`, `ack x` and `find -type f` in a cwd linked from `~/.ssh` give Deny (`0cfbba5c`'s ancestor check). D-141 makes these tools unknown, with no recursion modelling.
   - Fix: prefix probes only on exact protected component names, and no ancestor hit for tools D-141 doesn't model as Search.
4. **Medium: the exact model's cwd survives `eval`, `source` and `.`.**
   - `eval 'cd proj && echo hi' && cat .npmrc` in Home gives `Hard(credential-read)` everywhere. Expected `[Ask, Ask, Allow, Allow]`.
   - `v4/paths/command.rs` never invalidates the cwd after a code-runner unit. §5 (an invisible `cd` makes later units Unread) and §1 both apply.
   - Fix: mark the cwd unknown after a code-runner or `eval`/`source`/`.` unit. This is a P2 file, so it needs a CR or the driver's approval.
5. **Medium: file-tool `$HOME/…` paths are literal (§5, §10 row 11), but `links_into_credentials_reach_nothing` and `hard_deny_applies_to_searches_and_inside_substitutions` still expect Deny.** Fix the expectations, citing §5 and §10 row 11.
6. **Medium: `~` in the middle of a word is outside §3, so `git show --stat HEAD~1` and `git diff HEAD~1` became Ask in Home** (`cr_12_ordinary_lines_keep_devs_verdict`, `ordinary_commands_are_not_hard_denied`). By the text that is right (§3, §11 item 2), but §11 never lists it. **The driver should decide**: confirm it, or amend §3.
7. **Medium: other expectations that are stale or unsupported.**
   - `globs_expand_against_the_real_folder`: `ls proj/*` reaches `proj/link-key`, so HardDeny is right (§10 row 8).
   - `cat proj/link-ssh/.` reads the `~/.ssh` directory itself. Give its verdict a citation.
   - `cat ~/**/id_work` is a variant-only match, so it's Unread and Ask/Allow/Allow (§5, §11 item 10), not Deny.
   - `ls vault/work`, `ls vault` and `ls proj/sub`: D-145 item 3 supports Deny, but the citations given don't, and this changes `dev`'s M1-03 finding 5, which §11 doesn't list. **The driver should decide**, or §11 gets amended.
   - `fix4_finding1` F1b: the bounded literal alternative `.ssh/id_rsa` is a §8 hit, so HardDeny, with a citation.
   - `p1_search_globs_that_leave_their_roots` [home] column: in a virtual environment it's Ask. Migrate it to a real-disk `Tmp` like the `cd` tables.
   - `ls >&2` gives Ask. §3's closed list has only `n>&m` with an explicit `n`. **The driver decides**: fix the test with a §3 citation, or accept `>&N` in P1.
   - `Grep ~/.ssh` in a virtual environment gives Ask, since S8 needs an existing path. Move it to a real-disk fixture.
   - `grep --dir=recurse` and `grep --rec` give Allow under Any and FullAccess. Abbreviated long options aren't in P3's table: §6 says Unread, D-141 says keep `dev`'s tables. **The driver decides.**
8. **Medium: `git diff --no-index ~/.ssh/config x` gives `Hard(credential-read)`; the handoff says Allow in Home.**
   - CR-P5-1's workaround (`evaluate.rs:626`) doesn't downgrade a conditional `git diff`.
   - Also: `v4/evaluate/tests.rs:390`, `ls ~/.ssh` in Home, expects Allow and gets Ask (not traced).
   - Fix: CR-P5-1 properly, `operand_access = Read` in `classify_home_readonly`. That's P3's file.
9. **Medium: Deny rules are skipped once the budget is exhausted** (`evaluate.rs:736`, `:1783` use `RuleCoverage::default()`; `gate.rs:263` returns Ask without rules when the protected-set build fails).
   - §1 step 3, a winning Deny, comes before step 5.
   - Case: Deny rule `["cat"]`, then a padded line plus `cat x`. Expected Deny, actual Ask.
   - Fix: a reserved budget for Deny rules, or evaluate them first.
10. **Medium: 11 clippy errors, all in `atrium-core`.**
    - `best_effort.rs:154`: derive `Default`.
    - Identical `if` blocks: `994/1002`, `2876/2882`, `2893/2900`.
    - `len() * size_of` instead of `size_of_val`: `1419`, `3211`, `5846`.
    - A very complex type: `2803`.
    - `evaluate.rs:703`: collapse the `if`.
    - `:798`: 8 arguments.
    - `:1786`: use `unwrap_or_default`.
11. **Low: changes outside P5's owned paths, and the CRs.**
    - `v4/tools.rs:567` (`classify_cd` sets `tool_name = Some("cd")`) is a justified wiring fix to frozen P3, but it needs the driver's approval.
    - CR-P5-1 is real, but its workaround is incomplete (finding 8).
    - CR-P5-2 keeps the `deny.rs` constants, the `engine.rs` `#[cfg(test)]` lists, the `mod.rs` shims and `HOME_READONLY_COMMANDS` for frozen P0/P3 parity tests. Under D-120 that needs a recorded exception or a fix.
    - Nothing else is outside the owned paths. Nothing live depends on the deleted code.
    - `security.test.ts` lowercases both sides, so it wouldn't catch a case-only drift.
12. **Low: handoff inaccuracies.**
    - "Zero other verdict changes" in `adversarial_gate` is false.
    - The base had 6 `#[ignore]`s, M1-13 numbers 4, 5, 6, 10, 11, 11; the earlier 7th was already un-ignored. The branch has 0, so verification's "1 ignored" comes from `dev` or a doctest.
    - Several citations don't support their verdicts (`ls vault/work` Deny, the `cd` tables as Ask, `HEAD~1`).

**O1 (orchestrator): the DoD timing condition fails** (see above). Re-time after the tests pass. If it's still slower, profile `adversarial_regression` and `adversarial_fuzz`: the evaluator, the protected-set build per call, and the best-effort pass.

## Test gaps
- Finding 1: protected paths under a directory with more than 4,096 entries.
- Finding 2: a decode-level unit test for each `dev` generator.
- Finding 3: §10-style rows for `cat proj_atrium/*`, `Glob ["~/proj"] "**/.."`, and `ag x` in a cwd linked from ssh.
- Finding 4: `eval`, `source` and `.` variants.
- Finding 9: a Deny rule plus an exhausted budget.
- §13's properties 3 and 4 exist only as unit sweeps through a virtual resolver, not through `SessionGate` in every setting with real fixtures. Property 1 doesn't assert that each generator really produces an outside-model line, so it could pass trivially.
- §10 has no row for a variant-only `**`, `HEAD~1` or `>&2`.
- Linux wasn't run. The only macOS-only code is in test fixtures (`/System/Volumes/Data`, `/.vol`, gated).
