# Atrium

Atrium is a fast, lightweight agent development environment in Rust (Tauri 2 + SolidJS). The plan is in `PLAN.md`, which is the source of truth.

**Worker agents** (you were given a brief in `docs/agents/briefs/`): follow your brief and the worker rules in `docs/agents/README.md`, and ignore the rest of this file.

## Orchestrator sessions

Two people build Atrium, **Pedro and a partner, taking turns, never at the same time**. Whoever is working is the *driver*. Each driver runs their own Claude Code orchestrator. Claude Code memory is machine-local, so **the rules live here and in `docs/agents/`, not in memory.**

At the start of every session, before doing anything else:

1. The driver should already have run `scripts/handoff start`. If `docs/agents/DRIVER.md` doesn't show the current user as the active driver, stop and ask them to run it.
2. Read the latest `docs/agents/sessions/*.md` (its `## Next` section first), then `docs/agents/status.md`, `docs/agents/decisions.md` and `docs/agents/change-requests.md`.
3. Work only on the `dev` branch. `main` moves only at milestone gates.

Rules:

- **Never start milestone work** (contracts, briefs, dispatching workers) without an explicit go from the current driver for that phase. Approving one step is not permission for the next.
- Contracts and milestone gates are approved by the driver. Record who decided in `decisions.md`.
- **Push policy (D-005, D-007)**:
  - Plan, docs and orchestration tooling (`PLAN.md`, `docs/`, `scripts/`, `.gitignore`, `CLAUDE.md`, `AGENTS.md`) are pushed to `dev` right away.
  - Atrium's own code is pushed to `dev` at handoffs, and is merged to `main` only at milestone gates after sign-off.
  - Workers never push or merge.
- Heavy or privacy-sensitive features (local models, VMs, free hosted AI) are always opt-in and off by default.
- Follow the build orchestration in PLAN §8: contract → fan-out → integrate → gate, cross-provider review, the escalation ladder, `scripts/agent-run` for Codex/`agy` workers.

At the end of every session, follow the checklist in `docs/agents/HANDOFF.md`:
- no workers left running;
- worker worktrees committed;
- the session log written (with `## Next` first);
- then the driver runs `scripts/handoff end`.
