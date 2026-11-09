# M1-12-protocol-ts: TS generation, protocol shape tests, consistency checker

Model: Gemini 3.8 Flash · Reviewer: Sonnet 5.5 · Depends on: contract (M1 rev 2)
Branch: `agent/flash/M1-12-protocol-ts` · Worktree: `../atrium-wt/M1-12-protocol-ts`

## Goal
Make the protocol usable from the app and keep it honest: generate TypeScript types from `atrium-protocol`, pin the JSON shape of every rev-3 type, and strengthen the cross-adapter consistency checker so it can gate M1.

## Context
- `docs/contracts/m1.md` §1, §6. PLAN §3.3 ("exported to TS with tauri-specta").
- `crates/atrium-protocol/src/*.rs` (rev 3), `crates/atrium-protocol/tests/serde_shape.rs`.
- `crates/atrium-agents/tests/consistency.rs` and `m1-must-fix.md` X6 ("strengthen it: duplicates, per-closure ordering; it checks goldens, not live enforcement").
- `crates/atrium-agents/src/fixture.rs` (`Sanitizer`).
- The spike UI's TS types for comparison: `spikes/ui-shell/src/`, `spikes/stream-perf/src/`.

## Owned paths
- `crates/atrium-protocol/src/*.rs`: **only** adding `#[cfg_attr(feature = "ts", derive(specta::Type))]` lines (and the matching `specta` attributes where serde attributes need mirroring). No other change to these files.
- `crates/atrium-protocol/Cargo.toml`: the optional `specta` dependency and the `ts` feature.
- `crates/atrium-protocol/tests/**`, `crates/atrium-protocol/bindings/**`, `crates/atrium-protocol/examples/export-ts.rs`
- `crates/atrium-agents/tests/consistency.rs`, and in `crates/atrium-agents/src/fixture.rs` only `Sanitizer::is_clean` and new helper functions
- `crates/atrium-agents/Cargo.toml`: one `[dev-dependencies]` line for `sha2`, with `# M1-12`
- `scripts/check-protocol-ts`
- `docs/agents/handoffs/M1-12-protocol-ts.md`

## Contract to implement
- `cargo run -p atrium-protocol --example export-ts --features ts` writes `crates/atrium-protocol/bindings/protocol.ts`. No spike uses `specta` yet: pick the `specta`/`specta-typescript` versions that `tauri-specta` 2 for Tauri 2 expects, pin them exactly, and say which. Enable `specta`'s `serde_json` feature (for `Value`) and export `u64` as `number` (`BigIntExportBehavior::Number`; Atrium's ids and counters stay below 2^53, say so in a comment).
- `scripts/check-protocol-ts` (bash) regenerates into a temp file and fails if it differs from the committed one.
- `serde_shape.rs`: a round-trip test with the exact expected JSON for every public serde type in `agent`, `policy`, `thread` and `wire`, including every enum variant (rev 2 added `ToolSubject`, `RuleDraft`, `ErrorCode`, `ProviderProbe`, `rememberable`, the attention fields, and `Incompatible`'s new fields).
- Consistency checker: verify `MessageBody::Streamed` `sha256` (add `sha2` as a dev-dependency of `atrium-agents` with `# M1-12`); flag duplicate `ApprovalRequested`/`QuestionAsked` ids and duplicate resolutions; check the closing order per closure episode, not per session; restrict to `fixtures/agents/{claude,codex}` (agy and ACP are M2). The test stays `#[ignore]` until M1-07 and M1-08 merge; the orchestrator un-ignores it.
- `Sanitizer::is_clean` also checks decoded content: JSON-escaped (`\uXXXX`) and Unicode-escaped forms of every secret (A13).

## Definition of Done
- [ ] `scripts/check-protocol-ts` passes; the generated file is committed.
- [ ] Every rev-3 type and variant has a shape test.
- [ ] The checker's new rules each have a unit test with a small bad event list that it catches.
- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] `cargo test -p atrium-protocol --all-features` and `cargo test -p atrium-agents`
- [ ] Handoff with outputs.

## Out of scope
Changing any type, field or doc comment in `atrium-protocol` (change request instead); regenerating adapter fixtures (M1-07, M1-08); the app.

## Escalate if
`specta` can't represent a type as serde serializes it (for example an internally tagged enum with a newtype variant): stop and file a change request naming the type. Two fix rounds without passing.
