# M1-32-link-globs: handoff

**State: S11 complete (rework round 2 fixed); K5 closed under D-125.**

Brief read from `dev`: `docs/agents/briefs/M1-32-link-globs.md` (including scope change D-125: K5 is closed as correct behaviour; scope is S11 only).

---

## What this task did

1. Implemented **S11** in `477fe81` and fixed all 7 findings from `.orchestration/review-input/review-round1.md` in rework round 1:
   - `crates/atrium-core/src/policy/gate.rs`:
     - `evaluate_sync` (~318–401) builds `real_sensitive` (`deny::sensitive` on `resolve_fs`-resolved `home`/`atrium_data_paths`/`atrium_protected_paths`), passes `TreeKind::Sensitive` for `sensitive_roots`/`atrium_*` and `TreeKind::Raw` for `ssh.linked_dirs`/`ask_roots`, and passes `&c.cwd`, `&c.home`, and `Some(&real_sensitive)` / `None` to `reaches_tree_through_links`.
     - `expand_glob` / `expand_glob_with` (~849–964) supports file-tool dotfile matching (`include_dotfiles = true`), tracks `(canonical_dir, comp_idx)` for `**`, sets `truncated = true` on `MAX_DEPTH` (keeping already-found matches) but returns `None` (the whole glob asks, matches discarded) when the `MAX_ENTRIES` budget is exhausted (see rework round 2), and propagates `read_dir`, entry, `symlink_metadata`, and `metadata` errors (other than `NotFound` or non-directory targets) as `truncated = true`.
     - `real_path` (~992–1079) resolves symlinks (including dangling ones) component-by-component in filesystem order so `..` applies after preceding links.
     - `trees_of` (~1485–1496) retains absent protected roots using `resolve_fs(r, 0)` instead of dropping canonicalization failures.
     - `in_trees` (~1500–1525) applies `deny::Sensitive::hit(real, role)` for `TreeKind::Sensitive` targets so `.pub`, `known_hosts`, and read-only `.ssh/config` / `authorized_keys` keep their contract §9 exceptions.
     - `prune_mode` / `is_pruned` / `links_below` (~1527–1676) prunes directories only when the specific invocation excludes them (`grep*`/`find` prune nothing; `rg`/`ag` prune `.git` only unless `--hidden`/`-.`/`-u(u)` is set or flags are unreadable; `ack` prunes `.git`/`node_modules` unless `--noignore-dir` is set or flags are unreadable; walkers (tar, cp, zip, rsync, ...) prune nothing; `.git`/`node_modules`/`target` are walked on a separate 65536-entry budget, all inside the check's single shared budget).
     - `raw_glob_pattern`, `expand_targets`, `first_tree_hit`, and `reaches_tree_through_links` (~1685–1923) evaluate `ToolSubject::Paths { glob: Some(..), .. }` with file-tool dotfile matching and recursive basename expansion (`**/<trimmed>`) when `recursive == true` for single-component globs.
   - `crates/atrium-core/tests/adversarial_gate.rs`: removed `#[ignore = "M1-13 finding 10"]` on `finding_10_a_glob_that_reaches_a_credential_through_a_link_is_blocked` (~571).
   - `crates/atrium-core/tests/M1-32-link-globs.rs`: added S11 tests, ordinary-folder rows (`ls proj_clean/to_other`, `cat proj_clean/to_other`), and regression tests for rework round 1 findings 1–6.
2. **K5 closed under D-125:** `cwd=proj|rg -L pat` keeps `["Deny", "Deny", "Deny"]` as asserted in `crates/atrium-core/tests/adversarial_m1_18.rs:1030`.
3. Ran `cargo fmt --all` and `cargo check -p atrium-core --tests` (D-123: workers do not run `cargo test` or `cargo clippy`); both exited `0`.

---

## Rework round 1

- **Finding 1 (blocker — file-tool globs inherit shell matching semantics):** fixed in `expand_glob_with` and `reaches_tree_through_links` (`include_dotfiles = true` and `**/<trimmed>` basename expansion for recursive single-component file-tool globs); pinned by `rework1_finding1_file_tool_globs_match_dotfiles_and_recursive_basenames` in `crates/atrium-core/tests/M1-32-link-globs.rs`.
- **Finding 2 (major — dangling links to absent credential files lose protection):** fixed in `trees_of` using `resolve_fs(r, 0)` instead of filtering out `std::fs::canonicalize` failures; pinned by `rework1_finding2_dangling_links_to_absent_credential_files_are_denied` in `crates/atrium-core/tests/M1-32-link-globs.rs`.
- **Finding 3 (major — dangling targets normalize `..` before resolving intermediate links):** fixed in `real_path` (`gate.rs:992-1079`) by resolving components and symlinks in filesystem order before applying `..`; pinned by `rework1_finding3_dangling_targets_resolve_links_before_dotdot` in `crates/atrium-core/tests/M1-32-link-globs.rs`. (The sibling `normalize(&target)` call in `resolve_fs` at `gate.rs:84` is outside M1-32's owned paths and is escalated below.)
- **Finding 4 (blocker — following searches silently skip directories the command reads):** fixed in `prune_mode` / `is_pruned` / `links_below` so `grep -R` and `find -L` prune nothing, `rg -L` / `ag` prune `.git` only when hidden search is not enabled and flags parse cleanly, and `ack` prunes `.git`/`node_modules` unless `--noignore-dir` or unknown flags; pinned by `rework1_finding4_following_searches_do_not_skip_read_dirs` in `crates/atrium-core/tests/M1-32-link-globs.rs`.
- **Finding 5 (major — public SSH files become hard-denied through file-tool globs):** fixed in `in_trees` / `first_tree_hit` / `evaluate_sync` using `deny::Sensitive::hit(real, role)` for `TreeKind::Sensitive` targets (preserving `.pub`, `known_hosts`, and read-only `.ssh/config` / `authorized_keys`); pinned by `rework1_finding5_public_ssh_and_readable_config_keep_ordinary_verdicts` in `crates/atrium-core/tests/M1-32-link-globs.rs`.
- **Finding 6 (major — unreadable glob directories are treated as successfully checked):** fixed in `expand_glob_with` by treating `read_dir`, directory-entry, `symlink_metadata`, and `metadata` errors (other than `NotFound` or non-directory targets) as `truncated = true` while preserving any already-found matches; pinned by `rework1_finding6_unreadable_glob_dir_fails_closed_to_ask` in `crates/atrium-core/tests/M1-32-link-globs.rs`.
- **Finding 7 (minor — handoff still requests the already-decided K5 change):** fixed in `docs/agents/handoffs/M1-32-link-globs.md` by recording K5 as closed under D-125 and removing the blocked/decision-needed request.
- **Test gaps:** added `rework1_finding1_*`..`rework1_finding6_*` and explicit ordinary-folder rows (`ls proj_clean/to_other`, `cat proj_clean/to_other` -> `["Allow", "Allow", "Allow"]`) in `crates/atrium-core/tests/M1-32-link-globs.rs`.

---

## Rework round 2

- **Regression in `adversarial_m1_18::k2_links_at_any_hop_unexpanded_globs_and_following_walkers` (`cat big_dir/*/id_rsa`):**
  - Root cause: in rework round 1 (`9de4899`), `expand_glob_with` (`crates/atrium-core/src/policy/gate.rs:933,943`) changed `seen > MAX_ENTRIES` and `stack.len() > MAX_ENTRIES` from `return None;` to `truncated = true; break;`. On `big_dir` (4150 dirs + `z_ssh -> ~/.ssh`), `read_dir` pushed up to 4096 entries onto `stack` before breaking (including `z_ssh` depending on filesystem iteration order), and the subsequent literal component `"id_rsa"` (`!is_pat`) skipped `read_dir` and resolved `big_dir/z_ssh/id_rsa` via `symlink_metadata`, turning the over-budget glob from `Ask` (`Reach::Maybe`) into `HardDeny` (`Reach::Yes`).
  - Fix (kept from WIP `3b43254` and verified): restored `return None` in `expand_glob_with` when `seen > MAX_ENTRIES` or `stack.len() > MAX_ENTRIES` (matching `dev` for over-budget glob walks while keeping `truncated = true` for unreadable directories / I/O errors from Finding 6 and `MAX_DEPTH` in `**`).
  - Additional check/fix in `reaches_tree_through_links` (`crates/atrium-core/src/policy/gate.rs`): in the `u.word.contains("..") && u.word.contains(['*', '?', '['])` brace-expansion branch, gated `expand_targets(&raw_p, ...)` on `raw_p` containing glob/meta characters (`['*', '?', '[', '{', '}', '$', '`']`), matching the other two glob-expansion call sites so non-glob brace branches are not treated as failed glob expansions (`maybe = true`).
  - Pinned by `rework2_over_budget_glob_and_non_ssh_id_rsa_do_not_hard_deny` in `crates/atrium-core/tests/M1-32-link-globs.rs` (verifies `cat big_dir/*/id_rsa` -> `["Ask", "Ask", "Ask"]`, `cat big_plain/*/id_rsa` -> `["Ask", "Ask", "Ask"]`, `cat plain/*/id_rsa` -> `["Ask", "Allow", "Allow"]`, `Glob` `plain` `id_rsa` / `*/id_rsa` -> `["Allow", "Allow", "Allow"]`, and `Glob` `big_dir` / `big_plain` -> `["Ask", "Ask", "Ask"]`).
  - Ran `cargo fmt --all` and `cargo check -p atrium-core --tests` (D-123); both exited `0`.

---

## Escalation (Opus, D-127)

Review round 2 (`.orchestration/review-input/review-round2.md`), all findings closed in `crates/atrium-core/src/policy/gate.rs`; each test reproduces the reviewer's case and failed before the fix (commit "failing regressions for review round 2 findings 1-5"):

- **Finding 1 (blocker, relocated `~/.ssh -> vault`):** fixed — `Tree` keeps a `logical` spelling (root's folder resolved, its name kept) and `in_trees` classifies a Sensitive target by both its real path and that alias, so the public-file exceptions still apply. Test: `rework2_finding1_relocated_ssh_store_keeps_credential_protection` (`cat proj/key*`, Glob `key*` → Deny ×3; `pub*` keeps Ask/Allow/Allow and Allow ×3).
- **Finding 2 (blocker, non-UTF-8 link names):** fixed — `expand_glob_with` matches and joins the entry's own `OsString`/`e.path()`, no lossy copy. Test: `rework2_finding2_non_utf8_link_names_are_resolved_by_their_bytes` (`cat proj/k*`, Glob `k*` → Deny ×3).
- **Finding 3 (blocker, `tar --dereference` skipping `node_modules`/`.git`/`target`):** fixed — `prune_mode` defaults to `PruneDirs::None`; only rg/ag/ack prune what they really skip; `VcsAndBuild` removed. `.git`/`node_modules`/`target` are now walked on a separate 65536-entry budget in `links_below`, so ordinary `cp -r`/`zip -r repo_with_git` keep `dev`'s Ask/Allow/Allow (`adversarial_m1_18::k2_*`). Test: `rework2_finding3_dereferencing_tar_walks_pruned_dirs`.

Commands run (Opus may run tests): `cargo fmt --all --check` ok; `cargo clippy -p atrium-core --all-targets -- -D warnings` ok; `cargo test -p atrium-core`: all pass except the known Linux/timing failures also seen on `dev` (`adversarial_regression::round_5_blocker_1` N9, `adversarial_fuzz::random_hostile_lines…`, `adversarial_m1_18::fix4…` N11), plus `policy::engine_never_panics_on_garbage` (10.1 s, fails identically with `dev`'s `gate.rs`) and `adversarial_m1_18::fix2_nested_env_split_string_is_linear` (flaky 120–290 ms around a 200 ms cap; flaky identically before this escalation).
- **Finding 4 (major, unreadable folder in a following search):** fixed — `links_below` turns `read_dir`, entry, `file_type` and link-`canonicalize` errors (other than not-found, not-a-dir, link loop) into `Reach::Maybe`, keeping a sure hit. Test: `rework2_finding4_unreadable_dir_in_following_search_fails_closed` (`grep -R secret proj` → Ask ×3).
- **Finding 5 (major, file-tool dangling target `dir/../new_key`):** fixed inside the owned range — `raw_glob_pattern` no longer resolves a pattern-free path lexically (callers `real_path` it), and `reaches_tree_through_links` re-resolves each literal use with `real_path` from its raw spelling when that differs from the lexical resolution. Test: `rework2_finding5_file_tool_dangling_targets_resolve_links_before_dotdot` (Read and Glob → Deny ×3). `resolve_fs` itself (~line 84) is still unchanged (see Escalated).
- **Finding 6 (nit, handoff entry-cap wording):** fixed in the summary above.

---

## Escalation round 2 (D-127, review 3)

Approach change (coordinator): one fail-closed rule instead of case-by-case fixes, all in `gate.rs`'s owned range:

- **Rule A, canonical credential set:** each store root (`deny::sensitive_roots`) *and each member of `~/.ssh`* becomes a tree at its real path (`real_path`, links followed in order), classified through its whole spelling below the resolved home (`trees_in_home`). Every candidate (glob match, walked link, literal path) is resolved with `real_path` and compared with that set; `Sensitive::hit` on the real path or the home spelling keeps the `.pub`/`known_hosts`/read-only-config exceptions. A file tree (an SSH member) is never "named as a folder", so public members stay ordinary.
- **Rule B, bytes, not text:** glob expansion keeps every component, prefix and entry name as `OsString`; only a pattern component is read as text, and one that is not UTF-8 makes the expansion fail (Maybe → Ask).
- **Rule C, literal means literal:** a file tool's path is re-resolved with `real_path` whatever characters it holds; only shell words treat metacharacters as patterns.
- **Rule D, one budget per check:** `reaches_tree_through_links` runs under one shared budget (`LINK_CHECK_BUDGET` = 262144 filesystem operations: glob entries, literal lookups, walk entries, target resolutions). Running out makes the check Maybe (Ask, never Allow); a definite hit stays a hard deny. Roots, brace alternatives and candidates are deduplicated first. Unreadable folders, read errors and undecodable names already fail closed (round 2 findings 4 and 6 of round 1).

Findings:
- **Finding 1 (blocker, relocated parent `~/.config -> config_store`):** closed by rule A. Test: `rework3_finding1_relocated_parent_of_a_store_keeps_protection`.
- **Finding 2 (blocker, non-UTF-8 glob prefix):** closed by rule B. Test: `rework3_finding2_non_utf8_glob_prefix_is_kept_by_its_bytes`.
- **Finding 3 (blocker, `~/.ssh/id_rsa -> ../vault/key`):** closed by rule A (SSH members in the set). Test: `rework3_finding3_ssh_member_linked_out_of_ssh_is_protected`.
- **Finding 4 (major, Read `proj/dangling-key[1]`):** closed by rule C. Test: `rework3_finding4_literal_file_tool_path_with_brackets_is_resolved`.
- **Finding 5 (major, per-walk caps):** closed by rule D. Test: `rework3_finding5_one_budget_per_check` (the reviewer's 4096 roots × 256 alternatives finishes quickly and is not denied; 256 distinct alternatives over 4096 entries exceed the budget → Ask ×3).
- **Finding 6 (nit, stale pruning line):** fixed in the summary above.

Tests 1–4 fail with the previous `gate.rs` (`5bec74e`) and pass now; test 5 on the previous code is the reviewer's ~4·10⁹-entry walk (stopped, not run to the end).

Commands run: `cargo fmt --all --check` ok; `cargo clippy -p atrium-core --all-targets -- -D warnings` ok; `cargo test -p atrium-core`: `M1-32-link-globs` 21/21, `adversarial_gate`, `adversarial_m1_18::k2_*` and every other verdict test pass. The only failures are the known ones (`round_5_blocker_1` N9, `adversarial_fuzz` and `adversarial_m1_18::fix4` timing N11) plus timing caps that are flaky under load: `fix2_nested_env…`, `policy::engine_never_panics_on_garbage` (fails on `dev`'s `gate.rs` too) and `policy::pipe_to_shell_fan_out…` (engine only, does not touch `gate.rs`; passed 1 of 3 runs here, around its 2 s cap).

Added scope (D-120, brief "Added scope"), both former limits closed:
- **`resolve_fs` (~84):** now delegates to `real_path`, so every caller follows links in filesystem order before `..`; ordinary paths resolve as before (canonical when they exist, deepest existing part plus tail otherwise). Test: `added_scope_resolve_fs_follows_links_before_dotdot` (`fs_resolve("proj/dangling-key")` → `~/.ssh/new_key`; `printf x > proj/dangling-key`, `cat proj/dangling-key` → Deny ×3; `proj/a.txt` and `proj/missing/../a.txt` unchanged).
- **`alias_fix` for globs and walks:** the credential identity map (dev, inode) used for named paths is installed for the link check (`with_identity_roots`); every glob match, literal re-resolution, walked link target and multiply-linked file met on a following walk goes through `identity_hit` (`alias_fix` + the plain classifier), charged to the same per-check budget. Test: `added_scope_hard_links_met_by_globs_and_walks_match_by_identity` (`cat proj/h*`, Glob `h*`, `grep -R secret proj`, `tar --dereference …` → Deny ×3; an ordinary hard link keeps its verdict).
- Both tests fail with the previous `gate.rs` (`d09a373`) and pass now. Re-run: clippy clean; `M1-32-link-globs` 23/23; same failure set as above (known N9/N11 plus the load-sensitive timing caps), no verdict test fails.
- Note: a non-following recursive read (`grep -r` without dereference) is not walked by the link check, as on `dev`. Whether a hard-linked credential inside such a folder is caught was not checked in this task; flagged for the reviewer.

---

## Escalated (outside owned paths)

None: `resolve_fs` was brought into scope by D-120 and fixed above.

---

## Tests added and expected behavior on `dev`

Under D-123, `cargo test` was not run by this worker; compilation was verified with `cargo check -p atrium-core --tests`.

1. `crates/atrium-core/tests/M1-32-link-globs.rs`:
   - `s11_globs_through_links_match_direct_verdicts`: pins `["Deny", "Deny", "Deny"]` for one-hop and two-hop symlinks with `*`/`?`/`[...]`/`**`/`{a,b}`, dangling symlinks, links to ancestors of credential trees, `..` after a globbed symlink, and follow-flag searches.
   - `s11_file_tool_globs_through_links_are_blocked`: pins `["Deny", "Deny", "Deny"]` for 9 `ToolSubject::Paths` checks.
   - `s11_unbounded_brace_globs_fail_closed_to_ask`: pins `["Ask", "Ask", "Ask"]` for `cat proj/{1..65}` and `["Deny", "Deny", "Deny"]` for brace overflow.
   - `s11_ordinary_searches_and_globs_with_ordinary_links_keep_dev_verdicts`: pins `dev` verdicts for ordinary searches, folder operands (`ls proj_clean/to_other`, `cat proj_clean/to_other`), globs, and non-following searches in folders with `.ssh` links.
   - `rework1_finding1_*` .. `rework1_finding6_*`: pin each of findings 1–6 (reproducing cases that fail prior to `gate.rs` rework round 1 fixes and pass after).
   - `rework2_over_budget_glob_and_non_ssh_id_rsa_do_not_hard_deny`: pins `["Ask", "Ask", "Ask"]` for over-budget globs (`big_dir/*/id_rsa`, `big_plain/*/id_rsa`, and `Glob` over `big_dir`/`big_plain`) and ordinary verdicts (`["Ask", "Allow", "Allow"]` / `["Allow", "Allow", "Allow"]`) for `id_rsa` outside `~/.ssh`.
2. `crates/atrium-core/tests/adversarial_gate.rs`:
   - `finding_10_a_glob_that_reaches_a_credential_through_a_link_is_blocked` (~571): un-ignored in `477fe81`.

---

## Commands for the verification task (D-123)

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
for i in 1 2 3; do cargo test -p atrium-core; done
```

Also verify the fail-on-`dev` check by checking out `dev:crates/atrium-core/src/policy/gate.rs`, running `cargo test -p atrium-core --test M1-32-link-globs`, and restoring `HEAD`'s `gate.rs`.
