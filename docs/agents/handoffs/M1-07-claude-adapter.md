# Handoff: M1-07-claude-adapter

Model: Sonnet 5.5 (two workers, one per shift) · Status: **done**

## Done
- `crates/atrium-agents/src/claude/{mod,adapter,normalizer,settings,tools}.rs`: `ClaudeAdapter` (probe + start), `ClaudeNormalizer`, config inspection, tool subjects. Contract rev 3: descriptor via `evaluate`, `AdapterInput::Enforcement`/`Redeem`/`QuestionTicket`, tickets, redeem before the `can_use_tool` answer, `spawn_group` + `on_spawn` + `Session::process`, `terminate` on shutdown, bounded terminal tail, `MessageBody` + user messages, `ContextUpdated`, `Error::details`, limits (record, args, output, unmapped), an interrupt that hangs ends the session (no respawn), unparseable/unreadable settings = `inspected: false`.
- Tests:
  - `tests/claude_normalizer.rs` (60, new): the spike's hand-made normalizer tests ported to the rev-3 shapes (tickets for approvals and questions, `QuestionTicket`, `MessageBody` Inline/Streamed with chars/deltas/sha256, `Error::details`, redeem, interrupt now ends the session), plus new ones: the oversize argument cap (call shown and failed, never offered for approval, session continues), the output cap (cut on a char boundary, `truncated_bytes`, at-the-cap kept whole, error text capped), the record cap (established and pre-session, once), `Deadline` inputs (`record_too_large`, `interrupt`, unknown ones ignored), pre-session failures with details and no `Unmapped` or `SessionStarted`, diagnostics bounds (50 lines, 2 KiB, control characters), `Unmapped.raw` cut, long messages split into deltas, `ContextUpdated`, recorded descriptor / `EnforcementChanged`, tool subjects for every tool class.
  - `tests/claude_adapter.rs` (26, offline fake CLI `examples/claude-fake.rs`): saturation with process-group check, stale approval, redeem, oversize record, descriptor/plugin tests, and (new) `oversize_tool_arguments_are_denied_without_asking_the_gate` (fake mode `hook-bigargs`: the hook is answered `deny`, the gate is never called, the call finishes `failed`).
  - `tests/claude_golden.rs` (20), settings unit tests.
- Live fixtures, all recorded with `claude` 2.1.308, haiku, sanitized (`is_clean` asserted by the recorder and the golden test): simple, tool-read, gate-deny, gate-deny-{exec,edit,delete,fetch,fullaccess,mcp,subagent}, mcp-allow, approval-allow, approval-deny, question, interrupt, interrupt-resume, multi-turn, resume, startup-error, plus plugin-hook and stale-approval. `rate-limited` is the M0 live capture migrated (Enforcement line added), not re-recorded.
- Recorder `examples/claude-record.rs` (`cargo run -p atrium-agents --example claude-record -- <name>|all`); `examples/claude-probe.rs` (new) prints the sanitized descriptor for this machine.

## Changes this shift in the adapter's own paths
- No change in `src/claude/**`: every normalizer/adapter test written this shift passed against the code as it was.
- `tests/claude_adapter.rs`: the `alive()` helper ran `kill -0 -<pgid>`. With procps-ng 4.0.4 (this machine) `-<pgid>` is parsed as a signal, so the check was inverted (a dead group looked alive) and four tests failed here (`an_oversize_record_ends_the_session`, `shutdown_kills_the_whole_process_group`, `an_interrupt_the_child_ignores_ends_the_session`, `saturation_...`). Fixed with `kill -0 -- <pid>`. A test helper bug, not an adapter bug: the process groups really were gone.
- `examples/claude-fake.rs`: new mode `hook-bigargs`.

