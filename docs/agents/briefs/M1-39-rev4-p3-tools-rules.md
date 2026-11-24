# M1-39-rev4-p3-tools-rules: rev 4's tools and rules (P3)

Model: Gemini 4 Neon (`gemini-4-neon-high`, `scripts/agent-run agy`, D-134) · Reviewer: Codex GPT-6.1 Sol `high`, read-only (D-124) · Depends on: P0 (`M1-36-rev4-p0-interface`, branched from its approved tip `a5b9b3b`)
Go: D-138, D-151 · Branch: `agent/neon/M1-39-rev4-p3-tools-rules` · Worktree: `../atrium-wt/M1-39-rev4-p3-tools-rules`
Contract: `docs/contracts/m1-policy-rev4.md`. Read all of it. P3 implements **§6 and §7**, with §8's budget rules for these stages. The annex is `docs/contracts/m1-policy-rev4-tools.md`, generated from `policy/tools.rs`.

Also read these decisions in `docs/agents/decisions.md`. They narrow or clarify the contract, and the reviewer reviews against them:
- **D-141:** the tool table's six row kinds. Every other tool is an unknown tool.
- **D-145 and D-147:** how P2 protects paths. P3 hands P2 the targets and does not re-check paths itself.
- **D-146 and D-148:** the bracket-class form.
- **D-142, D-144 and D-149:** P0's later fields.

## Goal
Implement the stubs P0 left. They must match P0's signatures and the `model.rs` types exactly.
- **`crates/atrium-core/src/policy/v4/tools.rs`:**
  - `classify_unit`: apply `policy/tools.rs`'s table to one modelled unit;
  - `check_git_selection`: CR-12's git name-selection checks.
- **`crates/atrium-core/src/policy/v4/rules.rs`:**
  - `validate_rule_matcher`;
  - `evaluate_rules`;
  - `remember_matcher`.

Plus the rule-validation surfaces outside `policy/`:
- the daemon's rule creation (`crates/atrium-daemon/src/commands.rs` `create_rule`);
- the rules editor (`app/src/chat/rule-form.ts`).

**Nothing is wired into `SessionGate`.** That's P5, so the live policy doesn't change. The one exception is rule creation. The daemon and the rules editor start rejecting invalid Allow prefixes (§11 item 4), which is a live change. Its merge waits for Pedro's confirmation of the §11 items (D-138, D-013). Build it anyway.

## What it must do

### §6, `classify_unit`
Read P0's table through its accessors (`lookup`, `lookup_outside_model`, `lookup_unknown_by_design`, `subcommand`, `flag`, `flag_short`, `flag_long`, `is_forbidden_flag`, `ToolSpec::kind` and its methods). Don't edit `policy/tools.rs`. Return P0's `ToolEffects` for the unit.

**By row kind (D-141):**
- **`HomeReadOnly`:**
  - every word is a candidate path, for both read and write checks;
  - Home eligibility follows `dev`'s matching on the command word **as written**: `cat` qualifies, `/bin/cat` and `CAT` don't;
  - `m1.md` §9's guards apply: no globs, no assignments, and no redirects except fd duplication and `/dev/null`;
  - a forbidden flag only removes Home eligibility.
- **`Search`:**
  - the pattern operand and non-path flag values are not candidate paths (§11 item 9);
  - read the filter dialect and its case rule from the row;
  - an unsupported filter, or an unlisted flag, is Unread;
  - recursion and link-following flags set `recursive` and `follows_links`;
  - item 15's toggle (D-139) is P2's to apply. You pass the facts.
- **`Wrapper` and `NestedShell`:** P1 has already removed them. A unit that still starts with one is P1's Unread; don't re-model it.
- **`CodeRunner`:** always Unread.
  - When the row pins a program, record the code location or the pinned program, as Unread (program) (§11 item 8);
  - Allow-prefix validity is handled in `validate_rule_matcher`.
- **`ConditionalRunner`** (`find`, `git`, `sed`):
  - a listed trigger makes the unit Unread: a flag, a subcommand, or a sed script command `e`, `w` or `r`;
  - `sed -f` is Unread (program), pinned;
  - without a trigger, the unit gets unknown-tool treatment.
  - Detect sed's script commands without misreading pattern text. If you can't tell reliably, the unit is Unread.
