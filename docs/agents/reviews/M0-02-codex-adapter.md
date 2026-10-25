# Review: M0-02-codex-adapter

Author: GPT-6.1 Sol · Reviewer: Sonnet 5.5 (read-only, commit `49e6b20`) · Triage: orchestrator (Opus 5.5), 2026-10-25

## Verdict
**Approve with changes.** No blocker. There's no path where a command, file change or MCP call that Codex *asks about* runs without the gate deciding:
- gate panics and timeouts deny;
- aborted gate tasks never approve;
- unknown server requests get a JSON-RPC error;
- only single-call `accept` is sent.

`ProviderGated` is reported honestly. Lifecycle, `RequestResolved`, `Usage` and live/replay parity match the contract, and the saturation test drives the reserved-slot design. Fmt, clippy and all 34 tests pass.

## Findings and triage (all accepted → fix round 2, the last before escalation)

| # | Sev. | Finding | Fix asked for |
|---|---|---|---|
| 1 | major | The enforcement investigation is incomplete. It never evaluates Codex's `preToolUse` / `permissionRequest` hooks, the `granular` approval policy, per-MCP-server `default_tools_approval_mode`, or user `.rules` allow-prefixes. Injected MCP servers get no approval mode. Every `mcpServer/elicitation/request` gets `-32601`, so MCP tools can never be allowed under Ask. | Add matrix rows for all four (mark "untested, verify live" where needed). Set `default_tools_approval_mode="prompt"` on injected servers. Route elicitations to the gate (accept/decline/cancel). Make a `preToolUse` hook bridge an explicit verification item. |
| 2 | major | Recorder redaction gap: discovered MCP, plugin and hook names (and `sourcePath`s) aren't passed to the `Sanitizer`, so stderr lines and unknown notifications can still leak them. | Pass them as secrets. Placeholder stderr lines in committed fixtures, or allow-list strings in unknown methods. Test names in stderr and in an unknown notification. |
| 3 | major | The live `gate-deny` proof probably can't work. Under `untrusted`, Codex auto-runs safe reads like `cat secret.txt` without asking, and the recorder would write a `live` fixture containing the secret. | In `record.rs`, for `gate-deny*`, check the pre-sanitization stream: there's a Denied call and the secret never appears. Otherwise fail loudly ("read bypassed the gate") so it's recorded as an ungated read in the matrix. Never add the secret to the Sanitizer. |
| 4 | minor | Errors are mapped twice (the `error` notification and `turn/completed.error`), and `willRetry` is ignored, so a retried 429 pauses the thread. | Skip `Error`/`RateLimited` for `willRetry: true`. Dedupe per turn. Add a fixture with both, and fill `resets_at_ms` from the last rate-limit update. |
| 5 | minor | Stdout reading pauses while a write is in flight, so it can wedge on a large write. A child that dies while the event channel is saturated goes unnoticed. | Order by ack sequence instead of pausing reads, or poll `child.try_wait()` on the tick. |
| 6 | minor | Test gaps: no `STARTUP_TIMEOUT` test; the message-queue bound isn't tested end to end (live drop vs normalizer error can drift); the saturation test has no open tool call or pending Ask. | Add all three. |
| 7 | minor | The report and handoff are stale or inconsistent: test counts, "time to first event unavailable" (it's 416 ms), "uncommitted", and a local `/Users/...` path. | Fix. |
| 8 | minor | Fixture provenance: live `.md` files get the hand-made footer, the CLI version is hard-coded, and the model and command line are missing. `resume-origin` / `*-startup-failure` have no golden. | Fix. |
| 9 | minor | An invalid model may only fail at `turn/start`, which makes `startup-error` an established session. | Detect it and fall back to another pre-session failure (bad `-c` value, bad resume id). |
| 10 | minor | FullAccess (`approvalPolicy=never`) intercepts nothing, so user deny rules can't apply. | Say so in the matrix. |
| 11 | nit | `reason` dropped from `ApprovalRequested`; `Read`/`Search` kinds are advisory (Codex heuristic); a test hard-codes `/Users/pedro`; the gate timeout reuses `SHUTDOWN_GRACE`; no process-group kill; live scratch paths show as `~/Documents/...`. | Fix, or document in the report. |
