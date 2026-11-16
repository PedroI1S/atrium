# M1-25-export-damaged: export a thread even when one message is damaged (CR-16)

Model: Gemini 4 Hydrogen (`gemini-4-hydrogen-high`, `scripts/agent-run agy`; D-111) · Reviewer: Gemini 4 Argon `high` (read-only; Pedro, D-112) · Depends on: nothing open
Go: D-111, CR-16 (Pedro) · Branch: `agent/hydrogen/M1-25-export-damaged` · Worktree: `../atrium-wt/M1-25-export-damaged`

## Goal
Today one message that fails verification fails the whole `ExportThread` (`handoffs/M1-11-export.md`; contract §6.1: "a hard error at reconstruction, never patched"). CR-16 changes export only: the thread is exported, the damaged message is written as a clearly marked placeholder, and the result says how many were damaged. Display keeps §6.1's hard error.

## Context
- `docs/agents/change-requests.md` CR-16; contract `docs/contracts/m1.md` §6.1 and §13.
- `crates/atrium-core/src/history/export.rs` and `history/mod.rs` (`reconstruct_message`), `crates/atrium-core/tests/history.rs`.
- `crates/atrium-daemon/src/commands.rs` (`export`, `ExportThread` returns `{"bytes": u64}`), `crates/atrium-protocol/src/wire.rs` (`ExportThread`'s doc).
- `handoffs/M1-11-export.md`, `reviews/M1-11*.md`.

## Owned paths
- `crates/atrium-core/src/history/**`, `crates/atrium-core/tests/history.rs`.
- `crates/atrium-daemon/src/commands.rs` (`export` only), its tests.
- `crates/atrium-protocol/src/wire.rs` (`ExportThread`'s doc comment only), the regenerated TS bindings if a type changes.
- Contract `docs/contracts/m1.md` §6.1 and §13: write the CR-16 wording.
- `docs/agents/handoffs/M1-25-export-damaged.md`.
- Not yours: `atrium-core/src/policy/**` (another task is changing it), the app UI (another task is building the export action; report the new result field for it).

## What to do
1. In export, a message whose reconstruction fails is written as a placeholder:
   - Markdown: a visible note, e.g. "⚠ Damaged message (id …, events …–…): left out of the export."
   - JSONL: one line marked damaged, with the message id, the event range and the reason, and no patched or partial content.
   - Redaction still runs on everything else. Never write the damaged message's raw pieces.
2. `ExportThread`'s value becomes `{"bytes": u64, "damaged": u32}` (additive). Document it in `wire.rs` and contract §13.
3. Display and history reconstruction outside export keep the hard error.
4. Tests: a thread with one damaged message exports in both formats, with the marker and `damaged: 1`, and the other messages intact; a test proving no fragment of the damaged message reaches the file; the existing export tests unchanged. Show the new tests fail on `dev`.

## Definition of Done
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast`; if bindings changed, `scripts/check-protocol-ts` (or the project's equivalent) and `pnpm typecheck` in `app/`.
- [ ] Handoff: the formats, the result field, the contract wording, the outputs.

## Rules
- Work only in your worktree; commit on your branch only. Never push, merge or rebase. Never run a real provider.
