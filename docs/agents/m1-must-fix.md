# M1 must-fix list (from M0 spikes)

M0 spikes are merged as spikes: they answered their questions, but they aren't production code. When M1 folds an adapter or component into its real crate, **every item below is part of that task's Definition of Done**. Each M1 brief links this file, and nothing here gets closed without a test that would fail without the fix. Source: the cross-provider reviews in `docs/agents/reviews/` and the orchestrator's live runs. Decided by Pedro in D-018.

## Where M1 stands (orchestrator, 2026-11-16)

Each row below starts with its state: ✅ done (with evidence), 🟡 in progress, ⬜ open, ➡️ belongs to a later milestone. Open before the M1 gate:
- **Done:** M1-18 merged (`fb77f20b`, 2026-11-16): S1/CR-12, P1, K1–K4 closed, with known limits K7–K10 below. **In progress:** M1-08L, rodrigo's live Codex checks (X1–X3).
- **Open blockers:** none. **D-120/D-121:** every known limit is reopened and assigned to the M1-27 batch (K5, K7–K10, R1–R3). **CR-19** (engine shell/wrapper/`sudo` names matched case-sensitively; Argon: high for the gate) is approved for the post-M1-18 batch (D-118).
- **Open, minor:** S5, S7, S8, S11 (their tests are still ignored); K5; N3 and N6 need Linux runs (rodrigo; M1-19 merged). N5 and L10 are "later".
- **Decided (D-111):** Linux idle memory < 165 MB; Linux TTI is a gate; the Linux frame statistic counts frames over 1.5 × the interval; `bridge.rs` batching now; damaged messages export marked (CR-16, ✅ M1-25 merged); S5/S7/S8/S11/K5 + four more stores (CR-17) after M1-18; P2 now (CR-18, ✅ M1-26 merged).
- **The gate itself (PLAN §8):**
  - the demo end to end, with all checks green;
  - the Argon + Astra security and architecture review;
  - Pedro's sign-off.

## `agy` adapter (from M0-03; review: `reviews/M0-03-agy-adapter.md`, Sol, verdict rework)

| # | Sev. | Must fix |
|---|---|---|
| A1 | **blocker** | ➡️ **M2, not M1** (the `agy` adapter is promoted in M2; M1 ships only the Claude and Codex adapters). Close the fail-open race between validating the hook config and `agy` loading it (`agy` itself fails open on a broken config, D-016). For example, a config `agy` can't read until handed over, an immutable/owned location, or verifying the loaded hook from inside the first call. Prove it with a race test. |
| A2 | major | ➡️ **M2, not M1** (the `agy` adapter is promoted in M2; M1 ships only the Claude and Codex adapters). The tripwire binds each allow to the specific tool call and its arguments, not to session-wide counters. |
| A3 | major | ➡️ **M2, not M1** (the `agy` adapter is promoted in M2; M1 ships only the Claude and Codex adapters). `ask_question` goes through `SessionConfig::gate` like every other tool. |
| A4 | major | ➡️ **M2, not M1** (the `agy` adapter is promoted in M2; M1 ships only the Claude and Codex adapters). A rejected or cancelled approval can never create a retry grant (the bridge state and normalizer stay in sync). |
| A5 | major | ➡️ **M2, not M1** (the `agy` adapter is promoted in M2; M1 ships only the Claude and Codex adapters). Per-turn `Usage` semantics on multi-turn sessions (`result` is cumulative). |
| A6 | major | ➡️ **M2, not M1** (the `agy` adapter is promoted in M2; M1 ships only the Claude and Codex adapters). Question answers reach the provider in a form it uses; the test proves the provider received them. |
| A7 | major | ➡️ **M2, not M1** (the `agy` adapter is promoted in M2; M1 ships only the Claude and Codex adapters). A blocked stdin write never blocks Interrupt/Shutdown (a separate writer task). |
| A8 | major | ➡️ **M2, not M1** (the `agy` adapter is promoted in M2; M1 ships only the Claude and Codex adapters). Shutdown and bypass-abort kill the whole process group (tool descendants included). |
| A9 | major | ➡️ **M2, not M1** (the `agy` adapter is promoted in M2; M1 ships only the Claude and Codex adapters). Hook connections end with their helper and bridge; an unanswered Ask is cancelled, not leaked. |
| A10 | major | ➡️ **M2, not M1** (the `agy` adapter is promoted in M2; M1 ships only the Claude and Codex adapters). Saturated shutdown still delivers the closing events and `SessionEnded` (bounded wait, not a 50 ms drop). |
| A11 | major | ➡️ **M2, not M1** (the `agy` adapter is promoted in M2; M1 ships only the Claude and Codex adapters). The saturation test uses real `step_update` records and an active turn, and proves the guarantee. |
| A12 | major | ➡️ **M2, not M1** (the `agy` adapter is promoted in M2; M1 ships only the Claude and Codex adapters). Sanitizing works on reconstructed streams, not just per delta (temp-dir ids leaked through fragments). |
| A13 | major | ➡️ **M2, not M1** (the `agy` adapter is promoted in M2; M1 ships only the Claude and Codex adapters). Secret checks also catch JSON-escaped and Unicode-escaped disclosure. |
| A14 | major | ➡️ **M2, not M1** (the `agy` adapter is promoted in M2; M1 ships only the Claude and Codex adapters). Per-session MCP servers actually reach `agy`, or the limitation is surfaced to the user. |
| A15 | minor | ➡️ **M2, not M1** (the `agy` adapter is promoted in M2; M1 ships only the Claude and Codex adapters). (Fixed at integration: `agy-record`/`agy-chat`.) Example names still collide across `spike-agent-claude`, `-codex` and `-acp` (`record`/`chat`), and cargo warns. Give every crate's examples unique names when they move into M1 crates. |

