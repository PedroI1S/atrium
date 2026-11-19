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

---

## Rework round 3 (`ee57576`)

### Root cause of the `200 */ + 200 ..` timeout (766–807 ms)
- Although `apply_dotdot_run` stepped `NormArena` in linear `O(n)` time (`~1,000` steps for `n = 200`), the output loop in `normalize_glob_variants` (`while id != 0`) only called `arena.step()` once per state (`201` times) and still reconstructed all `201` variant strings of lengths `0, 2, 4, …, 400` segments (`40,200` segments / `~100 KB` per normalizer call, `O(n^2)`), cloned/hashed them in `seen`, and then passed them to `search_variants_from_cands` (which called `fold(&n)` + `split('/')` on all `201` strings before discarding the `200` `..` variants) and `check_escape_variant` (which built `PathBuf` + `components()` for all `201` strings before discarding all `201` in-root variants).
- Across the 3 calls per `evaluate` (`search_variants_from_cands` unjoined + joined, and `escape_from_cands` joined) × 3 presets (`eval_search_presets`), `9` calls reconstructed `~361,800` segments and `~3+ MB` of temporary strings/hashes in unoptimized debug mode, scaling quadratically (`4×` from `n = 100` to `n = 200`, ~200 ms -> 766–807 ms), while `400 */ + 400 ..` tripped `cap = 256` before reaching output reconstruction.

### Changes
- `crates/atrium-core/src/policy/deny.rs` (`ee57576`):
  - Added per-node metadata on `NormNode` (`has_dotdot`, `has_syntax_meta`, `seen_pat`, `literal_prefix_id`, `non_globstar_before_dd`) maintained in `O(1)` in `NormArena::push_seg`.
  - Added `normalize_glob_filtered` + `reconstruct_node_path` (counting `arena.step_n(count.max(1))` during path reconstruction so all reconstruction work is also bounded by `MAX_GLOB_WORK`), while keeping `total_count` (`states.len()`) checked against `MAX_GLOB_VARIANTS` (`256`) so `255 */ + 255 ..` preserves `Allow` and `256 */ + 256 ..` still fails closed to `Ask`.
  - Updated `search_variants_from_cands` to filter useless `..` variants (when `!home_has_dotdot && !node.has_syntax_meta && node.has_dotdot`, outside `ASK_TREES` depth `max_tree_slashes + 2`) **before** string reconstruction and `fold`.
  - Updated `check_escape_variant` to memoize `inside` by `(literal_prefix_id, is_curdir)` **before** reconstructing full pattern strings, skipping all in-root `*/..` variants in `O(1)` per state.
  - Updated `Sensitive::check`'s `glob_escape` loop to skip `ShellGlob` regex compilation on escaping `..` patterns when `!home_has_dotdot`, checking only the literal prefix via `self.s.hit`.
  - Updated `glob_norm_tests::k10_normalizer_work_is_linear_and_bounded` to test `normalize_glob_filtered`, unfiltered `normalize_glob_variants`, and `search_variants_from_cands` + `escape_from_cands` on `n = 200, 255, 256`.

### Checks run by this worker (D-123)
- `cargo fmt --all` — passed (exit 0).
- `cargo check -p atrium-core --tests` — passed in 7.66 s with 0 warnings.

### Not done (D-123)
- `cargo test` and `cargo clippy` were not run by this worker per D-123; left for the verification task.

---

## Rework round 4 (`af17263`)

### Root causes
1. **`M1-31-credential-folder-search.rs:562` (`200 */ + 200 ../ + 20000 a/` taking 6.10 s):**
   - `normalize_glob_filtered` counted every component against `MAX_GLOB_WORK`, but `MAX_GLOB_WORK = 100_000` allowed `200,000` `NormArena::push_seg` calls per `evaluate` (`600,000` across 3 presets) with `can_branch = true`. Each `push_seg` performed a `std::collections::HashMap` (SipHash) `get` and `insert` on `(parent, seg_id, count)` plus two `str::contains` scans (`['[', '{', '\\']` and `['*', '?', '[']`), costing ~6.10 s in unoptimized debug builds on Linux.
   - `expand_glob_candidates` also called `words::brace_expand` (allocating a 41k-`char` `Vec<char>`), `words::expand_home`, `may_be_dot_component`, `widen_pattern_dots`, and `widen_pattern_slashes` even when the glob contained no `{`, `~`, `$`, `?`, or `[`.
