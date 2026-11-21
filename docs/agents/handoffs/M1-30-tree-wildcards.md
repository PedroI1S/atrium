# Handoff: M1-30-tree-wildcards (S5 + K8)

Status: **ready for DoD verification** after the D-131 escalation (Opus), its round 2 (review 3), and the Neon round (D-134). "Neon round (D-134)" and "Escalation round 2 (review 3)" are the latest and override earlier matcher descriptions where they differ.

## What changed and where

### 1. S5 completion & `deny.rs` audit / deduplication (`crates/atrium-core/src/policy/deny.rs`)
- Audited the WIP commit `0ea7c98` against `dev` (`5e693ad6`) and confirmed every diff hunk in `crates/atrium-core/src/policy/deny.rs` stays strictly inside the brief's owned range (`573..=816` on `dev`: `Sensitive::probes`, `glob_hit`, `is_ssh_dir`, `ask_hit`, `ask_search_glob_hit`, and `ShellGlob` + helper pattern-matching types/functions before `const DOWNLOADERS`). `ask_roots` and `sensitive_roots` needed no change.
- Trimmed duplication in `deny.rs` (bringing the `deny.rs` diff from `+753/-42` down to `+650/-50` even with K8 added):
  - Extracted `pat_starts_with_dot` (shared by `brace_branch_dot_ok`, `Comp::from_part`, and `comp_glob_dp`).
  - Extracted `GlobAtom` / `next_glob_atom` and unified `comp_glob_matches_no_brace` (which previously duplicated a `u64` loop and a `Vec<bool>` loop) and `glob_matches_prefix_no_brace` into one `comp_glob_matches_impl` / `comp_glob_dp` helper (using a 64-entry stack buffer for `m < 64` and heap fallback otherwise).
  - Unified `Tail::matches_tree` and `Tail::matches_strictly_under` into `Tail::matches_tree_impl(&self, tree: &str, strictly_under: bool)`.
  - Added `ShellGlob::as_tail` and a shared `matches_dir_list` closure in `glob_hit` for `self.data` / `self.protected`.
- Verified the late `**` tree-leaf fix in `Tail::matches_tree_impl`: when `reach_indices` reaches `Comp::GlobStar` before a sensitive tree's leaf, it only matches if the remaining components are all `Comp::GlobStar` (`~/.local/**`, `~/.g*/**`), so ordinary `**` globs (`cat ~/**/*.rs`, `cat ~/**/notes.txt`) are not falsely hard-denied.

### 2. K8 (`deny.rs`, now the shared matcher; see "Escalation (Opus, D-131)")
- A non-recursive home `**` search asks only when the glob's file name can be a file of an ask tree (`*.session`, `*.history`, `*.historynew`, `*.zsh`, `.aws/sso/cache/*.json`) or an ask store. Final verdicts in Home: `**/*.rs`, `**/*.md`, `**/*/*.rs`, `**/*.cache` → Allow; `**/*.session`, `**/b.session`, `**/*.history`, `**/token.json`, `**/*.toml`, `**/*.{rs,toml}` (via `.cargo/credentials.toml`), `**/*token*`, `**/m*.rs`, `**/*.bin`, `.azure/*.rs` → Ask; `**/*.json`, `**/*`, `**`, `**/` → Hard(credential-read) (they reach `.docker/config.json` and other credential files).
- Note on `rg --glob '**/*.rs' x ~` and `Search ["~"] **/*.rs` (`recursive: true`): in `crates/atrium-core/src/policy/engine.rs` (outside M1-30 owned paths), recursive searches rooted directly at `~` still return `Ask` in Home because `Home::root_ok(~)` is `false` (walking all of `~` reaches `.ssh`/`.gnupg`/etc. regardless of `--glob`), whereas `rg --glob '**/*.rs' x ~/proj` returns `Allow`.

