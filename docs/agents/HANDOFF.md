# Two-person handoff

Pedro and a partner build Atrium **in turns, never at the same time**. Whoever is working is the **driver**: they run the orchestrator, dispatch workers, and merge. Everything the next driver needs must be on GitHub (`dev` branch) when a shift ends. Nothing important may live only on one machine.

## What lives where

| Thing | Shared? | How it's handed over |
|---|---|---|
| Plan, docs, decisions, briefs, handoffs, reviews, session logs, status board | ✅ git (`dev`) | Pushed right away |
| Merged Atrium code | ✅ git (`dev`) | Pushed at every handoff; `main` only at milestone gates |
| In-flight worker branches (`agent/*`) | ✅ git (`origin/agent/*`) | Committed (WIP is fine) and pushed by `scripts/handoff end` |
| Worktrees (`../atrium-wt/*`) | ❌ per machine | The next driver recreates them from the pushed `agent/*` branches |
| Running workers | ❌ | **Never hand off with a worker running.** Let it finish, or stop it and record a `partial` handoff |
| Native agent sessions (resume IDs) | ❌ per machine | Not resumable on the other machine. A new worker continues from the branch + handoff note |
| Raw transcripts (`.orchestration/`) | ❌ per machine | Stay with whoever produced them. Manifests note `driver` and `machine`; the curated tier in git is the shared record |
| Claude Code memory | ❌ per machine | Durable rules live in `CLAUDE.md` and `docs/agents/`, never only in memory |

## Starting a shift

```sh
git clone https://github.com/PedroI1S/atrium.git   # first time only
scripts/doctor                                      # first time, and after upgrading any agent CLI
scripts/handoff start
```

`scripts/handoff start`:
1. Syncs `dev` (fast-forward only).
2. Refuses if someone else is still the active driver (`--force` only if you're sure they've stopped).
3. Claims `docs/agents/DRIVER.md`, tags `shift/<date>-<you>`, and pushes.
4. Prints:
   - the **Next** section of the last session log,
   - every commit since the last handoff,
   - the status board,
   - the in-flight `agent/*` branches.

Then open Claude Code in the repo. `CLAUDE.md` tells the orchestrator what to read first.

## Ending a shift (checklist)

1. **No workers running.** Background Claude subagents, `codex exec`, `agy -p`. Finished workers are merged or left on their pushed branch with their handoff note.
2. **Interrupted work** gets a `partial` handoff note (`docs/agents/handoffs/<task-id>.md`): what's done, what's not, the exact next step. Worktree changes are committed on the worker's branch (WIP commits are fine).
3. **`docs/agents/status.md`** is current, including an owner/state for every task.
4. **Session log** `docs/agents/sessions/<date>-<n>.md`, starting with `## Next`: the first concrete actions for whoever comes next, in order. Then what was done, decisions, and open questions. Mention anything only you know.
5. **Decisions** recorded in `docs/agents/decisions.md`, with who decided.
6. Raw transcripts archived locally (PLAN §8.7).
7. Commit everything, then run:

```sh
scripts/handoff end
```

`scripts/handoff end`:
1. Refuses if workers are running, if a worktree has uncommitted changes, or if no session log was committed this shift.
2. Pushes every worker branch.
3. Frees the baton and tags `handoff/<date>-<you>`.
4. Pushes `dev` + tags.

To see what changed since a given handoff: `git log --oneline handoff/<tag>..dev`.

## Who decides what

Either driver can:
- approve contracts,
- give the go for a milestone phase,
- sign off a milestone gate.

Each decision is recorded in `decisions.md` with the person's name. Changes to security or approval defaults (policy hook, sandbox, editor AI trust levels) need **both** people, recorded as such.

## After upgrading `agy`

The worker policy depends on `agy` enforcing hook `deny` under `--dangerously-skip-permissions` (D-002). After an upgrade, `scripts/doctor` warns about it. Re-run the M0-00-style hook check (a scratch worktree via `scripts/agent-run`: one allowed command, one write to `.agents/` that must be denied) before dispatching `agy` workers, and update the tested version in `scripts/doctor`.
