# Handoff: M1-38-rev4-p2-paths (P2)

- **Branch:** `agent/neon/M1-38-rev4-p2-paths` (branched from P0's tip `1a09705`)
- **Model:** Gemini 4 Neon (High), rounds 1–2; Claude Opus 5.5, rounds 3–7 (D-142, D-145, D-147, D-149, D-151, §7–§11)
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

---

## 7. Round 3 (Opus 5.5, D-142)

Written by Claude Opus 5.5 (escalation, D-142) on the branch rebased onto P0's amended tip `b50f726` (`ProtectedSet::write_only_hard_deny_identities`, CR-P2-1). It addresses review round 2 (`.orchestration/review-input/review-round2.md`, 9 findings) and the round-1 findings that stayed open (7, 9, 11, 12). No file outside P2's owned paths changed (`git diff b50f726..HEAD -- crates/atrium-core/src/policy/{model,limits,tools}.rs` is empty).

Commits: `ca219ac` (identity collector, write-only identities, fold, Unicode, S8 spellings), `0e4d11b` (FsResolver budgets), `93fffc7` (linear brackets, ASK_TREES prefixes), `ba4c450` (walks report unreadable folders), `36733e0` (outcome and reason lists charged before they grow; also carries the round-3 tests and the two round-2 test fixes), and the handoff commit.

### 7.1 Design change: one identity collector, with gaps

- `build_protected_set` (`sensitive.rs`) is now the **only** collector of protected identities. It fills `hard_deny_identities`, `write_only_hard_deny_identities` (new, D-142) and `store_identities`. `check_target` checks the write-only list for every access other than Read. The lazy per-flow collector (`WriteOnlyIdCache`, `collect_write_only_identities`, and the block in `check_recursive_root_impl`) is gone, so the resolver-backed flows and a standalone `check_target` can no longer disagree.
- Every protected tree is walked within the contract's walk caps (`WALK_MAX_ENTRIES` entries and `WALK_MAX_DEPTH` levels per tree): credential trees, Atrium data and protected paths, `~/.ssh` (dev's `ssh_members`), browser roots (browser files only) and store trees. The old local cutoffs (1,024 entries, depth 8, `take(256)`) are gone.
- **Gaps.** A walk that stops at a cap, an unreadable folder and a filesystem cap are returned as `IdentityGap { reason, scope, write_only }` in `ProtectedSetBuild { set, gaps }`, never dropped. `GapScope` says which targets a gap can affect:
  - `Through(dir)`: a multiply-linked file, or a target whose identities include the tree root's (a path spelled through it);
  - `HardLinks`: a multiply-linked file;
  - `AnyTarget`: a protected entry that could not be established at all, and the `~/.ssh` walk (its linked folders protect files anywhere by name).
  A write-only gap affects only targets whose access is not Read. `ProtectedSetBuild::apply_identity_gaps(&mut ResolvedCallPaths, budget)` adds the Unread parts (never on a target that is already a hard-deny hit). **Until CR-P2-2 lands, P5 must call it after each resolver-backed entry point** (see 7.4).
- **Spelled-path identities.** `resolve_literal` keeps the identity of every directory that held a followed link. A path spelled through a protected folder (`~/.gnupg/zz_link -> /tmp/x`, or a chain to it) stays a hit by the folder's identity after the link leads out, as `dev` hits it by its written name. This also makes `GapScope::Through` meaningful.
- P5 should give `build_protected_set` its own `FsCaps`, so the builder's walks don't use up the check's caps.

### 7.2 Findings, fixes and tests

All tests are in `crates/atrium-core/src/policy/v4/paths/p2_tests.rs`. Each expectation was checked against the contract text, not against the code's output (D-123: no tests run by the worker).

| Finding | Fix | Regression test |
|---|---|---|
| R2-1 (test) exhaustion test measured a cold cache | The test measures each collector warm (two identical runs must cost the same) and then runs at exactly that cost minus one unit. The failing charge is then the run's last, after the first hit, so the hit must stand and Exhausted must be appended. Same intended assertions for all three collectors. | `round2_finding01_hit_preserved_after_later_exhaustion` (rewritten) |
| R2-2 (test) combined Unicode fixture required Clear | In the combined fixture `á` is Unread (Tibetan U+0F43 is uncovered: §11 item 13's conservative fallback). The `à` vs `á` Clear check moved to an isolated fixture with only covered components. | `round2_finding09_unicode_normalization_u01fb_and_independent_conformance` (edited) |
| R2-3 + R1-7 write-only collection stopped silently; pure `check_target` could not see write-only identities | 7.1: the builder fills `write_only_hard_deny_identities` within the walk caps; a cap is a write-only gap. | `round3_r2f3_write_only_members_are_collected_and_early_stops_are_unread` (depth 9, 1,100 siblings, Write and ReadWrite HardDeny, Read clear, pure `check_target`), `round3_r2f3_write_only_walk_past_its_caps_is_unread_never_clear` (4,100 siblings and depth 17: Unread after `apply_identity_gaps`; reads and singly linked files unaffected), `round3_identity_gaps_are_scoped_to_targets_they_can_affect` |
| R2-4 uncharged metadata calls in `read_link`, `canonicalize`, `DT_UNKNOWN` | `read_link` reads into a `PATH_MAX` stack buffer with `readlinkat`, with no metadata call (`EINVAL` means "not a link"). On Linux `canonicalize` opens with `O_PATH` and reads `/proc/self/fd/N`, with no metadata call; the `realpath` fallback charges one stat per component first. The `DT_UNKNOWN` `fstatat` charges the stat cap and compute first. | `round3_r2f4_fs_resolver_makes_no_uncharged_metadata_call` (Linux: zero stats left, both calls succeed and only their own cap moves) |
| R2-5 `canonicalize` charged expanded output after allocating | The `/proc` path's result is written into a stack buffer and charged at its exact size before the `Vec` exists. The fallback charges two `PATH_MAX` buffers first. `read_link` is charged the same way. | `round3_r2f5_canonicalize_charges_its_expanded_output_before_allocating` (a short link to a 200-byte folder: a budget covering the input fails; one exactly the output's size succeeds with 0 left) |
| R2-6 + R1-9 covered range held undecomposed scalars | The table adds U+0340, U+0341, U+0343, U+0344 (the only Combining Diacritical Marks with decompositions) and the singletons that decompose into ASCII or Latin-1 (U+037E, U+0387, U+1FEF, U+1FFD); every other scalar outside the exactly-covered blocks is uncovered. The conservative rule now applies unless the other side folds to pure ASCII (every mapping into ASCII is in the table). The default-ignorable list is complete (adds U+061C, U+1BCA0..=U+1BCA3). | `round3_r2f6_combining_marks_with_decompositions_are_unlisted_equivalences` (U+0341 vs `é`, U+0344 vs U+0308 U+0301: Unread; a CJK folder: Clear) |
| R2-7 + R1-12 nested unclosed brackets rescanned | `BracketIndex`: two right-to-left passes record where each `[:…:]` item ends and where a class-body scan starting at each position closes. Every `[` is decided in O(1), and each class body is scanned once. | `round3_r2f7_nested_unclosed_brackets_parse_linearly` (`[[:digit:]` ×2,048, `[[[:alpha:]` ×2,048, `[[:` ×6,000, `[a[:x` ×4,000: under 40 units per byte); `round2_finding12_unclosed_brackets_parse_linearly` kept |
| R2-8 per-character lowercase | `fold_listed_component` uses string-level `str::to_lowercase()` per valid UTF-8 segment, then `ſ`→`s` (and K→k), exactly `dev`'s `to_lowercase().replace('ſ', "s")`, keeping invalid bytes verbatim. It charges its work and allocation bound first. | `round3_r2f8_listed_fold_is_string_lowercase_with_final_sigma` (`ΟΣ` → `ος`; Atrium data at `ος`, distinct `ΟΣ` folder: HardDeny `atrium-self`) |
| R2-9 existing lowercase browser root hid fold siblings | `find_s8_folded_matches` always matches every listing entry by its folded name, so it returns every spelling. The exact `lstat` is only a fallback for unlistable folders. | `round3_r2f9_existing_lowercase_browser_root_does_not_hide_fold_siblings` |
| R1-11 budgets | All of the above, plus 7.3's budget items. | R2-4 and R2-5 tests; `round2_finding11_budgets_all_fs_caps_and_small_nonzero_alloc` kept |

