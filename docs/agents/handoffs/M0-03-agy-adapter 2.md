# Handoff: M0-03-agy-adapter

- **Task**: `M0-03-agy-adapter` — Antigravity (`agy`) stream-json client
- **Worker model**: Gemini 4 Argon
- **Branch**: `agent/argon/M0-03-agy-adapter`
- **Base commit**: `dbea25d`
- **Head commit**: `ebc3bb8` (code and report; this handoff is committed right after)
- **Fix rounds**: 1 (`6bf72fe`, `0ef8b90`), 2 (`09d6b74`, `7d02999`, `ebc3bb8`, taken over from Argon by a Claude worker because Argon is paused, D-015)
- **Status**: `ready-for-review`

## Summary
Implemented `AgyAdapter` (`impl AgentAdapter for AgyAdapter`) and `AgyNormalizer` (`impl Normalizer for AgyNormalizer`) in `spikes/agent-agy`, along with the per-session Unix-socket `PreToolUse` gate bridge (`GateBridge`), standalone fail-closed helper binary (`spike-agy-hook`), single-use canonical-JSON `RetryGrantStore` for the `DenyAndRetry` fallback, and per-session MCP config injection (`write_session_mcp_config`) that never touches `~/.gemini`. Generated golden events for `m0-00-smoke.jsonl` and all 10 required scenario fixtures (`simple`, `tool-read`, `gate-deny`, `approval-allow`, `approval-deny`, `question`, `interrupt`, `multi-turn`, `resume`, `startup-error`), built the `--example record`, `--example chat`, and `--example hook-failures` harnesses, verified all 12 golden/bridge/retry-grant/saturation/harness tests offline, and wrote the spike report in `docs/spikes/M0-03-agy-adapter.md`. **Live status**: the worker policy blocks invoking `agy`, so all live runs are done by the orchestrator. After Fix round 1 it verified `simple`, `tool-read`, `gate-deny` (read_file and run_command on `secret.txt` denied, secret never appears), `approval-allow` and `approval-deny` live, so the blocking hook bridge works; the hook-failure harness found that `agy` **fails open** on an unreadable or malformed `hooks.json` (everything else fails closed). Fix round 2 (below) hardens the adapter against that. Nothing from round 2 has been run live yet.

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

## Fix round 2
Taken over from Argon after the orchestrator's live verification of round 1. Details, exact cases and evidence are in `docs/spikes/M0-03-agy-adapter.md` sections 2.2 and 6.

1. **Fail-open hardening** (`src/bridge.rs`, `src/adapter.rs`, `tests/gate_hardening.rs`):
   - `start()` reads every hook config back before spawning `agy` and refuses (`AdapterError::Handshake`) unless it is a regular file (no symlink) with mode exactly `0600` in a `0700` dir (project dir: not group/other-writable), parses, and registers a `PreToolUse` matcher `"*"` command hook with exactly our command and timeout. The helper must exist and be executable. Unit-tested for mode 0000, malformed JSON (4 forms), missing/absent gate hook (7 mutations), loose modes, symlinks, missing helper.
   - A hook config already present in the working directory now also refuses to start (before, the adapter silently skipped installing its own and `agy` ran ungated). The new file is `create_new` + `0600`; on any failure only what the adapter created is removed.
   - Runtime tripwire (`enforce_gate_coverage`, default on): re-verifies the config at every `ToolCallStarted`, and counts tools that ran (`Succeeded`/`Failed`) against hook requests answered `allow`; a surplus aborts the session with a fatal `Error` ("gate bypass suspected ..."), locks the bridge down and SIGKILLs the child. A true pre-first-turn self-check is not possible (the hook only fires on a tool call); this is the "first gate call must arrive" check, per tool. `--no-coverage-check` on `record` and `chat` switches it off for investigation.
   - Fail-open finding and exact cases are in the report's enforcement matrix (2.2) and security section (6.1, 6.4). `hook-failures` now exits 3 with a `FAIL-OPEN:` line if any case ran the tool.
2. **Sanitizer** (`src/sanitize.rs`): `FixtureSanitizer` maps the scratch dir (given, realpath, `/private` forms, `file://`) to `/scratch` and other temp roots (incl. any `/var/folders/<id>/<id>/T`) to `/scratch-tmp`, then runs the contract `Sanitizer`. `is_clean` fails if a temp path survives; `record` asserts it; `tests/golden.rs` fails if any committed fixture has one; `tests/sanitize_and_record.rs` covers every spelling and every `AdapterInput` kind.
3. **Recorder** (`examples/record.rs`): per-scenario expectations checked before writing (nothing is written on a miss, and a sanitized dump goes to `$TMPDIR/atrium-agy-record-failures/`); raw-recording secret check (the old post-sanitize check was vacuous because the secret is a sanitizer secret); `resume` seed is awaited to full shutdown; `interrupt` waits up to 5 s after `Interrupt`; the 90 s deadline now fails; `-- all` keeps going and fails at the end. `tests/recorder_scenarios.rs` runs the real `record` against a scripted fake `agy` for all 10 scenarios (approvals through the real helper and socket).
4. **Bug found by that test**: `ask_question` produced two `QuestionAsked` (one from the tool step, one from the hook, different ids) when the step arrives first, which is the live order, so an answer could never reach the bridge. It is now emitted once, from the hook request, with the bridge's id (`tests/golden.rs`, both orders). Also: cleanup now runs before the final event flush.
5. **Report and handoff** updated; claims that are no longer true are corrected (live status, the false `AGY_MCP_CONFIG` statement, stale open risks).