## OpenCode / ACP adapter (from M0-04; review: `reviews/M0-04-acp-adapter.md`, Sol, verdict rework)

| # | Sev. | Must fix |
|---|---|---|
| O1 | **blocker** | ➡️ **M2, not M1** (the ACP/OpenCode adapter is promoted in M2). The Code Mode (`execute`) policy applies to **every** agent, including `build`, custom agents and inherited rules, not only `explore`/`general`. Proven against a config that tries to re-enable it. **Default decided (D-019): native Code Mode on + enforcement *Partial*; "turn Code Mode off" is an opt-in that must then apply to every agent.** The spike currently denies by default; M1 flips it. |
| O2 | major | ➡️ **M2, not M1** (the ACP/OpenCode adapter is promoted in M2). Cancelling a turn also clears runtime `pending_approvals`/`pending_questions`; a cancelled approval can never be executed. |
| O3 | major | ➡️ **M2, not M1** (the ACP/OpenCode adapter is promoted in M2). Waiting on a terminal never blocks Interrupt/Shutdown (off the session loop). |
| O4 | major | ➡️ **M2, not M1** (the ACP/OpenCode adapter is promoted in M2). `Usage`: ACP `UsageUpdate.used` is context size, not input tokens; cost is per-turn deltas, not cumulative. |
| O5 | major | ➡️ **M2, not M1** (the ACP/OpenCode adapter is promoted in M2). Shutdown delivers the closing events even when the event channel is full. |
| O6 | major | ➡️ **M2, not M1** (the ACP/OpenCode adapter is promoted in M2). The saturation test measures that shutdown completes (child reaped, events delivered), not just command enqueueing. |
| O7 | major | ➡️ **M2, not M1** (the ACP/OpenCode adapter is promoted in M2). The bypass checker requires an **Allow** gate decision **before** execution; Deny→completed or late gating counts as a bypass. |
| O8 | minor | ➡️ **M2, not M1** (the ACP/OpenCode adapter is promoted in M2). Failed tool updates keep their replacement title/kind/input. |
| O9 | minor | ➡️ **M2, not M1** (the ACP/OpenCode adapter is promoted in M2). Resume-only agents get `session/resume`, not `session/load`. |
| O10 | minor | ➡️ **M2, not M1** (the ACP/OpenCode adapter is promoted in M2). Report and handoff match the evidence (`tool-fetch` is now gated `webfetch`). |
| O11 | design | ➡️ **M2, not M1** (the ACP/OpenCode adapter is promoted in M2). M1 recommendation from the spike: keep the generic ACP adapter, and evaluate an OpenCode adapter over `opencode serve` (per-session rules at `POST /session`, model + variant per prompt; MonoCode prior art) if ACP keeps needing workarounds. |

