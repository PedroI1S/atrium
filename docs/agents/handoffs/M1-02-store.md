# Handoff: M1-02-store

Model: Sonnet 5.5 · Status: done

## Done

`atrium-core::store::SqliteStore` implementing `Store` and `RequestStore`, in `crates/atrium-core/src/store/`:

- `mod.rs`: `SqliteStore::open`, `StoreOptions` (plus `impl Default`), the writer thread plumbing, the `Store` impl. `codec.rs`: row/type conversions. `writer.rs`: the one read-write connection, event projections, attention fields, signals, in-memory SHA-256 hashers. `requests.rs`: `RequestStore`. `reader.rs`: pool of `STORE_READERS` read-only connections (2 MiB cache each) and the read queries. `migrate.rs`: numbered migrations. `snapshot.rs`: `VACUUM INTO`.
- Every write is one `IMMEDIATE` transaction on the writer thread; a batch updates the events and every projection (thread row, session, turns, open messages, open tool calls, requests `event_opened`/`event_closed`, question title) together.
- Closing never reads history: `open_items` reads the projection tables. A message's running hash is in writer memory and is recomputed from its stored deltas (SQL `json_extract` over that incarnation's `message_delta` rows) only when it is missing after a restart.
- `requests.status` is written only through `RequestStore`. `approve_with_rule` is one CAS plus rule insert plus revision bump.
- Read-only mode: ordinary writes return `ReadOnly`; allowed while read-only: `signal`, `set_process(.., None)`, `RequestStore` moves to final statuses (`redeemed`, `denied`, `answered`, `cancelled`, `expired`) and `expire`, and `append` batches made only of closing events. A mixed batch is refused whole. `storage_status()` probes free space on every call, so recovery needs no restart.
- Migrations run in the rollback journal before WAL is switched on, so a failed one leaves the file byte-identical.
- Snapshots: empty 0600 file is created first, then `VACUUM INTO` (on a reader connection), dir mode 0700, pruned to `SNAPSHOTS_KEPT`; named `atrium-YYYYMMDD-HHMMSS-mmm.db` (UTC).
- Attention fields: oldest open approval's title, the oldest open question's first prompt, fatal error message; also kept for unverified cleanup and unread replies (the `ThreadSummary` docs list them); cleared when nothing needs the user.
- Tests: `crates/atrium-core/tests/store.rs`, all 14 stubs implemented and un-ignored, plus 9 more (see below). Dependencies added with `# M1-02`: `rusqlite` (bundled), `sha2`, `rustix` (fs, for `statvfs`, which keeps `unsafe_code = deny`), dev `tempfile`.