- **`DerivedDestinationWriter`:**
  - with its trigger (`cp -R/-r/-a`, `mv` with a directory source, `rsync`, `tar -x`, `unzip`, `git clone`, `git checkout`), the writer is Unread(DerivedDestinations) (§11 item 10);
  - every word stays a candidate path;
  - without the trigger, the unit is an unknown tool.

**Unknown tools (§4)**, including everything in `UNKNOWN_BY_DESIGN`:
- every word is a candidate path for hard-deny, store and self-protection checks;
- the unit is a possible writer;
- it is never Home-eligible.

**`OUTSIDE_MODEL` entries** are Unread.

**Git (§6):**
- the program-running subcommands and flags are code runners (Unread);
- CR-12's name selection stays for `git show`, `git cat-file`, `git log -p` and `git diff`. In `check_git_selection`:
  - a path whose name matches a credential asks in Home, and so does a pathspec equal to, or an ancestor of, a credential-shaped name (M1-29 round 4, finding 1);
  - so does any pathspec magic or exclusion (`:(…)`, `:!`);
  - a pathspec with an active wildcard is Unread(GitWildcardPathspec);
  - pattern operands skip filesystem-path extraction but still go through these checks.

**`ln` and `ln -s`:** these are unknown tools under D-141, so their operands are candidate paths. A link to a protected path is therefore a hit through P2. Confirm this with a test rather than special-casing `ln`.

### §7, rules
- **`validate_rule_matcher`:** an Allow prefix rule that ends at a code runner with no pinned program is invalid (§11 item 4). Examples:
  - `["awk"]`, `["python"]`;
  - `["pipx","run"]`;
  - `["git"]`, which would cover `merge-index`.

  Use `ToolSpec::code_runner_no_pin` and the subcommand rows. Return P0's `RuleValidationError` with a reason the editor can show.
- **`evaluate_rules`:**
  - `dev`'s precedence: thread over Home over global, and a higher-ranked rule that covers the call suppresses lower-ranked ones (`engine.rs` `apply_rules`);
  - a Deny rule matches exact **and** best-effort units, and a winning Deny is Deny. With a Deny `["cat"]`, `cat "$X"` is Deny;
  - an Allow rule covers a unit only if the unit is Clear, a Store hit, or Unread (program) whose program is named in the rule's argv. The unit must also meet `m1.md` §9's simple-command conditions;
  - a rule never covers plain Unread or Exhausted;
  - path scopes are checked against §5's canonical targets, and an Unread target means the rule doesn't cover the unit;
  - **stored rules that validation now rejects stop covering anything** (§11 item 5).
- **`remember_matcher`:**
  - "always…" gives `CommandExact` or `PathGlob` as on `dev`;
  - it returns `None` whenever any part of the outcome is Unread or Exhausted.
- **The daemon:** `create_rule` validates through `validate_rule_matcher` and returns `ErrorCode::Invalid` with the reason. When rules are listed, a stored rule that is now invalid is marked invalid but stays listed, so the user can edit or delete it (§11 item 5). If marking it needs a protocol field, add one with `#[serde(default)]` and its TypeScript mirror.
- **`rule-form.ts`:** shows why a rule is invalid, and marks stored invalid rules.

### §8, budgets
Every stage charges the shared `ComputeBudget` before the work: one unit per byte scanned or copied, and allocations by size. Running out is Exhausted, nothing found before is lost, and TooLarge beats Exhausted. P1's and P2's handoffs explain how they did it, and their reviews show what the reviewer checks.

### `ReadOnlyCommand` (`atrium-protocol`)
`dev`'s evaluator still uses `HOME_READONLY_COMMANDS` until P5, so don't remove it. Add a parity test between it and the table's `HomeReadOnly` rows, so the two can't drift. P5 removes the old list.

