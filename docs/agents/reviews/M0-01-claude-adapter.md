# Review: M0-01-claude-adapter

Author: Sonnet 5.5 · Reviewer: Gemini 4 Argon (read-only via `agy --mode plan`, commit `3b0574c`; Sol was out of Codex quota) · Triage: orchestrator (Opus 5.5), 2026-10-25

## Verdict
**Approve with changes.** The `PreToolUse` hook callback + `can_use_tool` interception and the fail-closed `GateGuard` (gate panic/timeout) are sound. Fmt, clippy and all 43 tests pass.

## Findings and triage

| # | Sev. | Finding | Triage |
|---|---|---|---|
| 1 | blocker | Claude Code loads user/project/local settings by default. Hooks there run on the host outside the gate, and an external `PreToolUse` hook can rewrite `updatedInput` after the gate approved the original. | **Accepted as a finding; fix differs from the reviewer's.** Disabling all external settings by default conflicts with PLAN §3.2 (agents keep their native config) and D-011 (the user decides). Instead: an `setting_sources` option (default: native behaviour); the adapter **detects** external `PreToolUse` hooks (and other hook types that run commands) in the effective settings; when present, the report and the enforcement level say *Partial* (D-012), with the reason. Recording runs with external settings disabled so fixtures stay machine-independent. **Default to confirm with Pedro.** |
| 2 | major | `can_use_tool` without `tool_use_id` + gate `Ask` never emits `ApprovalRequested` (session hangs). `AskUserQuestion` is accepted at `can_use_tool` without checking the hook decision. | Accept. |
| 3 | major | `ApprovalRequested` can reference a call not yet started (subagent ordering, `TodoWrite`). | Accept. Open the call on first sight; treat `TodoWrite` like `AskUserQuestion`. |
| 4 | major | Pending requests aren't cancelled on `result` (turn end), so `RequestResolved` lands outside the turn. Some events can precede `SessionStarted` or fall outside a turn. | Accept. |
| 5 | major | The queue-overflow `Error` bypasses the normalizer, so live and replay drift. | Accept. Track the queue in the normalizer. |
| 6 | major | `Usage`: cost wrong after respawn (`prev_cost` not reset), subagent tokens dropped from `TurnCompleted`, mid-turn `reasoning_tokens` missing. | Accept. |
| 7 | major | Kill after grace orphans grandchildren (Bash, MCP servers), and the drain can hang on inherited pipes. | Accept. Process group + group kill + bounded drain. |
| 8 | minor | The `gate-deny-delete` fixture never attempts a deletion. | Accept. Re-record, assert `rm` in the denied command. |
| 9 | minor | Fixtures still contain `memory_paths` (another project's path), `/private/tmp/claude-501` (UID), `(America/Sao_Paulo)`, and an unmerged split path. | Accept. |
| 10 | minor | `gate_failures_deny` doesn't cover malformed hook payloads, unknown `callback_id` or the `CanUse` failure branch, though the report says it does. | Accept. |
| 11 | nit | A mid-session respawn handshake blocks the actor for up to 30 s (Shutdown delayed). | Accept. |

Reviewer's full report: orchestrator raw archive (`.orchestration/archive/M0-01-claude-adapter/`, Pedro's Mac).
