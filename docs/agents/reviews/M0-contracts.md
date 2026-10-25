# Review: M0 contracts and briefs

Reviewer: GPT-6 Astra (`codex exec -s read-only`, detached worktree at `f3fc75d`) · Requested by Pedro, 2026-10-24 · Triage: orchestrator (Opus 5.5)

## Orchestrator triage

| # | Severity | Finding (short) | Triage | Planned fix |
|---|---|---|---|---|
| 1 | blocker | `Ask` only sees what the provider already gates; the hard-deny list isn't enforceable on everything | **Accept, partly reversed by D-011** (`FullAccess` stays available on the host) | Every adapter must intercept *every* tool call before execution where the provider offers a hook (Claude `PreToolUse` hook + `can_use_tool`, the `agy` hook, Codex/ACP: find out); per-provider enforcement matrix in each spike report; `FullAccess` + `Transport::Local` → `Unsupported`. Scope of hard-deny is a driver question (security default). |
| 2 | blocker | `agy` hook failure modes (crash, bad output, timeout) could let a tool run | **Accept** | M0-03 DoD: orchestrator-run live tests of each failure mode on the pinned `agy`; a retry grant is bound to session + tool + canonical args, used once. |
| 3 | major | Fixture replay sees only stdout, so approval bridges (hook socket) can't be replayed | **Accept** | `Normalizer::on_input(AdapterInput)` with stdout, stderr, hook request/response, outbound command and exit; fixture `Direction` gains `hook` and `command`. |
| 4 | major | Backpressure can block interrupt/shutdown | **Accept** | Contract docs: commands are served even when the event queue is full; bounded pending-message queue; startup/shutdown deadlines; child kill + reap. Saturation test in every adapter DoD. |
| 5 | major | Approvals/questions never get a terminal state | **Accept** | New `RequestResolved { request_id, outcome }` event. |
| 6 | major | Multi-select answers joined with `, ` are ambiguous | **Accept** | Options get ids; answers are `{question_id, selected: [ids], text}`. |
| 7 | major | `ToolCallUpdated` can't carry input/title/kind changes | **Accept** | Optional replacement fields on `ToolCallUpdated`. |
| 8 | major | Quota updates ≠ being rate-limited; `Usage` semantics undefined | **Accept** | New `QuotaUpdated`; `RateLimited` means blocked. `Usage` = current turn's running total (replaces, never adds). |
| 9 | major | Reconnect cursor and command retry semantics undefined | **Accept** | Cursor = last *applied* event; commands are never auto-retried; a lost response returns an "unknown outcome" error. |
| 10 | major | `Subscribe` can't fail explicitly | **Accept** | `Subscribe` gets a `request_id` and a `Response`; cursor validation rules. |
| 11 | major | `AgentLaunch::Native` lacks policy/resume/effort | **Accept (placeholder)** | Mark `Native` as an M0 placeholder, explicitly not frozen for M1. |
| 12 | major | Socket permissions and PID-reuse in orphan reaping | **Accept** | M0-05: private 0700 dir, 0600 socket, owner/symlink checks, lock before stale cleanup, PID + start time. |
| 13 | major | String-replace sanitizing can corrupt keys and miss secrets | **Accept** | Structured sanitizing: secrets first, string values only, keys untouched; collision tests. |
| 14 | major | Consistency invariants break on startup-failure scenarios | **Accept** | Two lifecycles (pre-session failure vs established session); `on_exit` closes open turns/messages. |
| 15 | minor | Measurements: first paint vs interactive, Linux, benchmark params | **Accept** | Time-to-interactive; fixed benchmark params; Linux deferral recorded with an owner. |

Nothing rejected. Fixes 1–14 change the contracts, so they're applied before approval and the whole set goes back to the driver.

## Questions for the driver (from Astra, with the orchestrator's recommendation)

1. **Hard-deny on every operation, or only on what the provider submits for approval?** Recommend *every operation where a pre-execution hook exists*; a provider without one is documented as "sandbox recommended". This sets a security default, so it needs both drivers (D-007).
2. **Is the wire contract meant to carry M1 native sessions?** Recommend: no. `Native` is an M0 placeholder, and M1 defines it.
3. **Linux verification**: defer explicitly, and who owns it?
4. **Approval-default changes from this review**: yes, both drivers sign off (D-007).

## Astra's full review

## Verdict

**Rework before freezing.** The decomposition is sensible, the adapter trait is dyn-compatible, and its Tokio channels are bounded. However, approval enforcement, fixture replay, cancellation, and reconnect semantics need contract changes before implementation begins. Several current requirements would produce incompatible adapters or misleading verification results. This was a read-only review; no files were modified or build tests run.

## Findings