## DoD results (run in this worktree at the end of the shift)
- `cargo fmt --all --check`: exit 0, no output.
- `cargo clippy --workspace --all-targets -- -D warnings`: in the container it stops on the app crate (`glib-2.0` missing: WebKitGTK is not installed there). Run on the host (`CARGO_TARGET_DIR` outside the worktree): `Finished dev profile`, exit 0, no warnings.
- `cargo test -p atrium-agents`:
  - unit tests: 15 passed
  - `claude_adapter`: 26 passed
  - `claude_golden`: 20 passed
  - `claude_normalizer`: 60 passed
  - `consistency`: 11 passed, 1 ignored (`consistency_all_fixtures`, ignored for Codex fixtures by M1-12)
  - no failures. `consistency_all_fixtures` with `--include-ignored` fails on 30 files, all `fixtures/agents/codex/**` (old shapes, M1-12's); none under `fixtures/agents/claude/`.
- Saturation (receiver never read): `saturation_unread_events_still_obey_interrupt_and_shutdown` passes alone (1.56 s): `Interrupt` and `Shutdown` obeyed within `SHUTDOWN_GRACE`, process group gone, `SessionEnded` delivered after the drain.
- New fixtures `plugin-hook` and `stale-approval`: recorded live earlier, golden tests pass.
- Live re-recording was not repeated this shift (fixtures are recorded; no test proved one wrong).

## Enforcement matrix (M0 contract §3.1), CLI 2.1.308
Mechanism for every row: the `PreToolUse` hook callback registered through `initialize` (`hook_callback` control request, answered allow/deny/ask by the gate, fail closed on any gate error or timeout), with `--permission-prompt-tool stdio` (`can_use_tool`) as the second line for anything the hook left to the CLI's own permission flow. Fail-closed behaviour: `gate_failures_deny`, `can_use_tool_follows_the_gate_and_fails_closed`, `malformed_and_foreign_hook_requests_are_denied`.

| Tool class | Intercepted before execution? | How | Evidence |
|---|---|---|---|
| Read | yes | PreToolUse hook (registered with `matcher: null`, so every tool) | `gate-deny` (Read denied; contents never reach an event) |
| Edit / Write | yes | PreToolUse hook | `gate-deny-edit` (Write denied, file not created) |
| Delete / Move | yes (as Bash) | PreToolUse hook; Claude Code has no delete/move tool, it runs `rm`/`mv` through Bash | `gate-deny-delete` (file survives) |
| Search (Grep, Glob) | yes (thin evidence) | same hook; no live fixture made Claude call Grep or Glob. The only Search-kind call seen live is ToolSearch, which passed the hook in `gate-deny` and `mcp-allow` | `gate-deny`; subjects for Grep/Glob in `every_check_carries_the_extracted_subject` |
| Execute (Bash) | yes | PreToolUse hook; `Ask` shown through `can_use_tool` and redeemed before the answer | `gate-deny-exec`, `approval-allow`, `approval-deny`, `stale-approval`, `an_allowed_approval_is_redeemed_before_the_effect` |
| Execute under `FullAccess` | yes | same (`--permission-mode bypassPermissions` does not skip hooks) | `gate-deny-fullaccess` |
| Fetch / web (WebFetch; WebSearch is the same kind but not exercised live) | yes | PreToolUse hook | `gate-deny-fetch` (WebFetch denied before any request) |
| MCP | yes | PreToolUse hook (`mcp__<server>__<tool>`) | `gate-deny-mcp`, control `mcp-allow` |
| Subagents | yes, including the subagent's own tool calls | PreToolUse hook (the launch passes, the subagent's Read is denied) | `gate-deny-subagent` |
| Provider-specific (ToolSearch, AskUserQuestion, TodoWrite) | yes (weakest evidence) | ToolSearch: PreToolUse hook (seen in `mcp-allow`, classified Search); AskUserQuestion: hook answered `allow`, then the question goes through `gate.question_ticket` | `mcp-allow`, `question`, `questions_use_the_gates_question_ticket` |
| Settings hooks / plugin hooks / command-running keys | not controlled | run outside the gate; the session is `Partial` (`ExternalHooks`), plugin directories are `inspected: false` | `plugin-hook`, `external_hooks_make_the_session_partial_and_say_why`, `a_plugin_with_a_hook_is_partial` |

Coverage is `Intercepted` for all ten classes in the descriptor, but the descriptor is `Full` only when the CLI version is in `TESTED_VERSIONS` (`["2.1.308"]`) and every configuration source is inspected (`the_descriptor_is_full_only_when_everything_is_verified`).

## Descriptor printed for this machine
`cargo run -p atrium-agents --example claude-probe` (run on the host, with `~/.local/bin` on `PATH`, no `CLAUDE_CONFIG_DIR`), project dir = the worktree, sanitized by `Sanitizer` (home as `~`, `is_clean` asserted). The coverage block is identical for every machine and elided here after the first two entries; the full output has all ten classes, each `intercepted`.

```json
{
  "level": "partial",
  "reasons": [
    { "code": "untested_version", "message": "CLI version 2.1.312 hasn't been tested with Atrium" },
    { "code": "uninspected_config", "message": "Plugin not inspected: ~/.claude/plugins/cache/claude-plugins-official/clangd-lsp/1.0.0" }
  ],
  "provider": "claude",
  "cli_version": "2.1.312",
  "version_tested": false,
  "coverage": [
    { "class": "read", "coverage": "intercepted", "evidence": "fixtures/agents/claude/gate-deny" },
    { "class": "edit", "coverage": "intercepted", "evidence": "fixtures/agents/claude/gate-deny-edit" }
  ],
  "config_sources": [
    { "kind": "user_settings", "path": "~/.claude/settings.json", "sha256": "09dc66a5766899ba0b85c518e4a118c1337e14b34d23ff6db2a9267a1fc3fb5c", "inspected": true },
    { "kind": "plugin", "path": "~/.claude/plugins/cache/claude-plugins-official/clangd-lsp/1.0.0", "sha256": null, "inspected": false }
  ]
}
```

Why Partial here: the host `claude` auto-updated to 2.1.312 after the fixtures were recorded on 2.1.308, and an installed plugin's directory is never inspected. Both reasons are the descriptor working as designed. Adding 2.1.312 to `TESTED_VERSIONS` needs the fixtures re-recorded on it first; not done (out of scope, no live sessions this shift).

## Gotchas
- Do not set `CLAUDE_CONFIG_DIR` when recording: auth lives there ("Not logged in").
- Account was at about 76% of the 5h window during the earlier recording.
- AskUserQuestion hook is answered `allow` without `gate.check`; the gate is consulted through `question_ticket` only.
- Exit detection = pipe EOF or a 500 ms `identify` poll (never reap before `terminate`).
- AskUserQuestion/ProviderSpecific evidence is thin; spike gate files in `spikes/` untouched.
- On a fatal error in an established session, `Error` is emitted first, then the closing events in contract order (messages, calls, requests, turn), then `SessionEnded`. The contract fixes the closing order but not where the `Error` goes; `claude_normalizer::exit_mid_turn_closes_everything_in_order` pins the current order. Reviewer: confirm that is what you want.
- Host and container share `target/`; build the host side with another `CARGO_TARGET_DIR` (as done for clippy and the probe) to avoid rebuild thrash.