Additions beyond the contract signatures (all additive): `impl Default for StoreOptions` (its `free_space` default is a sentinel `Arc` that `open` replaces with `statvfs` on the store's folder), `SqliteStore::integrity_errors()`, and a `#[doc(hidden)]` `SqliteStore::open_with(path, options, migrations)` and public `store::Migration`, used only to test a migration that fails halfway.

## Not done

- No change request. `attention::next_state` is still M1-04's stub: the tests use their own small transition function, and nothing here calls the real one except `StoreOptions::default()`.
- `MessageCompleted` mismatches are logged with `eprintln!` (no logging crate in the workspace yet) and counted.

## Definition of Done results

- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test -p atrium-core --test store`: `test result: ok. 23 passed; 0 failed; 0 ignored` (37 s, almost all of it the 3M-event test, in a debug build).
- `cargo test -p atrium-core --lib`: ok, 0 tests (the crate has no unit tests).
- Tests required by the brief: 10k events in one batch (`append_ten_thousand_events_in_one_batch`), the 3M-token stream (`three_million_token_stream_appends_and_closes`), incarnation 2 restarting `seq` at 0 (`incarnations_scope_sequences_and_requests`), disk-full both ways (`low_disk_makes_store_read_only_and_recovers`, `read_only_store_still_accepts_closing_batches_and_signals`), failed migration byte-identical (`failed_migration_rolls_back_and_refuses_to_open`, two cases), snapshot 0600 and pruned (`snapshot_is_private_and_pruned`).

Other tests added: failed batch stores nothing and resets hashes; integrity mismatch counted; restart recovery of the open message hash; stale incarnation; oversized event; `approve_with_rule`; `expire` filters; resume id; threads/settings/live processes/archive.

### Memory of the 3M-token stream

3,000,000 `MessageDelta`s (one message), appended in batches of 10,000 as fast as the store accepts them (not paced at 10k tok/s: 36.6 s in a debug build), then `open_items` and the closing `MessageCompleted` + `TurnCompleted`. The hash and char count match; no integrity error. Measured on macOS, whole test process (includes the test's own batches and the tokio runtime):

| Metric | Value | How |
|---|---|---|
| Peak RSS | 19.3 MB (18.4 MiB) | `/usr/bin/time -l <test binary> three_million --nocapture`, "maximum resident set size" |
| Peak `phys_footprint` | 14.8 MB (14.1 MiB) | same run, "peak memory footprint" |
| Sampled peak RSS / footprint | 18 MiB / 14 MiB | test's own sampler thread: `ps -o rss= -p <pid>` and `footprint -p <pid>` every 250 ms |
| RSS before / after the stream | 6 MiB / 16 MiB | `ps` in the test |

Reproduce: `cargo test -p atrium-core --test store --no-run`, then `/usr/bin/time -l target/debug/deps/store-<hash> three_million --nocapture`.

## Gotchas

- `StoreOptions` has no room for a migrations seam (adding a field would break struct-literal construction), hence `open_with`.
- The ledger's status and the store's `event_closed` are independent: after a stored `RequestResolved`, the row can still be `pending` until the ledger moves it. `open_count` therefore counts it; the ledger (M1-03) must keep them in step through `withdraw`/`expire_*`.
- `pending_approvals`/`pending_questions` count rows whose opening event was stored and closing wasn't. An `ApprovalRequested` with no ledger row (the gate always creates one first) is stored but counts for nothing.
- `attention_since_ms` for approvals and questions is the ledger row's `opened_ms` (the ledger's clock), not the event's `at_ms`.
- `begin_incarnation` and `create_thread` do not apply `Signal::Starting`; the daemon sends it.
- `snapshot` is refused with `ReadOnly` when the disk is low (it writes a file); `VACUUM INTO` of a large database blocks only a reader connection, not the writer.
- On a database where an open message has stored deltas but the process restarted, the first delta or completion for it scans that incarnation's `message_delta` events once (no index on the message id; the schema is a contract). Only after a crash.

## Follow-ups

- M1-05: call `snapshot` daily; on `StorageStatus::ReadOnly` send `Signal::StorageReadOnly` and show the storage-full inbox item.
- M1-03: use `RequestStore::approve_with_rule` for "remember" and read `binding` for `redeem`. Question rows have no binding (`binding` returns `None` for them).
- A logging crate for the integrity error (currently `eprintln!`).

## Review fixes (Argon's M1-02 review, approve with changes)

1. `begin_incarnation` resets `pending_approvals`/`pending_questions` to 0 and recomputes attention in the same transaction (sticky Failed, unverified cleanup and Reply attention stay). `incarnations_scope_sequences_and_requests` now asserts a clean summary after `begin_incarnation` and `pending_approvals == 0`, state `Running`, no attention after incarnation 2 resolves `r1`.
2. The writer queue is a bounded `tokio::sync::mpsc` channel (64 jobs; callers `send().await`, writer thread uses `blocking_recv`). Reads take a `tokio::sync::Semaphore` permit (one per reader connection) before `spawn_blocking`. Test `concurrent_reads_and_writes_complete`.
3. The database is opened with `O_NOFOLLOW` on its canonical path; regular file and owner checked on the open handle; an existing file is `fchmod`ed to 0600; symlinked `-wal`/`-shm`/`-journal` are refused (`StoreError::Io`, PermissionDenied). Snapshot folder: `DirBuilder` mode 0700, `symlink_metadata` must be a real directory owned by the user (looser mode tightened through an `O_NOFOLLOW|O_DIRECTORY` handle), snapshot files opened with `O_NOFOLLOW`. Test `symlinks_are_refused` (db symlink, dangling symlink, wal symlink, loose db file, symlinked snapshot folder with its target untouched, loose snapshot folder). `rustix` now also uses its `process` feature (`getuid`).
4. `rules()` reads revision and rules in one read transaction.
5. After a caught writer panic the writer clears its hashers and rolls back if not in autocommit. Test `writer_survives_a_panic_and_rebuilds_hashes` (a transition function that panics mid-batch).
6. `ApprovalRequested` only sets `event_opened`; it never writes `requests.title`. `attention_detail` still falls back to the tool name when computed. `store_never_changes_request_status` covers a `Binding` with `title: None` round-tripping.
7. `events_after` converts with `i64::try_from(..).unwrap_or(i64::MAX)`; `limit == 0` returns empty. Test `events_after_handles_extreme_arguments`.
8. `statvfs_free` fails closed (0); the data dir is canonicalized at open. New test `usage_enforcement_and_two_approvals`: `Usage` and `TurnCompleted.usage`, `EnforcementChanged` and `set_enforcement`, two open approvals (oldest wins, then the next after resolving), and `approve_with_rule` and `snapshot` refused with `ReadOnly` (neither is cleanup; moves to final statuses stay allowed, checked in the same test).
9. `#[doc(hidden)]` on `pub use migrate::Migration`. `Cargo.lock` changes kept, as instructed.

Results: `cargo fmt --all --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test -p atrium-core --test store`: 28 passed, 0 failed, 0 ignored (37 s); `cargo test -p atrium-core --lib`: ok (0 tests).
