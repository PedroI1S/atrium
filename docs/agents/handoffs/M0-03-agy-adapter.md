# Handoff: M0-03-agy-adapter

- **Task**: `M0-03-agy-adapter` — Antigravity (`agy`) stream-json client
- **Worker model**: Gemini 4 Argon
- **Branch**: `agent/argon/M0-03-agy-adapter`
- **Base commit**: `dbea25d`
- **Head commit**: `6bf72fe`
- **Status**: `ready-for-review`

## Summary
Implemented `AgyAdapter` (`impl AgentAdapter for AgyAdapter`) and `AgyNormalizer` (`impl Normalizer for AgyNormalizer`) in `spikes/agent-agy`, along with the per-session Unix-socket `PreToolUse` gate bridge (`GateBridge`), standalone fail-closed helper binary (`spike-agy-hook`), single-use canonical-JSON `RetryGrantStore` for the `DenyAndRetry` fallback, and per-session MCP config injection (`write_session_mcp_config`) that never touches `~/.gemini`. Generated golden events for `m0-00-smoke.jsonl` and all 10 required scenario fixtures (`simple`, `tool-read`, `gate-deny`, `approval-allow`, `approval-deny`, `question`, `interrupt`, `multi-turn`, `resume`, `startup-error`), built the `--example record`, `--example chat`, and `--example hook-failures` harnesses, verified all 12 golden/bridge/retry-grant/saturation/harness tests offline, and wrote the spike report in `docs/spikes/M0-03-agy-adapter.md`. **Note: nothing has been verified live against `agy` 1.4.7 from inside this worker session because worker policy blocks invoking `agy`; all live verification runs (`record`, `hook-failures`, `chat`) are executed by the orchestrator on the host.**

## Fix round 1
1. **Adapter launch argv and stdin NDJSON shape (`spikes/agent-agy/src/adapter.rs`, `examples/record.rs`, `fixtures/agents/agy/*.jsonl`)**:
   - Replaced `-p` (which consumed `"--input-format"` as its prompt argument) with `--print=` (empty prompt attached with `=`) in `AgyAdapter::build_cli_args`: `agy --print= --input-format stream-json --output-format stream-json --print-timeout 0s --mode <accept-edits|plan|auto> --dangerously-skip-permissions ...`.
   - Replaced the stdin message shape with `{"event":"user","message":{"content":"<text>"}}` (`StdinFormat::EventUserMessage`), matching `agy` 1.4.7's parser.
   - Enabled `install_cwd_hook_if_absent = true` by default on `AgyAdapter` so sessions spawned in fresh directories automatically install `.agents/hooks.json` in `cwd` and clean it up on exit.
   - Regenerated all 10 hand-made fixtures in `fixtures/agents/agy/` with the new stdin shape and added unit tests in `tests/bridge_and_saturation.rs` and `tests/golden.rs` that pin the exact `argv` vector and `stdin` NDJSON line.
2. **Non-vacuous `examples/hook-failures.rs` with mandatory control case**:
   - Removed invalid flags `--mode agent` and `--workspace <dir>`; now uses `cmd.current_dir(&workspace)` and `--print=<prompt> --model <model> --mode accept-edits --dangerously-skip-permissions --output-format stream-json --print-timeout 0s`.
   - Added a mandatory `control-allow` case first (a healthy `PreToolUse` hook returning `{"decision":"allow"}`) that must see `init`, attempt the tool call, and create `canary.txt`. If `control-allow` does not create `canary.txt`, the harness prints `OVERALL RUN: INCONCLUSIVE` and exits with code 2.
   - Each failure case now parses `stdout` stream-json for `init_seen` and `tool_attempted`, and counts as `fail_closed: true` (`verdict: "fail_closed"`) **only** when `init_seen && tool_attempted && !canary_created`; if `!init_seen || !tool_attempted`, it reports `verdict: "inconclusive"` and `fail_closed: false`.
3. **Loud failure in `examples/record.rs` when `SessionStarted` is not reached**:
   - `write_fixture_files` and `record_live_scenario` now fail loudly (returning `Err` and exiting non-zero without writing any fixture files) whenever any scenario other than `startup-error` fails to reach `AgentEvent::SessionStarted` (including the seed session in `resume`). Tested offline in `record_and_hook_failures_harnesses_reject_broken_launches`.
