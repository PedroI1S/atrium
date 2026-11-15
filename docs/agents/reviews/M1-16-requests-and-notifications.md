# Review: M1-16-requests-and-notifications (commit 99e227e)

Reviewer: Claude Sonnet 5.5, read-only. Author: Gemini 4 Hydrogen `high`. Everything ran in a scratch clone of 99e227e; the worktree was not touched. Base of the branch is 0a3ce58, not the current `dev` tip; a trial merge of `origin/dev` into the branch is clean.

## Verdict: approve with changes

CR-13, CR-14 and CR-15 are implemented as rev 3.1 describes, the toolchain is green, and the new test binaries are stable (20 of 20). The tests catch 7 of 9 mutations. Two things should be fixed before merge: the fake approval uses `touch synthetic` where the contract says `echo synthetic`, and no test pins that an allowed fake approval is actually redeemed. Nothing else blocks.

## 1. Toolchain (clone of 99e227e)
- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace --no-fail-fast`: all pass, 0 failed. Daemon: requests_fake 5, requests_notifications 2, sessions 19, socket 9, gaps_snapshot 2, recovery 3. Agents: claude_adapter 38, claude_normalizer 61. Protocol serde_shape 13.
- `scripts/check-protocol-ts`: "up to date".
- App: `pnpm install --frozen-lockfile` ok, `pnpm typecheck` ok, `pnpm test` 14 files / 264 tests pass, `pnpm build` exit 0.
- The worker did not run `scripts/check-protocol-ts` or the 20-run loops (policy hook). I did both.

## 2. protocol.ts is the file the app uses
- `scripts/check-protocol-ts` diffs `crates/atrium-protocol/bindings/protocol.ts` against a fresh export.
- The app imports it by relative path: `app/src/daemon/types.ts` and others import `../../../crates/atrium-protocol/bindings/protocol` (no tsconfig path alias). So the checked file is the one the app compiles against.
- `app/src/chat/threads.ts:206` sends `{ type: "decline_question", ... }`, which type-checks against the regenerated `DaemonCommand`.

## 3. Stability: 20 runs each
- `requests_fake`: 20/20.
- `requests_notifications`: 20/20.
- No daemon was left running (the tests stop their own rig).

## 4. Contract conformance, rev 3.1
- **DeclineQuestion wire:** OK.
  - `DaemonCommand::authority()` returns `HumanControl` for every command (wire.rs:316), so an Agent role is refused like `AnswerQuestion`.
  - A direct `ThreadCommand { DeclineQuestion }` is refused as `Invalid` (commands.rs:549-555), tested.
  - Handler (commands.rs:264-301): session lock, current-incarnation check, "has a binding so it is an approval" check, `ledger.withdraw`, then `SessionCommand::DeclineQuestion`.
  - Wrong incarnation, already answered and an approval id all return `Expired`, same as `AnswerQuestion` (`ledger_failure` maps all of them to `Expired`). All three are in `fake_question_answered_declined_and_checked_by_ledger`.
  - The outcome is `RequestOutcome::Cancelled`, asserted.
- **Fake agent fields:** OK. `approval_at_ms` and `question_at_ms` are `#[serde(default)] Option<u64>` (wire.rs:362-367), with serde_shape tests for both present and absent.
- **Real `SessionGate` in the fake session:** OK. `Inner::session_gate` is shared by native and fake (sessions.rs:224-250, 596-598).
- **`echo synthetic` approval:** deviates, see finding 1. The behaviour is otherwise right:
  - Allow: `RequestResolved Approved`, `gate.redeem`, `ToolCallFinished Succeeded`.
  - Deny: `Denied`, with the message in `error`.
  - Interrupt or stop: `RequestResolved Cancelled` plus `ToolCallFinished Cancelled`. Turn end can't be reached while waiting, because the wait loop only exits on a decision or an interrupt/stop. (The turn's pause time is excluded from `elapsed`, so the turn does not run out while waiting.)
  - Ask path emits `ToolCallStarted` then `ApprovalRequested` with the ticket id and `rememberable`.
- **Question:** OK. One question, two options, `allow_text: true`. Answer gives `Answered`, decline gives `Cancelled`, interrupt gives `Cancelled`. Streaming continues afterwards; the declined path asserts `TurnCompleted { turn: 2, Completed }`.
- **Thread state:** `NeedsApproval` and `NeedsInput` and back to `Idle` are asserted, with `pending_approvals` and `pending_questions` going 1 to 0.
- **"Always":** `remember: Some(Home)` creates one rule (`ListRules` length 1). Turn two has no new `approval_requested` and two finished tool calls.
- **Stale approval:** wrong incarnation and a resolve after interrupt both return `Expired`.
- **CR-14:**
  - Keys and constants are in commands.rs:44-54, exported from lib.rs.
  - `set_setting` validates booleans (commands.rs:677-683); `"yes"`, `1` and `null` are `Invalid` for all three keys, tested.
  - The hub loads them at start (lib.rs:359-364), updates on `SetSetting`, and filters before `should_notify` (hub.rs:218).
  - Absent means on (`Default` is all true).
  - The test covers default-on, each kind off in turn, and persistence across a daemon restart.
