# M0 gate review

Reviewer: GPT-6 Astra (read-only, snapshot `666ce2d`, D-031) · 2026-10-26 · Verdict: **pass with conditions** (to start the M1 contract phase).

Note: the snapshot predates D-032 … D-034, so the review calls `macOSPrivateApi` "pending" and the 3-window budget "undecided"; both are now decided (D-034, D-028).

## 1. Gate verdict

**Pass with conditions—to start M1’s contract phase.** M0 established a workable architecture and exposed meaningful provider limitations. It did **not** establish production-ready enforcement or an unconditional performance-budget pass.

Conditions:

- **Blocker before M1 contracts freeze:** define enforcement per session, the scope of “hard deny,” and the daemon’s trust boundary. Preserve D-012/D-017/D-019’s user choices without implying guarantees the adapters cannot provide.
- **Blocker before affected adapters enter production:** retain D-018’s promotion gates, strengthen A1, and add the missing findings below. M1’s Claude/Codex scope need not wait for production agy/ACP.
- **Major:** approve the protocol changes, specify performance acceptance statistics, and give the Linux deferral a completion gate—not just an owner.
- **Major:** correct the consolidated summary’s overstatements. D-028 is already decided; direct distribution is still pending; agy currently returns `AllToolCalls`; full-history replay and all idle-memory samples do not pass their targets.

This was a static, read-only review. I modified nothing and ran no applications, benchmarks, or tests; cited test results are recorded evidence.

## 2. Security review

**Blocker — “Hard deny” is coherent only as precedence within intercepted calls.** D-012, D-017 and D-019 are internally consistent with that limited meaning. They cannot support a machine-wide prohibition. PLAN §3.12’s final qualification acknowledges this, but “Trusted: everything except the deny list” and Home’s “any change needs approval” still invite stronger interpretations.

Even `AllToolCalls` means all *tool invocations* are inspected, not all effects of allowed code. An approved `cargo test` can execute a modified build script that reads credentials or deletes files. Command-prefix matching cannot make that safe. M1 must distinguish **interception coverage**, **policy decision**, and **OS isolation**.

| Provider | Concrete bypass and required disposition |
|---|---|
| **Claude** | External hooks can execute host commands independently or rewrite approved inputs. The fail-closed responses for malformed requests, gate panic and timeout are good. However, plugin hooks and explicit `--settings` files are excluded from detection, yet absence of detected hooks produces `AllToolCalls`. A plugin can therefore run outside the gate while the UI says Full. **Major:** report incomplete inspection conservatively and bind the result to the session’s effective configuration. See [settings.rs:12](/Users/pedro/Documents/atrium-wt/M0-gate-review/spikes/agent-claude/src/settings.rs:12), M0-01 “External settings.” |
| **Codex** | `ProviderGated` is correct. Provider-trusted operations can bypass Atrium; FullAccess uses `approvalPolicy=never`, so a credential read or destructive command need not reach the deny list. The sampled denied read proves that invocation, not universal coverage. Keep X1–X3 and resolve the recorded schema/runtime policy discrepancy. See M0-02 “Exact policies and enforcement matrix.” |
| **agy** | A same-user process can replace valid `hooks.json` after validation. agy ignores malformed configuration under skip-permissions and executes before the tripwire reacts. Aggregate allowance credits can conceal another ungated call. **Blocker for promotion:** A1 remains unresolved; verification inside the first hook cannot protect the case where no hook loads. Require provider-side fail-closed loading or an enforced boundary protecting configuration. A downgraded badge alone does not satisfy the fail-closed contract. The current [adapter.rs:183](/Users/pedro/Documents/atrium-wt/M0-gate-review/spikes/agent-agy/src/adapter.rs:183) still advertises `AllToolCalls`. |
| **OpenCode** | Code Mode’s model-written JavaScript executed `fetch` without permission interception. Keeping it enabled with Partial enforcement follows D-019. Turning it off must withstand `build`, custom-agent, managed and persisted permission overrides. **Blocker for any “Full” claim:** disabling `execute` alone does not prove MCP/subagent coverage or eliminate other permission bypasses. D-019’s parenthetical “Full enforcement” is premature. See M0-04 §§3–4 and review finding 1. |