2. **N11 (`fix4` 322 ms vs 150 ms; `random_hostile_lines` 221 s vs 60 s on `dev` on Linux):**
   - `fix4` (`{1..64}{a,b,c,d}/x1*/../.../x7*/../*.rs`): on `dev`, `normalize_glob_variants` + `escape_from_cands` expanded 768 `Vec<&str>` paths and `PathBuf`s, and `expand_glob_candidates` ran `expand_home`, `widen_pattern_dots`, and `widen_pattern_slashes` on all 256 brace expansions (all without `~`, `$`, `?`, `[`).
   - `random_hostile_lines` (40,000 lines × 3 `evaluate` calls + `parse_command`): inside owned paths, `Ctx::check` called `self.s.glob_hit(p, u.role)` on every globby word (`*`, `?`, `[`, `*.rs` in `ALPHABET`), which compiled multiple `GlobBuilder` regexes (`ShellGlob::new`) and allocated ~137 `String`s (`Sensitive::probes()`) + 137 `Vec<&str>` splits even when the path could not match any probe (e.g., `/h/u/*`, `/h/u/*.rs`, `/h/u/?`, `/h/u/[`). `recursive_search_roots` also allocated `fold(base(first))` on every command unit.

### Changes (`crates/atrium-core/src/policy/deny.rs`)
- **Work budget & `NormArena` (`af17263`):**
  - Lowered `MAX_GLOB_WORK` from `100_000` to `20_000` (still >7× `255 */ + 255 ../`, and tripped cleanly by `200 */ + 200 ../ + 1000 a/`, `200 */ + 200 ../ + 20000 a/`, and `256 brace * 400 segments`).
  - Replaced `NormArena::intern: HashMap<(usize, u32, usize), usize>` with a lazy open-addressed `Vec<u32>` (`intern_buckets` + `enable_branch`), replaced SipHash `HashMap`/`HashSet` in `NormArena`, `normalize_glob_filtered`, `expand_glob_candidates`, `search_variants_from_cands`, and `check_escape_variant` with deterministic `FxHasher` (`FxMap`/`FxSet`), and precomputed `seg_flags` once per segment in `NormArena::intern_seg`.
  - Fast-pathed `expand_glob_candidates` to skip `words::brace_expand` when `!r.contains('{')`, `words::expand_home` when `!e.starts_with(['~', '$'])`, `may_be_dot_component` / `widen_pattern_dots` when `!s.contains(['?', '['])`, and `widen_pattern_slashes` when `!s.contains('[')`.
- **`Ctx::check` & `recursive_search_roots` (`af17263`):**
  - Added `Ctx::fast_glob_hit` + `Ctx::might_glob_hit_probe`: deduplicates `(lower(p), role)` within `Ctx::check`, runs the same literal-prefix `self.s.hit(&prefix, role)` check as `Sensitive::glob_hit`, and pre-checks segment counts, hidden-dot rules, and literal segments (or `byte_prefix` for `**`) against `CRED_FILES`, `CRED_TREES`, `SSH_PROBES`, `BROWSER_ROOTS`, and `self.s.data` with zero regex compilations before falling through to `self.s.glob_hit`.
  - Iterated `u.paths` / `a.aligned` directly in `Ctx::check` without allocating `Vec<&PathBuf>` per use.
  - Replaced `fold(base(first))` in `recursive_search_roots` with an ASCII stack-buffer `[u8; 6]` fold and early `len > 6` exit.

### Bottlenecks outside owned paths for N11 (`random_hostile_lines`)
Per the N11 brief instruction (*"If the time is outside your paths, name the function in the handoff"*), if `random_hostile_lines` still spends significant time on Linux after bypassing `ShellGlob::new` in `Ctx::check`, the remaining hotspots outside our owned paths are:
- `deny::Sensitive::new` (`crates/atrium-core/src/policy/deny.rs`, called 3 times per `DefaultPolicy::evaluate` — including unconditionally in `deny::analyze_with` even when `unbounded_unknown_words` is empty, in `deny::hard_deny`, and in `engine::DefaultPolicy::evaluate` — allocating ~56 `format!` strings per call).
- `deny::Sensitive::glob_hit` / `ShellGlob::new` / `Sensitive::probes` (`crates/atrium-core/src/policy/deny.rs`, lines ~567–814) when a pattern passes `might_glob_hit_probe` (e.g., `**` under `home`).
- `shell::lex_depth_uncached` / `shell::code_texts_bounded` (calls `shell::unwrap_steps` unconditionally per unit) / `shell::unwrap_readings_bounded` (`crates/atrium-core/src/policy/shell.rs`).
- `words::brace_expand` (`crates/atrium-core/src/policy/words.rs`, allocates `String` + `Vec<char>` per word in `words_of` even when `!w.contains('{')`).

