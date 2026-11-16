# M1-32-link-globs: handoff

**State: S11 complete (`477fe81f`); K5 blocked (needs driver/orchestrator decision).**

Brief read from `dev`: `docs/agents/briefs/M1-32-link-globs.md`.

---

## What this round did

1. Inspected `477fe81f` (S11) and `dev` (`174a2802`) across `crates/atrium-core/src/policy/gate.rs`, `crates/atrium-core/tests/M1-32-link-globs.rs`, `crates/atrium-core/tests/adversarial_gate.rs`, `crates/atrium-core/tests/adversarial_regression.rs`, and `crates/atrium-core/tests/adversarial_m1_18.rs`.
2. Confirmed **S11 is implemented and tested** in `477fe81f`:
   - `crates/atrium-core/src/policy/gate.rs`:
     - `evaluate_sync` (~327, ~347) passes `&c.cwd` and `&c.home` into `reaches_tree_through_links`.
     - `expand_glob` (~797–884) returns `Option<GlobExpansion>` (`matches`, `truncated`), tracks visited `(canonical_dir, comp_idx)` for `**` to avoid spinning on ancestor/loop links, and sets `truncated = true` instead of discarding shallow matches when `deep > MAX_DEPTH`.
     - `real_path` (~911–936) follows dangling symlinks by hand (`symlink_metadata` + `read_link` + `resolve_fs(&normalize(&target), 1)`), resolving relative targets against the canonicalized parent.
     - Added `raw_glob_pattern`, `expand_targets`, `first_tree_hit`, and `ToolSubject::Paths { glob: Some(..), .. }` handling plus `..`-after-glob resolution against `unit_cwd` inside `reaches_tree_through_links` (~1419–1640).
   - `crates/atrium-core/tests/adversarial_gate.rs`: removed `#[ignore = "M1-13 finding 10"]` on `finding_10_a_glob_that_reaches_a_credential_through_a_link_is_blocked` (~571).
   - `crates/atrium-core/tests/M1-32-link-globs.rs`: added 327 lines of tests covering S11.
3. Ran `cargo fmt --all --check` and `cargo check -p atrium-core --tests` (D-123: workers do not run `cargo test` or `cargo clippy`); both exited `0`.
4. **Stopped on K5** without editing `gate.rs` further because implementing K5 as written in the brief directly conflicts with existing tests on `dev`, the brief's owned-path rules, and the "no line more permissive than `dev`" rule (details below).

---

## Why K5 is blocked

