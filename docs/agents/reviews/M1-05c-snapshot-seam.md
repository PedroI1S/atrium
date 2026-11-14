# Review: M1-05c-snapshot-seam (commit 656ee94)

Reviewer: Claude Sonnet 5.5, read-only (D-077). Author: Gemini 4 Hydrogen `medium`. Mutations ran in a scratch clone.

## Verdict: approve with changes

The seam, `GatedStore` and the `conn.rs` nits are correct and deterministic. One mutation survives: moving the `running_turns()` check above the lock. So the tests don't pin the check-under-lock half of the rule; document that. Case B also has a latent start-up race.

## 1. Scope: OK
- The src diff is only:
  - the `Options.store_wrapper` field (lib.rs:118-121), with a test-seam doc comment; the `#[allow(clippy::type_complexity)]` is needed;
  - the `None` default (lib.rs:137);
  - the wrap in `Daemon::start` (lib.rs:300-303), applied right after `SqliteStore::open` and before the ledger, `Inner` and recovery use the store;
  - the two `conn.rs` test edits (conn.rs:777, 780-781).
- With `None` the daemon builds the same `Arc<dyn Store>` as before, so its behaviour is unchanged.

## 2. Case A `turn_started_during_snapshot_waits_for_snapshot_to_finish`
- **Deterministic:**
  - `notify_one` stores a permit, so no wake-up is lost;
  - the waits are bounded;
  - the only sleep-like wait is the 200 ms negative check, commented as such (gaps_snapshot.rs:226-227);
  - a run takes about 0.34 s.
- **It proves the appends wait:** `TurnStarted` is not stored while the snapshot is held. After the release, it is stored and a snapshot file exists.
- **[minor] gaps_snapshot.rs:240-250:** nothing asserts directly that the snapshot was made before `TurnStarted`. The ordering follows from the hold and the lock.

## 3. Case B `snapshot_is_not_called_while_a_turn_is_running`
- **It does test something:** with the `running_turns()` check skipped, it fails at gaps_snapshot.rs:294 (3 of 3 runs).
- **"At least two `storage_status()` checks" is a sound signal** that the snapshot loop reached its decision:
  - the storage watch calls it every `STORAGE_CHECK_EVERY` (upkeep.rs:116);
  - a turn start calls it (commands.rs:133);
  - the snapshot loop calls it once per 150 ms tick (upkeep.rs:170).
- **[minor, latent flake] gaps_snapshot.rs:262-272:** the first snapshot tick fires at 150 ms. If the turn isn't marked running yet (a slow CI box), a snapshot is legitimately taken and the test fails. It passed 20 of 20 runs here.
- **[nit] gaps_snapshot.rs:282-284:** the "only checks every 2s" comment isn't tied to the `STORAGE_CHECK_EVERY` constant.

## 4. GatedStore: OK
All 24 `Store` methods are delegated faithfully. Only these add behaviour:
- `append`: notifies after success;
- `storage_status`: counts calls;
- `snapshot`: waits on the gate.

## 5. conn.rs nits: addressed
- The precondition `assert!(received.len() < large.len())` is in (conn.rs:777).
- The comment no longer claims the origin check drops the later frame (conn.rs:780-781).

## 6. Toolchain (scratch clone of 656ee94)
- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test -p atrium-daemon`: all pass. Counts: lib 15, client 1, gaps_full_storage 2, gaps_snapshot 2, recovery 3, sessions 19, socket 9.
- `gaps_snapshot` passed 20 of 20 runs, and `conn::tests` passed 20 of 20.

## Mutation results (3 runs each)

| Mutation in `snapshot_daily` | Case A | Case B | Result |
|---|---|---|---|
| Drop `publish.lock()` | FAIL (gaps_snapshot.rs:233) | ok | caught |
| Move `running_turns()` check above the lock | ok | ok | **NOT caught** |
| Skip `running_turns()` check (`if false`) | ok | FAIL (gaps_snapshot.rs:294) | caught |

## Findings
- **[moderate] The "check above the lock" mutation survives.**
  - Catching it needs a turn marked running between the check and the lock, which needs another seam.
  - The tests pin two things: appends wait while the snapshot is held, and a running turn skips the snapshot. They don't pin that the check is made under the lock.
  - Fix: document it in a test comment and the handoff. A further seam is optional.
- **[minor]** The case B start-up race (§3).
- **[minor]** No direct snapshot-before-`TurnStarted` assertion (§2).

## Asked before merge
1. Document the surviving mutation (in a test comment and the handoff).
2. Make case B robust to a slow start.

Both were sent back to Hydrogen.