### Checks run by this worker (D-123)
- `cargo fmt --all` — passed (exit 0).
- `cargo check -p atrium-core --tests` — passed in 3.95 s with 0 warnings.

### Not done (D-123)
- `cargo test` and `cargo clippy` were not run by this worker per D-123; left for the verification task (including timing checks on `k10_400_star_dotdot_globs_stay_under_50ms_and_fail_closed_to_ask`, `fix4_finding3_glob_escape_total_variant_cap_performance`, and `random_hostile_lines_never_panic_and_nothing_sensitive_is_allowed`).

---

## Escalation (Opus, D-128)

### Profile findings (debug build, Linux; timing prints + gdb stack sampling)
- **The normalizer itself was not the main cost.** For `20000 */ + 20000 ../` the arena tripped its budget correctly, but `analyze` still took 110–190 ms in `expand_glob_candidates` alone. Microbenchmarks on a 100 KB glob: `str::contains([char; N])` (a multi-char pattern) runs at **~0.9 µs/byte** unoptimized, against ~2 ns/byte for a single-char `contains` (memchr). Every `contains(['?', '['])` scan of a long glob cost ~90 ms.
- **The work before and after the budget was unbounded:** the budget covered only arena steps. Byte scans, `FxHasher` (a per-byte loop), `fold`, joined `format!`s, `words::brace_expand` (74 ms for `{1..64}{a,b,c,d}/` + 400 `a/`, 290 ms per evaluate), one regex compile per `?`/`[` component in `may_be_dot_component` (10 s for `*?[ab]/` × 10 000), one per class in `class_may_match_dot` (6.4 s for 256 braces × 60 `[.]`), and the downstream `GlobBuilder` compiles of long variants (2.5 s for a 128 000-byte literal, 4 s for `[a]` × 30 000) all sat outside it. Each Hydrogen round fixed the stage that the next failing test hit.
- **N11 `random_hostile_lines` (fuzz):** 48 samples. Nothing landed in the glob normalizer. The time goes to `shell::lex_depth_uncached`/`lex` (~30 %), `deny::Sensitive::new` plus its drop (~30 %; it is built three times per `evaluate`: `analyze_with`, `hard_deny`, `engine::Home`), `words::walk`/`words_of`, and `Ctx::check` (~30 %). Inside `Ctx::check`, `fast_glob_hit` → `Sensitive::glob_hit` → `ShellGlob::new` regex compiles are ~8 %.
- **`engine_never_panics_on_garbage`:** the slow part is the `hostile` block. `ls {a,b}×20000` takes **9.4 s** in `words::brace_expand` (it re-collects a `Vec<char>` and rescans the whole 100 KB word for every work item). `ls {a,b}×2000` takes 0.9 s. Nothing here is glob-normalizer work.
- **`fix2_nested_env_split_string_is_linear`:** ~28 ms per evaluate in shell lexing (`env -S` unwrapping) and `Sensitive::new`. No glob work.

### Mechanism (`deny.rs`, commit `b09639c`)
- **One `GlobBudget` per glob check**, carried on `GlobCandidates`. It is a `Cell` counter capped at `MAX_GLOB_WORK = 20 000`. `expand_glob_candidates` creates it, and `search_variants_from_cands` and `escape_from_cands` draw from the same counter. Every stage charges it:
  - brace and home expansion;
  - every candidate and widened candidate (1 step + 1 per 64 bytes);
  - `..` collapsing and the arena (unchanged, now on the shared counter);
  - reconstruction (components + bytes/64);
  - joined strings;
  - `fold`ed outputs;
  - every regex compile: `class_may_match_dot` and `may_be_dot_component` (64 + 4 + len/4), the search matchers including the `ASK_TREES` dir-prefix matchers (4 + len/4 each), and the per-component `ShellGlob` matchers for escaping patterns.
- **Cheap lower-bound gates before each stage,** so a shape that can't fit stops at memchr speed:
  - the component count before normalizing, which equals the minimum number of arena steps;
  - `brace_estimate` (an upper bound on the number of words × the components and bytes outside the braces) before calling `words::brace_expand`.
- **When the budget runs out,** the stage reports unbounded, and the check fails closed to Ask everywhere (the existing `glob_unbounded` path).
- **Verdicts:**
  - A valid pattern of three or more characters skips the dot-probe compile, because it can't match `.` or `..`. That rule is exact.
  - A matcher that is over budget is parsed first. A pattern that doesn't parse is never compiled, so it costs nothing. This keeps the 100 000 unclosed-brackets verdict (Ask / Allow / Allow).
  - Repeated classes are memoized.
