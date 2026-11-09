# M1-03-policy: policy engine, command parsing, approval ledger

Model: Sonnet 5.5 (Opus 5.5 if it escalates) · Reviewer: Gemini 4 Argon · Depends on: contract (M1 rev 3)
Branch: `agent/sonnet/M1-03-policy` · Worktree: `../atrium-wt/M1-03-policy`

## Goal
Implement what decides whether a tool call runs: the policy engine (hard deny, rules, presets), the shell-command parser rules match against, and the approval ledger that makes every approval single use and bound to the exact call (D-046).

## Context
- `docs/contracts/m1.md` §3 (wording), §5 (approvals, in full), §9 (policy). Decisions D-044, D-046 and **D-050** (Home defaults and the always-blocked list, approved by Pedro) in `docs/agents/decisions.md`.
- `docs/agents/reviews/M1-contracts.md` findings 1–3 and 6: the attacks your code must withstand.
- `crates/atrium-core/src/lib.rs`: `PolicyEngine`, `PolicyInput`, `PolicyVerdict`, `ApprovalLedger`, `RequestStore`, `RequestKey`, `Binding`, `LedgerError`.
- `crates/atrium-protocol/src/policy.rs`: `Rule`, `RuleDraft`, `RuleMatcher`, `RuleScope`, `RememberScope`, `Preset`, `PolicyContext`, `ToolSubject`, `HARD_DENY_DEFAULTS`, `HOME_READONLY_COMMANDS` (with flag guards).
- `crates/atrium-agents/src/lib.rs`: `ToolGate` (`check`, `redeem`, `question_ticket`), `ApprovalTicket`, `GateDecision`, `ToolCheck`.
- `docs/agents/reviews/M0-gate.md` §2 ("cancelled requests can still authorize effects") and `m1-must-fix.md` A4, O2.

## Owned paths
- `crates/atrium-core/src/policy/**`
- `crates/atrium-core/tests/ledger.rs`, `crates/atrium-core/tests/policy.rs`
- `crates/atrium-core/Cargo.toml`: dependency lines only, each with `# M1-03` (e.g. a shell-words parser, `sha2`, `globset`)
- `docs/agents/handoffs/M1-03-policy.md`

## Contract to implement
- `policy::DefaultPolicy: PolicyEngine`, **pure** (no filesystem, clock or environment; everything comes in `PolicyInput`). Order: hard deny → thread rules → Home rules → global rules (deny before allow within a scope) → the preset.
  - Home preset (D-050): file reads and searches allowed, except a recursive search rooted at `~` itself (asks); `HOME_READONLY_COMMANDS` allowed with their `forbidden`/`only` guards when the line has no `$` expansion, globs, brace expansion or newline and no `git -c`/`--config-env`/`--exec-path`; everything else asks.
  - A `ToolSubject::Unknown` call is never allowed by a rule or preset: hard deny on the raw input, otherwise ask. A command subject is treated as `Execute` whatever `kind` says.
- `policy::parse_command` (separators `;`, `&&`, `||`, `|`, newline), `policy::canonical_input`, `policy::input_sha256`.
- Hard-deny matching for every entry of `HARD_DENY_DEFAULTS` as approved in D-050, for every tool kind whose subject names a matching command or path (searches included), on every segment, inside command and process substitutions, and on the raw string. Atrium's paths come in `PolicyInput::atrium_data_paths` (no read, no write) and `atrium_protected_paths` (no write).
- `policy::Ledger: ApprovalLedger` over an `Arc<dyn RequestStore>`, implementing every state and transition in `lib.rs` (including `approved → cancelled | expired`) and `TooMany`. Tests use an in-memory `RequestStore` you write in the test file.
- `policy::SessionGate: atrium_agents::ToolGate`, one per session (thread, incarnation, cwd, home, Atrium's paths, preset, provider), reading a rules snapshot `(revision, rules)` through a small trait you define in `policy/`. Exactly as documented on `SessionGate`: path resolution (its only filesystem access), the "always…" matcher computed at `check` (`Binding::remember_matcher`, and `ApprovalTicket::rememberable`), `redeem` re-evaluating the policy when the rules revision or preset changed (excluding the rule this approval itself created) before the ledger's `redeem`. Any internal error → `Deny`.
- `resolve` with `remember`: creates the rule from `Binding::remember_matcher` in the same transaction; with no matcher it still allows once and returns no rule. A key whose incarnation is older than `RequestStore::current_incarnation` → `StaleIncarnation`.
- `parse_command` fills `Segment::assignments` and `Segment::redirects`. Rules and Home's read-only list match a segment only with no assignments and only `Redirect::harmless` redirects; the hard-deny list looks through assignments and checks redirect targets as paths. In Home, a command whose `ReadOnlyCommand::recursion` applies asks unless every search root is given and resolves to a file or a folder strictly inside `~`.
- Path resolution, `cd` handling, the "always…" matcher's limits and `redeem`'s unconditional hard-deny re-check are as documented on `SessionGate` (review rev 2, finding 2: `ls > ~/.zshrc`, `rg token` in `~`, `LD_PRELOAD=… ls`, `cat .npmrc`, `cd .ssh && cat id_ed25519` are your test cases; from the rev-3 check: `file -f.npmrc`, `grep -f.ssh/id_ed25519 x sub/a`, `rg -e sub`, `dd if=link`, `cd tmp && cd && rm -rf .`, `cd tmp && cd - && cat .npmrc`).

## Definition of Done
- [ ] Every test in `tests/ledger.rs` and `tests/policy.rs` un-ignored and passing.
- [ ] A table-driven test with at least 60 command lines covering every hard-deny entry and each addition in D-050, quoting, `env`/`command`/`exec` prefixes, `sh -c '…'`, `$(…)`, backticks, `<(…)`, `&`, newlines, `>` outside the cwd, `~`/`..`/`$HOME` paths, symlinks into credential folders, and every flag guard in `HOME_READONLY_COMMANDS` (`rg --pre`, `git branch -D`, `git diff --output`, `tree -o`, …).
- [ ] A concurrency test: two tasks resolve and redeem the same request at once; exactly one redeem succeeds.
- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test -p atrium-core --test ledger --test policy` and `--lib`
- [ ] Handoff with the commands' output and a list of every command shape you chose to `Ask` rather than match.

## Out of scope
The store's SQL (M1-02: you only use `RequestStore`), UI wording, adversarial testing beyond the DoD (M1-13 attacks your code next).

## Escalate if
A hard-deny entry can't be matched reliably as written: say exactly which cases, propose wording, and stop on that entry (Pedro owns the list, D-013). Any contract type is missing something: change request. Two fix rounds without passing.