## Other open items from merged spikes

| # | From | Item |
|---|---|---|
| X1 | M0-02 Codex | 🟡 **M1-08L** (live Codex checks, rodrigo). Re-record `patch` live: file-change approval isn't live-verified (the run hit `usageLimitExceeded`). |
| X2 | M0-02 Codex | 🟡 **M1-08L** (live Codex checks, rodrigo). Live Codex didn't ask the `question` (`request_user_input` unavailable in its mode); find the mode that allows it, or document. |
| X3 | M0-02 Codex | 🟡 **M1-08L** (live Codex checks, rodrigo). Enforcement stays `ProviderGated`; verify the MCP row and the `preToolUse` hook / granular-policy options live. |
| X4 | M0-05 daemon | ✅ **Met under G5's metric** (contract §14: idle 60 s after the last activity): 18–19 MiB. Idle RSS right after load is ~89 MiB, over the 50 MB budget (60 s: 9–23 MiB). Reduce it (per-connection SQLite connections, page caches), or the gate decides how the budget is measured. |
| X5 | M0-06 UI shell | ✅ **Settled:** CR-01…CR-04 in contract rev 3 §11; `macOSPrivateApi` accepted (D-034); TTI statistic in §14. CR-01 (shadow/scrim/syntax/glass-blur tokens) is a Pedro decision. Linux/WebKitGTK checks belong to the partner (D-010). `macOSPrivateApi` rules out the Mac App Store. The gate decides the TTI statistic (median vs worst case). |
| X6 | M0-09 consistency | ✅ **Done:** M1-08 regenerated the Codex goldens; `consistency.rs` runs. 13 contract violations, all in Codex fixtures: 12 startup-failure goldens emit `Unmapped` events during the pre-session failure (only `Error { fatal }` + `SessionEnded` are allowed), and 1 hand-made `interrupt` golden has an inverted closing order. Fix the Codex normalizer/fixtures, then un-ignore `crates/atrium-agents/tests/consistency.rs` so it gates M1. |

## From the M0 gate review (Astra, `reviews/M0-gate.md`)

**Before the M1 contracts freeze (gate conditions):**

| # | Sev. | Must fix |
|---|---|---|
| G1 | **blocker** | ✅ **Done:** contract rev 3 §3 (D-044), implemented in M1. Define enforcement **per session** (descriptor: effective enforcement, reasons, provider/version, verified tool coverage), and make clear that **"hard deny" only takes precedence among intercepted calls**: it's not a machine-wide prohibition. Distinguish interception coverage, policy decision and OS isolation in the PLAN (§3.12) and the UI wording. Unknown coverage never shows as Full. `AgentAdapter::interception(&self)` becomes session-scoped. |
| G2 | **blocker** | ✅ **Done:** contract rev 3 §4 (D-045), implemented in M1. The daemon trust boundary: separate scoped agent/MCP authority from human-control authority (e.g. `ResolveApproval` only from UI clients); state the remaining same-UID limitation explicitly. |
| G3 | **blocker** | ✅ **Done:** contract rev 3 §5 (D-046), implemented in M1. Authoritative approval state: one place validates and consumes an approval before any I/O; grants are bound to session incarnation, request, tool, canonical args and policy context, then expire and are used once. Cancelled or stale requests can never authorize effects. (This promotes A4/O2 to blockers.) |
| G4 | major | ✅ **Done:** contract rev 3 §6 (D-047), frozen D-051. Approve the protocol changes: native launch config, session incarnations across resume, context occupancy separate from usage, pre-session diagnostics, wire byte limits, deadlines and quotas, a responsive control path. CR-03 is to be amended before freezing (no `Inline` fallback beyond the frame limit). |
| G5 | major | ✅ **Done:** contract rev 3 §14; measured on macOS in `docs/spikes/M1-macos-checks.md`. Performance acceptance statistics: metric, sampling duration, percentile, warm vs fresh launch. Includes the 30 s memory excursions (192–199 MB one window; 250–259 MB three windows). |
| G6 | major | ✅ **Done** (D-039). The Linux deferral (D-033) needs a completion gate: it must be done before M1's UI work merges. |

