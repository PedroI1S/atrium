# M1 must-fix list (from M0 spikes)

M0 spikes are merged as spikes: they answered their questions, but they aren't production code. When M1 folds an adapter or component into its real crate, **every item below is part of that task's Definition of Done**. Each M1 brief links this file, and nothing here gets closed without a test that would fail without the fix. Source: the cross-provider reviews in `docs/agents/reviews/` and the orchestrator's live runs. Decided by Pedro in D-018.

## `agy` adapter (from M0-03; review: `reviews/M0-03-agy-adapter.md`, Sol, verdict rework)

| # | Sev. | Must fix |
|---|---|---|
| A1 | **blocker** | Close the fail-open race between validating the hook config and `agy` loading it (`agy` itself fails open on a broken config, D-016). For example, a config `agy` can't read until handed over, an immutable/owned location, or verifying the loaded hook from inside the first call. Prove it with a race test. |
| A2 | major | The tripwire binds each allow to the specific tool call and its arguments, not to session-wide counters. |
| A3 | major | `ask_question` goes through `SessionConfig::gate` like every other tool. |
| A4 | major | A rejected or cancelled approval can never create a retry grant (the bridge state and normalizer stay in sync). |
| A5 | major | Per-turn `Usage` semantics on multi-turn sessions (`result` is cumulative). |
| A6 | major | Question answers reach the provider in a form it uses; the test proves the provider received them. |
| A7 | major | A blocked stdin write never blocks Interrupt/Shutdown (a separate writer task). |
| A8 | major | Shutdown and bypass-abort kill the whole process group (tool descendants included). |
| A9 | major | Hook connections end with their helper and bridge; an unanswered Ask is cancelled, not leaked. |
| A10 | major | Saturated shutdown still delivers the closing events and `SessionEnded` (bounded wait, not a 50 ms drop). |
| A11 | major | The saturation test uses real `step_update` records and an active turn, and proves the guarantee. |
| A12 | major | Sanitizing works on reconstructed streams, not just per delta (temp-dir ids leaked through fragments). |
| A13 | major | Secret checks also catch JSON-escaped and Unicode-escaped disclosure. |
| A14 | major | Per-session MCP servers actually reach `agy`, or the limitation is surfaced to the user. |
| A15 | minor | (Fixed at integration: `agy-record`/`agy-chat`.) Example names still collide across `spike-agent-claude`, `-codex` and `-acp` (`record`/`chat`), and cargo warns. Give every crate's examples unique names when they move into M1 crates. |

## OpenCode / ACP adapter (from M0-04; review: `reviews/M0-04-acp-adapter.md`, Sol, verdict rework)

| # | Sev. | Must fix |
|---|---|---|
| O1 | **blocker** | The Code Mode (`execute`) policy applies to **every** agent, including `build`, custom agents and inherited rules, not only `explore`/`general`. Proven against a config that tries to re-enable it. **Default decided (D-019): native Code Mode on + enforcement *Partial*; "turn Code Mode off" is an opt-in that must then apply to every agent.** The spike currently denies by default; M1 flips it. |
| O2 | major | Cancelling a turn also clears runtime `pending_approvals`/`pending_questions`; a cancelled approval can never be executed. |
| O3 | major | Waiting on a terminal never blocks Interrupt/Shutdown (off the session loop). |
| O4 | major | `Usage`: ACP `UsageUpdate.used` is context size, not input tokens; cost is per-turn deltas, not cumulative. |
| O5 | major | Shutdown delivers the closing events even when the event channel is full. |
| O6 | major | The saturation test measures that shutdown completes (child reaped, events delivered), not just command enqueueing. |
| O7 | major | The bypass checker requires an **Allow** gate decision **before** execution; Deny→completed or late gating counts as a bypass. |
| O8 | minor | Failed tool updates keep their replacement title/kind/input. |
| O9 | minor | Resume-only agents get `session/resume`, not `session/load`. |
| O10 | minor | Report and handoff match the evidence (`tool-fetch` is now gated `webfetch`). |
| O11 | design | M1 recommendation from the spike: keep the generic ACP adapter, and evaluate an OpenCode adapter over `opencode serve` (per-session rules at `POST /session`, model + variant per prompt; MonoCode prior art) if ACP keeps needing workarounds. |

## Other open items from merged spikes

| # | From | Item |
|---|---|---|
| X1 | M0-02 Codex | Re-record `patch` live: file-change approval isn't live-verified (the run hit `usageLimitExceeded`). |
| X2 | M0-02 Codex | Live Codex didn't ask the `question` (`request_user_input` unavailable in its mode); find the mode that allows it, or document. |
| X3 | M0-02 Codex | Enforcement stays `ProviderGated`; verify the MCP row and the `preToolUse` hook / granular-policy options live. |
| X4 | M0-05 daemon | Idle RSS right after load is ~89 MiB, over the 50 MB budget (60 s: 9–23 MiB). Reduce it (per-connection SQLite connections, page caches), or the gate decides how the budget is measured. |
| X5 | M0-06 UI shell | CR-01 (shadow/scrim/syntax/glass-blur tokens) is a Pedro decision. Linux/WebKitGTK checks belong to the partner (D-010). `macOSPrivateApi` rules out the Mac App Store. The gate decides the TTI statistic (median vs worst case). |
| X6 | M0-09 consistency | 13 contract violations, all in Codex fixtures: 12 startup-failure goldens emit `Unmapped` events during the pre-session failure (only `Error { fatal }` + `SessionEnded` are allowed), and 1 hand-made `interrupt` golden has an inverted closing order. Fix the Codex normalizer/fixtures, then un-ignore `crates/atrium-agents/tests/consistency.rs` so it gates M1. |

