# Handoff: M0-03-agy-adapter

- **Task**: `M0-03-agy-adapter` — Antigravity (`agy`) stream-json client
- **Worker model**: Gemini 4 Argon
- **Branch**: `agent/argon/M0-03-agy-adapter`
- **Base commit**: `dbea25d`
- **Head commit**: `883eb54`
- **Status**: `ready-for-review`

## Summary
Implemented `AgyAdapter` (`impl AgentAdapter for AgyAdapter`) and `AgyNormalizer` (`impl Normalizer for AgyNormalizer`) in `spikes/agent-agy`, along with the per-session Unix-socket `PreToolUse` gate bridge (`GateBridge`), standalone fail-closed helper binary (`spike-agy-hook`), single-use canonical-JSON `RetryGrantStore` for the `DenyAndRetry` fallback, and per-session MCP config injection (`write_session_mcp_config`) that never touches `~/.gemini`. Generated golden events for `m0-00-smoke.jsonl` and all 10 required scenario fixtures (`simple`, `tool-read`, `gate-deny`, `approval-allow`, `approval-deny`, `question`, `interrupt`, `multi-turn`, `resume`, `startup-error`), built the `--example record`, `--example chat`, and `--example hook-failures` harnesses, verified all golden/bridge/retry-grant/saturation tests offline, and wrote the spike report in `docs/spikes/M0-03-agy-adapter.md`.

## Files touched
Output of `git diff --stat dbea25d..883eb54` (plus this handoff file):

```text
 docs/spikes/M0-03-agy-adapter.md                |  175 ++
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
 spikes/agent-agy/examples/hook-failures.rs      |  345 ++++
 spikes/agent-agy/examples/record.rs             | 2132 +++++++++++++++++++++++
 spikes/agent-agy/src/adapter.rs                 |  609 +++++++
 spikes/agent-agy/src/bin/spike-agy-hook.rs      |  102 ++
 spikes/agent-agy/src/bridge.rs                  |  700 ++++++++
 spikes/agent-agy/src/lib.rs                     |   15 +-
 spikes/agent-agy/src/normalizer.rs              | 1452 +++++++++++++++
 spikes/agent-agy/tests/bridge_and_saturation.rs |  699 ++++++++
 spikes/agent-agy/tests/golden.rs                |  233 +++
 43 files changed, 8079 insertions(+), 4 deletions(-)
```

## Definition of Done check
- `cargo fmt --check` → exit 0
- `cargo clippy -p spike-agent-agy --all-targets -- -D warnings` → exit 0 (0 warnings, 0 errors)
- `cargo test -p spike-agent-agy` → exit 0 (`tests/golden.rs`: 3 passed, 0 failed; `tests/bridge_and_saturation.rs`: 8 passed, 0 failed)
  - Golden test over `m0-00-smoke.jsonl` + all 10 scenarios in `docs/contracts/m0.md` §3.3 (`simple`, `tool-read`, `gate-deny`, `approval-allow`, `approval-deny`, `question`, `interrupt`, `multi-turn`, `resume`, `startup-error`), marked `hand-made` in their `.md`, plus `Sanitizer::is_clean` check on all files.
  - Saturation test (`saturation_backpressure_interrupt_and_shutdown_complete_within_grace`): floods > `EVENT_BUFFER` (`2,448` events) while `session.events` is unread, verifies `Interrupt` and `Shutdown` complete within `SHUTDOWN_GRACE` and child is reaped.
  - Bridge unit/integration tests (`bridge_allow_and_deny`, `bridge_ask_block_allow_and_deny_and_question`, `bridge_fails_closed_when_adapter_dropped_mid_wait`, `bridge_fails_closed_on_all_error_cases`, `spike_agy_hook_binary_end_to_end_and_cli_args`): all 10 error cases return `decision == "deny"`.
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
- **`scripts/agy-policy.py` note for M1**: In `scripts/agy-policy.py`, `check_paths()` iterates over all string values in tool args (`strings(args)`), which includes `Content`, `Target`, and `Replacement` on `write_file`/`edit_file` (not just `Path`). If a file's content starts with `/` (such as a `//!` doc comment at byte 0) or contains a parent-directory dot-dot-slash sequence, the worker policy hook denies the write as a path traversal. We did not touch `scripts/agy-policy.py` (out of scope), but worth noting for future worker prompts.

## If blocked
Not blocked.