**Promotion blockers and additions** (2026-11-16: the Claude, supervision, credential-safe `Debug`, projections, streaming-memory, per-window, rendered-content, resource-exhaustion and version-drift items are written into contract rev 3 §§3, 7, 10–12 and `limits.rs`, and implemented by M1's tasks; the M1 gate review checks them. The agy/ACP and browser items stay with M2 and M3.) (owned by the milestone that promotes each component: agy/ACP in M2, browser in M3):
- A1 stays a blocker, and its suggested remedies aren't enough: it needs provider-side fail-closed loading or an enforced boundary protecting the config. The agy spike still advertises `AllToolCalls` (`spikes/agent-agy/src/adapter.rs:183`); it must report Partial until A1 is solved.
- O1 is a blocker for any "Code Mode off = Full" claim (overrides from build, custom, managed and persisted permissions; MCP and subagent coverage). D-019's default is unchanged.
- O9 becomes major wherever resume is promised.
- **Claude:** plugin hooks and `--settings` aren't inspected but still yield `AllToolCalls`, so report conservatively (`spikes/agent-claude/src/settings.rs:12`). Terminal events are lost after 300 ms under backpressure (`adapter.rs:696`).
- **Shared process-group supervision** for all providers plus daemon recovery; show "cleanup unverified" when identity can't be verified.
- **Credential-safe `Debug`**: `SessionConfig`/`McpServer` print MCP `env`/`args` (`crates/atrium-agents/src/lib.rs:118`). Export redaction and private logs ship with M1 history export. The common sanitizer must cover `AdapterInput::Command`, and checks run on decoded content.
- **Lifecycle projections** persisted transactionally; no full-history fold at close (`spikes/daemon-socket/src/daemon.rs:325`).
- **Streaming memory**: bound loaded history and virtualizer state; profile the 1–1.4 GB transient under concurrent load.
- **Per-window delivery/ACK state**, so an occluded pop-out can't stall others or approvals.
- **Rendered agent content security contract**: link-scheme allowlist, image loading, Tauri invoke boundaries (`spikes/stream-perf/src/markdown.ts`).
- **Resource exhaustion**: output/record byte limits, storage quotas, disk-full behaviour, atomic event+projection updates.
- **Config/version drift**: pin tested CLI versions, record config provenance, invalidate stale coverage claims. Resolve PLAN §6's "agy sandbox-only if hooks break" vs D-012.
- **Browser**: `ProfileLock` needs owner/mode/symlink validation (`spikes/browser-screencast/src-tauri/src/profile.rs:54`); bind every browser operation to an authorized context; narrow PLAN §3.13's "isolation" wording.
- **X6 checker**: strengthen it (duplicates, per-closure ordering). It checks goldens, not live enforcement.

**Process (for M1):** reviewer verification at the final merged commit after the last fix round; ignored/known-failing tests stay visible at gates; measurement evidence pinned and reproducible; D-030 becomes a machine preflight in `scripts/doctor` (real paths outside sync roots, `.git` integrity).

## From the M0 Linux checks (rodrigo, `docs/spikes/M0-linux-checks.md`; G6 done, D-039)

| # | Sev. | Must fix |
|---|---|---|
| L1 | minor | ✅ **Done:** M1-14b found the hint already right. Platform-aware appearance hints: "Blur strength" says macOS text on Linux, and the Linux note blames the compositor. Say "Atrium uses opaque windows on Linux." |
| L2 | major | ✅ **Done:** M1-14b (`linuxmem.py`, private PSS). The memory harnesses report **private PSS** on Linux (PSS without shared-library pages) as the budgeted number, with GPU memory, full PSS and RSS beside it (D-040). Each window's cost is tracked so it doesn't grow. |
| L3 | major | ✅ **Profiled** (M1-14b, `docs/spikes/M1-linux-checks.md`); the font preload landed (D-102). Open: the Linux TTI gate decision (D-041). Profile WebKitGTK startup: main → JS (~212 ms), SolidJS mount (~140 ms), input-ready → first paint (~85 ms). Report Linux TTI on every UI change; it's a goal, not a gate, until the driver decides otherwise (D-041). |
| L8 | major | ✅ **Met:** M1-14a measured daemon idle RSS at 18.4–18.6 MiB on Linux. The daemon keeps ~227 MB resident on Linux after closing a 3M-event session (macOS: 9 MB). `malloc_trim` after the fold, another allocator, or no full-history fold (see the lifecycle-projections item above). |
| L10 | later | 60 fps everywhere, no 120 Hz promise (D-042). Not M1's critical path: a timeboxed look at WebKitGTK's 60 fps cap, and a check on a 120 Hz Mac display. |


## From M1-13 adversarial tests (`handoffs/M1-13-adversarial.md` on `agent/sonnet/M1-13-adversarial`; 2026-11-12)

Each finding is a test marked `#[ignore = "M1-13 finding N"]` on that branch; fixing it means un-ignoring the test. **Must be fixed before the M1 gate:** S1 and S2 (they reach Allow on credentials), and S3–S5.

| # | Sev. | Must fix |
|---|---|---|
| S1 | **blocker** | ✅ **Done:** M1-18 merged `fb77f20b` (D-099–D-115; last reviews `reviews/M1-18-policy-followup-round5.md`, Argon, and `-round6.md`, Sonnet: no Allow on a sensitive path beyond dev). The finding-6 test stays ignored only for the four stores CR-17 adds. **CR-12** (finding 6; decided D-082: Ask, not hard-deny; fix owed): in Home with no rules, reading credential stores outside `HARD_DENY_DEFAULTS` is auto-allowed: `~/.cargo/credentials.toml`, `~/.pypirc`, `~/.pgpass`, `~/.vault-token`, `~/.azure/accessTokens.json`, `~/.aws/sso/cache/*`, `~/.config/git/credentials`, shell history, `~/.env`, and `git show HEAD:.npmrc`. Pedro decides the list (D-013). |
| S2 | **blocker** | ✅ **Done:** M1-13 rounds 3–8, merged `02c13bc`. Finding 2: a failing `cd` is assumed to succeed. With two ordinary prefix rules (`cd`, `cat`), `cd nope; cat .ssh/id_rsa` reaches Allow. Treat the cwd as unknown after any `cd` whose target isn't a known folder. |
| S3 | major | ✅ **Done:** M1-13 rounds 3–8, merged `02c13bc`. Finding 1: a `cd` inside `( … )` or a pipeline isn't followed (cwd tracking). Under a broad allow rule this reaches Allow on a hard-denied path. |
| S4 | major | ✅ **Done:** M1-13 rounds 3–8, merged `02c13bc`. Finding 8: `echo rm -rf ~ \| sh` (the `-rf` is dropped) and `printf '%s' '…' \| sh`: text piped into a shell must be read as code. |
| S5 | major | ⬜ **Open:** test still ignored ("finding 11"). Finding 11: a wildcard in the name of a credential tree or Atrium folder (`.g*`, `atr*`) must count as touching it. |
| S6 | minor | ✅ **Done:** M1-13, merged `02c13bc`. Finding 3: `pushd`/`popd`/`CDPATH` leave the cwd unknown (must not auto-allow afterwards). |
| S7 | minor | ⬜ **Open:** test still ignored ("finding 4"). Finding 4: ANSI-C words (`$'…'`) aren't decoded. |
| S8 | minor | ⬜ **Open:** test still ignored ("finding 5"). Finding 5: `rg x ~/.aws` (recursive search of a folder holding a credential) only asks; consider hard deny. |
| S9 | minor | ✅ **Done:** M1-13, merged `02c13bc`. Finding 7: `sh -c -- 'payload'` isn't read. |
| S10 | minor | ✅ **Done:** M1-13, merged `02c13bc` (test not ignored). Finding 9: a shell fed by process substitution (`bash < <(echo …)`); documented limit from M1-03 round 6 finding 2. |
| S11 | minor | ⬜ **Open:** test still ignored ("finding 10"). Finding 10: a glob that reaches a credential only through a symlink. |
| S13 | **blocker** | ✅ **Done:** the rework rounds; merged `02c13bc` with no Allow regression against `dev` (D-098). From the security review (`reviews/M1-13-adversarial.md`, 2026-11-14): S2 is **not closed** on the branch, and the branch has regressions against `dev` (a key symlinked into `~/.ssh` readable without asking; a `cd` through a symlink loses the write check; false hard denies after a working `cd`). `dev` doesn't have the regressions: the branch must not be merged before the rework (the brief's "Third part"). |
| S12 | — | ✅ **Done:** `adversarial_socket.rs`, `adversarial_recovery.rs` and `adversarial_supervise.rs`, merged with M1-13. Still to write (M1-13's remaining scope): the daemon and host adversarial tests (socket roles, `Hello` across the deadline, full control lane, many replaced subscriptions, a store read failing during replay, `kill_recorded` `Unverified`, a recovery closure that fails, the slow-command limit). |

### M1-13 merged (`02c13bc`, D-098): known limits left open

Rounds 3–8 worked through S2–S13 (the handoff says how each was handled); S1 is CR-12, still owed. The last review, `reviews/M1-13-adversarial-round8.md` (Hydrogen `high`), found **no Allow regression against `dev`**. Its one regression (major 5, a false hard deny in `rg -L` value pairing) was fixed in `e5ab941` before the merge. The four blockers below remain open; the reviewer marked each **same as `dev`**: `dev` doesn't catch them either. They go to the follow-up policy task, with CR-12 and P1, written by Claude.

| # | Sev. | Known limit (D-098) |
|---|---|---|
| K1 | blocker | ✅ **Done in M1-18** (`fb77f20b`). Round 8, finding 1: `runs_here` (`eval`/`source`/`.`), `env_payloads` and `<(...)` process substitutions read only the main reading of the line, not `other_readings` or substituted words. |
| K2 | blocker | ✅ **Done in M1-18** (`fb77f20b`). Round 8, finding 2: `reaches_tree_through_links` misses two-hop symlinks, drops `expand_glob`'s `None`, leaves out `atrium_*_paths`, and skips abbreviated long flags. |
| K3 | blocker | ✅ **Done in M1-18** (`fb77f20b`). Round 8, finding 3: `shell_c_code` stops at `-o`/`+o`/`-O`/`+O` values; `long_name` ignores 1–2 letter GNU prefixes (`script --c`, `flock --c`). |
| K4 | blocker | ✅ **Done in M1-18** (`fb77f20b`). Round 8, finding 4: `find_exec_units` doesn't extract `payloads_of`, `env_payloads` or `code_subs` from `-exec` commands. |
| K6 | — | 🟡 M1-18 covered CR-12's list; finding 6 stays ignored for the four stores CR-17 adds (post-M1-18 batch). On merged `dev`, 7 tests are still ignored. 6 are marked `#[ignore = "M1-13 finding N"]`: findings 4 (S7), 5 (S8), 6 (S1, CR-12), 10 (S11), and 11 (S5) twice. Fixing a finding means un-ignoring its test. |
| K5 | minor | 🟡 **In M1-32** (D-119, reopened D-121). `rg -L pat` with no folder, run from a working folder that holds a link into `~/.ssh`, is a hard deny (`dev`: Ask/Allow/Allow). This is round 6's design, not a regression from the fix. |

### M1-23's escalations (reopened by D-121, in M1-29)

| # | Sev. | Limit |
|---|---|---|
| R1 | major | 🟡 **In M1-29.** Rule path scope: `all_paths` adds the cwd and every path-like word, and `path_scope_ok` uses `all` for deny rules too, so a scoped command rule never matches in Home and a scoped deny rule fails open on multi-path calls. |
| R2 | major | 🟡 **In M1-29.** Allow rules cover only top-level units: a prefix rule on `find` covers `find -exec <cmd>`, one on `python3` covers `python3 -c '<code>'`. |
| R3 | minor | 🟡 **In M1-29.** No daemon-side `CreateRule` guard: any socket client can create an allow rule on a shell, wrapper or privilege tool. |

### M1-18 merged (`fb77f20b`, 2026-11-16): known limits

Five fix rounds (D-099–D-115). The last reviews found no Allow on a sensitive path that `dev` doesn't already allow, no false hard deny and no needless ask on ordinary lines (`reviews/M1-18-policy-followup-round6.md`, Sonnet, D-117). Left at the merge; **reopened by D-121** (the 100% gate, D-120) and assigned to the M1-27 batch:

| # | Sev. | Known limit |
|---|---|---|
| K7 | minor | 🟡 **In M1-27** (D-121). `sh -c` scripts that are only a positional-parameter expansion (`sh -c '$0' 'cat .ssh/id_rsa'`) aren't resolved to what they run: Ask in Home and under prefix rules, Allow under Any and FullAccess. Same as `dev`. |
| K8 | nit | 🟡 **In M1-30** (D-121). A `**` search rooted at the home folder asks in Home, because it could reach `.zsh_sessions` (CR-12 trees). |
| K9 | nit | 🟡 **In M1-27** (D-121). `env` with more than about 24 words before the command fails closed to Ask in every setting, including Any and FullAccess. |
| K10 | nit | 🟡 **In M1-31** (D-121). Search globs with 400 or more `*/` segments followed by as many `..` segments take a few seconds to evaluate, then ask. |

## From the M1 Linux checks (`handoffs/M1-14a-linux-host.md` on `agent/sonnet/M1-14a-linux-host`; 2026-11-13)

Found by running the daemon's tests 30 times in a row on Linux (6 of 30 runs failed; the base commit fails the same way, so M1-14a's change is not the cause). **Must be fixed before the M1 gate:** N1.