## Files touched
Output of `git diff --stat dbea25d..ebc3bb8 -- . ':!docs/agents/handoffs'` (47 files, +11079/-4); the files new or changed in round 2: `spikes/agent-agy/src/{bridge,adapter,normalizer,lib,sanitize}.rs`, `spikes/agent-agy/examples/{record,chat,hook-failures}.rs`, `spikes/agent-agy/tests/{gate_hardening,sanitize_and_record,recorder_scenarios,golden}.rs`, `docs/spikes/M0-03-agy-adapter.md`, and `fixtures/agents/agy/*.md` (prompt line added; no `.jsonl` or `.events.json` fixture changed). Nothing outside the brief's owned paths was touched.

## Definition of Done check
Run at `ebc3bb8` (Fix round 2):
- `cargo fmt --check` -> exit 0
- `cargo clippy -p spike-agent-agy --all-targets -- -D warnings` -> exit 0 (0 warnings)
- `cargo test -p spike-agent-agy` -> exit 0, 37 tests: `bridge_and_saturation` 9, `gate_hardening` 14, `golden` 4, `recorder_scenarios` 1, `sanitize_and_record` 9 (what each covers: report section 4). Golden over `m0-00-smoke.jsonl` + the 10 hand-made scenarios (marked `hand-made` in their `.md`); saturation test; bridge tests with a fake hook client (allow, deny, ask -> allow/deny, adapter dropped mid-wait, malformed request, socket missing: all deny); retry-grant tests.
- `cargo run -p spike-agent-agy --example agy-record -- --generate-handmade all` -> exit 0 (no `.jsonl` or `.events.json` changed)
- `cargo run -p spike-agent-agy --example hook-failures -- --self-test` -> exit 0 (3/3 bridge failure cases fail closed)

**Live commands for the orchestrator** (build first: `cargo build -p spike-agent-agy --bins --examples`; the adapter now refuses to start without the `spike-agy-hook` binary):
- Remaining scenarios: `cargo run -p spike-agent-agy --example agy-record -- question` (then `interrupt`, `multi-turn`, `resume`, `startup-error`)
- Re-record the first five (earlier ones were discarded for the temp-path leak): `record -- simple`, `tool-read`, `gate-deny`, `approval-allow`, `approval-deny` (or `record -- all`). Afterwards `git grep -n -e /var/folders -e /private/var fixtures/` must find nothing.
- Optional: `cargo run -p spike-agent-agy --example hook-failures` (expect `FAIL-OPEN:` for `unreadable-config-permissions` and `malformed-config-json`, exit 3); `cargo run -p spike-agent-agy --example agy-chat -- --cwd "$(mktemp -d)" --prompt "Run pwd and tell me the directory."` (use a fresh `--cwd`: a directory that already has a hook config is refused by design).
- If a run aborts with `gate bypass suspected`, that is a finding (a tool ran with no hook request, or the config stopped validating); `--no-coverage-check` shows what `agy` does without the abort.

## Spike report (M0 only)
Path: `docs/spikes/M0-03-agy-adapter.md`. Keep the `atrium-agents` and `atrium-protocol` contracts as-is (zero contract changes needed) and promote `AgyAdapter`, `AgyNormalizer`, `GateBridge`, and `RetryGrantStore` into `crates/atrium-agents` in M1 after running the live `hook-failures` and `record` harnesses on the host.

## Deviations / notes for reviewer
- **macOS `SUN_LEN` (104-byte) Unix socket path limit**: On macOS, `std::env::temp_dir()` expands to `/var/folders/<hash>/T/` (~50 chars). A verbose session directory and socket path easily exceeds 104 bytes and fails `UnixListener::bind` with `InvalidInput ("path must be shorter than SUN_LEN")`. `create_private_session_dir` and `AgyAdapter` use compact `<temp_dir>/agy/s-<pid:x>-<seq:x>/g.sock` paths (~68 bytes on macOS) with mode `0700` on the directory and `0600` on `g.sock`, `hooks.json`, and `mcp.json`.
- **`serde_json` `preserve_order`**: Because the workspace enables `serde_json`'s `preserve_order` feature, `canonical_json` in `normalizer.rs` recursively sorts all JSON object keys via `BTreeMap` before serializing so `RetryGrantStore` matches identical tool arguments regardless of key order.
- **`scripts/agy-policy.py` note for M1**: In `scripts/agy-policy.py`, `check_paths()` iterates over all string values in tool args (`strings(args)`), which includes `Content`, `Target`, and `Replacement` on `write_file`/`edit_file` (not just `Path`). If a file's content starts with `/` (such as a `//!` doc comment at byte 0) or contains a parent-directory dot-dot-slash sequence or a slash-dot-agents-slash sequence, the worker policy hook denies the write as a path or `.agents` violation. We did not touch `scripts/agy-policy.py` (out of scope), but worth noting for future worker prompts.

- **Open items I did not change** (see report 3 and 6.4): the helper gives up after 25 s while `agy` is configured to wait 300 s, so an approval that takes longer than 25 s is denied; `ask_question` is answered by the bridge without consulting `SessionConfig::gate`; nothing hands the generated MCP config to `agy` (the report previously claimed an env var did); only `read_file` and `run_command` are verified live as hooked.
- `scripts/agent-run` and other launchers that start `agy` with `--dangerously-skip-permissions` have the same fail-open exposure for their own hook config; a launcher-side read-back is worth adding (out of scope, `scripts/` not touched).

## If blocked
Not blocked.