**Blocker for approval integrity — cancelled requests can still authorize effects.** agy’s bridge can issue a retry grant after the normalizer rejects a stale Allow; ACP can execute a pending filesystem/terminal operation after cancellation. Reconnect or a second window supplies a realistic stale-response path. Elevate A4/O2 to promotion blockers: one authoritative request state must validate and consume approval before any I/O. Reviews M0-03 #4 and M0-04 #2 document both paths.

**Major — socket permissions protect against other users, not host agents.** M0-05’s private directory, owner/mode checks, lock-before-cleanup and staging-socket publication are sound. But `Hello` authenticates only protocol compatibility, and commands have no client role check ([daemon.rs:531](/Users/pedro/Documents/atrium-wt/M0-gate-review/spikes/daemon-socket/src/daemon.rs:531), `handle` at 630). An unrestricted same-UID agent can connect, read other threads and, once native forwarding is implemented, send `ResolveApproval::Allow` as a UI client.

M1 must separate scoped agent/MCP authority from human-control authority and explicitly state the remaining same-UID limitation. Peer UID checks or a token readable by that same agent do not create isolation.

**Major — reaping is improved, but termination guarantees remain incomplete.**

- M0-05 verifies PID/start/executable and handles ESRCH, but kills only the leader. A tool descendant can continue writing or transmitting after its thread becomes Interrupted. Add shared process-group supervision for Codex, agy and daemon recovery; process groups still do not contain deliberately detached descendants.
- Recovery marks threads closed even when identity cannot be verified. Surface **cleanup unverified**, rather than implying execution stopped.
- M0-08’s profile lock, identity checks, orphan check and re-verification before escalation substantially improve safety. However, `ProfileLock::acquire` lacks the daemon’s owner/mode/symlink validation ([profile.rs:54](/Users/pedro/Documents/atrium-wt/M0-gate-review/spikes/browser-screencast/src-tauri/src/profile.rs:54)). Require a private validated profile root and safe record creation. Reaping only on the next launch leaves Chromium active indefinitely if no restart occurs.

**Major — browser contexts establish storage separation, not a security boundary.** M0-08 proves cookie/localStorage/sessionStorage separation for tested contexts. It does not isolate host network access, browser-process compromise or daemon authorization. A cross-thread target-ID error could expose another thread’s authenticated page through `browser_eval` or screenshots. Bind every browser operation and frame attachment to an authorized context; retain the fixed generation checks and takeover cancellation/drain semantics. PLAN §3.13 should narrow “isolation” accordingly.

**Major — secret handling still has concrete leaks.**

- A12/A13 correctly cover fragmented and escaped disclosure; apply those tests across providers, on decoded/reconstructed content **before** redaction.
- The common sanitizer ignores `AdapterInput::Command`, and `is_clean` checks literal serialized strings. Prompts and answers require shared coverage, not adapter-specific patches ([fixture.rs:130](/Users/pedro/Documents/atrium-wt/M0-gate-review/crates/atrium-agents/src/fixture.rs:130)).
- `SessionConfig::Debug` prints `mcp_servers`; `McpServer` derives `Debug` including `env` and `args`. Logging configuration can disclose MCP credentials despite omitting top-level `env` ([lib.rs:118](/Users/pedro/Documents/atrium-wt/M0-gate-review/crates/atrium-agents/src/lib.rs:118)).
- M1 exports history, while PLAN places the general scanner later. Export redaction and private logs/configuration storage must accompany M1 exports.

**Major — the build’s agy policy remains a guardrail.** D-006 correctly avoids affecting Pedro’s sessions; D-016 catches broken launch-time configuration. Neither prevents later tampering. The allowlist permits `node`, build scripts and other interpreters: code can construct the `.agents/hooks.json` path dynamically, overwrite it, then exploit agy’s fail-open behavior. Moving the policy outside the worktree does not stop same-UID code modifying it. D-002 already acknowledges this trust model; keep it explicit, re-test upgrades, and use OS confinement if stronger protection is required. See `scripts/agy-policy.py`, `scripts/agent-run:51`.

