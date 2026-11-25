# M1-41-rev4-p5-integration (P5): review round 2

Reviewer: Claude Sonnet 5.5, read-only, a subagent on Pedro's plan (D-162, D-170). Round 2 was written by Opus 5.5 (D-176) and is marked partial in its handoff.

Orchestrator verification (macOS, `dev` 2037bdb2 merged with the branch):
- fmt OK; protocol OK; typecheck OK; vitest 373/373; build OK;
- **clippy: 2 errors;**
- **workspace: 1084 passed, 21 failed, 1 ignored** (round 1: 52 failed);
- timings not improved (`adversarial_regression` 11.24 s against `dev`'s 2.64 s);
- the demo was not run.

Details are in the worktree's `.orchestration/review-input/M1-41-verify-r2.md`.

**D-155 count (D-175): 12 → 17, a rise.** This goes to the driver.

## Verdict

**Request changes.**
- Closed: review 1's findings 3, 4, 5, 6 and 12, plus the stale expectations in finding 7 apart from two items.
- Open: findings 1, 2, 8, 10 and O1. Findings 7, 9 and 11 are partly closed.
- The new `dev_readings.rs` port causes most of the defects:
  - it panics on non-ASCII input, which crashes the gate;
  - it still misses `dev`'s HardDenies;
  - it adds false HardDenies on ordinary lines.

All 21 failures are accounted for:

| Cause | Tests |
|---|---|
| Panics | 2 |
| The port's false hits and misses | 12 |
| Finding 1 | 1 |
| Finding 8 | 1 |
| The MCP false deny | 1 |
| Wrong expectations | 3 |
| Not traced | 1 |

## Rulings on round 2's disputes
- **`cat proj_atrium/*` as atrium-self: withdrawn.** The fixture links `proj_atrium/data_link` to the Atrium data dir, so the identity hit is right (§5).
- **`ag`/`ack`/`find` at a cwd linked from ssh: withdrawn.** Under D-141 and §11 item 17, a candidate path under a protected cwd is a hit.
- **Finding 4, other code runners: the worker is right.** Children can't move the shell's cwd. They are already possible writers, and hits stand.
- **Untested:** `command eval`, `builtin eval`, `nohup eval`.

## Findings
1. **High: the port panics on non-ASCII input.** An unterminated `'`, `"`, `$(` or backtick, or `\` before a multibyte char, slices inside a character (`dev_readings.rs:595, 760, 767, 787, 1007, 1010`).
   - Case: `echo 'a\u{200b}`, ``echo `本``, `echo \é | cat`.
   - Fix: char-aware `skip_construct`, or clamp to `floor_char_boundary`; add these to the fuzz alphabet.
2. **High: false HardDenies in the port** (5 tests).
   - a. Any shell counts as a stdin reader even with `-c` (`:1085`): `curl … | bash -o errexit -c 'jq .'` gives `Hard(pipe-to-shell)`, expected Ask/Ask/Allow/Allow. Use `reads_stdin_as_code_budgeted`'s `-c` exemption.
   - b. `$(brew --prefix)/bin/jq` is expanded as a whole command word, so it's "unknown" and gives `Hard(pipe-to-shell)` everywhere. Substitute only when the base name has the substitution.
   - c. `extend_cd_context` (`:1224`) joins the `cd` context with `;` instead of `&&`, so `cd proj && source /dev/stdin <<<'cat .npmrc'` gives `Hard(credential-read)`. Keep the join operator.
3. **High: HardDenies `dev` gave are still missed** (7 tests).
   - a. The `script` forms are checked on the unwrapped reading, which `unwrap_simple_be` already unwrapped (`script -t0 rm -rf ~/`).
   - b. `env -S` sinks: the readings lack the raw argv.
   - c. `eval $(echo rm -rf ~)`, and `bash -c "$(cat <<'EOF' … EOF)"`.
   - d. `at_command_position` (`:481`) misses `f() {`, `coproc N {` and `time -p {`, so the depth drifts.
   - e. Probably: pipelines inside compounds (`(echo 'rm -rf ~' | sh)`, `if …; then … | sh; fi`). `dev` flattened compound bodies.
4. **Medium: the O1 gate skips lines `dev` hard-denied.** `raw.contains("script")` misses `SCRIPT`, `s''cript` and `\script`, which CR-19 folds. Gate on a folded, quote-stripped check, or drop the special case.
5. **Medium: MCP free text is hard-denied** (`evaluate.rs:1399`). `{"text":"put your token in ~/.npmrc"}` gives `Hard(credential-read)`; `dev` checked only `path_like` strings (`deny.rs:296`).
6. **High: review 1's finding 1 is still open.**
   - `Through(dir)` matches every target under `dir` (`gap_applies`, `sensitive.rs:2446`), so a cap on any ancestor of home is still open-ended for every write under it.
   - `echo > out.txt` is not rememberable; not traced.
   - Also:
     - a `stat` error still gives `AnyTarget`;
     - a fresh `FsCaps` per listing removes the aggregate cap.
   - Fix:
     - scope the gap to targets whose next component could be an unlisted equivalent;
     - or `lstat` the listed fold spellings instead of listing.
7. **Medium: review 1's finding 8 (CR-P5-1) is still open.** `git diff --no-index ~/.ssh/config x` still gives `Hard(credential-read)`. Not traced; candidates are `resolve_kv_hard_deny_aliases` and `check_candidate_word`.
8. **Medium: a Deny can still be skipped.**
   - `evaluate_rules_reserving_deny` (`evaluate.rs:1683`) drops a found Deny if the reserve run itself exhausts. Evaluate Deny rules first, or keep partial coverage.
   - Also: the rules run twice per exhausting call, and the gate's failure path runs a second full `evaluate_call`.
9. **Medium: three wrong expectations.**
   - `round2_finding4_…`'s `;` forms are a real hit by the cwd union (`dev`, §2), so use `&&`.
   - `~/**/id_*` is HardDeny by probe matching (§2's footnote).
   - P2's `round11_r10f1_…` (`p2_tests.rs:6864`) expects `all_clear` for a folder linked from ssh, against §11 item 17.
10. **Medium: false Ask for in-root anchored selectors on a real disk** (`p1_anchored_selectors_inside_their_roots_on_a_real_disk`). `Glob ["~/proj"] ~/proj/**/*.md` gives Ask in Home, expected Allow. Not traced.
11. **Low: `HOME_READONLY_COMMANDS` is kept for tests only.** Move it to `dev_parity` or the P3 fixtures, or the driver records an exception (brief item 3, D-176 item 4).
12. **Low: §8 ordering in `dev_readings.rs`.**
    - Allocation before its charge: `:1297`, `:1074`, `:1405`.
    - `command_parts` and `cooked()` charge by word count, not bytes.
    - Uncharged: the heredoc `extend`, and the `push_payload` compare.
    - `shell_quote` is charged at 1×.
13. **Low: 2 clippy errors**, at `dev_readings.rs:852` (`while let`) and `:1482` (collapse the `if`).
14. **Low: `push_payload`'s dedupe key drops the `pipe_to_shell` flag** (`:1141`).
15. **Medium: undone work.** §13's properties 3 and 4 through `SessionGate` with real fixtures. The worker proposes a `#[doc(hidden)]` budget hook on the gate, which needs the driver's decision.
16. **Medium: undone work.** No §10-style rows for a variant-only `**`, `HEAD~1`, `>&2`, or the new generators.
17. **Medium: O1 is still open.**
    - Hot path 1: `gate.rs` rebuilds `build_protected_set` on every call, and `folded_spellings` lists every ancestor of every root (up to 4,096 entries). `dev` never listed ancestors.
    - Hot path 2: the payload scanner re-lexes and re-runs the best-effort pass for up to 64 payloads at depth 2, and the adversarial suites trigger it on almost every line.
    - Fix: cache ancestor listings with identity validation, or list only when a component can have an unlisted spelling; cap the scan per line.

## Test gaps
- Panic inputs added to the fuzz alphabet.
- Rows for `| bash -c`, `| $(brew --prefix)/bin/jq`, and `cd proj && …` payloads.
- Unit tests for `env -S mksh`, `eval $(…)`, `$(cat <<EOF)`, unwrapped `script`, `coproc N {`, and pipelines inside compounds.
- `SCRIPT`, `s''cript`, `\script`.
- MCP free text and path-like values.
- A huge ancestor of home with `touch`, `echo >` and `rm`, plus a stat failure.
- `classify_home_readonly` for `git diff --no-index`.
- A Deny plus a reserve that exhausts.
- `;` rows expecting Hard.
- `command eval`, `builtin eval`, `nohup eval`.
- §13's properties 3 and 4, and the §10 rows.
- Linux is still unrun.
