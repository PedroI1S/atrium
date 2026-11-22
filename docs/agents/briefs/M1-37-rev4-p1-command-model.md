# M1-37-rev4-p1-command-model: rev 4's command model (P1)

Model: Gemini 4 Neon (`gemini-4-neon-high`, `scripts/agent-run agy`, D-134) · Reviewer: Codex GPT-6.1 Sol `high`, read-only (D-124) · Depends on: P0 (`M1-36-rev4-p0-interface`, branched from its tip `1a09705`)
Go: D-138, D-140 · Branch: `agent/neon/M1-37-rev4-p1-command-model` · Worktree: `../atrium-wt/M1-37-rev4-p1-command-model`
Contract: `docs/contracts/m1-policy-rev4.md`. Read all of it. P1 implements §3 and §4, the best-effort pass's lexing in §2, and §8's budget rules for these stages.

## Goal
Implement the stubs P0 left in `crates/atrium-core/src/policy/v4/lex.rs` and `v4/units.rs`:
- `lex_exact`;
- `lex_best_effort`;
- `fold_command_word`;
- `build_units`;
- `unwrap_unit`.

They must match the signatures and the `model.rs` types exactly. **Nothing is wired into `SessionGate`** (that's P5), so no behaviour changes.

## What it must do
- **§3, words with provenance:**
  - The exact lexer accepts only the POSIX/bash/zsh intersection: the allowed unquoted bytes, glob characters, `~` only at a word's start, the quote and escape rules, the separators `;` `&&` `||` `|` and newline, and the closed redirect list.
  - Every other construct gives a precise `UnreadReason` from P0's enum (`$`, backtick, `{`, `#`, `&`, `&>`, `<&`, descriptors above 2, here-documents, reserved words, assignments, non-ASCII, backslash-newline, `"\q"`, …).
  - Bytes that come out of a quote or an escape keep their provenance and are never re-parsed.
- **`HOME`:** a unit after one that sets `HOME` (an assignment, `env`, `export`, `unset`) is Unread for `~`.
- **§4, command words:**
  - fold the command word (CR-19 and `FoldRules`, no Unicode normalization; port M1-27's folding, see below);
  - a command word with an unquoted glob is Unread.
- **§4, wrappers:**
  - only `command [--]`, `exec`, `nice`, `nice -n N`, `timeout [-s SIG] [-k D] D`, `time` and bare `env`, removed one by one up to `MAX_WRAP` (8);
  - reaching `MAX_WRAP` makes the unit Unread;
  - any other form makes it Unread;
  - read the wrapper rows from P0's table (`policy/tools.rs`) through its accessors, without editing the table.
- **§4, nested shells:**
  - only `sh`, `bash` and `dash` as `NAME [-e|-u|-x]... -c SCRIPT`, with nothing after `SCRIPT`;
  - `SCRIPT` is lexed recursively by §3 and §4, up to `MAX_DEPTH`, and keeps its parent unit index;
  - operands after the script, script files, stdin shells, `-i`/`-l`/`--rcfile`/`--login`/`-o`, `zsh -c` and other shells are Unread;
  - `sh x.sh` is Unread (program).
  - Classifying code runners (`awk`, `python`, …) is P3's job; leave a hook for it.
- **§2, the best-effort pass:**
  - lex the raw text tolerantly and produce every unit, including those derived from `-c` scripts, so P5 can run `dev`'s hard-deny matching over them;
  - you may call `dev`'s existing lexing in `policy/shell.rs` and `words.rs` read-only, through `pub(super)` items that already exist;
  - say in the handoff what you reused, because P5 removes `dev`'s evaluator.
- **§8, budgets:**
  - every stage charges the shared `ComputeBudget` **before** the work: one unit per byte scanned or copied, and allocations by size;
  - running out is `Exhausted`, and nothing found before that is lost;
  - lexing is linear in the line length; no stage copies suffixes or rescans the rest;
  - `MAX_BYTES` and `MAX_UNITS` give `TooLarge`.

## Work carried from the M1-27 batch (contract §12)
For each item below, read the source branch's diff (`git diff dev...<branch> -- <path>`, read-only). Then keep the hunk, rewrite it or drop it, and record which in the handoff:
- **`agent/hydrogen/M1-27-engine-name-fold`:**
  - CR-19 folding: keep, or port to `fold_command_word`;
  - the `deny.rs` name comparisons: use them for the best-effort pass only;
  - each bounded code scan the best-effort pass still needs: list them one by one;
  - drop K7's positional resolution and K9's parity.
- **`agent/hydrogen/M1-31-credential-folder-search`:**
  - N11's bounded brace expansion in `words.rs`: best-effort only;
  - the lexer memo in `shell.rs`.

Write the disposition list (hunks and tests) in the handoff. A test kept from those branches gets the verdict this contract gives, citing §10 or §11; never the old expectation.

## Tests
In-module tests (the items are `pub(crate)`), or `#[cfg(test)]` files under `v4/`:
- **The §10 rows that only P1 decides:** 3, 4 (its best-effort units), 5, 6, 7, 10 and 25, plus the `MAX_UNITS` row 20, asserting P1's outputs (units, `UnreadReason`, `TooLarge`). P5 asserts the final verdicts.
- **Every §3 and §4 boundary,** inside and outside the model: one case for each allowed byte, each listed outside construct, each redirect form, each wrapper form and its near misses, and each nested-shell form and its near misses.
- **Provenance:** `cat 'proj/key*'` has no unquoted glob, and `cat proj/key*` has one.
- **Budgets:**
  - exhaustion at every stage;
  - a line padded to exhaust the budget gives `Exhausted`, never a Clear result;
  - a linear-time check: lexing time and charge grow linearly from 10 KB to `MAX_BYTES`, asserted on the charge count, not on wall time.

## Owned paths
- `crates/atrium-core/src/policy/v4/lex.rs`, `v4/units.rs`, and new files under `v4/` for P1 only (for example `v4/lex/`).
- The handoff `docs/agents/handoffs/M1-37-rev4-p1-command-model.md`.
- **The interface is frozen.** If you need a change in `model.rs`, `limits.rs`, `tools.rs` or another task's stub, don't make it. Write it in the handoff as a change request, with the reason, and work around it.

## Rules
- Follow the worker rules in `docs/agents/README.md`. Under D-123, run only `cargo fmt --all`, `cargo check -p atrium-core --tests` and `cargo clippy -p atrium-core --tests -- -D warnings`. No `cargo test` and no `pnpm`.
- Never put test data in shell heredocs; write test inputs as Rust string literals. Commit in small steps. Never push, merge or switch branches.

## Definition of Done (the orchestrator's verification)
- [ ] fmt, workspace clippy and `cargo test --workspace --no-fail-fast` clean; no existing test changed.
- [ ] Codex review: approve with no findings (D-120), against contract §2–§4 and §8.
- [ ] If P0 changes after `1a09705`, the branch is rebased onto P0's final tip, and verification and review run again.
