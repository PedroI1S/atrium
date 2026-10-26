# Handoff: M0-02-codex-adapter

Model: GPT-6.1 Sol (escalated to Sonnet 5.5) · Status: done for the escalation (live fixtures separated from hand-made unit fixtures; tests green); some enforcement rows remain unverified live

## Done

- Implemented adapter, normalizer, recorder/chat harness, required hand-made
  fixtures and golden tests within owned paths. Contracts, root manifest and
  Cargo.lock are unchanged.
- Addressed accepted review findings 1–11; mapping and evidence below.
- Updated [spike report](../../spikes/M0-02-codex-adapter.md), enforcement matrix,
  provenance and regression coverage.
- Initial work was committed by the orchestrator as `83225db`; fix round 1 as
  `49e6b20`. These are historical commits, not uncommitted work.

## Not done

- Real app-server recordings and provider enforcement verification. The user
  explicitly prohibited launching app-server in this sandbox, so no live record
  or chat session was attempted in this round. Offline subprocess tests use the
  deterministic Python fake server only.
- Every committed fixture remains **hand-made**. Orchestrator's previous live
  simple run used `gpt-5.6-luna`, reached its first normalized event at **416 ms**,
  and mapped the lifecycle/message/usage/quota correctly. That unsafe recording
  was discarded and does not prove tool enforcement.
- Hook bridge runtime coverage, native rule precedence, MCP prompt coverage,
  granular behavior and process-tree cleanup remain explicit verification items.

## Definition of Done results

```text
cargo fmt --check
  PASS, exit 0.

cargo clippy -p spike-agent-codex --all-targets -- -D warnings
  PASS, exit 0; recorder/chat examples compile.

cargo test -p spike-agent-codex
  PASS, exit 0: 47 tests, 0 failed.
  21 fixture/semantic + 17 subprocess/policy + 9 recording/privacy tests.
  Includes all 13 scenario goldens and corpus-wide auxiliary golden coverage.
  Actual startup timeout test completed at the 30-second contract deadline.

python3 spikes/agent-codex/tests/support/check_schema.py
  PASS, exit 0: 180 records, 0 errors; six enforcement candidate shapes pass.
  Shape validation does not establish runtime enforcement.

git diff --check
  PASS, exit 0.

cargo run -p spike-agent-codex --example record -- <scenario>
cargo run -p spike-agent-codex --example chat
  NOT RUN: explicit user instruction prohibits real app-server in sandbox.
  Orchestrator owns these live DoD checks. Both examples compile above.
```

## Gotchas

- `ProviderGated` remains correct. Safe reads or allow-prefix commands can bypass
  provider approval; no all-tool interception claim is made. Read/Search kinds
  are provider heuristics, not authorization evidence.
- FullAccess retains danger-full-access/never; deny rules cannot apply to calls
  without callbacks. Independently emitted requests are still gated.
- Stdin inputs now represent ordered submissions to the bounded writer queue.
  Failed writes emit diagnostics. Stdout does not wait for write acknowledgements.
- Elicitation consent is supported, with null form content. Arbitrary required
  form inputs need M1 UI mapping; consent cannot invent them. Correlation ids
  absent from MCP requests are not fabricated.
- Direct child is killed/reaped and adapter tasks joined. Descendant process
  groups are not supervised; the report explicitly defers that platform work.
- Resumed usage without a historical baseline remains unknown/Unmapped for the
  first resumed turn. `model/list` does not expose numeric prices.
- Raw recordings stay in memory; do not commit ignored scratch data. Denial
  failures refuse fixture writes and must remain visible in live verification.

## Fix round 1

Committed by orchestrator as `49e6b20`, verified with 34/34 tests, fmt and clippy.
Added OS hostname/account/installation redaction, recording-only native config
isolation, diagnostic pseudonyms and classification of eight Unmapped methods.
Round 2 extends those protections to unstructured text and invocation metadata.

## Fix round 2

Read-only cross-provider review base: `49e6b20`. Addressed in priority order
1, 3, 2, then remaining findings. Detailed enforcement matrix is in the report.

| Finding | Change / evidence |
| --- | --- |
| 1 | Injected MCP prompt mode; elicitation Allow/Deny/Ask/cancel gate routing; offline wire tests. Generated schemas inspected for hooks/granular/elicitation. `record -- <scenario> --granular` sends the all-true candidate explicitly; fake test confirms FullAccess remains never. Matrix covers PreToolUse/PermissionRequest bridge, granular, MCP prompt and native allow-prefixes with verify-live labels. |
| 3 | Raw decoded stream checked before sanitization: four PASS/FAIL claims cover target denial, matching denied completion, no sentinel and no native success. Missing evidence or bypass refuses fixture writing with `read bypassed the gate` / execute equivalent. Tests reject leaked sentinel, absent gate and native success masked by normalized Denied. Sentinel never supplied to sanitizer. |
| 2 | Config and stream identifiers become sanitizer secrets; stderr becomes a placeholder; unknown notifications retain their type with secrets redacted. Tests place MCP/hook/plugin names and source paths in both unknown notifications and stderr. Scratch and argv privacy also covered. |
| 4 | willRetry errors ignored; terminal errors deduplicated per turn; refusal reset uses last quota data. Hand-made quota fixture includes retry and both terminal records, with one RateLimited golden. |
| 5 | Removed circular stdout/write dependency; 2 MB stdin/blocked stdout regression passes. Saturated unexpected child exit detected/reaped without Shutdown. |
| 6 | Added real STARTUP_TIMEOUT/reap test, live queue overflow/wire count test, saturation with open tool and pending Ask plus cancelled closing events. |
| 7 | Current 47-test results, 416 ms real timing and orchestrator commit history reconciled; stale personal paths removed. |
| 8 | Accurate live/hand-made footers; version from recorded initialize, model/thread params from sanitized launch, sanitized actual argv. Corpus-wide golden test covers resume-origin and startup-failure recordings when present. Existing notes retain hand-made provenance and synthetic model identity. |
| 9 | Invalid-model session establishment triggers shutdown and missing-resume-id fallback. Recorder refuses startup-error unless fatal pre-session lifecycle has no SessionStarted. Offline trace tests cover acceptance/rejection; verify fallback live. |
| 10 | Matrix explicitly documents FullAccess callbacks absent for ordinary commands/files and consequent inability to enforce user deny rules there. |
| 11 | Approval reason preserved; classification advisory; environment-derived home/user tests; separate 30-second gate timeout; scratch normalization; direct-child-only cleanup documented. |