- **Constant-factor fixes:**
  - `has_any` (a single-char memchr per char) replaces the `[char; N]` `contains` calls;
  - `widen_pattern_dots` and `widen_pattern_slashes` copy runs between specials with `find`/`push_str`;
  - `FxHasher::write` hashes 8 bytes per round.
- **Verdict changes, fail-closed only:** a glob now asks everywhere when its estimated or real total work exceeds the budget. For example, `[a]` × 30 000 under `~/proj` was Allow in Home on `dev` and is now Ask. Every verdict-asserting test in `atrium-core` passes.

### Times (debug, Linux; single test unless noted)
| Test | Before (branch `e240dbb` / `dev`) | After |
|---|---|---|
| K10 `k10_400_star_dotdot…` worst case | 958 ms (256 brace × 400), 1.7 s (20000 `*/`+`../`), 6.1 s earlier shape | ≤ 41 ms per case (2000 `*/`+`../`); 20000 case 27 ms; 256 brace 1 ms; 100 000 brackets 324 ms over 3 presets (limit 750) |
| K10 whole binary | 14.3 s | 3.7–5.3 s, passes in parallel |
| `fix4_finding3_glob_escape_total_variant_cap_performance` | ~320 ms | 40–90 ms (limit 150) |
| `fix3_finding3_glob_variant_cap_performance` | passed | passes alone; failed once at 310 ms during the full parallel run (CPU contention) |
| `adversarial_fuzz::random_hostile_lines…` | 97 s (branch), 221 s (`dev`) | 96.7 s: **still fails**, the time is outside owned paths (see above) |
| `policy::engine_never_panics_on_garbage` | ~10 s | 10.6 s: **still fails**, `words::brace_expand` (9.4 s on `ls {a,b}×20000`) |
| `fix2_nested_env_split_string_is_linear` | 120–290 ms | 200–580 ms, noisy: **still fails**, shell lexing / `Sensitive::new` |
| Throwaway stress (31 glob shapes × 2 root sets, Paths subject) | worst 10 s | worst 310 ms per evaluate |

### Not done: functions outside owned paths (named per N11)
- `words::brace_expand` (`words.rs`): quadratic in word length × results. Fix: bound total output bytes, or count the words before materializing them. This blocks `engine_never_panics_on_garbage`, and `rg x --glob '{1..64}{a,b,c,d}<4 KB>'` takes 1 s on the command path.
- `deny::Sensitive::new`: built three times per `evaluate` (`analyze_with`, `hard_deny`, `engine`); build it once and share it.
- `shell::lex_depth_uncached` / `lex`, `words::walk`.
- `Sensitive::glob_hit` / `ShellGlob::new` (the `probes…is_match` range).
- Together these make up the fuzz and `fix2` time.
- `policy::pipe_to_shell_fan_out_is_linear_and_denied` fails only under full-suite load; it passes alone.

### Checks
- `cargo fmt --all --check`: clean.
- `cargo clippy -p atrium-core --all-targets -- -D warnings`: clean.
- `cargo test -p atrium-core`: every verdict test passes, including N9 `round_5_blocker_1`. The only failures are the timing tests listed above.

---

## Escalation round 2 (the rest of N11)

Code change: commit `4395d62`.

### Changes
- **`words::brace_expand` (`words.rs`).** Words without `{` return at once. A new `brace_results_lower_bound` does one linear pass that pairs braces exactly as `brace_expand` does. It takes the product of the comma counts of the top-level brace groups that hold no other group and have no `$` before them; every such group picks a word independently in every result. When the product is over `MAX_BRACE_RESULTS`, it returns `None`, the same answer the full expansion gave, without expanding. A fail-closed budget, `MAX_BRACE_WORK` (2 M characters scanned), bounds every other shape: past it the word counts as too large, as past `MAX_BRACE_RESULTS`.
- **`Sensitive::new` → `Sensitive::shared` (`deny.rs`).**
  - Built once per evaluate: `Analysis::sensitive: Rc<Sensitive>`, used by `analyze_with`, `hard_deny`'s `Ctx` and the engine's `Home` (`Ctx::s` and `Home::s` are now `&Sensitive`). The unused `deny::sensitive` was removed.
  - Also reused across evaluates on one thread while home and Atrium's data and protected paths stay equal (a one-entry thread-local cache, keyed on those paths).
