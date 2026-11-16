# M1-18-policy-followup: credential stores that ask (CR-12), search globs that leave their roots (P1), and M1-13's known limits K1–K4

Model: Claude Opus 5.5 (Agent subagent; D-074: security work whose tests need credential paths as literals) · Reviewer: Gemini 4 Hydrogen `high` (read-only, cross-provider) · Depends on: M1-13 (merged `02c13bc`)
Go: D-099 (Pedro, 2026-11-15) · Branch: `agent/opus/M1-18-policy-followup` · Worktree: `../atrium-wt/M1-18-policy-followup`

## Goal
Close the policy items still open after M1-13's merge, without any Allow regression against `dev`. Each item already has a decision or a known shape. Don't redesign the policy.

## Context
- Contract `docs/contracts/m1.md` §9: the hard-deny list, and the paragraph "Credential stores that ask (rev 3.1, CR-12, D-082)". That paragraph is what you implement for CR-12.
- `docs/agents/change-requests.md` CR-12; `docs/agents/decisions.md` D-082, D-098, D-099.
- `docs/agents/m1-must-fix.md`:
  - P1 (the "From the M1-07 reviews" table);
  - the section "M1-13 merged (`02c13bc`, D-098): known limits left open" (K1–K6).
- `docs/agents/reviews/M1-13-adversarial-round8.md`, blockers 1–4: mechanisms and evidence for K1–K4. Earlier rounds' reviews are beside it.
- `docs/agents/handoffs/M1-13-adversarial.md`: how rounds 3–8 handled each finding, and the invariants the gate relies on (B1–B5).
- Code: `crates/atrium-core/src/policy/{deny.rs,gate.rs,engine.rs,shell.rs,words.rs}`, `crates/atrium-protocol/src/policy.rs` (`HARD_DENY_DEFAULTS`), and the tests `crates/atrium-core/tests/adversarial_*.rs` with `common_adversarial`.

## Owned paths
- `crates/atrium-core/src/policy/**`, `crates/atrium-core/src/lib.rs`, `crates/atrium-core/tests/**`.
- `crates/atrium-protocol/src/policy.rs`, only if the CR-12 list belongs beside `HARD_DENY_DEFAULTS`. Then `bindings/protocol.ts` is regenerated only, and `scripts/check-protocol-ts` must pass.
- `crates/atrium-agents/src/**`, only where an adapter builds a search subject and P1's engine change needs it to.
- `docs/agents/handoffs/M1-18-policy-followup.md`.
- Not yours: `docs/contracts/**`. If the contract can't be implemented as written, escalate.

## What to do
1. **CR-12, credential stores that ask.** Add the contract §9 list as a sensitive (not hard-denied) set with the same identity, symlink and hard-link handling as the hard-deny list:
   - Home never auto-allows a call that reads, searches or names them;
   - `git show`/`git cat-file` of a path whose name matches a credential asks;
   - a one-time approval or a remembered rule may allow them;
   - Any/FullAccess behave as for other sensitive paths (read the contract; don't invent a new mode rule).

   Un-ignore the tests marked "M1-13 finding 6" and make them pass. The contract says the Settings page that lists what is protected names these stores. The app has no such list today (checked by the orchestrator in `app/src/ui/`), so don't build UI: say so in the handoff.
2. **P1, search globs that leave their roots.** The engine must refuse or ask for a `ToolSubject::Paths { paths, glob }` whose `glob` can name files outside every root:
   - an absolute glob;
   - `~`;
   - `..`;
   - any of these reached through braces or a character class (`{a,}/home/…`, `.{.,.}/.ssh/…`, `[.][.]/…`).

   Test with the patterns from P1 and the M1-07 round-2 review.
3. **K1–K4, M1-13's round-8 blockers 1–4.** Fix each by its invariant, not by the review's example lines alone, and add the review's evidence lines as tests. Rounds 3–8 show that fixing only the named inputs brings neighbouring findings in the next review.
   - **K1:** `runs_here` (`eval`, `source`, `.`), `env_payloads` and `<(...)` process substitutions must inspect every reading (`other_readings`) and the substituted words, not only the main reading.
   - **K2:** `reaches_tree_through_links` must handle multi-hop symlinks, treat `expand_glob`'s `None` as unknown (not as "no reach"), include `atrium_*_paths`, and handle abbreviated long flags.
   - **K3:** `shell_c_code` must skip the values of `-o`/`+o`/`-O`/`+O`, and `long_name` must accept GNU's short unambiguous prefixes (`script --c`, `flock --c`).
   - **K4:** `find_exec_units` must extract `payloads_of`, `env_payloads` and `code_subs` from `-exec` commands.
4. **Compare with `dev` as you go.** For every new test line, record `dev`'s verdict next to yours under Home, Any and FullAccess. The probe technique from M1-13 rounds 7–8 works: a test file using the `common_adversarial` helpers, dropped into a detached `dev` worktree with its own `CARGO_TARGET_DIR`.
   - **No line may become more permissive than `dev`.**
   - A new hard deny is allowed only where `dev` also hard-denies, or where the line truly reads a hard-denied path. Never on an ordinary command: a false hard deny can't be overridden, and that was round 8's major 5.
5. **Leave alone:** K5, and the other ignored findings (4, 5, 10, 11). If one falls out of your fix for free, un-ignore its test and say so.

## Definition of Done
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast`.
- [ ] `scripts/check-protocol-ts`, if the protocol changed.
- [ ] The new and un-ignored tests pass 20 times in a row (`cargo test -p atrium-core`).
- [ ] Each new test fails on `dev` where `dev` is wrong, and the handoff says which.
- [ ] Handoff:
  - each item (CR-12, P1, K1–K4): the invariant, the files, the tests;
  - the comparison table against `dev`;
  - what you left and why.

## Rules
- Work only in your worktree, and commit on your branch only. Never push, merge or rebase.
- No real home paths in any file: tests use `Tmp` homes and the `/h/u` constant.
- Never run a real provider.

## Escalate if
- the contract's CR-12 text can't be implemented as written;
- a fix would need a hard deny on a line that `dev` allows or asks about;
- K1–K4 can't be closed without a shell-grammar rewrite. If so, say how far a fail-closed grammar for the affected constructs would go, and stop.
