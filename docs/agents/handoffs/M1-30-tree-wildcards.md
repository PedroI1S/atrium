# Handoff: M1-30-tree-wildcards (S5 + K8)

Status: **ready for DoD verification** (D-123: worker wrote code and tests only; ran `cargo fmt --all` and `cargo check -p atrium-core --tests`).

## What changed and where

### 1. S5 completion & `deny.rs` audit / deduplication (`crates/atrium-core/src/policy/deny.rs`)
- Audited the WIP commit `0ea7c98` against `dev` (`5e693ad6`) and confirmed every diff hunk in `crates/atrium-core/src/policy/deny.rs` stays strictly inside the brief's owned range (`573..=816` on `dev`: `Sensitive::probes`, `glob_hit`, `is_ssh_dir`, `ask_hit`, `ask_search_glob_hit`, and `ShellGlob` + helper pattern-matching types/functions before `const DOWNLOADERS`). `ask_roots` and `sensitive_roots` needed no change.
- Trimmed duplication in `deny.rs` (bringing the `deny.rs` diff from `+753/-42` down to `+650/-50` even with K8 added):
  - Extracted `pat_starts_with_dot` (shared by `brace_branch_dot_ok`, `Comp::from_part`, and `comp_glob_dp`).
  - Extracted `GlobAtom` / `next_glob_atom` and unified `comp_glob_matches_no_brace` (which previously duplicated a `u64` loop and a `Vec<bool>` loop) and `glob_matches_prefix_no_brace` into one `comp_glob_matches_impl` / `comp_glob_dp` helper (using a 64-entry stack buffer for `m < 64` and heap fallback otherwise).
  - Unified `Tail::matches_tree` and `Tail::matches_strictly_under` into `Tail::matches_tree_impl(&self, tree: &str, strictly_under: bool)`.
  - Added `ShellGlob::as_tail` and a shared `matches_dir_list` closure in `glob_hit` for `self.data` / `self.protected`.
- Verified the late `**` tree-leaf fix in `Tail::matches_tree_impl`: when `reach_indices` reaches `Comp::GlobStar` before a sensitive tree's leaf, it only matches if the remaining components are all `Comp::GlobStar` (`~/.local/**`, `~/.g*/**`), so ordinary `**` globs (`cat ~/**/*.rs`, `cat ~/**/notes.txt`) are not falsely hard-denied.

### 2. K8 (`crates/atrium-core/src/policy/deny.rs`, `Sensitive::ask_search_glob_hit`)
- Fixed two causes of needless `Ask` on home-wide `**` searches such as `Search ["~"] **/*.rs` (`recursive: false`):
  - **`ASK_TREES` check:** when a directory prefix matching an ask tree ends in a generic catch-all component (`**` or `*`, e.g. `**` in `**/*.rs` or `**/*` in `**/*/*.rs`), `ask_search_glob_hit` now checks the remaining glob components (`rest`) against representative ask-tree filenames (`ASK_TREE_FILES`: `.aws/sso/cache` `0.json`/`a.json`/`x.json`/`cache.json`/`token.json`/`0`/`a`/`x`; `.zsh_sessions` and `.bash_sessions` `.session`/`.history`/`.historynew`/`.zsh`/`0`/`a`/`x`). Globs whose suffix cannot match an ask-tree file (`**/*.rs`, `**/*.md`, `**/*.toml`, `**/*/*.rs`, `*/*.rs`) return `false` (`Allow`), while `**/*.session`, `**/*/*.session`, `**/*.history`, `**/*.json`, `**/*`, `**`, `**/`, and explicit ask-tree paths (`.zsh_sessions/*.session`, `.z*/*.session`, `**/.zsh_sessions/*.txt`) still return `true` (`Ask`).
  - **`ASK_NAME_PREFIXES` check (`.azure/msal_token_cache.*`):** fixed the WIP regression where `under(rl, &dir_abs).is_some()` matched `rl == ~/.azure` unconditionally and `glob_matches_prefix("*.rs", "msal_token_cache.")` returned `true` (making `Search ["~"] **/*.rs` and `Search ["~/.azure"] *.rs` ask). For generic directory wildcards (`**` / `*`) or root `~/.azure`, `file_pat` must also match the `msal_` / `msal_token_cache.` prefix or a representative cache filename (`.json`, `.bin`, `.db`, `.txt`, `.cache`), while explicit `.az*/msal_token_cache.db` continues to ask.
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
  7. `ordinary_globs_keep_expected_verdicts`: ordinary globs (`~/Documents/*.md`, `src/*.rs`, `~/*`, `~/.config/nvim/*.json`, `~/.config/app/settings.json`, `~/Documents/[A-Z]*.md`) keep `dev`'s verdicts (`~/.config/*/settings.json` hits `.config/gcloud`, matching `cat ~/.config/*/*` on `dev`).
  8. `non_cd_fuzz_spellings_of_sensitive_paths_are_always_hard_denied`: 30,000-iteration fuzz test across all sensitive targets (excluding `cd <glob>; <reader> <file>`).
  9. `globstar_tree_leaf_rules_preserve_ordinary_globs_and_block_sensitive_trees` *(added this round)*: pins the `**` tree-leaf fix — confirms `cat ~/**/*.rs`, `cat ~/**/notes.txt`, `cat ~/.local/**/*.rs`, `cat ~/Library/**/*.md`, `cat ~/.config/**/*.rs` stay `Ask / Ask / Allow / Allow` and `Read ~/**/*.rs` stays `Allow`, while `cat ~/**`, `cat ~/.local/**`, `cat ~/.local/share/atr*/**`, `cat ~/.local/**/atrium.db`, `cat ~/**/atrium.db`, `cat ~/.g*/**`, `cat ~/**/.g*/secring.gpg`, `cat ~/.s?h/**`, `cat ~/.config/g*chrome/**`, `Read ~/.z*/**`, and `Read ~/**/.z*/*.session` remain blocked/asked.
  10. `k8_home_globstar_search_asks_only_when_suffix_can_match_ask_tree` *(added this round)*: pins K8 — non-recursive `Search ["~"]` with `**/*.rs`, `**/*.md`, `**/*.toml`, `**/*/*.rs`, `*/*.rs`, `src/**/*.rs`, `**/src/*.rs`, `**/*.{rs,toml}`, `**/m*.rs` (and `Search ["~/.azure"] *.rs`) returns `Allow / Allow / Allow`; non-recursive `Search ["~"]` with `**/*.session`, `**/*/*.session`, `**/*.history`, `**/*.json`, `**/*`, `**`, `**/`, `**/*.{rs,session}`, `**/[0-9]*.session`, `**/*token*`, `**/msal_token_cache.db`, `**/.zsh_sessions/*.txt`, `.zsh_sessions/*.session`, `.bash_sessions/*.history`, `.aws/sso/cache/*.txt`, `.z*/*.session`, `.az*/msal_token_cache.db` (and `Search ["~/.zsh_sessions"] *.rs`, `Search ["~/.azure"] msal_token_cache.db`) returns `Ask / Allow / Allow`; recursive `Search ["~"] **/*.rs` and `rg --glob '**/*.rs' x ~` remain `Ask` in Home (`rg --glob '**/*.rs' x ~/proj` is `Allow`).

