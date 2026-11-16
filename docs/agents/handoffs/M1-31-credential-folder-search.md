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
- `cargo check -p atrium-core --tests` — passed cleanly in 0.10 s with 0 warnings.

## Commands for the verification task

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
cargo test -p atrium-core --test adversarial_fuzz
cargo test -p atrium-core --test policy
cargo test -p atrium-core --test policy
cargo test -p atrium-core --test policy
cargo test -p atrium-core --test M1-31-credential-folder-search
cargo test -p atrium-core --test adversarial_policy finding_5_a_recursive_search_of_a_folder_with_a_credential_file_is_blocked
git diff dev...HEAD --stat
```

To verify the new/un-ignored tests fail on `dev`'s `src` files (from this worktree, then restore):
```bash
git checkout dev -- crates/atrium-core/src/policy/deny.rs
cargo test -p atrium-core --test M1-31-credential-folder-search -- --nocapture
cargo test -p atrium-core --test adversarial_policy finding_5_a_recursive_search_of_a_folder_with_a_credential_file_is_blocked
git checkout HEAD -- crates/atrium-core/src/policy/deny.rs
```