### 7.3 Neighbouring cases found and fixed

- **Identity (finding 7's class):**
  - Credential-tree and store-tree members are now walked: a hard link to `~/.gnupg/private-keys-v1.d/k.key` is HardDeny, and one to `~/.aws/sso/cache/tok.json` is a Store hit.
  - A written recursive root checks write-only identities in the hard-link walk (a hard link of `~/.ssh/config` under a written root is HardDeny).
  - Tests: `round3_neighbour_hard_links_to_tree_members_and_folded_abs_paths`, `round3_neighbour_written_recursive_root_checks_write_only_identities`.
- **Folds (finding 8's and 9's class):**
  - Atrium data and protected paths are discovered under every listed-fold spelling of **every** component (from home, or from `/` outside home), not only siblings of the last component: `/opt/atrium` present only as `/OPT/atrium` is collected.
  - The builder's fallback for an unlistable folder probes the configured spelling, not the lowercase one.
  - A link inside a protected folder that leads out keeps the hit (7.1).
  - Tests: `round3_neighbour_hard_links_to_tree_members_and_folded_abs_paths`, `round3_neighbour_paths_spelled_through_protected_folders_keep_their_hit`.
- **Walks (finding 3's class):**
  - The hard-link walk records an unreadable folder or a failed `lstat` as Unread; only its local cutoff counts as clear.
  - The filter and glob walks record a symlinked folder they cannot canonicalize, instead of silently not descending.
  - The builder's `~/.ssh`, browser and `ASK_NAME_PREFIXES` walks report gaps.
  - Test: `round3_neighbour_hard_link_walk_reports_unreadable_folders`.
- **Linear work (finding 12's class):** `dev`'s `ASK_TREES` directory-prefix rule built one matcher per `/` of a filter (quadratic). It now tries only prefixes that can still match (at most `2 * components + 2`), found in one pass that respects braces. Test: `round3_neighbour_ask_tree_prefix_rule_stays_linear` (`a/` ×5,000).
- **Budgets (finding 11's class):**
  - Folding: the listed and extended folds charge their work and allocation bound before running. `SharedSensitive` folds each key path through the budgeted fold. The unlisted-equivalence check compares protected names without allocating and charges the candidate's component list first.
  - Probe lists are charged before they are built.
  - Vector growth: outcome and Unread-reason lists grow through `push_charged`, which charges the new buffer before reallocating, and so does `read_dir`'s entry vector. A hit or Exhausted record whose slot is what exhausts the budget is still recorded (§8: a hit found before the budget ran out stands). The final Exhausted record is the only uncharged push, because the budget is already spent.
  - Paths of 256 bytes or more charge their heap `CString` conversion before each filesystem call.

### 7.4 Change request

**CR-P2-2: carry the identity gaps in `ProtectedSet` (`crates/atrium-core/src/policy/model.rs`).**
- **Field:** `pub(crate) identity_gaps: Vec<IdentityGap>`, with `IdentityGap { reason: UnreadReason, scope: GapScope, write_only: bool }` and `GapScope { HardLinks, Through(FileIdentity), AnyTarget }` moved from `v4/paths/sensitive.rs` to `model.rs`. Doc: "reasons the builder could not establish every protected identity; a target a gap can affect that is not a hit is Unread (contract §2, §5)."
- **Why:** collection that stops early must be Unread, never Clear (review round 2, finding 3). The frozen `ProtectedSet` has no field for that, so `check_target(target, protected, budget)` and the entry points that take `&ProtectedSet` cannot see the gaps. Today they reach callers only through `ProtectedSetBuild::apply_identity_gaps`, which P5 must call. That covers each call's top-level targets and the multiply-linked flag, but not entries checked inside a recursive walk.
- **Failing cases until it lands:**
  1. `~/.config/atrium/big` holds 4,100 files and `zz_key` hard-linked to `proj/alias`. A standalone `check_target` of `proj/alias` with Write returns Clear (plus the multiply-linked flag; Ask by §1 step 4) instead of Unread.
  2. `rg -g '*.toml' x proj` with `proj/l -> ~/.config/google-chrome/Default/x.toml`, where the browser walk passed its cap. The selected entry is checked inside `check_recursive_root` and is Clear instead of Unread.
- **With the field:** `check_target_impl` applies `ProtectedSetBuild::gap_outcomes`' rule itself, and `apply_identity_gaps` and `ProtectedSetBuild` go away.

### 7.5 Corrected dispositions

- §4's CR-P2-1 status is superseded: CR-P2-1 landed in P0 (`b50f726`). The resolver-backed lazy collection it described has been removed (7.1).
- §1's description of `build_protected_set` and the ~/.ssh walk bounds (`SSH_MAX_ENTRIES = 1,024`, depth 8) is superseded by 7.1: all builder walks use `WALK_MAX_ENTRIES` / `WALK_MAX_DEPTH` and report gaps.
- §1's description of the listed fold ("per-character") is superseded by R2-8's string-level fold.
- §2's dispositions are unchanged.

### 7.6 Verification

Run by the worker (D-123): `cargo fmt --all` (clean, `--check` passes), `cargo check -p atrium-core --tests` (clean), `cargo clippy -p atrium-core --tests -- -D warnings` (clean). No tests were run.

For the orchestrator's verification task (fish-compatible):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p atrium-core --lib policy::v4::paths
cargo test --workspace --no-fail-fast
```

Notes for the run:
- The FsResolver tests use a temporary folder and a fake home. Two of them (`round3_r2f4_*`, `round3_r2f5_*`) are Linux-only, since they rely on `/proc/self/fd`.
- The `DT_UNKNOWN` path of `FsResolver::read_dir` cannot be produced on ordinary local filesystems, so it has no runtime test; its charge is visible in the code.

---

## 8. Round 4 (Opus 5.5, review round 3, D-144)

On the branch rebased onto P0's round-7 tip `4f18a59` (`GapScope`, `IdentityGap`, `ProtectedSet::identity_gaps`, D-144). It addresses all 8 findings of `.orchestration/review-input/review-round3.md` and adopts CR-P2-2. No file outside P2's owned paths changed.

Commits: `ed83b2f` (code: CR-P2-2 adoption, conservative gaps, findings 1–8), `f29c839` (tests), and the handoff commit.

### 8.1 CR-P2-2 adopted

- `build_protected_set` returns P0's `ProtectedSet`, with `identity_gaps` filled. The local `GapScope`, `IdentityGap`, `ProtectedSetBuild` and `apply_identity_gaps` are gone, and so is §7.1's obligation on P5 to apply gaps.
- `check_target` applies the gaps itself (`check_target_impl` = exact check, then gaps). A Clear target, or a Store hit, that a gap applies to becomes Unread with the gap's reason; a hard-deny hit is never touched.
  - Every flow and every recursive walk goes through `check_target`, so gaps reach entries checked inside walks.
  - The resolver-backed flows (`push_checked`) record the Store hit **and** the gap's Unread. The single-result `check_target` reports the stricter Unread.
- Both of §7.4's failing cases now pass:
  - `round3_r2f3_write_only_walk_past_its_caps_is_unread_never_clear`: the standalone `check_target` with Write through a hard link past the cap is Unread;
  - `round4_cr_p2_2_gaps_reach_standalone_checks_and_recursive_walks`: a filter-selected link into a browser root whose walk passed its cap is Unread.

### 8.2 Conservative rules (where I chose one, and what it costs)

1. **Every builder gap is `GapScope::AnyTarget` for its access scope** (round-3 finding 2).
   - *Why:* a walk that stops early, an unreadable folder or a filesystem cap may hide a member link whose target, which can be anywhere, the complete walk would have recorded. So any target that is not already a hit could be that target. Narrow scopes (`Through`, `HardLinks`) kept missing those external targets.
   - *Cost:* more Unread, never a missed hit. While any protected tree exceeds its walk caps (`WALK_MAX_ENTRIES` = 4,096 entries, `WALK_MAX_DEPTH` = 16 levels per tree) or holds an unreadable folder, every target that is not a hit is Unread for that access scope. So it asks in Home and under prefix and exact rules, and is allowed under unscoped Any and FullAccess. A write-only gap affects only non-Read targets.
   - **For the driver:** on real machines the likely triggers are large browser profiles (Chrome or Chromium caches often exceed 4,096 entries), gcloud logs, `~/.zsh_sessions` and a growing Atrium data folder. If that makes Home ask too often, the options need a decision:
     - raise the builder's per-tree caps;
     - have P5 cache the build;
     - or narrow what the builder promises, for example recording link targets only for `~/.ssh` members, as `dev` does. Then gaps in other trees could be `Through(tree)`, but a direct read of a credential tree's link target would no longer be a hit.
2. **A directory reached through a link is walked like the directory** (round-3 finding 1), in browser roots and in every protected tree. Its members are recorded under their real identities, so they are protected under any name.
   - For browser roots and `~/.ssh`, a linked directory's own identity is **not** recorded. Recording it would protect everything below it, which neither the name rule nor `dev` (listing a linked folder stays ordinary) does.
3. **Discovery runs from `/` over every listed-fold spelling of every component** (round-3 finding 4): home's components, and Atrium paths inside or outside home. It then tries the configured spelling with `lstat`, for volumes whose equivalences the listings don't show (NFC/NFD, case).
   - A folder on the way that cannot be listed is an `AnyTarget` gap, and its configured spelling is still tried.
   - *Cost:* the builder lists `/` and every ancestor of home once per build (cached).
4. **TooLarge is decided before any budget is spent**, and its record is kept even when charging its slot exhausts the budget (round-3 finding 7, §1). A command line whose path operands and redirects alone exceed `MAX_PATHS` is TooLarge up front. That count is a structural check and is not charged (like `MAX_BYTES`), so an exhausted budget cannot hide it.
5. **`mv` with `-t` / `--target-directory`** (round-3 finding 8): every path operand may be a source. This covers a short-option cluster containing `t`.
   - *Cost:* `mv -t dir file` with a directory destination is Unread(DerivedDestinations).

### 8.3 Findings, fixes and tests

| Finding | Fix | Regression test (`p2_tests.rs`) |
|---|---|---|
| 1 browser profile links lost protection | The browser walk follows linked directories (8.2 rule 2). | `round4_f1_browser_profile_reached_through_a_link_is_collected` |
| 2 `Through` gaps missed external link targets | All gaps are `AnyTarget` (8.2 rule 1). | `round4_f2_external_target_of_an_uncollected_link_is_unread` |
| 3 cached listing failure kept the first access scope | Listings are cached as `Entries` / `Absent` / `Failed(reason)`; every consumer re-records a `Failed` gap in its own access scope. | `round4_f3_cached_listing_failure_is_recorded_for_every_access_scope` |
| 4 discovery anchored at the exact home spelling | 8.2 rule 3. | `round4_f4_fold_variants_of_home_components_are_discovered` |
| 5 Linux `canonicalize` skipped LinkReads | The `/proc/self/fd/N` read charges LinkReads first. | `round3_r2f4_fs_resolver_makes_no_uncharged_metadata_call` (updated: both caps move; zero LinkReads is `CapReached(LinkReads)`) |
| 6 uncharged error-path copies and cache clones | See the list below. | `round4_f6_error_path_copies_are_charged_before_allocating` (`resolve_literal(b"x\0")` and a NUL glob with zero allocation: Exhausted) |
| 7 allocation exhaustion dropped TooLarge | 8.2 rule 4. | `round4_f7_too_large_survives_an_exhausted_budget` (`MAX_USES + 1` subject paths and `MAX_PATHS + 1` operands, with zero allocation and with zero budget) |
| 8 `mv` destination treated as a source | `WhenSourceIsDir` tests source positions only: the last path-role operand is the destination (8.2 rule 5 for `-t`). | `round4_f8_mv_trigger_is_a_directory_source_not_the_destination` (the annex's all-`Path`, ReadWrite roles) |

Finding 6's fixes, all charged before the allocation happens:
- error-path copies into Unread reasons (`resolve_literal`, glob, every `path: X.clone()` / `X.to_vec()` push, inserted by a scripted pass);
- probe-name and tool-name copies;
- literal resolution's segment queue and stacks;
- the glob walk's stack, component list and visited set;
- the recursive walks' stacks and visited sets;
- the execution-tree maps.

Directory listings are shared through `Rc` instead of deep-cloned, and their one copy is charged.

### 8.4 Neighbouring cases

- A protected folder reached through a link is walked, and its members are hits under any name: `round4_neighbour_protected_folder_reached_through_a_link_is_walked`.
- `MAX_PATHS` on command lines is decided before the budget (8.2 rule 4).
- Every TooLarge record, wherever it is pushed, survives an exhausted budget.

### 8.5 Superseded statements

- §7.1's `ProtectedSetBuild` / `apply_identity_gaps` and the P5 obligation to call it are gone (8.1).
- §7.1's gap scopes are replaced by 8.2 rule 1.
- §7.4 (CR-P2-2) is accepted and implemented (D-144).
- The round-3 test `round3_identity_gaps_are_scoped_to_targets_they_can_affect` is replaced by `round4_identity_gaps_widen_unread_and_never_touch_hits`.
- `round3_r2f3_write_only_walk_past_its_caps_is_unread_never_clear` and `round3_neighbour_paths_spelled_through_protected_folders_keep_their_hit` now expect Unread (not Clear) for non-hit targets of a gapped build.

### 8.6 Verification

Run by the worker (D-123): `cargo fmt --all` (clean), `cargo check -p atrium-core --tests` (clean), `cargo clippy -p atrium-core --tests -- -D warnings` (clean). No tests were run.

For the orchestrator (fish-compatible), the same commands as in §7.6:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p atrium-core --lib policy::v4::paths
cargo test --workspace --no-fail-fast
```

The real-filesystem tests now also list `/` and the temporary folder's parent: discovery runs from the root. They read no file contents and no credential paths.

---

## 9. Round 5 (D-145)

Applies the driver's ruling D-145 and fixes the two failures in the orchestrator's verification of `39c290a`. Commits: `5550324` (builder), `eebec79` (tests), and the handoff commit. No file outside P2's owned paths changed.

### 9.1 What the ruling changed in the builder (`sensitive.rs`)

| D-145 rule | Round 4 | Round 5 |
|---|---|---|
| 1 Protected by name and root identities | Roots were recorded, but so was every member | Roots are recorded by their own identity (`lstat`, so a dangling link counts) and by what they name (as `dev`'s `metadata`). The roots are: credential files and trees, Atrium data and protected paths, store files and trees, browser credential files, and `msal_token_cache.*` files. A target's path and existing ancestors are compared with them. |
| 2 Members for hard links, cutoff Clear | Every member (files, links, folders) recorded; a cutoff was an `AnyTarget` gap | Only regular files with more than one link are recorded, within `WALK_MAX_ENTRIES` / `WALK_MAX_DEPTH`. A cutoff, an unreadable subfolder or a member that can't be stat'ed is Clear (no gap). An uncollected hard-linked alias still asks (multiply-linked, §1 step 4). |
| 3 Link targets only for `~/.ssh` | Link targets recorded in every tree | Recorded only for `~/.ssh` members (never a linked folder's own identity). A direct read of a link's external target in another tree is Clear, as on `dev`. Through the protected spelling it stays a hit, by name and by the root's retained identity. |
| 4 Gaps only for roots | Every early stop: `AnyTarget` | Only where a root or its name can't be established. See the list below. |

Rule 4's gaps, by case:
- A root that can't be listed: `Through(root)`.
- A root that can't be stat'ed: `AnyTarget`, since its identity is unknown.
- A root link whose target can't be stat'ed: `Through(link)`.
- An unreadable folder on the path to a root: `AnyTarget`. The fold spellings it hides have no known identity, and the configured spelling is still tried.
- Any `~/.ssh` incompleteness: `AnyTarget`.
- A browser root or `ASK_NAME_PREFIXES` folder that can't be listed: `Through(root)`.

Unchanged from round 4, as D-145 item 5 requires:
- linked folders walked like folders (round-3 finding 1);
- fold discovery from `/`;
- listing failures cached with their reason and re-recorded per access scope;
- budgets;
- TooLarge above Exhausted;
- write-only identities;
- gaps applied inside `check_target`.

Round 4's §8.2 rule 1 (every gap `AnyTarget`) is withdrawn, and with it that cost note. The cost now is that of `dev`, plus Unread for roots the builder can't establish.

### 9.2 The two verification failures

Both failing tests (`fs_resolver_real_tempdir_symlinks_dangling_hardlinks_unreadable_and_budgets` and `round2_finding11_budgets_all_fs_caps_and_small_nonzero_alloc`) build a protected set on the real filesystem with the default per-check budget (8 MiB of allocation). **The implementation was wrong; the tests' budgets are right** under §8:
- **Cause:** discovery from `/` lists `/` and the temp folder's parent (`/tmp`, 7,316 entries on this machine) once, then cached the listing. But every protected name (about 60) re-folded every cached entry, charging the fold's allocation bound each time. That is tens of MiB for one build.
- **Fix:** a cached listing stores each name's fold, computed once per build. Each discovery then compares folded names and charges only the matches it collects. A build on that `/tmp` now uses about 1.5 MiB.

The tests are unchanged. Real-filesystem tests still depend on the size of the temporary folder's parent (if it held more than about 16,000 entries, the builder's listing would reach its filesystem cap). That is an environment property, not a budget error.

### 9.3 Tests changed by the ruling (each cites D-145)

| Test | Before | Now |
|---|---|---|
| `round3_r2f3_write_only_walk_past_its_caps_is_unread_never_clear` → **`round5_d145_member_walk_cutoff_is_clear_and_still_asks_by_its_link_count`** | Write through a hard link past the entry and depth caps: Unread; an ordinary write was Unread too | No gaps. The write is Clear with `multiply_linked_file` (Ask by §1 step 4); an ordinary write is Clear; the member spelled through the folder is a hit. |
| `round4_identity_gaps_widen_unread_and_never_touch_hits` → **`round5_d145_gaps_only_where_a_root_cannot_be_established`** | A capped browser root made every target Unread | (a) A capped tree is no gap. (b) An unlistable `.gnupg` is `Through(.gnupg)`: ordinary targets Clear, a multiply-linked file Unread, a spelled member an exact hit. (c) An unlistable `~/.ssh` is `AnyTarget`: ordinary targets Unread, a Store hit kept with the Unread (single result: Unread), hard-deny hits untouched. |
| `round4_f2_external_target_of_an_uncollected_link_is_unread` → **`round5_d145_unlistable_tree_root_is_a_gap_through_it`** | `/tmp/token` (target of `~/.gnupg/key`) Unread | `/tmp/token` Clear (rule 3); the gap is `Through(.gnupg)`; `~/.gnupg/key` a hit; a multiply-linked file Unread. |
| `round3_neighbour_paths_spelled_through_protected_folders_keep_their_hit` | The links' targets Unread (walk-cap gaps) | No gaps; the targets are Clear (rules 2–3); the spelled paths stay hits. |
| `round4_f3_cached_listing_failure_is_recorded_for_every_access_scope` | Unread | Still Unread. The gap is now justified by rule 4 (an unreadable folder on the path to a root): the read-scope gap is `AnyTarget`. Comment updated. |
| `round4_cr_p2_2_gaps_reach_standalone_checks_and_recursive_walks` | A capped browser root | The browser root can't be listed (rule 4): the filter-selected link into it is Unread inside the recursive walk. |

All other round 1–4 regressions keep their expectations, each re-checked against the D-145 builder:
- hard-linked members (link count 2), browser files and roots are still collected;
- linked folders are still walked;
- `~/.ssh` link targets are still recorded.

### 9.4 Verification

Run by the worker (D-123): `cargo fmt --all` (clean), `cargo check -p atrium-core --tests` (clean), `cargo clippy -p atrium-core --tests -- -D warnings` (clean). No tests were run.

For the orchestrator: the same commands as in §7.6 and §8.6.

---

## 10. Round 6 (D-147)

Fixes review 4's 7 findings (`.orchestration/review-input/review-round4.md`) under D-147. Commits:
- `8439ac3`: written spellings, findings 1–6;
- `64b9883`: the frozen `check_recursive_root` wrapper;
- `e73a64c`: tests;
- the handoff commit.

No file outside P2's owned paths changed.

### 10.1 Protected names on the written spelling (findings 1 and 4, D-147)

`dev` checks protected names on the written, lexically normalised spelling as well as on the resolved path (`Sensitive::hit` over `Analysis::all_paths`). P2 checked only the canonical path.

**Now:** every resolver-backed flow also checks each target's written spelling, with the listed folds and `dev`'s exceptions (public `.ssh` names, readable `config` / `authorized_keys`).
- **The written spelling** (`sensitive::lexical_spelling`, `resolve::word_spelling`): `dev`'s `words::lexical_resolve`. That is `~` expanded, a relative path joined onto the written working directory, `.` removed, and `..` popping (never above `/`). A quoted leading `~` is a literal name.
- **The check:** `sensitive::written_name_hit` gives a HardDeny or Store hit for the part, recorded beside the exact check's outcome (`command::push_checked_written`). It runs even when resolution is Unread, so an unreadable protected folder still hits by name, as on `dev`.
- **Where:**
  - command operands and redirect targets;
  - glob exact matches (`glob::expand_shell_glob_written` returns each match's written spelling; variant-only matches get none, since they are never hits);
  - subject paths;
  - selector-glob bases (selector probes too);
  - implicit cwd roots;
  - recursive roots (`check_recursive_root_impl(…, written_root, …)`): the root's own name, S8 and the store or exempt-root rule on the written root as well, and the filter's selected entries and probes (`check_filter_on_root(…, written_root, …)`).
- **`cd`:** the working-directory state keeps the written spelling beside the real one. Later relative paths are resolved on the real directory and named on the written one.

### 10.2 Other findings

| Finding | Fix | Regression test (`p2_tests.rs`) |
|---|---|---|
| 1 names below aliased roots | 10.1 | `round6_f1_protected_names_on_the_written_spelling_below_aliased_roots`: `~/.config/google-chrome` and `~/.ssh` are links to empty folders. The absent `Cookies` and `id_rsa` are hits through file-tool paths, `cat`, `cd` then `cat Cookies`, and a redirect. `id_rsa.pub` and a readable `config` are not; a written `config` is. Also `round6_f1_glob_matches_are_checked_on_their_written_spelling`. |
| 2 S8 skipped linked profile folders | `browser_dir_has_existing_cred` follows linked folders (`linked_folder`), each once by identity; a link that can't be followed is Unread. A written root inside a browser root is followed (`stat`, not `lstat`). | `round6_f2_s8_walks_linked_browser_profile_folders` (`rg -g '*.rs'` over the root; also with a link cycle) |
| 3 a root link's identity narrowed an unknown-referent gap | A root whose followed `stat` fails is an `AnyTarget` gap (D-145 rule 4). | `round6_f3_root_link_with_unknown_referent_is_any_target` |
| 4 named MSAL roots cut at the member cap | Named-root discovery searches the whole listing (already within the filesystem caps); the written spelling is a hit by name anyway (10.1). | `round6_f4_named_store_root_after_many_siblings_is_found` (a Store hit by name and by identity) |
| 5 subject resolution allocated before charging | See the list below. | `round6_f5_subject_resolution_charges_before_allocating` |
| 6 `mv` of a link to a directory | `source_is_dir_or_unknown` `lstat`s the source's final entry before following it (`command::final_entry_path` for literals, the written spelling for glob matches); protection checks stay on the canonical target. | `round6_f6_mv_of_a_link_to_a_directory_moves_the_link` |
| 7 real-filesystem tests depended on host folder sizes | The two tests give `build_protected_set` its own phase caps and budget (unbounded, as P5 should give the builder its own caps); their assertions use fresh default ones. | the two tests themselves (`fs_resolver_real_tempdir_…`, `round2_finding11_…`) |

Finding 5's charges, all made before the allocation:
- the filter `Word`;
- the root list (no eager buffer), its clones and their written spellings;
- the selector bases and folded roots;
- the target lists and per-unit parts;
- the child scope's cwd clone;
- the variant-only pattern copies;
- the selector normalisation buffer.

Recursive roots' outcomes are appended through `extend_charged`, which, like `push_charged`, keeps hit, Exhausted and TooLarge records if the charge exhausts the budget.

The execution-tree maps keep their per-unit charge of 160 bytes, which covers their entries (about 100 bytes per unit).

### 10.3 Change request

**CR-P2-3: carry the written spelling in `Target` (`model.rs`).**
- **Why:** the frozen `check_target(target, protected, budget)` and `check_recursive_root(root, …)` receive only a `Target`, whose resolution holds the canonical path, never the written spelling. So a standalone `resolve_literal` + `check_target` of `~/.config/google-chrome/Cookies` is Clear when `google-chrome` is a link to an empty folder. It is also Clear when it is a link to a profile past the builder's walk cap (D-145: no gap). A standalone `check_recursive_root` misses S8 on a written root that is a link, because the canonical root lies outside home.
  - The resolver-backed flows (`resolve_command_targets`, `resolve_subject_paths`), which P5 calls, do apply D-147 (10.1).
- **Field:** `pub(crate) written: Option<Vec<u8>>` on `Target` (or on `TargetResolution::Resolved`), the lexical spelling. `resolve_literal` and glob expansion would fill it, and `check_target` / `check_recursive_root` would check it. P2's side channel (`push_checked_written`, `written_root`) would then go.
- **Failing cases until it lands:** the two above, through the standalone entry points only.

### 10.4 Verification

Run by the worker (D-123): `cargo fmt --all` (clean), `cargo check -p atrium-core --tests` (clean), `cargo clippy -p atrium-core --tests -- -D warnings` (clean). No tests were run.

For the orchestrator: the same commands as in §7.6.

---

## 11. Round 7 (D-149, D-151)

On the branch rebased onto P0's round-8 tip `a5b9b3b` (`Target::written`, CR-P2-3, D-149). Fixes review 5's 6 findings (`.orchestration/review-input/review-round5.md`) and sweeps the class rules from the orchestrator's brief. Commits:
- `30c162d`: code;
- `5a099d9`: tests;
- the handoff commit.

No file outside P2's owned paths changed.

### 11.1 CR-P2-3 adopted (D-149)

- **Who fills `Target::written`:**
  - `resolve_literal` fills it (the lexical spelling on its `cwd`).
  - Glob expansion fills it for exact matches: the matched path as spelled, on the written cwd for a relative pattern. Variant-only matches get `None`, since they are never hits.
  - The command flow replaces it with the spelling on its written working directory, for operands, redirects and implicit roots.
  - Filter-selected entries below a differently spelled root get the written root plus the relative path.
- **`check_target`** now names the written spelling itself, through `check_target_all`, which keeps every record (§1):
  - the written name hit, which is checked first, so an Unread resolution keeps it beside its reason;
  - the canonical exact check;
  - the written spelling's unlisted equivalences (review 5, finding 3);
  - the gap that applies, beside a Clear or Store-hit outcome.

  The single-result `check_target` reports the strictest: hard deny, then Unread, then Store hit or Clear. The flows and the recursive walks keep every record (`push_target_records`).
- **`check_recursive_root`** reads `root.written`. The side channel (`push_checked_written`, `written_root`) is gone.
- **Both CR-P2-3 standalone cases pass:** `round7_cr_p2_3_standalone_checks_name_the_written_spelling`.

### 11.2 `dev`'s alias rules for protected roots (finding 2 and its class)

`dev` canonicalizes its roots (`sensitive_roots`: `~/.ssh`, credential files and trees, browser roots, Atrium data and protected paths; plus the store roots) and applies two rules. `RootAliases` reproduces them through a table of canonical roots that is built lazily once per call and charged.
- **`alias_fix`** (`alias_name_hit`): a path at or below a root's canonical folder, when that folder differs from the root's spelling, is named on the root's spelling plus the rest. Its protected names then hold even for entries that don't exist yet, and the public `.ssh` names stay public. It applies to every flow target, filter entry, followed link and recursive root, where S8 runs under the alias spelling too.
- **`links_below` / `in_trees`** (`RootAliases::reached`): a followed link whose real path is in a root, is a root, or holds a credential root reaches it. That is a hit without a filter, and Unread (`FollowedLinkUnderFilter`) with one. This closes `rg -L token proj` through `proj/p -> ~/.config/google-chrome -> /tmp/profile` (finding 2).
- **The pure `check_target` has no resolver**, so it can't build this table. It relies on the root identities D-145 records, which cover wholesale-protected roots and existing browser and `~/.ssh` members within the walk caps. Every resolver-backed entry point, including the frozen `check_recursive_root`, applies both rules.

### 11.3 Findings, fixes, tests

| Finding | Fix | Regression test (`p2_tests.rs`) |
|---|---|---|
| 1 Unread recursive root skipped its written name | The root's records (`push_target_records`) come first, and an Unread root keeps its name hit beside its reason. | `round7_f1_unread_recursive_root_keeps_its_written_name_hit` |
| 2 `rg -L` through an aliased browser root | 11.2 | `round7_f2_followed_link_through_an_aliased_browser_root_reaches_it` (Unread under a filter), `round7_alias_rule_names_paths_below_an_aliased_root` |
| 3 written spelling skipped unlisted equivalences | `check_target_all` checks them on the written spelling when nothing else matched. | `round7_f3_written_spelling_gets_the_unlisted_equivalence_check` |
| 4 globbed `mv` source typed by the lexical spelling | Glob expansion returns each match's actual entry path (not followed), and the type check uses it. | `round7_f4_globbed_mv_source_is_typed_by_the_expanded_entry` |
| 5 initial cwd undercharged | Both cwd copies are charged. | `round7_f5_initial_cwd_copies_are_charged` |
| 6 recursive filter entries lost Store hits under a gap | Filter entries keep every record. | `round7_f6_recursive_filter_entry_keeps_store_hit_beside_gap` |

### 11.4 Class sweep (the orchestrator's three rules)

- **Every early return still names the written spelling first and keeps every hit.** Covered by:
  - `check_target_all` (Unread resolutions);
  - the recursive root (11.3, row 1);
  - gaps (kept beside hits, never replacing a hard deny).
- **Written spelling for names only; the real entry for types and identities.**
  - S8 and the store or exempt rule on a written or alias spelling now list the real root and take only the names from the spelling. Round 6 listed the written path, which differs physically after `..` through a link.
  - `mv` uses the expanded entry (finding 4) or the unnormalised physical path of a literal operand.
  - `cd` resolves on the real directory and names on the written one.
  - Filter entries are enumerated on the real root.
- **Protected-root aliases:** `dev`'s two rules (11.2), applied wherever P2 has a resolver.

### 11.5 Verification

Run by the worker (D-123): `cargo fmt --all` (clean), `cargo check -p atrium-core --tests` (clean), `cargo clippy -p atrium-core --tests -- -D warnings` (clean). No tests were run.

For the orchestrator: the same commands as in §7.6.

## 12. Round 8 (D-152)

Review 6 (Codex) had 5 findings. It accepted the pure `check_target` alias limit as not a finding. Commits: `c81e19a` (fixes and class sweeps) and `28c6e2a` (regression tests). No change requests.

### 12.1 Findings, fixes, tests

| Finding | Fix | Regression test (`p2_tests.rs`) |
|---|---|---|
| 1 A later exhaustion discarded an established written-name hit | `check_target_all` is replaced by `check_target_into`, which appends each record to the caller's vector as soon as it is established. `push_target_records` passes the outcomes vector through. `check_target` returns an established hard-deny hit instead of `Exhausted`. | `round8_f1_written_name_hit_survives_a_later_exhaustion` (every budget from the first hit up to completion, for `check_target` and `check_recursive_root` on a file and on a folder) |
| 2 The alias table treated a cap or I/O failure as absence | `RootAliases` counts only `NotFound` and `ENOTDIR` as absence. A canonicalization cap or another I/O failure marks the table incomplete and keeps the first reason. `alias_name_hit` returns that reason as Unread when it finds no alias. | `round8_f2_alias_table_cut_short_by_a_cap_is_unread` |
| 3 Alias spellings skipped the unlisted equivalences | `alias_name_hit` checks the listed folds, then `matches_unlisted_equivalence`, which gives `Unread(UnlistedNameEquivalence)`. | `round8_f3_alias_spelling_gets_the_unlisted_equivalence_check` |
| 4 `~/.azure` was reached as a whole store root | `ProtectedRoot::wholesale` is false for the MSAL folder. It still spells entries below it (`alias_fix`), but `reached` skips it, as `dev`'s `ask_roots` lists the MSAL entries and not the folder. MSAL entries are still caught by name. | `round8_f4_msal_folder_is_not_a_wholesale_store_root` (README is Clear; a link to `msal_token_cache.json` is a Store hit) |
| 5 Alias rewriting was charged by the short canonical path | `rewrite` charges compute and allocation for the copied spelling before it joins. | `round8_f5_alias_rewrite_charges_the_copied_spelling` |

### 12.2 Class sweeps

- **(a) Established records survive exhaustion.**
  - `ChargedRecord for PartResult` now keeps Unread as well as hits when the slot charge fails.
  - The followed-link walk collects a link's records with `followed_link_records`. It then emits them with `emit_followed_link_records` before it propagates an error from the collection.
  - The S8 check of a followed link keeps its partial records on exhaustion (the filter rule still applies). Its uncharged `outcomes.extend` became `extend_charged`.
  - The other sites already pushed into the caller's vector before the next fallible step: the recursive root's S8 and store rule, filter entries, and the command flows (`push_checked`). Covered by `round8_class_a_followed_link_hit_survives_a_later_exhaustion`.
- **(b) Filesystem errors and caps in the checking phase are Unread.** The alias table was the remaining site (finding 2). It is never cached as complete when it is not.
  - The other `Err(_)` arms in the checking phase were rechecked. Each already reports Unread, except two that give the stricter result:
    - the walk's `is_dir` stat treats an unreadable type as "not a folder", which counts the root's own path as reached;
    - a filter that fails to fold keeps the unfolded compiled filter.
  - The builder's member-walk arms are unchanged, as D-145 requires.
- **Copied spellings are charged first:**
  - `word_entry_path` charges compute for the whole copied path, not only the word;
  - the glob's relative written spelling (`rest.join`) charges its compute;
  - the filter's written entry join charges its compute;
  - the selector normalizer charges both of its copies.

### 12.3 Behaviour notes for review

- An incomplete alias table makes every later check that finds no alias Unread for that call. This is strict, but a cap or I/O failure means the alias rule cannot be shown not to apply (§8).
- When `check_target` runs out of budget after a hard-deny hit, it returns the hit, not `Exhausted`. An established Unread or Store hit still gives `Exhausted`, which is no weaker.

### 12.4 Verification

Run by the worker (D-123): `cargo fmt --all` (clean), `cargo check -p atrium-core --tests` (clean), `cargo clippy -p atrium-core --tests -- -D warnings` (clean). No tests were run.

The budget-sweep tests run `check_recursive_root` many times, densely for 4,096 budgets after the first hit and then every 61. They may take a few seconds.

## 13. Round 9 (D-154)

Review 7 (Codex) had 4 findings, all in the alias code. D-154 asks for `dev`'s `alias_fix` and `links_below` (`gate.rs`) to be ported as literally as P2's types allow. Commits: `d731e0a` (port and fixes) and `328b86a` (regression tests). No change requests.

### 13.1 The port (`sensitive.rs`, `RootAliases`)

| `dev` (`gate.rs`) | P2 |
|---|---|
| `roots`: `sensitive_roots` + `ask_roots`, each with `identity(&r)` (`fs::metadata`, through links); unidentifiable roots skipped | `RootAliases::roots`: the same lists. Each root is read with `resolver.stat` (charged). It is skipped only when absent (`NotFound`, `ENOTDIR`); a cap or I/O failure marks the table incomplete (§8). |
| `ask_roots`: `ASK_FILES`, `ASK_TREES`, and up to 256 entries of `~/.azure` whose lowercased name starts with `msal_token_cache.` | The same, through `resolver.read_dir`, with every name and copy charged. |
| `alias_fix`: strip the firmlink; for each ancestor, shortest first, compare `identity(prefix)` with the roots; the first match gives root spelling + rest | `RootAliases::scan(canonical)`: the same order and rule, by `FileIdentity`, with every stat and copy charged. An absent ancestor ends the scan, since nothing below it exists. A cap or I/O failure ends it and is returned as the scan's Unread reason. |
| `trees_of` (`fs::canonicalize`) and `in_trees` for `links_below` | `ProtectedRoot::canonical` and `RootAliases::reached`, unchanged in logic. The store roots that ask get `held` as `dev`'s `trees_of` gives them. |

Differences from `dev`, each a P2 type constraint or the stricter side:
- **The `~/.azure` folder:** it is kept in the alias table to spell the paths below it (review 7, finding 2's first case), but with `reach: false`, so `links_below` never reaches it wholesale (review 6, finding 4). Only the names below it decide.
- **`~/.ssh` members:** `dev` adds the members of `~/.ssh` to the alias roots. P2 already covers them by identity in `ProtectedSet` (hard links, and the link targets recorded for `~/.ssh` under D-145), so they are not repeated here.

### 13.2 Findings, fixes, tests

| Finding | Fix | Regression test (`p2_tests.rs`) |
|---|---|---|
| 1 Aliases compared canonical spellings, not identities | 13.1. `FakeFs::bind_dir` models a bind mount: a second directory name with the same inode. | `round9_f1_protected_root_under_another_name_is_found_by_identity` (browser root and `~/.ssh`) |
| 2 Filter probes ignored alias spellings | `other_spellings` gives the written and alias spellings of a root. Probes run on the canonical spelling and on every other one in `check_filter_on_root`, which takes the whole list, not `spellings.first()`. This covers the subject flow (explicit and implicit roots) and recursive roots. | `round9_f2_filter_probes_see_the_protected_root_alias` (the `~/.azure` store probe; `~/other` + browser alias) |
| 3 A found alias dropped the incomplete-table Unread | `push_alias_records` keeps the scan's reason (incomplete table or unreadable ancestor) as Unread unless the alias gave a hard-deny hit. The reason sits beside a Store hit. | `round9_f3_alias_found_with_an_incomplete_table_keeps_its_reason` (`~/.ssh/config` alone; `~/.cargo/credentials` with its Store hit) |
| 4 Glob spelling allocated before charging | The relative written spelling is built straight into a charged buffer, with no intermediate list. Growth of `exact_matches` and `variant_only_matches` goes through `push_charged`. | `round9_f4_glob_spelling_and_matches_are_charged` (every allocation budget completes or reports the allocation budget, monotonically). An allocation-counting allocator is not possible in a library test, so the ordering holds by construction. |
| Correction to round 6, finding 3 | The dotless-i test now says that Unicode default case folding keeps `ı` distinct from `i`, and that Unread is the conservative answer. | `round8_f3_…` (comment); new independent case `round9_alias_spelling_gets_an_independent_unlisted_equivalence` (a zero-width space, a documented default-ignorable code point, below an aliased browser root) |

### 13.3 Class sweeps

- **Every comparison with a protected root uses identity as `dev` does.**
  - Alias spellings (single targets, recursive roots, filter and selector bases, followed links) all come from `RootAliases::scan`.
  - `links_below` stays canonical, as in `dev`.
  - Hard links and members are compared by identity in `ProtectedSet`, unchanged.
- **Every probe sees every spelling:**
  - filter probes (13.2, row 2);
  - selector-glob probes (base, written base, and now the base's alias);
  - S8 and the store rule of a followed link (canonical and alias spellings, on the real folder);
  - S8 and the store rule of a recursive root (canonical, written and alias).
- **Charges:** the S8 candidate list (`next`) now grows through `push_charged`.

### 13.4 Verification

Run by the worker (D-123): `cargo fmt --all` (clean), `cargo check -p atrium-core --tests` (clean), `cargo clippy -p atrium-core --tests -- -D warnings` (clean). No tests were run.

Expectations that rest on the review's text rather than code tracing:
- `~/.ssh/config` is not a hard-deny name (13.2, row 3).
- The filter `msal*` on a non-recursive root uses the probe path.

## 14. Round 10 (D-155)

There were two inputs: the orchestrator's verification of round 9 (two failing tests) and review 8 (3 findings). Commits:
- `81c6ab7`: verification failures;
- `8471af0`: review 8 and the sweeps;
- `e2ab0e0`: regression tests.

No change requests.

### 14.1 Verification failures (both were code bugs; the tests stand)

| Test | Cause | Fix |
|---|---|---|
| `round7_f1_unread_recursive_root_keeps_its_written_name_hit` | Round 8's `check_target_into` returned right after a written hard-deny hit, so an Unread resolution's reason was never recorded. §1 keeps both. | A written hard-deny hit ends the list only for a resolved target. An Unread resolution still records its reason beside the hit. |
| `round8_f1_written_name_hit_survives_a_later_exhaustion` (folder root `lnk/../.ssh`) | S8's `~/.ssh` walk (`dir_has_existing_hard_deny_ssh_entry`) named entries by their real path (`/tmp/pub/.ssh/id_rsa`). A root written `~/.ssh` therefore missed its keys, which breaks D-147. Only the written Store record (`ssh-dir`) remained. | The walk reads the real folder and names each entry on the spelling it is checked under: the folded root spelling plus the entry's path below the folder. The same applies to a `.ssh` found below a root. |

### 14.2 Review 8

| Finding | Fix | Regression test |
|---|---|---|
| 1 `~/.ssh` member folders were missing from the alias roots | `RootAliases::push_ssh_members` ports `dev`'s `ssh_members`, with its bounds and order. See the details below the table. | `round10_f1_ssh_member_folder_under_another_name_is_an_alias` (`~/.ssh/keys` bound at `/mnt/keys`; `/mnt/keys/future` is a hard deny) |
| 2 The incomplete-table reason was cloned before it was charged | `scan` charges the reason's compute and storage first, then clones it. Gap reasons in `check_target_into` are charged the same way. | `round10_f2_incomplete_reason_is_charged_before_it_is_copied` |
| 3 MSAL discovery undercharged its lossy lowercase | `lossy_lowercase_starts_with` gives the result of `dev`'s `to_string_lossy().to_lowercase().starts_with(pre)` without allocating. It decodes chunk by chunk, maps each invalid sequence to one U+FFFD, and lowercases each scalar. For an ASCII prefix this matches `dev`: per-scalar and string-level lowercasing differ only for a final sigma, which is not ASCII. | `round10_f3_msal_prefix_test_matches_devs_lossy_lowercase` (agreement with `dev`'s expression on valid, mixed-case and invalid names, including 64 `0xFF` bytes; and discovery of such an entry) |

How finding 1's port maps `dev`'s `ssh_members`:
- **Bounds and order:** depth 8 and 1,024 entries, placed between `sensitive_roots` and `ask_roots` as in `dev`.
- **Members:** these are alias roots, spelled through `~/.ssh`. They are files, links to files, and folders not reached through a link.
- **Folders linked into `~/.ssh`:** these are trees for `links_below` (held), not alias roots, as `dev`'s `linked_dirs` are.
- **Name exceptions:** they come from checking each member's alias spelling by name, so `~/.ssh/config` and `*.pub` stay ordinary.
- **Cutoffs:** a cutoff is silent, as in `dev` (D-145). A cap or I/O failure makes the table incomplete.

**Correction to §13.1:** round 9 claimed that `ProtectedSet` identities replace `dev`'s member aliases. That was wrong. `ProtectedSet` holds member identities only for hard links (D-145), so an ordinary member folder under another name was missed. The member aliases are now ported.

### 14.3 Sweep: P2's alias roots against `gate.rs`, line by line

| `dev` (`evaluate_sync`, `gate.rs`) | P2 (`RootAliases`) | Result |
|---|---|---|
| `sensitive_roots`: `~/.ssh` | same, first | same |
| `CRED_FILES`, `CRED_TREES`, `BROWSER_ROOTS` | same constants | entries identical (diffed) |
| Atrium data, then protected paths | same | same |
| `ssh.members` | `push_ssh_members` | ported this round (14.2) |
| `ask_roots`: `ASK_FILES` | `ASK_FILES` | P2 adds the four CR-17 stores (`.gem/credentials`, `.terraform.d/credentials.tfrc.json`, `.config/op/config`, `.yarnrc.yml`), as the contract requires. **Departure fixed:** P2's list folds `.azure/accessTokens.json` to lowercase for names, so a case-sensitive volume never found the root. The table now reads it on `dev`'s spelling (`DEV_ROOT_SPELLINGS`). Test: `round10_sweep_store_root_is_read_on_devs_spelling`. |
| `ASK_TREES` | same | same |
| MSAL entries of `~/.azure` (`take(256)`) | same | same |
| — | the `~/.azure` folder, alias only, never reached | kept: names only (review 7, finding 2; review 6, finding 4) |
| `identity` = `fs::metadata` (through links); a missing identity is skipped | `stat` through links; absence is skipped | a cap or I/O failure makes the table incomplete (§8) |
| `trees_of` adds `linked_dirs` | linked folders as reach-only roots | same |

**Clones and lossy conversions** (charged before they run):
- the incomplete-table reason and gap reasons;
- the S8 hit paths (`raw_d`);
- followed-link paths and visited sets;
- probe names (`from_utf8_lossy`, compute added beside the existing allocation charge);
- the `.azure` folder spelling.

The other lossy conversions are covered by `fold_alloc_bound`, or are in the test-only fake.

### 14.4 Verification

Run by the worker (D-123): `cargo fmt --all` (clean), `cargo check -p atrium-core --tests` (clean), `cargo clippy -p atrium-core --tests -- -D warnings` (clean). No tests were run.

An allocation-counting allocator is not possible in a library test, so charge-before-clone ordering holds by construction. The tests check that each charge exists and that results are correct.

## 15. Round 11 (D-161)

Input: review 9 (Sonnet 5.5, `.orchestration/review-input/M1-38-rev4-p2-paths-round9.md`, on round 10 `cae98db` plus the merge of `dev` at `635fe23b`), with two items (V1 and Finding 1) and five test-gap groups.

Commits on `agent/neon/M1-38-rev4-p2-paths`:
- `0721aa8`: V1 (`read_dir` on non-Linux Unix) and Finding 1 (unified `FsResolver::canonicalize` charges and bounded proper-ancestor stat cache in `RootAliases::scan`);
- `bf62832`: updated `round3_r2f4` and round 11 regression/cross-platform tests;
- handoff commit: this section (`docs/agents/handoffs/M1-38-rev4-p2-paths.md`).

No change requests: neither the frozen P0 interface (`model.rs`, `limits.rs`, `paths.rs` signatures) nor the contract (`docs/contracts/m1-policy-rev4.md`) required changes. All edits stay strictly inside P2's owned paths (`resolver.rs`, `sensitive.rs`, `p2_tests.rs`, and this handoff).

### 15.1 V1: macOS `read_dir` build failure and `#[cfg(...)]` cross-platform audit

- **Cause:** `FsResolver::read_dir` in `crates/atrium-core/src/policy/v4/paths/resolver.rs` imported `rustix::fs::RawDir` inside `#[cfg(unix)]`, but `rustix` only defines `RawDir` on Linux (`#[cfg(linux_kernel)]`), so `atrium-core` failed to compile on macOS.
- **Fix:** Split Unix directory iteration inside `FsResolver::read_dir` into:
  - `#[cfg(target_os = "linux")]`: existing `rustix::fs::RawDir` over a 4 KiB stack buffer;
  - `#[cfg(all(unix, not(target_os = "linux")))]`: `rustix::fs::Dir::read_from(&dir_fd)`, which borrows `&dir_fd` (keeping the directory descriptor open for `DT_UNKNOWN` `statat` fallbacks without `unsafe`).
  - Extracted `#[cfg(unix)] fn push_unix_dir_entry` so Linux and non-Linux Unix execute the exact same per-entry charges in the exact same order, always before allocating or stat'ing (§8): `caps.charge_entries_read(1)?` + `budget.charge(1)?` before `next()`, skip `.`/`..`, `budget.charge(name.len() - 1)?`, `charge_aux_stat` before `statat` on `DT_UNKNOWN`, `budget.charge_alloc(name.len())?` before allocating the name `Vec`, and `push_entry_charged`.
- **Cross-platform `#[cfg(...)]` audit across all P2 files:** Checked every `#[cfg(...)]` in `paths.rs`, `resolver.rs`, `recursive.rs`, `sensitive.rs`, and `p2_tests.rs`:
  - `resolver.rs`: `bytes_to_path`, `path_to_bytes`, `map_metadata`, `map_io_error`, `map_errno`, `map_rustix_file_type`, `PATH_MAX` (`1024` on Apple / `4096` elsewhere), `canonicalize_via_proc` (`linux` only), `push_unix_dir_entry` (`unix`), `read_link` (`unix` / `not(unix)`), `canonicalize` (`linux` fast path + `unix`/`not(unix)` fallback), `read_dir` (`linux` `RawDir` / `all(unix, not(linux))` `Dir` / `not(unix)`). No Linux-only APIs remain in `#[cfg(unix)]` blocks, and `#[cfg(target_os = "linux")]` blocks were checked for matching signatures and zero unused imports/vars.
  - `recursive.rs`: `unix` (`OsStrExt::as_bytes`) vs `not(unix)` (`as_encoded_bytes`) for glob entry conversion — compiles on both macOS and Linux.
  - `p2_tests.rs`: `round3_r2f4` widened from `#[cfg(target_os = "linux")]` to `#[cfg(unix)]`; `round3_r2f5` stays `#[cfg(target_os = "linux")]` (tests exact `/proc/self/fd` output-only allocation charge) with a non-Linux Unix companion (`round11_f1_fallback_canonicalize_checks_peak_before_allocating_and_charges_real_len`); `round11_v1_f1_real_fs_1100_plain_operands_and_large_read_dir` is `#[cfg(unix)]` (runs on both Linux and macOS); `round11_macos_real_fs_firmlink_strip_and_mixed_case_ssh` is `#[cfg(target_os = "macos")]`.

### 15.2 Finding 1: `FsResolver::canonicalize` platform divergence and premature caps

- **Cause:**
  - On Linux, `canonicalize_via_proc` charged `caps.charge_link_read()?` for the fixed `/proc/self/fd/N` read, tripping `FS_MAX_READLINKS` (1,024) well before `FS_MAX_CANONICALIZATIONS` (4,096).
  - On macOS (and any fallback path), `FsResolver::canonicalize` charged `charge_aux_stat` once per path component against `FS_MAX_STATS` (16,384) and permanently charged `2 * PATH_MAX` (2,048 bytes on macOS) of allocation per call, tripping `Stats` around ~1,300 operands and `AllocationBudget` around ~3,000 operands.
  - In addition, `RootAliases::scan` in `sensitive.rs` re-stat'ed every ancestor of every operand on every call; on macOS, `tempfile::tempdir()` canonicalizes to 6–7 components (`/private/var/folders/xx/.../T/.tmp...`), so 1,100 files at depth 8–9 would consume ~19,800 stats (`resolve_literal` + `RootAliases::scan`) and trip `FS_MAX_STATS` (16,384) even after removing the double-counted stats from `canonicalize`.
- **Fix:**
  - **Unified caps & compute on `FsResolver::canonicalize` (`resolver.rs`):** Both Linux and macOS charge `caps.charge_canonicalize()?`, `budget.charge(path.len() + components.max(1))?`, and `charge_path_conversion(path, budget)?`. Neither platform consumes `FsCap::Stats` or `FsCap::LinkReads` during `canonicalize`. Removed `caps` and `caps.charge_link_read()?` from `canonicalize_via_proc`, and removed the per-component `charge_aux_stat` loop from the fallback.
  - **Fallback allocation accounting keeping §8 "charge before allocate" (`resolver.rs`):** Added `FALLBACK_CANON_OVERHEAD = 64` and `FALLBACK_CANON_PEAK = PATH_MAX * 2 + FALLBACK_CANON_OVERHEAD`. Before calling `std::fs::canonicalize(p)`, the fallback verifies `budget.remaining_alloc_bytes() >= FALLBACK_CANON_PEAK` (failing via `budget.charge_alloc(FALLBACK_CANON_PEAK)?` before invoking `std::fs::canonicalize` when insufficient, so the OS call can never allocate past the remaining budget). Once `std::fs::canonicalize` returns `bytes` (and `realpath`'s transient buffer has been freed), it charges `budget.charge(bytes.len() as u64)?` and `budget.charge_alloc(bytes.len() + FALLBACK_CANON_OVERHEAD)?`. This preserves §8 strictly while allowing all 4,096 single-component operands to clear inside the 8 MiB `COMPUTE_BUDGET_ALLOC_BYTES` on both Linux and macOS.
  - **Bounded proper-ancestor stat cache in `RootAliases::scan` (`sensitive.rs`):** Added `ancestor_stats: HashMap<Vec<u8>, Option<FileIdentity>>` (scoped to one `RootAliases` instance per call, capped at 1,024 entries, caching only proper ancestors `i + 1 < comps.len()` and never leaves, with compute and allocation charged before every insertion and compute charged on cache hits). Under a 7-component macOS tempdir, 1,100 files now use ~11,060 stats total (well under `FS_MAX_STATS = 16,384`).

### 15.3 Items, fixes, and regression tests (`p2_tests.rs`)

All test expectations were derived from the rev 4 contract (§1, §2, §3, §5, §8, §10) and decisions (D-145, D-147, D-149, D-154, D-161).

| Item / Test gap | Fix | Regression test (`p2_tests.rs`) |
|---|---|---|
| V1 & F1: real-fs 1,100 operands + >1,000-entry `read_dir` + 4,096 canonicalizations (both Linux & macOS) | `Dir::read_from` on non-Linux Unix (`push_unix_dir_entry`), unified `canonicalize` caps/charges, `RootAliases::scan` proper-ancestor cache | `round11_v1_f1_real_fs_1100_plain_operands_and_large_read_dir` (`#[cfg(unix)]`); `round3_r2f4_fs_resolver_makes_no_uncharged_metadata_call` (widened to `#[cfg(unix)]`, asserts 0 `Stats` / 0 `LinkReads` / 1 `Canonicalizations`) |
| F1: 4,096 single-component operands clear under default limits; 4,097 trips `Canonicalizations` | Unified `canonicalize` charges | `round11_f1_4096_single_component_operands_clear_under_default_limits` |
| F1 & §8: non-Linux fallback peak guard before `std::fs::canonicalize` + real-length-plus-overhead deduction | `FALLBACK_CANON_PEAK` pre-check + `bytes.len() + FALLBACK_CANON_OVERHEAD` charge | `round11_f1_fallback_canonicalize_checks_peak_before_allocating_and_charges_real_len` (`#[cfg(all(unix, not(target_os = "linux")))]`) |
| Test gap: charge ordering proxy (rounds 8 & 10) | Verified exact minimum alloc budgets (`min - 1` fails with `AllocationBudget`, `min` succeeds with 0 remaining, `min >= copy_size` and delta matches copy) | `round11_charge_ordering_exact_minimum_alloc_budgets` (incomplete-table reason in `RootAliases::scan`, MSAL entry copy in `spelled_roots`, `~/.ssh` member spelling in both `push_ssh_members` and S8 `dir_has_existing_hard_deny_ssh_entry`) |
| Test gap: round 10 finding 1 (`~/.ssh` members) | Covered member file via hard link & bound member folder, subfolder under linked tree into `~/.ssh` (not alias root, files are members, `links_below` reaches tree), linked `~/.ssh` itself, and >1,024 entries silent cutoff -> Clear | `round11_r10f1_ssh_member_file_linked_tree_and_cutoff_cases` |
| Test gap: round 10 fix 2 (`.ssh` below root & alias spelling) | Covered S8-exempt `~`, `~/proj` reaching `~/.ssh` via followed link (unfiltered HardDeny / filtered Unread), alias `/mnt/keys` with private key vs only `.pub`+`config`, `~/proj` reaching `/mnt/keys`, and written subfolder `lnk/../.ssh/sub` | `round11_r10fix2_ssh_below_root_and_alias_spelling` |
| Test gap: macOS real-fs firmlink strip & mixed-case `~/.SSH` | Real tempdir (never real `~`) exercising `/System/Volumes/Data/<tmp>` canonicalization + firmlink strip (existing credential, absent `~/.aws/credentials`, ordinary file) and mixed-case `~/.SSH` (`ID_RSA`, absent key, `ID_RSA.PUB`, `CONFIG` read vs write) | `round11_macos_real_fs_firmlink_strip_and_mixed_case_ssh` (`#[cfg(target_os = "macos")]`) |

### 15.4 Verification

Run by the worker (D-123):
- `cargo fmt --all` (clean)
- `cargo check -p atrium-core --tests` (clean on macOS, 0 errors, 0 warnings)
- No tests or `clippy` were run (D-123). No real provider was invoked.

Exact commands for the orchestrator's verification (fish-compatible; run on both macOS and Linux):
```fish
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p atrium-core --lib policy::v4::paths
cargo test --workspace --no-fail-fast
```