- **`PROTOCOL_VERSION = 2`** (lib.rs:22). The app's `Incompatible` handling is unchanged: the client compares `PROTOCOL_VERSION` to the daemon's welcome (`connection.rs:234`), and the existing socket and delivery tests pass.

## 5. Claude adapter decline
- **What it does:** `Actor::on_command` (adapter.rs:1499-1509) removes the entry from `self.questions`, feeds the normalizer, and calls `deny_can_use(&native, "The user declined to answer.")`. That sends a `control_response` with `{"behavior":"deny","message":...}` on the pending `can_use_tool` request id.
- **Normalizer** (normalizer.rs:1176-1186): for `Pending::Question` it drops the `can_use` entry and emits `RequestResolved { Cancelled }`. For anything else it emits the non-fatal unknown-request error.
- **Compared to the recordings:**
  - `question.jsonl` line 40 shows the CLI asking `AskUserQuestion` as a `control_request` with `subtype: can_use_tool`. The adapter answers it as in line 43 (`control_response`, `subtype: success`, `behavior` in the inner response).
  - `approval-deny.jsonl` line 26 shows the same envelope with `"behavior":"deny","message":"..."`. So the shape and the message field match the recorded permission protocol.
  - The existing code already denies an `AskUserQuestion` this way when no ticket can be taken (adapter.rs:1341).
- **Gap:** there is no recording of a deny sent to `AskUserQuestion` specifically, so "the CLI treats it as dismissed and the turn goes on" is by analogy with the Bash deny recording, not recorded. The handoff says "checked against recorded fixtures"; that is true for the shape only. See finding 5.
- **Other adapters:** only the Claude adapter exists in `atrium-agents/src`, so no "interrupt and say so" fallback was needed. `RequestResolved` has no detail field (agent.rs:186-189), so the contract's "say so in `RequestResolved`'s detail" cannot be done as written. See finding 4.
- **Test:** `questions_use_the_gates_question_ticket` sends the decline, expects `RequestResolved Cancelled`, and reads the written response file for `behavior == "deny"` and the message. Mutation M9 shows it catches a missing deny.

## 6. Owned paths
- Inside owned paths: protocol src/tests/bindings, daemon `src/{fake,sessions,commands,hub,lib}.rs`, new `tests/requests_*.rs`, `atrium-agents/src/claude/**` and `fixture.rs`, `app/src/**`, the handoff doc.
- Outside, all mechanical and **needed to compile** (they build `AgentLaunch::Fake` literals that now have two more fields): `examples/perf.rs` (+2 lines), `tests/gaps_snapshot.rs` (+2), `tests/sessions.rs` (+6). Each is only `approval_at_ms: None, question_at_ms: None`. `tests/common/` was not changed.
- Brief names `tests/claude_decline*.rs` for agents tests. The worker added to the existing `claude_adapter.rs` and `claude_normalizer.rs` instead. No conflict, but it is a small departure from the named pattern.
- Not touched: `atrium-core/src/policy/**`, `docs/contracts/**`, `app/src-tauri/**` (the app sends the command through the existing generic command path, so no bridge change was needed).

## 7. App
- `QuestionCard` shows "Answer", "Decline", "Stop the turn" in that order (RequestCard.tsx:169-171, asserted in the vitest). "Decline" is a ghost button with a title that explains it.
- `declineQuestion` (threads.ts:206) sends `decline_question` with thread, incarnation and request id.
- `cardState` shows "Declined" only for a locally declined cancelled request; a stop or another client's cancel keeps the old texts. States "Declining…" and "Declined. Waiting for the agent." are in `StateLine`.
- vitest (3 tests): state table for sending, sent, resolved here, resolved by stop, resolved elsewhere and stale; render and click send the right payload; retry after a non-expired refusal keeps all three buttons.
- Settings: both "doesn't read these switches yet" notes are gone (Settings.tsx:62-63, 279).
- Hand-check not done: I did not open the UI. The vitest renders the real component in happy-dom.

## Mutation results (3 runs each, in the clone)

