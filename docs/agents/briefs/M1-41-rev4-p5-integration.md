# M1-41-rev4-p5-integration: wire rev 4 into `SessionGate`, remove `dev`'s evaluator (P5)

Model: Gemini 4 Neon (`gemini-4-neon-xhigh`, `scripts/agent-run agy`, D-158, D-161) · Reviewer: Claude Sonnet 5.5, read-only (D-162; Codex when its usage is back) · **Integrated review (contract §13 step 5): the driver picks the reviewer** (Codex if it's back; Opus 5.5 otherwise)
Depends on: P2 (`M1-38-rev4-p2-paths`) and P3 (`M1-39-rev4-p3-tools-rules`), **both approved and merged into `dev`**. P0, P1 and P4 are already merged.
Go: needs the driver's go · Branch: `agent/neon/M1-41-rev4-p5-integration`, cut from `dev` after P2 and P3 merge · Worktree: `../atrium-wt/M1-41-rev4-p5-integration`
Contract: `docs/contracts/m1-policy-rev4.md`. Read all of it. P5 implements **§1's decision procedure, §2's table and best-effort pass, and §13 step 4**, and makes §10's examples and §11's changes (confirmed by Pedro, D-157) live.

**This is the first rev 4 task that changes behaviour.** Until P5, `dev`'s evaluator decides every call, and `policy/v4/` is built alongside it but not wired in. After P5, rev 4 decides, and `dev`'s evaluator is gone.

Also read these decisions in `docs/agents/decisions.md`. They narrow or clarify the contract, and the reviewer reviews against them:
- **D-133, D-135, D-138, D-139, D-157:** the rev 4 design, and §11 items 1 to 15 as confirmed.
- **D-141:** the tool table's six row kinds.
- **D-142, D-144, D-149:** P0's later fields (`write_only_hard_deny_identities`, `identity_gaps`, `Target::written`).
- **D-145, D-147, D-154:** what P2's path protection promises (`dev`'s, by identity and on the written spelling).
- **D-146, D-148:** the bracket class. **D-150:** structural caps before charges.
- **D-153, D-156:** P3's validator surface, and triggers that never consume option values.

## Read first, from the earlier tasks
- **The P0 to P4 handoffs** (`docs/agents/handoffs/M1-36…`, `M1-37…`, `M1-38…`, `M1-39…`, `M1-40…`). Each lists what it leaves for P5. The ones known now:
  - **P1:**
    - use `build_units_with_hook` / `CodeRunnerHook` (with `UnitReasonSink`) to attach P3's code-runner reasons while units are built;
    - use `lex::brace_expand_budgeted` (N11) for the best-effort pass's brace expansion;
    - `lex_exact` / `build_units` return `Err(Exhausted)` only when the budget is spent on entry;
    - P1's CR on `unwrap_unit` (it can't hand the unit back on exhaustion) stays a workaround, so don't change the frozen signature.
  - **P2:**
    - call the **resolver-backed** entry points (`resolve_command_targets`, `resolve_subject_paths`), not the pure `check_target`. Only they apply D-147;
    - give `build_protected_set` **its own `FsCaps`**, so the builder's walks don't use up the check's caps;
    - cache the build where it's safe;
    - after round 11, `FsResolver` charges the same on macOS and Linux. Keep it that way.
  - **P3:**
    - CR-P3-2's `RuleEvalContext` outcomes (D-153): P1/P2 Unread parts must reach `evaluate_rules` the same way as P3's `ToolEffects::unread`;
    - optionally, fold CR-17's five entries into `tools::GIT_CRED_SUFFIXES` / `GIT_CRED_NAMES`;
    - `HOME_READONLY_COMMANDS` in `atrium-protocol` was kept only for `dev`'s evaluator.
  - **P4:** every live call site passes `unread: None` today. P5 sets the Unread notice on `ApprovalRequested` from the evaluation's Unread reasons, so the UI shows it and hides "always…" (§9).
- **The disposition lists** in the P1, P2 and P3 handoffs (§12). They say, test by test, which of the M1-27 batch's tests and hunks P5 migrates, and with what verdict. The source branches are kept as tags: `archive/M1-27-engine-name-fold` to `archive/M1-32-link-globs` (D-164).

## Goal
1. **`crates/atrium-core/src/policy/v4/evaluate.rs`.** Implement P0's three stubs, matching their signatures and the `model.rs` types exactly:
   - `run_best_effort`: §2's best-effort pass. That is `dev`'s hard-deny matching on the raw text and on every unit the tolerant lexer produced (derived `-c` units included), plus `dev`'s hard-deny probe matching for shell globs and search filters (`Sensitive::glob_hit`, `search_glob_hit`). A hit is a HardDeny; a miss is nothing.
   - `decide`: §1's six steps in order, the first that applies decides, and §2's table per setting. Over-approximation never produces a hard-deny hit, except for the store-probe exception in §1.
   - `evaluate_call`: P1, then P3's tool effects, then P2's paths, globs and filters, then the best-effort pass, then P3's rules, then `decide`. Budgets per §8, with the structural caps checked first (D-150).
