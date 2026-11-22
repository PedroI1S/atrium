# M1-33-nonpolicy-fixes: M1-28's non-policy work, on its own

Extraction: the orchestrator (D-136), no writer · Reviewer: Codex GPT-6.1 Sol `high`, read-only (D-124) · Depends on: nothing open
Go: D-136 · Branch: `agent/orchestrator/M1-33-nonpolicy-fixes` · Worktree: `../atrium-wt/M1-33-nonpolicy-fixes`

## Goal
Land the parts of M1-28-ansi-c-words that don't depend on the policy engine, so they don't wait for rev 4 (`docs/contracts/m1-policy-rev4.md` §12–§13).

## What it takes from `agent/hydrogen/M1-28-ansi-c-words` (tip `5c3b62b`)
The branch's versions of these paths, as whole files (none of them holds policy work):
- `app/src/chat/markdown.ts`, `markdown.test.ts`: the quadratic parser fixes.
- `app/src/daemon/client.ts`, `client.test.ts`: batch-error logging (M1-24).
- `crates/atrium-agents/src/claude/adapter.rs`, `crates/atrium-agents/src/codex/adapter.rs`, `crates/atrium-agents/tests/M1-28-followups.rs`: the orphan withdrawal (D-122, CR-18), the Codex question saturation race, the bounded Claude shutdown wait (M1-26).
- `crates/atrium-daemon/src/{conn,hub,lib,main,server}.rs`, `crates/atrium-daemon/tests/{M1-28-followups,adversarial_socket,binary_adapters}.rs`: N10 (the 8 MiB lane), N12 (socket races, `go_live`/`ReplayDone`), fish and login-shell PATH discovery (M1-22).
- The M1-25 export test, moved out of `crates/atrium-core/tests/M1-28-ansi-c-words.rs` into its own file `crates/atrium-core/tests/M1-33-export.rs`.

Not taken: everything under `crates/atrium-core/src/policy/`, the ANSI-C and `cd` tests, `adversarial_fuzz.rs`, `adversarial_policy.rs`.

## Definition of Done (the orchestrator's verification, D-123)
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast`, against `dev`'s policy.
- [ ] `pnpm -C app typecheck` and `pnpm -C app test`.
- [ ] `git diff dev --stat` touches only the paths above.
- [ ] Codex review: approve with no findings (D-120). The review checks the approval lifecycle path too (the adapters' tests use a real `SessionGate`).