| Mutation | Test binary | Result |
|---|---|---|
| Skip the per-kind switch check in `Hub::thread_changed` (hub.rs:218) | requests_notifications | caught, `notify_switches_gate_each_reason_and_persist_across_restart` (3/3) |
| Don't read notify settings at start (lib.rs:362) | requests_notifications | caught, same test (3/3) |
| DeclineQuestion: no binding check, so an approval can be declined (commands.rs:286-293) | requests_fake | caught, `fake_question_answered_declined_and_checked_by_ledger` (3/3) |
| DeclineQuestion: skip `ledger.withdraw` | requests_fake | caught, same test (3/3) |
| Fake resolves a decline as `Answered` instead of `Cancelled` | requests_fake | caught, same test (3/3) |
| Fake doesn't withdraw its approval on interrupt | requests_fake | caught, `fake_interrupt_withdraws_...` and `fake_stale_approval_...` (3/3) |
| Claude adapter doesn't send the deny on decline | claude_adapter | caught, `questions_use_the_gates_question_ticket` (3/3) |
| DeclineQuestion: skip the explicit incarnation check (commands.rs:282) | requests_fake | **NOT caught**, but equivalent: the ledger key includes the incarnation, so `withdraw` finds nothing and still returns `Expired` |
| Fake skips `gate.redeem` on Allow (fake.rs:494) | requests_fake | **NOT caught** (3/3 pass) |

## Findings
- **[moderate] crates/atrium-daemon/src/fake.rs:375: the approval runs `touch synthetic`, the contract says `echo synthetic`.** Rev 3.1 §6.2 says `Bash` `{"command": "echo synthetic"}`. The periodic tool call in the same file uses `echo synthetic` (fake.rs:300-301), and the handoff states `touch` without saying it is a deviation. It is not needed: with the line changed to `echo synthetic`, all 5 `requests_fake` tests pass here. Change it back, or ask the driver if `touch` was meant (then change the contract).
- **[moderate] crates/atrium-daemon/tests/requests_fake.rs: nothing pins that Allow redeems the ticket.** Mutation "skip `gate.redeem`" survives. The contract says "Allow redeems and finishes the call Succeeded". Add an assertion that the redeem happened, for example that a second `ResolveApproval` for the same id is `Expired` after the thread is idle, or a store check that the row is `redeemed`.
- **[minor] crates/atrium-daemon/tests/requests_fake.rs:197: `sleep(80ms)` then waiting for `Idle`.** After `say(...)` the thread may still read `Idle` from turn one, so the wait can return before turn two runs. It passed 20/20 because the sleep is longer than the 50 ms turn. Wait for a `TurnCompleted { turn: 2 }` in the history instead.
- **[minor] crates/atrium-daemon/tests/requests_fake.rs: untested edges.** No test for declining an unknown request id, and the answered path doesn't assert that streaming continues after the answer (only the decline path asserts `TurnCompleted`). The code handles both (`withdraw` returns refused for a missing row; the loop continues).
- **[minor] The contract's "RequestResolved's detail" does not exist.** §6.2 rev 3.1 says an adapter that can't dismiss "says so in `RequestResolved`'s detail", but `RequestResolved` is `{ request_id, outcome }` only (agent.rs:186-189). Only the Claude adapter exists, so nothing breaks now. Carry it to the M1-08 (Codex) and later adapter briefs, or add a field in a later CR.
- **[minor] crates/atrium-agents/src/claude/adapter.rs:1499-1509: the CLI's reaction to a denied `AskUserQuestion` is not recorded.** The shape matches `approval-deny.jsonl`, but no fixture shows the turn continuing after a denied question. Re-record `question` with a decline at the next live recording pass, or accept the analogy.
- **[nit] crates/atrium-daemon/src/commands.rs:282-285: `Ok(Some(_))` returns `StaleIncarnation` before `withdraw`.** Harmless and clearer, but redundant (see mutation table). Same for `session_of(..., Some(incarnation))`.
- **[nit] crates/atrium-daemon/src/fake.rs:538: `question_ticket` returning `None` skips the question silently.** Fine for a fake; a comment would help.
- **[nit] Handoff:** the Definition of Done items `scripts/check-protocol-ts` and "20 times in a row" are reported as blocked, and the handoff lists `touch synthetic`'s command nowhere. Both checks pass when run (sections 1 and 3).

## Asked before merge
1. Change `touch synthetic` to `echo synthetic` in `fake.rs:375` (finding 1), or tell the driver which is intended.
2. Add one assertion that the allowed fake approval is redeemed (finding 2).
3. Replace the `sleep(80ms)` wait with a wait on `TurnCompleted { turn: 2 }` (finding 3).
4. Orchestrator: record the "RequestResolved has no detail" gap for the next adapter briefs, and decide whether a recorded decline fixture is wanted before M1 sign-off.
