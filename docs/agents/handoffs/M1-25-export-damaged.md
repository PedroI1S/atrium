# Handoff: M1-25-export-damaged (CR-16)

Model: Gemini 4 Hydrogen (high) · Branch: `agent/hydrogen/M1-25-export-damaged` · Status: done

## Done

Implemented CR-16 (D-111): `ExportThread` and `atrium-core::history` now export threads containing messages that fail verification by writing a clearly marked placeholder instead of failing the whole export, while keeping `reconstruct_message` (and display reconstruction) as a hard `HistoryError::Integrity` error.

- `crates/atrium-core/src/history/mod.rs`:
  - Kept `reconstruct_message` and `HistoryError::Integrity` unchanged for display/non-export callers.
  - Extracted `verify_message_reason` returning static failure reasons (`"inline body after deltas"`, `"delta count differs"`, `"chars differ"`, `"sha256 differs"`) without leaking message content.
  - Re-exported `ExportReport` and `export_thread_report`.
- `crates/atrium-core/src/history/export.rs`:
  - Added `ExportReport { pub bytes: u64, pub damaged: u32 }` and `export_thread_report(...) -> Result<ExportReport, HistoryError>`.
  - Kept `export_thread(...) -> Result<u64, HistoryError>` (the `crates/atrium-core/src/lib.rs` contract signature) delegating to `export_thread_report` so existing callers and export tests stay intact.
  - Tracked `first_id: EventId` on `OpenMessage` so each damaged message reports its inclusive event range (`first_id`..=`completed_id`, or `completed_id` alone when 0 deltas were stored).
  - On verification failure, `open.text` and any `Inline` completion text are dropped immediately and never passed to emitters, writers, or redaction.
- `crates/atrium-daemon/src/commands.rs` (`write_export`, `export`):
  - Switched `write_export` to `history::export_thread_report`.
  - Updated `ExportThread` response to `{"bytes": report.bytes, "damaged": report.damaged}`.
- `crates/atrium-protocol/src/wire.rs` and `crates/atrium-protocol/bindings/protocol.ts`:
  - Documented `Value: {"bytes": u64, "damaged": u32}` on `DaemonCommand::ExportThread` and regenerated `protocol.ts`.
- `docs/contracts/m1.md` §6.1 and §13:
  - Added CR-16 wording.
- Tests (`crates/atrium-core/tests/history.rs`, `crates/atrium-daemon/tests/sessions.rs`):
  - Verified the new tests failed on `dev` before the fix (`Integrity { message_id: "m2" }`, `Integrity { message_id: "d1" }`, and `ErrorCode::Internal`).
  - Replaced the obsolete `export_fails_on_a_message_that_does_not_match_its_completion` test with `damaged_message_fails_reconstruct_but_exports_as_placeholder`, added `export_writes_placeholder_for_damaged_message_and_keeps_other_messages` and `export_never_writes_any_fragment_of_a_damaged_message` in `history.rs`, and added `export_thread_with_damaged_message_succeeds_with_damaged_count` plus updated damaged-thread assertions in `sessions.rs`. All 14 existing tests under `// Export` and `// Export files` in `history.rs` are unchanged.

## Placeholder formats

### Markdown

Written through `Block` (so `thread_id` and `message_id` still pass through `Redactor`, and `message_id` is formatted as inline code):

- Multiple events (`first_id != last_id`):
  `⚠ Damaged message (id `<message_id>`, events <first_id>–<last_id>): left out of the export (<reason>).`
- Single event (`first_id == last_id`, e.g. `Streamed` completion with 0 deltas):
  `⚠ Damaged message (id `<message_id>`, event <first_id>): left out of the export (<reason>).`

### JSONL

One JSON line, passed through `redactor.value(&mut value)` before serializing, with no `envelope`, `text`, `body`, or `deltas` fields:

```json
{"damaged":true,"type":"damaged_message","id":8,"thread_id":"t-1","incarnation":1,"seq":8,"at_ms":1762696209000,"message_id":"m2","kind":"assistant","events":{"first":6,"last":8},"reason":"sha256 differs"}
```

## Result field (for UI / M1-23)

`DaemonCommand::ExportThread` returns:

```json
{ "bytes": 1234, "damaged": 0 }
```

- `bytes`: `u64` — total bytes written to the export file (including placeholder lines/notes).
- `damaged`: `u32` — count of damaged messages replaced with placeholders (`0` for a clean export). Additive over `{"bytes": u64}`; `PROTOCOL_VERSION` is unchanged.

## Contract wording (`docs/contracts/m1.md`)

- **§6.1**: "A `chars`/`sha256` mismatch is a hard error at reconstruction for display, never patched; export replaces a damaged message with a marked placeholder (§13, CR-16)."
- **§13**: "Export (M1-11, amended by CR-16): one thread to Markdown or JSONL. Messages are reconstructed from deltas and verified (§6.1); a message that fails verification is written as a marked placeholder (Markdown: a visible note with its id and event range; JSONL: an entry marked damaged with the message id, event range and reason), never patched or partial content, while the rest of the thread still exports. `ExportThread` returns `{"bytes": u64, "damaged": u32}`."

## Failure on `dev` (before implementation)

- `cargo test -p atrium-core --test history export_writes_placeholder -- --nocapture`: failed (`panicked at crates/atrium-core/tests/history.rs:222:33: exports: Integrity { message_id: "m2" }`).
- `cargo test -p atrium-core --test history export_never_writes_any_fragment_of_a_damaged_message -- --nocapture`: failed (`panicked at crates/atrium-core/tests/history.rs:222:33: exports: Integrity { message_id: "d1" }`).
- `cargo test -p atrium-daemon --test sessions export_thread_with_damaged_message_succeeds_with_damaged_count -- --nocapture`: failed (`ExportThread ... failed: (Internal, "message m2 doesn't match its completion")`).

## Definition of Done outputs

- `cargo fmt --all --check`: exited 0 (clean).
- `cargo clippy --workspace --all-targets -- -D warnings`: exited 0 (clean).
- `cargo test -p atrium-core --test history`: 27 passed, 0 failed, 0 ignored.
- `cargo test -p atrium-daemon --test sessions`: 23 passed, 0 failed, 0 ignored.
- `cargo test --workspace --no-fail-fast --quiet`: exited 0 (all test targets passed, 0 failed; pre-existing ignored tests: 1 in `atrium-agents` consistency, 2 in `atrium-core` adversarial, 3 in `atrium-core` adversarial, 1 in `atrium-daemon` server).
- `scripts/check-protocol-ts`: exited 0 (`crates/atrium-protocol/bindings/protocol.ts is up to date.`).
- `pnpm --dir app typecheck`: exited 0 (`tsc --noEmit` clean).