4. **Updated spike report (`docs/spikes/M0-03-agy-adapter.md`) and handoff**:
   - Stated plainly that nothing has been verified live from inside the worker session yet and documented the Fix round 1 changes.

## Files touched
Output of `git diff --stat dbea25d..6bf72fe`:

```text
 docs/agents/handoffs/M0-03-agy-adapter.md       |   87 +
 docs/spikes/M0-03-agy-adapter.md                |  179 ++
 fixtures/agents/agy/approval-allow.events.json  |  100 ++
 fixtures/agents/agy/approval-allow.jsonl        |   14 +
 fixtures/agents/agy/approval-allow.md           |    7 +
 fixtures/agents/agy/approval-deny.events.json   |  100 ++
 fixtures/agents/agy/approval-deny.jsonl         |   14 +
 fixtures/agents/agy/approval-deny.md            |    7 +
 fixtures/agents/agy/gate-deny.events.json       |  100 ++
 fixtures/agents/agy/gate-deny.jsonl             |   19 +
 fixtures/agents/agy/gate-deny.md                |    7 +
 fixtures/agents/agy/interrupt.events.json       |   71 +
 fixtures/agents/agy/interrupt.jsonl             |   11 +
 fixtures/agents/agy/interrupt.md                |    7 +
 fixtures/agents/agy/m0-00-smoke.events.json     |  424 +++++
 fixtures/agents/agy/m0-00-smoke.md              |    6 +-
 fixtures/agents/agy/multi-turn.events.json      |  106 ++
 fixtures/agents/agy/multi-turn.jsonl            |   12 +
 fixtures/agents/agy/multi-turn.md               |    7 +
 fixtures/agents/agy/question.events.json        |  121 ++
 fixtures/agents/agy/question.jsonl              |   13 +
 fixtures/agents/agy/question.md                 |    7 +
 fixtures/agents/agy/resume.events.json          |   66 +
 fixtures/agents/agy/resume.jsonl                |    7 +
 fixtures/agents/agy/resume.md                   |    7 +
 fixtures/agents/agy/simple.events.json          |   72 +
 fixtures/agents/agy/simple.jsonl                |    8 +
 fixtures/agents/agy/simple.md                   |    7 +
 fixtures/agents/agy/startup-error.events.json   |   12 +
 fixtures/agents/agy/startup-error.jsonl         |    2 +
 fixtures/agents/agy/startup-error.md            |    7 +
 fixtures/agents/agy/tool-read.events.json       |   83 +
 fixtures/agents/agy/tool-read.jsonl             |   14 +
 fixtures/agents/agy/tool-read.md                |    7 +
 spikes/agent-agy/examples/chat.rs               |  176 ++
 spikes/agent-agy/examples/hook-failures.rs      |  523 ++++++
 spikes/agent-agy/examples/record.rs             | 2162 +++++++++++++++++++++++
 spikes/agent-agy/src/adapter.rs                 |  601 +++++++
 spikes/agent-agy/src/bin/spike-agy-hook.rs      |  102 ++
 spikes/agent-agy/src/bridge.rs                  |  700 ++++++++
 spikes/agent-agy/src/lib.rs                     |   15 +-
 spikes/agent-agy/src/normalizer.rs              | 1452 +++++++++++++++
 spikes/agent-agy/tests/bridge_and_saturation.rs |  882 +++++++++
 spikes/agent-agy/tests/golden.rs                |  266 +++
 44 files changed, 8586 insertions(+), 4 deletions(-)
```

