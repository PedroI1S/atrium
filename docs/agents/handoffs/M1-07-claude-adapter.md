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
