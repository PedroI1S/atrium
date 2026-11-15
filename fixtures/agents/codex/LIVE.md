# Codex live verification runs (X1 - X3)

These checks require a real logged-in `codex` binary and must **not** be run
from offline agent workers. `codex-record` defaults to
`PermissionPolicy::ReadOnly` (Home's mode, with `-c features.hooks=false`) unless
explicitly overridden via `--permissions read-only|ask|full-access` (only
`probe-fullaccess` defaults to `full-access`).

Before accepting any `EnforcementDescriptor` upgrade or deleting M0 hand-made
fixtures, run these probes on a developer machine:

## List available scenarios

```fish
cargo run -p atrium-agents --example codex-record -- --list
```

## X1 - File-change (`patch`) re-recording and per-class interception probes

`m1-must-fix.md` item X1 requires re-recording `patch` against a live Codex
session and probing `fileChange` `grantRoot` + every tool class currently marked
`Unknown`, `ProviderGated`, or `NotIntercepted`:

```fish
cargo run -p atrium-agents --example codex-record -- --permissions ask patch
cargo run -p atrium-agents --example codex-record -- --permissions ask x1-grant-root
cargo run -p atrium-agents --example codex-record -- x1-read
cargo run -p atrium-agents --example codex-record -- x1-edit
cargo run -p atrium-agents --example codex-record -- x1-delete
cargo run -p atrium-agents --example codex-record -- x1-move
cargo run -p atrium-agents --example codex-record -- x1-search
cargo run -p atrium-agents --example codex-record -- x1-fetch
cargo run -p atrium-agents --example codex-record -- x1-subagent
```

Check each recorded fixture:
- Confirm `ATRIUM-7Q2X-CANARY` does not appear in any `.jsonl` or `.events.json`.
- Inspect `ToolCheck` vs actual effect in `--keep` scratch dir (and confirm `/tmp/atrium-grant-root-probe.txt` was NOT created when `x1-grant-root` declined `grantRoot`).
- Only upgrade a class in `crates/atrium-agents/src/codex/normalizer.rs` (`coverage()`) when backed by a checked-in recorded denial fixture showing no leak.

## X2 - Question flow and `DeclineQuestion` (`requestUserInput`)

`m1-must-fix.md` item X2 requires verifying `item/tool/requestUserInput` live
under both answer and decline:

```fish
cargo run -p atrium-agents --example codex-record -- question
cargo run -p atrium-agents --example codex-record -- x2-question-decline
```

Check `x2-question-decline`:
- Verify whether live Codex accepts `{"answers": {}}` as a user dismissal and continues the turn cleanly.
- If live Codex rejects empty answers or wedges the turn, switch `DeclineQuestion` in `adapter.rs` / `normalizer.rs` to the D-091 fallback (`RequestResolved { outcome: Cancelled }`, then `Error { fatal: false }`, then `turn/interrupt`).

## X3 - MCP row, `preToolUse` hook, granular policy, `.rules`, and exec amendment

`m1-must-fix.md` item X3 requires verifying MCP interception with a configured
MCP server, `preToolUse` (`features.hooks`), granular-policy options, `.rules`
allow-prefix behavior, and repeated command approvals:

```fish
cargo run -p atrium-agents --example codex-record -- x3-mcp
cargo run -p atrium-agents --example codex-record -- x3-hooks
cargo run -p atrium-agents --example codex-record -- x3-rules
cargo run -p atrium-agents --example codex-record -- x3-granular
cargo run -p atrium-agents --example codex-record -- --extra-arg '-c' --extra-arg 'approval_policy="untrusted"' x3-granular
cargo run -p atrium-agents --example codex-record -- x3-exec-amendment
```

Notes:
- `x3-mcp` spawns `codex-record --mcp-probe-server` as stdio MCP server `probe` exposing `ping`, and denies its gate check. Verify whether Codex sends an approval request before executing `probe:ping` and whether `Mcp` can be verified or stays `Unknown`.
- `x3-hooks` populates an isolated `$CODEX_HOME/hooks.json` with a `PreToolUse` command hook and runs with `-c features.hooks=true`; verify hook invocation and external-hook detection with `--keep`.
- `x3-rules` populates an isolated `$CODEX_HOME/rules/allow.rules` with an `allow` prefix rule for `echo` to verify whether `.rules` bypasses app-server approval requests under `untrusted`.
- Confirm `x3-exec-amendment` prompts each command execution individually under `approvalPolicy = "untrusted"` and `approvalsReviewer = "user"`.

## Supplemental probe - `FullAccess` vs Atrium policy

Confirm behavior under `PermissionPolicy::FullAccess` (`approvalPolicy = "never"`):

```fish
cargo run -p atrium-agents --example codex-record -- probe-fullaccess
```

Verify whether `approvalPolicy: "never"` suppresses app-server approval requests.

## Re-sanitizing and missing `enforcement` records note

Note that the checked-in M1 fixtures do not currently contain recorded
`{"dir":"enforcement",...}` wire lines, so `consistency_all_fixtures` evaluates
them against `default_enforcement(cli_version)` with empty `config_sources`.

After recording live `patch`, `question`, `interrupt`, `quota-usage`, and probe
runs, copy/re-sanitize verified fixtures and run:

```fish
cargo run -p atrium-agents --example codex-record -- --resanitize all
cargo test -p atrium-agents --test codex_golden
cargo test -p atrium-agents --test consistency
```
