# M1-16 Requests and Notifications — Handoff (Gemini 4 Hydrogen)

Branch: `agent/hydrogen/M1-16-requests-and-notifications` (branched from `0a3ce58`)
Protocol version: `PROTOCOL_VERSION = 2`

## Commits

- `5fca750` — `CR-15: allow declining questions without stopping the turn`
- `0657079` — `CR-13: wire synthetic approvals and questions through SessionGate`
- `329e934` — `CR-14: enforce per-kind notification settings in hub`
- `08677f0` — `tests: drop duplicate dead_code allow on common module`

## Summary of Changes

### 1. CR-15 — Decline a Question Without Stopping the Turn

- **Protocol (`atrium-protocol`)**:
  - Bumped `PROTOCOL_VERSION` from `1` to `2` in `crates/atrium-protocol/src/lib.rs`.
  - Added `DaemonCommand::DeclineQuestion { thread_id, incarnation, request_id }` in `crates/atrium-protocol/src/wire.rs` (HumanControl/App authority; ledger-checked like `AnswerQuestion`, resolving as `RequestOutcome::Cancelled`).
  - Added `SessionCommand::DeclineQuestion { request_id }` in `crates/atrium-protocol/src/agent.rs`.
  - Regenerated `crates/atrium-protocol/bindings/protocol.ts` and added round-trip cases in `crates/atrium-protocol/tests/serde_shape.rs`.
- **Agents (`atrium-agents`)**:
  - Updated `Sanitizer::command` in `crates/atrium-agents/src/fixture.rs` to pass through `SessionCommand::DeclineQuestion`.
  - Updated `Normalizer::command` in `crates/atrium-agents/src/claude/normalizer.rs` to match `Pending::Question { native }`, remove the `can_use` entry, and emit `AgentEvent::RequestResolved { request_id, outcome: RequestOutcome::Cancelled }` (or non-fatal unknown-request error otherwise).
  - Updated `Claude` adapter `on_command` in `crates/atrium-agents/src/claude/adapter.rs` so `SessionCommand::DeclineQuestion` feeds the normalizer and dismisses the pending `AskUserQuestion` control request.
- **How the Claude adapter dismisses a question**:
  - Checked against recorded fixtures (`fixtures/agents/claude/question.jsonl` and `fixtures/agents/claude/approval-deny.jsonl`, since live provider runs are disallowed): `AskUserQuestion` arrives as a `control_request` with `subtype: "can_use_tool"`, and responding with `{"behavior": "deny", "message": ...}` causes Claude CLI to return an error `tool_result` for that tool call and continue its turn.
  - When `SessionCommand::DeclineQuestion { request_id }` arrives in `Claude::on_command`, the adapter removes `(native, _input)` from `self.questions`, feeds `AdapterInput::Command` into the normalizer (emitting `RequestResolved { request_id, outcome: RequestOutcome::Cancelled }`), and calls `self.deny_can_use(&native, "The user declined to answer.")`, sending `{"behavior": "deny", "message": "The user declined to answer."}` on the `can_use_tool` response channel.
- **Daemon (`atrium-daemon`)**:
  - Added `DaemonCommand::DeclineQuestion` handling in `crates/atrium-daemon/src/commands.rs`: locks `live.requests`, validates current incarnation (`current_incarnation == Some(incarnation)`, otherwise `ErrorCode::Expired`), rejects approvals (`requests.binding(&key).is_some()` -> `ErrorCode::Expired`), transitions the question row via `inner.ledger.withdraw(&key)` (`Cancelled`), and forwards `SessionCommand::DeclineQuestion { request_id }` to the live session.
  - Rejected direct `DaemonCommand::ThreadCommand` with `SessionCommand::DeclineQuestion` (`ErrorCode::Invalid`).
- **App (`app/src/`)**:
  - Added `"decline"` action in `app/src/chat/requests.ts`, showing `"Declined"` when resolved locally via `"decline"`.
  - Added `declineQuestion` in `app/src/chat/threads.ts` sending `{ type: "decline_question", thread_id, incarnation, request_id }`.
  - Added a `"Decline"` button beside `"Stop the turn"` and `"Declining…"` / `"Declined. Waiting for the agent."` status text in `app/src/ui/RequestCard.tsx`.

