# Handoff: M1-38-rev4-p2-paths (P2)

- **Branch:** `agent/neon/M1-38-rev4-p2-paths` (branched from P0's tip `1a09705`)
- **Model:** Gemini 4 Neon (High)
- **Contract:** `docs/contracts/m1-policy-rev4.md` (draft 5) — §2 (filesystem caps & walk-cap rule), §3 (process-dependent paths, `~`, redirect targets), §5 (paths, globs, links, recursive reads, `cd`, intra-call writer effects), §8 (budgets), §10–§12
- **Behaviour change:** None. `SessionGate` is not wired yet (that is P5), and no P0 interface file (`model.rs`, `limits.rs`, `tools.rs`, other `v4/` stubs) or `dev` file was modified.

---

## 1. What was implemented, by contract section

All P2 code lives under `crates/atrium-core/src/policy/v4/paths.rs` and `crates/atrium-core/src/policy/v4/paths/`:

### §2 & §8 — Filesystem resolvers, caps, and budgets (`v4/paths/resolver.rs`)
- **`FsResolver` (`PathResolver` for the real filesystem):** wraps `std::fs::metadata`, `symlink_metadata`, `read_link`, `canonicalize`, and `read_dir` on raw Unix bytes (`OsStrExt`). Precharges `FsCaps` (`stat`, `link_read`, `canonicalize`, `dir_open`, per-entry `entries_read`) and `ComputeBudget` (work units + allocation bytes) **before** executing each operation or pushing into returned buffers. Maps `ENOENT` strictly to `ResolveError::NotFound`, distinct from `ResolveError::Io(FsIoError)` (`PermissionDenied`, `NotADirectory`, `NotASymlink`, `SymlinkLoop` via `Errno::LOOP`, `Other`).
- **`FakeFs` (`#[cfg(test)]` in `resolver.rs`):** deterministic in-memory POSIX-like `PathResolver` supporting regular files, directories, symlinks, dangling links, symlink loops, hard links (`nlink`), unreadable directories (`read_error`/`exec_error`), per-inode `stat_error`, case-insensitive volumes, NFC/NFD-equivalent volumes, and non-UTF-8 entry names, with the same `FsCaps` + `ComputeBudget` precharging.
- **Cap vs. budget separation:** reaching any `FsCaps` counter or walk cap returns `Unread(WalkCapReached(FsCap))` (never `Exhausted` and never a fabricated hit); running out of `ComputeBudget` (compute or allocation bytes) propagates `Err(Exhausted)`.

### §3 & §5 — Literal path resolution (`v4/paths/resolve.rs`, `v4::paths::resolve_literal`)
- Modeled `~` expansion (`~` and `~/...` only; `~user` → `Unread(UnsupportedTildeForm)`), respecting `Word` provenance in `resolve_word_literal` so quoted/escaped leading `~` stays literal.
- Process-dependent path detection (`is_process_dependent_path`, stripping macOS `/System/Volumes/Data` firmlink first): `/proc/self`, `/proc/thread-self`, `/proc/<pid>`, `/dev/fd/`, `/dev/stdin`, `/dev/stdout`, `/dev/stderr` → `Unread(ProcessDependentPath)` directly, via `..`, or via any symlink hop before OS canonicalization can resolve them in the gate's own process. macOS `/.vol` → `Unread(UnresolvablePath)`.
- Ordered component-by-component resolution: follows symlinks up to `limits::MAX_HOPS` (40) before applying `..` (even on dangling tails), tracks whether `missing_tail` was entered while expanding a symlink target (`dangling_symlink`), canonicalizes the deepest existing ancestor for volume spelling, collects `(dev, ino)` identities of the resolved path and all existing ancestors (plus all symlink identities along a dangling chain), and propagates `multiply_linked` for existing regular files with `nlink > 1`. Unreadable directories, `MAX_HOPS` loops, and non-absence I/O failures produce `Unread(UnresolvablePath)`.

### §5 & §11 item 13 — Names, folds, unlisted equivalences, and shared `Sensitive` (`v4/paths/sensitive.rs`, `v4::paths::check_target`)
- **Identity + listed folds:** `check_target` checks `(dev, ino)` identities of the target and all existing ancestors against `ProtectedSet::hard_deny_identities` / `store_identities`, then checks listed folds (`FoldRules::REV4`: Unicode `str::to_lowercase()`, `ſ` → `s`, `K` → `k`, no Unicode normalization, stripping `/System/Volumes/Data`) against `CRED_FILES`, `CRED_TREES`, `~/.ssh` (preserving `.pub`/`known_hosts*` and read-only `config`/`authorized_keys` exceptions, blocking `~/.ssh` writes), `BROWSER_ROOTS` × `BROWSER_FILES`, `atrium_data_paths`, write-only `atrium_protected_paths`, `ssh_linked_dirs`, and CR-12 + CR-17 (`ASK_FILES`, `ASK_TREES`, `ASK_NAME_PREFIXES`).
- **Unlisted equivalences (§11 item 13):** if neither identity nor listed folds hit, checks `fold_path_extended` (NFD canonical decomposition + canonical combining-class ordering, unlisted Unicode case folds `ß`/`ẞ`→`ss`, `İ`/`ı`→`i`, `ς`→`σ`, `µ`→`μ`, ligatures `ﬀ`–`ﬆ`, and APFS default-ignorable code points). A match there yields `Unread(UnlistedNameEquivalence)` and never downgrades a listed-fold or identity hit.
- **Over-approximation guard:** `check_target` on a `TargetOrigin::ExpandedFrom { variant_only: true, .. }` target always returns `Unread(VariantOnlyGlobMatch)`, never a hit.
- **Shared `Sensitive` (M1-31 N11):** `SharedSensitive::for_protected` caches folded `ProtectedSet` state per thread keyed strictly by value on the folded home, Atrium data/protected, `ssh_linked_dirs`, and `FoldRules`.
- **`build_protected_set`:** populates `hard_deny_identities`, `store_identities`, and `ssh_linked_dirs` through `PathResolver` (including dangling `~/.ssh` symlink identities per M1-32 rework 4 finding 2, keeping `~/.ssh/<subdir>/key.pub` exempt while protecting subdir private files by identity).

### §3 & §5 — Shell glob expansion (`v4/paths/glob.rs`, `v4::paths::expand_shell_glob`)
- Provenance-aware parsing: only `ByteProvenance::Unquoted` `*`, `?`, `[...]` expand; quoted, escaped, and `Derived` bytes stay literal (so resolved symlink names containing `[1]` are never re-parsed as globs; §10 rows 8–9).
- Rejects unquoted `!` or `^` inside `[...]` as `Unread(NegatedGlobClass)` (§3). Supports the 14 POSIX `[:name:]` classes (`[:alpha:]`, `[:lower:]`, `[:digit:]`, etc.) without breaking on the inner `]`; rejects locale-dependent `[=...=]`/`[....]` as `Unread(VariantOnlyGlobMatch)`.
- Budgeted O(|atoms| × |name|) two-row DP component matcher (charged before execution).
- Separates intersection matches (`sh`/`bash`/`zsh` defaults: no dotglob, `**` as single-component `*`) in `exact_matches` from variant-only matches (dotfiles when no explicit leading `.`, 0-level and 2+-level `**` including symlinked dir descent) in `variant_only_matches`.
- Enforces `limits::GLOB_MAX_DEPTH` (12) and `limits::WALK_MAX_ENTRIES` (4,096) as `Unread(WalkCapReached)`.
- No-match fallback: when `exact_paths` is empty and no cap/I/O unread occurred, checks the literal word via `resolve_word_literal` (even if a dialect variant matched dotfiles or recursive `**`).

### §1 step 4, §2, §5, §11 items 7/11/12/15 — Recursive reads, S8, filters, followed links, and hard links (`v4/paths/recursive.rs`, `v4::paths::check_recursive_root`)
- **S8 protected-root policy (§11 item 12):** checks whether an existing hard-deny path lives under the root (Atrium data paths inside or outside home, `ssh_linked_dirs`, `CRED_FILES`, `CRED_TREES`, existing private `~/.ssh` entries, and existing `BROWSER_FILES` inside browser roots/profile directories while keeping ordinary files like `Default/Preferences` clear). Exempts `home`, ancestors of `home`, and `~/.config` (D-133), which fall through to `StoreHit` (`Ask / Allow / Allow / Allow` per §10 row 16).
- **Search filters (§5):** supports single `SearchGlob`, `RgGlob`, and `GrepInclude` filters; rejects multiple filters, negated `!` filters, and `..` in filters as `Unread(UnsupportedFilterForm)`. Step 1 enumerates selected entries within `WALK_MAX_ENTRIES`/`WALK_MAX_DEPTH` and resolves/checks each (following symlinked intermediate dirs for `SearchGlob`). Step 2 matches against `dev`'s hard-deny probes (`HitSource::BestEffort`) and store probes (`HitSource::Model`, plus `ASK_TREES` directory-prefix matching), independently of Step 1.
- **Followed-link walk (§5, §10 row 15, §11 item 15 D-139):** when `links.below_root`, walks links within budget (cycle-guarded by canonical path and `(dev, ino)`). A protected link target (or linked S8/store root) is a hit without a filter and `Unread(FollowedLinkUnderFilter)` with one; reaching a walk cap is `Unread(WalkCapReached)`; completing with no protected link is `Clear`. Propagates `multiply_linked_file` for followed hard-linked regular files.
- **Bounded hard-link walk (§2, §5, §11 item 7):** exact `dev` bounds (`HARD_LINK_WALK_ENTRIES = 4,096`, `HARD_LINK_WALK_DEPTH = 8`, skipping `node_modules` and `.git`), setting `multiply_linked_file` on regular files with `nlink > 1` (and catching hard links to protected identities when unfiltered), counting as clear when the local walk cutoff runs out.

### §5 — Command targets, `cd`, writer effects, and file/search subjects (`v4/paths/command.rs`, `v4::paths::resolve_command_targets`, `v4::paths::resolve_subject_paths`)
- **`resolve_command_targets`:** uses `ToolEffects` roles, `operand_access`, `writes`, `derived_destinations` (`Always` and filesystem-resolved `WhenSourceIsDir` for `mv` of a directory), `reads_recursively`, `links`, `filters`, and `cd_target_operand`.
  - Tracks `cd` to a literal existing directory; any non-literal `cd`, `cd -`, `cd` in a pipeline, `cd` after a possible writer, `cd` with `unread`, or `cd` to a nonexistent/non-directory target switches cwd to `Unknown` (emitting `Unread(UnknownCwd)` on later units while still resolving absolute/`~` paths for hard-deny checks).
  - Pre-scans pipelines so every stage of a pipeline containing a possible writer (including earlier stages; §10 row 21) gets `Unread(PipelineWithPossibleWriter)`, and every unit running after a possible writer gets `Unread(AfterPossibleWriter)`, while keeping hard-deny hits. Output redirects to `/dev/null` do not mark a unit a writer (§10 row 22).
  - Enforces `limits::MAX_PATHS` → `TooLarge(Cap::Paths)`.
- **`resolve_subject_paths`:** treats all `paths` as strictly literal (§10 row 11), enforces `MAX_USES`/`MAX_PATHS`, splits `glob` into selectors (leading `/`, `~`, or `..` component → shell-glob expansion + selector probe checks + D-100 escaping `Unread`) vs. `SearchGlob` filters (two-step selected-entry + probe check), and runs `check_recursive_root` when `recursive == true`.

---

## 2. Disposition list for carried hunks and tests (contract §12)

### A. `agent/hydrogen/M1-29-cr17-stores`

#### Hunks
| Source hunk | Disposition | Notes |
|---|---|---|
| `deny.rs` `ASK_FILES`: `.gem/credentials`, `.terraform.d/credentials.tfrc.json`, `.config/op/config`, `.yarnrc.yml` | **Kept (ported to P2)** | Added to `v4/paths/sensitive.rs::ASK_FILES` with CR-17 family labels, probes, and `build_protected_set` identity tracking. |
| `gate.rs` `known_ask_ids` multiply-linked exemption for remembered file-tool hard links | **Dropped (contract §1 step 4)** | Rev 4 §1 step 4 keeps the multiply-linked regular file Ask in every setting unchanged ("The multiply-linked-file Ask" in §11). |
| `engine.rs` (`GIT_CRED_*`, path scopes, derived-unit rules), `rule_check.rs`, `daemon/commands.rs`, `Security.tsx`, `rule-form.ts` | **Out of P2 scope (P3/P4)** | Owned by P3/P4 per contract §12–§13. |

#### Tests (`crates/atrium-core/tests/M1-29-cr17-stores.rs`)
| Source test | Disposition | Rev 4 verdict / citation |
|---|---|---|
| `cr17_stores_ask_in_home_and_keep_allow_under_rules_and_full_access` | **Kept (migrated in P2)** | `StoreHit` in `carried_m1_29_cr17_four_stores_direct_symlink_hardlink_and_recursive` (§1, §2, §11 D-111). |
| `cr17_ordinary_siblings_keep_dev_verdict` | **Kept (migrated in P2)** | `Clear` for `~/.gem/ordinary.txt`, `~/.terraform.d/plugins.json`. |
| `cr17_gate_catches_symlinks_and_hardlinks_to_new_stores` | **Kept (migrated in P2)** | `StoreHit` on symlinks and hard links (with `multiply_linked_file == true` on hard link; §1 step 4, §5). |
| `round3_finding8_and_cr17_test_gaps` (CR-17 folder search cases) | **Kept (migrated in P2)** | Recursive search on `~/.gem`, `~/.terraform.d`, `~/.config/op` → `StoreHit` (never `HardDenyHit`). |
| All other 18 tests in `M1-29-cr17-stores.rs` (path scopes, `find -exec`/`-delete`, `xargs`, `pwsh`, `sed`, `git`, `stdin`, `round5_finding4_gate_hardlink_remembered_rule`) | **Out of P2 scope / dropped for P2** | Rule validation, path scopes, and Git selection belong to P3 (§12); `round5_finding4` dropped per §1 step 4. |

### B. `agent/hydrogen/M1-30-tree-wildcards`

#### Hunks
| Source hunk | Disposition | Notes |
|---|---|---|
| `deny.rs` replacement `ShellGlob` / byte-level pattern-vs-pattern matcher, variant-widening, class-complement, `Specs` | **Dropped** | Explicitly dropped by contract §5 & §12 ("No pattern-versus-pattern reasoning"). |
| `deny.rs` K8 store probes (`*.session`, `*.history`, `*.historynew` under `.zsh_sessions`/`.bash_sessions`) and `msal_token_cache.*` probes | **Rewritten in P2** | Kept as probe entries in `SharedSensitive::store_probes()` and `hard_deny_probes()` (`v4/paths/sensitive.rs`), matched in `check_filter_on_root` / `check_selector_probes`. |

#### Tests (`crates/atrium-core/tests/M1-30-tree-wildcards.rs`)
| Source test | Disposition | Rev 4 verdict / citation |
|---|---|---|
| `wildcards_in_each_component_of_cred_trees_and_files` | **Migrated (split)** | Absent literal paths → `HardDenyHit` (`carried_m1_30_absent_targets_and_search_filter_probes`); shell-glob wildcards over existing trees → `HardDenyHit` via `expand_shell_glob`; unquoted `{...}` braces → `Unread` (§3, §11 item 2); pattern-vs-pattern absent shell globs covered by P5's best-effort `glob_hit` (§2). |
| `wildcards_in_ssh_dir_keep_plain_ssh_verdicts` | **Migrated (new verdict where changed)** | Existing `~/.ssh/id_*` → `HardDenyHit`; `.pub`/known_hosts/read-only `config`/`authorized_keys` → `Clear`; empty `~/.ssh` literal fallback `~/.ssh/id_*` → `HardDenyHit` (§5). |
| `wildcards_in_browser_roots` | **Migrated** | Browser credential files (`Cookies`, `Login Data`, etc.) → `HardDenyHit`; ordinary files (`Preferences`, `README.md`) → `Clear` (§5). |
| `wildcards_in_atrium_data_and_protected_folders` | **Migrated** | Absent `~/.local/share/atrium/*` → `HardDenyHit("atrium-self")`; `~/.config/atrium/*` Write → `HardDenyHit("atrium-self")`, Read → `Clear` (`carried_m1_30_absent_targets_and_search_filter_probes`). |
| `wildcards_in_ask_stores_and_trees` | **Migrated** | Search filter `msal*` on `~/.azure` (present and absent) → `StoreHit` (§10 rows 12 & 24); ordinary siblings → `Clear`. |
| `relative_paths_from_subdir_and_file_tools_reads_and_searches` | **Migrated** | Escaping selectors `../.ssh/id_*` → `HardDenyHit`, `../.aws/credentia?s` (absent) → `HardDenyHit(BestEffort)`, `../other.txt` → `Unread` (D-100; §5). |
| `ordinary_globs_keep_expected_verdicts` | **Migrated (new verdict for `~` `**/*.rs`)** | `Search ~ **/*.rs` → `StoreHit` (no HardDeny; `dev`'s retained `ASK_TREES` prefix probe matches `**` against `.aws/sso/cache`, §5 & §11); ordinary literal/glob targets → `Clear`. |
| `k8_home_globstar_search_asks_only_when_suffix_can_match_ask_tree` | **Migrated with rev 4 verdict** | `Search ~ **/*.rs` → `StoreHit` (no HardDeny; M1-30's suffix-reasoning matcher is dropped per §5 & §12, so `dev`'s retained `ASK_TREES` directory-prefix probe stands); `**/*.session` → `StoreHit`; `**/*.json` → `HardDenyHit` (`carried_m1_30_absent_targets_and_search_filter_probes`, §5, §11). |
| `session_gate_blocks_wildcarded_credential_and_atrium_trees_when_absent_on_disk` | **Migrated (new verdict for absent globs)** | Absent literal paths → `HardDenyHit`; absent `Search ~/.aws credentia?s` → `HardDenyHit(BestEffort)` (§2, §5); absent shell-glob probes belong to P5 best-effort (§2). |
| `review2_finding_1_contained_search_globs_reach_sensitive_trees` | **Kept (migrated in P2)** | `Search ~/.aws credentia?s`, `Search ~ **/*.json` → `HardDenyHit` (§2, §5). |
| `posix_named_classes_match_real_character_sets` | **Kept (migrated in P2)** | `[[:alpha:]]` in `expand_shell_glob` (`regression_posix_classes_...`, §3, §5). |
| `review2_finding_4_session_families_ask_for_read_paths`, `review2_finding_6_*`, `review3_finding_3_*`, `review3_finding_4_*` | **Migrated in P2** | `ASK_TREES` dir-prefix probe (`.zsh_sessions/*.foo`) and `msal*` probe → `StoreHit` (§5). |
| `review3_finding_1_negated_unicode_classes_reach_atrium_folders` | **Migrated with new verdict** | Unquoted `[!…]` and non-ASCII unquoted bytes are `Unread(NegatedGlobClass)` / `Unread(UnquotedByteOutsideModel)` (§3, §10 row 10); `Read` literal paths stay literal (§10 row 11). |
| `review3_finding_5_unclosed_classes_parse_in_bounded_work`, `review3_finding_6_budget_exhaustion_asks_and_never_hard_denies` | **Migrated in P2** | Budgeted linear parsing; `ComputeBudget` exhaustion returns `Err(Exhausted)` → Ask everywhere, never a hit (`boundary_walk_caps_are_unread_while_compute_budget_is_exhausted`, §8). |
| `non_cd_fuzz_spellings_*`, `globstar_tree_leaf_rules_*`, `rework_round2_regressions`, `review2_finding_2/3/5/7`, `review3_finding_2` | **Dropped (matcher dropped)** | Tested M1-30's dropped pattern-vs-pattern `ShellGlob` matcher and brace/variant-widening heuristics (§5, §12). |

### C. `agent/hydrogen/M1-31-credential-folder-search`

#### Hunks
| Source hunk | Disposition | Notes |
|---|---|---|
| `deny.rs` S8 protected-root verdicts (recursive root containing existing hard-deny path → HardDeny regardless of filter; home / ancestors of home / `~/.config` exempt; Atrium data paths outside home covered; browser profile folder vs ordinary file distinction) | **Kept (rewritten on `PathResolver` in P2)** | Implemented in `v4/paths/recursive.rs::check_s8_protected_root` using real/fake existence checks (`lstat`/`read_dir`) instead of purely lexical checks. |
| `deny.rs` N11 `Sensitive::shared` thread-local cache keyed strictly by value on folded strings | **Kept (ported to P2)** | Implemented in `v4/paths/sensitive.rs::SharedSensitive::for_protected`. |
| `deny.rs` root parser (`parse_search_opts`, `git_grep_roots`, `recursive_search_roots`) and K10 glob normalizer (`NormArena`, `normalize_glob_variants`) | **Dropped** | Explicitly dropped by contract §12 (replaced by P3's tool table and P2's filesystem-based expansion). |
| `words.rs` brace expansion, `shell.rs` lexer memo, `engine.rs` `glob_work_exhausted` | **Out of P2 scope (P1/P5)** | Assigned to P1/P5 per contract §12. |

#### Tests (`crates/atrium-core/tests/M1-31-credential-folder-search.rs`)
| Source test | Disposition | Rev 4 verdict / citation |
|---|---|---|
| `recursive_search_tools_block_every_hard_denied_folder_and_ancestors` | **Kept (migrated in P2)** | Existing hard-deny under root (`~/.aws`, `/opt` for Atrium data outside home) → `HardDenyHit`, even with non-matching filter (`carried_m1_31_s8_...`, §5, §10 row 16, §11 item 12). |
| `paths_subject_recursive_blocks_hard_denied_folders_and_keeps_non_recursive` | **Kept (migrated in P2)** | Recursive `ToolSubject::Paths` hits S8; non-recursive stays Clear unless target itself hits (§5, §11 item 12). |
| `cr12_stores_keep_asking_in_home` | **Kept (migrated in P2)** | Exempt roots (`~`, `~/.config`) and CR-12/CR-17 parent folders → `StoreHit`, never `HardDenyHit` (§10 row 16, D-133). |
| `non_recursive_reads_and_ordinary_searches_keep_devs_verdict` | **Kept (migrated in P2)** | Ordinary reads and searches → `Clear`. |
| `gate_blocks_symlinks_into_credential_folders` | **Kept (migrated in P2)** | Symlinks into credential folders resolved in `resolve_literal` and `walk_followed_links` → `HardDenyHit` (§5). |
| `review1_f1_sensitive_cache_is_keyed_by_the_strings_it_uses` | **Kept (migrated in P2)** | Tested directly via `Rc::ptr_eq` on `SharedSensitive::for_protected` (`carried_m1_31_s8_...`). |
| `review1_f4_ordinary_browser_profile_files_keep_devs_verdict` | **Kept (migrated in P2)** | `Default/Preferences` → `Clear`; `Default` folder with `Cookies` → `HardDenyHit` (`carried_m1_31_s8_...`). |
| `special_spellings_wrappers_brace_expansion_and_implicit_cwd`, `review1_f2_globbed_recursive_roots_*`, `review1_f5_*`, `review1_f6_*`..`review1_f10_*`, `k10_*` | **Dropped / P1/P3 scope** | Tested dropped K10 glob normalizer, root flag parser (P3), or brace expansion (P1) (§12). |

### D. `agent/hydrogen/M1-32-link-globs`

#### Hunks
| Source hunk | Disposition | Notes |
|---|---|---|
| `gate.rs` link-glob expansion code (`reaches_tree_through_links`, `expand_glob_with`, `walker_reads_operands`, `prune_mode`) | **Dropped** | Explicitly dropped by contract §12; archive operand roles belong to P3. |
| Obligations: ordered resolution (`real_path`), dangling targets, dangling `~/.ssh` identities, I/O uncertainty (`ResolveScope`), identity comparison, non-UTF-8 byte paths, literal `Read` paths, `rg -L` cap → Unread & filtered-link → Unread, single shared budget | **Rewritten cleanly in P2** | Built directly into `resolve.rs`, `glob.rs`, `sensitive.rs`, `recursive.rs`, and `command.rs` behind `PathResolver` + `FsCaps` + `ComputeBudget`. |

#### Tests (`crates/atrium-core/tests/M1-32-link-globs.rs`)
| Source test | Disposition | Rev 4 verdict / citation |
|---|---|---|
| `s11_globs_through_links_match_direct_verdicts` | **Migrated (with rev 4 verdicts)** | Single/multi-hop symlink globs in intersection → `HardDenyHit` (§10 row 8); dotfile / 2+-level `**` matches → `Unread(VariantOnlyGlobMatch)` (§5, §11 item 10); `{a,b}` braces → `Unread` (§3). |
| `s11_file_tool_globs_through_links_are_blocked` | **Kept (migrated in P2)** | `Glob(paths=["proj"], glob="key*")` and `sub_ssh/*` → `HardDenyHit` (§10 row 23). |
| `s11_ordinary_searches_and_globs_with_ordinary_links_keep_dev_verdicts` | **Kept (migrated in P2; item 15)** | Ordinary followed-link search `rg -L x proj` with no protected link within budget → `Clear` (§10 row 15, §11 item 15 D-139). |
| `rework1_finding1_file_tool_globs_match_dotfiles_and_recursive_basenames` | **Kept (migrated in P2)** | `SearchGlob` filter matches dotfiles and recursive basenames in `check_filter_on_root`. |
| `rework1_finding2_dangling_links_to_absent_credential_files_are_denied` | **Kept (migrated in P2)** | Dangling 1-hop and 2-hop symlinks to absent `~/.ssh/missing_key` → `HardDenyHit` (`boundary_absent_and_dangling_targets_and_ordered_dotdot`). |
| `rework1_finding3_dangling_targets_resolve_links_before_dotdot`, `rework2_finding5_*`, `added_scope_resolve_fs_follows_links_before_dotdot` | **Kept (migrated in P2)** | `proj/link_sub/../new_key` resolves symlink before `..` → `HardDenyHit`. |
| `rework1_finding5_public_ssh_and_readable_config_keep_ordinary_verdicts` | **Kept (migrated in P2)** | `~/.ssh/sub/key.pub` → `Clear`; `.pub`/known_hosts/read-only `config`/`authorized_keys` preserved. |
| `rework1_finding6_unreadable_glob_dir_fails_closed_to_ask`, `rework2_finding4_*`, `rework4_finding4_*` | **Kept (migrated in P2)** | Unreadable directories produce `Unread(UnresolvablePath)` without hiding a `HardDenyHit` on another operand (`boundary_unreadable_folder_...`). |
| `rework2_over_budget_glob_and_non_ssh_id_rsa_do_not_hard_deny`, `rework3_finding5_*`, `rework4_finding5_*` | **Kept (migrated in P2)** | Reaching `FsCaps`/`WALK_MAX_ENTRIES` produces `Unread(WalkCapReached)`, never `HardDenyHit` (§2, §5, §11 item 11); `ComputeBudget` produces `Exhausted` (§8). |
| `rework2_finding1_*`, `rework3_finding1_*`, `rework3_finding3_*`, `added_scope_hard_links_*` | **Kept (migrated in P2)** | `(dev, ino)` identity checked on path and existing ancestors, plus bounded hard-link walk (`boundary_hard_links_...`). |
| `rework2_finding2_non_utf8_link_names_*`, `rework3_finding2_non_utf8_glob_prefix_*` | **Kept (migrated in P2)** | Non-UTF-8 byte symlink `proj/bad\xfflink` resolved by raw bytes → `HardDenyHit` (`carried_m1_32_...`). |
| `rework3_finding4_literal_file_tool_path_with_brackets_is_resolved`, `rework4_finding3_literal_read_paths_are_not_globs` | **Kept (migrated in P2)** | `Read /h/u/[!é]trium/secret` stays literal (§10 row 11). |
| `rework4_finding1_metacharacters_from_a_resolved_link_stay_literal` | **Kept (migrated in P2)** | `proj -> store[1]`, `store[1]/key -> ../.ssh/id_rsa` (§10 row 8). |
| `rework4_finding2_dangling_ssh_members_are_in_the_credential_set` | **Kept (migrated in P2)** | Dangling symlink inside `~/.ssh` retains its own identity (including 2-hop chains) → `HardDenyHit`. |
| `s11_unbounded_brace_globs_*`, `rework1_finding4_*`, `rework2_finding3_*`, `added_scope_non_reading_walks_*`, `rework4_finding6_archive_listing_*` | **Dropped / P1/P3 scope** | Braces are Unread in P1 (§3); tool recursion/link/archive operand roles live in P3's tool table (§6, §12). |

---

## 3. What was reused from `dev`'s evaluator

Per contract §13, `dev`'s evaluator (`deny.rs`, `gate.rs`, `engine.rs`) remains live and untouched until P5, so P2 could not edit or delete `dev` internals. Within P2's own modules, we preserved exact parity with `dev`'s constants and rules:
- **Path & sensitive lists (`deny.rs`):** `CRED_FILES`, `CRED_TREES`, `BROWSER_ROOTS`, `BROWSER_FILES`, `SSH_PROBES`, `BROWSER_FILE_PROBES`, `ASK_FILES` (+ M1-29 CR-17), `ASK_TREES`, `ASK_NAME_PREFIXES`, `~/.ssh` public/readable-config exceptions, `/System/Volumes/Data` firmlink stripping, and `ASK_TREES` directory-prefix matching.
- **Frozen limits (`policy/limits.rs`, matching `dev`):** `MAX_HOPS` (40), `MAX_USES` (4,096), `MAX_PATHS` (16,384), `WALK_MAX_ENTRIES` (4,096), `WALK_MAX_DEPTH` (16), `GLOB_MAX_DEPTH` (12), `HARD_LINK_WALK_ENTRIES` (4,096), `HARD_LINK_WALK_DEPTH` (8), `HARD_LINK_WALK_SKIP_DIRS` (`["node_modules", ".git"]`).
- **Resolution & hard-link semantics (`gate.rs`):** deepest-existing-ancestor canonicalization, `/.vol` rejection, `~/.ssh` member walk (`SSH_MAX_ENTRIES = 1,024`, `SSH_MAX_DEPTH = 8`) with `ssh_linked_dirs`, and the bounded hard-link walk returning `multiply_linked_file`.
- **Filter matching (`globset`):** `GlobBuilder::new(...).literal_separator(true)` as used by `dev`.

---

## 4. Change requests against P0's interface

### CR-P2-1: Add `write_only_hard_deny_identities` to `ProtectedSet` (`crates/atrium-core/src/policy/model.rs`)
- **Exact field:**
  ```rust
  /// Identities `(dev, ino)` of existing write-only protected files/trees
  /// (`~/.ssh/config`, `~/.ssh/authorized_keys`, and `atrium_protected_paths`
  /// plus their member files), mapped to their `HARD_DENY_DEFAULTS` id
  /// (`"credential-read"` / `"atrium-self"`). Checked only when
  /// `target.access != AccessMode::Read` (contract §5).
  pub(crate) write_only_hard_deny_identities: Vec<(FileIdentity, &'static str)>,
  ```
- **Why:** `v4::paths::check_target(target: &Target, protected: &ProtectedSet, budget: &mut ComputeBudget)` is pure (it does not accept a `PathResolver`), and `ProtectedSet::hard_deny_identities` applies unconditionally to both reads and writes. Write-only protected paths (`~/.ssh/config`, `~/.ssh/authorized_keys`, `atrium_protected_paths`) cannot be placed in `hard_deny_identities` without causing false hard-denies on `AccessMode::Read`.
- **Current status & what still fails until this lands:**
  - All resolver-backed P2 entry points (`resolve_command_targets`, `resolve_subject_paths`, and `check_recursive_root`) **pass today** by lazily collecting write-only identities via `sensitive::collect_write_only_identities` on non-read targets (`round2_finding07_write_only_hard_link_aliases_in_resolver_flows`).
  - **Still fails until CR-P2-1 lands:** a standalone call to `check_target(&target, &protected, &mut budget)` (without going through `resolve_command_targets` / `resolve_subject_paths` / `check_recursive_root`) on a `Target` with `AccessMode::Write` or `AccessMode::ReadWrite` pointing to a hard-linked alias of `~/.ssh/config`, `~/.ssh/authorized_keys`, or a file under `atrium_protected_paths` (e.g., `proj/alias_cfg` hard-linked to `~/.ssh/config`) returns `Clear` instead of `HardDenyHit`, because `ProtectedSet` cannot carry write-only identities to the pure `check_target` function.

---

## 5. Verification commands

Run by worker (allowed under D-123):
- `cargo fmt --all` — clean
- `cargo check -p atrium-core --tests` — clean (0 errors, 0 warnings)
- `cargo clippy -p atrium-core --tests -- -D warnings` — clean (0 errors, 0 warnings)

Commands for the orchestrator's verification task (fish-compatible):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
```

---

## 6. Round 2 (rework against `.orchestration/review-input/review-round1.md`)

All 14 findings from Codex review round 1 have been addressed on top of P0 tip `a1f0174`:

| # | Finding | Fix (file) | Regression test (`p2_tests.rs`) |
|---|---|---|---|
| 1 | Later exhaustion discards earlier hits | Added `finish_call_exhausted` / `finish_recursive_exhausted` and per-root immediate checking so `resolve_command_targets`, `resolve_subject_paths`, and `check_recursive_root` retain all partial outcomes and append `PartOutcome::Exhausted` when subsequent work exhausts `ComputeBudget` (`command.rs`, `recursive.rs`). | `round2_finding01_hit_preserved_after_later_exhaustion` |
| 2 | Hard-link walk bypasses S8 exemptions (`section10_row_16` failure) | Restricted identity hits in `walk_hard_links` to `md.nlink > 1 && !has_filter && !exempt_root` while still setting `multiply_linked_file` for any `nlink > 1` regular file (`recursive.rs`). | `section10_row_16_s8_aws_hard_deny_config_and_home_exempt`, `round2_finding02_exempt_root_identity_walk_preserves_s8_exemptions` |
| 3 | Filters silently exclude non-UTF-8 names | Updated `CompiledFilter::matches_entry` to match raw filesystem bytes via `globset::GlobMatcher::is_match(&Path)` (`OsStrExt::from_bytes` on Unix), returning `Unread(UnresolvablePath)` on non-Unix encoding failures instead of `false` (`recursive.rs`). | `round2_finding03_non_utf8_filter_matches_symlink_to_credential` |
| 4 | S8 misses listed-fold credential spellings | Added cached listed-fold directory discovery (`find_s8_folded_matches` in `recursive.rs`, `resolve_folded_under_base` / `resolve_folded_abs_path` in `sensitive.rs`) across `CRED_FILES`, `CRED_TREES`, `.ssh`, `BROWSER_ROOTS`, Atrium paths, and stores within filesystem budgets. | `round2_finding04_s8_matches_listed_fold_credential_spellings` |
| 5 | Browser S8 traversal silently stops after 3 levels | Replaced `depth < 2` with `limits::WALK_MAX_DEPTH` (16) and `limits::WALK_MAX_ENTRIES` (4,096) in `browser_dir_has_existing_cred` (and added depth-cap `Unread` in `dir_has_existing_hard_deny_ssh_entry`), emitting `Unread(WalkCapReached(FsCap::WalkDepth \| EntriesRead))` on cap (`recursive.rs`). | `round2_finding05_browser_s8_deeper_levels_and_depth_cap_unread` |
| 6 | Dangling protected entries outside `.ssh` lose protection | Changed `build_protected_set` (`record_lstat_and_stat`) to always record `lstat` identity first (plus `stat` target identity when symlink resolves) for `CRED_FILES`, `CRED_TREES`, `atrium_data_paths`, `BROWSER_FILES`, and `ASK_*` stores (`sensitive.rs`), and always retain `symlink_identities` in `resolve.rs`. | `round2_finding06_dangling_protected_entries_outside_ssh` |
| 7 | Write-only protected identities omitted | Implemented `collect_write_only_identities` (`sensitive.rs`) and lazy `WriteOnlyIdCache` in `resolve_command_targets`, `resolve_subject_paths`, and `check_recursive_root` (`command.rs`, `recursive.rs`) for `~/.ssh/config`, `~/.ssh/authorized_keys`, and `atrium_protected_paths` (+ descendants); filed concrete change request **CR-P2-1** above for pure standalone `check_target`. | `round2_finding07_write_only_hard_link_aliases_in_resolver_flows` |
| 8 | Canonicalization erases listed protected spelling (`.SSH/ID_RSA -> ordinary`) | `build_protected_set` now discovers all listed-fold spellings of `.ssh` (including `.SSH`, `.ſsh`) and records both `lstat` and `stat` identities; `resolve_literal_impl` always retains traversed `symlink_identities` (`sensitive.rs`, `resolve.rs`). | `round2_finding08_canonicalization_preserves_listed_protected_spelling` |
| 9 | Unicode normalization handling incomplete (`U+01FB` `ǻ`) | Expanded NFD/CCC/ignorable tables (complete Latin-1, Latin Extended-A, key Latin Extended-B including `U+01FB`, full `U+0300..=U+036F` CCCs, Hangul) and added a sound conservative §11 item 13 fallback in `component_may_match_unlisted` that marks non-ASCII component pairs `Unread(UnlistedNameEquivalence)` whenever either contains a scalar outside the exact table (`sensitive.rs`); decoupled `FakeFs` normalization from production folding (`resolver.rs`). | `round2_finding09_unicode_normalization_u01fb_and_independent_conformance` |
| 10 | Global `FsCaps` failures become Clear in hard-link walk | `walk_hard_links` now records `Unread(WalkCapReached(c))` on `ResolveError::CapReached(c)` from `read_dir`/`lstat`, preserving the Clear exception strictly for the local `HARD_LINK_WALK_ENTRIES` / `HARD_LINK_WALK_DEPTH` cutoffs (`recursive.rs`). | `round2_finding10_global_fs_caps_in_hard_link_walk_are_unread` |
| 11 | Budget accounting omits work and charges allocations after they occur | Precharged allocations and compute before folding/cache work in `check_target_impl` and `SharedSensitive::for_protected_budgeted` (`sensitive.rs`); reserved output buffers before OS calls in `FsResolver::read_link` (`rustix::fs::readlink` + `lstat` size) and `canonicalize`, and rewrote `FsResolver::read_dir` with `rustix::fs::RawDir` on a stack buffer charging `charge_entries_read(1)` and compute BEFORE advancing `next()` and `charge_alloc` BEFORE allocating entry names (`resolver.rs`); fixed post-allocation charges in `resolve.rs`, `glob.rs`, `recursive.rs`, and `command.rs`. | `round2_finding11_budgets_all_fs_caps_and_small_nonzero_alloc` |
| 12 | Unclosed brackets trigger repeated suffix scans | Eliminated quadratic rescanning in `parse_component` / `parse_bracket_class` / `scan_posix_bracket_item` via `skip_brackets_until` and stopping POSIX bracket-item scans on unquoted `[` (`glob.rs`). | `round2_finding12_unclosed_brackets_parse_linearly` |
| 13 | Cwd state ignores nested-shell scope | Replaced single flat mutable `cwd_state` with tree-structured execution keyed by `parent_unit` and `depth` (`exec_unit_sequence` in `command.rs`), passing each parent's entry cwd to its children regardless of flattened vector order while isolating child `cd` changes from outer scopes and propagating child writers into `earlier_writer` / `pipeline_writers`. | `round2_finding13_nested_shell_cwd_scope_and_isolation` |
| 14 | Carried `**/*.rs` test retains wrong verdict (`Clear` vs `StoreHit`) | Updated `carried_m1_30_absent_targets_and_search_filter_probes` to assert `StoreHit` and no `HardDenyHit` (keeping `dev`'s retained `ASK_TREES` directory-prefix probe without restoring M1-30's dropped suffix matcher) and corrected the M1-30 disposition entries above (`p2_tests.rs`, handoff §2B). | `carried_m1_30_absent_targets_and_search_filter_probes` |
