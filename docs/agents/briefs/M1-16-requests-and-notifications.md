# M1-16-requests-and-notifications: decline a question, the fake agent's requests, per-kind notifications

Model: Gemini 4 Hydrogen (`gemini-4-hydrogen-high`, `scripts/agent-run agy`; D-076) · Reviewer: Sonnet 5.5 (read-only) · Depends on: M1-07, M1-09b (merged into `dev`)
Go: D-083 (Pedro, 2026-11-14) · Branch: `agent/hydrogen/M1-16-requests-and-notifications` · Worktree: `../atrium-wt/M1-16-requests-and-notifications`

## Goal
Implement CR-13, CR-14 and CR-15 as decided in D-082 and written into contract revision 3.1 (`docs/contracts/m1.md`, the paragraphs marked "(rev 3.1)" in §6.2 and §8). CR-09 and CR-12 are not yours (M1-15 and a policy task).

## Context
- `docs/agents/change-requests.md` CR-13, CR-14, CR-15; `docs/agents/decisions.md` D-082, D-083.
- Contract §5 (approvals, tickets, the ledger), §6.2, §8, §10 (incompatible versions).
- `crates/atrium-protocol/src/{wire.rs,agent.rs,lib.rs}`; `crates/atrium-daemon/src/{fake.rs,sessions.rs,commands.rs,hub.rs}`; `crates/atrium-agents/src/claude/{adapter.rs,normalizer.rs}`, `src/fixture.rs`; `app/src/` (the question card, the bridge's commands, `ui/Settings.tsx`).
- How the Claude CLI's question tool (`AskUserQuestion`) is answered today: `normalizer.rs` around `SessionCommand::AnswerQuestion`.

## Owned paths
- `crates/atrium-protocol/src/**` and `crates/atrium-protocol/tests/**` (the new fields and command, `PROTOCOL_VERSION` = 2, serde shape tests).
- `bindings/protocol.ts`, regenerated only (`scripts/check-protocol-ts` must pass).
- `crates/atrium-daemon/src/{fake.rs,sessions.rs,commands.rs,hub.rs,lib.rs}` and new `crates/atrium-daemon/tests/requests_*.rs`.
- `crates/atrium-agents/src/claude/**`, `crates/atrium-agents/src/fixture.rs`, `crates/atrium-agents/tests/claude_decline*.rs`, and any other adapter file that must handle the new `SessionCommand` variant to compile.
- `app/src/**` (question card, bridge command, Settings note) and its vitest tests.
- `docs/agents/handoffs/M1-16-requests-and-notifications.md`
- Not yours: `crates/atrium-core/src/policy/**` (M1-13 is changing it), `docs/contracts/**` (the orchestrator wrote rev 3.1), `app/src-tauri/**` except a new bridge command if the UI needs one.

## What to do
1. **CR-15, decline a question.**
   - Wire: `DaemonCommand::DeclineQuestion { thread_id, incarnation, request_id }`, App-only like `AnswerQuestion`, checked by the ledger the same way (stale, wrong incarnation, already resolved, not a question: the same `ErrorCode`s), resolving the request as `RequestOutcome::Cancelled`.
   - Session: `SessionCommand::DeclineQuestion { request_id }`.
   - Claude adapter: answer the CLI's pending `AskUserQuestion` permission request so the CLI treats the question as dismissed and the turn goes on (read the normalizer to see how answers are sent; a deny with a short "the user declined to answer" message is the expected form; check against the recorded fixtures' shapes). Emit `RequestResolved` as cancelled.
   - Any adapter that can't dismiss: interrupt the turn and say so in `RequestResolved`'s detail (contract §6.2 rev 3.1).
   - App: the question card gets a "Decline" button beside "Stop the turn"; the bridge sends the command; vitest for the card's states.
2. **CR-13, the fake agent's requests.**
   - `AgentLaunch::Fake` gains `approval_at_ms: Option<u64>` and `question_at_ms: Option<u64>`, both `#[serde(default)]`, so existing JSON and fixtures parse unchanged.
   - Give the fake session a real `SessionGate` (as a native session has), and implement exactly what contract §6.2 rev 3.1 says: the `echo synthetic` approval (Allow redeems → `Succeeded`, Deny → `Denied`, interrupt or turn end → withdrawn) and the question (answer or decline, then streaming goes on).
   - Daemon tests over a real socket (`tests/requests_*.rs`, using `tests/common/`): approve, deny, "always" creating a rule and the next turn's call not asking, a stale approval refused with `Expired`, a question answered, a question declined, an interrupt withdrawing both, and the thread's state going `NeedsApproval`/`NeedsInput` and back.
3. **CR-14, per-kind notifications.** `notify_approval`, `notify_question`, `notify_failed` in `atrium-daemon::commands` next to `KEEP_RUNNING_SETTING`, validated as booleans; the hub reads them at start and on `SetSetting` and posts nothing for a kind that is off. Tests with a recording `Notifier`. Remove the "doesn't read these switches yet" notes in `app/src/ui/Settings.tsx`.
4. `PROTOCOL_VERSION` = 2; regenerate `bindings/protocol.ts`; check the app's version handling still shows `Incompatible` for a mismatch (existing tests).

## Definition of Done
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast`.
- [ ] `scripts/check-protocol-ts`; in `app/`: `pnpm typecheck`, `pnpm test`, `pnpm build`.
- [ ] Each new daemon test binary passes 20 times in a row.
- [ ] Handoff: each CR, the files, the tests, how the Claude adapter dismisses a question (and whether it was checked against a recorded fixture or only by reading), the outputs.

## Rules
Work only in your worktree. Tests use temp folders and the fake agent; never run a real provider. No real home paths in any file. Commit on your branch only; never push, merge or rebase. Stop every daemon a test starts. Shell substitutions, `~` and `../` are refused by the worker policy.

## Escalate if
The contract's rev 3.1 text can't be implemented as written (say what and why, don't change the contract); the fake session can't get a `SessionGate` without changes to `crates/atrium-core`; the Claude CLI's dismissal can't be expressed through its permission protocol.