### 2. CR-13 — Synthetic Agent Requests via `SessionGate`

- **Protocol (`atrium-protocol`)**:
  - Added `#[serde(default)] approval_at_ms: Option<u64>` and `#[serde(default)] question_at_ms: Option<u64>` to `AgentLaunch::Fake` in `crates/atrium-protocol/src/wire.rs` and regenerated `crates/atrium-protocol/bindings/protocol.ts`.
  - Updated `crates/atrium-protocol/tests/serde_shape.rs` with both explicit-field round-trips and omitted-field default deserialization.
- **Daemon (`atrium-daemon`)**:
  - Extracted `Inner::session_gate` in `crates/atrium-daemon/src/sessions.rs` so `Agent::Fake` and `Agent::Native` share the real `SessionGate` construction.
  - Updated `crates/atrium-daemon/src/commands.rs`, `crates/atrium-daemon/src/fake.rs`, `crates/atrium-daemon/examples/perf.rs`, `crates/atrium-daemon/tests/sessions.rs`, and `crates/atrium-daemon/tests/gaps_snapshot.rs` for the new `fake::Params` and `AgentLaunch::Fake` fields.
  - Implemented per-turn synthetic requests in `crates/atrium-daemon/src/fake.rs`:
    - At `approval_at_ms` of each turn, runs a rememberable `Bash` check (`{"command": "touch synthetic"}`) through `SessionGate::check`, emits `ToolCallStarted` + `ApprovalRequested` (when `Ask`), and blocks until `ResolveApproval` (`Allow` emits `RequestResolved { Approved }`, calls `gate.redeem`, and emits `ToolCallFinished { Succeeded }`; `Deny` skips redeem and emits `RequestResolved { Denied }` + `ToolCallFinished { Denied }`; `Interrupt`/`Shutdown` emits `RequestResolved { Cancelled }` + `ToolCallFinished { Cancelled }`). Once remembered by a rule, subsequent turns receive `GateDecision::Allow` and run the tool call without asking.
    - At `question_at_ms` of each turn, obtains a ticket via `SessionGate::question_ticket(Some(turn))`, emits `QuestionAsked` (1 question, 2 options, `allow_text: true`), and blocks until `AnswerQuestion` (`RequestResolved { Answered }`) or `DeclineQuestion` (`RequestResolved { Cancelled }`), continuing the turn afterwards, or withdraws on `Interrupt`/`Shutdown` (`RequestResolved { Cancelled }`).

### 3. CR-14 — Per-Kind Notification Settings

- **Daemon (`atrium-daemon`)**:
  - Exported `NOTIFY_APPROVAL_SETTING` (`"notify_approval"`), `NOTIFY_QUESTION_SETTING` (`"notify_question"`), and `NOTIFY_FAILED_SETTING` (`"notify_failed"`) in `crates/atrium-daemon/src/commands.rs` and `crates/atrium-daemon/src/lib.rs`.
  - Validated those keys as booleans in `commands::set_setting` (`ErrorCode::Invalid` otherwise) and updated `Hub` live when changed.
  - Updated `Hub` in `crates/atrium-daemon/src/hub.rs` with `NotifySwitches` (defaulting to `true` for all three) and `load_notify_settings` / `set_notify_setting`, filtering notifications in `Hub::thread_changed` when a reason's switch is `false`.
  - Loaded persisted notification settings into `Hub` in `Daemon::start` (`crates/atrium-daemon/src/lib.rs`) right after recovery.
- **App (`app/src/ui/Settings.tsx`)**:
  - Removed the stale notes saying the daemon does not read the notification switches yet.

## Files Added / Modified

