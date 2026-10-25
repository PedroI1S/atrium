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

## D-008 — 2026-10-24 — Go for the M0 contract phase
**Pedro gave the go** for the M0 contract phase only: contracts and briefs, no dispatch. Drafted by the orchestrator:
- `crates/atrium-protocol` (normalized agent events, daemon wire protocol), `crates/atrium-agents` (adapter interface, fixtures);
- `docs/contracts/m0.md` (layout, ownership, fixture scenarios, UI token architecture);
- briefs `M0-01` … `M0-09`.

Live provider runs that workers' sandboxes or policy can't do (Codex app-server inside Codex, `agy`/`opencode` under the `agy` worker policy) are recorded by the orchestrator during verification. The worker policy is **not** widened, since that would change a security default and need both drivers. **Contract approval and dispatch: pending.**

## D-009 — 2026-10-24 — M0 contracts revision 2 (Astra review applied)
GPT-6 Astra reviewed revision 1 read-only (`docs/agents/reviews/M0-contracts.md`): 2 blockers, 12 majors, 1 minor, all accepted. **Pedro asked for all 15 fixes to be applied.** Main changes:
- **Policy enforcement**: a `ToolGate` in `SessionConfig` sees every interceptable tool call before it runs, fails closed, and adapters report `Interception::AllToolCalls | ProviderGated`. Each adapter report has an enforcement matrix. `FullAccess` is rejected on local transport.
- Events: `RequestResolved`, structured question answers, replacement fields on `ToolCallUpdated`, `QuotaUpdated` vs `RateLimited`, per-turn `Usage` semantics, and two lifecycles (established vs pre-session failure) with a fixed closing order.
- Adapters: `AdapterInput` + `Normalizer::on_input` (live and replay share one state machine), bounded buffers, deadlines, a saturation test.
- Wire: socket security, `Subscribe` with a response, "last applied" cursors, no automatic command retries; `AgentLaunch::Native` is an M0 placeholder.
- Fixtures: structured `Sanitizer`; new scenarios `gate-deny`, `question`, `interrupt`, `startup-error`.
- `agy`: the hook bridge fails closed, retry grants are bound to exact arguments and used once, and live hook-failure tests are run by the orchestrator.

**Still open for the drivers:** hard-deny scope for `ProviderGated` providers (a security default, needs both); the Linux measurement owner/deadline; the push-policy question for local contract code. **Contract approval: pending.**

## D-010 — 2026-10-24 — Linux measurements are owned by Pedro's partner
**Decided by Pedro.** The Linux/WebKitGTK measurements that PLAN §5 asks for in M0 (screencast, streaming, UI shell: blur fallback, 120 Hz smoothness, idle memory) are run by the partner on their machine. Each UI spike report lists exactly what that run must check.

## D-011 — 2026-10-24 — The user decides; Atrium sets safe defaults but doesn't forbid choices
**Decided by Pedro.** Atrium ships safe defaults, but it doesn't take choices away from the user. In particular, `PermissionPolicy::FullAccess` is available on the host, not only in a sandbox. This reverses that part of revision 2 (Astra finding 1, which made `FullAccess` + local transport `Unsupported`). The gate is still called wherever an adapter can intercept, and applies whatever rules the user has set. Defaults are unchanged (`Ask` for workspace threads, `ReadOnly` for Home). Added to PLAN §0 as design principle 7.

## D-012 — 2026-10-24 — Provider enforcement levels are shown to the user, never used to restrict
**Decided and approved by Pedro (owner), 2026-10-24.** (Originally recorded as also needing the partner; D-013 removed that requirement.)
- Each provider gets an enforcement level from the M0 enforcement matrix: *Full* (every tool call passes the gate) or *Partial* (only what the provider asks about).
- The level is shown in the provider picker, the thread header and *Settings → Agents*, with an explanation and the matrix.
- Atrium never blocks, downgrades or forces a sandbox on a provider because of its level.
- Rules and the hard-deny list still apply to every call the gate sees.
- PLAN §3.12 updated, which also adds the *Full access* preset from D-011.

## D-013 — 2026-10-24 — Pedro is the project owner; security defaults need only his approval
**Decided by Pedro.** This amends D-007: there is no partner sign-off for security or approval defaults (policy hook, sandbox, approval presets, editor AI trust levels). Pedro decides them as the owner; the partner can propose. Either driver can still approve contracts, give phase gos and sign off gates. It closes the D-009 open item on hard-deny scope through D-012, approved by Pedro. PLAN §8.9 and `docs/agents/HANDOFF.md` updated.

## D-014 — 2026-10-24 — M0 contracts revision 2 approved and frozen
**Approved by Pedro.** Frozen: `crates/atrium-protocol`, `crates/atrium-agents`, `docs/contracts/m0.md`, and briefs M0-01 … M0-09, all as of revision 2 (D-009, with D-011 applied). The code is tagged `contracts/m0-r2`, local until the next handoff (D-007). From now on, any change goes through `docs/agents/change-requests.md`. **Dispatch: not yet. Pedro will give a separate go.** Before M0-03/M0-04 are dispatched, the `agy` 1.4.7 hook check (HANDOFF.md) must pass.