### 3. Tests (`crates/atrium-core/tests/M1-30-tree-wildcards.rs` and `adversarial_gate.rs`)
- `crates/atrium-core/tests/adversarial_gate.rs`: `finding_11_a_glob_inside_atriums_folder_names_is_blocked` un-ignored (from `0ea7c98`).
- `crates/atrium-core/tests/M1-30-tree-wildcards.rs` tests and what each pins:
  1. `wildcards_in_each_component_of_cred_trees_and_files`: `*`, `?`, `[...]` classes, and `{...}` braces in every component of `CRED_TREES` (`.gnupg`, `.config/gcloud`, `.gemini`, `Library/Keychains`) and `CRED_FILES`, home-anchored and relative, are `HardDeny(credential-read)`.
  2. `wildcards_in_ssh_dir_keep_plain_ssh_verdicts`: wildcarded `.ssh` paths (`~/.s?h/id_rsa`, `~/.[s]sh/id_ed25519`, `~/.ssh/id_*`, `~/.ssh/**`) hard-deny secrets and writes, while `ls ~/.[s]sh`, `*.pub`, `known_hosts*`, `config`, and `authorized_keys` reads keep `Ask / Ask / Allow / Allow`.
  3. `wildcards_in_browser_roots`: wildcards in browser root components and cookie/login filenames hard-deny (`credential-read`), while non-browser files (`README.md`) and mismatched depths keep `Ask / Ask / Allow / Allow`.
  4. `wildcards_in_atrium_data_and_protected_folders`: wildcards in `~/.local/share/atrium`, `~/Library/Application Support/atrium`, and protected config folders (`~/.config/atr*`, `atr*-cfg`) hard-deny reads/writes on data and writes on protected, while reads on protected keep `Ask / Ask / Allow / Allow`.
  5. `wildcards_in_ask_stores_and_trees`: wildcards in `ASK_FILES`, `ASK_TREES`, and `ASK_NAME_PREFIXES` (`~/.[a]zure/msal_token_cache.json`, `~/.a?ure/msal_*`) ask in Home and allow in FullAccess; `~/.azure/config` allows.
  6. `relative_paths_from_subdir_and_file_tools_reads_and_searches`: `../` relative command words from `~/proj` and `ToolKind::Read` / `Edit` / `Search` (`ToolSubject::Paths`, including escaping and non-escaping globs and ask-tree/prefix searches) preserve expected `HardDeny` / `Ask` / `Allow` verdicts.
  7. `ordinary_globs_keep_expected_verdicts`: ordinary globs (`~/Documents/*.md`, `src/*.rs`, `~/*`, `~/.config/nvim/*.json`, `~/.config/app/settings.json`, `~/Documents/[A-Z]*.md`, `~/.config/*/settings.json`, `~/.config/*/README.md`, `~/.local/share/*/notes.md`, `~/Library/*/notes.md`, `~/*/*/notes.md`, `**/*.rs`, `src/**/mod.rs`) keep `dev`'s verdicts (`Ask / Ask / Allow / Allow`, or `Allow` for `~/.config/app/settings.json`).
  8. `non_cd_fuzz_spellings_of_sensitive_paths_are_always_hard_denied`: 30,000-iteration fuzz test across all sensitive targets (excluding `cd <glob>; <reader> <file>`).
  9. `globstar_tree_leaf_rules_preserve_ordinary_globs_and_block_sensitive_trees` *(added in round 1)*: pins the `**` tree-leaf fix — confirms `cat ~/**/*.rs`, `cat ~/**/notes.txt`, `cat ~/.local/**/*.rs`, `cat ~/Library/**/*.md`, `cat ~/.config/**/*.rs` stay `Ask / Ask / Allow / Allow` and `Read ~/**/*.rs` stays `Allow`, while `cat ~/**`, `cat ~/.local/**`, `cat ~/.local/share/atr*/**`, `cat ~/.local/**/atrium.db`, `cat ~/**/atrium.db`, `cat ~/.g*/**`, `cat ~/**/.g*/secring.gpg`, `cat ~/.s?h/**`, `cat ~/.config/g*chrome/**`, `Read ~/.z*/**`, and `Read ~/**/.z*/*.session` remain blocked/asked.
  10. `k8_home_globstar_search_asks_only_when_suffix_can_match_ask_tree` *(added in round 1; expectations last changed in escalation round 2)*: non-recursive `Search ["~"]` returns `Allow / Allow / Allow` for `**/*.rs`, `**/*.md`, `**/*/*.rs`, `*/*.rs`, `src/**/*.rs`, `**/src/*.rs`, `**/*.{rs,md}` and `**/*.cache`. It returns `Ask / Allow / Allow` for the session/history families, `**/token.json`, `**/cache.json`, `**/*.bin`, `**/*.toml`, `**/*.{rs,toml}`, `**/*.{rs,session}`, `**/[0-9]*.session`, `**/*token*`, `**/msal_token_cache.db` and `**/m*.rs` (`m` pins the cache's stem). It also asks for `.azure/*.rs`, `Search ["~/.azure"] *.rs` (both can be `msal_token_cache.rs`) and the explicit ask-tree paths. `**/*.json`, `**/*`, `**` and `**/` give `Hard(credential-read)` everywhere. Recursive `Search ["~"] **/*.rs` and `rg --glob '**/*.rs' x ~` still ask in Home.
  11. `session_gate_blocks_wildcarded_credential_and_atrium_trees_when_absent_on_disk` *(added in round 1)*: pins S5 through `SessionGate` when targets do not exist on disk.
  12. `rework_round2_regressions` *(added in round 2, updated in round 3)*: pins fixes for round 1 review findings 3–8 (POSIX named classes including `~/[[:punct:]]ssh/id_rsa` matching `.ssh` and `~/.[[:punct:]]sh/id_rsa` not matching `.ssh`, Atrium WAL/SHM/journal/snapshots/logs under `**`, K8 filename-family matching for `**/b.session`/`**/b.history`, Azure `msal_token_cache.*` arbitrary extensions, bare `*` leaf rule preserving `~/.config/*/settings.json`, and singleton class spellings of `.ssh/config` / `authorized_keys` / `.pub` / `known_hosts`).
  13. `review2_finding_1_contained_search_globs_reach_sensitive_trees` *(was `rework_round2_finding_1_…_escalated`; un-ignored in the D-131 escalation)*: `Search ["~"] false ".g*/private-keys-v1.d/k.key"` → `Hard(credential-read)` and `".local/share/atr*/missing.db"` → `Hard(atrium-self)` in Home, Any and FullAccess.
  14. `posix_named_classes_match_real_character_sets` *(added in round 3)*: table test covering all 13 POSIX named classes (`alpha`, `alnum`, `lower`, `upper`, `punct`, `print`, `graph`, `space`, `digit`, `xdigit`, `word`, `blank`, `cntrl`), naming for each class a component that matches `.ssh` (`Hard(credential-read)`) and one that does not (`Ask / Ask / Allow / Allow`), plus `.` / `s` boundary and `^` negation cross-checks.

## Tests expected to fail on `dev`'s code

Running the current test suite against `dev`'s `crates/atrium-core/src/policy/deny.rs` is expected to fail on:
- `crates/atrium-core/tests/M1-30-tree-wildcards.rs`:
  - `wildcards_in_each_component_of_cred_trees_and_files`
  - `wildcards_in_ssh_dir_keep_plain_ssh_verdicts`
  - `wildcards_in_browser_roots`
  - `wildcards_in_atrium_data_and_protected_folders`
  - `wildcards_in_ask_stores_and_trees`
  - `relative_paths_from_subdir_and_file_tools_reads_and_searches`
  - `non_cd_fuzz_spellings_of_sensitive_paths_are_always_hard_denied`
  - `globstar_tree_leaf_rules_preserve_ordinary_globs_and_block_sensitive_trees` (sensitive `**` with wildcarded tree names like `cat ~/.g*/**`, `cat ~/.local/share/atr*/**`, `cat ~/**/.g*/secring.gpg`, `Read ~/.z*/**`)
  - `k8_home_globstar_search_asks_only_when_suffix_can_match_ask_tree` (`dev` returns `Ask` instead of `Allow` in Home for `Search ["~"] **/*.rs` and the other non-recursive `**/*.{md,...}` home searches; `.az*/msal_token_cache.db` returns `Allow` instead of `Ask` on `dev`)
  - `session_gate_blocks_wildcarded_credential_and_atrium_trees_when_absent_on_disk` (`dev` returns `Ask` instead of `Deny` when wildcarded directories do not exist on disk)
  - `rework_round2_regressions` (`dev` fails on POSIX classes, WAL under `**`, Azure `msal_token_cache.*`, and `cat ~/.config/g*/settings.json`)
  - `posix_named_classes_match_real_character_sets` (`dev` fails on POSIX named classes matching `.ssh`)
- Note: `ordinary_globs_keep_expected_verdicts` is expected to **pass** on `dev` (verifying no regressions on ordinary globs, including `cat ~/.config/*/settings.json` -> `Ask / Ask / Allow / Allow`). `adversarial_gate::finding_11_a_glob_inside_atriums_folder_names_is_blocked` also passes on `dev` because `world()` creates the target files on disk.

## Escalated / outside owned paths

- **Finding 1 (contained search globs):** fixed in the D-131 escalation. It needed one line in `Ctx::search_glob_hit` (owned by M1-31); see "Escalation round 2 (review 3)", finding 7, for the explanation and the coordination needed before merge.
- **Finding 2 — wildcarded `cd` destination (`crates/atrium-core/tests/adversarial_fuzz.rs`, `random_spellings_of_sensitive_paths_are_always_hard_denied`, line 422):** still `#[ignore = "M1-13 finding 11"]`. Its `shape_pick == 8` generates `cd <glob>; <reader> <file>` (e.g. `cd ~/.a?s; cat credentials`, `cd ~/.s?h; cat id_rsa`). In `crates/atrium-core/src/policy/words.rs:cd_effect` (line 628, owned by M1-28), any `cd` target containing `*`, `?`, or `[` returns `None` (`Cd::Lost`). Left to M1-28 per rework instructions.

## Checks run by this worker (D-123)

- `cargo fmt --all`: exited 0.
- `cargo check -p atrium-core --tests`: exited 0 (7.45s, 0 warnings on initial pass; 0.49s, 0 warnings in rework round 1; 0.51s, 0 warnings in rework round 2; 5.42s, 0 warnings in rework round 3; 8.73s, 0 warnings in rework round 4; 7.36s, 0 warnings in rework round 5).

## Rework round 1

1. **`Read ~/**/*.rs` failure at `crates/atrium-core/tests/M1-30-tree-wildcards.rs:738`:**
   - **Root cause:** `Sensitive::ask_hit` called `Tail::matches_dir_and_name_prefix(".azure", "msal_token_cache.")` -> `Comp::matches_prefix`, which used `comp_glob_matches_impl("*.rs", "msal_token_cache.", true)`. Because `comp_glob_dp` with `prefix = true` short-circuits as soon as `GlobAtom::Star` fills `dp[m] = true`, `*.rs` falsely matched the prefix `"msal_token_cache."`. Meanwhile `ask_search_glob_hit` had its own `PREFIX_PROBES` / `matches_prefix_pat` filter.
   - **Fix (`crates/atrium-core/src/policy/deny.rs`):** Extracted a shared `ASK_PREFIX_PROBES` + `matches_ask_name_prefix_pat(file_pat, pre)` helper used by both `Comp::matches_prefix` (for `ask_hit` / `Tail::matches_dir_and_name_prefix`) and `Sensitive::ask_search_glob_hit` (also handling `dir_rel.is_empty()` with `**/` or `dir_abs/`). Now `Read ~/**/*.rs`, `Read ~/**/*.md`, `Read ~/.azure/*.rs`, and `Search ["~"] false ".azure/*.rs"` allow in Home, while `~/**/msal_token_cache.*`, `~/.[a]zure/*`, and `~/.[a]zure/*.json` continue to ask.

2. **`k8_home_globstar_search_asks_only_when_suffix_can_match_ask_tree` expectations (`crates/atrium-core/tests/M1-30-tree-wildcards.rs`):**
   - **`**/*.toml` and `**/*.{rs,toml}`:** The test expected `Allow` in Home, which was wrong: `ASK_FILES` (CR-12) includes `.cargo/credentials.toml`, so `ask_probes()` matches `~/.cargo/credentials.toml` against `**/*.toml` (just as on `dev`). Per the brief, K8 suppresses false positives from `ASK_TREES` when later components cannot match a file in an ask tree, while keeping CR-12 ask files/trees asking for globs that can match their files. Moved `**/*.toml` and `**/*.{rs,toml}` to the `Ask` list (`["Ask", "Allow", "Allow"]`), and added `**/*.{rs,md}` and `.azure/*.rs` to the `Allow` list.
   - **`**/*.json`, `**/*`, `**`, `**/`:** These were in the `["Ask", "Allow", "Allow"]` loop after `**/*.toml` (not reached when `**/*.toml` failed). In `DefaultPolicy::evaluate`, `hard_deny` (`search_glob_hit`) runs before `ask_search_glob_hit`, and `CRED_FILES` probes (`.docker/config.json`, `.netrc`, `.claude.json`, `.codex/auth.json`, etc.) match those globs -> `Hard(credential-read)` in all presets (same as `dev`). Moved them to assert `["Hard(credential-read)", "Hard(credential-read)", "Hard(credential-read)"]`, and added `**/token.json`, `**/cache.json`, and `**/*.cache` to the `["Ask", "Allow", "Allow"]` list to exercise `.aws/sso/cache` and `ASK_NAME_PREFIXES` without hitting `CRED_FILES`.

3. **Full workspace test failure investigation (9/10 failing in 0.03s, 7 in `common_adversarial/mod.rs:180`):**
   - Audited `crates/atrium-core/tests/M1-30-tree-wildcards.rs`, `crates/atrium-core/tests/common_adversarial/mod.rs`, and `crates/atrium-core/src/policy/` for any global/shared state, `std::env` mutation, real `$HOME`/`/tmp` usage, or test-order dependency:
     - `DefaultPolicy::new().evaluate(...)` in `engine.rs`/`deny.rs` is pure (zero filesystem reads, zero `std::env`, zero mutable globals; `shell.rs` only uses monotonic `AtomicU32` IDs for equality within one lex call).
     - All existing tests in `M1-30-tree-wildcards.rs` use stack-local `Env::home()` / `env_with_app_support()` with virtual `HOME = "/h/u"` and `resolved_paths: &[]`.
     - Notably, only 7 tests in `M1-30-tree-wildcards.rs` call `run_table` (`common_adversarial/mod.rs:180`): tests 1–6 plus test 9 (`globstar_tree_leaf_rules_preserve_ordinary_globs_and_block_sensitive_trees`), which only reaches `run_table` (line 747) if `Read ~/**/*.rs` (line 738) passes — which happens on `dev`'s `deny.rs` (where 9/10 tests fail in ~0.03s and 7 fail at `common_adversarial/mod.rs:180`, while only `ordinary_globs_keep_expected_verdicts` passes).
   - The new `SessionGate` test added in rework round 1 uses its own isolated `common_adversarial::Tmp::new()` temp directory (unique per PID + atomic counter, cleaned up on `Drop`) and never touches real `$HOME` or shared fixtures.

4. **`adversarial_gate.rs` (`finding_11_a_glob_inside_atriums_folder_names_is_blocked`):**
   - Explained why it already passes on `dev`: `adversarial_gate.rs`'s `world()` fixture creates `.gnupg/private-keys-v1.d/k.key`, `.local/share/atrium/atrium.db`, and `.config/atrium/config.toml` on disk, so `gate.rs:expand_glob` expands every entry in `GLOB_IN_ATRIUM_NAMES` to existing disk paths and passes them in `resolved_paths` to `Sensitive::hit`, which hard-denies them even on `dev`.
   - Kept `adversarial_gate.rs` untouched beyond removing `#[ignore]` (per brief's owned-paths rule: new tests go in `M1-30-tree-wildcards.rs`), and added `session_gate_blocks_wildcarded_credential_and_atrium_trees_when_absent_on_disk` in `crates/atrium-core/tests/M1-30-tree-wildcards.rs`. It tests wildcarded credential and Atrium folders (`.g*/nonexistent`, `.gem*/oauth_creds.json`, `.con*/gcloud/credentials.db`, `.local/share/atr*/missing.db`, `.config/atr*/new.toml`, `.s?h/nonexistent`) through `SessionGate` when those targets do not exist on disk (`gate.rs:expand_glob` returns empty, so `dev` returns `Ask` and fails, while this branch returns `Deny` and passes).

## Rework round 2

1. **Finding 1 (non-escaping search globs miss arbitrary files inside sensitive trees):** escalated (blocked on owned paths) — fixing `Search ["~"] false ".g*/private-keys-v1.d/k.key"` and `".local/share/atr*/missing.db"` requires changing `Ctx::search_glob_hit` (`crates/atrium-core/src/policy/deny.rs`, ~1470 on `dev`), which is owned by M1-31 (`check`/`search_glob_hit`, ~1175–1520). Left `search_glob_hit` untouched per brief rules; reproduction test `rework_round2_finding_1_non_escaping_search_globs_escalated` added as `#[ignore]`.
2. **Finding 2 (wildcarded `cd` loses the sensitive destination):** left to M1-28 (`words.rs:cd_effect`) per rework instructions; `adversarial_fuzz.rs:422` remains `#[ignore = "M1-13 finding 11"]`.
3. **Finding 3 (POSIX named character classes bypass component matcher):** fixed (`posix_bracket_end`, `posix_class_matches`, `first_class_end`, `class_matches_char` in `deny.rs`, with fail-closed fallback on unsupported POSIX bracket syntax); pinned by `rework_round2_regressions` (`cat ~/.s[[:alpha:]]h/id_rsa`, `cat ~/.s[![:digit:]]h/id_rsa`, `cat ~/.s[[:digit:]]h/id_rsa`).
4. **Finding 4 (`**` leaf exception misses Atrium WAL):** fixed (`Sensitive::DATA_PROBE_FILES` and `Sensitive::PROTECTED_PROBE_FILES` in `probes`/`glob_hit`, adding `atrium.db-wal`, `atrium.db-shm`, `atrium.db-journal`, `db`, `app-identity.json`, `snapshots`, `logs`, and `atrium.sock`/`atrium.lock`); pinned by `rework_round2_regressions` (`cat ~/.local/**/atrium.db-wal`, `cat ~/**/atrium.db-wal`, `cat ~/.local/**/snapshots/t1.json`).
5. **Finding 5 (K8 fixed filename samples allow valid session-file searches):** fixed (`ASK_TREE_SUFFIXES`, `glob_matches_stem_suffix`, `ask_tree_matches_rest` in `ask_search_glob_hit` matching filename families symbolically rather than `0`/`a`/`x` stems); pinned by `rework_round2_regressions` (`Search ["~"] false "**/b.session"`, `"**/b.history"`, `"**/b.rs"`).
6. **Finding 6 (Azure prefix matching misses arbitrary extensions):** fixed (`glob_matches_name_prefix` + `matches_ask_name_prefix_pat` intersecting `msal_token_cache.<any-ext>` symbolically without `ASK_PREFIX_PROBES`, while keeping generic code/doc stems like `*.rs`/`m*.rs` allowed); pinned by `rework_round2_regressions` (`Read ~/.a?ure/*token_cache.custom`, `Search ["~"] false ".a?ure/*token_cache.custom"`).
7. **Finding 7 (ordinary-glob test codified a new hard deny for `cat ~/.config/*/settings.json`):** fixed (`Comp::is_bare_star`/`Comp::is_catchall` in `Tail::matches_tree_impl` so a bare `*` at a tree leaf only matches when trailing components are catch-alls, matching `dev`, and restored `cat ~/.config/*/settings.json` to `["Ask", "Ask", "Allow", "Allow"]` per brief "What to do" §2 and "Rules"); pinned by `ordinary_globs_keep_expected_verdicts` (including `**/*.rs` and `src/**/mod.rs`) and `rework_round2_regressions`.
8. **Finding 8 (singleton class spellings of readable SSH config hard-denied):** fixed (`Comp::is_singleton_literal`, `pat_is_singleton_literal`, `pat_is_public_ssh_name` in `Tail::matches_ssh_secret` so singleton class/brace spellings like `confi[g]` keep `Allow`/`Ask` while broader patterns like `confi[gh]`/`confi?`/`{config,id_rsa}` stay `HardDeny`); pinned by `rework_round2_regressions` (`Read ~/.s?h/confi[g]`, `cat ~/.s?h/confi[g]`, `rm ~/.s?h/confi[g]`).
9. **Finding 9 (handoff reported superseded K8/ordinary-glob verdicts):** fixed by updating the K8 summary, ordinary-glob baseline note, and verification instructions in `docs/agents/handoffs/M1-30-tree-wildcards.md`.

## Rework round 3

1. **`rework_round2_regressions` failure on `"cat ~/.[[:punct:]]sh/id_rsa"` (`crates/atrium-core/tests/M1-30-tree-wildcards.rs`):**
   - **Why the test expectation was wrong:** `.[[:punct:]]sh` requires a literal `.`, followed by a single punctuation character, followed by `sh`. Because `.ssh` has `'s'` (not punctuation) as its second character, `.[[:punct:]]sh` does **not** match `.ssh` (so `cat ~/.[[:punct:]]sh/id_rsa` returns `Ask` in Home and `Allow` under Any/FullAccess). By contrast, `~/[[:punct:]]ssh/id_rsa` places `[[:punct:]]` at the leading `.` position (where `.` is ASCII punctuation) and therefore matches `.ssh` (`Hard(credential-read)`).
   - **Fix (`crates/atrium-core/tests/M1-30-tree-wildcards.rs`):** Replaced `("cat ~/.[[:punct:]]sh/id_rsa", Want::Hard(CR))` with `("cat ~/[[:punct:]]ssh/id_rsa", Want::Hard(CR))` in `run_table`, and added `("cat ~/.[[:punct:]]sh/id_rsa", ["Ask", "Ask", "Allow", "Allow"])` to `check_4_settings`.
2. **Real character sets for all 13 POSIX named classes (`crates/atrium-core/src/policy/deny.rs`, `crates/atrium-core/tests/M1-30-tree-wildcards.rs`):**
   - Audited `posix_class_matches` in `deny.rs` for all 13 classes (`alpha`, `alnum`, `lower`, `upper`, `punct`, `print`, `graph`, `space`, `digit`, `xdigit`, `word`, `blank`, `cntrl`): updated `"space"` to `ch.is_ascii_whitespace() || ch == '\x0B'` so vertical tab (`\x0B`) is included alongside `' '`, `'\t'`, `'\n'`, `'\x0C'`, `'\r'`; confirmed the remaining 12 classes already match their real ASCII/C-locale character sets (`is_ascii_alphabetic`, `is_ascii_alphanumeric`, `is_ascii_lowercase`, `is_ascii_uppercase`, `is_ascii_punctuation`, `is_ascii_graphic() || ' '`, `is_ascii_graphic`, `is_ascii_digit`, `is_ascii_hexdigit`, `is_ascii_alphanumeric() || '_'`, `' ' | '\t'`, `is_ascii_control`).
   - Added `posix_named_classes_match_real_character_sets` in `crates/atrium-core/tests/M1-30-tree-wildcards.rs`, with a table naming all 13 classes and testing for each one a component matching `.ssh` (`Hard(credential-read)`) and a component not matching `.ssh` (`["Ask", "Ask", "Allow", "Allow"]`), plus cross-checks on `.`/`s` boundaries and `^` negation.

## Rework round 4

1. **Per-word and per-command performance optimizations (`crates/atrium-core/src/policy/deny.rs`):**
   - **Zero-allocation path lowercasing (`with_lower_or` / `with_lower_path` / `with_lower_str`):** `glob_hit`, `is_ssh_dir`, and `ask_hit` now avoid `lower(p)` (`String` allocation + `.replace('ſ', "s")`) for ASCII paths without uppercase (and use a 512-byte stack buffer for ASCII paths with uppercase), falling back to `lower(p)` only for non-ASCII or > 512-byte paths.
   - **`is_ssh_dir` fast path:** Replaced `format!("{}/.ssh", self.home)` with `under(pl, &self.home) == Some(".ssh")`, and when `pl` contains glob metacharacters, checks whether the final component is `"**"` or matches `".ssh"` before building `ShellGlob` or running `tails_after`.
   - **`ask_hit` fast path:** Replaced the 16 per-word `f.to_lowercase()` allocations in `ASK_FILES` matching with `eq_ignore_ascii_case` (literal branch) and stack-lowercased `with_lower_str` (glob branch); added `path_could_reach_root` before `ShellGlob::new` when `!pl.contains("**")`.
   - **`glob_hit` & `ShellGlob` / `Tail` stack buffers:** Replaced `split_comps` (`Vec<&str>` allocations) and `format!("{d}/{f}")` / `format!("{tree}/x")` / `format!(".ssh/{n}")` / `format!("{b}/{f}")` allocations with stack-backed `Segs`, `CompBuf`, `IdxBuf`, `TailBuf`, and `BoolDp` buffers (with heap fallback only above stack capacities). Added `path_could_reach_root` early exit, zero-alloc `hit_prefix` slicing when no `/./`/`/../`, `has_deep_tail` guard before `BROWSER_FILES`/`BROWSER_ROOTS`, and 1D DP in `reach_indices` (eliminating `Vec<Vec<bool>>`).
   - **Component glob DP (`comp_glob_dp`) & `Comp::from_part`:** Added fast literal prefix/suffix checks when `!prefix`, zero-alloc ASCII byte-slice matching in `comp_glob_dp`, and `c.contains(['.', '['])` guard in `Comp::from_part` before calling `brace_branch_dot_ok` (so `{a,b}...` without `.`/`[` skips `words::brace_expand`). Replaced per-atom `Vec<bool>` allocations in `glob_matches_stem_suffix` and `glob_matches_name_prefix` with `BoolDp`.
   - **`ask_search_glob_hit` (`ASK_NAME_PREFIXES` / `ASK_TREES`):** Checked `matches_ask_name_prefix_pat` before compiling `GlobBuilder::new(dir_pat)`, skipped `GlobBuilder::new(dir_pat)` when `dir_pat == "**"`, and deferred `GlobBuilder::new(rest)` validation until after `dir_pat` matches a tree and `ask_tree_matches_rest` returns `false`.
   - **Note on `policy::engine_never_panics_on_garbage`:** By inspection of `policy.rs:2234-2269`, `words.rs:464-478`, and `engine.rs:908-915`, all five `hostile` inputs inside the timed `started..elapsed()` block (`hostile[0]` > `MAX_BYTES`, `hostile[1..=2]` `brace_expand` overflow, `hostile[3]` > `MAX_UNITS`, `hostile[4]` > `MAX_USES`) set `a.overflow = true` in `deny::analyze` and return `PolicyVerdict::Deny` at `engine.rs:910` before `hard_deny` or `Home` matchers run; the 20,000 random-line loop before `started` does exercise `is_ssh_dir`, `glob_hit`, and `ask_hit`, and `fix2_nested_env_split_string_is_linear` exercises `is_ssh_dir` and `ask_hit` across `uses` and `unknown_tok_dangerous`. All edits stay strictly inside the owned `deny.rs` range (`563..=816` on `dev`).

## Rework round 5

- **Clippy `large_enum_variant` (`crates/atrium-core/src/policy/deny.rs`):** Boxed the large `Stack` variant's payload in `Segs<'a>` (`Box<SegsStack<'a>>`) and `CompBuf<'a>` (`Box<CompBufStack<'a>>`) without silencing the lint and with no behaviour change.

## Commands for the verification task (DoD)

1. `cargo fmt --all --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test --workspace --no-fail-fast`
4. Run `cargo test -p atrium-core` 3 times in a loop and record timings for `adversarial_fuzz` and `policy` (no slowdown).
5. Check out `dev:crates/atrium-core/src/policy/deny.rs` and run `cargo test -p atrium-core --test M1-30-tree-wildcards`. Confirm the S5/K8/review tests fail on `dev` and `ordinary_globs_keep_expected_verdicts` passes. Then restore `HEAD`'s `deny.rs` and re-verify that all 27 tests pass (none are ignored).
6. Confirm `git diff 5e693ad6..HEAD --stat` touches only owned paths (`crates/atrium-core/src/policy/deny.rs`, `crates/atrium-core/tests/M1-30-tree-wildcards.rs`, `crates/atrium-core/tests/adversarial_gate.rs`, `docs/agents/handoffs/M1-30-tree-wildcards.md`), with the single documented 1-line `self.s.search_filter_hit(variants)` call in `Ctx::search_glob_hit` pending M1-31 / driver confirmation (R3-7).

## Escalation (Opus, D-131)

### Matcher design
One bounded glob-component matcher in `deny.rs` replaces `ShellGlob`/`Tail`/`Comp`, `ASK_TREE_SUFFIXES` helpers, the Azure prefix helpers and the singleton/public-name helpers.
- **Components:** a pattern component becomes atoms: `*`, and one-character sets (literal, `?`, `[...]` with `!`/`^`, ranges, escapes and all 13 POSIX classes, stored as an ASCII bitmap plus a non-ASCII flag). An unknown `[:name:]` matches any character, so it fails closed. An unclosed `[` is literal. Braces are expanded once at path level with `words::brace_expand`, up to 64 variants. When a pattern has more variants, or a group that won't close, it is widened (a group becomes `*`, or `/**` when the group holds a `/`) and checked **without keys**, so the check gets stricter. A brace that isn't an expansion (`{g}`) stays literal, so nothing recurses.
- **Component DP** (`comp_intersect`): states (pattern atom, target atom, started), with a stack and a visited bitmap. It answers "do these two name globs share a non-empty name?". It handles names against patterns and patterns against patterns (`m*.rs` against `msal_token_cache.*`). `*` and `?` never match a leading `.` unless the pattern holds a `**` (globset, `dev`'s search semantics).
- **Path DP** (`path_meets`): states (pattern component, shape component). Adjacent `**` are merged while parsing, and `..` pops lexically. Shapes are built once per `Sensitive` (`OnceLock`) from the lists the engine already reads: `CRED_FILES`, `CRED_TREES`, `SSH_PROBES`, `BROWSER_ROOTS`/`BROWSER_FILES`, `ASK_FILES`, `ASK_TREES` (+ `ASK_TREE_SUFFIXES`), `ASK_NAME_PREFIXES`, the data and protected folders, and `Sensitive::DATA_FILES`/`DATA_SUBTREES`/`PROTECTED_FILES`. That is now the single artifact list, read by both the probes and the shapes, and it includes `daemon.sock`. Shape components: a literal; a name glob; a set of names; one name; any number of names; an SSH name that isn't public (exact on singleton atoms: forced `.pub` suffix, forced `known_hosts` prefix, literal `config`/`authorized_keys` at the top level when not writing); and an Azure cache name spelled with a literal run of at least four of the prefix's characters.
- **Keys:** each shape marks the components that name it (a tree's folder, a file's name). A pattern component that matches every listed name (`*`, `***`, `?*`, `*[!.]*`, `**`) only counts against a key when every later component also does. This keeps `~/.config/*/settings.json`, `~/**/*.rs`, `src/**/mod.rs` and `*.{ts,tsx}` ordinary, while `~/.g*/k.key`, `~/**/b.session`, `~/.config/*/*` and `~/**` are not.
- **Floor:** `dev`'s probes are still matched without keys, so no line is more permissive than `dev`.
- **Budget:** 4,000,000 DP states per query. A check that runs out of budget answers "can match", so the shape's own verdict applies (hard deny or Ask), never Allow.
- **Callers:** `glob_hit`, `is_ssh_dir`, `ask_hit` (globs), `ask_search_glob_hit` and `Ctx::search_glob_hit` all use it. Both search checks run every anchored variant of the filter through it. `words::cd_effect` (M1-28) was not touched; `Sensitive::glob_str_hit(pattern, role)` is ready for it.

### Findings and the tests that pin them (`crates/atrium-core/tests/M1-30-tree-wildcards.rs`)
- R2-1 / R1-1 (contained search globs `.g*/private-keys-v1.d/k.key`, `.local/share/atr*/missing.db`): `review2_finding_1_contained_search_globs_reach_sensitive_trees` (un-ignored, renamed).
- R2-2 (literal braces recurse forever, `~/.s?h/confi{g}`): `review2_finding_2_literal_braces_terminate_and_hard_deny`.
- R2-3 (4,096 adjacent globstars, cubic work): `review2_finding_3_adjacent_globstars_are_bounded`.
- R2-4 (`Read ~/**/b.session`, `~/**/b.history`): `review2_finding_4_session_families_ask_for_read_paths`.
- R2-5 (`rm /run/user/1000/**/daemon.sock`): `review2_finding_5_globstar_reaches_the_daemon_socket`.
- R2-6 (`~/.a?ure/m*.rs`): `review2_finding_6_azure_cache_keeps_every_extension` and `review2_finding_6_globstar_names_the_azure_cache`. Changed expectation: `Read ~/.azure/*.rs`, `Search ["~"] .azure/*.rs` and `Search ["~/.azure"] *.rs` now Ask in Home (they can match `msal_token_cache.rs`).
- R2-7 (`cat ~/.config/***/settings.json`): `review2_finding_7_catchall_spellings_stay_ordinary`.
- R2-8 (stale K8 summary): sections 2 and 3 above were rewritten.
- R1-3 to R1-8: still pinned by `rework_round2_regressions` and `posix_named_classes_match_real_character_sets`, now passing on the new matcher.
- R1-2 (`cd <glob>`): still M1-28's (`words::cd_effect`). `adversarial_fuzz.rs`'s ignored test stays ignored until M1-28 merges (brief's last step).

### Commands run (Opus)
- `cargo fmt -p atrium-core`: clean.
- `cargo clippy -p atrium-core --all-targets -- -D warnings`: no warnings.
- `cargo test -p atrium-core --test M1-30-tree-wildcards`: 21 passed, 0 ignored (about 28 s, mostly the 30,000-iteration fuzz).
- `cargo test -p atrium-core --no-fail-fast`: everything passes except `round_5_blocker_1_searches_and_walkers_reaching_a_folder_linked_into_ssh` (the known `dev` N9 failure). The timing tests (`random_hostile_lines`, `fix2`, `fix4`, `engine_never_panics`, `pipe_to_shell`) passed.

### Not done / left for others
- The "fails on `dev`" check (DoD step 5) and the workspace-wide DoD: these belong to the verification task (D-123).
- `atrium-host/src/lifecycle.rs` still spells `daemon.sock`/`daemon.lock`/`atrium.db` itself. `atrium-host` doesn't depend on `atrium-core`, so the policy's list (`Sensitive::DATA_FILES`/`PROTECTED_FILES`) is a parallel copy. Sharing one constant, for example in `atrium-protocol`, is outside this task.

## Escalation round 2 (review 3)

### What changed in the matcher (`crates/atrium-core/src/policy/deny.rs`)
- **Bytes:** matching is over bytes. A literal is its UTF-8 bytes. `?`, a negated class, an unknown `[:class:]`, and a class naming a non-ASCII character match one byte from their set (all non-ASCII bytes included) plus a `Tail` atom (zero or more non-ASCII bytes). That is a superset of "one character", and nothing is complemented approximately: `[!é]` takes every non-ASCII character. Sets are 256-bit `BSet`s.
- **Budget covers parsing:** `PathPat::parse_all`, `parse_atoms` (one step per byte) and `class_end` (one step per scanned byte) all charge the query budget. An opener with no `]` after it is a literal and is never scanned. Comparisons charge each visited state instead of the whole rectangle, and tables over 4,096 cells are hashed.
- **Three-valued answers:** `Reach::{No, Undecided, Yes}` and `GlobHit::{Hit, Undecided, Miss}`. Running out of budget is `Undecided`. `glob_hit` turns that into no hard deny, and `ask_hit` and `ask_search_glob_hit` turn it into Ask. A plain path that is itself hard-denied still hard-denies through `glob_hit`'s literal-prefix check. A per-query cache keeps answers for a pattern component against a literal name, because the same names recur across the probes.
- **`dev` floor:** a spelling holding `**` anywhere (`~/**ssh/id_rsa`) is read as `dev` read it, with globset, where `*` takes a leading dot (`PathPat::dstar`).
- **Ask-tree families at any depth:** `<tree>/**/*.session` and the other families.
- **Azure, decided by the matcher:** `SComp::Pinned(msal_token_cache.*, stem)`. `comp_intersect` tracks whether a non-wildcard atom of the pattern took a byte of the stem `msal_token_cache`. Past a `**`, the pattern asks only if it does: `ms?l_*.custom`, `*token_cache.custom`, `*e.custom` and `m*.rs` ask, while `*.rs`, `*.custom` and `*.cache` (the wildcard covers the whole stem) don't. The literal-run heuristic is gone. In the named folder, any `msal_token_cache.*` asks, as before.

### Findings and the tests that pin them (`crates/atrium-core/tests/M1-30-tree-wildcards.rs`)
- R3-1 (negated Unicode class, `/h/u/[!é]trium/secret` with data `/h/u/átrium`): `review3_finding_1_negated_unicode_classes_reach_atrium_folders`.
- R3-2 (embedded `**`, `cat ~/**ssh/id_rsa`): `review3_finding_2_embedded_globstar_keeps_devs_hard_deny`.
- R3-3 (`Search ["~"] **/foo/b.session`): `review3_finding_3_session_families_reach_ask_tree_descendants`.
- R3-4 (`Search ["~"] **/ms?l_*.custom`): `review3_finding_4_azure_cache_searches_are_decided_by_the_matcher`. Changed expectations, both with comments in the tests: `**/m*.rs` (Search and Read) now asks, because it pins the stem's `m` (`dev` asks for that search too); `**/*.cache` is now Allow, because it is ordinary by the matcher like `*.rs` (it asked before only through the removed literal-run rule). The real cache names still ask through `dev`'s probes, pinned by `**/*.bin`.
- R3-5 (32,768 unclosed `[`; also escaped and `[[:` openers): `review3_finding_5_unclosed_classes_parse_in_bounded_work`.
- R3-6 (`cat ~/aaaa…a*/notes.txt` exhausting the budget into a hard deny): `review3_finding_6_budget_exhaustion_asks_and_never_hard_denies`. It also pins that `cat ~/.gnupg/<long>*` stays hard-denied.
- R3-7 (ownership): **explained, not moved back.** A search's hard deny can only come from `Ctx::check` → `Ctx::search_glob_hit` (M1-31's). The owned functions it calls (`probes`, `hit`) take concrete paths and can't carry a pattern, and `ask_search_glob_hit` (owned) only feeds Ask. The change is now one call at the end of `search_glob_hit`, `self.s.search_filter_hit(variants)`, and all the logic sits in an owned `Sensitive` method. **Before merge, M1-31's owner (or the driver) must accept this line, and it must be recorded in `decisions.md`.** Without it, R2-1/R1-1 (`Search ["~"] .g*/private-keys-v1.d/k.key` → Allow) would reopen.
- R3-8 (stale handoff): test items 10 and 13, the escalated-findings list and verification step 5 were rewritten.

### Commands run
- `cargo fmt -p atrium-core`: clean. `cargo clippy -p atrium-core --all-targets -- -D warnings`: no warnings.
- `cargo test -p atrium-core --test M1-30-tree-wildcards`: 27 passed, 0 ignored (about 33 s).
- `cargo test -p atrium-core --no-fail-fast`: everything passes except `round_5_blocker_1_searches_and_walkers_reaching_a_folder_linked_into_ssh` (known `dev` N9). Timings: `adversarial_fuzz` 29 s, `policy` 8.7 s. The previous run had 24 s and 8.5 s on the same machine, so a slowdown can't be ruled out.
- The round-3 tests were also run against the previous commit's `deny.rs` (`e4fa939`); see the report for which fail there.

## Neon round (D-134)

Audited the unchecked Opus WIP commit (`ccce204`), kept its byte-level matcher and round-3 fixes, and closed additional edge cases, budget/performance gaps, and stale handoff text:
- **One budget per check:** `Sensitive` now owns a shared `AtomicU64` (`MATCH_BUDGET = 4_000_000`) checked out via `BudgetGuard` across all glob calls (`glob_hit`, `search_filter_hit`, `is_ssh_dir`, `ask_hit`, `ask_search_glob_hit`), and `parse_atoms` charges 4 steps/byte to bound peak memory across brace variants.
- **No write-only masking of budget exhaustion:** `ask_glob_str_hit_with` checks `specs.ask`, `specs.ssh_dir`, `specs.floor`, and read-applicable `specs.hard` (`!s.write_only`) on the single parsed pattern and shared budget instead of re-parsing and calling `glob_str_hit(..., Role::Write)`.
- **Search-filter `globset` dot semantics & brace-overflow widening:** `search_filter_hit` and `ask_search_glob_hit` pass `loose_dots = true` (matching `globset`, where `*` and `?` match leading dots), closing `Search ["~"] "*/*.session"`, `"*/b.session"`, and `"*gnupg/private-keys-v1.d/k.key"` while keeping `"*/*.rs"` `Allow`. Brace widening (`!keyed`) and post-expansion `v.contains("**")` also enable loose dot matching.
- **Strictly linear class parsing & unsupported POSIX bracket items:** `class_end` memoizes unclosed loop positions (`dead`), scans `[:name:]`/`[=x=]`/`[.x.]` without rescanning (`posix_item_end`), and `class_body` uses O(1) `const` bitmasks (`posix_class_bset` + `ascii_range`) with zero-alloc POSIX scanning (closing quadratic traps on `"[".repeat(32768) + "\\].rs"`, `"[[:".repeat(10000) + "].rs"`, `"[" + "[:".repeat(10000) + "].rs"`, and `"[" + "a".repeat(32768) + "].rs"`, and failing closed on `[=s=]` / `[.ch.]`).
- **Matcher fast paths & `Specs` caching:** thread-local `SpecsCacheEntry` cache (`Arc<Specs>`, 8-entry cap), `Seen::Small([u64; 8])` stack bitset, reused DP stacks in `Query`, zero-alloc `&str` cache keys, stack `[Atom; 64]` literal matching with first/last-byte rejection, literal-prefix fast path in `path_meets`, and exact `astay && bstay && started` handling in `comp_intersect`.

### Review 3 findings (fixed and pinned)
- **R3-1 (negated Unicode classes bypass Atrium folders):** fixed (byte-level `BSet` + `Atom::Tail`, negated/non-ASCII classes preserve `BSet::HI`), pinned by `review3_finding_1_negated_unicode_classes_reach_atrium_folders`.
- **R3-2 (embedded `**` loses `dev` hard deny):** fixed (`dstar` set for `s.contains("**") || v.contains("**")` and widened `!keyed` brace overflows), pinned by `review3_finding_2_embedded_globstar_keeps_devs_hard_deny`.
- **R3-3 (session-family shapes omit ask-tree descendants):** fixed (`SComp::Any` between ask tree and suffix glob, plus `loose_dots = true` for `Search` globs), pinned by `review3_finding_3_session_families_reach_ask_tree_descendants`.
- **R3-4 (four-literal heuristic misses Azure cache searches):** fixed (`SComp::Pinned` in `comp_intersect`), pinned by `review3_finding_4_azure_cache_searches_are_decided_by_the_matcher`.
- **R3-5 (quadratic class parsing before budget):** fixed (budgeted, strictly linear `class_end` with `dead` memoization and `posix_item_end`, plus O(n) zero-alloc `class_body` with O(1) `BSet` masks), pinned by `review3_finding_5_unclosed_classes_parse_in_bounded_work`.
- **R3-6 (budget exhaustion fabricates hard deny):** fixed (`Reach::Undecided` / `GlobHit::Undecided` -> Ask never HardDeny, one shared budget per `Sensitive`, read-only undecided check without `write_only` masking, first/last-byte & literal-prefix fast rejects), pinned by `review3_finding_6_budget_exhaustion_asks_and_never_hard_denies`.
- **R3-7 (excluded boundary in `Ctx::search_glob_hit`):** kept the 1-line `self.s.search_filter_hit(variants)` call per reviewer's instruction ("Keep the security fix"); documented here and in verification step 6 for M1-31 owner / driver sign-off and `decisions.md` entry before merge, pinned by `review2_finding_1_contained_search_globs_reach_sensitive_trees`.
- **R3-8 (stale handoff):** fixed (updated opening K8 summary line 17 to match `**/m*.rs` → Ask and `**/*.cache` → Allow, updated test inventory/verification steps, and added this section).

### Checks run (Neon, D-123)
- `cargo fmt --all`: clean (exit 0).
- `cargo check -p atrium-core --tests`: clean (0 warnings, exit 0).
- DoD verification (`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --no-fail-fast`, 3× timing loop, and `dev` failure check) left to the verification task per D-123.