- `crates/atrium-protocol/src/lib.rs`
- `crates/atrium-protocol/src/wire.rs`
- `crates/atrium-protocol/src/agent.rs`
- `crates/atrium-protocol/bindings/protocol.ts`
- `crates/atrium-protocol/tests/serde_shape.rs`
- `crates/atrium-agents/src/fixture.rs`
- `crates/atrium-agents/src/claude/normalizer.rs`
- `crates/atrium-agents/src/claude/adapter.rs`
- `crates/atrium-agents/tests/claude_normalizer.rs`
- `crates/atrium-agents/tests/claude_adapter.rs`
- `crates/atrium-daemon/src/lib.rs`
- `crates/atrium-daemon/src/commands.rs`
- `crates/atrium-daemon/src/hub.rs`
- `crates/atrium-daemon/src/sessions.rs`
- `crates/atrium-daemon/src/fake.rs`
- `crates/atrium-daemon/examples/perf.rs`
- `crates/atrium-daemon/tests/sessions.rs`
- `crates/atrium-daemon/tests/gaps_snapshot.rs`
- `crates/atrium-daemon/tests/requests_fake.rs` (new)
- `crates/atrium-daemon/tests/requests_notifications.rs` (new)
- `app/src/chat/requests.ts`
- `app/src/chat/threads.ts`
- `app/src/ui/RequestCard.tsx`
- `app/src/ui/Settings.tsx`
- `app/src/requests.test.ts` (new)

## Tests Added

- `crates/atrium-agents/tests/claude_normalizer.rs`: `a_question_can_be_declined_without_ending_the_turn`
- `crates/atrium-agents/tests/claude_adapter.rs`: `questions_use_the_gates_question_ticket` expanded to exercise `SessionCommand::DeclineQuestion`, `RequestResolved { outcome: Cancelled }`, and the `"deny"` control response payload
- `app/src/requests.test.ts`: 3 vitest (`happy-dom`) tests covering `cardState` transitions for declining/cancelled questions, `QuestionCard` rendering `"Decline"` beside `"Stop the turn"`, sending `decline_question`, `StateLine` states, and retrying after a non-expired refusal
- `crates/atrium-daemon/tests/requests_fake.rs`: 5 daemon socket tests (`fake_approval_allow_and_deny`, `fake_approval_always_creates_rule_and_skips_next_turn`, `fake_stale_approval_is_refused_with_expired`, `fake_question_answered_declined_and_checked_by_ledger`, `fake_interrupt_withdraws_pending_approval_and_question`)
- `crates/atrium-daemon/tests/requests_notifications.rs`: 2 daemon socket tests (`notify_settings_require_booleans`, `notify_switches_gate_each_reason_and_persist_across_restart`)

## Definition of Done Outputs & Notes

- `cargo fmt --all --check`: exited 0
- `cargo clippy --workspace --all-targets -- -D warnings`: exited 0 (`Finished dev profile ... in 1.12s`)
- `cargo test --workspace --no-fail-fast`: exited 0; all workspace test targets and doc-tests passed across `atrium-agents`, `atrium-app`, `atrium-client`, `atrium-core`, `atrium-daemon`, `atrium-host`, and `atrium-protocol` (0 failed, 0 ignored)
- Protocol TS check:
  - Note: in the initial round, `scripts/check-protocol-ts` was blocked by the worker allowlist hook, and `export-ts` + `git diff --exit-code` passed with no diff. The Sonnet reviewer subsequently ran `scripts/check-protocol-ts` directly on commit `99e227e` and confirmed it passes ("up to date").
- `pnpm --dir app typecheck`: exited 0 (`$ tsc --noEmit`)
- `pnpm --dir app test`: exited 0 (`Test Files 14 passed (14)`, `Tests 264 passed (264)`)
- `pnpm --dir app build`: exited 0 (`✓ 2307 modules transformed`, built in 6.12s)
- New daemon test binaries:
  - Note: in the initial round, shell loops (`repeat`, `for`, `&&`) were blocked by the worker policy hook, so each binary was run once by the worker (`requests_fake`: 5 passed; `requests_notifications`: 2 passed). The Sonnet reviewer subsequently ran both 20 times in a row on commit `99e227e` and confirmed both pass (`requests_fake`: 20/20; `requests_notifications`: 20/20).

## Fix round 1 (D-089)

### Commits

