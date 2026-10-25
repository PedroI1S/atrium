# M0-02: Codex app-server adapter

Status: fix round 2 implemented offline; all 13 scenarios then recorded **live** (codex 0.163.0, `gpt-5.6-luna`) by the orchestrator and committed as the primary fixtures; escalation fixed the tests. Live facts are summarized in "Live verification results" below. Rows still marked verify live were not exercised.
Installed CLI: `codex-cli 0.163.0` (`codex --version`).

## Implementation

`spikes/agent-codex` implements `AgentAdapter` and the pure `CodexNormalizer` over
newline-delimited stdio JSON-RPC. It initializes, starts/resumes native threads,
starts turns, queues messages, interrupts and owns shutdown/kill/reap. It never
fabricates native session ids. The [official app-server reference](https://learn.chatgpt.com/docs/app-server)
describes the thread, turn and approval lifecycle; local generated schemas pin
this spike's actual shapes.

Command/file approval requests preserve numeric versus string JSON-RPC ids on the
wire; normalized ids are strings. Each intercepted approval consults the gate.
Ask exposes `ApprovalRequested`; client decisions reply to the server.
`serverRequest/resolved` closes exposed approvals/questions exactly once.
Unknown, duplicate and invalid answers emit nonfatal errors. Only single-call
`accept` grants are sent. Gate panic/timeout denies; interrupt cancels gate tasks.
Unsupported server requests receive a JSON-RPC error rather than authorization.

The mapper handles assistant/reasoning text, command/patch/MCP/web/collaboration
items, replacement tool inputs/outputs, plans, token totals, quota windows and
explicit limit refusals. It closes messages, tools, exposed requests, turns and
sessions in contract order. Initialization diagnostics wait until SessionStarted
or a fatal pre-session failure.

Every stdin write, stdout/stderr record, gate decision, client command and exit
passes through `AdapterInput` and the same normalizer used by replay. A writer
task acknowledges writes; stdin submissions are recorded in enqueue order before
stdout responses. Write failures produce transport diagnostics. Stdout never
waits on an outstanding stdin write, avoiding circular pipe backpressure. Transport
diagnostics also pass through the mapper as stderr records.

Recording is opt-in and memory-resident. Before writing, examples sanitize decoded
nested values, including prompts/answers the base sanitizer does not rewrite.
`Sanitizer::is_clean` is asserted on transcripts read back from disk and on golden
output. Global Codex configuration is never edited; MCP servers are supplied
through quoted process-local `-c mcp_servers.<name>.command/args/env.<key>=...`.

## Exact policies and enforcement matrix

| Atrium policy | thread `sandbox` | thread `approvalPolicy` | `approvalsReviewer` |
| --- | --- | --- | --- |
| ReadOnly | `read-only` | `untrusted` | `user` |
| Ask | `workspace-write` | `untrusted` | `user` |
| FullAccess | `danger-full-access` | `never` | `user` |

Settings also apply on resume. FullAccess is accepted on the local host; any
approval the provider nevertheless sends still passes the gate.

**Compatibility finding:** the installed binary's generated `AskForApproval`
schema includes `untrusted`, the strongest legacy candidate implemented here.
Current [official sandbox documentation](https://learn.chatgpt.com/docs/sandboxing)
says the selectable policy is retired. The orchestrator verified runtime acceptance in the live simple run. Exact
enforcement still needs live verification. There is no automatic downgrade to `on-request`: rejection
fails startup. Resolve this schema/runtime discrepancy before M1.

`interception()` always returns **ProviderGated**. The schema does not establish
any setting that sends every tool call through the client. An approval transcript
for a read does not establish that every read asks; the same caveat applies to
safe commands and writes inside provider-granted roots.

| Tool class | Intercepted before execution? | How | Evidence and limitation |
| --- | --- | --- | --- |
| Read | partial; **live verified** for the sampled reads | command approval request, when issued | Live: `tool-read` (compound `rg`/`sed` read of `public.txt`) and `gate-deny` (`cat secret.txt`) both raised `item/commandExecution/requestApproval` under ReadOnly/untrusted. `gate-deny` was **denied, sentinel absent, no native execution**. Two samples do not prove every read or "safe" command asks. |
| Edit/write | partial, **not live verified** | `item/fileChange/requestApproval`, when issued | The live `patch` recording ended in a `usageLimitExceeded` refusal (mapped to `RateLimited`) before any file change, so the file-change request/mapping is proven only on the hand-made `handmade/patch` trace. Re-record `patch` when quota allows. |
| Delete/move | partial | command/file approval, when issued | shared routing only; no live delete/move evidence. |
| Execute | partial; **live verified** for the sampled commands | `item/commandExecution/requestApproval` | Live: `gate-deny-execute` **denied, sentinel absent, no native execution**; `approval-allow` (Allow, command ran) and `approval-deny` (denied) work; `interrupt` cancels a pending `sleep 60` (tool Cancelled, turn Interrupted). Fake-server panic denial is offline. Safe-command bypass not ruled out. |
| Search | partial | command approval, when issued | action classification; universal interception unproven. |
| Fetch/web | no | item notifications are observations | web items map to tools; no universal authorization request established. |
| MCP | partial, verify live (not exercised: recordings inject no MCP servers) | injected servers use `default_tools_approval_mode="prompt"`; `mcpServer/elicitation/request` routes through the gate | Offline allow/deny/Ask/cancel tests pass; elicitations do not establish coverage of all MCP calls. |
| Subagents | no | collaboration notifications | observed as tools; universal authorization callback unproven. |
| Extra permissions/dynamic tools/auth refresh | unsupported | server requests return `-32601` | no execution or permission grant is authorized. |

Hand-made fixtures prove mapping only. The live recordings add real wire evidence
for the rows marked live above; they are samples, not coverage proofs. The brief's
stop condition (writes or non-read commands bypassing the client under every
setting) was **not observed**: no non-read command or write ran without a request
in any live scenario. Writes are the unverified part (live `patch` hit the quota).

## Lifecycle, bounds and saturation

Channels and queued messages use the frozen contract constants. Normal output
leaves channel capacity for lifecycle cleanup; provider reads pause before those
slots are consumed. Commands, gate completion, stdin acknowledgements and shutdown
remain selectable under event backpressure. Flooding client errors without
reading events triggers bounded shutdown before cleanup capacity is exhausted.

Shutdown closes stdin, aborts/joins the writer and all gate tasks, permits a grace
interval, then kills and reaps the child. The kill threshold leaves a small
scheduling margin inside `SHUTDOWN_GRACE`. No detached event-forwarding task is
retained. The saturation test never reads events before shutdown, verifies that
the child consumed `turn/interrupt`, checks reaping via `kill -0`, and then drains
through SessionEnded. It covers both cooperative exit and a child ignoring EOF.
Interrupts during turn startup are retained until the native turn id arrives.
Dropping both session halves also reaps the child.

```sh
cargo test -p spike-agent-codex --test live saturation_interrupt_shutdown_and_forced_kill
cargo test -p spike-agent-codex --test live interrupt_cancels_stalled_gate_without_leaving_task
cargo test -p spike-agent-codex --test live interrupt_during_turn_start_is_not_lost
```

## Fixtures and protocol stability

All required scenarios have separate golden tests: simple, tool-read, gate-deny,
approval-allow, approval-deny, question, interrupt, multi-turn, resume, startup-error.
Additional scenarios cover execution denial, patches and quota/usage. Fixtures are
split by purpose:

- `fixtures/agents/codex/<scenario>.*`: **live recordings** (codex 0.163.0,
  `gpt-5.6-luna`), the primary scenario fixtures with a golden test each. The
  `*-startup-failure` and `resume-origin` files are auxiliary live traces.
  Regenerate goldens with `UPDATE_GOLDEN=1` only after re-recording, and review the diff.
- `fixtures/agents/codex/handmade/<scenario>.*`: **hand-made** synthetic traces
  (`approval-allow`, `interrupt`, `question`, `quota-usage`, `patch`) that unit and
  semantic tests need for specific content live Codex did not produce: the
  question round trip, `RequestResolved(Cancelled)` after interrupt, per-turn totals
  `[10, 25, 5]` with a refusal and an unknown record, and the file-change mapping.
  They prove mapping only and are labeled in their `.md`. The `--hand-made`
  recorder flag now writes only here and can no longer overwrite a live recording.

Live deviations from the hand-made assumptions (none violates a contract invariant;
all live streams satisfy the structural invariants: first SessionStarted, last
SessionEnded, every tool call and request closed exactly once):

- `question`: Codex answered in plain text ("I can't use `request_user_input` in the
  current mode") and did **not** ask, so there is no `QuestionAsked` live. The
  experimental question transport is verified only on the hand-made trace.
- `interrupt`: the interrupt raced the gate. The request (id 0) arrived, the
  interrupt was sent before any gate decision, so `ApprovalRequested` was never
  emitted; the late `serverRequest/resolved` has nothing to resolve and emits no
  event. No pending request leaks; the tool ends Cancelled and the turn Interrupted.
- `patch`: refused with `usageLimitExceeded` (`RateLimited`, `resets_at_ms` null,
  empty quota windows, turn Failed); the account's primary window was at 96-99%.
- Reads like `rg`/`sed` compound commands are classified `execute`, not `read`; only
  the plain `cat` is `read` (classification is advisory).

The CLI labels app-server experimental. Local schemas require item timestamps and
session/project fields on threads. `schema/` and raw `live/` files are ignored.

```sh
codex app-server generate-json-schema --out spikes/agent-codex/schema
python3 spikes/agent-codex/tests/support/check_schema.py
```

The structural check reports **717 records passed, 0 errors** on the live corpus, plus six enforcement candidate shape checks for the committed
corpus. It checks references, required fields, types, enums and alternatives with
Python's standard library; it is not a full JSON Schema validator. The deliberate
`future/notification` example is excluded from known-schema checks. Upgrade the
binary alongside schema regeneration, fixture review and real recordings.

## Quotas, usage and Unmapped records

`account/rateLimits/updated` carries primary/secondary windows, used percentages,
reset seconds and duration minutes. Normalized windows retain names, percentages
and reset times converted to milliseconds. Credits, plan metadata and spend-control
fields remain available in raw transcripts, outside the smaller normalized quota
type. Window fullness alone never emits RateLimited. An explicit usage-limit
refusal or HTTP 429 does (unless `willRetry=true`). Duplicate terminal errors are
deduplicated per turn; reset time comes from the last quota update (latest exhausted
window, otherwise earliest known window). It is an estimate, not proof that the
refusal concerned that window; other error variants remain errors/raw records pending
live coverage.

Thread cumulative token counts become per-turn running totals by subtracting the
baseline at turn start, including cached/reasoning counts when present. `last` is
not repeatedly added. Final turn usage is the last running total; costs remain
unknown. If resume does not supply a historical baseline, the first resumed turn's
token records become Unmapped (`missing_resume_baseline`) and final usage stays
unknown rather than charging historical tokens to the new turn. Subsequent turns
can use the observed baseline.

Known transport acknowledgements and user-message items do not need UI events.
Unknown notifications/items, malformed stdout and stderr survive as Unmapped.
The quota-usage golden deliberately proves `future/notification` retention.
Real-world unknown-record frequency is unmeasured.

## Measurements and live verification

**Time to first real event: 416 ms** (SessionStarted at 181 ms in the committed live `simple` recording: thread/start response; turn/started 351 ms; first text delta 1.8 s), reported by the orchestrator from
`cargo run -p spike-agent-codex --example record -- simple` outside the worker
sandbox, using `gpt-5.6-luna`. Session/turn/message/usage/quota ordering passed.
That recording was discarded for privacy issues; it is not a committed fixture.
The original worker attempts to run record/chat failed before handshake. Per the
user's instruction, no real app-server was launched in either fix round.

The live recorder queries model/list in a separate reaped discovery process.
The model schema has no prices: mini/nano preference is a heuristic, and the
verifier should set CODEX_MODEL to the cheapest entitled model after confirming
price. Discovery still runs when an override is supplied. Startup failure does
not establish entitlement or price.

Live `ms` uses a monotonic clock started immediately before child spawn. The
recorder prints the first input timestamp producing a normalized event, measuring
launch-to-first-normalized-event, including startup. Hand-made offsets are
ordering aids, not performance measurements. No fake-server timing is presented
as provider performance.

Run `cargo run -p spike-agent-codex --example record -- <scenario>` for every
scenario listed above, plus gate-deny-execute and patch. The recorder operates in
its owned, ignored scratch directory and bounds scenario duration. Explicit offline
generation is `cargo run -p spike-agent-codex --example record -- simple --hand-made`.

## Validation and M1 recommendation

`cargo fmt --check`, clippy with `--all-targets -- -D warnings`, and
`cargo test -p spike-agent-codex` pass. The latter runs 28 fixture/semantic tests
(live goldens plus hand-made unit traces), 17 subprocess/policy tests
and 9 recording/privacy tests; see the handoff for command results.

Carry the pure mapper, request routing, sanitizer and process-owner tests into M1
after independent review and real recordings. Keep ProviderGated visible to daemon
policy. Full Atrium enforcement requires a real pre-execution mechanism for every
provider tool class.

The orchestrator owns live verification: resolve the retired-untrusted discrepancy,
confirm actual runtime settings, record read/execution denials, exercise safe
commands/writes/MCP/subagents under every policy, prove native resume, choose the
cheapest entitled model, inspect refusal variants and measure latency. No frozen
contract, root manifest, lockfile or other provider implementation was changed.

## Fix round 1

The orchestrator committed the initial implementation as `83225db` and reran fmt,
clippy and all 29 original tests successfully. Its real
`cargo run -p spike-agent-codex --example record -- simple` run selected
`gpt-5.6-luna` and produced the first event at **416 ms**. Session/turn lifecycle,
message deltas/completion, usage and quota events mapped correctly. These are
orchestrator-reported measurements from the pre-fix recorder, not measurements
repeated in the worker sandbox. The unsafe recording was discarded; no live
fixture or enforcement proof is claimed from that run. This supersedes the
original section's lack of any real latency result.

### Recorder privacy and configuration

The recorder now uses `recording::write` for both JSONL and golden output. It
obtains the hostname from the OS `hostname` command, and also checks macOS
`LocalHostName` through `scutil` when available. Failure to obtain the OS hostname
stops fixture writing. Nested JSON-RPC strings are decoded before redaction.
`installationId`/`installation_id` become a stable string placeholder; account
`authMode`/`planType` (including snake_case spellings) become null, including in
unknown/nested records. Remote-control hostname and environment id fields are
replaced. A whole-stream pass collects installation and remote-control identifiers
so copies in earlier diagnostic text are redacted too. Generic secret, home and
user sanitization still runs; both outputs are checked before writing and read
back for cleanliness assertions. Raw session inputs are not modified.

`codex app-server --help` exposes `-c` overrides but no `--ignore-user-config` flag.
The local ConfigReadParams schema supports `config/read` with `cwd` and
`includeLayers`; ConfigReadResponse permits additional effective config fields.
The [configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference)
documents `features.hooks=false`, `mcp_servers.<name>.enabled=false`, and per-plugin
MCP server enablement. Recording therefore uses this sequence:

1. Start the catalog/config-discovery process with `-c features.hooks=false`.
   It initializes and reads model/list plus effective config for the recorder's
   scratch cwd, without starting an agent thread or turn.
2. For the actual recording processes, including both resume processes, pass
   `-c features.hooks=false` and individual `enabled=false` overrides for the MCP
   servers discovered in `config.mcp_servers` and plugin MCP tables.
3. Keep native Codex home and authentication unchanged. No config file or auth store
   is rewritten, and normal `CodexAdapter::default()` / chat sessions have no extra
   overrides. A fake-child test checks the actual process arguments in both modes.

There is no demonstrated blanket MCP switch in app-server help. Empty-table
`mcp_servers={}`/`hooks={}` overrides are not assumed to erase inherited layers.
Effective config may omit plugin or managed sources, and config/read may fail.
In those cases the recorder uses the authorized redaction fallback; it does not
claim complete configuration isolation. MCP startup/status records retain their
method and status but replace names with `mcp-1`, `mcp-2`, etc. Hook start/completion
records share `hook-1` aliases for the same run. Names, commands, source paths,
error/output text and unknown diagnostic text fields receive stable placeholders;
lifecycle enums and thread/turn correlations remain. This fallback always runs,
including if a source emits metadata despite the disable overrides. Discovery
errors are not printed verbatim. Auth continuity and suppression of real user
sources still need the orchestrator's live verification; no app-server was launched
inside the worker sandbox during this fix round.

### Observed Unmapped records: M1 disposition

All eight records remain `Unmapped` now. These are proposed M1 interpretations,
not changes to the frozen contract or current normalizer:

| Native notification | M1 relevance / proposed mapping |
| --- | --- |
| `account/updated` | Ignorable in the normalized session lifecycle; shared account UI may consume it separately. No existing AgentEvent represents account plan/auth metadata. Actual auth failures should map from provider errors to `Error`, not be inferred from a plan change. |
| `hook/started` | Relevant execution visibility: `ToolCallStarted`, with a provider hook name and `Execute` for command hooks (`Mcp` for MCP handlers). It is an observation, not a pre-execution approval. |
| `hook/completed` | Relevant execution visibility: corresponding `ToolCallFinished` with succeeded/failed/denied status and safe output; preserve start/completion correlation. |
| `mcpServer/startupStatus/updated` | Relevant readiness failures: `Error { fatal: false }` on failed startup. Starting/ready updates are ignorable in agent-tool execution history; they describe a server, not a tool invocation. |
| `remoteControl/status/changed` | Ignorable for the local stdio adapter; belongs to a future remote-device connection UI, not agent turn events. Host and installation identifiers are fixture-private. |
| `thread/readState/changed` | Ignorable in execution history; read/unread state belongs to local client/storage state, with no matching AgentEvent. |
| `thread/status/changed` | Relevant reconciliation: `systemError` can become `Error { fatal: false }`; active/idle/waiting flags corroborate TurnStarted/TurnCompleted/ApprovalRequested/QuestionAsked. Do not synthesize duplicate lifecycle or request events from status alone. |
| `turn/moderationMetadata` | Relevant for diagnosing moderation/refusals, but the schema leaves metadata opaque. Keep raw metadata pending interpretation; only an explicit refusal should become `Error` alongside the failed TurnCompleted. Metadata alone is not a failure or RateLimited event. |

### Fix validation

Synthetic-stream tests exercise the production writer with all eight notifications,
current OS hostname, installation identifiers (including earlier text copies),
account fields, multiple MCP servers and repeated hook start/completion records.
They read JSONL/goldens back, check stable aliases and preserved Unmapped methods,
and verify the original session data was untouched. A separate corpus test fails
if any committed Codex fixture contains the current hostname. JSON escaping and
scalar line preservation are also covered. Process tests verify recorder overrides
reach the fake child while default sessions retain their original arguments.

Required fmt, clippy and test command results are recorded in the handoff's Fix
round 1 section. Live record/chat commands are deliberately deferred to the
orchestrator per the fix-round instruction; the existing hand-made scenarios stay
in place until safe live recordings replace them.


## Fix round 2 — accepted review findings

Base: `49e6b20` (fix round 1, committed by the orchestrator, 34 tests verified).
At the time of fix round 2 all fixtures were hand-made (since replaced by live recordings; see Fixtures). Offline routing and shape checks do
not establish provider enforcement. The original 29-test and round-1 34-test
counts above describe historical verification, not this revision.

| Finding | Change and evidence |
| --- | --- |
| 1 | Injected MCP servers request prompt approval. Elicitations route through gate Allow/Deny/Ask, emit approval reasons, use native action responses, and send cancel on interruption. Fake-server tests exercise all four paths. The investigation matrix below separates schema support from runtime coverage. |
| 3 | `record` checks the decoded, raw stream before sanitization or fixture writing. It prints PASS/FAIL for target gate denial, matching denied-call completion, sentinel absence and absence of native successful execution. A failure says `read bypassed the gate` (or execute), returns failure and refuses the recording. Native success cannot be hidden by the mapper's Denied status. The sentinel is never passed to the sanitizer. Missing evidence fails; it is not itself proof of execution. |
| 2 | Whole-stream identifiers plus discovered config MCP/plugin names and hook names/source paths become sanitizer secrets, including for earlier unknown notifications. Stderr becomes a stable placeholder in committed recordings. Synthetic tests put those identifiers in both stderr and an unknown method. Recording metadata uses the same privacy pass; scratch roots become `/scratch`. |
| 4 | Retry notifications do not emit Error/RateLimited. Terminal duplicates are deduplicated per turn; RateLimited uses cached quota reset metadata. The quota-usage hand-made fixture includes retry, terminal notification and the same completion error; its golden contains one refusal. |
| 5 | Ordered stdin submission records remove the stdout/write-ack dependency. A 2 MB input versus blocked provider stdout regression passes. A saturated child that exits unexpectedly is polled/reaped without a Shutdown command, with lifecycle cleanup. |
| 6 | Added actual STARTUP_TIMEOUT/reap test, end-to-end queue overflow/wire count test, and saturation with an open tool plus pending Ask; cancellation and SessionEnded survive reserved slots. |
| 7 | Corrected live timing to the orchestrator's 416 ms, runtime acceptance, current status and historical commit references. Machine-specific paths removed from handoff. Current DoD results are in the handoff. |
| 8 | Live footer identifies live data; hand-made footer remains mapping-only. Protocol version comes from the initialize response, model/parameters from the sanitized launch transcript, and actual argv from the invocation (private values redacted). If initialization never completes the version is explicitly unknown. A corpus-wide golden test covers auxiliary `resume-origin` and `*-startup-failure` files whenever recorded. |
| 9 | If invalid-model startup establishes a session, recorder shuts it down and retries with the missing zero UUID resume id. If that also establishes a session it fails. Before saving, it asserts fatal Error then SessionEnded and no SessionStarted. Offline tests reject established-session traces. Verify live on this CLI. |
| 10 | FullAccess has `approvalPolicy=never`; command/file user deny rules have no callback unless Codex independently sends a request. Explicit matrix row below. |
| 11 | Approval reason preserved; Read/Search classification documented as advisory; home/user tests use environment values; gate decision timeout is a separate 30 seconds, independent of shutdown grace; scratch paths normalized. Process-group limitation documented below. |

### Enforcement investigation and live verification checklist

The installed generated schemas are evidence for wire **shapes**, not guarantees
about trigger coverage. `tests/support/check_schema.py` validates granular policy,
command hook configuration, elicitation params and accept/decline/cancel responses.
For comparison, run `cargo run -p spike-agent-codex --example record -- gate-deny --granular`
and the same execute scenario. The selected policy is captured in fixture metadata.
The default ReadOnly/Ask setting remains `untrusted`; schema presence alone does
not justify silently replacing it with granular policy.

| Mechanism / policy | Intercepted before execution? | Offline finding | Live evidence required |
| --- | --- | --- | --- |
| `preToolUse` / `PreToolUse` | untested, verify live | HookRunSummary lists preToolUse; ManagedHooksRequirements exposes PreToolUse matchers with synchronous command/MCP handlers. `hook/started` is a notification, not a client gate callback. | Build and verify a synchronous **preToolUse hook bridge** to ToolGate, including reads, safe commands, patches, MCP and subagents. Check deny, panic, timeout and unavailable bridge before side effects. Recorder normally disables user hooks; use a deliberately injected test bridge, never rewrite native config. |
| `permissionRequest` / `PermissionRequest` | untested, verify live | Separate configured hook event; notification schema alone gives no all-tool coverage guarantee. | Verify ordering against JSON-RPC approval, native user hooks, allow rules and FullAccess. Ensure hooks cannot override Atrium denial. |
| `approvalPolicy.granular` | untested, verify live | Object supports booleans `mcp_elicitations`, `rules`, `sandbox_approval`, `request_permissions`, `skill_approval`. All-true candidate validates offline and reaches the fake server through the opt-in `--granular` recorder flag; FullAccess still uses never. These flags permit request categories; schema does not say “ask for every tool”. | Compare all-true candidate with untrusted for safe cat/search, writes, execute and MCP. Test each disabled category's fail-closed behavior. Do not count shape validation as coverage. |
| MCP `default_tools_approval_mode="prompt"` | partial, verify live | Added to every injected server's process args, including FullAccess. Native elicitation is gated; URL-mode consent can accept/decline/cancel. Form content remains null; arbitrary form data is not invented. | Verify every injected tool prompts, per-tool overrides, native user servers and required-form behavior. A form requiring structured values needs M1 UI/input mapping; consent alone does not supply those values. |
| User `.rules` allow-prefixes | partial, verify live | Native config and rules remain untouched; allow-prefixes may bypass provider requests. No schema-backed switch demonstrated that forces every allowed command through a client callback. | Test allowed safe and non-read command prefixes under untrusted and granular, with and without hook bridge. Record each bypass explicitly. |
| FullAccess (`never`) | no command/file interception expected; independent requests remain gated, verify live | Required danger-full-access/never policy removes ordinary provider approval callbacks; user deny rules cannot apply to calls that produce no callback. Injected MCP prompt requests, if emitted, still pass the gate. | Verify command/file bypass and independent MCP/hook behavior; never claim AllToolCalls. |

All original tool-class rows remain **verify live** for execution coverage. Safe
reads may run without any request under untrusted. `gate-deny` then fails loudly;
record that observed bypass in the read row rather than committing a misleading
proof. A successful denial proves that call only. If writes/non-read commands
bypass the client under **every** tested setting, stop and escalate per the brief.

Read/Search kinds come from Codex `commandActions`, not trusted static command
analysis. Gate policy must inspect the original command/input and must not grant
access solely because Codex labels a call Read. Elicitation requests can lack
itemId/turnId, so correlation to an MCP ToolCall is not fabricated.

The process owner kills/reaps its direct child and joins its own tasks. It does
**not** create/kill a process group. Provider-launched descendants can outlive the
child; M1 needs platform-specific process-tree supervision and descendant tests.
This limitation does not weaken fail-closed handling of received gate requests,
but prevents claiming complete provider process-tree cleanup.

## Live verification results (codex 0.163.0, gpt-5.6-luna)

Recorded by the orchestrator; recorder keeps user MCP on (disabling it makes
`app-server` exit with "invalid transport" on 0.163, so names are redacted instead).
All 13 scenarios are live and leak-clean.

| Scenario | Live result |
| --- | --- |
| simple, multi-turn, resume, startup-error | **live verified**, including native resume in a second process and the startup fallback |
| tool-read | **live verified**; read command asked, Allow ran it |
| gate-deny | **live verified**: denied, no secret, no native execution |
| gate-deny-execute | **live verified**: denied, no secret, no native execution |
| approval-allow / approval-deny | **live verified** (Ask, resolved Approved / Denied) |
| interrupt | **live verified**: tool Cancelled, turn Interrupted; raced the gate (see above) |
| question | recorded live, **but Codex did not ask the question** (no QuestionAsked); the question mapping is hand-made verified only |
| patch | recorded live but ended in a usage-limit refusal; file-change approval **not live verified**; RateLimited mapping is live verified |
| quota-usage | **live verified**: Usage and QuotaUpdated, no spurious RateLimited |

Runtime accepted `approvalPolicy: "untrusted"` with ReadOnly (the schema/runtime
discrepancy is not an issue on 0.163). Still unverified live: file-change requests,
MCP, granular, hooks, allow-prefixes, FullAccess, subagents, the `Ask` policy's workspace-write behavior beyond thread parameters, and refusal variants other
than `usageLimitExceeded`.
