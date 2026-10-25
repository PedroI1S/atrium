# Handoff: M0-02-codex-adapter

Model: GPT-6.1 Sol · Status: partial (offline implementation complete; live verification blocked)

## Done

- Implemented `CodexAdapter` and `CodexNormalizer`, initialize/start/resume/turn/interrupt,
  gate approvals, structured questions, messages/reasoning/tools/patches/plans,
  per-turn usage, quota/refusal mapping, bounded queues and shutdown/kill/reap.
- Every required golden scenario exists, plus execution denial, patch and quota
  coverage. All fixtures are explicitly **hand-made** and sanitized.
- Added recorder and chat examples, schema regeneration instructions and a
  stdlib structural schema checker. Generated schemas/raw scratch data are ignored.
- Added real subprocess tests using a deterministic fake server: live/replay parity,
  approvals, panic denial, gate cancellation, interrupt during startup, dropped
  session cleanup, saturation, forced kill and reaping. Reserved cleanup slots
  preserve `SessionEnded` even when no events are read before shutdown.
- Spike report: [M0-02-codex-adapter](../../spikes/M0-02-codex-adapter.md).
- Changed only owned paths. Contracts, root manifest and Cargo.lock are unchanged.

## Not done

- Real Codex recordings, real time-to-first-event, live enforcement matrix evidence,
  cheapest-model verification, native persistence/resume and real question/quota
  behavior. Direct `codex app-server` printed `Operation not permitted` and exited
  before initialization. No sandbox workaround was attempted.
- Commit blocked: `git add spikes/agent-codex fixtures/agents/codex` failed creating
  `/Users/pedro/Documents/Harness/.git/worktrees/M0-02-codex-adapter/index.lock`
  with `Operation not permitted`. The chained commit could not run. All work,
  including this handoff, is left uncommitted for the orchestrator. No alternate
  git metadata or sandbox workaround was used.

## Definition of Done results

```text
cargo fmt --check
  PASS (exit 0)

cargo clippy -p spike-agent-codex --all-targets -- -D warnings
  PASS (exit 0)

cargo test -p spike-agent-codex
  PASS: 20 fixture/semantic tests + 9 subprocess/policy tests, 0 failed.
  Includes all 13 scenario golden tests, saturation and forced kill/reap.

codex app-server generate-json-schema --out spikes/agent-codex/schema
  PASS (exit 0); output ignored, not committed.

python3 spikes/agent-codex/tests/support/check_schema.py
  PASS: 179 records passed structural schema checks; 0 errors.

cargo run -p spike-agent-codex --example record -- simple
  BLOCKED (exit 1): app-server closed before model/list; check sandbox permissions.

cargo run -p spike-agent-codex --example chat
  BLOCKED (exit 1): Handshake("stdout closed before thread creation").

cargo run -p spike-agent-codex --example record -- <scenario> --hand-made
  PASS (exit 0) for all 13 scenarios; neighboring .md files identify provenance.
```

## Gotchas

- `interception()` is always `ProviderGated`. Offline approval examples do not prove
  that the provider asks before every read, safe command, file write or MCP call.
- ReadOnly/Ask currently request `untrusted` from the installed 0.163.0 schema.
  Current official docs say that selectable policy is retired; runtime behavior
  could not be checked. Startup fails rather than silently weakening settings.
  FullAccess uses `danger-full-access` + `never` on the host as required.
- On resumed threads with no historical token baseline, the first turn's token
  records remain `Unmapped` and its usage unknown. Old tokens are not charged to
  the new turn. Later turns can use the observed cumulative baseline.
- `model/list` has no price field. The recorder's mini/nano preference is a heuristic;
  verification must choose the cheapest entitled model explicitly with `CODEX_MODEL`.
- Extra permission grants, dynamic tool execution, MCP elicitations and auth refresh
  are unsupported server requests and fail closed with JSON-RPC errors.
- Opt-in raw recordings live in memory until sanitized writing; do not commit raw
  scratch files. The live scratch directory is under the owned, ignored `live/`.
- Hand-made `resume` covers the second-process wire flow only. The live harness
  records a separate `resume-origin` process before resuming its actual native id.

## Follow-ups

1. Orchestrator: commit owned changes and this handoff from a session with git metadata access.
2. Reviewer: review the pure mapper and process-owner paths, especially reserved
   lifecycle slots, write acknowledgement ordering and failure cleanup.
3. Orchestrator verification: run the recorder outside this worker sandbox for every
   required scenario and replace hand-made fixtures after review/sanitization.
4. Resolve the `untrusted` schema/runtime discrepancy, test actual bypasses across
   tool classes and all policies, and fill the matrix with live evidence. If writes
   or non-read commands bypass every setting, stop and escalate the security finding.
5. Record launch-to-first-event from real traces, confirm cheapest-model choice,
   native resume, supported question mode and refusal/error variants before M1.
