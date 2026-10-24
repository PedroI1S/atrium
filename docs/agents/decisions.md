# Decisions

Only the orchestrator writes here. Others propose changes through `change-requests.md`.

## D-001 — 2026-10-22 — Orchestration runs from Claude Code
Opus 5.5 in Claude Code orchestrates. Workers run headless: Claude subagents (Sonnet/Opus), `codex exec` (Sol/Astra), `agy -p` (Argon/Flash). MonoCode is used for viewing and as a fallback. See PLAN §8.7.

## D-002 — 2026-10-22 — agy workers are governed by a PreToolUse policy hook
Headless `agy` 1.4.5 auto-denies every command that would need a prompt, and hook `allow` can't grant permissions, but hook `deny` is enforced even under `--dangerously-skip-permissions`. So `agy` workers run with skip-permissions, and `scripts/agy-policy.py` (matcher `"*"`, fail-closed allowlist; originally `.agents/hooks/agy_policy.py`, moved by D-006) is the policy. It's a guardrail, not a security boundary: build scripts can still run arbitrary code. Re-test with every `agy` upgrade. **Approved by Pedro, 2026-10-22.**

## D-003 — 2026-10-22 — Orchestration logs live in `.orchestration/`
`.agents/` is `agy`'s project customization root (hooks, skills, plugins), so orchestration state lives in the git-ignored `.orchestration/` instead.

## D-004 — 2026-10-22 — Codex workers get network access in their sandbox
`codex exec -s workspace-write` blocks network by default, which breaks crate and npm downloads. Workers run with `-c sandbox_workspace_write.network_access=true`; writes stay confined to the worktree.

## D-005 — 2026-10-22 — Push policy
Remote: `github.com/PedroI1S/atrium` (private). Plan and doc changes (`PLAN.md`, `docs/`) are pushed right away. Code is pushed only at milestone gates, after Pedro's sign-off. Workers never push. **Decided by Pedro.**
**Amended 2026-10-23 (Pedro):** orchestration tooling (`scripts/`, `docs/agents/`, `.gitignore`, agent config) counts as docs and is pushed right away. The milestone-gate rule applies to Atrium's own code (`crates/`, `app/`, `images/`, …).

## D-006 — 2026-10-23 — The agy worker policy applies only to worker worktrees
**Problem:** the hook was committed in the repo's `.agents/`, so `agy` applied it to Pedro's own sessions in this project. It also found its script via `$(git rev-parse --show-toplevel)`, which fails whenever `agy` runs a hook outside the repo. The result was "External hook 'atrium-worker-policy' failed to execute" and blocked tool calls, e.g. reading `PLAN.md`.
**Fix:**
- The policy moved to `scripts/agy-policy.py` and is no longer committed as a hook.
- `scripts/agent-run` writes `.agents/hooks.json` (git-ignored) into each worker worktree at launch, with absolute paths to `python3` and the policy, so it doesn't depend on the working directory.
- The policy now denies any worker write to `.agents/`, so workers can't edit their own hook. The policy file itself lives in the main checkout, outside the worker's worktree.

## D-007 — 2026-10-24 — Two drivers, `dev` branch, handoff checkpoints
Pedro builds Atrium with a partner, in turns, never at the same time. **Requested by Pedro.**
- All work happens on `dev`. Docs and tooling are pushed right away; merged code is pushed at every handoff. `main` moves only at milestone gates. This amends D-005, where merged code would otherwise stay on one machine.
- `docs/agents/DRIVER.md` is the baton, managed by `scripts/handoff start|end|status`. `handoff/<date>-<driver>` tags are the checkpoints.
- Rules are shared through `CLAUDE.md` / `AGENTS.md`, not machine-local memory. `scripts/doctor` checks a machine.
- Either driver can approve contracts, phase gos and gates; security/approval default changes need both.
