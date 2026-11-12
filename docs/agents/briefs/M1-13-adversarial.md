# M1-13-adversarial: adversarial tests (hard deny, rules, ledger, socket roles) and the policy leftovers

Model: Sonnet 5.5 (contract §15 said Argon; D-060), second part Gemini 4 Argon (D-066) · Reviewer: Opus 5.5, Fable 5.2 on rodrigo's shifts (read-only) · Depends on: M1-02, M1-03, M1-05, M1-10 (all merged)
Go: D-062 (Pedro, 2026-11-12) · Branch: `agent/sonnet/M1-13-adversarial` · Worktree: `../atrium-wt/M1-13-adversarial`

## Second part (rodrigo's shift, D-065, D-066): Gemini 4 Argon, reviewed by Claude Fable 5.2
The first part (Sonnet 5.5) is on the branch: read `docs/agents/handoffs/M1-13-adversarial.md` first. Same branch, same owned paths. Do these, in order:
1. **Engine fixes for findings 2, 1, 7 and 8** (in that order), in `crates/atrium-core/src/policy/**` only. The handoff proposes a fix for each. Each fix gets table tests that fail on the old code, and its `#[ignore = "M1-13 finding N"]` test loses the `ignore` and passes. A fix may only make a verdict stricter (Allow → Ask or Deny); nothing ordinary may become a hard deny, and the round 1–6 tables and the existing `policy` and `ledger` tests still pass. If a fix can't be done without touching a contract type or a default, stop and leave it as a finding.
2. **Daemon adversarial tests**, `crates/atrium-daemon/tests/adversarial_*.rs`: the list in the handoff's "Not done / next step" item 1.
3. **Host adversarial tests**, `crates/atrium-host/tests/adversarial_*.rs`, for what is missing on process identity (Linux paths included: this machine is Linux).
4. The full Definition of Done below, and the handoff updated: append a "Second part" section, keep the first part's text.
- **Not yours:** finding 6 (CR-12, the owner decides), findings 3, 4, 5, 9, 10 and 11 (they stay ignored tests), `docs/agents/change-requests.md`.
- **Worker rule 6** (`docs/agents/README.md`): never put test data in a shell heredoc and never write command lines that name real credential files as shell text. Hostile inputs are Rust string literals inside tests, written with the file tools.

## Goal
Attack what M1 relies on for safety, as an agent running as the user would: the policy's hard denies and auto-allows, remembered rules, the approval ledger, the daemon's socket roles and limits, and process identity. Turn every attack into a test. Fix the small leftovers the M1-03 and M1-05 reviews put on this task; anything bigger becomes a finding in your handoff, not a silent change.

## Context
- `docs/contracts/m1.md` §4 (daemon authority, roles, peer verification), §5 (approvals, binding, remember, hard deny wins), §9 (policy, Home's auto-allowed commands, D-050/D-051/D-054), §10 (lifecycle), §6.2 (wire, D-058's subscription rule).
- `crates/atrium-core/src/policy/**`, `crates/atrium-core/src/ledger*`, `crates/atrium-daemon/**`, `crates/atrium-host/**`.
- Where M1-03 said to attack: `docs/agents/handoffs/M1-03-policy.md` ("Follow-ups" and "Known limits for M1-13"): `shell::lex` (quotes, expansions), `words::apply_cd` and `deny::delete_blocked` (cwd tracking), the `rg`/`grep` flag tables in `engine.rs`, `Sensitive::hit` (`~/.ssh` name rules), hard links into the credential trees.
- The reviews' leftovers: `docs/agents/reviews/M1-03-policy-round3.md` (hard links, all four kinds of sensitive tree), `M1-03-policy-round6.md` findings 1–3 (the `$(…)` false hard deny; bodies written to a file, fifo or fd and run later; unrecognised wrappers and shells).
- M1-05's list: `docs/agents/handoffs/M1-05-daemon.md`, "For M1-13".
- `docs/agents/status.md`, the M1-03 row ("for M1-13").