| # | Sev. | Must fix |
|---|---|---|
| N1 | major | ✅ **Done:** M1-15, merged `fd11c78` (`resume_right_after_final_state_in_a_tight_loop`). **A resume right after `Ended` is refused.** `sessions.rs` `run()` stores `SessionEnded` (the thread shows `Ended`) and removes the thread from `Inner::sessions` only at its very end; `launch()` refuses a thread still in the map, so `ResumeThread` answers `Invalid: "the thread already has a session"`. The app's Resume button will hit this. Seen as 4 failures of `sessions::a_stale_approval_is_expired_and_never_reaches_the_session`. In M1-15's brief. |
| N2 | minor | ✅ **Done:** M1-15, merged `fd11c78` (the test waits on the frames directly). Flaky tests, test-side: `sessions::the_inbox_stream_follows_threads_without_following_every_token` reads `ThreadUpdated { Running }` from one lane before it must be there (ordering race in the test). |
| N3 | minor | 🟡 **Fixed on macOS, Linux to measure** (M1-19, merged `f11193d`; D-103, D-107): root cause 2 (glibc `posix_spawn`/`CLONE_VFORK` exe race) fixed by `identify_after_vfork_exec`; root cause 1 (a gone leader's group still draining) now gets a 2 s drain wait in `kill_recorded` with no signal, Verified still needs `ESRCH`. 30/30 on macOS. **Left:** rodrigo runs both N3 tests 100+ times on Linux; whatever remains is a known fail-safe limit (Opus review). Note: recovery calls `kill_recorded` one record at a time, so each group that never drains adds 2 s to a post-crash start. |
| N4 | minor | ✅ **Done:** made deterministic in M1-13's later rounds (two phases, waits for a marker). M1-13's `adversarial_socket::flooding_unread_list_threads_past_8_mib_control_lane_closes_connection` failed 4 of 7 runs on the orchestrator's re-run (20 s timeout; 0.4 s when it passes). Back with the M1-13 worker to tell a racy test from a daemon that doesn't close the connection (finding 14 may be the cause). |
| N5 | later | Linux branches no test reaches: `peer.rs` error paths and the whole success path of `check_root_owned_tree` (a root-owned packaged executable; needs a packaged build), `supervise.rs` real `PF_EXITING`/`PF_KTHREAD` tasks (pure table only), `lifecycle.rs` XDG fallbacks and replacing an existing systemd unit. List in `docs/spikes/M1-linux-checks.md` on the branch. |
| N6 | minor | 🟡 **Fixed, Linux-only, untested here** (M1-19, merged `f11193d`): `judge_exe_read` requires `num_threads <= 1` for `PF_EXITING`; `read_linux_exe` falls back to `/proc/<pid>/task/*/exe` regardless of `first.state`; table rows for both review mutations plus the Opus nits. **Left:** rodrigo runs the Linux path. |
| N7 | major | ✅ **Fixed, merged into `dev` (`05823bc`, M1-07b, D-086, Opus review approve); macOS run owed.** **On Linux, a `Proc` dropped outside a runtime hangs its thread for ever** (found by the orchestrator, 2026-11-15, on `dev` after M1-14b's merge; 10 of 10 runs). `claude_adapter::a_start_dropped_outside_a_runtime_kills_the_process_group` never ends; the other 37 tests pass in 6 s. `adapter.rs` `terminate_now` builds a runtime of its own and awaits `supervise::terminate`, whose `child.child.wait().await` waits on the tokio `Child` registered with the runtime that spawned it (tokio 1.54 reaps with a pidfd on Linux; the stuck thread sat in `epoll_wait` with a `pidfd` open). That runtime is the caller's, blocked in `join()`, so the wake never comes. macOS reaps through `SIGCHLD`, so the test passes there. Not test-only: an abandoned `start` dropped off a runtime blocks the caller. Fix: reap without the spawning runtime (a blocking `waitpid` on the leader after the group `SIGKILL`, keeping `terminate`'s order), with the test as the regression; check the in-runtime path, which `std::thread::spawn`s the same call. Owner: `atrium-agents` / `atrium-host` (`supervise.rs`). |
| N8 | minor | ✅ **Fixed, merged into `dev` (`f097a48`, M1-07c, D-091; Hydrogen `high` review: approve).** From M1-07b's review (`reviews/M1-07b-proc-drop-linux.md`, finding 1; it predates M1-07b). A provider leader that leaves its own process group (`setpgid`/`setsid` after `spawn_group`) is never sent `SIGKILL`, so `terminate` and `terminate_blocking` wait until it exits on its own. On a synchronous `Drop`, that blocks the caller. Fix: in `terminate_blocking`, also `kill(pid, SIGKILL)` the still-unreaped leader after the group kill, or write the limit into the doc comment. Nits from the same review: the `ECHILD` arm can never succeed; `let _ = pid`; the doc comment should give the blocking bounds. |
| N9 | minor | ✅ **Fixed, merged into `dev` (`a584523`, M1-17, D-091, D-095).** From M1-14b (`docs/spikes/M1-linux-checks.md`): a `logs` folder inside `ATRIUM_HOME` with mode 0755 makes `atriumd` exit at once without writing anything, and the app retries the connection silently. The refusal is right; the silence is not. **M1-17 (D-091).** |
| N10 | minor | ✅ **Fixed** (M1-19, merged `f11193d`): on a macOS `EPERM` with the leader not yet seen as exited, `terminate`/`terminate_blocking` poll up to 50 ms (`MACOS_EPERM_EXIT_POLL`, async in `terminate`) before failing; the bound is in their docs. Tests failed 25/25 on the old code, pass 40/40 (also under CPU load). |

## From the M1-07 reviews (`reviews/M1-07-claude-adapter*.md`, Argon; 2026-11-13)

For the policy engine, not the adapter (the adapter's own findings are fixed in M1-07's fix rounds).

| # | Sev. | Must fix |
|---|---|---|
| P1 | major | ✅ **Done in M1-18** (`fb77f20b`): search globs are normalized and checked against every root, failing closed past the variant cap. **The policy trusts a search subject's `glob` to stay under its `paths`.** `deny.rs` `search_glob_hit` only looks at probes under the subject's roots, and `Home::evaluate` checks the roots alone, so a `ToolSubject::Paths { paths: [cwd], glob }` whose `glob` is absolute, starts with `~`, climbs with `..`, or does either through braces or a character class (`{a,}/home/…`, `.{.,.}/.ssh/…`, `[.][.]/…`) is allowed although it names files outside every root. M1-07 closes it in the Claude adapter's subject (such patterns become extra paths or `Unknown`), but every adapter builds subjects, so the engine should refuse or ask for a `glob` that can leave its roots. For M1-13's follow-up or the policy's owner; test with the patterns in the round-2 review. |
| P2 | minor | ✅ **Fixed** (M1-26, merged `ea9b93f7`; CR-18, D-111): `SessionGate::withdraw` cancels an open ticket (never approves; idempotent) and frees its `MAX_PENDING_REQUESTS` slot; the Claude and Codex adapters call it on provider cancel, turn end, exit and unmatched asks. Tests fill the limit, withdraw, reopen. |

