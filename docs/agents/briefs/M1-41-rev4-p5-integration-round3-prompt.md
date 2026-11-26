<!-- Round 3 worker prompt for P5 (D-178). Give it to an Opus 5.5 worker started in P5's worktree. Written 2026-11-26 by Pedro's orchestrator. -->

You are the worker (Claude Opus 5.5, D-176, D-178) on Atrium task M1-41-rev4-p5-integration (P5 of policy rev 4), rework round 3, in the worktree `../atrium-wt/M1-41-rev4-p5-integration`, on branch agent/neon/M1-41-rev4-p5-integration (HEAD `145667af`).

## Rules (they override anything else)
- **D-123: the only commands you may run:**
  - `cargo fmt --all`;
  - `cargo check --workspace --tests`;
  - read-only commands (`git log`, `git show`, `git diff`, `grep`, reading files);
  - `git add`, `git rm` and `git commit` on this branch.
  
  Never run `cargo test`, `cargo clippy`, `cargo build` or `pnpm`. Never push, merge, rebase, reset, stash or switch branches.
- **Never put anything in shell heredocs**, test data included. Write files with your edit and write tools. Test inputs are Rust string literals. Tests that need a home folder use a temp folder and a fake `HOME`. Never read real credential paths.
- **Derive every expected verdict from the contract text and the decisions,** never from what the code returns. You can't run the tests.
- **Both macOS and Linux must build.** Gate platform-only tests with `cfg`.
- **Use the current contract, decisions and brief from `dev`, not the branch's older copies.** Read them with `git show dev:<path>`:
  - `docs/contracts/m1-policy-rev4.md`;
  - `docs/agents/decisions.md` (D-176, D-177 and D-178 in full);
  - `docs/agents/briefs/M1-41-rev4-p5-integration.md` (item 3 is corrected).

  Never edit those files on the branch.
- Commit after each finished piece. If you run out of context or time, commit what you have and write a `partial` Round 3 section saying exactly where you stopped.

## Read first
1. Both reviews, in full: `git show dev:docs/agents/reviews/M1-41-rev4-p5-integration-round1.md` and `…-round2.md`.
2. `git show dev:docs/agents/reviews/M1-41-rev4-p5-integration-verify-r2.md`: the 21 failing tests with their panics, the 2 clippy errors, and the timing table.
3. The handoff `docs/agents/handoffs/M1-41-rev4-p5-integration.md`: rounds 1 and 2.
4. The contract's §2 (the best-effort pass and its footnote), §8, and §11/§12, which list the `dev` behaviours rev 4 drops (K7, K9, `$'…'`, the globbed `cd`).
5. **`dev`'s code, at the branch's base:** `git show 5c4d154f:crates/atrium-core/src/policy/deny.rs`, `…/shell.rs`, `…/words.rs` and `…/engine.rs`.

## The main change (D-178): restore `dev`'s matching, don't rewrite it
The best-effort pass is, by §2, `dev`'s hard-deny matching on the raw text and on every unit, plus `dev`'s hard-deny probe matching for shell globs and search filters. That code exists and passed every adversarial suite on `dev`. Rounds 1 and 2 rewrote it by hand instead, and most of the open findings come from the rewrite.

**Step 1: map before you delete.**
- Make a table of every function in `v4/evaluate/best_effort.rs` (round 1, 6,302 lines) and `v4/evaluate/best_effort/dev_readings.rs` (round 2, 1,758 lines). For each, say one of:
  - **replaced:** it reimplements part of `dev`'s matching, and you name the `dev` function that replaces it;
  - **kept:** it is rev 4–specific, and you say why. Examples: the glue that runs the pass over P1's tolerant-lexer and derived `-c` units, the §2 table hooks, `ToolSubject` handling for MCP and file tools, the store-probe exception.
- Put the table in the handoff's Round 3 section and commit it **before** deleting anything.
- **Do not touch** `v4/lex/best_effort.rs`. It is P1's tolerant lexer.

**Step 2: restore.**
- Bring back `dev`'s `shell.rs`, `words.rs` and the `analyze`/`hard_deny` path of `deny.rs` (with what it needs, such as `Analysis` and `reads_stdin_as_code`) from `5c4d154f`. Put them under a module such as `policy/v4/evaluate/dev/`.
- Keep the code as it was. **Only these changes are allowed:**
  - **§8 charge points:** every loop or step that scans, copies or matches charges the shared `ComputeBudget` before it runs, one unit per byte or step, and allocations are charged by size before they're made. Thread the budget through; on `Err(Exhausted)` stop, keeping hits already found (§8: exhaustion never creates a hit).
    - Don't replace `dev`'s own bounds (`MAX_UNITS`, depth caps, `push_bounded_str`): keep them as they are.
    - If a `dev` routine copies suffixes or rescans the remainder (§8 complexity rule), charge it as it is and list it in the handoff. Don't restructure it.
  - **The §11/§12 drops:** remove exactly the `dev` behaviours the contract drops (K7, K9, `$'…'`, the globbed `cd`), each with a comment citing the item.
  - **Non-ASCII safety:** `dev` lexes over `chars`. Wherever a byte index slices a `&str`, make it char-safe.
  - **Names and paths:** use the rev 4 `Sensitive`/protected set (`v4/paths/sensitive.rs`) where `dev` used `deny.rs`'s `Sensitive`, keeping `dev`'s matching semantics. List each such seam in the handoff.
  - **Wiring:** call the restored analysis from the best-effort pass, on the raw text and on each P1 unit, including derived `-c` units. A hit is a HardDeny; a miss is nothing (§2).