1. **[blocker] Approval coverage does not establish Atrium’s policy authority.**  
   `crates/atrium-agents/src/lib.rs:62`, M0-02 “Contract to implement,” and `PLAN.md:312`: `Ask` delegates only actions the provider already gates; Codex explicitly uses workspace-write with approval on escalation. That leaves no demonstrated interception point for Atrium’s hard-deny rules on otherwise permitted operations. Read-tool auto-allow also conflicts with denying reads of credential stores. Additionally, `FullAccess` is described as sandbox-only, although the only available transport is `Local`.  
   **Fix:** Define a provider-by-provider enforcement matrix, including inherited permission rules and MCP tools. Require proof that prohibited operations are intercepted before execution; return `Unsupported` where the promised policy cannot be enforced. Reject `FullAccess + Local` under the current contract.

2. **[blocker] The agy hook’s failure cases are not covered by the safety gate.**  
   `docs/agents/briefs/M0-03-agy-adapter.md:13`: under skip-permissions, anything except an explicit deny runs. The required fake-client test covers an adapter disappearing while a functioning helper waits; it does not establish what happens when the helper cannot launch, crashes, produces malformed output, or exceeds agy’s timeout. The fallback also lacks a rule binding approval to the exact retried operation.  
   **Fix:** Require orchestrator-run tests against the pinned CLI for all these failures, plus config loading and hook coverage. Any failure that permits execution must block host use. Bind retry grants to session, tool, canonical arguments, and one consumption; changed arguments require another decision.

3. **[major] The fixture interface cannot reproduce the proposed approval bridge.**  
   `crates/atrium-agents/src/lib.rs:107` and `crates/atrium-agents/src/fixture.rs:45`: normalization receives only stdout and process exit. agy approval requests arrive over a separate socket, and outgoing requests/decisions are discarded during fixture replay. Consequently, its approval goldens cannot faithfully reproduce the live session through the frozen interface.  
   **Fix:** Introduce a replayable adapter-input enum covering provider output, outbound commands/request correlation, hook requests and responses, and process exit. Drive live normalization and golden replay through the same state machine.

4. **[major] Backpressure can prevent interruption and shutdown unless control handling is specified separately.**  
   `crates/atrium-agents/src/lib.rs:38` and `crates/atrium-protocol/src/agent.rs:227`: bounded channels are present, but there is no requirement that commands remain serviceable while event delivery is blocked. Moving queued user messages into an unrestricted internal queue would also defeat the channel bound. Shutdown behavior for queued turns, pending approvals, startup cancellation, and unresponsive children is unspecified.  
   **Fix:** Specify bounded pending-message storage, responsive control handling under a full event queue, startup/shutdown deadlines, and child termination/reaping. Add saturation tests that interrupt and shut down without draining events.

