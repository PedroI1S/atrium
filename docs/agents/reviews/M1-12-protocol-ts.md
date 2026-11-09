# Review: M1-12-protocol-ts
Reviewer: Sonnet 5.5 (read-only) · 2026-11-09

## Verdict
**approve with changes**: contract safety, TS output, shape tests and the sanitizer are sound; the closing-order rule in the consistency checker is broader than the contract and can false-positive on valid fixtures.

Checked: `src/*.rs` diff is only 57 `cfg_attr(feature = "ts", derive(specta::Type))` lines (one per serde type, all 57 registered in `export-ts.rs`); `fixture.rs` changes stay in `is_clean` plus helpers and one test; `cargo test --workspace --all-features` and clippy pass; regenerating `protocol.ts` is byte-identical to the committed file. The pinned specta rc.26 / tauri-specta rc.26 exist on crates.io; I could not verify tauri-specta's own pins on specta-typescript/serde/util 0.0.13.

## Findings

1. [major] **Closing-order rule is not scoped to a session close and can false-positive**
   Where: crates/atrium-agents/tests/consistency.rs:~237-246 (ToolCallFinished), ~262-266 (MessageCompleted), ~314-319 (RequestResolved).
   Problem: the contract (agent.rs module docs) orders closing only "when a session ends". The checker raises the stage on any `ToolCallFinished(cancelled)` or `RequestResolved(cancelled)` and keeps it until the next `TurnStarted`. A mid-turn cancellation is valid (the `RequestOutcome::Cancelled` doc says the provider may withdraw a request; a single tool can be cancelled), and a later `MessageCompleted` or a second cancelled tool in the same turn is then flagged. The fixtures in the tree are rev-2 and fail to parse, so this was not exercised against real data.
   Fix: only start an episode when it can be a closure: for example, treat the stage as active only after an `Interrupt`-style close, or only check ordering among the events between the last non-cancel event of the turn and `TurnCompleted`/`SessionEnded` (cancelled tools, then cancelled requests, then `TurnCompleted`, with nothing else in between). Re-validate when M1-07/M1-08 regenerate fixtures, before un-ignoring.

2. [minor] **"Turn completed last" is not enforced, and a late request resolution lowers the stage**
   Where: consistency.rs, `RequestResolved` arm (~314-319).
   Problem: `RequestResolved(cancelled)` after `TurnCompleted` is not flagged (only tool/message events are), and it sets the stage back from `TurnCompleted` to `RequestsCancelled`. Contract order is requests, then turn, then `SessionEnded`. The `ToolCallFinished` message also prints event 0 as the "previous" index when the stage came from `TurnCompleted`.
   Fix: flag `RequestResolved(cancelled)` when `closure_stage == TurnCompleted`, use `max` when raising the stage, and add a unit test for it.

3. [minor] **Shape tests don't pin `#[serde(default)]` omission, which the TS encodes as `?`**
   Where: crates/atrium-protocol/tests/serde_shape.rs (all `round_trip` calls use full JSON).
   Problem: `tools`, `truncated_bytes`, `rememberable`, `details`, `Question.{options,multi_select,allow_text}` and `QuestionAnswer.selected` are optional in `protocol.ts` because of `default`, but no test deserializes JSON with them omitted, so a removed `default` would not fail the shape tests (only the TS diff would). Every other public type and all variants are covered with exact JSON (I checked every tag and variant name in the TS against the test file).
   Fix: add `from_value` cases with the fields absent, expecting the defaults.

4. [minor] **`scripts/check-protocol-ts` is not executable**
   Where: scripts/check-protocol-ts (mode 100644; `agent-run`, `doctor`, `handoff` are 755).
   Problem: `./scripts/check-protocol-ts` fails with permission denied. The script itself is correct and portable (bash, `mktemp`, `diff -u`, trap cleanup; it targets the repo root).
   Fix: `chmod +x` (the orchestrator can do this at integration).

5. [minor] **specta-typescript/serde/util are unconditional dev-dependencies**
   Where: crates/atrium-protocol/Cargo.toml, `[dev-dependencies]`.
   Problem: the brief allows "the optional specta dependency and the `ts` feature". The three pinned dev-deps are built for every `cargo test -p atrium-protocol`, even without `ts`, adding compile time. The example is `cfg(feature = "ts")`-gated, so they are only needed with it.
   Fix: keep, or make them optional behind `ts = ["dep:specta", "dep:specta-typescript", ...]` (examples can use optional deps when the feature is on). Orchestrator's call.

6. [nit] **`#[ignore]` reason is stale**
   Where: consistency.rs `consistency_all_fixtures` (~line 952).
   Problem: it points at M0-findings.md. Contract §2 wants `#[ignore = "M1-xx"]` naming the task that un-ignores it; the brief says it stays ignored until M1-07 and M1-08 merge.
   Fix: `#[ignore = "M1-07, M1-08: fixtures regenerate for rev 3"]`.

7. [nit] **u64 to number is documented only in the example, and `Sanitizer` escape decoding has no malformed-input tests**
   Where: examples/export-ts.rs:76 (comment); fixture.rs tests.
   Problem: the u64-below-2^53 note is not in the generated `protocol.ts` header, so TS consumers never see it. `decode_escapes` is panic-free on truncated or invalid escapes by reading (peek-based, no indexing) and handles `\uXXXX` case-insensitively, surrogate pairs, `\u{..}` and repeated escaping up to 4 rounds, but the unit test only covers valid forms.
   Fix: optional: add a comment line in the generated file (via specta header) and a test with inputs like `\u`, `\u12`, `\ud83d`, `\u{110000}`, `\x`.

Contract safety, TS-vs-serde correctness (internally tagged enums, `transparent` ids as `string`/`number`, `Option` as `| null`, `Value` as the JSON union, snake_case renames, `ToolSubject::None`/`Unknown` unit variants as `{ type: "none" }`) and the u64 mapping (ids, ms timestamps and counters are far below 2^53) are correct.
