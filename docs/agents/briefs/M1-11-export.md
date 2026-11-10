# M1-11-export: history export, redaction, export files

Model: Claude Fable 5 (the orchestrator, inline; D-055) · Reviewer: Gemini 3.8 Flash (D-056) · Depends on: M1-02 (merged)
Branch: `agent/fable/M1-11-export` · Worktree: `../atrium-wt/M1-11-export`

## Goal
Implement `atrium-core::history`: rebuild and verify messages from their deltas, redact secrets from real sessions, and write one thread as Markdown or JSONL into a private new file. The daemon's `ExportThread` command (M1-05) calls it.

## Context
- `docs/contracts/m1.md` §6.1 (`MessageBody`, user messages, incarnations) and §13.
- `crates/atrium-core/src/history/mod.rs`: the four public items and their doc comments.
- `crates/atrium-protocol/src/agent.rs` (events), `wire.rs` (`StoredEvent`, `ExportFormat`).
- `docs/agents/m1-must-fix.md`: A12 (sanitize reconstructed streams, not single deltas), A13 (escaped and Unicode-escaped secrets), "Credential-safe `Debug`".

## Owned paths
- `crates/atrium-core/src/history/**`
- `crates/atrium-core/tests/history.rs`
- `crates/atrium-core/Cargo.toml`: dependency lines only, each with `# M1-11`
- `docs/agents/handoffs/M1-11-export.md`

## Contract to implement
- `reconstruct_message`: concatenates the deltas and checks the delta count, `chars` (Unicode scalar values) and `sha256`. Any mismatch is `HistoryError::Integrity`; nothing is patched up.
- `Redactor`: the token patterns in its doc comment, extra literal secrets, and the home directory as `~`. It works on decoded content: a secret written with JSON or `\uXXXX` escapes, or inside JSON nested in a string, is still found. Its `Debug` never prints a secret.
- `create_export_file`: new file only, `O_NOFOLLOW`, mode 0600, and the parent folder must be owned by the user.
- `export_thread`: Markdown or JSONL, returning the bytes written. No secret may survive because it was split across message deltas or across streamed tool output: redaction runs on the rebuilt message and on the whole tool output. User messages are exported like the rest. A message that fails verification fails the export.

## Definition of Done
- [ ] The three tests in `crates/atrium-core/tests/history.rs` un-ignored and passing, plus tests for: every token pattern and its near misses, the home-directory boundaries, escaped and nested-JSON secrets, a secret split at every position across deltas, both export formats, a thread exported while a message or tool call is still open, two incarnations reusing a message id, and the file checks (existing file, symlink, folder owned by someone else).
- [ ] No new `unsafe`, no panic on any input (`export_thread` and `Redactor::text` take untrusted agent text).
- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] Handoff with the commands' output and what the redaction does not catch.

## Out of scope
The `ExportThread` command and reading events from the store (M1-05), the export UI (M1-09b), full-text search.

## Escalate if
A public signature in `history/mod.rs` can't express something needed: write a change request and stop that part.