## From the M0 gate review (Astra, `reviews/M0-gate.md`)

**Before the M1 contracts freeze (gate conditions):**

| # | Sev. | Must fix |
|---|---|---|
| G1 | **blocker** | Define enforcement **per session** (descriptor: effective enforcement, reasons, provider/version, verified tool coverage), and make clear that **"hard deny" only takes precedence among intercepted calls**: it's not a machine-wide prohibition. Distinguish interception coverage, policy decision and OS isolation in the PLAN (§3.12) and the UI wording. Unknown coverage never shows as Full. `AgentAdapter::interception(&self)` becomes session-scoped. |
| G2 | **blocker** | The daemon trust boundary: separate scoped agent/MCP authority from human-control authority (e.g. `ResolveApproval` only from UI clients); state the remaining same-UID limitation explicitly. |
| G3 | **blocker** | Authoritative approval state: one place validates and consumes an approval before any I/O; grants are bound to session incarnation, request, tool, canonical args and policy context, then expire and are used once. Cancelled or stale requests can never authorize effects. (This promotes A4/O2 to blockers.) |
| G4 | major | Approve the protocol changes: native launch config, session incarnations across resume, context occupancy separate from usage, pre-session diagnostics, wire byte limits, deadlines and quotas, a responsive control path. CR-03 is to be amended before freezing (no `Inline` fallback beyond the frame limit). |
| G5 | major | Performance acceptance statistics: metric, sampling duration, percentile, warm vs fresh launch. Includes the 30 s memory excursions (192–199 MB one window; 250–259 MB three windows). |
| G6 | major | The Linux deferral (D-033) needs a completion gate: it must be done before M1's UI work merges. |

**Promotion blockers and additions** (owned by the milestone that promotes each component: agy/ACP in M2, browser in M3):
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
| L1 | minor | Platform-aware appearance hints: "Blur strength" says macOS text on Linux, and the Linux note blames the compositor. Say "Atrium uses opaque windows on Linux." |
| L2 | major | The memory harnesses report **private PSS** on Linux (PSS without shared-library pages) as the budgeted number, with GPU memory, full PSS and RSS beside it (D-040). Each window's cost is tracked so it doesn't grow. |
| L3 | major | Profile WebKitGTK startup: main → JS (~212 ms), SolidJS mount (~140 ms), input-ready → first paint (~85 ms). Report Linux TTI on every UI change; it's a goal, not a gate, until the driver decides otherwise (D-041). |
| L8 | major | The daemon keeps ~227 MB resident on Linux after closing a 3M-event session (macOS: 9 MB). `malloc_trim` after the fold, another allocator, or no full-history fold (see the lifecycle-projections item above). |
| L10 | later | 60 fps everywhere, no 120 Hz promise (D-042). Not M1's critical path: a timeboxed look at WebKitGTK's 60 fps cap, and a check on a 120 Hz Mac display. |


## From M1-13 adversarial tests (`handoffs/M1-13-adversarial.md` on `agent/sonnet/M1-13-adversarial`; 2026-11-12)

Each finding is a test marked `#[ignore = "M1-13 finding N"]` on that branch; fixing it means un-ignoring the test. **Must be fixed before the M1 gate:** S1 and S2 (they reach Allow on credentials), and S3–S5.

