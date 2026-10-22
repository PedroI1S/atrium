# Agent orchestration

How Atrium is built by a team of agents. The full rules are in [PLAN.md §8](../../PLAN.md#8-build-orchestration-how-we-build-atrium-with-agents).

| Path | Written by | Purpose |
|---|---|---|
| `briefs/<id>.md` | Orchestrator | One task: goal, owned paths, contract, Definition of Done |
| `handoffs/<id>.md` | Worker | Done / not done / gotchas / follow-ups, written at the end of a task |
| `reviews/<id>.md` | Reviewer (different provider) | Line comments and findings |
| `decisions.md` | Orchestrator only | Decision log (ADR style) |
| `change-requests.md` | Workers (append) | Requests to change a frozen contract |
| `routing.md` | Orchestrator | Scoreboard per model × task type |
| `status.md` | Orchestrator | Live board of all tasks |
| `sessions/<date>-<n>.md` | Orchestrator | Log of each orchestrator session: dispatched, decided, escalated, open. Read first by a fresh session. |

## Rules for workers

1. Work **only** in your worktree and **only** in the paths your brief lists as owned.
2. Never change a contract (types, traits, schemas marked as contract). Append a request to `change-requests.md` instead and stop.
3. Never commit to `main`, merge, or push. Commit only on your own branch.
4. Before finishing, run every Definition of Done command from your brief and paste the results into your handoff.
5. Always write `handoffs/<id>.md`, even if you failed. Say what blocked you.

## Conventions

- Task IDs: `M<milestone>-<nn>-<slug>`, e.g. `M0-01-codex-client`.
- Branches: `agent/<model>/<task-id>`, e.g. `agent/sol/M0-01-codex-client`.
- Worktrees: `../atrium-wt/<task-id>`.
- Logs (git-ignored): `.orchestration/logs/<task-id>.jsonl`.
- Raw transcript archive (git-ignored, never committed): `.orchestration/archive/<task-id>/` with `manifest.json`. See PLAN §8.7.