## Tests expected to fail on `dev`'s code

Running the current test suite against `dev`'s `crates/atrium-core/src/policy/deny.rs` is expected to fail on:
- `crates/atrium-core/tests/adversarial_gate.rs`: `finding_11_a_glob_inside_atriums_folder_names_is_blocked` (wildcarded `.g*` and `atr*` components are not hard-denied on `dev`).
- `crates/atrium-core/tests/M1-30-tree-wildcards.rs`:
  - `wildcards_in_each_component_of_cred_trees_and_files`
  - `wildcards_in_ssh_dir_keep_plain_ssh_verdicts`
  - `wildcards_in_browser_roots`
  - `wildcards_in_atrium_data_and_protected_folders`
  - `wildcards_in_ask_stores_and_trees`
  - `relative_paths_from_subdir_and_file_tools_reads_and_searches`
  - `non_cd_fuzz_spellings_of_sensitive_paths_are_always_hard_denied`
  - `globstar_tree_leaf_rules_preserve_ordinary_globs_and_block_sensitive_trees` (sensitive `**` with wildcarded tree names like `cat ~/.g*/**`, `cat ~/.local/share/atr*/**`, `cat ~/**/.g*/secring.gpg`, `Read ~/.z*/**`)
  - `k8_home_globstar_search_asks_only_when_suffix_can_match_ask_tree` (`dev` returns `Ask` instead of `Allow` in Home for `Search ["~"] **/*.rs` and the other non-recursive `**/*.{md,toml,...}` home searches; `.az*/msal_token_cache.db` returns `Allow` instead of `Ask` on `dev`).
- Note: `ordinary_globs_keep_expected_verdicts` is expected to **pass** on `dev` (verifying no regressions on ordinary globs).

## Escalated / outside owned paths

- **`crates/atrium-core/tests/adversarial_fuzz.rs` (`random_spellings_of_sensitive_paths_are_always_hard_denied`, line 422):** still `#[ignore = "M1-13 finding 11"]`. Its `shape_pick == 8` generates `cd <glob>; <reader> <file>` (e.g. `cd ~/.a?s; cat credentials`, `cd ~/.s?h; cat id_rsa`). In `crates/atrium-core/src/policy/words.rs:cd_effect` (line 628, outside M1-30's owned paths), any `cd` target containing `*`, `?`, or `[` returns `None` (`Cd::Lost`), so the following `<reader> <file>` does not know `cwd` may be inside the wildcarded directory. Per the brief's rule ("If the fix needs a change elsewhere, stop that part and say so in the handoff: don't make it"), `words.rs` was left untouched and the `#[ignore]` on `adversarial_fuzz.rs:422` was kept.

## Checks run by this worker (D-123)

- `cargo fmt --all`: exited 0.
- `cargo check -p atrium-core --tests`: exited 0 (7.45s, 0 warnings).

## Commands for the verification task (DoD)

1. `cargo fmt --all --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test --workspace --no-fail-fast`
4. Run `cargo test -p atrium-core` 3 times in a loop and record timings for `adversarial_fuzz` and `policy` (no slowdown).
5. Checkout `dev:crates/atrium-core/src/policy/deny.rs`, run `cargo test -p atrium-core --test M1-30-tree-wildcards --test adversarial_gate`, confirm the S5/K8 tests fail on `dev`, then restore `HEAD`'s `deny.rs` and re-verify pass.
6. Confirm `git diff dev --stat` / `git diff 5e693ad6..HEAD --stat` touches only owned paths (`crates/atrium-core/src/policy/deny.rs`, `crates/atrium-core/tests/M1-30-tree-wildcards.rs`, `crates/atrium-core/tests/adversarial_gate.rs`, `docs/agents/handoffs/M1-30-tree-wildcards.md`).
