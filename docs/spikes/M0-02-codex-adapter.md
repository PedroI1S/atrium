# M0-02: Codex app-server adapter

Status: offline implementation complete; real provider verification blocked by the worker sandbox.
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
task acknowledges actual writes; stdout processing waits for outstanding write
acknowledgements so replay observes requests before their responses. Transport
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
says the selectable policy is retired. Runtime acceptance and exact enforcement
could not be tested. There is no automatic downgrade to `on-request`: rejection
fails startup. Resolve this schema/runtime discrepancy before M1.

`interception()` always returns **ProviderGated**. The schema does not establish
any setting that sends every tool call through the client. An approval transcript
for a read does not establish that every read asks; the same caveat applies to
safe commands and writes inside provider-granted roots.

| Tool class | Intercepted before execution? | How | Evidence and limitation |
| --- | --- | --- | --- |
| Read | partial | command approval request, when issued | `tool-read`, `gate-deny` goldens; hand-made, unchecked reads remain possible. |
| Edit/write | partial | `item/fileChange/requestApproval`, when issued | `patch` golden; no proof that every write asks. |
| Delete/move | partial | command/file approval, when issued | shared routing only; no live delete/move evidence. |
| Execute | partial | `item/commandExecution/requestApproval` | `gate-deny-execute`, both approval goldens, fake-server panic denial; safe-command bypass not ruled out. |
| Search | partial | command approval, when issued | action classification; universal interception unproven. |
| Fetch/web | no | item notifications are observations | web items map to tools; no universal authorization request established. |
| MCP | no universal hook | tool notifications; unsupported elicitations fail closed | native MCP items map to `ToolKind::Mcp`; elicitation does not establish coverage of all calls. |
| Subagents | no | collaboration notifications | observed as tools; universal authorization callback unproven. |
| Extra permissions/dynamic tools/auth refresh | unsupported | server requests return `-32601` | no execution or permission grant is authorized. |

Fixtures prove mapping only, not provider enforcement or real absence of secret
leakage. No live evidence established the brief's stop-condition finding: writes
or non-read commands bypassing the client under every setting. The orchestrator
must explicitly test that condition during verification.

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
Additional scenarios cover execution denial, patches and quota/usage/reasoning/
plan/unknown records. **Every committed fixture is hand-made**, with neighboring
provenance notes. Resume's fixture represents the second process resuming a
synthetic id. The live harness separately records `resume-origin`, then resumes
that real native id in a second process. Experimental question transport is
implemented; the selected live model/mode's willingness to invoke it is unverified.

The CLI labels app-server experimental. Local schemas require item timestamps and
session/project fields on threads. `schema/` and raw `live/` files are ignored.

```sh
codex app-server generate-json-schema --out spikes/agent-codex/schema
python3 spikes/agent-codex/tests/support/check_schema.py
```

The structural check reports **179 records passed, 0 errors** for the committed
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
refusal or HTTP 429 does; other error variants remain errors/raw records pending
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

**Time to first real event: unavailable.** The direct app-server probe printed
`Operation not permitted` and exited before initialization. No real model list,
thread, inference, denial, resume or quota update was obtained. Codex home was not
changed and no sandbox workaround was attempted.

```sh
cargo run -p spike-agent-codex --example record -- simple
# exit 1: app-server closed before model/list; check sandbox permissions
cargo run -p spike-agent-codex --example chat
# exit 1: Handshake("stdout closed before thread creation")
```

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
`cargo test -p spike-agent-codex` pass. The latter runs 20 fixture/semantic tests
and 9 subprocess/policy tests; see the handoff for command results.

Carry the pure mapper, request routing, sanitizer and process-owner tests into M1
after independent review and real recordings. Keep ProviderGated visible to daemon
policy. Full Atrium enforcement requires a real pre-execution mechanism for every
provider tool class.

The orchestrator owns live verification: resolve the retired-untrusted discrepancy,
confirm actual runtime settings, record read/execution denials, exercise safe
commands/writes/MCP/subagents under every policy, prove native resume, choose the
cheapest entitled model, inspect refusal variants and measure latency. No frozen
contract, root manifest, lockfile or other provider implementation was changed.
