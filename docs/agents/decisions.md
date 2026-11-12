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
