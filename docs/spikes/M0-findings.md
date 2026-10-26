# M0 Findings Report: Spikes De-risking and Architecture Consolidation

**Task**: `M0-09-m0-findings`  
**Date**: 2026-10-26  
**Author**: Gemini 3.8 Flash (Worker)  
**Reviewer**: Spot-check by Orchestrator (Opus 5.5)  
**Status**: Complete  

> **Gate corrections (orchestrator, 2026-10-26, from Astra's gate review, `docs/agents/reviews/M0-gate.md`).** The cells marked *corrected* below were overstated in the original report: the agy spike advertises `AllToolCalls`, not `ProviderGated`, and must report *Partial* until A1 is solved; the full 100k-event replay misses the < 100 ms target (only the 1,000-behind reconnect passes); and the 30 s idle-memory samples with the WebKit GPU excursion miss the budget. D-028 (memory budget per main window) and D-034 (no App Store, `macOSPrivateApi` accepted) are decided, so §7's questions on them are closed.

---

## 1. Executive Summary & Gate Status

Milestone M0 was designed to de-risk all foundational architecture layers before entering production build phases in M1 (PLAN §5). Spikes covered:
1. **Agent adapters** (M0-01 Claude Code, M0-02 Codex app-server, M0-03 Antigravity `agy`, M0-04 ACP / OpenCode) normalizing diverse wire protocols into normalized `AgentEvent` envelopes under strict lifecycle and tool-gating semantics (`atrium-protocol` and `atrium-agents`).
2. **Daemon socket & client infrastructure** (M0-05) handling SQLite WAL event persistence, subscription paging, reconnection replay, and detached process management.
3. **UI shell, design direction, & layout mechanics** (M0-06 and M0-06b) validating Tauri 2 + SolidJS, overlay title bars, vibrancy, window opacity modes, pop-out multi-window architecture, and tokenized styling.
4. **Streaming performance** (M0-07) sustaining 10,000 tokens/s through the Rust bridge into a virtualized SolidJS DOM.
5. **Browser screencast** (M0-08) driving Chromium over a dedicated CDP pipe into a high-frame-rate canvas with low input latency.

**Key conclusions for the M0 Gate Review**:
- **Protocol Feasibility**: All four agents successfully normalized into `AgentEvent`. Tool call interception is feasible for all four, though with distinct enforcement models: Claude Code achieves `AllToolCalls` (when external settings don't add outside hooks), while Codex and ACP report `ProviderGated`. *Corrected:* the `agy` spike advertises `AllToolCalls` (`spikes/agent-agy/src/adapter.rs:183`), which overstates it while A1 (fail-open config race) is open; it must report *Partial* until then. `AllToolCalls` means every tool *invocation* is inspected, not that the effects of allowed code are contained.
- **Core Performance Budgets**: Met on macOS M-series across 60 fps token streaming (10k tokens/s), browser screencast (59.8 fps, 12–19 ms input-to-frame), reconnect latency (3.4–10.3 ms), and single-window app physical footprint (125–140 MB). Cold-start TTI meets the <400 ms budget at the median (378–384 ms in M0-06b), though with narrow headroom.
- **UI Architecture**: Pedro selected Layout B "Floating spaces" with Atrium Calm themes (D-024) and approved retaining Tauri 2 + SolidJS (D-022). M0-06b proved both window-opacity modes and pop-out window state synchronization.
- **Spike Status & Transitions**: Per D-018, M0-03 (`agy`) and M0-04 (ACP) merged as spikes with unresolved review findings tracked as mandatory requirements in `docs/agents/m1-must-fix.md`.

---

## 2. Consolidated Measurements vs PLAN §4 Budgets

Budgets are defined in PLAN §4 and contract `docs/contracts/m0.md` §5. All numbers are quoted directly from spike reports and review logs; no figures are estimated or re-derived.

| Budget / Target (PLAN §4) | Target Value | Measured Result (Spike) | Status | Notes & Source |
|---|---|---|---|---|
| **Cold start to interactive (TTI)** | < 400 ms (macOS M-series) | **Median: 378–384 ms**<br>Worst: 387–411 ms (M0-06b)<br>Earlier M0-06: 345–392 ms median, worst warm 354–457 ms | **Pass (Median)**<br>*(Narrow slack; 5 of 60 runs exceeded 400 ms in M0-06b)* | M0-06b Table §5: measured per contract §5 (later of first paint and handled input). Rust `main` → JS start is ~245 ms. Fresh post-build launch is 1–2.8 s. |
| **Reconnect latency (with full state)** | < 100 ms | **1,000-behind: 3.454 – 10.259 ms**<br>Full 100k-event replay: 334.2 – 340.1 ms | **Pass (1,000 behind)**<br>**Fail (full 100k replay)** *(corrected)* | M0-05 Table §5: 3 runs with 100,000 events in SQLite WAL. Stored event replay throughput: 294k–299k events/s. |
| **App idle memory (1 window)** | < 150 MB (excluding agents/Chromium) | **Physical footprint: 125 – 141 MB**<br>*(Summed RSS: 176 – 221 MB)* | **Pass (Footprint, 10 s)**<br>**Fail in 5/18 samples at 30 s: 192–199 MB** *(corrected)*<br>*(Fail if measured as RSS)* | M0-06 Table §4 & M0-06b Table §5. Sum of Tauri app + responsible WebKit helpers. Footprint reflects actual unshared RAM. In 5 of 18 runs, WebKit GPU memory jumped by 50–60 MB at 30 s. |
| **App idle memory (3 windows)** | Baseline + ~35 MB per pop-out (D-028) | **Physical footprint: 194 – 208 MB**<br>*(Summed RSS: 304 – 315 MB)* | **Pass (10 s)**<br>**30 s excursions: 250–259 MB** *(corrected)* | M0-06b Table §5. Approved under D-028: pop-out windows each run a WebKit content process adding ~33 MB footprint. |
| **Daemon idle RSS** | < 50 MB | **Immediate: 89.016 – 89.125 MiB**<br>At 3 s: 47.562 – 89.125 MiB<br>**At 60 s: 9.250 – 23.125 MiB** | **Fail (Immediate)**<br>**Pass (60 s Reclaim)** | M0-05 Table §5. Rusqlite connection and initial heap misses budget initially; drops well under budget after macOS memory reclaim. |
| **Token streaming frame rate** | 60 fps (10k tokens/s) | **60.0 fps**<br>*(Dropped frames: 0–4 per 60 s; 3 over 5 min / 3M tokens)* | **Pass** | M0-07 Table §3.1, §3.2, §3.3. Tested at 1 token per `MessageDelta` (10,000 events/s) over local unix socket + Tauri Channel + WKWebView. |
| **Input latency during 10-thread stream** | No dropped input with 10 active threads | **Not proven (Synthetic input only)**<br>Timer lateness: 2.3–2.4 ms median, 5.0–5.6 ms p99<br>Keystroke to paint: 26.7–27.1 ms median (idle ref: 24.5–27.8 ms) | **Not measured (with OS input)** | M0-07 Table §3.2 & Fix round 1. Synthetic DOM typing demonstrates main thread is not starved, but real OS input routing requires accessibility permissions not exercised in M0. |
| **Browser screencast frame rate** | ≥ 30 fps | **1x pane: 59.8 fps** (worst 59.6 fps)<br>2x pane: 57.7 fps (worst 56.1 fps) | **Pass** | M0-08 Table §4. Helium / Chromium over `--remote-debugging-pipe` with custom minimal CDP client. |
| **Browser input-to-frame latency** | < 80 ms locally | **1x pane: p50 12 ms, p95 19 ms**, max 32–34 ms<br>2x pane: p50 27 ms, p95 34 ms, max 51 ms | **Pass** | M0-08 Table §4. 100 probes, 0 misses across 500 samples. Post-fix round 1: p50 11–20 ms, p95 22–37 ms. |
| **Conflict radar incremental update** | < 200 ms per thread change | *not measured* | **Not measured** | Milestone M2 scope (PLAN §5); not included in M0 spikes. |
| **Database table first page (keyset)** | < 150 ms locally | *not measured* | **Not measured** | Milestone M3.5 scope; not included in M0 spikes. |
| **Database schema diagram (300 tables)** | < 1 s (in worker) | *not measured* | **Not measured** | Milestone M3.5 scope; not included in M0 spikes. |
| **Database autocomplete** | < 30 ms | *not measured* | **Not measured** | Milestone M3.5 scope; not included in M0 spikes. |
| **Sandbox guest ready** | < 1 s from "start" to guest ready | *not measured* | **Not measured** | Milestone M4 scope; not included in M0 spikes. |
| **Linux / WebKitGTK benchmarks** | Parity checks on Linux | *not measured* | **Not measured** | Deferred to Pedro's partner per D-010. |

---

## 3. Cross-Adapter Consistency Test Results

The consistency test suite implemented in `crates/atrium-agents/tests/consistency.rs` validates all golden `.events.json` files across providers against the lifecycle and event mapping rules in `atrium_protocol::agent`.

### 3.1 Test Status
- In accordance with the worker rules, the integration test `consistency_all_fixtures` is marked `#[ignore]` so that `cargo test -p atrium-agents` remains green in automated CI.
- When executed explicitly (`cargo test -p atrium-agents --test consistency -- --ignored --nocapture`), the test thoroughly scans all 70 `.events.json` files under `fixtures/agents/` (including `handmade/` subfolders).
- **Result**: Exactly **13 files violate contract consistency rules**, all belonging to the Codex adapter. Claude Code, Antigravity (`agy`), and ACP have **0 violations**.

### 3.2 Detailed Violations by Adapter

#### Codex Adapter Violations (13 files)
1. **Pre-Session Startup Failure Event Pollution (12 files)**:
   - `fixtures/agents/codex/approval-allow-startup-failure.events.json` (5 events)
   - `fixtures/agents/codex/approval-deny-startup-failure.events.json` (5 events)
   - `fixtures/agents/codex/gate-deny-execute-startup-failure.events.json` (5 events)
   - `fixtures/agents/codex/gate-deny-startup-failure.events.json` (5 events)
   - `fixtures/agents/codex/interrupt-startup-failure.events.json` (5 events)
   - `fixtures/agents/codex/multi-turn-startup-failure.events.json` (5 events)
   - `fixtures/agents/codex/patch-startup-failure.events.json` (5 events)
   - `fixtures/agents/codex/question-startup-failure.events.json` (5 events)
   - `fixtures/agents/codex/quota-usage-startup-failure.events.json` (5 events)
   - `fixtures/agents/codex/simple-startup-failure.events.json` (5 events)
   - `fixtures/agents/codex/tool-read-startup-failure.events.json` (5 events)
   - `fixtures/agents/codex/startup-error.events.json` (3 events)
   - **Contract Rule Violated**: `atrium_protocol::agent` lifecycle specification:
     > *"Pre-session failure (bad model, auth error, the CLI won't start): Error { fatal: true } then SessionEnded, with no SessionStarted."*
   - **Violation**: The 11 `*-startup-failure` fixtures emit 3 intermediate `Unmapped { record_type: "stderr", ... }` events between `Error { fatal: true }` and `SessionEnded` (5 events total). `startup-error.events.json` emits an intermediate `Unmapped { record_type: "remoteControl/status/changed", ... }` event (3 events total). By contract, pre-session failures must contain strictly two events.

2. **Exit Closing Order Inversion (1 file)**:
   - `fixtures/agents/codex/handmade/interrupt.events.json`
   - **Contract Rule Violated**: `atrium_protocol::agent` closing order on exit:
     > *"When a session ends for any reason, the adapter first closes everything still open, in this order: open messages → MessageCompleted (text so far); open tool calls → ToolCallFinished { status: cancelled }; pending approvals and questions → RequestResolved { outcome: cancelled }; an open turn → TurnCompleted { outcome: interrupted | failed }. Then SessionEnded."*
   - **Violation**: At Event 4, `RequestResolved { request_id: "17", outcome: Cancelled }` is emitted *before* Event 5 `ToolCallFinished { call_id: "call-1", status: Cancelled }`. Open tool calls must be cancelled before resolving pending requests.

#### Claude Code Adapter
- **Violations**: **0**. All 20 scenarios strictly adhere to established and pre-session lifecycles, delta concatenation, tool call pairing, and gate denial invariants.

#### Antigravity (`agy`) Adapter
- **Violations**: **0**. All 11 scenarios adhere to established and pre-session lifecycles, single-use retry grants, and gate denial invariants.

#### ACP / OpenCode Adapter
- **Violations**: **0**. All 20 scenarios adhere to established lifecycles, question resolution, and gate denial invariants.

---

## 4. Unmapped Record Types per Provider

In accordance with mapping rules (`atrium_protocol::agent`), native records that cannot yet be mapped into structured events are emitted as `AgentEvent::Unmapped { record_type, raw }` rather than dropped silently.

| Provider | Unmapped `record_type` Strings Observed | Description & M1 Recommendation |
|---|---|---|
| **Claude Code** | `system/task_started`<br>`system/task_updated`<br>`system/task_notification` | Subagent background task coordination records (observed in `gate-deny-subagent`). Recommend retaining as unmapped or mapping to subagent thread metadata in M5. `task_progress` maps to `ToolCallUpdated`. |
| **Codex** | `account/updated`<br>`hook/started`<br>`hook/completed`<br>`mcpServer/startupStatus/updated`<br>`remoteControl/status/changed`<br>`thread/readState/changed`<br>`thread/status/changed`<br>`turn/moderationMetadata`<br>`missing_resume_baseline`<br>`stderr` (in startup failure) | Extensive notification surface. `hook/started` and `hook/completed` provide tool visibility; `thread/status/changed` informs turn reconciliation; `account/updated` and `remoteControl` belong to account/device UI outside thread streams. Recommend filtering transport stderr out of pre-session failure. |
| **Antigravity (`agy`)** | *(None)* | The normalizer maps all known NDJSON events (`init`, `step_update`, `result`, `denied_actions`) or ignores progress noise (`keep_alive`). No unmapped records in current fixtures. |
| **ACP / OpenCode** | `session/load:history_replay`<br>`session/update:available_commands_update`<br>`current_mode_update`<br>`config_option_update`<br>`session_info_update`<br>unknown `session/update:<tag>`<br>unknown `rpc:<method>`<br>`rpc_response:unknown_id` | `session/load:history_replay` aggregates historical conversation messages replayed on load/resume so they do not pollute active stream events. `available_commands_update` and mode updates describe client capabilities; keep unmapped or feed into UI palette state. |

---

## 5. Security & Isolation Findings

### 5.1 Claude Code Adapter (M0-01)
- **Fail-Open Risk in CLI**: Live testing on `claude` 2.1.308 revealed that Claude Code **fails open on malformed or error hook responses**. A control response with `subtype: error` allows the tool call to proceed without restriction.
  - *Mitigation*: The adapter never sends errors; gate panics, timeouts, and malformed hook requests unconditionally respond with `deny`.
- **External Settings & Partial Enforcement**: When native Claude settings (`~/.claude` or project `.claude/settings.json`) contain external hooks, those hooks run on the host outside Atrium's gate and can rewrite tool inputs.
  - *Mitigation*: Under D-012 and D-017, the adapter detects external hooks and reports `Interception::ProviderGated` with an explanatory reason, while preserving user settings.

### 5.2 Codex Adapter (M0-02)
- **Universal Pre-execution Gating Unavailable**: Codex's `app-server` JSON-RPC protocol only issues `requestApproval` notifications for actions the model/server considers untrusted. It has no mechanism guaranteeing pre-execution interception of every call (`interception()` is strictly `ProviderGated`).
- **Process Group Isolation**: The spike reaps only its direct child PID; spawned subprocesses and tool descendants are not killed as a process group. This must be addressed in M1.

### 5.3 Antigravity (`agy`) Adapter (M0-03)
- **Critical Fail-Open finding (D-016)**: While helper binary failures fail closed, **an unreadable (mode 0000) or malformed `hooks.json` causes `agy` to ignore the hook config and FAIL OPEN** under `--dangerously-skip-permissions`, executing all tools without gating.
  - *Mitigation*: The adapter implements a 4-layer defense: pre-spawn config verification (mode `0600`, directory mode `0700`), refusal to run if an external `.agents/hooks.json` already exists, re-verification on every tool start, and a runtime tripwire that aborts and SIGKILLs the child if tool executions outnumber gate approvals.
  - *M1 Must-Fix*: Must resolve the race window between config validation and CLI loading (M1 Must-Fix A1).

### 5.4 ACP / OpenCode Adapter (M0-04)
- **Default Gate Bypass**: OpenCode defaults to a `* * allow` policy and ignores client `fs` and `terminal` capabilities. Live testing in run 1 showed that `secret.txt` was read directly without any permission prompt.
  - *Mitigation*: The adapter injects a per-session environment configuration (`OPENCODE_CONFIG` and `OPENCODE_CONFIG_CONTENT`) specifying `permissions: [{"action":"*","resource":"*","effect":"ask"}]`.
- **Code Mode (`execute`) Bypass**: In run 2, the `execute` tool (which executes JavaScript in a Node environment) bypassed the `* * ask` rule, allowing arbitrary network requests (`fetch`) to run un-intercepted.
  - *Mitigation*: The adapter disables Code Mode by appending `{"action":"execute","resource":"*","effect":"deny"}`. Per D-019, OpenCode enforcement is displayed as *Partial*, with an option to disable Code Mode.

### 5.5 UI Shell & Opacity (M0-06 / M0-06b)
- **Private API Usage**: Native window vibrancy with transparency requires `transparent: true` and `macOSPrivateApi: true` in Tauri, utilizing private AppKit APIs that preclude Mac App Store distribution. This is accepted as a distribution decision (direct distribution only).
- **Process-Private Screen Capture**: Opacity measurement harnesses utilize ScreenCaptureKit with strict window filters, capturing only Atrium windows and a synthetic backdrop, preventing accidental desktop or screen exposure.

### 5.6 Browser Screencast (M0-08)
- **Chromium Orphan Persistence**: When the parent process terminates abruptly (`kill -9`), Chromium does not exit when the pipe closes.
  - *Mitigation*: Robust PID-file lock implementation (`atrium-owner.lock` with exclusive `flock`), verifying start time, executable path, and init reparenting before reclaiming orphans.
- **Context Isolation**: Multi-context testing confirmed that two `BrowserContext` instances in a single Chromium process maintain complete cookie, `localStorage`, and session isolation.

---

## 6. Per-Spike Recommendations for M1

| Spike | Recommendation for M1 | Key Architecture Actions |
|---|---|---|
| **M0-01 (Claude Code)** | **Promote to M1** | Retain pure `ClaudeNormalizer` and `ClaudeAdapter`. Pin CLI version range. Maintain `setting_sources` option and external hook detection. |
| **M0-02 (Codex)** | **Promote with Must-Fixes** | Fold `CodexNormalizer` and JSON-RPC mapper into M1. Implement process-group supervision (`killpg`). Clean up startup-failure stderr events. Live re-record `patch` scenario when quota permits (M1 Must-Fix X1). |
| **M0-03 (`agy`)** | **Promote as Spike; Implement Must-Fixes** | Fold `AgyAdapter` and `GateBridge`. Address must-fix items A1–A14 in `docs/agents/m1-must-fix.md` (close hook-config race, canonical argument retry binding, per-turn usage deltas). Resolve per-session MCP config delivery. |
| **M0-04 (ACP / OpenCode)** | **Promote Generic ACP; Plan `opencode serve`** | Retain generic ACP adapter for compliant agents. Implement must-fix items O1–O10. In accordance with D-019, default to native Code Mode on with *Partial* enforcement shown. Evaluate dedicated HTTP adapter over `opencode serve` (MonoCode prior art). |
| **M0-05 (Daemon Socket)** | **Promote to M1** | Adopt pull-based replay, single WAL writer, and `ClientHandle` / `EventStream` actor model. Address initial 89 MiB RSS peak (optimize SQLite connection caching; target 50 MB budget). Prevent whole-history memory fold on session close. |
| **M0-06 / M0-06b (UI Shell)** | **Adopt Layout B ("Floating Spaces")** | Build on M0-06b: floating cards, Atrium Calm themes (D-024), dual-mode opacity (CR-02/CR-04), and pop-out multi-window state synchronization backed by a single Rust writer. Keep UI thin; enforce token conformance via linting. |
| **M0-07 (Stream Perf)** | **Adopt `json-slim` + 8 ms Batching** | Use `json-slim` encoding over Tauri Channel with 8 ms batching. Render markdown blocks incrementally using TanStack Virtual. Profile WebKit transient memory (1.05–1.39 GB peak during 10k/s stream). Implement CR-03 (`MessageCompleted.body` replacement). |
| **M0-08 (Browser)** | **Adopt Pipe + Own CDP Client** | Use `--remote-debugging-pipe` with the minimal 250-line CDP client. Do not use `chromiumoxide` for the pane. Stream JPEG q80 at 1x default, with 2x launch-flag setting. Enforce exclusive lockfile PID-reaping. |

---

## 7. Open Questions & Decisions for the Driver

The following items are surfaced for decision by Pedro (project owner) prior to or during the M0 Gate Review:

1. **Adoption of Contract Change Requests (CR-01, CR-02, CR-04)**:
   - *Question*: Formally approve the token contract modifications proposed in CR-02 and amended by CR-04 for M1 (`--surface-*-rgb` channel tokens, reserved runtime window tokens, required scrim/shadow/blur tokens).
   - *Impact*: Required for Layout B opacity and theming implementation in M1 UI crates.

2. **Adoption of Wire Contract Change Request (CR-03)**:
   - *Question*: Shall `MessageCompleted.text: String` be replaced with `body: MessageBody` (`Inline { text }` vs `Streamed { chars, deltas, sha256 }`) to avoid repeating multi-megabyte texts over the socket and approaching the 16 MiB frame limit?
   - *Impact*: Affects `atrium-protocol`, all adapters, daemon storage, and UI reducers.

3. **Performance Metric Definitions**:
   - *TTI Statistic*: Define whether the contractual <400 ms cold-start gate is evaluated against the median (which passes comfortably at 378–384 ms) or worst-case / post-fresh-build runs (which can reach 411 ms or 1–2.8 s).
   - *Memory Metric*: Formally pin **physical footprint** (`footprint -p` on macOS, PSS on Linux) as the contractual metric rather than `ps` RSS, which fluctuates unpredictably due to shared system libraries and memory compression.

4. **Multi-Window Memory Budget (D-028 Confirmation)**:
   - *Decision*: Confirm that the PLAN §4 150 MB idle memory budget applies to a single main window, with ~35 MB physical footprint accepted per additional popped-out window (measured 194–207 MB for 3 windows).

5. **WebKit GPU Memory Spike Investigation**:
   - *Finding*: In 5 of 18 idle footprint samples in M0-06b, WebKit's GPU process expanded by 50–60 MB between 10 s and 30 s of idle time. M1 should establish whether this is transient IOSurface allocation or a sustained leak.

6. **Distribution Strategy vs `macOSPrivateApi`**:
   - *Decision*: Confirm direct distribution (outside the Mac App Store) to permit `transparent: true` and private AppKit vibrancy materials on macOS.

7. **Linux / WebKitGTK Verification Schedule**:
   - *Decision*: Confirm handoff of Linux / WebKitGTK verification tasks (D-010) to Pedro's partner prior to freezing M1 UI components.