| # | Sev. | Must fix |
|---|---|---|
| S1 | **blocker** | **CR-12** (finding 6): in Home with no rules, reading credential stores outside `HARD_DENY_DEFAULTS` is auto-allowed: `~/.cargo/credentials.toml`, `~/.pypirc`, `~/.pgpass`, `~/.vault-token`, `~/.azure/accessTokens.json`, `~/.aws/sso/cache/*`, `~/.config/git/credentials`, shell history, `~/.env`, and `git show HEAD:.npmrc`. Pedro decides the list (D-013). |
| S2 | **blocker** | Finding 2: a failing `cd` is assumed to succeed. With two ordinary prefix rules (`cd`, `cat`), `cd nope; cat .ssh/id_rsa` reaches Allow. Treat the cwd as unknown after any `cd` whose target isn't a known folder. |
| S3 | major | Finding 1: a `cd` inside `( … )` or a pipeline isn't followed (cwd tracking). Under a broad allow rule this reaches Allow on a hard-denied path. |
| S4 | major | Finding 8: `echo rm -rf ~ \| sh` (the `-rf` is dropped) and `printf '%s' '…' \| sh`: text piped into a shell must be read as code. |
| S5 | major | Finding 11: a wildcard in the name of a credential tree or Atrium folder (`.g*`, `atr*`) must count as touching it. |
| S6 | minor | Finding 3: `pushd`/`popd`/`CDPATH` leave the cwd unknown (must not auto-allow afterwards). |
| S7 | minor | Finding 4: ANSI-C words (`$'…'`) aren't decoded. |
| S8 | minor | Finding 5: `rg x ~/.aws` (recursive search of a folder holding a credential) only asks; consider hard deny. |
| S9 | minor | Finding 7: `sh -c -- 'payload'` isn't read. |
| S10 | minor | Finding 9: a shell fed by process substitution (`bash < <(echo …)`); documented limit from M1-03 round 6 finding 2. |
| S11 | minor | Finding 10: a glob that reaches a credential only through a symlink. |
| S13 | **blocker** | From the security review (`reviews/M1-13-adversarial.md`, 2026-11-14): S2 is **not closed** on the branch, and the branch has regressions against `dev` (a key symlinked into `~/.ssh` readable without asking; a `cd` through a symlink loses the write check; false hard denies after a working `cd`). `dev` doesn't have the regressions: the branch must not be merged before the rework (the brief's "Third part"). |
| S12 | — | Still to write (M1-13's remaining scope): the daemon and host adversarial tests (socket roles, `Hello` across the deadline, full control lane, many replaced subscriptions, a store read failing during replay, `kill_recorded` `Unverified`, a recovery closure that fails, the slow-command limit). |

## From the M1 Linux checks (`handoffs/M1-14a-linux-host.md` on `agent/sonnet/M1-14a-linux-host`; 2026-11-13)

Found by running the daemon's tests 30 times in a row on Linux (6 of 30 runs failed; the base commit fails the same way, so M1-14a's change is not the cause). **Must be fixed before the M1 gate:** N1.

| # | Sev. | Must fix |
|---|---|---|
| N1 | major | **A resume right after `Ended` is refused.** `sessions.rs` `run()` stores `SessionEnded` (the thread shows `Ended`) and removes the thread from `Inner::sessions` only at its very end; `launch()` refuses a thread still in the map, so `ResumeThread` answers `Invalid: "the thread already has a session"`. The app's Resume button will hit this. Seen as 4 failures of `sessions::a_stale_approval_is_expired_and_never_reaches_the_session`. In M1-15's brief. |
| N2 | minor | Flaky tests, test-side: `sessions::the_inbox_stream_follows_threads_without_following_every_token` reads `ThreadUpdated { Running }` from one lane before it must be there (ordering race in the test). |
| N3 | minor | `sessions::a_process_the_adapter_left_running_is_killed_or_reported` (`Unverified` where `Verified` was expected) and `recovery::a_killed_daemon_is_recovered_at_the_next_start` fail about once in 30 to 100 runs: both go through `kill_recorded` on a `sleep` that is a zombie until something reaps it. Not established whether the daemon's 2 s drain or the test is early. In M1-15's brief to establish. |
| N4 | minor | M1-13's `adversarial_socket::flooding_unread_list_threads_past_8_mib_control_lane_closes_connection` failed 4 of 7 runs on the orchestrator's re-run (20 s timeout; 0.4 s when it passes). Back with the M1-13 worker to tell a racy test from a daemon that doesn't close the connection (finding 14 may be the cause). |
| N5 | later | Linux branches no test reaches: `peer.rs` error paths and the whole success path of `check_root_owned_tree` (a root-owned packaged executable; needs a packaged build), `supervise.rs` real `PF_EXITING`/`PF_KTHREAD` tasks (pure table only), `lifecycle.rs` XDG fallbacks and replacing an existing systemd unit. List in `docs/spikes/M1-linux-checks.md` on the branch. |

## From the M1-07 reviews (`reviews/M1-07-claude-adapter*.md`, Argon; 2026-11-13)

For the policy engine, not the adapter (the adapter's own findings are fixed in M1-07's fix rounds).

| # | Sev. | Must fix |
|---|---|---|
| P1 | major | **The policy trusts a search subject's `glob` to stay under its `paths`.** `deny.rs` `search_glob_hit` only looks at probes under the subject's roots, and `Home::evaluate` checks the roots alone, so a `ToolSubject::Paths { paths: [cwd], glob }` whose `glob` is absolute, starts with `~`, climbs with `..`, or does either through braces or a character class (`{a,}/home/…`, `.{.,.}/.ssh/…`, `[.][.]/…`) is allowed although it names files outside every root. M1-07 closes it in the Claude adapter's subject (such patterns become extra paths or `Unknown`), but every adapter builds subjects, so the engine should refuse or ask for a `glob` that can leave its roots. For M1-13's follow-up or the policy's owner; test with the patterns in the round-2 review. |
| P2 | minor | A ticket opened by an `Ask` whose request is then withdrawn (cancelled, or never followed by `can_use_tool`) stays `pending` in the ledger until the turn ends and counts against `MAX_PENDING_REQUESTS`. The adapter can't withdraw it: `SessionGate` has no call for that. A change request if it ever bites; bounded by the turn today. |