- `6490c99` — `fake: use echo synthetic for approval command per contract`
- `400db41` — `tests(requests_fake): verify Allow redeems ticket through gate`
- `f3de67e` — `tests(requests_fake): wait for TurnCompleted instead of sleeping`
- `8955f5b` — `tests(requests_fake): cover unknown decline id and post-answer streaming`
- `54c39a6` — `fake: log when question_ticket returns None instead of skipping silently`

### Changes per Item

1. **Fake approval command (`6490c99`)**: Updated `crates/atrium-daemon/src/fake.rs` (`run_approval`) to use `"echo synthetic"` instead of `"touch synthetic"`, matching contract §6.2 (rev 3.1).
2. **Gate redeem verification (`400db41`)**: Added `fake_approval_allow_redeems_through_gate` in `crates/atrium-daemon/tests/requests_fake.rs`. While the synthetic approval is pending (`NeedsApproval`), the test creates a matching `RuleDecision::Deny` rule for `Bash` `["echo", "synthetic"]` via `DaemonCommand::CreateRule`, then sends `ResolveApproval { decision: Allow }`. It asserts `RequestResolved { outcome: Approved }` and `ToolCallFinished { status: ToolStatus::Denied }` (with no `ToolCallFinished { status: Succeeded }`). Verified by temporarily skipping `self.gate.redeem(&ticket, check).await` in `crates/atrium-daemon/src/fake.rs`, running `cargo test -p atrium-daemon --test requests_fake` (observed `fake_approval_allow_redeems_through_gate` fail: 5 passed, 1 failed), reverting `fake.rs`, and re-running (6 passed, 0 failed).
3. **Remove fixed sleep in `requests_fake.rs` (`f3de67e`)**: Replaced `tokio::time::sleep(Duration::from_millis(80))` in `fake_approval_always_creates_rule_and_skips_next_turn` with `wait_turn_completed(&mut wire, &thread, 2)`, which subscribes to the thread and waits for `AgentEvent::TurnCompleted { turn: 2, .. }` before checking `ThreadState::Idle`.
4. **Untested edges in `requests_fake.rs` (`8955f5b`)**: Extended `fake_question_answered_declined_and_checked_by_ledger` to cover:
   - declining an unknown `request_id` (`DaemonCommand::DeclineQuestion`) returns `ErrorCode::Expired`, matching `DaemonCommand::AnswerQuestion` on the same unknown `request_id`;
   - after turn 1's question is answered (`RequestResolved { outcome: Answered }`), streaming continues (`MessageDelta`, `MessageCompleted` for `MessageKind::Assistant`, and `TurnCompleted { turn: 1, outcome: Completed }`).
5. **Handling `question_ticket` returning `None` (`54c39a6`)**: In `crates/atrium-daemon/src/fake.rs` (`run_question`), when `self.gate.question_ticket(Some(turn)).await` returns `None`, chose to emit no `AgentEvent` (matching `ToolGate::question_ticket` in `crates/atrium-agents/src/lib.rs` and the native Claude normalizer in `crates/atrium-agents/src/claude/normalizer.rs`, which decline to the provider and emit nothing to the event stream) and log the event via `eprintln!("atriumd: fake agent turn {turn}: gate returned no question ticket; question skipped")` (following `atrium-daemon`'s stderr logging convention, since there is no provider process to send a deny response to).
6. **Handoff & DoD note corrections**: Corrected the earlier notes above regarding `scripts/check-protocol-ts` and the 20-run stability loops (both verified passing by the Sonnet reviewer on commit `99e227e`). Left the explicit incarnation check in `crates/atrium-daemon/src/commands.rs` unchanged and did not modify `docs/contracts/**`.

### Fix Round 1 Verification

- `cargo fmt --all --check`: exited 0
- `cargo clippy --workspace --all-targets -- -D warnings`: exited 0
- `cargo test --workspace --no-fail-fast`: exited 0 (all workspace unit, integration, and doc tests passed, 0 failed)
- `cargo test -p atrium-daemon --test requests_fake`: exited 0 (`6 passed; 0 failed`)
- `cargo test -p atrium-daemon --test requests_notifications`: exited 0 (`2 passed; 0 failed`)