## Work carried from the M1-27 batch (contract §12)
For each item, read the source branch read-only (`git diff dev...agent/hydrogen/M1-29-cr17-stores -- <path>`). Then keep, rewrite or drop each hunk and test, and record the disposition list in the handoff:
- **`agent/hydrogen/M1-29-cr17-stores`:**
  - daemon rule validation (`crates/atrium-daemon/src/commands.rs`) and its tests (`crates/atrium-daemon/tests/create_rule.rs`);
  - the core validator wiring and canonical-scope semantics (`policy/rule_check.rs`, `engine.rs`): fold them into `v4/rules.rs`;
  - `app/src/chat/rule-form.ts` and its tests.

  Drop the per-tool prefix guards: §6 and §7 and the table replace them. `Security.tsx` and `security.test.ts` go to **P4**, not P3, so each file has one owner. Don't touch them.
- **`agent/hydrogen/M1-32-link-globs`:** the archive operand roles become D-141's `DerivedDestinationWriter` trigger for `tar -x`. Record that disposition.

A kept test gets this contract's verdict, citing §10 or §11, never the old one.

## Tests
- **The §10 rows P3 decides.** Read §10 and list in the handoff which rows are P3's: the rules rows, the code-runner prefix rows, the git selection rows, and the tool-table rows. Assert P3's outputs for them (`ToolEffects`, `RuleCoverage`, `RuleValidationError`, the remembered matcher). P5 asserts the final verdicts.
- **Each row kind:** an accepted form, an unlisted flag, a trigger and a near miss.
- **Unknown tools** from `UNKNOWN_BY_DESIGN`: `rm`, `patch`, `curl`, `diff`, `ln -s`.
- **Rules:**
  - precedence;
  - Deny matching best-effort units;
  - Allow coverage for each unit outcome;
  - pinned programs;
  - path scopes with Unread targets;
  - invalid prefixes at creation, and stored ones that now fail validation;
  - "always…" being None for Unread and Exhausted.
- **Budgets:** exhaustion at each stage, and earlier results kept.
- **Daemon:** `create_rule` rejects invalid prefixes with a reason, and lists stored invalid rules as invalid.
- **App:** `rule-form.test.ts`.

## Owned paths
- `crates/atrium-core/src/policy/v4/tools.rs`, `v4/rules.rs`, and new files under `v4/` for P3 only (for example `v4/tools/`, `v4/rules/`).
- `crates/atrium-daemon/src/commands.rs` (rule creation and listing only) and `crates/atrium-daemon/tests/create_rule.rs`.
- `crates/atrium-protocol/src/policy.rs`: only a parity test, or a rule-validity field if the daemon needs one.
- `app/src/chat/rule-form.ts`, `app/src/chat/rule-form.test.ts`, and the TypeScript protocol mirror if you add a field.
- The handoff `docs/agents/handoffs/M1-39-rev4-p3-tools-rules.md`.
- **The interface is frozen.** If you need a change in `model.rs`, `limits.rs`, `tools.rs` or another task's stub, don't make it. Write it in the handoff as a change request, with the reason and the failing case, and work around it.

## Rules
- Follow the worker rules in `docs/agents/README.md`. Under D-123, run only:
  - `cargo fmt --all`;
  - `cargo check -p atrium-core -p atrium-daemon -p atrium-protocol --tests`;
  - `cargo clippy -p atrium-core -p atrium-daemon -p atrium-protocol --tests -- -D warnings`.

  No `cargo test` and no `pnpm` at all: the orchestrator runs the app's typecheck and tests.
- Never put test data in shell heredocs; write test inputs as Rust or TypeScript string literals. Tests that need a home folder use a temp folder and a fake `HOME`. Never read real credential paths.
- Commit in small steps. Never push, merge or switch branches.
- Since you can't run the tests, check each expectation against the contract's text, never against what the code returns.

## Definition of Done (the orchestrator's verification)
- [ ] fmt, workspace clippy, `cargo test --workspace --no-fail-fast` and the app's typecheck and tests clean; no existing test changed except by the disposition list.
- [ ] Codex review: approve with no findings (D-120), against contract §6, §7 and §8 and the decisions above.
- [ ] If P0 changes after `a5b9b3b`, the branch is rebased onto P0's final tip, and verification and review run again.