## Owned paths
- New test files only, in each crate: `crates/atrium-core/tests/adversarial_*.rs`, `crates/atrium-daemon/tests/adversarial_*.rs`, `crates/atrium-host/tests/adversarial_*.rs` (shared helpers may go in a new `tests/common_adversarial/` folder per crate).
- Fixes, limited to the leftovers listed below: `crates/atrium-core/src/policy/**`.
- `docs/agents/handoffs/M1-13-adversarial.md`
- Nothing else. A bug found outside `policy/**` is a finding with a failing test marked `#[ignore = "M1-13 finding N"]`, not a fix.

## What to do
1. **Fix the listed policy leftovers** (each with a table test):
   - round 6 finding 1: `$(…)` in an unquoted here-doc body or in the owner's arguments must not cause a false hard deny;
   - round 6 finding 3: add the wrappers (`caffeinate`, `arch`, `sandbox-exec`, `flock`, `chroot`, `nice`, `nohup`, `timeout`, `env` forms you find unhandled) to `unwrap`, `mksh` and `yash` to `SHELLS`, a command word starting with `$` reads code, `at`, `batch`, `crontab -`, `make -f -`, `script` are shell readers;
   - M1-03 finding 5: a symlink inside `~/.ssh` pointing at an ordinary folder no longer makes that folder sensitive (`symlink_metadata`);
   - hard links: never auto-allow a regular file under Home with `st_nlink > 1` outside the sensitive roots (asks instead).
   Round 6 finding 2 (a body run later through a file, fifo or fd) stays a documented limit unless the cheap fix in that review is clean.
2. **Attack and test**, recording every input you try and its verdict in table tests:
   - Policy: lexer edge cases (quotes, ANSI-C `$'…'`, backslash-newline, unicode look-alikes, NUL and control characters, very long lines), cwd tracking (`cd -`, `pushd`, `CDPATH`, subshells), flag tables (clusters, `--opt=value`, abbreviations), path aliases (`/private/var`, firmlinks, `..` through symlinks, case-insensitive APFS names), `~user` forms, globbing. **No input may reach Allow that touches a sensitive path or writes outside the allowed places**, and nothing ordinary may be hard-denied (false hard denies can't be overridden).
   - Rules and ledger: an exact rule never matches a longer argv; a remembered decision never outranks a hard deny; a stale, replayed, cross-thread, cross-incarnation or already-redeemed approval is never executed; concurrent redeems of one ticket redeem once.
   - Socket roles (§4): a non-`App` peer can't resolve approvals or change settings; a forged `Hello`, a `Hello` a byte at a time across the deadline, oversized frames, a full control lane (8 MiB), many replaced subscriptions in a row and a replacement while live (D-058), a store read that fails during a replay.
   - Daemon/host leftovers from M1-05's list: `kill_recorded` returning `Unverified` for a recorded process at start-up, a closure that fails in recovery (the start fails and the next one closes it), the slow-command limit.
3. Every finding: the input, what happened, what the contract says should happen, severity.

## Definition of Done
- [ ] The four leftover fixes, each with tests; the round 1–6 review tables still pass.
- [ ] New adversarial test files, all passing, except findings marked `#[ignore = "M1-13 finding N"]`.
- [ ] A proptest or fuzz-style test for the policy (random command lines from a hostile alphabet: no panic, nothing sensitive reaches Allow), bounded to run in under a minute.
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test -p atrium-core -p atrium-daemon -p atrium-host`.
- [ ] Handoff: what was attacked, the findings with severities, the limits left, the test counts.

## Rules
Work only in your worktree. Tests that touch the filesystem use temp folders and a fake `HOME`; never read or write the real `~/.ssh` or credential files. Stop every daemon you start.

## Escalate if
A finding needs a contract change (write a change request); an attack reaches Allow on a sensitive path (stop and report it at once in your final message).