- **`shell::lex` (`shell.rs`).** A thread-local memo of the last 4 top-level lexes. A hit is cloned and gets fresh ids through `refresh_ids`, the same way `lex_depth_fresh` already serves cached sub-lexes. One fuzz iteration lexes the same line four times (three evaluates and `parse_command`); it now lexes it once.
- **`Ctx::might_glob_hit_probe` (`deny.rs`, owned range).**
  - Single-char `has_any` scans replace the multi-char `contains([...])` calls.
  - Browser-store probes check the segment count (root + 1 or root + 2) before walking segments.
- **Verdicts:** no verdict test changed. The only new answer is "too large" for a brace word that would scan more than 2 M characters, which is fail-closed.

### Hunks in `shell.rs` / `words.rs` (merge check with M1-28)
- `shell.rs`: one hunk, `pub fn lex` and the `LEX_MEMO` constant above it (~442–472). It doesn't touch the ANSI-C decoder.
- `words.rs`:
  - `MAX_BRACE_WORK` const after `MAX_BRACE_RESULTS` (~66);
  - three hunks at the top of `brace_expand`: the fast path, the lower-bound check and the `scanned` budget (~337–350);
  - new fn `brace_results_lower_bound`, appended at the end of the file (~1049).
  - `cd_effect` is not touched.

### Times (debug, Linux)
| Test | Before (round 1, `527a866`) | After |
|---|---|---|
| `adversarial_fuzz::random_hostile_lines…` (60 s cap) | 96.7 s (`dev` 221 s) | 52.9 / 53.4 s |
| `policy::engine_never_panics_on_garbage` (10 s cap on the hostile block) | 10.6 s (`ls {a,b}×20000` alone 9.4 s) | passes; `ls {a,b}×20000` 101 ms, `×2000` 11 ms |
| `adversarial_m1_18::fix2_nested_env_split_string_is_linear` (200 ms) | 200–580 ms | 150 ms (whole test run) |
| `policy::pipe_to_shell_fan_out_is_linear_and_denied` | passes alone | 2.2–2.4 s alone, passes. It failed once in the full parallel run; it does so at round 1's commit too (CPU contention). |

### Checks
- `cargo fmt --all --check`: clean.
- `cargo clippy -p atrium-core --all-targets -- -D warnings`: clean.
- `cargo test -p atrium-core`: all pass except the one parallel-load failure of `pipe_to_shell_fan_out…` above. That test passes 3/3 alone.

---

## Escalation round 3 (`fix3_finding3_glob_variant_cap_performance` regression)

Code: commit `961bb7a` (`deny.rs` only).

### Profile
- Input: `{1..64}{a,b,c,d}/[a-z]/$VAR/**/../**/../**/../**/../*.rs`.
- Per evaluate, nearly all the time is in `expand_glob_candidates`. `hard_deny` takes ~0.2 ms, and search and escape return at once, because expansion already reports unbounded.
- Expansion walks about 84 brace words through home expansion, `$VAR` widening, dot widening, slash widening and the dot check before `MAX_GLOB_VARIANTS` trips. Round 1 made the dot-check memo per word, so the `[a-z]` component was re-checked for every word. Each word also pays budget hashing.
- The verification failures (164–300 ms) happened under heavy load (load average ~20 from parallel verification jobs). On a quiet machine (load ~1) both `2215897` and this fix finish the test in ≤ 10 ms, so most of the overrun was CPU contention. The fix still removes the extra work.

### Fix
- **Exact variant floor, before any widening.** Every kept brace word `h` is always a candidate, and so is its `**` widening of `$VAR` when `h` has a `$`. When those alone exceed `MAX_GLOB_VARIANTS`, the full pass would trip the cap too, so expansion fails closed right away with the same result: unknown + unbounded → Ask everywhere. The floor only runs when there are more than 128 words.
- **Shared dot-check memo.** `may_be_dot_component` answers are kept in `GlobBudget::dot_memo`, shared by every word.

### Times (debug, Linux, alone)
| Test | Before | After |
|---|---|---|
| `fix3_finding3_glob_variant_cap_performance` (150 ms) | 164–300 ms under load; ~100 ms on a moderately loaded run | ≤ 10 ms quiet; 70–110 ms in 4 of 5 runs under load ~20 (1 of 5 failed at 317 ms) |
| `fix4`, K10, fuzz | pass | pass (full `cargo test -p atrium-core` run, quiet machine) |

### Checks
- `cargo fmt --all --check`: clean.
- `cargo clippy -p atrium-core --all-targets -- -D warnings`: clean.
- `cargo test -p atrium-core`: all pass.
