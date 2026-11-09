# M1-02-store: SQLite store

Model: Sonnet 5.5 · Reviewer: Gemini 4 Argon · Depends on: contract (M1 rev 1)
Branch: `agent/sonnet/M1-02-store` · Worktree: `../atrium-wt/M1-02-store`

## Goal
Implement `atrium-core`'s store: the daemon's only persistence. Every event is appended together with the projections it touches, in one transaction; closing a thread never reads its whole history; the store survives a full disk and a failed migration.

## Context
- `docs/contracts/m1.md` §5 (ledger persistence), §6.1 (incarnations, `MessageBody`), §7 (the store), §13.
- `crates/atrium-core/src/lib.rs`: `Store`, `RequestStore`, `Signal`, `OpenItems`, `StorageStatus`, `StoreError`.
- `crates/atrium-core/schema/0001_init.sql`: the schema (contract).
- `crates/atrium-protocol/src/limits.rs`: `MAX_EVENT_BYTES`, `MIN_FREE_DISK_BYTES`, `SQLITE_CACHE_KIB`, `STORE_READERS`, `SNAPSHOTS_KEPT`.
- Prior art: `spikes/daemon-socket/src/store.rs` (M0-05). Its `close_thread` folds the whole history (`daemon.rs:325`), which is what this task must not do.
- `docs/agents/m1-must-fix.md`: X4, L8, "Lifecycle projections", "Resource exhaustion", CR-03.

## Owned paths
- `crates/atrium-core/src/store/**`
- `crates/atrium-core/tests/store.rs`
- `crates/atrium-core/Cargo.toml`: add dependency lines only, each with `# M1-02` (e.g. `rusqlite` with `bundled`, `sha2`)
- `docs/agents/handoffs/M1-02-store.md`

## Contract to implement
- `store::SqliteStore` implementing `Store` and `RequestStore`, with `SqliteStore::open(path, StoreOptions)` as declared in `store/mod.rs`.
- The thread state is computed with `StoreOptions::transition` on every append and signal. Production passes `attention::next_state` (M1-04, not merged yet). Your tests pass their own small transition function.
- `MessageCompleted` handling: keep each open message's chars, delta count and an incremental SHA-256 in `open_messages`, so a `Streamed` completion can be checked without reading the deltas back.
- `requests` rows change in the same transaction as the event that opened or closed them: `ApprovalRequested`/`QuestionAsked` don't create rows (the ledger did, through `RequestStore`), but `RequestResolved` moves a row to its final status.

## Definition of Done
- [ ] Every test in `crates/atrium-core/tests/store.rs` un-ignored and passing, plus your own tests for: an append of 10k events in one batch; the 3M-token stream from M0-07 (`MessageDelta`s at 10k tok/s for 5 min) appended and closed with peak RSS reported; incarnation 2 restarting `seq` at 0.
- [ ] Disk-full: with `free_space` returning less than the minimum, writes return `StoreError::ReadOnly` and `storage_status()` says so; when it returns more, writes work again with no restart.
- [ ] A migration that fails halfway leaves the file byte-identical and `open` returns `StoreError::Migration`.
- [ ] Snapshot files are mode 0600 and pruned to `SNAPSHOTS_KEPT`.
- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test -p atrium-core --test store` and `cargo test -p atrium-core --lib`
- [ ] Handoff with the commands' output, the RSS numbers (macOS `phys_footprint` and RSS) and how you measured them.

## Out of scope
The ledger's logic (M1-03), the state machine (M1-04), export (M1-11), the daemon (M1-05), FTS.

## Escalate if
The `Store`/`RequestStore` traits or the schema can't express something you need: append a change request to `docs/agents/change-requests.md` and stop that part. Two fix rounds without passing.