5. **[major] Pending approvals and questions have no normalized terminal state.**  
   `crates/atrium-protocol/src/agent.rs:90`: requests can be created but never explicitly resolved, cancelled, or expired in the event stream. After interruption or provider-side cleanup, a replaying client cannot reliably reconstruct which inbox items remain actionable. Codex explicitly reports server-request resolution, including cleanup on interruption. [App-server documentation](https://developers.openai.com/codex/app-server/)  
   **Fix:** Add a request-resolution event with request ID and outcome. Define late/duplicate response rejection and cleanup on interruption, turn completion, and session termination.

6. **[major] Multi-select answers lose information.**  
   `crates/atrium-protocol/src/agent.rs:235`: joining answers with `", "` makes selecting one option named `"one, two"` indistinguishable from selecting `"one"` and `"two"`. The adapter cannot reconstruct the user’s choice reliably.  
   **Fix:** Use structured answers, such as selected option IDs plus optional free text, and give question options stable IDs.

7. **[major] Tool updates cannot carry changes to tool input or metadata.**  
   `crates/atrium-protocol/src/agent.rs:77`: `ToolCallUpdated` supports only appended text output. ACP updates can supply or change input, title, kind, content, and locations after the initial tool call. Emitting another start violates the exactly-one lifecycle; hiding these updates in `Unmapped` prevents normalized consumers from using them. [ACP tool-call specification](https://agentclientprotocol.com/protocol/v1/tool-calls)  
   **Fix:** Add explicitly defined metadata/input update fields and distinguish replacement content from appended output. Document how partial provider arguments become complete input.

8. **[major] Rate-limit updates are incorrectly equated with exhausted limits.**  
   `docs/agents/briefs/M0-02-codex-adapter.md:24` and `crates/atrium-protocol/src/agent.rs:113`: the brief maps rate-limit notifications to an event whose documented effect is pausing the thread. Codex sends updates for ordinary quota changes, including usage well below exhaustion. [App-server rate-limit documentation](https://developers.openai.com/codex/app-server/)  
   **Fix:** Separate quota-window updates from an actual rate-limit blockage. Define usage scope as well: whether `Usage` is incremental, per-turn, or cumulative, and whether `TurnCompleted.usage` repeats an earlier report.

9. **[major] Reconnect cursors lack an application-delivery boundary, and command retries are undefined.**  
   `crates/atrium-protocol/src/wire.rs:23` and `docs/agents/briefs/M0-05-daemon-socket.md:20`: “last seen” could mean read from the socket, queued in the client, or applied by its consumer. Advancing before application can lose buffered events on reconnect; replaying them requires deduplication. Separately, losing a connection after executing `StartThread` but before its response leaves the client unable to retry safely.  
   **Fix:** Define the cursor as the last applied event, with acknowledgement or documented client deduplication. Specify either durable command idempotency keys or an explicit unknown-outcome error with no automatic command retry. Test both disconnect windows.

10. **[major] Subscription failures cannot be represented explicitly.**  
    `crates/atrium-protocol/src/wire.rs:80`: subscriptions have no request ID or error response. Unknown threads, invalid/future cursors, and repeated subscriptions have no defined outcome. `ReplayDone` cannot distinguish an empty valid subscription from a failed one.  
    **Fix:** Add subscription acknowledgement/error semantics and define cursor validation, duplicate subscription behavior, and the replay high-water mark.

11. **[major] Native thread launch omits configuration required by M1.**  
    `crates/atrium-protocol/src/wire.rs:97` and `:117`: the socket can launch a native provider with only provider, model, and cwd. It cannot request permission policy, resume, or effort, despite those existing in `SessionConfig` and Home/resume being explicit M1 requirements.  
    **Fix:** Define a serializable native launch configuration now, with daemon-controlled policy validation and server-side MCP injection. Alternatively, explicitly mark native launch as an incomplete M0 placeholder rather than a reusable frozen contract.

12. **[major] Socket access controls and safe orphan identification are missing from M0-05.**  
    `docs/agents/briefs/M0-05-daemon-socket.md:22`: the brief specifies a predictable directory, socket, and lock but no owner/mode checks or safe handling of pre-existing paths. At line 26, stored PIDs alone are insufficient to identify orphans after PID reuse.  
    **Fix:** Require a daemon-owned private directory, restricted socket permissions, rejection of unexpected ownership/symlinks, and lock acquisition before stale-socket cleanup. Record and verify process identity beyond PID before signaling; skip ambiguous processes. Test unsafe existing paths and PID-identity mismatch.

13. **[major] Sanitization can corrupt protocol records and miss listed secrets.**  
    `crates/atrium-agents/src/fixture.rs:89`: global username replacement can alter keys or event names—for example, user `"type"` changes JSON `"type"` keys. Because it runs before secret replacement, a secret containing the username is changed before the exact secret match and escapes full redaction. JSON-escaped values are another gap in raw substring matching.  
    **Fix:** Sanitize decoded structured values, redact secrets before identity substitutions, preserve protocol keys/discriminators, and validate the resulting records. Add overlap, escaping, and protocol-key collision cases; review generated goldens and metadata too.

14. **[major] The consistency brief requires invariants that valid failure scenarios cannot satisfy.**  
    `docs/agents/briefs/M0-09-m0-findings.md:21`, `docs/contracts/m0.md:46`, and `crates/atrium-agents/src/lib.rs:112`: every fixture must start with `SessionStarted`, yet an invalid-model/startup failure may never create a session. Every turn must complete, but exit handling only mandates closing tools and emitting `SessionEnded`.  
    **Fix:** Define separate pre-session failure and established-session lifecycles. Specify interrupted/failed closure for open turns and messages, then make consistency assertions conditional on that lifecycle. Do not fabricate a native session ID to satisfy the test.

15. **[minor] The measurement briefs do not fully verify the stated M0 gate.**  
    `docs/agents/briefs/M0-06-ui-shell.md:39` measures first paint, whereas `PLAN.md:694` budgets time to interactive. M0-06 and M0-08 exclude Linux despite explicit M0 Linux measurements. M0-05’s 100k-event reconnect benchmark leaves the starting cursor and event sizes unspecified, making its result non-comparable.  
    **Fix:** Measure interactivity separately, define benchmark payload/cursor conditions, and record an explicit driver-approved Linux deferral with an owner and later gate.

## Questions for the driver

- Must Atrium enforce its hard-deny list on **every** provider operation, or only on operations the provider submits for approval? The current plan promises the former without establishing that capability.
- Is the frozen wire contract intended to support M1 native sessions, or may M1 deliberately replace the M0 launch/subscription API?
- Will Linux verification be deferred explicitly, and who owns the outstanding runs?
- Should approval default changes arising from this review require both drivers’ sign-off under D-007?