- Then delete everything the table marks **replaced**, including `dev_readings.rs`.

**Step 3: the acceptance target.** Every adversarial and policy test that passed on `dev` (base `5c4d154f`) and that §11/§12 or a decision doesn't change must give `dev`'s verdict again. In particular, the 12 tests review 2 lists under findings 2 and 3 (`k3_ordinary…`, `round_5_major…`, `round_6_major…`, `round_6_ordinary…`, `round_8_ordinary…`, `finding_7`, `round_6_item_4`, `round_7_item_5`, `round_7_item_6`, `leftover_wrappers…`, `round2_finding2…`, `…substituted_words…`). Remove or rewrite `dev_readings.rs`'s own unit tests with it; keep tests that pin `dev` behaviour, pointed at the restored code.

## Then close every other open finding (D-120: all of them)
Fix the findings from review 2 that step 2 doesn't remove, plus every review 1 finding review 2 marks open:

| Finding | Fix |
|---|---|
| R2-1 (panics) | No panic on any input. Add `\é`, `$(本`, `` `本 `` and unterminated `'` and `"` before a multibyte char to the fuzz alphabet. |
| R2-4 (the `script` gate) | Remove the O1 raw-text gate, or make it fold case and strip quotes and escapes as CR-19 does, so `SCRIPT`, `s''cript` and `\script` aren't skipped. |
| R2-5 (MCP) | Raw checks only on `path_like` strings, as `dev`'s `deny.rs:296` did. |
| R2-6 / R1-1 (listing cap) | A cap or `stat` error on an ancestor must never make every target under it Unread. Scope the gap to targets whose next component could be an unlisted equivalent of the folded name, or `lstat` the listed fold spellings instead of listing. Restore an aggregate cap across listings. Trace why `echo > out.txt` stays unrememberable while `touch` passes, and fix it. Add tests: a huge ancestor of home with `touch`, `echo >` and `rm`, and a `stat` failure. |
| R2-7 / R1-8 (CR-P5-1) | Trace `git diff --no-index ~/.ssh/config x` (still `Hard(credential-read)`) to its source and fix it. Add a unit test of `classify_home_readonly` for that line. |
| R2-8 (Deny skipped) | A Deny found is never dropped. Evaluate Deny rules first, or keep partial coverage on `Err`. Avoid the second full `evaluate_call` in `gate.rs`'s failure path. Add a test where the reserve itself exhausts. |
| R2-9 (wrong expectations) | Each with its citation: `round2_finding4_…`'s `;` forms (use `&&`, and add a `;` row expecting Hard); `~/**/id_*` is HardDeny by probe matching (§2's footnote); P2's `p2_tests.rs:6864` `round11_r10f1_…` under §11 item 17. |
| R2-10 | Trace the false Ask for in-root anchored selectors on a real disk (`p1_anchored_selectors_inside_their_roots_on_a_real_disk`) and fix it. |
| R2-11 (D-177 item 2) | Remove `HOME_READONLY_COMMANDS` from `atrium-protocol` and move the table into the test fixtures. Run `cargo check` on the workspace; the orchestrator runs `check-protocol-ts`. |
| R2-12 | §8 ordering in whatever of `dev_readings.rs` survives (probably nothing after step 2). |
| R2-13, R2-14 | Clippy; `push_payload`. Moot if `dev_readings.rs` is deleted; say so. |
| R2-15 (D-177 item 1) | Add a `#[doc(hidden)]` test-only `SessionGate` constructor that takes the budget limits. Then write §13's properties 3 and 4 through `SessionGate`, in every setting, with real fixtures. |
| R2-16 | §10-style rows through `SessionGate` for a variant-only `**`, `HEAD~1`, `>&2`, `| bash -c '…'`, `| $(brew --prefix)/bin/jq .`, `cd proj && …` payloads, and `command eval`, `builtin eval`, `nohup eval`. Each expectation cites the contract. |
| R2-17 / O1 (timings) | `gate.rs` rebuilds the protected set on every call, and `folded_spellings` lists every ancestor of every root. List only when a component can have an unlisted spelling (non-ASCII, or a name the folds can change). Or cache ancestor listings across calls, keyed by identity and checked again at `redeem`. Don't drop the per-call check that catches a symlink swap. Say in the handoff what you changed and why it's safe. |

## Finish
- `cargo check --workspace --tests` clean with no warnings, then `cargo fmt --all`.
- Add `## Round 3` to the handoff, with:
  - the mapping table (step 1);
  - what was restored, and every change to the restored code (charge points, drops, char safety, seams);
  - each finding: the cause, the fix, the commits and the tests;
  - every changed expectation with its citation;
  - every change to a P1, P2 or P3 file (D-176 item 4);
  - the cfg audit.

Your final message lists what you did for each step and finding, and the commits.