The brief asks for:
> K5: `rg -L` (and `--follow`, `grep -R`, `find -L`) with no folder named, from a folder that holds a link into a sensitive tree: ask in Home (`dev`'s verdict), not a hard deny; keep the hard deny when the line names the credential.

1. **Breaks existing non-owned tests on `dev`:**
   `dev` already asserts `Deny` (across `Home`, `Any`, and `FullAccess`) for following searches with no folder (or `.`) from a directory holding a link into `~/.ssh`:
   - `crates/atrium-core/tests/adversarial_regression.rs:2401-2404` (`round_8_blocker_2_generated_inbound_links_through_globs_and_following_searches`): `"cwd=proj|rg -L id"`, `"cwd=proj|find -L ."`, `"cwd=proj|grep -R id ."`, `"cwd=proj|ag --follow id"` all require `GateDecision::Deny` in `Home`, `Any`, and `FullAccess`.
   - `crates/atrium-core/tests/adversarial_m1_18.rs:930-932, 1030` (`gate_rows`): `"cwd=proj_2hop|rg -L id"`, `"cwd=proj_3hop|rg -L id"`, `"cwd=proj_2hop|grep -R id ."`, `"cwd=proj_3hop|grep -R id ."`, `"cwd=proj_2hop|ag --fol id"`, `"cwd=proj_3hop|ag --fol id"`, and `"cwd=proj|rg -L pat"` all assert `["Deny", "Deny", "Deny"]`.
   - `crates/atrium-core/tests/adversarial_regression.rs:1749-1779` (`round_6_item_2_readers_without_a_folder_from_linked_dir`): `"cwd=vault|grep -R x"`, `"cwd=vault|find -L -print"`, `"cwd=vault|rg x"`, etc. (where `~/.ssh/work -> ~/vault/work`) require `GateDecision::Deny` in all settings.
   - The brief's rules forbid modifying existing test files other than removing the `#[ignore]` at `adversarial_gate.rs:571`.
2. **Violates "no line more permissive than `dev`" and creates a real credential leak:**
   - The brief's `(`dev`: Ask/Allow/Allow)` note comes from `docs/agents/m1-must-fix.md`, which referred to *pre-M1-13* `dev` (`02c13bc^`). On current `dev` (`174a2802`), `cwd=proj|rg -L pat` is `["Deny", "Deny", "Deny"]`.
   - Returning `["Ask", "Allow", "Allow"]` would be more permissive than `dev` and would let `FullAccess` or an `Any`-rule session run `rg -L id_rsa` in `proj` (with `proj/vault_link -> ~/.ssh`) and read `~/.ssh/id_rsa` without hard-denying.
   - Even changing only `Home` to `Ask` (`["Ask", "Deny", "Deny"]`) still makes `Home` more permissive than `dev` and still fails `adversarial_regression.rs` and `adversarial_m1_18.rs` (both check `Preset::Home` for `Deny`).
3. **Owned-paths boundary for round-6 `reaches_linked_ssh_dir`:**
   - The round-6 case (`cwd=vault` with `~/.ssh/work -> ~/vault/work`, cited in `docs/agents/handoffs/M1-13-adversarial.md:849`) is enforced by `reaches_linked_ssh_dir` (`gate.rs:942-1044`) and its caller (`gate.rs:302-314`), which are outside M1-32's owned lines (`reaches_tree_through_links` and its callers at `~320-350`).

Note: cases that *are* ordinary on `dev` — `rg -L pat safe_sub` from `proj`/`vault`, `rg -e -L` / `rg -f -L` / `rg -rL pat` with `-L` as an option value, and `cwd=asks|rg -L x` for `ask_roots` (CR-12) — already return `["Ask", "Allow", "Allow"]` on `dev`.

**Decision needed:** Either grant an exception for K5 under D-120 (keeping `dev`'s `Deny` on `cwd=proj|rg -L pat` etc.), or widen M1-32's owned paths to update `adversarial_regression.rs` / `adversarial_m1_18.rs` / `M1-32-link-globs.rs` and confirm what verdict (`Ask/Allow/Allow` vs `Ask/Deny/Deny`) is wanted.

---

## Tests added (in `477fe81f`) and expected behavior on `dev`

Under D-123, `cargo test` was not run by this worker; the `dev` vs branch comparison below is by inspection of `git show dev:crates/atrium-core/src/policy/gate.rs` vs `HEAD`.

1. `crates/atrium-core/tests/M1-32-link-globs.rs`:
   - `s11_globs_through_links_match_direct_verdicts`: pins `["Deny", "Deny", "Deny"]` for one-hop and two-hop symlinks with `*`/`?`/`[...]`/`**`/`{a,b}`, dangling symlinks (one-hop, two-hop, relative inside a linked dir), links to ancestors of credential trees (`proj/link-home`, `proj/link-home2`), `..` after a globbed symlink (`cat proj/link-s*/../id_rsa`), and follow-flag searches (`rg -L`, `rg --follow`, `grep -R`, `grep --dereference-recursive`, `grep --deref`, `find -L`, `find ... -follow`, `ag -f`, `ag --follow`, `ack --follow`) on folders/globs/`.` holding links to `.ssh` or `~`.
     - **Expected to fail on `dev`'s code:** dangling link globs (`cat proj/dangling*`, `cat proj/dangling-*`, `cat proj/dangling-2*`, `echo x > proj/dangling*`, `cat proj/link-sub/dangling_*`) because `dev`'s `real_path` does not follow dangling symlinks; `**` globs with ancestor/loop links (`cat proj/**`, `cat proj/**/id_rsa`, `cat proj/**/.ssh/id_rsa`, `cat proj/**/.npmrc`) because `dev`'s `expand_glob` returns `None` (`Ask`) on `MAX_DEPTH` without `visited` or `truncated`; and `cat proj/link-s*/../id_rsa` (`dev` does not re-resolve `..` after a glob component against `unit_cwd`).
   - `s11_file_tool_globs_through_links_are_blocked`: pins `["Deny", "Deny", "Deny"]` for 9 `ToolSubject::Paths` (`Glob`/`Grep`) checks with `*/id_rsa`, `link-*`, `dangling*`, `**/id_rsa`, `[l]ink-ssh/id_rsa`, `proj/*/id_rsa`, `link-key`, `*/*/.ssh/id_rsa`.
     - **Expected to fail on `dev`'s code:** all 9 checks (`dev`'s `reaches_tree_through_links` does not evaluate `ToolSubject::Paths { glob: Some(..), .. }`).
   - `s11_unbounded_brace_globs_fail_closed_to_ask`: pins `["Ask", "Ask", "Ask"]` for `cat proj/{1..65}` and `["Deny", "Deny", "Deny"]` for brace-expansion overflow.
     - **Expected on `dev`:** passes (same `words::brace_expand` / `analysis.overflow` behavior).
   - `s11_ordinary_searches_and_globs_with_ordinary_links_keep_dev_verdicts`: pins `dev`'s verdicts (`Allow` or `Ask/Allow/Allow`) for searches/globs in `proj_clean` (symlinks to `other_proj`) and non-following searches (`rg id proj_sub`, `grep -r id proj_sub`) in a folder holding a symlink to `.ssh`, plus 3 `Glob` tool checks.
     - **Expected on `dev`:** passes.
2. `crates/atrium-core/tests/adversarial_gate.rs`:
   - `finding_10_a_glob_that_reaches_a_credential_through_a_link_is_blocked` (~571): un-ignored in `477fe81f`; pins `Deny` in `Home` for `cat proj/*/id_ed25519` and `cat proj/link-*`. (Already returned `Deny` on `dev` since M1-13 round 8, though previously `#[ignore]`.)

---

## Commands for the verification task (D-123)

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
for i in 1 2 3; do cargo test -p atrium-core; done
```

Also verify the fail-on-`dev` check by checking out `dev:crates/atrium-core/src/policy/gate.rs`, running `cargo test -p atrium-core --test M1-32-link-globs`, and restoring `HEAD`'s `gate.rs`.