## Definition of Done check
- `cargo fmt --check` → exit 0
- `cargo clippy -p spike-agent-agy --all-targets -- -D warnings` → exit 0 (0 warnings, 0 errors)
- `cargo test -p spike-agent-agy` → exit 0 (`tests/golden.rs`: 3 passed, 0 failed; `tests/bridge_and_saturation.rs`: 9 passed, 0 failed; 12 passed total)
  - Golden test over `m0-00-smoke.jsonl` + all 10 scenarios in `docs/contracts/m0.md` §3.3 (`simple`, `tool-read`, `gate-deny`, `approval-allow`, `approval-deny`, `question`, `interrupt`, `multi-turn`, `resume`, `startup-error`), marked `hand-made` in their `.md`, plus `Sanitizer::is_clean` check and exact stdin JSON shape check (`{"event":"user","message":{"content":"..."}}`) on all files.
  - Saturation test (`saturation_backpressure_interrupt_and_shutdown_complete_within_grace`): floods > `EVENT_BUFFER` (`2,448` events) while `session.events` is unread, verifies `Interrupt` and `Shutdown` complete within `SHUTDOWN_GRACE` and child is reaped.
  - Bridge unit/integration tests (`bridge_allow_and_deny`, `bridge_ask_block_allow_and_deny_and_question`, `bridge_fails_closed_when_adapter_dropped_mid_wait`, `bridge_fails_closed_on_all_error_cases`, `spike_agy_hook_binary_end_to_end_and_cli_args`): all 10 error cases return `decision == "deny"`, and exact `argv` + `stdin` line are pinned.
  - Harness rejection test (`record_and_hook_failures_harnesses_reject_broken_launches`): verifies `hook-failures` exits 2 with `OVERALL RUN: INCONCLUSIVE` (never `fail_closed: true`) when `agy` fails to start, and `record` exits non-zero without writing a fixture when `SessionStarted` is not reached.
  - Retry-grant tests (`retry_grant_single_use_and_canonical_args_and_argument_change`): same canonical args (even with reordered JSON object keys) allowed once; second use or changed args/tool/session requires a new gate decision.
- `cargo run -p spike-agent-agy --example record -- --generate-handmade all` → exit 0
- `cargo run -p spike-agent-agy --example hook-failures -- --self-test` → exit 0 (3/3 bridge failure cases fail closed)
- Live commands for the orchestrator to run during verification against installed `agy`:
  - `cargo run -p spike-agent-agy --example hook-failures`
  - `cargo run -p spike-agent-agy --example record -- <scenario>` (where `<scenario>` is `simple`, `tool-read`, `gate-deny`, `approval-allow`, `approval-deny`, `question`, `interrupt`, `multi-turn`, `resume`, `startup-error`, or `all`)
  - `cargo run -p spike-agent-agy --example chat -- --prompt "Run pwd and tell me the directory."`

## Spike report (M0 only)
Path: `docs/spikes/M0-03-agy-adapter.md`. Keep the `atrium-agents` and `atrium-protocol` contracts as-is (zero contract changes needed) and promote `AgyAdapter`, `AgyNormalizer`, `GateBridge`, and `RetryGrantStore` into `crates/atrium-agents` in M1 after running the live `hook-failures` and `record` harnesses on the host.

## Deviations / notes for reviewer
- **macOS `SUN_LEN` (104-byte) Unix socket path limit**: On macOS, `std::env::temp_dir()` expands to `/var/folders/<hash>/T/` (~50 chars). A verbose session directory and socket path easily exceeds 104 bytes and fails `UnixListener::bind` with `InvalidInput ("path must be shorter than SUN_LEN")`. `create_private_session_dir` and `AgyAdapter` use compact `<temp_dir>/agy/s-<pid:x>-<seq:x>/g.sock` paths (~68 bytes on macOS) with mode `0700` on the directory and `0600` on `g.sock`, `hooks.json`, and `mcp.json`.
- **`serde_json` `preserve_order`**: Because the workspace enables `serde_json`'s `preserve_order` feature, `canonical_json` in `normalizer.rs` recursively sorts all JSON object keys via `BTreeMap` before serializing so `RetryGrantStore` matches identical tool arguments regardless of key order.
- **`scripts/agy-policy.py` note for M1**: In `scripts/agy-policy.py`, `check_paths()` iterates over all string values in tool args (`strings(args)`), which includes `Content`, `Target`, and `Replacement` on `write_file`/`edit_file` (not just `Path`). If a file's content starts with `/` (such as a `//!` doc comment at byte 0) or contains a parent-directory dot-dot-slash sequence or a slash-dot-agents-slash sequence, the worker policy hook denies the write as a path or `.agents` violation. We did not touch `scripts/agy-policy.py` (out of scope), but worth noting for future worker prompts.

## If blocked
Not blocked.
