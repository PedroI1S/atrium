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
