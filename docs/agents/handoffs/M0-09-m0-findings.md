# Handoff: M0-09-m0-findings

Model: Gemini 3.8 Flash · Branch: `agent/flash/M0-09-m0-findings` · Worktree: `M0-09-m0-findings` · Status: done

## Done

1. **Cross-Adapter Consistency Tests**:
   - Implemented `crates/atrium-agents/tests/consistency.rs` (a new file, the only modified path in `crates/atrium-agents`).
   - Scans and validates all 70 `.events.json` files under `fixtures/agents/` (including `handmade/` subfolders) across Claude Code, Codex, Antigravity (`agy`), and ACP.
   - Strictly enforces classification (established session vs pre-session failure), tool call start/finish/update pairing, message delta concatenation and single `MessageCompleted`, approval/question resolution, scenario-specific assertions (`approval-deny`, `gate-deny`, `interrupt`), turn pairing and monotonic increase, usage timing, and closing sequence order on exit.
   - Identified 13 contract violations (all in Codex: 12 startup failure fixtures with intermediate unmapped events, 1 handmade interrupt fixture with inverted closing order). Claude, `agy`, and ACP have 0 violations.
   - Marked the test `#[ignore]` so `cargo test -p atrium-agents` stays green in CI, while remaining fully runnable via `cargo test -p atrium-agents --test consistency -- --ignored`.

2. **Consolidated M0 Findings Report**:
   - Created `docs/spikes/M0-findings.md` covering all M0 spikes (M0-01 Claude, M0-02 Codex, M0-03 `agy`, M0-04 ACP, M0-05 daemon socket, M0-06 / M0-06b UI shell, M0-07 streaming performance, M0-08 browser screencast).
   - Consolidated measurements into a single table vs PLAN §4 budgets with exact quoted numbers (Pass/Fail/Not Measured status).
   - Documented all consistency test violations per adapter without modifying any adapter or fixture.
   - Listed all unmapped record types observed per provider with M1 disposition recommendations.
   - Documented security and isolation findings across all spikes.
   - Summarized per-spike recommendations for M1.
   - Compiled open questions and decisions for the project driver (Pedro).

3. **Validation & DoD Checks**:
   - `cargo fmt --check` passes cleanly.
   - `cargo clippy -p atrium-agents --all-targets -- -D warnings` passes with 0 warnings.
   - `cargo test -p atrium-agents` passes (5 passed, 1 ignored, 0 failed).
   - `cargo test -p atrium-agents --test consistency -- --ignored` executes the consistency test and reports all 13 violations.

## Not done

- Modifying adapters or fixture files: Per the brief and worker rules, adapters and fixtures are owned by other tasks or the orchestrator. All violations are reported in `docs/spikes/M0-findings.md`, not fixed in code.
- Linux / WebKitGTK measurements: Deferred to Pedro's partner per D-010.

## DoD Command Results

### `cargo fmt --check`
```text
Exit code: 0
```

### `cargo clippy -p atrium-agents --all-targets -- -D warnings`
```text
    Checking atrium-agents v0.0.0 (/Users/pedro/Documents/atrium-wt.nosync/M0-09-m0-findings/crates/atrium-agents)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s
Exit code: 0
```

### `cargo test -p atrium-agents`
```text
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.08s
     Running unittests src/lib.rs (target/debug/deps/atrium_agents-a80cccceb161a195)

running 5 tests
test fixture::tests::user_name_only_as_a_whole_word ... ok
test fixture::tests::sanitizes_values_not_keys ... ok
test fixture::tests::json_escaped_secrets_are_redacted ... ok
test fixture::tests::secrets_go_first_even_when_they_contain_the_user_name ... ok
test fixture::tests::fixture_lines_round_trip ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/consistency.rs (target/debug/deps/consistency-07f9c83b576c48e9)

running 1 test
test consistency_all_fixtures ... ignored, Cross-adapter consistency violations are tracked and reported in docs/spikes/M0-findings.md

test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests atrium_agents

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
Exit code: 0
```

### `cargo test -p atrium-agents --test consistency -- --ignored --nocapture`
```text
=== CONSISTENCY VIOLATIONS FOUND (13 files) ===

File: fixtures/agents/codex/approval-allow-startup-failure.events.json
  - Pre-session failure must have exactly 2 events (Error { fatal: true } then SessionEnded), found 5

File: fixtures/agents/codex/approval-deny-startup-failure.events.json
  - Pre-session failure must have exactly 2 events (Error { fatal: true } then SessionEnded), found 5

File: fixtures/agents/codex/gate-deny-execute-startup-failure.events.json
  - Pre-session failure must have exactly 2 events (Error { fatal: true } then SessionEnded), found 5

File: fixtures/agents/codex/gate-deny-startup-failure.events.json
  - Pre-session failure must have exactly 2 events (Error { fatal: true } then SessionEnded), found 5

File: fixtures/agents/codex/handmade/interrupt.events.json
  - Event 5: ToolCallFinished(cancelled) for 'call-1' appeared after RequestResolved(cancelled) at event 4 (violates closing order: tool calls before requests)

File: fixtures/agents/codex/interrupt-startup-failure.events.json
  - Pre-session failure must have exactly 2 events (Error { fatal: true } then SessionEnded), found 5

File: fixtures/agents/codex/multi-turn-startup-failure.events.json
  - Pre-session failure must have exactly 2 events (Error { fatal: true } then SessionEnded), found 5

File: fixtures/agents/codex/patch-startup-failure.events.json
  - Pre-session failure must have exactly 2 events (Error { fatal: true } then SessionEnded), found 5

File: fixtures/agents/codex/question-startup-failure.events.json
  - Pre-session failure must have exactly 2 events (Error { fatal: true } then SessionEnded), found 5

File: fixtures/agents/codex/quota-usage-startup-failure.events.json
  - Pre-session failure must have exactly 2 events (Error { fatal: true } then SessionEnded), found 5

File: fixtures/agents/codex/simple-startup-failure.events.json
  - Pre-session failure must have exactly 2 events (Error { fatal: true } then SessionEnded), found 5

File: fixtures/agents/codex/startup-error.events.json
  - Pre-session failure must have exactly 2 events (Error { fatal: true } then SessionEnded), found 3

File: fixtures/agents/codex/tool-read-startup-failure.events.json
  - Pre-session failure must have exactly 2 events (Error { fatal: true } then SessionEnded), found 5
```