**Minor — opacity is chiefly a distribution/accessibility issue.** `macOSPrivateApi` is not evidence of a permission bypass. Record the pending distribution decision; the summary incorrectly calls it accepted. Native whole-window alpha is technically appropriate, but neither a 30% floor nor opaque-theme contrast validation guarantees readable approval text over arbitrary backgrounds. Preserve accessibility overrides and test approval surfaces at minimum opacity. Sources: CR-04; ui-direction §5; M0-06b §6.

## 3. Architecture review

**Keep the overall decomposition.** Pure normalizers plus recorded `AdapterInput`, daemon-owned persistence, pull-based replay, and a thin client survived real protocol differences. ACP’s use of schema types with custom framing is reasonable given its SDK/runtime mismatch. Most adapter findings are implementation defects, not reasons to abandon `AgentEvent`.

**Major — revise these contracts before fan-out:**

1. **Session-scoped capabilities and enforcement.** `AgentAdapter::interception(&self)` cannot describe concurrent sessions with different settings. Claude’s “last start” implementation demonstrates the mismatch. Return/persist a session descriptor containing effective enforcement, reasons, provider/version, verified tool coverage and supported features. Unknown coverage must not become Full.
2. **Authoritative approval state and effects.** Keep the normalizer/effect separation, but a rejected normalized command must prevent runtime execution. Bind grants to session incarnation, request, tool, canonical arguments and policy context; expire and consume them once. Represent hook disconnection, deadlines and failed writes in replayable inputs where they affect outcomes.
3. **Lifecycle completion under backpressure.** Separate bounded process cleanup from eventual ordered delivery. This is also missing for Claude: [adapter.rs:696](/Users/pedro/Documents/atrium-wt/M0-gate-review/spikes/agent-claude/src/adapter.rs:696) abandons pending events after 300 ms. A later reader can miss `SessionEnded`. Reserve terminal delivery capacity or retain a bounded terminal tail until consumption/disconnection; test cleanup independently of draining.
4. **Native launch and identity.** Replace the intentional M0 placeholder with validated policy/resume/effort/provider options and daemon-owned MCP configuration. Specify session incarnations across resume, since sequences, turn numbers and native request IDs can restart.
5. **Usage and diagnostics.** Preserve per-turn usage; add separate context-occupancy semantics rather than putting ACP context size into input-token totals. Define where pre-session diagnostics live without violating the two-event failure lifecycle.
6. **Wire limits beyond framing.** Add byte limits for provider records, partial arguments, outputs and pending state; deadlines and connection/subscription quotas; and a control path that remains responsive when replay or a hidden webview stalls.

**CR assessment:**

| Request | Recommendation |
|---|---|
| **CR-01** | Accept shared scrim/shadow/blur tokens; specify syntax-token mapping and fallbacks. |
| **CR-02** | Accept reserved runtime tokens, derived surface channels and state tokens. Validate collisions across every layer; opaque contrast checks must remain explicitly limited. |
| **CR-03** | Accept the direction, amend before freezing. Define session-scoped reconstruction, hashing, retained-delta access and bounded verification. An older-client `Inline` fallback cannot work for text exceeding the frame limit: reject that negotiation or provide chunking. Large completion-only messages and tool outputs also need handling. |
| **CR-04** | Accept: native alpha, accurate blur semantics, reserved derived names and live accessibility observation match the platform evidence. |

**Major — retain the daemon/client split and replay design, but add projections.** SQLite paging plus wake-only notifications avoids a replay/live race; applied cursors and no automatic command retries are sound. However, `close_thread` materializes the entire history ([daemon.rs:325](/Users/pedro/Documents/atrium-wt/M0-gate-review/spikes/daemon-socket/src/daemon.rs:325)). Persist open-object projections transactionally; do not carry the 487 MB close-time fold into M1. The <100 ms result covers a cursor 1,000 events behind, not full reconstruction: 100k-event replay took approximately 334–340 ms.

**Major — adopt JSON slim and 8 ms batching as defaults, not a memory solution.** M0-07 supports their simplicity and throughput. The 1.05–1.39 GB WebContent transient and approximately 250 MB retained transcript remain unresolved; changing encoding did not remove them. Profile allocations/native memory, bound loaded history and virtualizer state, and test concurrent build load. Frame rate alone does not capture the recorded upstream latency tails.