## Follow-ups

1. Orchestrator: record all scenarios live, including startup fallback and resume;
   review generated metadata and privacy before committing fixtures.
2. Compare `gate-deny` and `gate-deny-execute` with default untrusted and
   `--granular`. Record bypasses in the matrix; never reinterpret a missing
   denial as a passing proof. A passing trace proves that call only.
3. Verify the synchronous PreToolUse hook bridge and PermissionRequest ordering,
   native allow-prefixes, MCP prompt mode/per-tool overrides and FullAccess.
   If writes or non-read commands bypass every setting, stop and escalate.
4. M1: implement platform-specific descendant supervision and structured MCP form
   input support; retain ProviderGated until all tool classes have live proof.

## Commit status

Both `git add` and `git commit -am` failed (exit 128): `index.lock` could not
be created in the shared repository's external `.git/worktrees` metadata,
`Operation not permitted`. No workaround was attempted. All fix-round-2 changes,
including this handoff, remain uncommitted on `agent/sol/M0-02-codex-adapter`
for the orchestrator to commit.

## Escalation (Sonnet)

Cross-provider escalation after both Sol fix rounds. The orchestrator had committed
13 live recordings (codex 0.163.0, `gpt-5.6-luna`) over the hand-made fixtures, which broke
4 tests that depended on synthetic content. Changes:

- Restored the hand-made traces from `a4b6163` into `fixtures/agents/codex/handmade/`
  (`approval-allow`, `interrupt`, `question`, `quota-usage`, `patch`; each with
  `.jsonl`, golden `.events.json` and a `hand-made` `.md`). Live recordings stay the
  primary scenario fixtures; their goldens were not regenerated (no diff, so
  `UPDATE_GOLDEN` was not needed).
- Tests (`tests/fixtures.rs`): `exit_closes_in_contract_order_and_is_idempotent`,
  `queued_messages_are_bounded_and_unknown_records_are_retained`,
  `totals_replace_and_reset_per_turn`, `duplicate_resolution_is_rejected` now use the
  hand-made traces. `lifecycle_and_security_semantics` runs the structural invariants
  (SessionStarted first, SessionEnded last, calls and requests closed once, no pending
  leftovers) on **all live and hand-made** streams, keeps the live denial /
  no-sentinel / Approved / Denied / Cancelled-tool / Interrupted checks, takes
  `RequestResolved(Cancelled)` and `Answered` from the hand-made traces, and adds
  explicit assertions that live `question` has no `QuestionAsked` and live `interrupt`
  never reached the gate (so these flip loudly if re-recorded differently). Added live
  `quota-usage` (Usage and QuotaUpdated, no RateLimited), `patch` (RateLimited, no edit
  tool live; Edit mapping on hand-made) and hand-made goldens. The sanitizer, golden
  corpus and hostname tests now walk `handmade/` too. No assertion was weakened.
- `examples/support/mod.rs`: hand-made saves go to `handmade/` so `--hand-made`
  cannot overwrite live recordings again.
- Report: matrix rows updated with live facts, fixtures section split, new "Live
  verification results" table with "live verified" markers.

Findings (no contract invariant violated by any live fixture):

1. Live Codex **did not ask the question** in `question`; no `QuestionAsked` live.
2. Live `patch` ended in `usageLimitExceeded` before any file change, so the
   file-change approval is **not live verified** (account primary window 96-99%).
   Re-record `patch` when quota is available, then `UPDATE_GOLDEN=1` for it only.
3. Live `interrupt` raced the gate: no `ApprovalRequested`, so no live
   `RequestResolved(Cancelled)`; the late `serverRequest/resolved` yields no event.
   Nothing leaks. The Cancelled resolution is proven on the hand-made trace.
4. Even a plain read (`pwd && rg ... && sed public.txt`) asked for approval under
   ReadOnly/untrusted; no write or non-read command ran without a request. Samples only.

DoD (run in this worktree, no live `codex` run):

```text
cargo fmt --check                                              PASS
cargo clippy -p spike-agent-codex --all-targets -- -D warnings PASS
cargo test -p spike-agent-codex                                PASS: 28 fixtures + 17 live(fake server) + 9 recording
python3 spikes/agent-codex/tests/support/check_schema.py       PASS: 717 records, 0 errors
```
