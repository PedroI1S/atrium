# M1-02-store: SQLite store

Model: Sonnet 5.5 · Reviewer: Gemini 4 Argon · Depends on: contract (M1 rev 3)
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
- `MessageCompleted` handling: keep each open message's chars and delta count in `open_messages` and its running SHA-256 in the writer task's memory (`sha2`'s hasher state isn't serializable); after a restart, recompute it from the message's stored deltas.
- **The store never writes `requests.status`**: the ledger owns it (contract §5). Rows are created by the ledger through `RequestStore`; a stored `RequestResolved` only sets `event_closed = 1`.
- `RequestStore::transition` is a compare-and-set from any of `from`; `open_count` counts pending + approved rows of an incarnation; `current_incarnation` is the thread's latest; `approve_with_rule` inserts the rule and bumps `rules_revision` only if its `pending → approved` succeeded.
- Storing `ApprovalRequested`/`QuestionAsked` sets the row's `event_opened` (and a question's `title` to its first prompt); `open_items().requests` are rows with `event_opened = 1 AND event_closed = 0`. Tests open rows through `requests()` before appending the event, as the gate does.
- `rename` sets the title. `unread` is set by `Assistant`/`Reasoning` messages only, never `User`.
- `append` rejects a batch for a non-current incarnation (`StaleIncarnation`) and one with an event over `MAX_EVENT_BYTES` (`EventTooLarge`), storing nothing.
- Read-only (disk full): new writes fail with `ReadOnly`, **except** `signal`, `set_process(.., None)`, `RequestStore` moves to final statuses and `expire`, and `append` batches made only of closing events (`MessageCompleted`, `ToolCallFinished`, `RequestResolved`, `TurnCompleted`, `Error`, `SessionEnded`).
- Keep `threads.attention_since_ms`/`attention_detail` up to date from events: the oldest pending approval's title, the first question's prompt, a fatal error's message; cleared when nothing needs the user.

## Definition of Done
- [ ] Every test in `crates/atrium-core/tests/store.rs` un-ignored and passing, plus your own tests for: an append of 10k events in one batch; the 3M-token stream from M0-07 (`MessageDelta`s at 10k tok/s for 5 min) appended and closed with peak RSS reported; incarnation 2 restarting `seq` at 0.
- [ ] Disk-full: with `free_space` returning less than the minimum, ordinary writes return `StoreError::ReadOnly` and `storage_status()` says so, while a closing batch, a signal and request expiry still succeed; when it returns more, writes work again with no restart.
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
