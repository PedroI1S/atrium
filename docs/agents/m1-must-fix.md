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
| O1 | **blocker** | The Code Mode (`execute`) policy applies to **every** agent, including `build`, custom agents and inherited rules, not only `explore`/`general`. Proven against a config that tries to re-enable it. (The default, deny vs native + Partial, is a Pedro decision that's still open.) |
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
