# M1-03-policy: policy engine, command parsing, approval ledger

Model: Sonnet 5.5 (Opus 5.5 if it escalates) · Reviewer: Gemini 4 Argon · Depends on: contract (M1 rev 1)
Branch: `agent/sonnet/M1-03-policy` · Worktree: `../atrium-wt/M1-03-policy`

## Goal
Implement what decides whether a tool call runs: the policy engine (hard deny, rules, presets), the shell-command parser rules match against, and the approval ledger that makes every approval single use and bound to the exact call (D-046).

## Context
- `docs/contracts/m1.md` §3 (wording), §5 (approvals, in full), §9 (policy). Decisions D-044, D-046 in `docs/agents/decisions.md`.
- `crates/atrium-core/src/lib.rs`: `PolicyEngine`, `PolicyInput`, `PolicyVerdict`, `ApprovalLedger`, `RequestStore`, `RequestKey`, `Binding`, `LedgerError`.
- `crates/atrium-protocol/src/policy.rs`: `Rule`, `RuleMatcher`, `RuleScope`, `RememberScope`, `Preset`, `PolicyContext`, `HARD_DENY_DEFAULTS`.
- `crates/atrium-agents/src/lib.rs`: `ToolGate` (`check`, `redeem`, `question_ticket`), `ApprovalTicket`, `GateDecision`, `ToolCheck`.
- `docs/agents/reviews/M0-gate.md` §2 ("cancelled requests can still authorize effects") and `m1-must-fix.md` A4, O2.

## Owned paths
- `crates/atrium-core/src/policy/**`
- `crates/atrium-core/tests/ledger.rs`, `crates/atrium-core/tests/policy.rs`
- `crates/atrium-core/Cargo.toml`: dependency lines only, each with `# M1-03` (e.g. a shell-words parser, `sha2`, `globset`)
- `docs/agents/handoffs/M1-03-policy.md`

## Contract to implement
- `policy::DefaultPolicy: PolicyEngine`. Order: hard deny → thread rules → Home rules → global rules (deny before allow within a scope) → the preset. Home preset: Read, Search and the read-only command allowlist are allowed; everything else asks.
- `policy::parse_command`, `policy::canonical_input`, `policy::input_sha256` as declared.
- Hard-deny matching for every entry of `HARD_DENY_DEFAULTS`, checked on every segment and on the raw string, for `Execute` calls and for file tools' paths (`credential-read`, `atrium-self`).
- `policy::Ledger: ApprovalLedger` over an `Arc<dyn RequestStore>`. Tests use an in-memory `RequestStore` you write in the test file.
- `policy::SessionGate: atrium_agents::ToolGate`, one per session: knows thread, incarnation, cwd, preset, provider and the current turn; reads a rules snapshot (`(revision, rules)`) through a small trait you define in `policy/`. `check` → `Allow`, `Deny` (hard deny included), or `Ask { ticket }` after opening a ledger entry. Any internal error → `Deny` (fail closed).
- "Remember" (`resolve` with `remember`): `Execute` → `CommandPrefix` with the full argv of a simple command; file tools → `PathGlob` of the exact path; a compound or unparseable command can't be remembered (`resolve` still allows once, returns no rule).

## Definition of Done
- [ ] Every test in `tests/ledger.rs` and `tests/policy.rs` un-ignored and passing.
- [ ] A table-driven test with at least 40 command lines covering every hard-deny entry, quoting, `env`/`command`/`exec` prefixes, `sh -c '…'`, `$(…)`, backticks, `&`, `>` outside the cwd, and `~`/`..` paths.
- [ ] A concurrency test: two tasks resolve and redeem the same request at once; exactly one redeem succeeds.
- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test -p atrium-core --test ledger --test policy` and `--lib`
- [ ] Handoff with the commands' output and a list of every command shape you chose to `Ask` rather than match.

## Out of scope
The store's SQL (M1-02: you only use `RequestStore`), UI wording, adversarial testing beyond the DoD (M1-13 attacks your code next).

## Escalate if
A hard-deny entry can't be matched reliably as written: say exactly which cases, propose wording, and stop on that entry (Pedro owns the list, D-013). Any contract type is missing something: change request. Two fix rounds without passing.