2. **Wire it into `SessionGate`** (`crates/atrium-core/src/policy/gate.rs`). The gate keeps its own jobs: the resolver, link walks, the multiply-linked-file check, `redeem`'s re-evaluation and the ledger. Only the evaluation underneath changes.
   - The adapters' and the daemon's use of `SessionGate` stays the same, and no protocol message changes, except setting P4's existing `unread` field.
3. **Remove `dev`'s evaluator.** That's `policy/shell.rs`, `policy/words.rs`, the `deny.rs` and `engine.rs` code no longer used, and `HOME_READONLY_COMMANDS` if nothing uses it any more.
   - Keep what rev 4 reuses on purpose, and say in the handoff, item by item, what was deleted and what was kept and why:
     - the best-effort pass's `dev` matching;
     - `apply_rules`' precedence;
     - `Sensitive`.
   - P4's Settings test parses `deny.rs`'s `ASK_FILES` / `ASK_TREES` / `ASK_NAME_PREFIXES`. If those move, update that test's source path in the same change, and say so.
4. **Migrate the test suites** by the disposition lists, plus §13's test plan:
   - **§10's table as a test suite**, with its fixtures, through `SessionGate`, end to end;
   - **property tests** of the boundaries, independent of the implementation's classifier, with a small generator per construct outside §3 and §4:
     - in Home and under prefix rules, a line containing such a construct is never Allow;
     - a fixture-planted hard-deny target that a modelled part touches is HardDeny in every setting;
     - compute exhaustion with no earlier decision is Ask in every setting;
     - exhaustion never creates a hit;
   - **the current adversarial suites** (`crates/atrium-core/tests/adversarial_*.rs`, `policy.rs`, the adapters' `*_policy_e2e.rs`), with expected verdicts migrated by the disposition lists. Every changed expectation cites §10, §11 or a decision.
   - **The 7 tests still `#[ignore]`d on `dev`** (M1-13 findings 4, 5, 6, 10 and 11, and others) each get their rev 4 verdict and run, or are dropped with the reason.

## Owned paths
- `crates/atrium-core/src/policy/**`: `v4/evaluate.rs` (and new `v4/evaluate/` files), `gate.rs`, `mod.rs`, and the deletions in `shell.rs`, `words.rs`, `deny.rs` and `engine.rs`. The other `v4/` files (P1–P3's) only for wiring fixes, each listed in the handoff.
- `crates/atrium-core/tests/**` (the policy and adversarial suites), and `crates/atrium-agents/tests/*_policy_e2e.rs` and `consistency.rs` (expected verdicts only).
- `crates/atrium-protocol/src/policy.rs`: only removing `HOME_READONLY_COMMANDS`, if nothing uses it.
- `app/src/ui/security.test.ts`: only the source path of the `deny.rs` parsing test, if those lists move.
- The handoff, `docs/agents/handoffs/M1-41-rev4-p5-integration.md`.
- **The interface is frozen** (`model.rs`, `limits.rs`, `tools.rs` and P1–P3's public functions). If a change is needed, don't make it: write a change request in the handoff, with the reason and the failing case.

## Rules
- Follow the worker rules in `docs/agents/README.md`. Under D-123, run only:
  - `cargo fmt --all`;
  - `cargo check --workspace --tests`.

  No `cargo test`, `cargo clippy` or `pnpm`: the orchestrator runs them.
- **Derive every expectation from the contract text and the decisions,** never from what the code returns. You can't run the tests, and most earlier rework rounds were wrong expectations.
- **Both macOS and Linux are supported.** Every `cfg` must build on both (P2 shipped Linux-only code once). Platform-only tests are `cfg`-gated.
- Never put test data in shell heredocs; write test inputs as Rust string literals. Tests that need a home folder use a temp folder and a fake `HOME`. Never read real credential paths.
- Commit in small steps: evaluator, wiring, deletion, then each suite. Never push, merge, rebase or switch branches.
- This task is large. If you run out of time or quota, commit what you have and write a `partial` handoff saying exactly where you stopped.

## Definition of Done (the orchestrator's verification)
- [ ] fmt, workspace clippy, `scripts/check-protocol-ts`, and `cargo test --workspace --no-fail-fast` are clean, on macOS and on Linux. Linux runs on rodrigo's machine, or is listed as owed for the gate.
- [ ] App typecheck, `pnpm test` and build are clean.
- [ ] No `#[ignore]` left without a reason. No existing test changed except as the disposition lists or §10/§11 say.
- [ ] The policy suites' run times on macOS are no worse than `dev`'s today, with the fuzz and policy timings in the handoff.
- [ ] The scripted M1 demo (`python3 app/scripts/m1_demo.py`, 27 steps) passes, because P5 changes live behaviour.
- [ ] The review approves with no findings (D-120). Then the integrated rev 4 review (§13 step 5) approves with no findings, before the merge.
