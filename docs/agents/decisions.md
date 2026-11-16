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

## D-015 — 2026-10-25 — Argon paused (weekly limit used up; no paid overage)
**Decided by Pedro.** Gemini 4 Argon isn't used until Pedro says otherwise. Reassignments, following PLAN §8.3 (escalate across providers):
- **M0-03 (`agy` adapter) and M0-04 (ACP adapter)**: Argon's work stays on its branches. Any further fix rounds go to **Sonnet 5.5**, and their reviewer becomes **GPT-6.1 Sol** (never the author's provider).
- **Reviews Argon would have done** (cross-provider for Anthropic-authored work, security and isolation reviews): **Sol**, and **GPT-6 Astra** for gate/security reviews (with Pedro's OK for each Astra launch, as before).
- Gemini 3.8 Flash isn't affected by this decision (M0-09 stays with Flash).

Both providers have limits too (Codex on ChatGPT Plus ran out on 2026-10-25), so dispatch respects them. When a worker hits a limit, it's paused and resumed after the reset, not re-routed silently.

## D-016 — 2026-10-25 — `agy` fails open on a broken hook config; `agent-run` now validates it
A live run of the M0-03 hook-failure harness on `agy` 1.4.7 (with a control case that proved tools do run) showed:
- a broken hook **helper** fails closed: missing binary, non-zero exit, invalid JSON output, timeout;
- a broken hook **config** fails **open**: when `hooks.json` is unreadable or malformed, `agy` ignores it, and under `--dangerously-skip-permissions` it runs every tool.

This amends D-002's assumption. `scripts/agent-run` now parses the generated `hooks.json` and self-tests the policy before launching an `agy` worker, and refuses otherwise. The M0-03 adapter gets the same check (fix round 2). It's hardening, not a change to any default.

## D-017 — 2026-10-25 — Claude keeps its native settings; external hooks make enforcement Partial
**Decided by Pedro (owner).** The Claude adapter loads the user's native Claude Code settings by default (user, project, local, managed), as PLAN §3.2 and D-011 require. When those settings define hooks that run outside Atrium's gate, the adapter reports its enforcement level as *Partial*, with the reason (D-012), instead of disabling the user's configuration. A `setting_sources` option lets a user restrict it. Fixtures are recorded with external settings off so they stay machine-independent. This closes the open item from the M0-01 review (finding 1).

## D-018 — 2026-10-25 — M0-03 and M0-04 merged as spikes; review findings become M1 must-fixes
**Decided by Pedro.** Both reviews (Sol) said rework, each with a security blocker: the M0-03 hook-config race, and M0-04 inherited agent rules re-enabling Code Mode. Both tasks had used their two fix rounds. They're merged as spikes, since their M0 questions are answered (real protocols, gating feasibility, fail-open behaviour). Every finding is recorded in `docs/agents/m1-must-fix.md`, which becomes part of the Definition of Done of the M1 tasks that turn these spikes into real adapters.

## D-019 — 2026-10-25 — OpenCode keeps native Code Mode; enforcement shown as Partial
**Decided by Pedro (owner).** As with Claude (D-017): Atrium sessions keep OpenCode's native Code Mode (`execute`) on by default. It can't be gated, so the provider's enforcement level is shown as *Partial*, with the reason (D-012). The user's own MCP servers keep working through it. Turning Code Mode off per session (Full enforcement, but then the user's own MCP servers need direct mode) is offered as an option. The M0-04 spike denies it by default; M1 flips the default (`m1-must-fix.md` O1).
*Gate note (2026-10-26, `reviews/M0-gate.md`):* "Full" with Code Mode off is only claimed once O1 holds for every agent (build, custom, managed and persisted permissions; MCP and subagents). Until then it shows as *Partial*. The default above is unchanged.

## D-020 — 2026-10-25 — Go for M0 wave 2
**Pedro gave the go** for wave 2: M0-07 (stream perf, on the merged M0-05 `ClientHandle`) and M0-08 (browser screencast), both Sonnet 5.5, reviewed by Sol (cross-provider, D-015). Wave 3 (M0-09) still needs its own go.

## D-021 — 2026-10-25 — Everything paused: Tauri vs GPUI under consideration
**Decided by Pedro.** All work is paused while Pedro considers switching the UI from Tauri 2 + SolidJS to GPUI (Rust-native, the framework Zed uses).
- **M0-07** (stream perf) and **M0-08** (browser screencast, in fix round 1 after an approve-with-changes review) were stopped mid-task. Their work is committed as WIP on their branches.
- **Unaffected by the choice**: the contracts, the four agent adapters, the daemon/client and most of the CDP client (all UI-agnostic Rust, ~32k lines).
- **Affected**: the M0-06 shell code (the token design survives), M0-07's IPC question, M0-08's frontend, and the PLAN's web-library assumptions (CodeMirror, xterm.js, Shiki, elkjs, Kobalte/Corvu, CSS theming).
- **The orchestrator's recommendation** if GPUI stays on the table: a GPUI shell spike mirroring M0-06's checks (tokens, panes, 5k-message transcript, TTI and memory, plus screencast frames as images) before deciding. MonoCode's own Tauri→GPUI migration (`mc/migrate-monocode-to-gpui`) is prior art.

## D-022 — 2026-10-25 — Stay on Tauri for now
**Decided by Pedro**, after the GPUI discussion (D-021). Tauri 2 + SolidJS stays the UI stack for v1. The UI layer stays thin and the daemon protocol clean, so a native (GPUI) client remains possible later. The pause from D-021 stays in effect until Pedro says to resume: wave 2 (M0-07, M0-08) waits on his go.

## D-023 — 2026-10-25 — UI design direction (input for the design session)
**Pedro's requirements**, replacing the M0-06 directions, none of which he liked:
- Minimalist, light, calm, easy on the eyes; not overwhelming.
- Progressive disclosure: deep customization exists but appears only when you look for it.
- Settings: a simple view of the essentials; **Advanced** opens as a side pane, never a large popup full of information; extensible, so features add sections without clutter.
- Any chat, editor or database view can become **its own window** if wanted.
- Chats can be **organized by topic/theme** (groups or folders).
- **Window opacity setting**: fundamental, many users want it.

**Next step (not started; waits on Pedro's go)**: an Opus design session produces a clickable design artifact with 3–4 layout options (sidebar and chat grouping, settings plus the advanced pane, a popped-out window, light and dark). Pedro picks; the result becomes the M1 UI spec and token values (PLAN §3.20).

## D-024 — 2026-10-25 — UI direction: layout B "Floating spaces", Atrium Calm themes
**Chosen by Pedro** in the design session (Opus, `docs/design/ui-direction.md`).
- **Layout:** B, *Floating spaces*. It also takes C's centered file path in the editor header, and keeps B's inbox list.
- **Look:** near-monochrome greys with one ink accent; colour only where it carries meaning. References: Things/Bear/Craft for calm, Arc/Zed for minimal chrome.
- **Opacity:** both modes (background-only and whole window), the user picks.
- **Draft themes:** `docs/design/themes/atrium-calm-{light,dark}.toml`.
- **Prototype:** the "Atrium UI directions" artifact, artboard B.

This replaces the M0-06 directions; M0-06's token system and layout mechanics stay. **CR-02** (runtime window-opacity tokens, surface channel tokens, overlay state tokens, folding in CR-01) is a token-contract change proposal for the M1 contract phase. It needs Pedro's approval with the M1 contracts, as does the `macOSPrivateApi` distribution decision that transparency needs.

## D-025 — 2026-10-25 — Add M0-06b (layout B, opacity, pop-out windows) to M0
**Decided by Pedro.** M0-06 is not redone; its token system, layout mechanics and measurements stand. M0-06b de-risks what layout B adds: both opacity modes (CR-02), pop-out windows, and the performance of the floating-card look. **Worker: GPT-6.1 Sol; reviewer: Sonnet 5.5** (Pedro is short on Claude credits). Sol can't launch GUI apps in its sandbox, so the orchestrator runs the app and the measurements. Brief: `docs/agents/briefs/M0-06b-ui-shell-layout-b.md`. It runs with wave 2, so the D-021/D-022 pause still applies until Pedro says go.

## D-026 — 2026-10-25 — Resume: M0-06b and M0-08 (M0-07 stays paused)
**Pedro gave the go** to start M0-06b (Sol, reviewer Sonnet) and to resume M0-08's fix round 1 (Sonnet, from WIP `f50361e`). M0-07 stays paused (~65% done, WIP `c06628a`), and so does wave 3 (M0-09). The D-021 pause is lifted only for these two tasks.

## D-027 — 2026-10-26 — M0-06b handed from Sol to Opus
**Decided by Pedro**: Sol's M0-06b result was underwhelming. An Opus 5.5 subagent takes over the fix round. It fixes all 13 review findings, raises the design fidelity to `docs/design/ui-direction.md`, and runs the full measurement matrix and screenshots itself (it runs on the host). The cross-provider reviewer for Opus's work is Sol (read-only). Routing note: for UI/design-heavy frontend work, Sonnet/Opus outperformed Sol in M0 (M0-06 vs M0-06b).

## D-028 — 2026-10-26 — Idle memory budget is per main window; pop-outs add ~35 MB each
**Decided by Pedro.** The PLAN §4 app idle budget (< 150 MB) applies to the app with **one window**. Each popped-out window has its own WebKit content process and adds about 35 MB (M0-06b: 194–207 MB with three windows); that's acceptable. M1 doesn't need to cap or lazily create pop-outs for the budget, but it should still measure each window's cost and keep it from growing.

## D-029 — 2026-10-26 — M0-06b gets its final fix round (2/2) with Opus
**Decided by Pedro.** After Sol's second review (rework, 9 findings), Opus does the final round on exactly those 9 findings: the close-time edit race, the scroll flush on pop-out, TTI per contract §5 (paint and input), measurements under normal scheduling (no harness-only latency-critical activity), the vibrancy material cache, cold-matrix resilience, the reproduction command, a deterministic forced bring-back, and the font licence notices. Then it re-measures and is merged.

## D-030 — 2026-10-26 — Repo out of iCloud sync (`.nosync`); M0-09 launched
**Decided by Pedro.**
- **The problem:** `~/Documents` is synced by iCloud Drive, which kept creating "`file 2.md`"-style conflict copies while git and agents rewrote files. There were ~1,700 in the checkout and 7 inside `.git` (including a broken ref `origin/dev 2`). It also caused the "local changes to Cargo.lock" rebase interruptions, a stray `Sidebar 2.tsx` that would have broken a build, and 23 copies swept into pushed docs commits (removed in `cdab9cc`).
- **The fix:** `~/Documents/Harness` → `Harness.nosync` and `~/Documents/atrium-wt` → `atrium-wt.nosync`, which iCloud skips. Symlinks keep the old paths working. All copies were deleted (untracked only), `git worktree repair` run, `git fsck` clean. `.gitignore` now ignores `* [0-9].*`.
- **Rule for every machine** (in `docs/agents/README.md`): the repo and worktrees must not live in a cloud-synced folder.
- **M0-09** (wave 3, Flash) launched with updated inputs.

## D-031 — 2026-10-26 — M0 gate review by Astra
**Pedro approved launching GPT-6 Astra** (read-only, `codex exec -s read-only`) for the M0 gate review: security and architecture, starting from `docs/spikes/M0-findings.md`, on a frozen snapshot of `dev` (`666ce2d`). Argon is paused (D-015), so Astra reviews alone. Still to come for the gate: Pedro's hands-on checks, the partner's Linux checks (or an explicit deferral), decisions on CR-01 … CR-04 and `macOSPrivateApi`, then Pedro's sign-off (merge `dev` → `main`, push the code).

## D-032 — 2026-10-26 — M0-06b hands-on check: accepted as is
**Pedro**: the M0-06b build (layout B, Atrium Calm, opacity, pop-outs) "feels great for now, nothing to change". The hands-on gate check is done.

## D-033 — 2026-10-26 — Linux checks run by the partner after the M0 gate
**Pedro**: once M0 is closed, he hands the baton over (`scripts/handoff end`), and the partner runs the Linux/WebKitGTK checks (D-010) on their shift: blur fallback, opaque Linux mode, 60/120 Hz smoothness, TTI and memory for the shell, stream perf and screencast. They're explicitly **outside the M0 gate** and must be done before M1's UI work depends on them.

## D-034 — 2026-10-26 — No Mac App Store: `macOSPrivateApi` accepted
**Pedro** doesn't plan to ship through the Mac App Store. Tauri's `macOSPrivateApi` (needed for transparent windows / background-only opacity) is therefore accepted, and distribution is a notarized DMG (PLAN §5 M8). The CR-01 … CR-04 token and contract proposals are still decided in the M1 contract phase.

## D-035 — 2026-10-26 — M0 gate signed off; `dev` merged to `main`
**Signed off by Pedro (owner).** M0 is closed on Astra's verdict, *pass with conditions* (`reviews/M0-gate.md`), Pedro's hands-on check (D-032) and the corrected findings report (`docs/spikes/M0-findings.md`). `dev` is merged to `main` and all M0 code is pushed.
- **Conditions carried into M1** (`m1-must-fix.md`, "From the M0 gate review"): G1–G3 are blockers for freezing the M1 contracts (per-session enforcement and honest "hard deny" wording; the daemon's human-control authority; authoritative, single-use approvals). G4–G6 are majors (protocol changes incl. amended CR-03; performance statistics; Linux checks before M1 UI work merges).
- G1–G3 change approval and enforcement behaviour, so their defaults are Pedro's call (D-013).
- The Linux/WebKitGTK checks stay outside the gate and go to the partner (D-033).
- CR-01 … CR-04 are decided in the M1 contract phase. M1 starts only on an explicit go from the driver.

## D-036 — 2026-10-26 — Repo moved out of `~/Documents` (corrects D-030)
**Decided by Pedro.** The `.nosync` rename (D-030) didn't stop iCloud. After it, iCloud created 1,097 more conflict copies, including in `.git` (`index 4…7`, `AUTO_MERGE 6…8`) and in build output during a build. It also renamed the Codex spike's `src` folder to `src 4`, which is what made the 2026-10-26 rebase "local changes would be overwritten" glitch and the four missing files. No pushed commit was affected (exact-file staging; `fsck` clean).
- **The fix:** the repo is now `~/Harness` and the worktrees are in `~/atrium-wt`. There are no symlinks left in `~/Documents`. `git worktree repair` was run, and 1,473 conflict copies were quarantined in `~/Harness-icloud-copies` (Pedro deletes them).
- **Verified:** a full clean rebuild plus the workspace tests (231 passed, 0 failed, 1 ignored) produced no new copies.
- After a move, run `cargo clean`: test binaries keep the old `CARGO_MANIFEST_DIR` and fail on fixture paths.
- **Rule for every machine** (`docs/agents/README.md`): keep the repo outside any synced folder; a `.nosync` rename inside one is not enough.

## D-037 — 2026-10-27 — Linux measurement plan for the M0 spikes (G6) approved
**Approved by rodrigo (driver).** This approves the method for the D-033 checks. It's not a go to build, dispatch or measure; that needs its own go.
- **Where:** the orchestrator runs in a Debian distrobox on rodrigo's CachyOS laptop. Builds and measurements run on the host (`distrobox-host-exec`), against the host's WebKitGTK and Mesa, never in the container.
- **Rules:** release builds; one GUI app at a time under `flock /tmp/atrium-gui-measure.lock`; sweep for orphaned processes after each run. "Warm" means one discarded launch first, as on macOS.
- **ui-shell (layout B), 1 and 3 windows:**
  - opaque mode and disabled controls: `roundtrip` checks, plus a hands-on check with screenshots;
  - TTI: 10 warm launches per configuration, against 400 ms as a reference (PLAN §4 states that budget for macOS only);
  - memory: PSS summed over the app and its WebKit children at 10 s and 30 s, 3 launches per configuration, plus a 5-minute 1 Hz idle trace; RSS, USS and DRM GPU memory reported separately;
  - smoothness: rAF frames, 3 runs per configuration at 144 Hz and 60 Hz (120 Hz if offered), plus one set with compositing disabled; pass if every run's median fps is at least 97% of the refresh rate.
- **stream-perf:** `single` (3 runs at 144 Hz, 1 at 60 Hz), `multi` (3 runs), `memory` (one 300 s run). Typing stays synthetic, so input latency stays unproven.
- **browser-screencast:** `bench.sh`, 10 runs with Brave, plus 1 run at 2× scale. Chromium CPU and PSS are sampled from `/proc`.
- **Harness:** Linux branches in `measure.py` and `bench.py`, plus a `/proc` sampler. No app code changes without asking. No worker.
- **Machine answers (rodrigo):** shell zsh. Claude Code (all models), Codex and Google plans are all available on this machine; their limits aren't known. As in D-015, a worker that hits a limit is paused, not silently re-routed. Pedro's routing constraints (Argon paused, at most 3 Sol jobs at once) applied to his accounts and don't bind this machine.
- **Power:** the orchestrator chose **balanced on AC** for every run, since it's the default a user runs. The laptop was in power-saver on battery when this was recorded. Each run records the profile and AC state.
- **Still open:** fresh-launch TTI (needs sudo to drop caches) and a read-only Sol review of the harness before its numbers count.

## D-038 — 2026-10-27 — Linux measurements run on battery
**Decided by rodrigo (driver).** This replaces D-037's "balanced on AC": the charger isn't detected (`AC0` offline, battery discharging at about 20 W), and rodrigo can't plug in now, so the measurements run on battery with the balanced profile. rodrigo gave the go to measure this way. Each run records the power state. The report marks every number as "on battery", since firmware power limits on battery can lower CPU and GPU clocks. If the battery falls below 20%, runs stop. A repeat on AC power is a follow-up only if a result is borderline.

## D-039 — 2026-11-04 — M0 Linux checks finished (G6)
**Decided by rodrigo (driver).** The Linux/WebKitGTK checks of D-033 are finished (`docs/spikes/M0-linux-checks.md`):
- **Pass:** opaque mode (one hint still has macOS text, L1); stream-perf; screencast; 60 Hz frames on the `performance` profile.
- **Fail:** TTI (median 446–472 ms against 400, not a power-saving effect); memory, measured as PSS; 144 Hz (WebKitGTK renders content updates at 60 fps); 60 Hz on `balanced` on battery (3 sections).
- **Harness:** reviewed read-only by Codex, as D-037 required. Its two accounting bugs were fixed. The raw data shows the reported numbers weren't affected, so nothing was re-measured. Fresh-launch TTI (needs root) was dropped.

G6 is met: the checks are done before any M1 UI work merges. The failures aren't blockers for G6. They become M1 inputs, and three of them are Pedro's call: the Linux memory metric (L2), the TTI budget on Linux (L3) and high refresh (L10).

## D-040 — 2026-11-09 — Linux memory budget is measured as private PSS (L2)
**Decided by Pedro (owner).** On Linux, the PLAN §4 app idle budget (< 150 MB, one window) applies to **private PSS**: PSS without shared-library pages (anonymous memory plus private file pages), summed over the app and its WebKit processes. That's the closest match to macOS `phys_footprint`, which also leaves out shared system libraries. It's about 144 MB today (`M0-linux-checks.md` §2.5), so within budget.
- GPU memory (DRM fdinfo) is reported next to it and watched for growth. It has no budget yet, since there's no like-for-like macOS figure.
- Full PSS and RSS keep being reported for context. They aren't budgeted.
- Pop-outs (~75 MB PSS each on Linux, vs ~35 MB on macOS) are measured, not capped, as in D-028. M1 keeps each window's cost from growing.

## D-041 — 2026-11-09 — 400 ms TTI stays the goal on Linux; M1 profiles WebKitGTK startup (L3)
**Decided by Pedro (owner).** The 400 ms cold-start target applies to both platforms. On Linux it's a goal, not a gate, until profiling shows what's reachable: Linux TTI is measured and reported for M1 work, but a miss doesn't block a merge.
- M1 profiles WebKitGTK startup: the pre-JS block (main → JS, ~212 ms), SolidJS mount (~140 ms, about twice macOS), and the step from input-ready to first paint (~85 ms vs ~60 ms). Measured median today: 446–472 ms (`M0-linux-checks.md` §2.4).
- After that profiling, the driver decides whether 400 ms becomes a gate on Linux.

## D-042 — 2026-11-09 — 60 fps on every display for now; no 120 Hz promise (L10)
**Decided by Pedro (owner).** Atrium targets 60 fps on every display, which is what PLAN §4 asks for. Linux meets it cleanly, including on a 144 Hz monitor where WebKitGTK caps content updates at 60 fps (`M0-linux-checks.md` §2.3).
- No UI, docs or release notes promise 120 Hz or higher.
- Later, as a timeboxed task (not M1's critical path): find what caps WebKitGTK at 60 fps, and check macOS on a 120 Hz display (ProMotion or external). The spikes only ran on 60 Hz Mac screens.

## D-043 — 2026-11-09 — Go for the M1 contract phase; no Codex, so Sonnet and agy take its roles
**Decided by Pedro (driver).** Pedro gave the go for M1's contract phase only: contracts, test stubs and briefs. Dispatching workers needs its own go after he approves the contracts.

Pedro's Codex subscription has ended, so GPT-6.1 Sol and GPT-6 Astra are unavailable on his shifts. Argon's usage has reset, which ends D-015's pause. Routing until this changes (PLAN §8.2's pairing rule still holds: the reviewer is never from the author's provider):
- **Sol's work** (Rust daemon, store, client, policy engine) goes to **Sonnet 5.5**. Opus 5.5 is used as a worker only when a task needs it, such as a fix after two failed rounds or a contract-heavy task.
- **Astra's reviews** (contract review, gate review, security review) go to **Gemini 4 Argon**, read-only through `agy --mode plan`.
- **Cross-provider review** of anything Claude writes is done by Argon or Gemini 3.8 Flash, and Claude reviews anything agy writes.
- Argon keeps its M1 role (adversarial tests of the deny list and approval rules, socket permissions), and Flash keeps tests, fixtures and docs.
- Live Codex work (X1–X3) needs a Codex login. On rodrigo's shifts their Codex can be used, under the same rules.

`agy` is now 1.5.1. The worker policy hook was last verified on 1.4.7, so it is re-checked before the first agy worker runs (PLAN §8.7). *Update, same day:* re-checked on 1.5.1, and again on 1.5.3 after agy updated itself mid-session: `cargo` allowed, `.agents/` writes and `git push` denied. `scripts/doctor` now pins 1.5.3.

## D-044 — 2026-11-09 — Enforcement is per session; unverified coverage shows as Partial (G1)
**Decided by Pedro (owner, D-013).** Enforcement is a property of each session, not of a provider. Each session records a descriptor: effective level, reasons, provider and CLI version, and the tool coverage that was verified.
- Only two levels are shown: **Full** and **Partial**. Anything Atrium can't verify is Partial, with the reason shown, such as "plugin hooks not inspected" or "untested CLI version". Unknown coverage never shows as Full.
- The hard-deny list takes precedence only over calls Atrium intercepts. It is not a machine-wide prohibition, and the UI and PLAN §3.12 say so. They also keep three things apart: interception coverage (which calls Atrium sees), the policy decision (what it does with them) and OS isolation (the sandbox, the only thing that limits what allowed code does).
- `AgentAdapter::interception(&self)` becomes session-scoped in the M1 contract.

## D-045 — 2026-11-09 — Human-control commands come only from the verified Atrium app (G2)
**Decided by Pedro (owner, D-013).** The daemon separates two kinds of authority:
- **Agent and MCP connections** get a per-thread scope. They can never approve, answer a question, create or edit rules, or change settings.
- **Human-control commands** (resolving approvals, answering questions, rules, settings, policy presets) are accepted only on a connection whose peer process is the Atrium app binary. On macOS that's checked through the peer's code signature (audit token). On Linux it's the peer's `/proc/<pid>/exe`, checked against the installed app.
- The remaining limit is documented: an unconfined agent running as the same user can still do anything that user can, for example by modifying the app or the daemon's files. The sandbox is the real boundary. Peer checks raise the bar; they are not isolation.

## D-046 — 2026-11-09 — Approvals wait, are never auto-allowed, and expire with their turn (G3)
**Decided by Pedro (owner, D-013).** One authoritative approval state in the daemon validates and consumes each approval before any effect runs.
- An unanswered request waits while the agent is alive and is never auto-allowed. It expires when the turn ends, the agent exits, the daemon restarts or the provider times out. An expired or cancelled request can never authorize anything.
- **Allow once** is single use, bound to the session incarnation, the request, the tool, the canonical arguments and the policy context.
- An answer from a stale window or an old connection is rejected, and the window shows the request as expired.
- This promotes A4 and O2 to blockers wherever those adapters are promoted.

## D-047 — 2026-11-09 — Codex stays in M1; its live checks run on rodrigo's shift
**Decided by Pedro (driver).** M1 keeps Home chat with Claude Code and Codex. The Codex adapter is built against the recorded fixtures on Pedro's shifts. The live checks (X1–X3: file-change approval, the question mode, the `ProviderGated` matrix) run on rodrigo's shift with their Codex login. They're required before the M1 gate.

## D-048 — 2026-11-09 — Every UI task goes to Opus 5.5
**Decided by Pedro (driver).** Any UI job (SolidJS app, Tauri windows and bridge, tokens and themes, UI harnesses and screenshots) is done by **Opus 5.5** as a Claude Code subagent, not Sonnet. This follows M0-06b, where design fidelity only came right with Opus (D-027).
- In M1, that covers M1-09a (app shell) and M1-09b (Home chat, inbox, approvals, questions).
- Reviewers stay cross-provider: Gemini 3.8 Flash or Argon, with screenshots as evidence.
- At most one Opus UI worker at a time, besides the orchestrator.

## D-049 — 2026-11-09 — The M1 contract code is pushed to `dev` right away
**Decided by Pedro (driver).** The contract code (`crates/atrium-protocol`, `crates/atrium-agents/src/lib.rs`, the `atrium-core` and `atrium-host` signatures, the schema and test stubs) is pushed to `dev` as soon as it's committed, like the docs. It's the spec workers and the other driver build against. Worker code still reaches `origin` only at handoffs (D-005, D-007).

## D-050 — 2026-11-09 — Home policy defaults and the always-blocked list (M1 contract review)
**Decided by Pedro (owner, D-013)**, on Argon's M1 contract review (`reviews/M1-contracts.md` findings 3, 6, 10):
- **Always-blocked list (`HARD_DENY_DEFAULTS`)**: approved, expanded as Argon proposed:
  - agent and CLI tokens count as credentials (`~/.claude` credentials, `~/.claude.json`, `~/.codex/auth.json`, `~/.gemini/`, `~/.config/gcloud/`, `~/.git-credentials`, `~/.npmrc`), as does every private key in `~/.ssh/`; writing `~/.ssh/config` or `authorized_keys` is blocked;
  - force-push covers `dev`, `develop` and `trunk`, and any force-push without an explicit non-protected destination;
  - pipe-to-shell covers command and process substitution (`bash -c "$(curl …)"`, `sh <(curl …)`, `eval`, `source`);
  - Atrium's own data (database, WAL, snapshots, logs) can't be read either, and its config, service units and app binary can't be written;
  - privilege escalation adds `pkexec`, `run0` and `osascript … with administrator privileges`;
  - in Home, `.` and every folder above `~` count as outside the thread's folder.
- **Home's auto-allowed commands** (`HOME_READONLY_COMMANDS`): `ls`, `pwd`, `cat`, `head`, `tail`, `wc`, `file`, `stat`, `du`, `df`, `which`, `uname`, `date`, `whoami`, `tree`, `rg`, `grep`, and `git status|log|diff|show|branch|rev-parse|ls-files`. Never auto-allowed: anything with `$` expansion, globs, brace expansion, a newline, or `git -c`/`--config-env`/`--exec-path`. The always-blocked list applies on top.
- **Codex in Home** uses `sandbox=read-only` with `approval_policy=untrusted`, as verified in M0 (not `on-request`, under which Codex reads without asking).
- **Whole-home search asks**: in Home, a recursive search rooted at `~` itself (no subfolder) needs approval. Searches in a subfolder stay auto-allowed.

## D-051 — 2026-11-09 — M1 contracts approved and frozen (revision 3)
**Decided by Pedro (driver, owner per D-013).** `docs/contracts/m1.md` revision 3 and its code (protocol rev 3, `atrium-agents` `lib.rs` and `fixture.rs`, the `atrium-core` and `atrium-host` signatures, the schema and test stubs, commit `10b72bf`) are approved and frozen, after three Argon reviews (`reviews/M1-contracts.md` rework → `M1-contracts-rev2.md` rework → `M1-contracts-rev3.md` approve with fixes, fixes applied). This includes the security defaults revision 3 adds on top of D-050:
- Home's auto-allowed commands never match a line with a redirect other than fd duplication or output to `/dev/null`, or a leading `NAME=value`; they forbid `-f` for `file`, `rg` and `grep`, `tree -o/--output/-R` and `rg --hostname-bin`. `rg` and recursive `grep` ask unless every search root is a file or a folder strictly inside `~`.
- Reading `~/.ssh/config` is allowed (only writing it, and `authorized_keys`, is blocked), as D-050 said.
- On Linux, App authority needs `SO_PEERPIDFD` (kernel 6.5+); there is no `pidfd_open` fallback.
- Known limits stated in Settings: auto-allowed git reads trust each repository's own git config; exports turn the home path into `~` and keep the user name.

Changes from now on go through `docs/agents/change-requests.md`. Dispatching M1 workers needs its own go.

## D-052 — 2026-11-09 — CR-05 approved: documented macOS API for peer verification
**Decided by Pedro (driver).** Contract §4 now names `SecCodeCopyGuestWithAttributes` with `kSecGuestAttributeAudit` instead of the undocumented `SecCodeCreateWithAuditToken`, and states that unsigned dev builds compare the running (dynamic) code object's cdhash, never the static code on disk. No signature or behaviour change. macOS 11 is the stated minimum. From Sonnet's and Opus's M1-10 reviews; M1-10 implements it in its fix round.

## D-053 — 2026-11-09 — Claude fallback when Argon runs out of usage
**Decided by Pedro (driver).** Argon's usage is running low. If an Argon run fails (quota or otherwise), the task continues on a Claude model instead of waiting:
- reviews of Sonnet's work (M1-03's security review) go to Opus 5.5: still a fresh, independent reviewer, though not cross-provider;
- worker and fix rounds (M1-10's fixes) go to Sonnet 5.5, with Opus re-auditing any changed `unsafe` block.
The work done so far on the branch is kept; the Claude model picks up from the branch and the reviews.

## D-054 — 2026-11-09 — CR-06 approved, option (a): remembered command rules match exactly
**Decided by Pedro (driver, owner per D-013)**, on the Opus security review of M1-03 (`reviews/M1-03-policy.md` findings 3 and 12):
- "Always…" on a command now creates `RuleMatcher::CommandExact` (the whole argv). A remembered `rm -rf build` no longer allows `rm -rf build ~/Documents`. `CommandPrefix` rules can be created only in the rules editor.
- In `HOME_READONLY_COMMANDS`, `date` also forbids `-f`/`--file`, because GNU `date -f FILE` prints FILE's lines. This is stricter than D-050/D-051.
Applied to `atrium-protocol` (`policy.rs`, the serde shape test, `bindings/protocol.ts`) and contract §5 and §9. M1-03 implements it in its fix round. The M1-07/M1-08 approval UI shows the exact command.

## D-055 — 2026-11-09 — The orchestrator makes all changes inline (trial of Claude Fable 5)
**Decided by Pedro (driver).** From this shift, the orchestrator writes every change itself, in its own session, instead of fanning tasks out to workers. The orchestrator on Pedro's Mac is now Claude Fable 5 (`claude-fable-5-2`), and Pedro wants to see how it does. He accepts that this is slower than parallel workers.
- While this holds, it replaces the fan-out step of PLAN §8 and the worker routing of D-043, D-048 and D-053 on Pedro's shifts. It doesn't bind rodrigo's shifts.
- Unchanged: the frozen M1 contract and the change-request process, an explicit go from the driver for each phase, work on `dev` only, and the push policy (D-005, D-007).
- **Still open:** the go for the second-wave tasks, and who reviews work the orchestrator wrote itself (PLAN §8.2 wants a reviewer from another provider).

*Update, 2026-11-10:* both open points are settled by D-056.

Housekeeping the same day, asked for by Pedro: the stale `claude/design-ui-direction-6c0810` worktree and branch (content already on `dev` as `ea3a1f3`) and the five merged first-wave `agent/*` branches were removed. They existed only on Pedro's Mac.

## D-056 — 2026-11-10 — Go for the second wave of M1, inline, with a Flash review per task
**Decided by Pedro (driver).** Pedro gave the go to start the second wave. Under D-055 the orchestrator (Claude Fable 5) writes each task itself, one at a time, in this order: M1-11 (history export), M1-06 (client), M1-05 (`atriumd`), M1-07 and M1-08 (Claude and Codex adapters), M1-09a and M1-09b (app shell, Home chat), M1-13 (adversarial tests, with the M1-03 leftovers).
- **Review:** every task is reviewed read-only by **Gemini 3.8 Flash** (`agy`, through `scripts/agent-run`) before it's merged into `dev`. That keeps the review cross-provider (PLAN §8.2). It replaces the reviewers in contract §15 and D-053 on Pedro's shifts while D-055 holds.
- Each task still gets a short brief (scope, owned paths, Definition of Done), which is what the reviewer checks against, and its own branch `agent/fable/<task-id>` and worktree, merged after the review.
- **Still open, before M1-07:** whether remembered command rules are bound to a folder (`path_scope`) or stay global with "in any folder" in the approval UI (`reviews/M1-03-policy-round2.md`, finding h).

## D-057 — 2026-11-10 — rodrigo's shift: the orchestrator writes inline too, with a Flash review per task
**Decided by rodrigo (driver).** D-055 and D-056 now hold on rodrigo's shifts as well: the orchestrator (Claude Fable 5, on rodrigo's machine) writes each second-wave task itself, in D-056's order, and Gemini 3.8 Flash reviews each one read-only before it's merged into `dev`. This replaces the contract §15 worker routing on rodrigo's shifts while it holds.
- rodrigo's go covers continuing the second wave from where Pedro's shift stopped: M1-06's review and merge, then M1-05.
- **Put off, by rodrigo:** the Linux leftovers (M1-10's Linux paths run live, Codex's live checks X1–X3 from D-047). They stay owed on a later rodrigo shift and don't block the second wave.

## D-058 — 2026-11-10 — CR-07: points 2 and 3 approved, point 1 put off until after the M1 gate
**Decided by rodrigo (driver), who left the choice to the orchestrator; Pedro was present.** The reasons are the orchestrator's.
- **Point 3, approved (wording only).** `wire.rs` states that once the daemon has answered a `Subscribe` or processed an `Unsubscribe`, it sends no event, `ReplayDone` or `SubscriptionLagged` of an earlier subscription to that thread on that connection, frames already queued included. A `Subscribe` whose cursor is beyond the thread's last event is answered with `ErrorCode::Invalid`; an unknown thread with `UnknownThread`. The client already depends on this, so M1-05 must do it either way; M1-13 tests it.
- **Point 2, approved.** New `ServerMessage::ThreadRemoved { thread_id }` on the control lane, sent to inbox subscribers when a thread is archived, whoever archived it. It is one small additive message, and without it a second window or a later CLI leaves a dead thread in the list until the next reconnect.
- **Point 1, put off.** No `ReadEvents` in M1. It changes the `Store` trait, `SqliteStore`, the daemon and the client's cache, and M1 works without it: a thread loads by replay and the client keeps up to 64 MiB of folded history per thread, which is far more than a normal thread. The command is additive, so it can come later without breaking M1 clients. Looked at again after the M1 gate, with the measured load time of a large thread against the real daemon (contract §14).
- Applied on `dev` after M1-06 is merged and before M1-05's brief: `wire.rs`, the serde shape test, `bindings/protocol.ts`, the contract's wording, and the client's handling of `ThreadRemoved`.

## D-059 — 2026-11-11 — Go for M1-09a, M1-09b and M1-14, ahead of M1-07 and M1-08
**Decided by rodrigo (driver).** rodrigo gave the go for the app shell (M1-09a), the Home chat UI (M1-09b) and the Linux task (M1-14), in that order. This changes D-056's order: the adapters (M1-07, M1-08) come after. The dependencies allow it (contract §15: M1-09a needs M1-06, M1-09b needs M1-09a, M1-14 needs M1-05 and M1-09a; all merged or in this go).
- **How:** as D-057: the orchestrator (Claude Fable 5) writes each task inline on its own branch and worktree, and Gemini 3.8 Flash reviews it read-only, with screenshots as evidence (D-048's review rule), before it's merged into `dev`.
- **What it means for M1-09b:** until M1-07 and M1-08 are merged, `atriumd` has no real provider. The Home chat is built and tested against the daemon's `AgentLaunch::Fake` agent and the client's scripted-daemon tests. A live chat with Claude Code or Codex is checked when the adapters land; that check stays owed for the M1 gate.
- **Open questions of `docs/design/ui-direction.md` §9, settled in the M1-09a brief by the orchestrator** (the driver can overturn them): Instrument Sans and Geist Mono stay the bundled defaults; Home is a chat like any other, pinned as a tile; Linux gets client-side window controls on the right of the same frame; topic management (§4) is not in M1, whose threads are all Home threads, and the sidebar shows one list.
- **Not covered by this go:** M1-07, M1-08, M1-08L, M1-13, and the decisions still waiting (CR-08, CR-09, CR-10).

## D-060 — 2026-11-12 — Model routing while Argon and Codex are out of usage
**Decided by Pedro (driver).** Argon and Codex are both out of usage. Until that changes, work routed to them, or to the other GPT models, goes to a Claude agent or to Flash:
- **Gemini 4 Argon** → a Sonnet 5.5 agent. This extends D-053 to every Argon job.
- **Codex** (`codex exec`, any model) → a Sonnet 5.5 agent.
- **GPT-6 Astra** → Opus 5.5.
- **GPT-6.1 Sol** → Sonnet 5.5, or Gemini 3.8 Flash through `agy`.
- Gemini 3.8 Flash reviews through `agy` (D-056, D-057, D-059) are unchanged.

Pedro took over from rodrigo's shift (2026-11-12) and asked to continue where it stopped: finish M1-09a under D-059.

## D-061 — 2026-11-12 — CR-11 approved: `unsafe` in the app's macOS module
**Decided by Pedro (driver).** `unsafe_code` is allowed in `app/src-tauri/src/macos.rs` only (macOS-only module, `#![allow(unsafe_code)]`); the workspace lint stays `deny` everywhere else. Every block carries a `// SAFETY:` comment. Today there are four: three dereferences of Tauri's `NSWindow` pointer (`set_alpha`, `apply_to_new_window`, `window_number`) and the accessibility observer registration. An Opus 5.5 audit of the four blocks is required before M1-09a is merged (as for M1-10, D-053). Any new `unsafe` in the app needs its own change request.

## D-062 — 2026-11-12 — Parallel workers again on Pedro's shifts; go for M1-09b, M1-07 and M1-13
**Decided by Pedro (driver).** Pedro asked for parallel work (up to six agents at once, contract §15's cap) and chose which tasks start now.
- **D-055 ends on Pedro's shifts.** The orchestrator dispatches workers again (PLAN §8 fan-out), routed by D-060. It still writes small fixes and integration itself.
- **Go:**
  - **M1-09b-home-chat**: Opus 5.5 worker (contract §15, D-048), reviewed by Gemini 3.8 Flash with screenshots. It starts from M1-09a's branch tip before M1-09a is merged and rebases onto `dev` after M1-09a's merge.
  - **M1-07-claude-adapter**: Sonnet 5.5 worker. Reviewed by Gemini 3.8 Flash (cross-provider, D-056) instead of Argon.
  - **M1-13-adversarial**: Sonnet 5.5 worker (Argon → Sonnet, D-060), reviewed by Opus 5.5 (contract §15). It includes the M1-03 and M1-05 leftovers listed for it.
- **Not in this go:** M1-08 (Codex adapter), M1-08L, M1-14 (Linux, needs a Linux machine; go already given in D-059).
- **Remembered command rules stay global** (D-056's open point), with "in any folder" shown in the approval UI. This is the orchestrator's default, given with the M1-07 option; Pedro can overturn it before M1-09b's approval UI is reviewed.

## D-063 — 2026-11-12 — OpenCode gets its own adapter, ahead of the generic ACP adapter
**Decided by Pedro (driver).** OpenCode is the provider Pedro will actually use. It settles M0-04's open design point (O11): OpenCode gets its **own adapter over `opencode serve`** (HTTP API, per-session permission rules, model and `variant` per prompt), instead of going through the generic ACP adapter. The generic ACP adapter stays in the plan for Goose and other ACP agents, after OpenCode.
- **When:** M2, as planned for ACP (PLAN §5), OpenCode first. Not pulled into M1: the M1 contract is provider-agnostic (`AgentAdapter`, per-session enforcement, the gate), so the adapter is additive and needs no change to the frozen contract, the protocol or the UI.
- The M0-04 fixtures and the must-fix items O1–O10 carry over where they still apply. Free hosted OpenCode models stay `untrusted` by default (PLAN §3.21).
- PLAN.md updated: providers table, architecture sketches, §3.2, §5 (M2), the open questions, the routing row, the milestone table.

## D-064 — 2026-11-12 — D-060 and D-062 bind Pedro's shifts only
**Decided by Pedro (driver).** The usage limits behind D-060 (Argon and Codex out of usage) are Pedro's accounts, not rodrigo's. So:
- **D-060's model routing** and **D-062's way of working** (parallel workers routed to Claude, Flash and Opus reviews in place of Argon and Codex) apply **on Pedro's shifts only**.
- **On rodrigo's shifts the original workflow holds**: contract §15's routing and reviewers (Argon, Codex/Sol, Astra where they're named), PLAN §8, and rodrigo's own decisions (D-057, D-059). This stays true unless rodrigo decides otherwise and records it here.
- What D-062 decided about tasks still holds for everyone: the go for M1-09b, M1-07 and M1-13, and remembered rules staying global. On a rodrigo shift, the remaining work and reviews of those tasks follow his routing.

## D-065 — 2026-11-12 — rodrigo's shift: parallel workers again, Fable 5.2 takes Opus 5.5's role
**Decided by rodrigo (driver).** The Fable inline trial is over on rodrigo's shifts as well.
- **D-057 ends.** The orchestrator dispatches workers again (PLAN §8 fan-out, up to six agents at once per contract §15). It still writes small fixes and integration itself. D-059's go and task order stand; only who writes the code changes.
- **Claude Fable 5 (`claude-fable-5-2`) takes every role that Opus 5.5 holds** on rodrigo's shifts: the orchestrator (D-001), the UI worker (D-048, at most one at a time besides the orchestrator), the `unsafe` audits (D-053, D-061), the escalation and contract-heavy worker (D-047), and the reviews contract §15 or D-062 give to Opus (M1-13's review).
- **Everything else follows D-064:** contract §15's routing and reviewers (Sonnet 5.5, Argon, Codex/Sol, Astra, Flash) on rodrigo's accounts. A worker that hits a usage limit is paused, not silently re-routed (D-047). PLAN §8.2's cross-provider rule is unchanged: Fable is Anthropic, so its work is reviewed by Gemini or GPT models, not by Sonnet or Opus.
- **In-flight tasks on rodrigo's shifts:** M1-09b continues with a Fable worker from `agent/opus/M1-09b-home-chat` (new branches named `agent/fable/<task-id>`); M1-13's review goes to Fable. Pedro's shifts are unchanged (D-060, D-062, D-064).

## D-066 — 2026-11-12 — rodrigo's go: continue M1-09b, M1-07 and M1-13 with workers; brief for M1-14
**Decided by rodrigo (driver).** rodrigo gave the go for the wave the orchestrator (Claude Fable 5.2) proposed at the start of the shift, routed by D-065:
- **M1-09b-home-chat**: a Fable 5.2 worker (Claude subagent) continues from the partial handoff. Review: Gemini 3.8 Flash with screenshots. On this machine the GUI runs, the screenshots and the benches are on Linux; the macOS screenshots and the macOS startup gate of its brief stay owed to a Pedro shift.
- **M1-07-claude-adapter**: a Sonnet 5.5 worker (Claude subagent) finishes it. Review: Gemini 4 Argon, as contract §15 says (Flash was D-062's stand-in on Pedro's shifts).
- **M1-13-adversarial**: Gemini 4 Argon through `agy` (contract §15) writes the second part: the daemon and host adversarial tests, and the engine fixes for findings 1, 2, 7 and 8 (S-items in `m1-must-fix.md`), which change no contract and no default. Review: Fable 5.2 (D-065). Finding 6 stays with CR-12 (Pedro, D-013).
- **M1-14-linux**: the orchestrator writes the brief now. Its dispatch waits until M1-09b is not using the GUI lock, and is covered by D-059's go.
- **Branches:** the three tasks continue on their existing branches (`agent/opus/…`, `agent/sonnet/…`) whatever model works on them. This is the orchestrator's reading of D-065's note on branch names, chosen to avoid deleting pushed branches; the driver can overturn it.
- **Before any `agy` worker:** the hook check of `HANDOFF.md` is re-run, because `agy` here is 1.5.7 and the tested version is 1.5.3.
- **Not covered by this go:** M1-08, M1-08L, and the decisions that wait for Pedro (CR-09, CR-12) or a driver (CR-08, CR-10).

*Update, same day (orchestrator):*
- The `agy` 1.5.7 hook check passed (Flash, scratch worktree): `cargo --version` allowed, a write to `.agents/` and `git push` denied by the hook. `scripts/doctor` now names 1.5.7 as the tested version.
- M1-14 is split, as M1-09 was: **M1-14a-linux-host** (M1-10's four Linux review findings, a live run of the Linux paths, L8; no GUI, no app file; Sonnet 5.5, reviewed by Argon) is dispatched now under D-059's go; **M1-14b-linux-app** (L1, L2 harness, L3 profiling, M1-09a's and M1-09b's runs on Linux; Fable 5.2, reviewed by Flash) waits for M1-09b's merge, because M1-09b owns `app/src/**` and `app/scripts/**`.

## D-067 — 2026-11-13 — CR-08 and CR-10 approved; the stale M1-09a worktree removed
**Decided by rodrigo (driver).**
- **CR-08, approved:** `Store::launch(&ThreadId) -> StoreResult<Option<NewThread>>`, so a resumed thread keeps the effort and preset it was started with. M1-05's workaround (Home preset, no effort on resume) goes when it lands.
- **CR-10, approved:** `Store::unfinished_incarnations() -> StoreResult<Vec<(ThreadId, u32)>>`, so recovery closes an unfinished incarnation of an archived thread and can clear a process record whose incarnation already ended. M1-13's finding 12 (a failed closure in recovery leaves `sessions.process` set) is looked at in the same change.
- Both add a method to the frozen `Store` trait and change no existing one. They are applied on `dev` as one task (brief `M1-15-store-launch-recovery`), after M1-13 is merged, because it touches `atrium-daemon`'s recovery, which M1-13's new tests cover.
- **Housekeeping, asked for by rodrigo:** the worktree `~/atrium-wt/M1-09a-app-shell` on rodrigo's machine was removed on 2026-11-13 (`git worktree remove`). M1-09a is merged into `dev` (`1d8d9ca`); the worktree was clean when removed (the untracked `skills/` folder seen in it on 2026-11-12 was already gone). The local branch `agent/fable/M1-09a-app-shell` (at `b35ea1c`, eight commits behind its pushed branch) and `origin/agent/fable/M1-09a-app-shell` were left as they are.

## D-068 — 2026-11-13 — Gemini 4 Hydrogen takes Gemini 3.8 Flash's place
**Decided by rodrigo (driver).** Gemini 4 Hydrogen, through `agy`, is used wherever the plan or an open task names Gemini 3.8 Flash: the high-volume utility work of PLAN §8.1 and §8.2 (tests, fixtures, docs, exploration, triage), the cheap end of the escalation ladder (§8.3), and the cross-provider reviews given to Flash (contract §15, D-056, D-057, D-059, D-066: today M1-09b's and M1-14b's reviews, with screenshots).
- **Model id:** `gemini-4-hydrogen-medium` (`agy models` also lists `low`, `high` and `xhigh`). Medium is the orchestrator's default, as it was for Flash; the driver can change it.
- **Updated:** PLAN §8 (roster, routing table, ladder, limits, the launch table), the briefs of M1-09b and M1-14b, the status board, `routing.md`.
- **Left as written:** the frozen M1 contract's task table, earlier decisions, finished briefs, reviews and session logs. Where one of them gives Flash a role that is still open, it reads Hydrogen.
- **Not changed:** PLAN §3.17's example of Atrium's own default routing (`agy:gemini-3.8-flash-medium`), which is the product's default and not this build's roster.
- **Limits and policy:** Hydrogen takes Flash's limit of four at once (§8.4). The worker policy hook depends on the `agy` version, not on the model, so the 1.5.7 check (D-066) stands; Hydrogen's first run is watched for tool names the policy doesn't know.
- **Pedro's shifts:** his own decisions that name Flash (D-056, D-060, D-062) are his to change, as D-064 says for routing. Until he does, Flash stays available on his shifts.
- Hydrogen has no scoreboard yet: `routing.md` starts a row for it, and its first reviews are read with that in mind.


## D-069 — 2026-11-14 — rodrigo's shift: Opus 5.5 is the orchestrator again; more work goes to Gemini 4 Hydrogen
**Decided by rodrigo (driver).** rodrigo asked to go back to the original orchestration set-up, change nothing else, and give Gemini 4 Hydrogen every task the orchestrator judges it can handle.
- **D-065's model swap ends.** Claude Opus 5.5 (`claude-opus-5-5`) takes back, on rodrigo's shifts, every role D-065 gave Claude Fable 5.2: the orchestrator (D-001, PLAN §8.1, §8.7), the UI worker (D-048, at most one at a time besides the orchestrator), the `unsafe` audits (D-053, D-061), the escalation and contract-heavy worker (D-047), and the reviews given to Opus (contract §15, D-062). This is PLAN §8 as written. Fable 5.2 has no standing role on rodrigo's shifts; the driver can bring it back.
- **The rest of D-065 stands:** parallel workers (D-057 stays ended), routing by contract §15 and D-064, a worker that hits a usage limit is paused, not re-routed. PLAN §8.2's cross-provider rule: Opus is Anthropic, so its work is reviewed by Gemini or GPT models. D-066, D-067 and D-068 stand. Open tasks keep their branches; a new Opus branch is `agent/opus/<task-id>`.
- **Gemini 4 Hydrogen (`gemini-4-hydrogen-medium`, through `agy`), on rodrigo's shifts** (the orchestrator's judgment): besides its PLAN §8.2 rows (tests, fixtures, docs, exploration, triage) and the reviews D-068 gave it (M1-09b, M1-14b):
  1. **M1-14a's fix round 1, close-out.** The Sonnet worker stopped at the account's 5-hour limit after findings 1–3 (`823aa7e`..`2cd28c3`). Hydrogen does the review's finding 4 (`perf.rs`'s 60 s idle window holds the daily `VACUUM INTO` snapshot, so the L8 samples are not 60 s after the last activity: fix the window and take the L8 numbers again, raw output in the report), re-runs the DoD and writes the handoff's fix-round section. The nit (`PF_FORKNOEXEC` for `PF_RANDOMIZE`) is already fixed (`2cd28c3`). Argon's re-review follows, as D-066 says.
  2. **M1-13's third part, its tables (step 5 of the brief).** Before the fixes, Hydrogen writes the regression table (every input of `reviews/M1-13-adversarial.md` with the verdict the review expects) and the table of ordinary lines with the verdict each has on `dev`, on the task's branch, test files only. They fail where the review says the branch is wrong. The Opus worker then does the fixes and checks the tables, as the owner of the code under test (§8.2). Worker rule 6 holds.
  3. **The M1-05 late-fix review's test gaps** (`reviews/M1-05-daemon-late-fixes.md`: the full-storage test can't tell the new rule from the old; the `write_loop` gaps): a tests-only task in new `crates/atrium-daemon/tests/` files. It needs a brief and the driver's go.
  4. **Orchestrator utility work:** triage of failing check output, codebase questions, first drafts of summaries. The orchestrator checks what comes back.
- **Not Hydrogen's** (security, contracts, frontier or specialist work): M1-07's fix round 2 (Sonnet; left: new finding 7 and what follows it), M1-13's fixes (Opus) and its re-review (Argon), M1-15 (Sonnet, Argon review), M1-14b (Opus), M1-09b's remaining UI work and the fixes its screenshots call for (Opus), `unsafe` audits, contract changes, gate reviews.
- Hydrogen has no scoreboard yet: its first run (M1-14a's close-out) is watched for tool names the policy hook doesn't know (D-068), and `routing.md` gets a row per task.
- **Pedro's shifts are unchanged** (D-060, D-062, D-064).
- **Not a go:** this decision routes work. Items 1 and 2 are parts of tasks D-066 already covers, item 3 is new; the orchestrator dispatches none of them until the driver says so.

## D-070 — 2026-11-14 — rodrigo's go: resume the paused workers, Hydrogen's first task included
**Decided by rodrigo (driver).** The three workers stopped by the usage limits on 2026-11-14 resume, routed by D-069:
- **M1-09b-home-chat:** an Opus 5.5 worker (Claude subagent) continues from `c884a76` and the handoff on the branch.
- **M1-07-claude-adapter:** a Sonnet 5.5 worker finishes fix round 2: item 7 (`live` checked before `Redeem`/`Question` are fed; separate id namespaces), item 8 (the five helper keys count as command keys), the DoD and the handoff. Then Argon's review round 3.
- **M1-14a-linux-host:** Gemini 4 Hydrogen (`gemini-4-hydrogen-medium`, `scripts/agent-run agy`) closes fix round 1 (D-069 item 1). Its first run is watched. Then Argon's re-review.
- **Not in this go:** M1-13's third part (Hydrogen's tables and the Opus fixes), the M1-05 test-gap task, M1-14b, M1-15.

*Update, same day (orchestrator):* `agy` on the host is now 1.5.8. Before the Hydrogen worker, the hook check of `HANDOFF.md` was re-run with Hydrogen in a scratch worktree (`M1-00-hookcheck-1.5.8`, removed afterwards): `cargo --version` allowed, a write to `.agents/probe.txt` and `git push` denied by the hook, no file written. `scripts/doctor` names 1.5.8 as the tested version. Hydrogen used the tool name `write_file`, which the policy already knows.

## D-071 — 2026-11-14 — M1-07 escalates to Gemini 4 Argon, reviewed by GPT-6.1 Sol
**Decided by rodrigo (driver).** M1-07's review round 3 is rework again (`reviews/M1-07-claude-adapter-round3.md`): the `Glob`/`Grep` subject, open since round 1, still reaches Allow on credentials (two blockers, one major). Both automatic fix rounds are used, so the task escalates across providers (PLAN §8.3): the author was Sonnet 5.5, the next attempt is **Gemini 4 Argon** (`gemini-4-argon-high`, through `agy`), and its work is reviewed by **GPT-6.1 Sol** (`gpt-6.1-sol`, `codex exec -s read-only`). The orchestrator offered Astra (reviewed by Argon) as its first choice; rodrigo chose Argon, who found the holes.
- **Same branch and worktree** (`agent/sonnet/M1-07-claude-adapter`, D-066's reading on branch names), same owned paths.
- **The orchestrator's design rule for the subject** (in the brief's "Escalation" section): stop expanding and bounding arbitrary patterns; accept only a small, explicit grammar and make everything else `Unknown` (which asks). Ordinary patterns (`**/*.rs`, `src/**/mod.rs`, `*.{ts,tsx}`, `src/{a,b}/mod.rs`, `*.[ch]`, `*.[a-z]`) must keep their Allow in Home.
- The policy engine's side (P1 in `m1-must-fix.md`) stays with `atrium-core`'s owner.

## D-072 — 2026-11-14 — M1-09b merged into `dev` with its macOS checks owed
**Decided by rodrigo (driver).** M1-09b-home-chat is merged into `dev` at `8e7dffd` after the Gemini 4 Hydrogen review (round 1 approve with changes, fixed by the orchestrator; round 2 approve). Its status is partial only for what this machine can't do:
- **Owed to a Pedro shift (macOS):** the screenshots in both themes, the `startup_bench.py` gate (median < 400 ms, p90 < 450 ms, idle < 150 MB `phys_footprint`) and the §14 streaming run.
- **Owed to M1-07 (a real provider):** approvals and questions end to end, "Always…" from a live request, the daemon's stale refusal, Resume, real markdown and usage, a probed agent in Settings.
- **Linux idle memory over 150 MB is not this task's:** M1-09a's shell measures 219–223 MiB private PSS on the same machine and script (M1-09b: 212–216 MiB). It goes to M1-14b (L3 profiling) and a gate decision.
- **Open for the drivers, not decided here:** how §14's frame statistic reads on Linux (144 Hz display, 1 ms rAF timestamps; the worker proposes "frames that skip an update", 0.1–0.3 % measured) and batching events in `bridge.rs` (about 10,000 webview script runs a second at 10k tokens/s).
- The merge unblocks M1-14b (it waited for M1-09b's `app/src/**` and `app/scripts/**`); its dispatch still needs a go.

## D-073 — 2026-11-14 — Pedro's shifts: Gemini 4 Hydrogen takes the Flash and Argon jobs
**Decided by Pedro (driver).** In Pedro's tests Gemini 4 Hydrogen performs better than Gemini 4 Argon in most cases. On **Pedro's shifts**:
- **Every `agy` Flash job goes to Hydrogen** (`gemini-4-hydrogen-medium`, through `scripts/agent-run agy`). This confirms D-068 for his shifts.
- **Every Gemini 4 Argon job goes to Hydrogen too**: work, reviews and escalations. This replaces the Argon → Sonnet 5.5 routing of D-053 and D-060 on his shifts. The orchestrator picks the effort level per job: `medium` by default, `high` for security, `unsafe` and escalation work, which is what Argon used (`gemini-4-argon-high`, D-071).
- The rest of D-060 is unchanged on Pedro's shifts: Codex → Sonnet 5.5, Astra → Opus 5.5, Sol → Sonnet 5.5 or, where Flash was the alternative, Hydrogen.
- **Open tasks this changes, when they run on Pedro's shift:**
  - M1-07's escalation (D-071, Argon, reviewed by Sol) runs on Hydrogen `high`, reviewed by Sonnet 5.5 (Sol → Sonnet, D-060). Hydrogen and Sonnet are different providers, so the review stays cross-provider.
  - Argon's reviews of M1-15, of M1-14a's re-review if still open, and of M1-13's re-review go to Hydrogen.
- **Rodrigo's shifts keep his own routing** (D-064, D-066 to D-071) unless he decides otherwise.
- Hydrogen's limit is four at once (D-068). Its `routing.md` rows track how it does on the Argon-class work.

## D-074 — 2026-11-14 — Pedro's shifts: security-test writing goes to Claude, Hydrogen takes Claude work it can do
**Decided by Pedro (driver).** `scripts/agy-policy.py` stays as it is: it refuses path-like text (`~…`, `../`, absolute paths outside the worktree) in file contents too. So work that has to write such text goes to Claude workers. To balance the load, Gemini 4 Hydrogen takes Claude-bound work it can do within the policy. This refines D-073 on Pedro's shifts.
- **To Claude (they write credential paths as test data):**
  - **M1-07's escalation (D-071):** an **Opus 5.5** worker. The author was Sonnet, so this is a step up the ladder, not a change of provider.
  - **M1-13's third part:** the regression and ordinary-line tables go to a **Sonnet 5.5** worker, the fixes to **Opus 5.5** (as D-069).
- **Reviews of that work stay cross-provider: Gemini 4 Hydrogen `high`, read-only.** The review is returned as the agent's final message and the orchestrator writes it to `reviews/<id>.md`, because the policy would refuse a review file that quotes the inputs.
- **To Hydrogen, from Claude** (the orchestrator's judgment of what it can do: Rust or TypeScript work run with `cargo`/`pnpm`, no credential literals, no GUI):
  - **The M1-05 late-fix test gaps** (tests only, `crates/atrium-daemon/tests/`; `reviews/M1-05-daemon-late-fixes.md` findings 1 and 4). Needs a brief. Reviewed by Sonnet 5.5.
  - **M1-15-store-launch-recovery** (CR-08 and CR-10, once M1-13 is merged): Hydrogen `high` instead of Sonnet, reviewed by Opus 5.5. If its tests need path-like literals, that part goes back to Claude.
  - Hydrogen's PLAN §8.2 rows (fixtures, docs, exploration, triage) and the orchestrator's utility work, as on rodrigo's shifts (D-069).
- **Not Hydrogen's:** anything that needs `python3` or a GUI (the `agy` allowlist has neither), so M1-09b's macOS checks stay with the orchestrator; security tests and fixes (above); contract changes.

## D-075 — 2026-11-14 — Pedro's go: M1-07 escalation, M1-13 tables, M1-05b, M1-09b's macOS checks
**Decided by Pedro (driver).** Pedro said "start all" for the work D-074 routed:
- **M1-07-claude-adapter, escalation:** an Opus 5.5 worker on `agent/sonnet/M1-07-claude-adapter`, from the brief's "Escalation" section. Review by Gemini 4 Hydrogen `high`.
- **M1-13-adversarial, third part, step 5's tables:** a Sonnet 5.5 worker writes the regression and ordinary-lines tables, commits and stops. The Opus fixes (items 1–4 and 6) follow on the same branch once the tables are in, as part of this go.
- **M1-05b-daemon-test-gaps** (new brief): Gemini 4 Hydrogen `medium`, reviewed by Sonnet 5.5.
- **M1-09b's owed macOS checks (D-072):** run by the orchestrator on Pedro's Mac.
- Not in this go: M1-15 (waits for M1-13's merge), M1-14b (Linux), M1-08.

## D-076 — 2026-11-14 — Pedro's shifts: Gemini 4 Hydrogen first
**Decided by Pedro (driver).** Hydrogen is fast and cheap, so on **Pedro's shifts** it is the default worker. This extends D-073 and D-074; `scripts/agy-policy.py` is unchanged.
- **Every new task, review or follow-up goes to Gemini 4 Hydrogen** (`medium`; `high` for security, `unsafe` and escalation work) unless it falls under one of these, which keep their Claude routing (D-060, D-074):
  - it needs `python3`, a GUI or another command outside the `agy` allowlist;
  - it writes path-like text the policy refuses (credential paths as test data, security tables);
  - it is a security fix, an `unsafe` change or a contract change;
  - Hydrogen has already failed it (the PLAN §8.3 ladder goes to Claude next).
- **Reviews stay cross-provider:** Hydrogen reviews Claude's work; Claude (Sonnet 5.5 by default, Opus 5.5 for security) reviews Hydrogen's.
- **Work already running keeps its worker** (M1-07 escalation and M1-13 fixes on Opus, M1-05b on Hydrogen with a Sonnet review).
- At most four Hydrogen jobs at once (D-068). The orchestrator says in each brief header why a task did *not* go to Hydrogen when it doesn't.
- Rodrigo's shifts keep his own routing (D-064).

## D-077 — 2026-11-14 — Pedro's go: M1-05c-snapshot-seam
**Decided by Pedro (driver).** Go for the follow-up the M1-05b review named (`reviews/M1-05b-daemon-test-gaps.md` §5): a test-only store hook in the daemon's `Options` and a test of the snapshot-ordering rule, plus the review's two test nits. Gemini 4 Hydrogen `medium` (D-076), reviewed by Sonnet 5.5. Brief `M1-05c-snapshot-seam.md`.

## D-078 — 2026-11-14 — An uncertain cwd keeps dev's verdict under Any rules and FullAccess
**Decided by Pedro (driver).** M1-13's third part (`253c7f4`) makes Home ask when the engine can't know the cwd (after `;`, `||`, `pushd`, `popd`, `cd -`, `cd $VAR`), and prefix, exact and path-glob rules can't cover such a line. **`Any` rules and the FullAccess preset keep `dev`'s behaviour:** they still allow on an uncertain cwd, so a line like `cd -; cat id_rsa` is Allow under them. FullAccess and `Any` mean "don't ask me"; the hard denies on direct credential paths still apply. The orchestrator had recommended asking instead; Pedro chose to keep `dev`'s behaviour. The M1-13 handoff's "Left" item for the owner is closed by this decision.

## D-079 — 2026-11-14 — Pedro's go: M1-13 fourth round (rework) on Opus
**Decided by Pedro (driver).** The third part's re-review (`reviews/M1-13-adversarial-third-part.md`, Gemini 4 Hydrogen `high`) says **rework**: five blockers (inputs reaching Allow) and two majors (new false hard denies). Go for a rework round by the same Claude Opus 5.5 worker on `agent/sonnet/M1-13-adversarial` (security fixes stay with Claude, D-074), table test first for every input the review names, then a Hydrogen `high` re-review. The review's item 8 is settled by D-078 and is not reopened. The review's minor (fuzz bound) and nit (ignore reason wording) are in this round too.

## D-080 — 2026-11-14 — Pedro's go: M1-13 fifth round, another fix round on Opus
**Decided by Pedro (driver).** The fourth round's re-review (`reviews/M1-13-adversarial-round4.md`, Hydrogen `high`) says **rework**: seven blockers and one major family of false hard denies. The orchestrator offered a change of approach (a fail-closed shell grammar, with Any/FullAccess-only gaps documented as known limits); Pedro chose **another fix round** of the same kind, by the same Claude Opus 5.5 worker, table test first, then a Hydrogen `high` re-review.
- This goes past PLAN §8.3's two automatic fix rounds without changing provider: the next rungs (Argon, Astra) are out of use on Pedro's shifts, and Hydrogen can't write these tests through the `agy` policy (D-074). Recorded as a deliberate exception.
- The re-reviewer gets the previous reviews copied into its worktree's scratch area, since the policy refused the absolute path last time.

## D-081 — 2026-11-14 — Pedro's go: M1-13 sixth round, fix the root causes
**Decided by Pedro (driver).** The fifth round's re-review (`reviews/M1-13-adversarial-round5.md`, Hydrogen `high`) says **rework**: seven blockers (none reaching Allow in Home with no rules) and one major family of false hard denies (a compound command inside a `cd X && …` chain). The reviewer judged six of the seven to be neighbours of the previous fixes and listed seven mechanisms to generalise. Go for a sixth round by the same Claude Opus 5.5 worker that fixes each **mechanism**, not each spelling, with tables of neighbouring spellings, then a Hydrogen `high` re-review. Same exception to PLAN §8.3 as D-080.

## D-082 — 2026-11-14 — Pedro decides CR-09, CR-12, CR-13, CR-14, CR-15
**Decided by Pedro (driver).** None of these is dispatched yet: each needs a go for the task that implements it.
- **CR-09, approved.** In `attention::next_state`, `Signal::Recovered { cleanup: Unverified }` moves `Ended` and `Interrupted { cleanup: Verified }` to `Interrupted { cleanup: Unverified }` (`Failed` keeps its message). Contract §8 wording to change with it. Natural home: M1-15 (daemon recovery).
- **CR-12, Ask, not hard-deny** (the orchestrator recommended hard-deny). The credential stores outside `HARD_DENY_DEFAULTS` that M1-13 finding 6 lists (`~/.cargo/credentials.toml`, `~/.pypirc`, `~/.pgpass`, `~/.vault-token`, `~/.azure/accessTokens.json`, `~/.aws/sso/cache/*`, `~/.config/git/credentials`, shell history files, `~/.env`), and `git show`/`git cat-file` of a credential-named path, are **sensitive**: Home never auto-allows them and asks; a remembered rule may allow them; they are not hard-denied. Contract §9 and the Settings text that lists what is protected change with it. It touches `crates/atrium-core/src/policy/**`, so it is done after M1-13 merges. This closes the decision part of must-fix S1; the fix itself is still owed before the gate.
- **CR-13, approved as proposed.** `AgentLaunch::Fake` gains `approval_at_ms` and `question_at_ms`, both off by default, so the gate and live checks can drive approvals and questions without a real provider.
- **CR-14, approved as proposed.** `notify_approval`, `notify_question` and `notify_failed` are named in contract §8 and validated like `KEEP_RUNNING_SETTING`; the daemon skips a kind whose switch is `false`; the note in `app/src/ui/Settings.tsx` goes when it lands.
- **CR-15, approved: `DaemonCommand::DeclineQuestion { thread_id, incarnation, request_id }`**, checked by the ledger like `AnswerQuestion`, resolved as `RequestOutcome::Cancelled`, with the adapter passing the provider's own "dismissed" answer on; the question card gets a "Decline" button besides "Stop the turn".
- CR-13, CR-14 and CR-15 change the wire or the contract: they go through the protocol bindings check (`scripts/check-protocol-ts`) and a contract revision note.

## D-083 — 2026-11-14 — Pedro's go: M1-16 (CR-13, CR-14, CR-15); contract revision 3.1
**Decided by Pedro (driver).** Go for M1-16-requests-and-notifications: CR-13 (the fake agent raises an approval and a question), CR-14 (per-kind notification switches) and CR-15 (`DeclineQuestion`), as D-082 decided them. Gemini 4 Hydrogen `high` (D-076), reviewed by Sonnet 5.5. Brief `M1-16-requests-and-notifications.md`.
- The orchestrator wrote **contract revision 3.1** (`docs/contracts/m1.md`): the five CRs of D-082 in §6.2, §8 and §9, each marked "(rev 3.1)".
- **Orchestrator's call, for Pedro to overturn:** `PROTOCOL_VERSION` goes from 1 to 2, because `DeclineQuestion` is a command a version-1 daemon doesn't know; an app and a daemon of different versions then meet as `Incompatible` and §10's replacement runs, instead of the command failing at random.
- CR-09 goes into M1-15; CR-12 into a policy task after M1-13 merges.

## D-084 — 2026-11-15 — rodrigo's shifts adopt Pedro's routing (D-073, D-074, D-076)
**Decided by rodrigo (driver).** rodrigo asked to copy Pedro's agent routing and use it on his own shifts. From this shift on, **rodrigo's shifts route work exactly as Pedro's do**:
- **Gemini 4 Hydrogen first** (D-076): every new task, review or follow-up goes to Hydrogen through `scripts/agent-run agy` (`medium`; `high` for security, `unsafe` and escalation work), unless it needs `python3`, a GUI or another command outside the `agy` allowlist; writes path-like text the policy refuses; is a security fix, an `unsafe` change or a contract change; or Hydrogen already failed it. Those keep their Claude routing.
- **Hydrogen takes the Flash and Argon jobs** (D-073), work, reviews and escalations.
- **Security-test writing goes to Claude** (D-074); Hydrogen takes the Claude work it can do within the policy.
- **The rest of D-060 that D-073 kept**: Codex → Sonnet 5.5, GPT-6 Astra → Opus 5.5, GPT-6.1 Sol → Sonnet 5.5 or Hydrogen. rodrigo's accounts are not out of usage, so he can bring Codex, Sol or Argon back for a task by saying so.
- **Reviews stay cross-provider:** Hydrogen reviews Claude's work; Claude (Sonnet 5.5 by default, Opus 5.5 for security) reviews Hydrogen's. At most four Hydrogen jobs at once (D-068).
- **Kept from rodrigo's own decisions:** Opus 5.5 is the orchestrator (D-069), parallel workers (D-065), a worker that hits a usage limit is paused, not re-routed.
- **Replaces** D-064's "the original workflow holds on rodrigo's shifts" and D-069's Hydrogen list; both shifts now follow the same routing. Pedro's later routing decisions do not carry over automatically: each driver records his own.

## D-085 — 2026-11-15 — rodrigo's go: M1-14b-linux-app
**Decided by rodrigo (driver).** Go for M1-14b-linux-app (brief `briefs/M1-14b-linux-app.md`, D-059, D-066), unblocked by M1-09b's merge (D-072).
- **Worker: Claude Opus 5.5** (Claude subagent), not Hydrogen: the task needs a GUI and `python3`, both outside the `agy` allowlist (D-076's first exception). Branch `agent/opus/M1-14b-linux-app` (D-069), worktree `../atrium-wt/M1-14b-linux-app`.
- **Review: Gemini 4 Hydrogen `medium`, read-only**, with the numbers and screenshots (cross-provider, D-084).
- **Added to the brief:** the Linux idle memory (M1-09a's shell 219–223 MiB, M1-09b 212–216 MiB private PSS, gate 150 MB) is part of L3: say where the memory goes and what is reachable.
- **Not in this go:** batching events in `bridge.rs`, the §14 frame-statistic wording on Linux and the idle-memory gate itself: those stay open decisions; the worker brings numbers for them.

*Update, same day (orchestrator):* `agy` on the host is now 1.5.9. Before the review, the hook check of `HANDOFF.md` was re-run with Hydrogen in a scratch worktree (`M1-00-hookcheck-1.5.9`, removed afterwards): `cargo --version` allowed, a write to `.agents/probe.txt` and `git push` denied by the hook, no file written; tool names `run_command` and `write_file`, both known to the policy. `scripts/doctor` names 1.5.9 as the tested version. A first review launch had started before the check; it was stopped within a minute, and its worktree was clean.

## D-086 — 2026-11-15 — rodrigo's go: M1-07b-proc-drop-linux (N7)
**Decided by rodrigo (driver).** Go for fixing N7: on Linux, a Claude adapter `Proc` dropped outside a runtime hangs its thread for ever, and `claude_adapter`'s test binary never ends. The brief is `briefs/M1-07b-proc-drop-linux.md`. **Gemini 4 Hydrogen `high`** (D-084: Rust run with `cargo`, no credential literals, no GUI; `high` for process-lifecycle work), reviewed by **Sonnet 5.5**, read-only. Branch `agent/hydrogen/M1-07b-proc-drop-linux`.

*Update, same day (orchestrator):*
- **The reviewer changed from Sonnet 5.5 to Opus 5.5.** The fix adds an `unsafe` `waitid` block in `atrium-host`, and D-053 gives every changed `unsafe` block there an Opus audit. Opus is still a different provider from the author.
- **Review verdict: approve** (`reviews/M1-07b-proc-drop-linux.md`).
- **The orchestrator re-ran the checks on the branch:** fmt, workspace clippy, `claude_adapter` 5/5, and `cargo test --workspace` (495 passed, 0 failed, 1 ignored). All clean.

## D-087 — 2026-11-15 — Pedro's go: M1-16 review and the owed macOS checks, in parallel
**Decided by Pedro (driver):** "do both paralel".
- **M1-16-requests-and-notifications review:** Claude Sonnet 5.5, read-only (cross-provider: the author is Hydrogen), on `99e227e`. It checks what the worker didn't run: the 20-run loops, `scripts/check-protocol-ts`, the bindings file, contract §6.2/§8 rev 3.1, the Claude decline's shape and owned paths.
- **macOS checks, run by the orchestrator (no worker):** `cargo test -p atrium-host -p atrium-agents` on `dev` (M1-07b's `terminate_blocking` on macOS); M1-09b's `startup_bench.py` and `stream_bench.py --runs 3` under `lockf`; the font-preload TTI check. The benchmarks run only while no build is going, so they wait for the reviewer's compiles.
- **Not in this go:** M1-13 (resume, redo or fail-closed), N8, the daemon's silent exit, M1-08.

## D-088 — 2026-11-15 — Pedro's go: resume M1-13's sixth round; M1-08 to Hydrogen
**Decided by Pedro (driver):** "resume round six of m13, also dispatch m1-08 to hydrogen".
- **M1-13 sixth round (D-081) resumes** from the work-in-progress commit `87d6679` (not reset to `5a8c57c`), by the same Claude Opus 5.5 worker, whose conversation is continued. The plan is the branch handoff's "To resume": finish items 2, 4 and 5, then 1, 6 and 7, the "Sixth round" tables, the DoD; then a Hydrogen `high` re-review. Same recorded exception to PLAN §8.3 as D-080 and D-081.
- **M1-08-codex-adapter: Gemini 4 Hydrogen `high`** (D-076; `high` because approvals are redeemed through the policy gate), replacing Argon in the brief. **Review: Opus 5.5, read-only** (cross-provider; Opus because of the gate path). Branch `agent/hydrogen/M1-08-codex-adapter`, worktree `../atrium-wt/M1-08-codex-adapter`. No live Codex run (the brief's X1–X3 stay for a live task).
- **Effect on D-087:** the macOS benchmarks wait until both builds are quiet.

## D-089 — 2026-11-15 — Pedro's go: M1-16 fix round
**Decided by Pedro (driver):** "Yes, fix round". The Sonnet review (`reviews/M1-16-requests-and-notifications.md`) asked two fixes before merge: the fake approval must run `echo synthetic` as the contract says, and a test must pin that Allow redeems the ticket (the "skip `gate.redeem`" mutation survives). The same Hydrogen `high` worker also takes the minors that are its to fix (the racy wait in `requests_fake.rs`, the two untested edges, a silent skip when `question_ticket` returns `None`, and the handoff's wrong "blocked" notes). The contract's missing `RequestResolved` detail stays with the orchestrator. The orchestrator checks the fixes (the mutation re-run) and merges; no second review unless the fixes go beyond these.

## D-090 — 2026-11-15 — Pedro's go: start M1-15 now, ahead of M1-13's merge
**Decided by Pedro (driver):** "start m1-15 for now".
- **M1-15-store-launch-recovery: Gemini 4 Hydrogen `high`** (D-074, D-076), **reviewed by Opus 5.5**, read-only. Branch `agent/hydrogen/M1-15-store-launch-recovery`, from `dev` after M1-16's merge. It carries CR-08, CR-10, M1-13's finding 12, N1–N3 and now **CR-09** (D-082), whose code is `attention::next_state` plus a daemon test.
- **The brief's "M1-13 merged" dependency is waived.** M1-13's adversarial tests (its recovery tests among them) aren't on `dev`, so the worker can't run them. When M1-13 merges, the orchestrator runs them against M1-15's branch before M1-15 merges. M1-15 doesn't touch `policy/**`, so the two branches shouldn't conflict in code.
- The brief was rewritten for this shift: the Linux-container rules became the `agy` policy's, and CR-09 was moved from "out of scope" to step 0.

## D-091 — 2026-11-15 — Pedro's go: the small items (contract erratum, N8, N9)
**Decided by Pedro (driver):** "lets fix those small itens".
- **Contract erratum (rev 3.1, §6.2):** the sentence on an adapter that can't dismiss a question named a `RequestResolved` detail that doesn't exist. It now says: `RequestResolved { outcome: Cancelled }`, then `Error { fatal: false }` saying the provider can't dismiss a question, then the turn is interrupted. No wire change. Written by the orchestrator, found by M1-16's review.
- **N8 → M1-07c-leader-sigkill: Claude Opus 5.5** (D-076's exception: process-signal code in `atrium-host`, possibly `unsafe`), **reviewed by Gemini 4 Hydrogen `high`**. Brief `briefs/M1-07c-leader-sigkill.md`.
- **N9 (new in `m1-must-fix.md`, the daemon's silent exit) → M1-17-daemon-start-refusal: Gemini 4 Hydrogen `medium`**, reviewed by Sonnet 5.5. Brief `briefs/M1-17-daemon-start-refusal.md`.
- **Not in this go:** the macOS benchmarks and font-preload A/B (D-087; they still wait for a quiet machine).

## D-092 — 2026-11-15 — Pedro's go: M1-13 seventh round
**Decided by Pedro (driver):** "Seventh round". The round-6 re-review (`reviews/M1-13-adversarial-round6.md`, Hydrogen `high`) says rework but converging: six blockers (none reaching Allow in Home with no rules), one major false hard deny. The same Claude Opus 5.5 worker fixes each by its invariant, not its spelling (the review's "Convergence assessment"), with tables of neighbouring spellings, then a Hydrogen `high` re-review. Same recorded exception to PLAN §8.3 as D-080, D-081 and D-088.

## D-093 — 2026-11-15 — Pedro's go: M1-08 fix round
**Decided by Pedro (driver):** "Yes, fix round". The Opus review (`reviews/M1-08-codex-adapter.md`: approve with changes) asked eight things before merge. The same Hydrogen `high` worker does them, after the orchestrator merged `dev` into its branch (`4af4b39`; the worker policy refuses `git merge`), because the branch predated M1-16 and doesn't compile on `dev` without handling `DeclineQuestion`. The orchestrator checks the fixes (re-running the two surviving mutations); a second Opus look only if the approval path changes beyond the review's asks.

## D-094 — 2026-11-15 — Pedro: review M1-15 and M1-17
**Decided by Pedro (driver).** After stopping the first M1-15 review ("dont"), Pedro said "yeah you can review both". M1-15 gets its Opus 5.5 review as D-090 planned, restarted from the beginning; M1-17 gets its Sonnet 5.5 review as D-091 planned. Both read-only, in scratch clones that are deleted afterwards.

## D-095 — 2026-11-15 — Pedro's go: M1-17 fix round; the daemon keeps its logs-folder check
**Decided by Pedro (driver):** "Fix, keep the check". The Sonnet review (`reviews/M1-17-daemon-start-refusal.md`) found that a start racing a daemon already starting now parks the client in `Failed` (F1, major), and that the new `secure_dir(logs_dir)` in `Daemon::start` is a new refusal rather than the fix (F2). **Pedro keeps that check**: a daemon refuses an insecure logs folder however it was started (the orchestrator had recommended dropping it). The same Hydrogen `medium` worker fixes F1 and the minors F3–F5, and corrects the handoff's diagnosis (the silence was the launcher's). The orchestrator checks the fixes and merges.

## D-096 — 2026-11-15 — Pedro's go: M1-13 eighth and last round
**Decided by Pedro (driver):** "make a eight last round". The round-7 re-review (`reviews/M1-13-adversarial-round7.md`) said rework (five blockers, one major); the orchestrator's check against `dev` showed none of the blockers is a regression (`dev` gives the same verdicts, 9 lines are stricter on the branch) and the only regression is the major M6 (a false hard deny on `ag --filename-pattern`/`ack --match`). The same Claude Opus 5.5 worker fixes M6 first, then the five blockers by invariant, runs the 20-run loops, then a Hydrogen `high` re-review. **This is the last fix round:** after it, whatever the review says, the orchestrator brings M1-13 to Pedro for a merge decision, with anything left written down as known limits (not regressions against `dev`) and a follow-up policy task. Same recorded exception to PLAN §8.3 as D-080 to D-092.

## D-097 — 2026-11-15 — Pedro's go: M1-15 fix round
**Decided by Pedro (driver):** "Yes, fix round". The Opus review (`reviews/M1-15-store-launch-recovery.md`: approve with changes) asked for: the recovery regression fixed (a non-archived thread stopped while its provider was starting, then a daemon crash, is never closed and can stay `Ended` with an unverified process) with a test; three behaviour changes documented (`SessionEnded` clears `process`; a stored fatal `Error` stops the session; an archived incarnation with no events is still never closed); two doc comments fixed. The same Hydrogen `high` worker does them; the minors it can fix in its paths too. The orchestrator checks the fix (the regression test must fail on `cc8978d`), resolves the contract text conflict with `dev` when merging (keep both sentences), and merges.

## D-098 — 2026-11-15 — Pedro's go: fix M1-13's major 5, then merge
**Decided by Pedro (driver):** "Fix M5, then merge". This ends M1-13's rounds (D-096).
- **The fix.** The round-8 review's only regression against `dev` is major 5: `searches_cwd` strips `rg`'s `-L`/`--follow` before pairing flags with values, so it swallows a value (`rg -e -L safe_sub`, `rg -rL pat safe_sub`, `rg -g -L pat safe_sub`, `rg -f -L safe_sub`, `rg --file -L safe_sub`, `rg -e --follow safe_sub`). With a folder linked into `~/.ssh` in the cwd, these become a hard deny.
  - The same Opus worker fixes only that, with a test per form.
  - The orchestrator checks that the test fails on `b336b2c` and that the six forms match `dev`'s verdicts, then merges and runs the full checks.
- **Known limits, not fixed before the merge.** Blockers 1–4 of `reviews/M1-13-adversarial-round8.md` are marked "same as dev" by the reviewer. They are recorded as known limits for a follow-up policy task, together with CR-12 and P1.

## D-099 — 2026-11-15 — Pedro's go: M1-18 policy follow-up
**Decided by Pedro (driver):** "Start policy task". M1-18-policy-followup (`briefs/M1-18-policy-followup.md`) takes:
- CR-12: the credential stores that ask (contract §9 rev 3.1);
- P1: search globs that leave their roots;
- K1–K4: M1-13's round-8 blockers 1–4, left as known limits by D-098.

**Routing:**
- Worker: Claude Opus 5.5. The tests need credential paths as literals (D-074), and it is security work.
- Review: Gemini 4 Hydrogen `high`, read-only, cross-provider.

**The bar:** no line becomes more permissive than `dev`, and no new hard deny lands on an ordinary command (the lesson of round 8's major 5).

**Not included:** K5 and the other ignored findings (4, 5, 10, 11).

## D-100 — 2026-11-16 — Pedro: escaping search globs are allowed under FullAccess and Any rules
**Decided by Pedro (driver):** "Allow there".
- **Where they're allowed.** P1's search globs that can leave their roots, and whose expansions can't hit the hard-deny list, keep `dev`'s Allow under FullAccess and under an Any rule. Examples: `../other/*.rs`, `/etc/*.conf`.
- **Elsewhere it is unchanged.** They still ask in Home and under prefix rules. They are still hard-denied when an expansion can hit the hard-deny list, and a rule's deny still wins.
- **When it lands.** M1-18 makes this one change in `engine.rs::evaluate`, with tests, after its Hydrogen review. That way the reviewer reads a fixed tree, and the change can ride with any review fixes.

## D-101 — 2026-11-16 — Pedro's go: M1-18 fix round on Hydrogen, review on Argon
**Decided by Pedro (driver):** "yes but run the fix round on hydrogen itself, review on argon".

**The fix round.** Gemini 4 Hydrogen `high` works on the same branch (`agent/opus/M1-18-policy-followup`), with `reviews/M1-18-policy-followup.md` as its input. It fixes:
- the performance regression (`lex_depth` re-lexing nested `$(...)`): back within the fuzz and garbage test limits, close to round 8's times;
- the three same-as-`dev` gaps: P1 `**` then `..`; K1 `env` with unknown options; CR-12 multi-part relative globs and git pathspec magic;
- the minors where cheap;
- D-100: escaping globs that can't hit the hard-deny list are allowed under FullAccess and an Any rule.

**The review.** Gemini 4 Argon `high`, read-only. Author and reviewer are both Gemini models, a recorded exception to cross-provider review (PLAN §8), Pedro's call. This also lifts D-015's pause on Argon for this review.

**Known constraint (D-074).** The `agy` policy refuses credential paths as literals. If a test can't be written for that reason, the worker lists the exact line and expected verdicts in its handoff, and the orchestrator (Claude) adds those tests before the merge.

## D-102 — 2026-11-16 — Pedro: land the font preload for now
**Decided by Pedro (driver):** "lets keep it for now, we can drop it later if the gaps is still this small".
- **What landed.** The five `<link rel="preload">` lines from M1-14b's report (`docs/spikes/M1-linux-checks.md`) are in `app/index.html`, before `boot.js`. The orchestrator landed them.
- **Checked.** `pnpm build` maps them to the same five hashed font files the CSS uses, with no duplicates. Typecheck passes and the 264 app tests pass.
- **What the measurements showed.**
  - Linux: −57 ms median TTI, the reason to keep it.
  - macOS (`docs/spikes/M1-macos-checks.md`): no measurable gain, within ±6 ms on a busy machine.
- **Revisit:** drop it later if a quiet re-measure still shows a gap this small.

## D-103 — 2026-11-16 — Pedro's go: Hydrogen workers for the small open fixes; old branches cleaned
**Decided by Pedro (driver):** "launch hydrogen workers for those small code fixes, also i think there are some old branches in git that were suposed to be deleted".
- **M1-19-host-supervision** (`briefs/M1-19-host-supervision.md`): N3, N6 and N10, all in `atrium-host` `supervise.rs`, so they make one task.
  - Worker: Gemini 4 Hydrogen `high`.
  - Review: Opus 5.5, because these paths sit beside `unsafe` `libc` calls (D-053), and it keeps the review cross-provider.
  - Linux-only paths go to rodrigo's next shift.
- **What doesn't go to Hydrogen now, and why:**
  - P2 needs a new `SessionGate` call to withdraw a ticket, which is a contract change (§5). It goes to a CR if it ever bites.
  - S5, S7, S8, S11 and K5 are in the policy files M1-18 is changing, and their tests need credential literals the `agy` policy refuses (D-074). They go to Claude after M1-18 merges.
- **Branches:**
  - The local `agent/*` branches already merged into `dev` are deleted locally.
  - Remote `agent/*` branches go once `origin/dev` contains them.
  - The orchestrator recorded the deletions in the session log.

## D-104 — 2026-11-16 — Pedro: more freedom for Hydrogen, more tasks in parallel
**Decided by Pedro (driver):** "hydrogen wont make the same mistakes as flash, you can give it a little more freedom, run more of the pending tasks, and break them off in multiple agents to acelarate the process".

**The worker policy (`scripts/agy-policy.py`), loosened by the orchestrator** (tested against 16 sample calls):
- **File tools** check only the arguments that name a path, no longer the content being written. Tests may now contain `~/.ssh`, `../` or credential paths as literals. This lifts D-074's routing reason for test writing. Paths outside the worktree, and `.agents/`, are still refused.
- **Shell:**
  - loops are allowed (`for … do … done`, `seq`, `sleep`, `time`), so workers can run their own 20-run checks;
  - also allowed: `python3`, `scripts/check-protocol-ts`, `scripts/doctor`;
  - read-only git is allowed (`ls-tree`, `cat-file`, `merge-base`, `rev-list`, `shortlog`, `describe`).
- **Still refused:** network tools, nested shells, command substitution, `~` and `../` in command lines, and `git push`/`merge`/`rebase`/`switch`.

**New tasks, in parallel with M1-19** (Hydrogen `high`, within the limit of four Hydrogen jobs):
- **M1-20-gate-audit** (`briefs/M1-20-gate-audit.md`): a read-only, evidence-backed check of PLAN's M1 scope, contract rev 3 and the promotion additions against `dev`, for the gate review.
- **M1-21-demo** (`briefs/M1-21-demo.md`): the gate demo. It is scripted end to end with the fake agent, comes with a written live-demo script, and is reviewed by Sonnet 5.5.

**Waiting on M1-18's merge** (same policy files): S5, S7, S8, S11, K5, and the Settings list of protected paths (CR-12). Hydrogen now takes these too.

## D-105 — 2026-11-16 — Pedro: M1-18's last round is fail-closed, then merge
**Decided by Pedro (driver):** "Fail-closed round".

**Why:** the Argon review of fix round 1 (`reviews/M1-18-policy-followup-round2.md`) found no Allow regression against `dev`. It did find new spellings around K1, P1 and CR-12, the same convergence pattern as M1-13.

**What fix round 2 does** (Gemini 4 Hydrogen `high`; review Gemini 4 Argon `high`, as D-101):
- **Stop parsing new spellings and fail closed.** Whatever the parser can't fully read counts as unknown:
  - an `env`/`command` invocation with any option it doesn't know: its payload is read in every way, or the line asks;
  - a glob with `$`, `\`, `?` or a character class that could stand for a dot or a slash, or a path that isn't canonical: treated as possibly escaping and possibly naming a store;
  - git pathspec magic it can't fully strip: treated as naming whatever follows.

  The hard-deny check still runs on everything that can be read.
- **Fix the rest:** major 4 (an `Allow` path rule at a higher scope masking a lower `Deny` under D-100) and minor 5 (`split_words` and the lex cache).

**After the round:** whatever the Argon review says, the orchestrator brings M1-18 to Pedro for a merge decision, with anything left recorded as a known limit.

## D-106 — 2026-11-16 — Orchestrator, under Pedro's D-104 go: M1-22, `atriumd` registers its adapters
- **The finding.** M1-20's gate audit (`reviews/M1-20-gate-audit.md`, blocker 1) found that the built `atriumd` registers no agent adapters: `main.rs` never fills `Options::adapters`. Only the tests do. The orchestrator confirmed it in the code. So a real Home chat with Claude Code or Codex can't start, and no test runs the real binary with a native launch.
- **The task.** It is a blocker and a small fix, inside D-104's "run more of the pending tasks":
  - M1-22-daemon-adapters (`briefs/M1-22-daemon-adapters.md`) goes to Gemini 4 Hydrogen `high`, reviewed by Sonnet 5.5;
  - it adds a test against the real binary.
- **Left for Pedro.** The audit's other gaps are UI surfaces PLAN lists for M1 that `dev` lacks:
  - export, rename and archive actions;
  - the rules editor, and the Settings safety disclosures (including CR-12's list);
  - the enforcement details view;
  - batch approve/deny;
  - `⌘⇧J`.

  Whether each lands in M1 or moves to M2 through a CR is Pedro's call. Also his: the Linux idle-memory gate, and the open S5/S7/S8/S11.

## D-107 — 2026-11-16 — Pedro: M1-19 adds the drain wait before merging
Pedro chose to add the Opus reviewer's optional fix before merging M1-19: in `kill_recorded`, when the recorded leader is gone, wait up to `GROUP_DRAIN_TIMEOUT` for the group to drain (no signal sent) before the single `group_is_empty` check. Verified still requires `ESRCH`. Hydrogen runs the fix round. Whatever N3 root cause 1 leaves after that stays a known fail-safe limit, measured on Linux by rodrigo (100+ runs). The orchestrator also sent M1-21 into a fix round on Hydrogen for the Sonnet review's findings, under D-104.

## D-108 — 2026-11-16 — Pedro: M1-22 fixes the GUI `PATH` before merging
The M1-22 Sonnet review found that `atriumd` started from the GUI app or as the login service inherits launchd's minimal `PATH`, so `claude` and `codex` aren't found and a real Home chat fails. Pedro chose to fix it in M1-22 rather than defer it to M8. The fix: `atriumd` appends well-known install dirs that exist (`~/.local/bin`, `~/.claude/local`, npm/bun/cargo bins, `/opt/homebrew/bin`, `/usr/local/bin`) after the inherited entries, with a switch (`ATRIUM_NO_PATH_EXTRAS`) so tests can't reach a real CLI. Reading the login shell's `PATH` is left as a possible follow-up. Hydrogen runs the fix round. Also recorded: M1-19 merged into dev (`f11193d`) on Pedro's go.

## D-109 — 2026-11-16 — Pedro: the missing M1 UI lands in M1, built by an Opus agent
The M1-20 gate audit's UI gaps are built now, in M1, not moved to M2: thread rename, archive, export and end-session actions; batch approve/deny; `⌘⇧J`; the rules editor; the Settings safety disclosures (hard-deny list, `git config` limit, same-UID boundary; the CR-12 list after M1-18 merges); the enforcement details view. Pedro asked for a Claude Opus 5.5 worker ("do it now, launch an opus agent"), so this task runs on Opus instead of Hydrogen. Review is cross-provider, on Argon. Brief: `briefs/M1-23-ui-gaps.md`. M1-18's merge decision stays open ("Not yet").

## D-110 — 2026-11-16 — Pedro: one more M1-18 round for what Argon's round-3 review left
D-105 said fix round 2 was the last one. After Argon's review of it (rework; no regression vs dev; gaps all same as dev or stricter), Pedro chose "Not yet" on the merge and then asked to fix what's left on Hydrogen before deciding: the glob normalizer gaps (`*/..`, dot-and-slash classes, brace overflow, `ASK_TREES` probes), unbounded wrappers (split `-S` arguments, `security`/`secret-tool`/interpreters, every wrapper not only `env`/`command`), a cap on glob variants, and the two new Home asks. Same rules: nothing more permissive than dev, no new hard deny on an ordinary line. The review after this round goes to Argon, then the merge decision to Pedro.

## D-111 — 2026-11-16 — Pedro: the open M1 gate decisions
Pedro decided the open gate items:
- **Linux idle memory:** the Linux gate is < 165 MB private PSS (measured 158.7–160 MB, WebKitGTK's floor); macOS stays < 150 MB. Contract §14 and PLAN updated.
- **Linux TTI (D-041):** a gate on Linux too, with the macOS statistic (30 warm launches, median < 400 ms, p90 < 450 ms), measured from `main`, starting at load < 2.5 with no build running, spawn → TTI reported beside it.
- **§14 frame statistic on Linux:** with whole-millisecond rAF timestamps, "over 16.7 ms" reads as a frame over 1.5 × the refresh interval; p99 ≤ 33 ms unchanged.
- **Batching in `bridge.rs`:** now, in M1 (deliveries batched per animation frame).
- **A damaged message in an export:** export with the message marked damaged (CR-16).
- **S5, S7, S8, S11 and K5:** fixed after M1-18 merges (same files), with the four extra credential stores (CR-17).
- **P2:** fixed now, with a `SessionGate` withdraw call (CR-18).
`env -C` was already settled (it keeps dev's hard deny); it is removed from the open list.

## D-112 — 2026-11-16 — Pedro: Argon reviews M1-24, M1-25 and M1-26
Pedro asked for Gemini 4 Argon `high` to review the three D-111 tasks when they finish, instead of Sonnet. Argon and the Hydrogen authors are both Gemini models, so this is a same-provider review, by the driver's choice. **Withdrawn the same day:** once told that Argon and Hydrogen are both Gemini models, Pedro kept the Sonnet reviews (cross-provider) for all three.

## D-113 — 2026-11-16 — Pedro: M1-23 fix round
After Argon's review (rework: 2 rules-editor blockers, 1 major in the Linux save dialog, 4 minors, 2 nits; `docs/agents/reviews/M1-23-ui-gaps.md`), Pedro chose a fix round ("Yes, fix round"). Same worker model (Claude Opus 5.5, D-109), same branch and worktree, owned paths unchanged. The daemon-side guard for blocker 1 (`CreateRule` refusing shell/runner prefixes) is outside M1-23's paths; it is only noted for a possible CR, not decided.

## D-114 — 2026-11-16 — Pedro: one small M1-18 round
Asked how M1-18 should finish after Argon's round-4 review (two "same as dev" blockers and a slow synthetic glob; `docs/agents/reviews/M1-18-policy-followup-round4.md`), Pedro chose "small m1-18 round" over merging with known limits. Fix round 4 goes to Gemini 4 Hydrogen `high`: relative `..` re-entry globs, propagating the search-variant overflow, the `env -S` off-by-one, and the variant total cap. Argon reviews it (D-101).

## D-115 — 2026-11-16 — Pedro: one more M1-18 round
After Argon's review of fix round 4 (all 20 lines closed, 0 regressions vs dev; two "same as dev" edge cases left and a nit; `docs/agents/reviews/M1-18-policy-followup-round5.md`), Pedro chose "One more round" over merging with known limits (the orchestrator had recommended merging). Fix round 5 goes to Gemini 4 Hydrogen `high`; Argon reviews it.

## D-116 — 2026-11-16 — Pedro: M1-23 round 3, fail-closed rule check
After Argon's review of M1-23 round 2 (rework: the rules editor's refusal misses attached inline code, digit clusters, `deno -q eval`, `bun eval`, `pwsh`, case and APFS lookalikes, and crashes on `constructor`; Inbox `A`/`D` can resolve another thread's approval; `docs/agents/reviews/M1-23-ui-gaps-round2.md`), Pedro chose a fail-closed round 3 over patching the listed spellings: names compared case- and APFS-folded, any option word carrying an inline-code flag letter refused, and prefix allow rules on interpreters and `find` refused unless a script or path is named. Same worker (Claude Opus 5.5). The engine's case-sensitive shell matching (`/bin/BASH -c` payloads aren't read) is outside M1-23; it is noted for a CR.