**Major — D-028’s multi-window model is reasonable.** Keep one Rust client/state authority, per-view ownership and revisioned updates. Each window needs independent delivery/ACK state so an occluded pop-out cannot stall other windows or approval commands. Test forced close, crash, stale ownership and draft recovery. The measured ~35 MB marginal idle cost does not predict streaming memory multiplied across windows.

## 4. Risks the plan doesn’t yet cover

**Major — resource exhaustion and durable failure recovery need explicit requirements.** An agent can produce oversized JSON, unlimited output or enough events to fill disk. Specify output limits, storage quotas, disk-full behavior, atomic event/projection updates and recovery after interrupted migrations. “Bounded channel” does not bound message bytes, persistent history or connection count.

**Major — native configuration and version drift are part of the trust model.** Plugins, managed settings, persisted permission grants and CLI upgrades can invalidate enforcement without changing Atrium. Version-pin tested behavior, record configuration provenance, and invalidate stale coverage claims. PLAN §6’s agy “sandbox-only if hooks break” mitigation also conflicts with D-012’s never-restrict policy; resolve that explicitly.

**Major — rendered agent content needs an M1 security contract.** The streaming renderer escapes raw HTML, but explicitly leaves link-scheme allowlisting for M1 (`spikes/stream-perf/src/markdown.ts`). Define safe navigation, image loading and Tauri invocation boundaries before real transcripts become interactive. Agent-generated links must not become arbitrary native actions.

**Major — correct the must-fix priorities and omissions:**

- Keep A1 a blocker, replacing its insufficient suggested remedies.
- Elevate A4/O2 to approval-integrity blockers; make O1 a blocker for the promised Code Mode-off behavior, without reversing D-019’s default.
- Add Claude coverage detection and terminal-event loss, shared process supervision, same-UID daemon authority, credential-safe Debug, export redaction, lifecycle projections and streaming-memory bounds.
- Promote O9 from minor where resume is promised: the wrong RPC can prevent session recovery entirely.
- X6 needs both adapter fixes and a stronger checker. The checker inspects goldens rather than actual execution, permits some duplicate request/completion cases, and tracks cancelled-request ordering across the entire session rather than a closure episode. Zero violations is not proof of live enforcement.
- Assign requirements to the milestone that promotes each component. PLAN puts ACP in M2 and browser integration in M3; “M1 must-fix” should not obscure those ownership boundaries.

**Major — budget acceptance remains conditional.** In M0-06b, three of nine one-window launches reached **192–199 MB** at 30 seconds; two three-window launches reached **250–259 MB**. Those are observed excursions, not merely a theoretical GPU risk. Define metric, sampling duration and percentile explicitly. Likewise, distinguish warm-cache startup from fresh launch, synthetic input from OS input, and canvas/rAF timing from actual presentation. Linux, real IME and 120 Hz remain unverified.

## 5. Process

**Major — keep the cross-provider reviews, but require closure evidence at the merged revision.** Reviews found substantive defects: ineffective reaper tests, stale approvals, replay duplication and measurement artifacts. After a final fix round, require a focused reviewer verification tied to the final commit. An author’s “all fixed” statement is insufficient.

**Major — D-018 was a reasonable spike exception, not a green implementation gate.** Preserve explicit promotion blockers with owners and failing-first tests. Two exhausted fix rounds should trigger escalation or architectural reconsideration. The ignored consistency test must remain visible as a known failure; ordinary green Cargo output does not satisfy PLAN §8.4’s “all checks green.”

**Major — make measurement evidence reproducible and comparable.** Keep raw failures, distinguish independently evidenced environmental invalidation, pin configuration/provider versions and report pre-fix versus post-fix measurements separately. Run normal scheduling and representative contention. The consolidated report should inherit qualifications, not upgrade medians or partial workloads into unconditional passes.

**Major — turn D-030 into a machine preflight.** Resolve real checkout/worktree/git-directory paths and verify they are outside sync roots. The incident altered `.git`, tracked docs and build inputs; ignoring `* [0-9].*` merely hides future symptoms. Keep integrity checks and inspect ignored conflict copies at handoffs.

**Minor — routing calibration is useful but still a small sample.** D-027 supports routing UI-heavy work toward Sonnet/Opus and systems work toward Sol, while retaining independent review. Budget provider quota and host verification time in briefs so missing live evidence is tracked deliberately rather than discovered at integration.