## Change requests
None filed.

## Fix round 1 (cross-provider review: rework)
Every finding was reproduced with a test written first (the failing output was seen before the fix); the tests are in `tests/claude_review.rs` (normalizer, subjects), `tests/claude_adapter.rs` (`f1_*`, `f3_*`, `f5_*`, `f7_*`, `a_start_dropped_*`, with new fake-CLI modes in `examples/claude-fake.rs`) and `src/claude/settings.rs` (unit tests). Several of the review's line numbers and details do not match this code (it describes `MultiEdit` edit lists, a `WebSearch` arm, a `Task` subject, `inspect_with`, `MAX_EVENT_BYTES` as 1 MiB and a `claude_normalizer` test named `tools_cover_every_documented_tool_and_unknown_fallback`, none of which exist here); the sequences were rebuilt from the descriptions.

| # | Reproduced | Test | Fix |
|---|---|---|---|
| 1 | yes: an `Allow` written after `control_cancel_request` and after `result`, for `can_use_tool` and for a hook call; an `Ask` after the turn became the next turn's approval | `f1_a_gate_answer_after_control_cancel_request_is_not_sent`, `f1_a_gate_answer_after_the_turn_ended_is_not_sent`, `f1_a_hook_answer_after_the_turn_ended_is_not_sent` (Allow and Ask), `f1_an_ask_after_the_turn_ended_is_not_reused_by_the_next_turn` | `adapter.rs`: hook calls are tracked in `live` like `can_use_tool` requests; `on_internal` feeds and answers a gate decision only if its request is still live (otherwise nothing is sent and no ticket is kept); `inflight` keeps each gate task's abort handle: `forget_native` aborts that task, `clear_requests` (turn end, session end) aborts all |
| 2 | yes: an absolute, `~` or `..` Glob pattern named nothing outside the base; `path`/`pattern` of the wrong type became `"."` | `f2_a_glob_pattern_that_leaves_the_base_names_its_target`, `f2_a_glob_pattern_that_cannot_be_bounded_is_unknown`, `f2_missing_empty_or_non_string_fields_are_unknown`, `f2_a_grep_glob_that_leaves_the_path_names_its_target_too` | `tools.rs`: `Glob`/`Grep` share `search_subject`: the base stays in `paths` and, when the glob leaves it, its fixed prefix up to the first wildcard (its directory if there is a wildcard in the last component), resolved against the base, is added; a glob that can't be bounded (a `..` or `{/`, `,/`, `{~`, `,~` after a wildcard, or a backslash) is `Unknown`; empty or non-string `path`, `pattern` or `glob` is `Unknown` |
| 3 | yes: events carried a 4 MiB input (case A and B) | normalizer: `f3_an_oversize_can_use_tool_before_the_tool_use_is_never_offered`, `..._after_...`; adapter: `f3_an_oversize_can_use_tool_is_denied_without_asking_the_gate` | `adapter.rs`: `on_can_use` takes `big` and denies at once, gate not asked (as `on_hook`); `normalizer.rs`: an oversize `can_use_tool` registers nothing, shows the call once as `failed` with empty input (`oversize_call`, shared with `tool_use`) and never an approval |
| 4 | yes, for (a) approval, question, (b) deltas and usage, (c) a command `Error`, (d) records with no `init` | `f4_*` (5 tests) | `normalizer.rs`: `on_input` holds every event of an input in `held` while the session is not established (only `Exit` may show a pre-session `Error` + `SessionEnded`); `establish` runs before approvals, questions, deltas and usage, opens a turn when held events need one, and releases them after `SessionStarted` (+ `TurnStarted` + the user message); `cancel_pending` shows nothing for a session that never started |
| 5 | yes (6,000 records after EOF used the 4,096 tail; only `SessionEnded` was left) | `f5_thousands_of_records_during_shutdown_do_not_drop_the_closing_events` | `adapter.rs`: `finish` sets `finishing` after `stop_process`, so `closing` counts only the closing tail |
| 6 | yes (`fileSuggestion` clean; flat/odd `hooks` shapes inspected-and-clean; `/home/anabel` shown as `~bel`) | settings unit tests: `file_suggestion_runs_a_command_so_it_is_external_hooks`, `hooks_in_a_shape_we_do_not_parse_are_never_inspected_and_clean`, `a_sibling_of_the_home_directory_is_not_shown_as_inside_it` | `settings.rs`: `fileSuggestion` added to `COMMAND_KEYS`; `command_findings` also returns whether every `hooks` entry had the parsed shape, otherwise the file is `inspected: false` (an empty `hooks` is clean); `display_path` uses `Path::strip_prefix` |
| 7 | yes (a hook ticket reused for another input; the second of two anonymous calls overwrote the first's ticket, third gate question) | normalizer: `f7_a_hook_ticket_is_not_reused_when_can_use_tool_changes_the_input`, `f7_two_anonymous_calls_keep_their_own_tickets_in_order`; adapter: `f7_a_hook_ticket_is_not_reused_for_a_changed_can_use_input`, `f7_two_identical_anonymous_hook_calls_keep_their_own_tickets` | `tools.rs`: `gate_key` = call id (or `anon`) + SHA-256 of tool name and input; `hook_asked` (adapter) and `asked` (normalizer) hold a FIFO queue per key |
| 8 | yes (a second `MessageCompleted` for the same id) | `f8_a_late_delta_after_the_assistant_record_does_not_complete_the_message_twice` | `normalizer.rs`: `done_msgs`; a delta for a completed id is dropped |
| 9 | yes for the descriptor; the `plugi` is **not** a typo | `f9_a_descriptor_that_changes_before_the_session_starts_is_reported_as_a_change`; `a_long_plugin_path_is_cut_to_80_characters_in_the_reason` | `normalizer.rs`: `SessionStarted` carries the first descriptor recorded (what `start()` returned), a later one follows as `EnforcementChanged`. Fixture left as recorded: the reason says `plugin {clip(id, 80)} ships hooks`, and the path is exactly 80 characters there (`plugin-hook.events.json` has it at 80 characters, ending `.../plugin-hook/plugi`); the source entry keeps the whole path |
| Suspected 1 | yes: a `start()` future dropped during the handshake left the group (the leader died by `kill_on_drop`, its `sleep` child stayed) | `a_start_dropped_during_the_handshake_kills_the_process_group` | `adapter.rs`: `Proc` has a `Drop` that, if the process is still its own, spawns `supervise::terminate` (kill the group and reap); `stop_process` takes the child out first, so a normal stop does not run it twice. (A first attempt, `kill_recorded` in a guard, killed the group but could not reap the leader, so the group stayed non-empty as a zombie; and my first test dropped a `Pin<&mut _>`, not the future: both found and corrected.) |

Not taken from the review: the second "Suspected" item (symlinked settings files, TOCTOU between `refresh_enforcement` and the CLI's reads) was not in the accepted list and is untouched.

Item 8 of the round (tool subjects): `WebSearch`, `Task`, `Agent`, `Skill`, `BashOutput`, `KillShell`, `KillBash`, `ExitPlanMode`, `TodoWrite`, `ToolSearch` and unknown tools all get `ToolSubject::Unknown` (kinds: WebSearch `Fetch`, Bash-family `Execute`, ToolSearch `Search`, the rest `Other`); pinned by `item8_subjects_of_the_tools_without_a_dedicated_one_are_pinned`. Nothing hides what a tool acts on: the policy asks about every `Unknown`. `MultiEdit` (one `file_path`) and `PowerShell` keep theirs.

`fileSuggestion`: confirmed in the installed `claude` 2.1.312 binary (read with `strings`; no session started): its settings schema defines `fileSuggestion: {type: "command", command: string}` "Custom file suggestion configuration for @ mentions", and the code that runs it logs "FileSuggestion command". The same binary has a list of helper keys that also holds `gcpAuthRefresh`, `processWrapper`, `policyHelpers`, `proxyAuthHelper` and `subagentStatusLine`; I could not confirm from the strings that each runs a command, so they are **not** added (a follow-up for the orchestrator: adding them only makes more sessions Partial).

Goldens: no golden file changed (`UPDATE_GOLDEN` was not needed); `claude_golden` (20) and the replay test pass on the unchanged `*.events.json`. The only recorded-fixture edit would have been `plugin-hook` (finding 9) and it was left alone.

### DoD after fix round 1
- `cargo fmt --all --check`: exit 0.
- `cargo clippy --workspace --all-targets -- -D warnings`: host, `CARGO_TARGET_DIR` outside the worktree: exit 0, no warnings (the container still lacks WebKitGTK).
- `cargo test -p atrium-agents`: unit 19, `claude_adapter` 35 (three consecutive runs, no flake), `claude_golden` 20, `claude_normalizer` 60, `claude_review` 16, `consistency` 11 passed + 1 ignored. No failures. `consistency_all_fixtures --include-ignored`: no violation under `fixtures/agents/claude/` (the Codex files are M1-12's).

## Fix round 2 (cross-provider review of round 1: rework)
Each item: failing test first, then the fix. Tests are in `tests/claude_policy_e2e.rs` (new, through the real `atrium_core` policy; `atrium-core` added as a dev-dependency, `# M1-07`), `tests/claude_review.rs` (`r2_*`), `tests/claude_adapter.rs`, and unit tests.

| # | Reproduced | Test | Fix |
|---|---|---|---|
| 1 `Glob`/`Grep` escapes (blocker) | yes, end to end: `Preset::Home` allowed `{a,}/home/ana/.ssh/id_ed25519` | `glob_escapes_are_never_allowed_and_credentials_are_hard_denied` (Home and FullAccess, `SessionGate` + `DefaultPolicy`, home `<tmp>/home/ana`, cwd `<tmp>/home/ana/work`; the review's patterns and variants, as `Glob` pattern and as `Grep` glob), `patterns_we_cannot_bound_are_unknown_and_never_allowed`, `ordinary_patterns_keep_their_plain_subject_and_are_allowed_at_home` (`**/*.rs`, `src/**/mod.rs`, `*.{ts,tsx}`, `src/{a,b}/mod.rs`, `*.[ch]`), `f2_braces_that_hide_a_root_are_expanded_and_the_target_named` | `tools.rs`: braces are expanded (nested, at most 32 alternatives, else `Unknown`) and each expansion is checked; a `[...]` class that is negated, unclosed, or can be `.`, `/` or `~` (or a range spanning them, or `[:class:]`) is `Unknown`, and so is a backslash; a glob that leaves the base puts the directory it points into **and the whole pattern joined to the base** in `paths` (the policy's own glob matching runs on a `paths` entry with a wildcard). Round-1 tests that encoded the old subject shape were updated (the braces cases are now expanded, not `Unknown`) |
| 3 `gate_key` canonical | yes (keys in another order missed the hook ticket) | `r2_a_ticket_survives_the_input_being_reserialized_with_other_key_order` (nested objects, arrays keep their order), `r2_our_canonical_text_is_the_policys`, unit test `canonical_json_sorts_keys_and_keeps_array_order` | `tools.rs`: `gate_key` hashes `canonical_json` (keys sorted recursively, same text as `atrium_core::policy::canonical_input`'s body). `atrium-core` depends on this crate, so its function can't be called from the library; a test compares the two |
| 2 held events | yes (cases A, B, C of the review) | `r2_records_before_any_init_establish_the_session_from_the_record`, `r2_an_approval_or_question_before_init_is_shown_at_once` (A), `r2_a_result_before_init_resolves_the_request_and_keeps_turns_in_order` (B), `r2_what_waits_for_the_session_is_bounded` (C), `r2_a_failed_start_with_only_noise_is_still_two_events` (the pre-session failure stays `Error` + `SessionEnded`) | `normalizer.rs`: `establish_with`: a record that proves the model answered (`message_start`, delta, usage, `assistant`, non-error `result`, approval, question, rejection) with no `init` before it establishes the session at once, from a fallback `InitInfo` (the record's model if it has one, else none; native id empty, not invented); `hold` keeps at most `MAX_DIAGNOSTIC_LINES` (newest) of what still waits (pre-session `Unmapped`, quota, command errors). The round-1 test `f4_records_before_any_init_produce_nothing...` encoded the old behaviour and was replaced by these |
| 4 inline `--settings` plugins | yes (no plugin source, no reason, `Full`) | `plugins_enabled_by_an_inline_settings_argument_are_listed` (settings.rs; checks the source, the reason and the evaluated level `Partial`) | `settings.rs`: `enabled` is declared before the argument loop and the inline settings' `enabledPlugins` are added to it |
| 5 late `assistant` after `close_messages` | yes | `r2_a_late_assistant_record_does_not_complete_a_closed_message_again` | `normalizer.rs`: `complete_message` returns at once for an id in `done_msgs` |
| 6 `Proc::drop` without a runtime | yes (the leader died, its `sleep` child stayed) | `a_start_dropped_outside_a_runtime_kills_the_process_group` (dropped on a plain `std::thread`) | `adapter.rs`: `Proc::drop` signals the group through `supervise::kill_recorded` (atrium-host's recorded-identity kill) before anything async: inline when there is no runtime (then reaps the leader with `try_wait`; the drop waits up to 2 s for the group to drain, which `kill_recorded` does), on a short-lived thread when there is one, plus the async `terminate` task for the reap. Doc comment says why. Round 1's runtime case test still passes |
| 7 `live` (minor) | yes, for `Redeem` and `Question` (a refused redeem for a withdrawn request showed `ToolCallFinished { Denied }`; a ticket for a withdrawn question showed `QuestionAsked`); yes for the shared id (the answer to the approval was dropped after a hook call with the same id was answered) | unit tests in `adapter.rs`: `a_redeem_for_a_request_that_is_not_live_is_not_fed`, `a_question_ticket_for_a_request_that_is_not_live_is_not_fed`, `a_hook_call_does_not_keep_a_can_use_request_of_the_same_id_live`; integration `r2_a_hook_call_and_a_can_use_request_with_one_id_are_separate` (new fake mode `id-clash`) | `adapter.rs`: `live` and `inflight` are keyed by `Req::Hook(id)` / `Req::CanUse(id)`; `on_internal` checks `live` before feeding a `Redeem` or a `QuestionTicket` (as it already did for `Gate`), and a gate failure is still recorded for a withdrawn request. `control_cancel_request` names no kind, so it withdraws both. The queued-`Redeem`-before-cancel race itself can't be driven through the fake CLI (the loop is `biased` towards internal messages and the cancel aborts the task), so it is tested by calling `on_internal` on an actor with no process |
| 8 helper keys (orchestrator decision) | yes: a file with `gcpAuthRefresh` had no reason and stayed Full | `the_other_helper_keys_are_command_keys_too` (settings.rs; each of the five: one `ExternalHooks` reason naming the key, the file still `inspected`, evaluated level `Partial`) | `settings.rs`: `gcpAuthRefresh`, `processWrapper`, `policyHelpers`, `proxyAuthHelper`, `subagentStatusLine` added to `COMMAND_KEYS` (fail closed). **Only `fileSuggestion` was confirmed to run a command** (round 1, strings of the 2.1.312 binary); the other five are in the same helper-key list there but I (and the reviewer) could not confirm that each runs one, so a file that sets one makes the session Partial even if the key is inert. Doc comments say so |

Left for the policy side (not mine): finding 1's other half, the policy engine in `atrium-core`, is P1 in `docs/agents/m1-must-fix.md`. No contract change and no change request in this round. No golden file changed (the fixtures were not re-recorded).

### DoD after fix round 2
- `cargo fmt --all --check`: exit 0.
- `cargo clippy --workspace --all-targets -- -D warnings`: host, `CARGO_TARGET_DIR` outside the worktree: exit 0, no warnings.
- `cargo test -p atrium-agents --no-fail-fast` (after `cargo build -p atrium-agents --examples`, which `--test` runs do not do; three consecutive runs, no flake): unit 25, `claude_adapter` 37, `claude_golden` 20, `claude_normalizer` 60, `claude_policy_e2e` 3, `claude_review` 24, `consistency` 11 passed + 1 ignored. No failures.

## Escalation (D-071): not started, stopped at the handoff (2026-11-14)

The Gemini 4 Argon worker (`agy` 1.5.8) ran for about 35 minutes and changed nothing: it read the code and the review, and its edits were refused. The cause is the worker policy, not the task: `scripts/agy-policy.py` (`check_paths`) refuses any file-tool argument, **content included**, that starts with `~`, contains `../`, or starts with an absolute path outside the worktree. This escalation's tests need exactly such literals (`../.aws/cred*`, `~bob/*`, `/home/ana/.ssh/id_ed25519`), so an `agy` worker can't write them under today's policy. The orchestrator stopped it for the handoff; the branch is unchanged since `ea14b48`.

Next: the work list is the brief's "Escalation" section on `dev`. Either the policy checks only path arguments (not file content), decided by a driver, or the escalation goes to a worker that isn't `agy`.

## Escalation (D-071, D-074, D-075): Claude Opus 5.5, 2026-11-14

Done on Pedro's shift by a Claude Opus 5.5 worker, from the brief's "Escalation" section and `reviews/M1-07-claude-adapter-round3.md`. Each item got a test that failed first (the failure was seen before the fix). The policy tests run end to end through `atrium_core::policy` `SessionGate` + `DefaultPolicy`, under `Preset::Home` and `Preset::FullAccess`, with home `<tmp>/home/ana` and cwd `<tmp>/home/ana/work`. The hostile patterns are Rust string literals, written with the file tools. Commits: `36fb949` (code and tests) and the handoff commit after it.

**No pattern reaches `Allow` on a credential**, as far as these tests and the exhaustive check can tell.

### Round-3 findings

| # | Finding | Reproduced | Test (fails before, passes after) | Fix |
|---|---|---|---|---|
| 1 | blocker: `[` and `{…}` interleaved (`[{a],.}][{a],.}]/.ssh/id_ed25519`, `[{a],/}]/etc/passwd`, `[{a],~}]/…`) | yes (`Paths`, allowed) | `patterns_we_cannot_bound_are_unknown_and_never_allowed` (Glob and Grep; Home and FullAccess; `Unknown`, never `Allow`), `r3_globs_outside_the_grammar_are_unknown` | `tools.rs` `in_grammar`: the grammar is checked on the raw pattern before any expansion. A brace body can't hold `[` or `]`, and a class body can't hold `{` or `}`, so the two can't interleave |
| 2 | blocker: `~user`, `~*`, `~?…` (`~bob/*`, `~a*/.ssh/id_ed25519`, base `~bob`) | yes | the same two tests (also for the base: `~bob`, `~*`, `~bob/x`, `~?`, `~ana`) | `tilde_ok`: the pattern and the base may start with `~` only as `~` or `~/…`, otherwise the subject is `Unknown` |
| 3 | major: a credential glob of more than one component that stays inside its base (`..` + `.ssh/id_ed25519`, `..` + `.aws/cred*`, home + `.aws/cred*`, `~/.config` + `gh/hosts.yml`, cwd = home + `.ssh/id_ed25519`) | yes (FullAccess allowed it; Home asked instead of hard-denying) | `r3_credential_globs_that_stay_inside_their_base_are_hard_denied` (hard-denied in both presets, as Glob and as Grep glob, including a `cwd = home` gate) | `glob_target`: every expansion with a `/` goes into `paths`, joined to the base |
| 4 | minor: `*.[a-z]` and `file-[0-9].rs` were `Unknown`; `[}-\u{7f}]` was accepted | yes | `ordinary_patterns_keep_their_plain_subject_and_are_allowed_at_home` (now also covers `*.[a-z]`, `file-[0-9].rs`, `*.{d.ts,ts}`, `[A-Z_]*.md`), `r3_globs_inside_the_grammar_name_every_expansion_with_a_slash`; `[}-\u{7f}]` in both `Unknown` lists | `class_body`: only ASCII letters, digits and `_`, plus low-to-high ranges inside one of `a-z`, `A-Z` or `0-9` |
| 5 | minor: `Proc::drop` outside a runtime, with an exited, unreaped leader, left the leader's child alive | yes (the `sleep` grandchild survived) | `r3_a_start_dropped_outside_a_runtime_after_the_leader_exited_kills_the_group` (`claude_adapter.rs`; new fake mode `leader-exits`; the test waits until `ps` shows the leader as `Z`, then drops `start` on a plain thread) | `adapter.rs`: `Proc::drop` runs `supervise::terminate(child, 0)` on a current-thread runtime of its own: inline on a plain thread, or on a short-lived thread when the caller has a runtime (that runtime may be shutting down). `terminate` signals the group while the leader is unreaped, so the pgid can't have been reused, then reaps. If the runtime can't be built, it falls back to `kill_recorded`. This fix stays inside `src/claude/**`, with no `atrium-host` change. The two older drop tests still pass |
| 6 | minor: an oversize `can_use_tool` between turns emitted the call outside a turn; before `init`, `hold` could split `ToolCallStarted` from `ToolCallFinished` | yes (both variants) | `r3_an_oversize_can_use_tool_between_turns_is_shown_inside_a_turn`, `r3_an_oversize_can_use_tool_before_init_is_never_split_by_what_waits` (`claude_review.rs`) | `normalizer.rs`: `establish` + `ensure_turn` before `oversize_call` |

Not taken: the two "Suspected, not shown" items (a dangling `pending` ledger entry; mid-turn settings changes). They were not in the work list.

### Escalation items

| Item | Result | Tests |
|---|---|---|
| 1. Subject grammar | `in_grammar` covers backslash, tilde, braces (letters, digits, `_`, `-`, `.`, `,`; no nesting; no empty, `.` or `..` alternative) and classes (letters, digits, `_`, one-run ranges; no negation; no `[:class:]`). Any other `{ } [ ]` makes the subject `Unknown`. The 32-expansion cap stays (new: at most 64 extra `paths` entries). Every expansion with a `/` is joined to the base; climbing and absolute expansions keep the round-2 directory entry | `r3_globs_outside_the_grammar_are_unknown` (55 patterns plus 5 bases, as Glob and as Grep), `r3_globs_inside_the_grammar_name_every_expansion_with_a_slash` (exact `paths`; the cap at 32/33), `patterns_we_cannot_bound_are_unknown_and_never_allowed` (27 inputs, Glob and Grep, both presets) |
| 1b. Found by item 3 (**not in the review**) | After item 1, the exhaustive test found **196 credential reaches** in 59 patterns, all allowed: for example `~/*` and `~/*.*` from the cwd reach `~/.netrc`, `~/.npmrc`, `~/.git-credentials` and `~/.claude.json`; `../*`, `..//*` and `./*` from `..` reach them too; `*//*a` from `..` reaches `~/.ssh/id_rsa`. The cause is in the policy: it matches a `paths` entry with shell rules (`ShellGlob`: `*`, `?` and a class never match a leading `.`, and an empty `//` component matches nothing). The tool's glob, and `globset`, have neither rule. Fixed on the adapter side with `pattern_forms`: a joined pattern drops its empty and `.` components, and, unless it has a `**` (which the policy matches whole, with no dot rule), it also gets every form with a `.` in front of its wildcard-led components (`~/*` also gives `~/.*`). At most 4 such components (16 forms), otherwise `Unknown`. | the exhaustive test; `r3_globs_inside_the_grammar_*` pins `~/.ssh/*`, `*//*a` and `./x/./y/` |
| 2. Ordinary patterns stay Allow | `**/*.rs`, `src/**/mod.rs`, `*.{ts,tsx}`, `src/{a,b}/mod.rs`, `*.[ch]`, `*.[a-z]`, `file-[0-9].rs` (plus `*.{d.ts,ts}` and `[A-Z_]*.md`) are `Allow` in Home with base cwd, as Glob and as Grep | `ordinary_patterns_keep_their_plain_subject_and_are_allowed_at_home` |
| 3. Exhaustive test | `every_short_pattern_the_policy_allows_reaches_no_credential`: every pattern of 1 to 5 characters, alone and with the suffix `/.ssh/id_ed25519`, from base cwd and base `..`. For each pattern the policy allows in Home or in FullAccess (FullAccess is asked only when Home doesn't allow), it asserts that no credential file of the layout (10 of them: `.ssh/id_ed25519`, `.ssh/id_rsa`, `.aws/credentials`, `.netrc`, `.git-credentials`, `.npmrc`, `.claude.json`, `.kube/config`, `.docker/config.json`, `.config/gh/hosts.yml`) matches. The match uses `globset` with `literal_separator(true)`, after the oracle's own brace expansion, with `~` expanded and `.`/`..`/empty components normalized. The joined pattern is matched against the whole path, and a pattern without `/` against the names of the files under the base. A trailing `/` is dropped, which only widens the match. `Unknown` subjects (never allowed) go through the policy on a sample, every 997th | **444,440 patterns** (411,244 `Unknown`, 417 of them sampled through the policy; 33,196 understood, 32,727 allowed): **0 credential reaches**, in **23.6 to 26.2 s** for the test on this 10-core Mac in a debug build. Alphabet: **`-` dropped** (`{ } [ ] , . / ~ * a`), as the brief allows: with `-` it is 708,620 patterns (71,184 understood, 70,644 allowed), also with 0 reaches, but 38.7 to 46.7 s. Nearly all of the time is in the policy's own `Sensitive::glob_hit`, which compiles a glob per call (seen with `sample`). When other builds share the CPU, the whole `claude_policy_e2e` binary took 29 to 40 s (load average about 418 during the DoD runs) |
| 4. Minors 5 and 6 | done (see findings 5 and 6). Minor 5 needed no `atrium-host` change | above |
| 5. DoD and this section | below | |

Old tests updated because the brief changes what they expect (each one is commented in place):
- `ESCAPES` in `claude_policy_e2e.rs`: round 2's brace escapes (`{a,}…`, `.{.,.}`, `{..,x}`, `{x,~}`, `.{,}.`, nested groups) moved to `NOT_UNDERSTOOD`. They are now `Unknown`, so Home asks instead of hard-denying; they are still never allowed.
- `f2_braces_that_hide_a_root_are_expanded_and_the_target_named` became `f2_braces_that_hide_a_root_are_unknown`.
- `~*` in `f2_a_glob_pattern_that_leaves_the_base_names_its_target` is now `Unknown`, and `**/*.{rs,toml}` there now names its two joined expansions.
- The `Grep` `../../x/**` case drops the `.` component.
- `every_check_carries_the_extracted_subject` (`claude_normalizer.rs`): `**/*.rs` now carries its joined form.

Other changes:
- `Cargo.toml`: dev-dependency `globset = "0.4" # M1-07`. `Cargo.lock` gains that one dependency line for `atrium-agents`; `globset` was already in the lock through `atrium-core`.
- `examples/claude-fake.rs`: new mode `leader-exits`.
- No contract change, no change request, no golden change, no new recording.

For the policy side (P1, `atrium-core`, not mine), one finding: `Sensitive::glob_hit` (`ShellGlob`) and `search_glob_hit` treat a search tool's glob with shell semantics (dot rule, empty components), while the tool's glob has none. The adapter now hands the policy the extra forms, but the policy could match search globs `globset`-style itself, and compile them once per check rather than once per `paths` entry (that compiling is what makes the exhaustive test slow).

Limits of the exhaustive check: it is bounded to length 5 and this alphabet (no `?`, no other letters, and no `-` in the run that is kept). Longer hostile patterns are covered by the hand-written lists. The oracle treats the Glob tool's matching as `globset`'s, with a pattern without `/` also matching file names at any depth. If the real CLI's Glob matched a directory's contents when the directory's name matched, the check would not see that.

### DoD after the escalation
- `cargo build -p atrium-agents --examples`: exit 0.
- `cargo fmt --all --check`: exit 0, no output.
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0, no warnings (macOS host).
- `cargo test -p atrium-agents --no-fail-fast`, three consecutive runs, no flake: unit 25, `claude_adapter` 38, `claude_golden` 20, `claude_normalizer` 60, `claude_policy_e2e` 5, `claude_review` 28, `consistency` 11 passed + 1 ignored. No failures. (`claude_policy_e2e` took 29.4, 35.1 and 39.7 s while other builds were running; alone, its exhaustive test takes 23.6 to 26.2 s.)

### Review fix (Gemini 4 Hydrogen `high`, `reviews/M1-07-claude-adapter-escalation.md`: approve with changes)
- **Minor 1, the exhaustive test over the 30 s budget** (the reviewer measured 41.2 s standalone and 49.3 s for the binary): fixed in `tests/claude_policy_e2e.rs` only.
  - The test now checks the same property turned around. "Every allowed pattern reaches no credential" is equivalent to "no pattern that reaches a credential is allowed".
  - The oracle (`reached`) is cheap. It runs on every understood pattern, one thread per core.
  - Only the patterns that reach a credential go through the real `SessionGate`, in Home and in FullAccess. The `Unknown` sample also still goes through the gate.
  - Nearly all the old time was the policy's own `hard_deny` / `ShellGlob` compiling, about 7 ms of CPU per call over about 33,000 calls (seen with `sample`).
  - Two other approaches were measured and **rejected**:
    - Calling `DefaultPolicy::evaluate` directly: 26 to 38 s, so the gate wrapper was not the cost.
    - Asking FullAccess first, with work stealing: 26 to 32 s.
  - **`-` is back in the alphabet**, so the test now covers all of the brief's `{ } [ ] , . / ~ * a -`. 708,620 patterns: 637,436 `Unknown` (640 of them sampled through the gate) and 71,184 understood. Of those, 341 reach a credential and go through the gate. None is allowed.
  - **Time:** before, 23.6 to 26.2 s here (41.2 s on the reviewer's run) with `-` dropped, and 38.7 to 46.7 s with it. After, **6.7 to 8.5 s** standalone with `-` included, and 9.0 to 12.6 s for the whole `claude_policy_e2e` binary, with a load average of 24 to 30 from other builds.
- **Mutation check (not committed):** I set `pattern_forms` to never add the dot forms (`if true || joined.contains("**")`). The test then failed with **196 leaks**, the same count found in the escalation (for example, FullAccess allows `{"path":"..","pattern":"*.*/"}`, which reaches `~/.netrc`). I restored the file with `git checkout` and confirmed that `tools.rs` is unchanged.
- **Nit 2, for the policy owner (P1, `atrium-core`, not mine):** `search_glob_hit` compiles a single-component glob case-sensitively and matches it against lowercased probes. If a provider's glob ever ran case-insensitively (the macOS default filesystem), `{"path":"..","pattern":".NETRC"}` could slip past it under FullAccess. Today's `globset`/ripgrep matching is case-sensitive, so this is a note, not a finding against M1-07.
- **DoD after the review fix:**
  - `cargo build -p atrium-agents --examples`, `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings`: exit 0.
  - `cargo test -p atrium-agents --no-fail-fast`, three runs: unit 25, `claude_adapter` 38, `claude_golden` 20, `claude_normalizer` 60, `claude_policy_e2e` 5, `claude_review` 28, `consistency` 11 passed + 1 ignored. No failures.
