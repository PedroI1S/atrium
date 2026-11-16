# M1-31-credential-folder-search: handoff

**State: ready for verification (D-123).** S8 was implemented in `6075ebb`; this round completed the added scope **K10**, audited and trimmed S8's `deny.rs` diff, and added K10 tests. Per **D-123**, this worker ran only `cargo fmt --all` and `cargo check -p atrium-core --tests` (no `cargo test` or `cargo clippy`).

---

## What changed and where

### Prior round (`6075ebb`, S8)
- `crates/atrium-core/src/policy/deny.rs` (base lines ~1229–1496, strictly inside `Ctx::check`..`Ctx::search_glob_hit` before `GlobEscape`):
  - `Ctx::check`: added recursive `ToolSubject::Paths` check via `dir_contains_hard_deny`, and per-unit `recursive_search_roots` -> `check_search_roots`.
  - `Ctx::dir_contains_hard_deny`, `Ctx::check_search_roots`, `SearchRootSpec`, `SearchOpts`, `resolve_long_opt`, `parse_search_opts`, `strip_git_pathspec_magic`, `git_grep_roots`, `recursive_search_roots`.
- `crates/atrium-core/tests/adversarial_policy.rs`: removed `#[ignore]` on `finding_5_a_recursive_search_of_a_folder_with_a_credential_file_is_blocked`.
- `crates/atrium-core/tests/M1-31-credential-folder-search.rs`: added S8 tests (494 lines).

### This round (`ae7f7c0`, S8 audit/trim + K10)
- `crates/atrium-core/src/policy/deny.rs`:
  1. **S8 audit and trim** (still strictly inside base lines ~1229–1496, `check`..`search_glob_hit`, −60 lines net in that block):
     - Reused `super::engine::RG_VAL_L` and `super::engine::GREP_BOOL_L` via `extra_val_l` / `extra_bool_l` on `SearchOpts` (keeping hard-deny's extra flags `--file`, `--ignore-file`, `--help`, etc., without touching `engine.rs` and without failing open on unknown flags as `engine::search_roots` does).
     - Extracted `fold(s: &str)` shared by `eq_folded`, `recursive_search_roots`, and `search_variants_from_cands`.
     - Deduplicated the three repeated `val_eq` / next-arg consumption blocks in `parse_search_opts` into `take_long_val`.
  2. **K10: bounded glob normalizer** (base lines ~1572–2039: `apply_dotdot`, `normalize_glob_variants`, and helpers):
     - Replaced `Vec<Vec<&str>>` state cloning and prefix-scanning `sort_unstable`/`dedup` in `apply_dotdot` / `normalize_glob_variants` with `NormArena<'a>` (a parent-pointer reverse trie with `(parent_id, seg) -> node_id` interning, so push/pop/dedup are `O(1)` per state transition regardless of depth).
     - Added `MAX_GLOB_WORK = 100_000` threaded across `search_variants_from_cands` and `escape_from_cands`; when spent, `normalize_glob_variants` returns `None`, setting `unbounded = true` and failing closed to `Ask`.
     - Fixed `O(N^2)` unclosed-`[` scanning in `widen_pattern_dots` and `widen_pattern_slashes` via `s.rfind(']')`.
     - Added `glob.len() > MAX_BYTES` fail-closed guard (`unknown = true`, `unbounded = true` -> `Ask`) in `expand_glob_candidates`.
- `crates/atrium-core/tests/M1-31-credential-folder-search.rs`: added K10 tests (`eval_search_presets`, `k10_400_star_dotdot_globs_stay_under_50ms_and_fail_closed_to_ask`, `k10_bounded_star_dotdot_globs_preserve_verdicts_and_stay_fast`).

### Owned-path check (`git diff --stat dev...HEAD`)
Touches only:
- `crates/atrium-core/src/policy/deny.rs` (hunks only at base lines 1229–1496 and 1572–2039)
- `crates/atrium-core/tests/M1-31-credential-folder-search.rs`
- `crates/atrium-core/tests/adversarial_policy.rs` (`#[ignore]` removal only)
- `docs/agents/handoffs/M1-31-credential-folder-search.md`

---

## Tests added in this round and what each pins

In `crates/atrium-core/tests/M1-31-credential-folder-search.rs`:

1. **`k10_400_star_dotdot_globs_stay_under_50ms_and_fail_closed_to_ask`**:
   - Pins that `*/` repeated 400, 500, 2,000, and 20,000 times followed by as many `..` (with or without trailing `/`, and with `/.ssh/id_rsa`) fail closed to `Ask` across `Home`, `Home + Any(Search)`, and `FullAccess`, and evaluate in `< 150 ms` across all 3 presets (`< 50 ms` per evaluation).
   - Pins that `**/` × 500 + `../` × 500 fails closed to `Ask` in `< 150 ms`.
   - Pins that `*/` × 200 + `../` × 200 + `a/` × 20,000 and `{1..64}{a,b,c,d}/` + `a/` × 400 (which stay within `MAX_GLOB_VARIANTS = 256` per call but exceed `MAX_GLOB_WORK = 100_000`) fail closed to `Ask` in `< 150 ms`.
   - Pins that `>` `MAX_BYTES` (`128 * 1024 + 1` bytes) fails closed to `Ask` in `< 150 ms`.
   - Pins that 100,000 unclosed `[` brackets keep `dev`'s verdict (`Ask` in `Home`, `Allow` in `Home + Any(Search)` and `FullAccess`) in `< 150 ms`.

2. **`k10_bounded_star_dotdot_globs_preserve_verdicts_and_stay_fast`**:
   - Pins that globs within both `MAX_GLOB_VARIANTS` and `MAX_GLOB_WORK` keep `dev`'s exact verdicts and run fast:
     - `100 */ + 100 ../` on `~/proj` -> `Allow` (`< 150 ms` for 3 presets)
     - `200 */ + 200 ../` on `~/proj` -> `Allow` (`< 500 ms` for 3 presets; was ~2,000 ms on `dev`)
     - `255 */ + 255 ../` on `~/proj` -> `Allow` (exact `MAX_GLOB_VARIANTS = 256` boundary)
     - `256 */ + 256 ../` on `~/proj` -> `Ask` (`257 > 256`)
     - `200 */ + 200 ../ + .ssh/id_rsa` on `~` -> `Hard(credential-read)`
     - `100 */ + 100 ../ + .cargo/credentials.toml` on `~` -> `Ask / Allow / Allow` (CR-12)
     - `100 */ + 101 ../ + *.rs` on `~/proj` -> `Ask / Allow / Allow` (escapes root)

---

## Expected verdicts (`dev` vs branch) and tests expected to fail on `dev`

*(Not executed by this worker per D-123; verifier will run and record actual outputs.)*

| Test / Input | Expected on `dev` | Expected now |
|---|---|---|
| `adversarial_policy::finding_5_a_recursive_search_of_a_folder_with_a_credential_file_is_blocked` | **Fail** (Ask instead of HardDeny) | **Pass** (`Hard(credential-read)`) |
| `recursive_search_tools_block_every_hard_denied_folder_and_ancestors` | **Fail** (Ask/Allow instead of HardDeny) | **Pass** (`Hard(credential-read)` / `Hard(atrium-self)`) |
| `special_spellings_wrappers_brace_expansion_and_implicit_cwd` | **Fail** (Ask/Allow instead of HardDeny) | **Pass** (`Hard(credential-read)`) |
| `paths_subject_recursive_blocks_hard_denied_folders_and_keeps_non_recursive` | **Fail** on recursive `Grep` | **Pass** |
| `gate_blocks_symlinks_into_credential_folders` | **Fail** on `rg`/`grep -r`/`find`/… through symlink | **Pass** (`GateDecision::Deny`) |
| `cr12_stores_keep_asking_in_home` | Pass | **Pass** (`Ask / Allow / Allow / Allow` for cmds; `Ask / Ask / Allow / Allow` for `Paths`) |
| `non_recursive_reads_and_ordinary_searches_keep_devs_verdict` | Pass | **Pass** |
| `k10_400_star_dotdot_globs_stay_under_50ms_and_fail_closed_to_ask` | **Fail** (400/500/2000/20000 `*/+..`, 100k `[`, `>128 KiB`, work-budget cases exceed 150 ms / take seconds or fail) | **Pass** (`Ask` in `< 150 ms`; `100k [` `Ask/Allow/Allow` in `< 150 ms`) |
| `k10_bounded_star_dotdot_globs_preserve_verdicts_and_stay_fast` | **Fail** on time bound (`200 */ + 200 ..` took ~2,000 ms on `dev`) | **Pass** (same verdicts, `< 500 ms` / `< 150 ms`) |

---

## Checks run by this worker (D-123)

- `cargo fmt --all` — passed (exit 0).
- `cargo check -p atrium-core --tests` — passed cleanly in 0.61 s with 0 warnings.

## Commands for the verification task

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
cargo test -p atrium-core --lib glob_norm_tests
cargo test -p atrium-core --test adversarial_fuzz
cargo test -p atrium-core --test policy
cargo test -p atrium-core --test policy
cargo test -p atrium-core --test policy
cargo test -p atrium-core --test M1-31-credential-folder-search
cargo test -p atrium-core --test adversarial_policy finding_5_a_recursive_search_of_a_folder_with_a_credential_file_is_blocked
cargo test -p atrium-core --test adversarial_regression round_5_blocker_1_searches_and_walkers_reaching_a_folder_linked_into_ssh
git diff dev...HEAD --stat
```

To verify the new/un-ignored tests fail on `dev`'s `src` files (from this worktree, then restore):
```bash
git checkout dev -- crates/atrium-core/src/policy/deny.rs
cargo test -p atrium-core --test M1-31-credential-folder-search -- --nocapture
cargo test -p atrium-core --test adversarial_policy finding_5_a_recursive_search_of_a_folder_with_a_credential_file_is_blocked
git checkout HEAD -- crates/atrium-core/src/policy/deny.rs
```

---

## Rework round 1 (`bfda00d`, `681f7e0`, `bce1a5a`)

### Root cause of the K10 timeouts
1. **Quadratic `..` run stepping in `normalize_glob_variants`:** `..`-terminated states remained in `states` and were re-stepped + HashMap-interned on every subsequent `..` (`1 + 2 + … + k`), so `400 */ + 400 ..` did ~33k HashMap ops per normalizer call before `cap = 256` tripped (193 ms in debug mode).
2. **Redundant `GlobBuilder` compiles on `..` variants:** `100 */ + 100 ..` returned 202 variants (200 containing literal `..` segments), and both `search_glob_hit` and `ask_search_glob_hit` compiled `GlobBuilder` regexes for all 202 variants across 3 presets (~1,212 wildcard regex builds, 383 ms in debug mode).

### Changes
- `crates/atrium-core/src/policy/deny.rs` (`bfda00d`):
  - Replaced `apply_dotdot` with `apply_dotdot_run` + `apply_one_dotdot` and run-length `NormNode` (`seg_id: u32`, `count: usize`, `intern_seg` fast cache) in `NormArena`: consecutive `..` segments (ignoring `.`/empty) are grouped into a single `run`, `..`-ending tails are recorded once as `(parent, count)` without re-stepping, fixed-point runs (e.g., `/h/u/proj/**/ + 500 ..`) exit early, and `MAX_NORM_OUTPUT_BYTES = 512 * 1024` bounds string reconstruction. Cost on `*/ * n + .. * n` is now strictly linear (`O(n)` up to `cap = 256`).
  - Filtered redundant `..`-containing variants in `search_glob_hit` (when no probe contains `..`) and in `search_variants_from_cands` (when `!home_has_dotdot`, syntax-safe without `[`/`{`/`\`, and either no `ASK_TREES` under `roots` or the prefix before `..` exceeds `max_tree_slashes + 2`), while still counting all normalized variants against `MAX_GLOB_VARIANTS`.
  - Added `#[cfg(test)] mod glob_norm_tests::k10_normalizer_work_is_linear_and_bounded` inside `deny.rs` pinning the deterministic `work` step counter (`w200 <= w100 * 2`, `O(n)` up to `cap`, bounded `500 **/ + 500 ..`) and filtered `search_variants_from_cands` count (`<= 4`).
- `crates/atrium-core/tests/M1-31-credential-folder-search.rs` (`681f7e0`):
  - Widened debug-build timing bounds in both K10 tests to `< 750 ms` (`< 250 ms` per evaluate across the 3 presets), added timing bound on `255 */ + 255 ..`, and added linear-scaling check (`d200 <= d100 * 2.5 + 20 ms`).
- `crates/atrium-core/tests/adversarial_regression.rs` (`bce1a5a`, **N9**):
  - In `round_5_blocker_1_searches_and_walkers_reaching_a_folder_linked_into_ssh`, replaced `"cd /tmp && rg x"` and `"find /tmp -name x"` with commands targeting a fresh empty temp folder (`Tmp::new()` with `.home` removed) outside the test fixture.

### Not done (D-123)
- `cargo test` and `cargo clippy` were not run by this worker per D-123; left for the verification task.

---

## Rework round 2 (`2a9be1d`)

### Root cause of the two K10 failures
Both `deny.rs:2744` (`glob_norm_tests::k10_normalizer_work_is_linear_and_bounded`) and `M1-31-credential-folder-search.rs:561` (`500 **/ + 500 ../`) came from the same early-break bug in `apply_dotdot_run`:
- On `500 **/ + 500 ../` (`abs = false`), each step added two `(0, rem)` entries to `dotdot_tails`. At `step_idx = 252`, `dotdot_tails` held `254` items (`2 + 254 = 256 <= 256`), so dedup did not run (`tails_before` at `step_idx = 253` was `254`, including a duplicate). At `step_idx = 253`, pushing two more items triggered `dotdot_tails.dedup()`, shrinking `dotdot_tails.len()` back to `254 == tails_before`.
- `if next_active == active && dotdot_tails.len() == tails_before { break; }` mistook that post-dedup length match for a fixed point, broke early at `256` variants, and returned `Some(...)` instead of failing closed (`None`).
- Because `search_variants_from_cands` then returned `unbounded = false`, while `escape_from_cands` on `/h/u/proj/` + `500 **/ + 500 ../` (`abs = true`) converged in 4 steps with `glob_leaves = true` and `unbounded = false`, `DefaultPolicy::evaluate` treated it as a bounded escaping glob (`Ask` in `Home`, `Allow` under `Any(Search)` and `FullAccess`) instead of failing closed (`unknown = true` -> `Ask` everywhere).

### Changes
- `crates/atrium-core/src/policy/deny.rs` (`2a9be1d`):
  - Fixed `apply_dotdot_run` to compute `let tails_added = dotdot_tails.len() > tails_before;` **before** `dotdot_tails.dedup()` and break only when `next_active == active && !tails_added`. On `500 **/ + 500 ../`, `step_idx = 254` now hits `257 > MAX_GLOB_VARIANTS` and returns `None`, setting `unbounded = true` in `search_variants_from_cands` so `DefaultPolicy::evaluate` returns `Ask` in `Home`, `Any(Search)`, and `FullAccess`.
  - Counted `..` segments scanned in `normalize_glob_variants` (`iter.peek()`) and each reconstructed output state against `arena.step()`, capped `*total_work` on `apply_dotdot_run` failure via `.min(max_work)`, and updated `*total_work` after output reconstruction.
  - Added `can_branch` on `NormArena` so non-branching globs (no `**`/`..`) skip `intern` HashMap lookups/inserts, and cleared `last_rb = None` in `widen_pattern_dots` / `widen_pattern_slashes` when no unescaped `]` exists.
  - Updated linear work assertions in `glob_norm_tests::k10_normalizer_work_is_linear_and_bounded` and added an explicit `MAX_GLOB_WORK` budget-exhaustion test (`star_dd(200)` + `1,000 a/` -> `201 <= 256` variants, returns `None` with `budget_work == MAX_GLOB_WORK`).

### Checks run by this worker (D-123)
- `cargo fmt --all` — passed (exit 0).
- `cargo check -p atrium-core --tests` — passed in 0.48 s with 0 warnings.

### Not done (D-123)
- `cargo test` and `cargo clippy` were not run by this worker per D-123; left for the verification task.
