# Handoff: M1-29-cr17-stores

- **Branch:** `agent/hydrogen/M1-29-cr17-stores` (cut from `a3b2a60` on `dev`)
- **Status:** Code and tests complete; formatted and checked (`cargo fmt --all`, `cargo check -p atrium-core --tests`, `cargo check -p atrium-daemon --tests` passed). Under D-123, test suites, `cargo clippy`, and `pnpm` were not run here and are listed below for the verification task.

## What changed and where

1. **CR-17 credential stores (`crates/atrium-core/src/policy/deny.rs`, `engine.rs`, `tests/adversarial_policy.rs`, `app/src/ui/Security.tsx`, `app/src/ui/security.test.ts`, `docs/contracts/m1.md`):**
   - Added `.gem/credentials`, `.terraform.d/credentials.tfrc.json`, `.config/op/config`, and `.yarnrc.yml` to `ASK_FILES` in `crates/atrium-core/src/policy/deny.rs` (lines 445–468). Left `ASK_TREES` and everything below it untouched.
   - Added `.gem/credentials`, `.terraform.d/credentials.tfrc.json`, and `.config/op/config` to `GIT_CRED_SUFFIXES`, and `.yarnrc.yml` and `credentials.tfrc.json` to `GIT_CRED_NAMES` in `crates/atrium-core/src/policy/engine.rs` (lines 536–570) so `git show` / `git cat-file` of committed stores asks.
   - Removed `#[ignore]` from `finding_6_stores_outside_the_cr_12_list_ask_in_home` in `crates/atrium-core/tests/adversarial_policy.rs` (line 1138).
   - Implemented `ASK_STORES` and `CredentialStoresThatAsk()` in `app/src/ui/Security.tsx` (lines 46–103) listing every CR-12 and CR-17 store that asks, and added `app/src/ui/security.test.ts`.
   - Updated `docs/contracts/m1.md` §9 (marked `(rev 3.2, CR-17, D-111)`).

2. **Path-scope semantics (`crates/atrium-core/src/policy/engine.rs`):**
   - Updated `all_paths` (with `looks_like_path` and `is_command_path`) so command calls collect only the paths the command names (redirect targets and path-shaped arguments/operands), excluding `a.start` (working directory) and plain non-path words (`push`, `test`, `status`, etc.). File tools (`u.unit == usize::MAX`) keep all resolved paths.
   - Updated `path_scope_ok` so:
     - `RuleDecision::Allow` requires `!paths.is_empty() && paths.iter().all(|p| glob_matches(g, p))` (a command that names no path is never covered by a scoped allow rule).
     - `RuleDecision::Deny` requires `paths.iter().any(|p| glob_matches(g, p))` (fail closed if any named path is inside the scope).

3. **Derived units, `find -delete`, `xargs`, and inline code under prefix rules (`crates/atrium-core/src/policy/engine.rs`, `rule_check.rs`):**
   - Updated `apply_rules`, `allow_covers`, and added `rule_covers_argv` in `engine.rs` so command allow rules (`CommandPrefix`, `CommandExact`) cover a call only when every derived unit (`u.derived`: `find -exec`/`-execdir`/`-ok`/`-okdir`, `sh -c`/`eval` payloads, etc.) and every wrapper target (`u.eff` for `xargs`, `env`, etc., non-empty and not still a wrapper/shell) are also covered by allow rules (or covered by `RuleMatcher::Any` / `PathGlob`); and lower-ranked `Deny` rules still check uncovered derived units before later ranks can cover them.
   - `command_prefix_blocked` / `command_exact_blocked` (in `rule_check.rs`) ensure `find -delete` is never covered by a `CommandPrefix` rule on `find`, malformed `find -exec` / unlexed uppercase `FIND -EXEC` fail closed, and interpreters/package runners with inline code (`-c`, `-e`, `-r`, `--eval`, `deno eval`, `bun exec`, `pwsh -c`, `npx -yc`, etc.) or no pinned script/package are never covered by a prefix rule on the interpreter/runner, while ordinary lines (`find . -name '*.rs'`, `python3 scripts/build.py`) keep `Allow`.

4. **Shared rule validator and daemon `CreateRule` guard (`crates/atrium-core/src/policy/rule_check.rs`, `policy/mod.rs`, `crates/atrium-daemon/src/commands.rs`, `docs/contracts/m1.md`):**
   - Added `crates/atrium-core/src/policy/rule_check.rs` (exported via `crates/atrium-core/src/policy/mod.rs`) as the Rust source of truth matching `shell.rs`, `deny.rs`, and `app/src/chat/rule-form.ts` (`fold_name`, `SHELLS`, `RUNS_HERE`, `WRAPPERS` including `PRIV`, `INTERPRETERS`, `pwsh`, `PACKAGE_RUNNERS`, `STDIN`, `FIND_EXEC`, `RESERVED`, and `pathScopeOffered`).
   - Updated `create_rule` in `crates/atrium-daemon/src/commands.rs` to call `atrium_core::policy::validate_rule_draft(&draft)` and return `ErrorCode::Invalid` on refusal.
   - Updated `docs/contracts/m1.md` §9 (marked `(rev 3.2, D-121)`).

## Tests added

- **`crates/atrium-core/tests/adversarial_policy.rs`:** un-ignored `finding_6_stores_outside_the_cr_12_list_ask_in_home` (CR-17 stores ask in Home, allow under `Any`/`FullAccess`).
- **`crates/atrium-core/tests/M1-29-cr17-stores.rs`:**
  - `cr17_stores_ask_in_home_and_keep_allow_under_rules_and_full_access`: pins `Ask` in Home and `Allow` under `Any`/`FullAccess` for reading, copying, searching, and `git show`/`git cat-file` across `~/.gem/credentials`, `~/.terraform.d/credentials.tfrc.json`, `~/.config/op/config`, `~/.yarnrc.yml` (home-anchored, `$HOME`, relative, and `Read`/`Search` tools).
  - `cr17_ordinary_siblings_keep_dev_verdict`: pins `Allow` in Home for `.yarnrc`, `~/.gem/specs`, and `~/.terraform.d/plugins` (command, `git show`, `Read`, `Search`).
  - `cr17_gate_catches_symlinks_and_hardlinks_to_new_stores`: pins `SessionGate` symlink and hard-link detection for the four CR-17 stores while keeping `proj/safe.txt` as `Allow`.
  - `path_scope_allow_and_deny_for_commands_and_file_tools`: pins allow (`.all()`) and deny (`.any()`) path-scope behavior for single- and multi-path commands and file tools, in Home, `Env::cwd`, and `ToolSubject::Command { cwd: Some(...) }`, plus confirming commands naming no path never match scoped allow rules.
  - `find_prefix_rules_require_derived_units_to_be_covered_and_reject_delete`: pins `find . -name '*.rs'` -> `Allow`, `find . -delete` -> `Ask`, `find . -exec`/`-execdir`/`-ok`/`-okdir` / `FIND . -EXEC` -> `Ask` under `find` prefix alone, `Allow` when derived unit is also covered, `Deny` when lower-ranked `Deny` hits derived unit, and `Allow` for `CommandExact` on `find . -delete`.
  - `xargs_and_inline_code_and_shells_never_bypass_prefix_rules`: pins `xargs` target coverage (`echo a | xargs rm` asks without `rm` rule, allows with `rm` rule, bare `xargs` asks), shell prefix refusal (`sh -c`, `bash -c`, `eval`), inline-code/stdin/bare-invocation refusal under prefix rules across `python3`, `node`, `perl`, `ruby`, `php`, `lua`, `deno`, `bun`, `pwsh`, `npx`, and `Allow` for ordinary script runs (`python3 scripts/build.py`, pinned script with trailing `-c`, `node build.js`, `pwsh -File build.ps1`).
- **`crates/atrium-daemon/tests/create_rule.rs`:**
  - `create_rule_refuses_unsafe_command_allow_rules_and_invalid_path_scopes`: pins daemon `CreateRule` `ErrorCode::Invalid` refusals for allow rules (prefix & exact) on shells (incl. case/APFS lookalikes), `eval`/`source`/`.`, wrappers/privilege tools, interpreters with inline code/stdin/subcommands/no script, package runners with `-c`/`--call`/no package, `find` prefix, `find -exec`/`-execdir`/`-ok`, and `path_scope` on command, deny, and non-file rules; plus acceptance of ordinary allow rules, deny rules on shells/wrappers, and `path_scope` on file-tool allow rules.
- **`app/src/ui/security.test.ts`:**
  - `lists every CR-12 and CR-17 store in ASK_STORES` and `renders the ask-stores list inside SecurityPage`: pins that `ASK_STORES` and `<CredentialStoresThatAsk />` render all CR-12 and CR-17 stores inside `SecurityPage`.

## Verdict table (expected `dev` vs now)

*(Under D-123, `cargo test` was not run in this worker; `dev` vs now verdicts below are from inspection and must be confirmed by the verification task.)*

| Line / Call | Rules / Preset | `dev` (expected) | Now (expected) |
|---|---|---|---|
| `cat ~/.gem/credentials` (and `.terraform.d/credentials.tfrc.json`, `.config/op/config`, `.yarnrc.yml`) | Home, no rules | `Allow` | `Ask` |
| `git show HEAD:.gem/credentials` (and the other three) | Home, no rules | `Allow` | `Ask` |
| `rg token ~/.gem` / `~/.terraform.d` / `~/.config/op` | Home, no rules | `Allow` | `Ask` |
| `cat ~/.yarnrc`, `cat ~/.gem/specs/index`, `rg pat ~/.terraform.d/plugins` | Home, no rules | `Allow` | `Allow` (unchanged) |
| `rm ~/work/a.txt` (in Home `cwd = ~`) | Allow `rm`, `path_scope: ~/work/**` | `Ask` | `Allow` |
| `cargo test --manifest-path ~/work/Cargo.toml` (in Home) | Allow `cargo test`, `path_scope: ~/work/**` | `Ask` | `Allow` |
| `cargo test` (in `cwd = ~/work`) | Allow `cargo test`, `path_scope: ~/work/**` | `Ask` | `Ask` (names no path) |
| `cp ~/work/a.txt ~/other/b.txt` | Deny `cp`, `path_scope: ~/work/**` | `Ask` (fail open) | `Deny` |
| `Edit ["~/work/a.rs", "~/other/b.rs"]` | Deny `Edit Any`, `path_scope: ~/work/**` | `Ask` (fail open) | `Deny` |
| `find . -name '*.rs'` | Allow prefix `find` | `Allow` | `Allow` (unchanged) |
| `find build -delete` (and `find . -name '*.o' -delete` in `~/proj`) | Allow prefix `find` (or `find .`) | `Allow` | `Ask` |
| `find . -delete` (in Home or unfiltered in `~/proj`) | Allow prefix `find` | `Hard(recursive-delete-outside-cwd)` | `Hard(recursive-delete-outside-cwd)` (unchanged) |
| `find . -exec rm {} +` | Allow prefix `find` | `Allow` | `Ask` |
| `find . -exec echo {} +` | Allow prefix `find` + `echo` | `Allow` | `Allow` |
| `echo a \| xargs rm` | Allow prefix `echo` + `xargs` | `Allow` | `Ask` |
| `python3 -c 'import os'` (and `node -e`, `perl -0e`, `pwsh -c`, `npx -yc`, …) | Allow prefix on interpreter/runner | `Allow` | `Ask` |
| `python3 scripts/build.py` | Allow prefix `python3` | `Allow` | `Allow` (unchanged) |
| `CreateRule` Allow `sh`, `bash -c`, `env`, `sudo`, `python3 -c`, `find` prefix, or `path_scope` on command/deny | Daemon socket | `Ok` | `Err(Invalid)` |

## Expected failures on `dev`'s code

If `crates/atrium-core/src/` and `crates/atrium-daemon/src/` are checked out from `dev`:
- `adversarial_policy::finding_6_stores_outside_the_cr_12_list_ask_in_home` (CR-17 stores auto-allow in Home on `dev`)
- `M1-29-cr17-stores::cr17_stores_ask_in_home_and_keep_allow_under_rules_and_full_access`
- `M1-29-cr17-stores::cr17_gate_catches_symlinks_and_hardlinks_to_new_stores`
- `M1-29-cr17-stores::path_scope_allow_and_deny_for_commands_and_file_tools`
- `M1-29-cr17-stores::find_prefix_rules_require_derived_units_to_be_covered_and_reject_delete`
- `M1-29-cr17-stores::xargs_and_inline_code_and_shells_never_bypass_prefix_rules`
- `create_rule::create_rule_refuses_unsafe_command_allow_rules_and_invalid_path_scopes`

(`cr17_ordinary_siblings_keep_dev_verdict` and `app/src/ui/security.test.ts` pass on `dev` / UI branch.)

## Checks run here (D-123)

- `cargo fmt --all` — passed (exit 0)
- `cargo check -p atrium-core --tests` — passed (exit 0, 0 warnings)
- `cargo check -p atrium-daemon --tests` — passed (exit 0, 0 warnings)

## Exact DoD commands for the verification task

All commands work in `fish` and `bash`:

1. Format, lint, and workspace tests:
```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
```
2. App checks (`app/src/ui/Security.tsx` and `app/src/ui/security.test.ts` were touched):
```sh
pnpm --dir app typecheck
pnpm --dir app test
```
3. Performance loop on `atrium-core` (3 runs, inspect `adversarial_fuzz` and `policy` binary times):
```sh
cargo test -p atrium-core --test adversarial_fuzz --test policy -- --nocapture
cargo test -p atrium-core --test adversarial_fuzz --test policy -- --nocapture
cargo test -p atrium-core --test adversarial_fuzz --test policy -- --nocapture
```
4. Prove new and un-ignored tests fail on `dev`'s `src` and pass on this branch:
```sh
git checkout dev -- crates/atrium-core/src crates/atrium-daemon/src
cargo test -p atrium-core --test adversarial_policy finding_6_stores_outside_the_cr_12_list_ask_in_home
cargo test -p atrium-core --test M1-29-cr17-stores
cargo test -p atrium-daemon --test create_rule
git checkout HEAD -- crates/atrium-core/src crates/atrium-daemon/src
cargo test -p atrium-core --test adversarial_policy finding_6_stores_outside_the_cr_12_list_ask_in_home
cargo test -p atrium-core --test M1-29-cr17-stores
cargo test -p atrium-daemon --test create_rule
```
5. Owned-paths diff check (against branch base `a3b2a60`, or `dev` after merging doc commits):
```sh
git diff a3b2a60 --stat
```

## Anything not done / notes

- Under **D-123**, `cargo test`, `cargo clippy`, and `pnpm` were not run by this worker.
- `app/src/ui/Security.tsx` renders the CR-12 + CR-17 list directly in `ASK_STORES` (as permitted by the brief's owned paths, since `app/src-tauri/src/save.rs`, `app/src/platform.ts`, and `crates/atrium-protocol/src/policy.rs` are not owned paths).

## Rework round 1

- **`crates/atrium-core/src/policy/engine.rs` (`clippy::too_many_arguments`):** Grouped the per-evaluation arguments of `allow_covers` (`input`, `a`, `paths`, `glob_leaves`, `has_derived`) into `AllowCtx<'a>`, constructed once in `apply_rules` and passed by reference to all three `allow_covers` calls (`&part_ids`, `&derived_ids`, `&wrapper_eff_ids`). `allow_covers` now takes 4 arguments instead of 8 without silencing the lint.
- **`crates/atrium-core/tests/M1-29-cr17-stores.rs:490` (`find_prefix_rules_require_derived_units_to_be_covered_and_reject_delete`):** Updated the test cases so the `-delete` prefix-rejection checks target paths strictly inside the working folder (or filtered inside a non-home project folder `~/proj`):
  - **Why the original expectation (`Ask` for `find . -delete` and `find . -name '*.o' -delete` in `Env::home()`, and `Allow` for `exact(&["find", ".", "-delete"])`) was wrong:** Per contract §9 (`HARD_DENY_DEFAULTS`, checked before any rules; `DefaultPolicy` / `deny.rs::delete_blocked`), recursive deletes targeting `~`, outside `cwd`, or unfiltered `cwd` itself are hard-denied as `HardDeny { id: "recursive-delete-outside-cwd" }`. In `Env::home()`, `cwd == ~`, so `.` resolves to `~`, and even in a project folder `~/proj`, unfiltered `find . -delete` deletes `cwd` itself. Per the brief ("no new hard deny on an ordinary line that `dev` allows or asks about"; hard deny for recursive deletes outside the working folder stays), those lines are supposed to be `Hard(recursive-delete-outside-cwd)` both on `dev` and now.
  - Replaced the prefix-rejection checks with `proj.cmd_rules("find . -name '*.o' -delete", rules)` (where `proj.cwd = /h/u/proj`, filtered so `delete_blocked` does not fire; `Allow` on `dev`, `Ask` now under both `find` and `find .` prefix rules) and `env.cmd_rules("find build -delete", &find_rule)` / `env.cmd_rules("find build -name '*.o' -delete", &find_rule)` / `proj.cmd_rules("find build -delete", &find_rule)` (`Allow` on `dev`, `Ask` now), plus exact-rule checks for `find build -delete` in `env` and `find . -name '*.o' -delete` in `proj` (`Allow`), and explicitly asserted `HardDeny { id: "recursive-delete-outside-cwd" }` for `find . -delete` (in `env` and `proj`) and `find . -name '*.o' -delete` in `env`.
- **Checks run after rework round 1 (D-123):**
  - `cargo fmt --all` — passed (exit 0)
  - `cargo check -p atrium-core --tests` — passed (exit 0, 0 warnings)
  - `cargo check -p atrium-daemon --tests` — passed (exit 0, 0 warnings)

## Rework round 2

- **`app/src/ui/security.test.ts` (`pnpm typecheck` fix):**
  - Imported `type JSX` from `"solid-js"`, typed `SecurityRow = { id: string; label: string }` for `rows`, passed the explicit props type argument `<{ rows: readonly SecurityRow[]; row: (row: SecurityRow) => JSX.Element }>` to `createComponent(SecurityPage, ...)`, and typed the `row` callback parameter `(r: SecurityRow)`.
  - `app/src/ui/Security.tsx` types were left untouched and no `any` was added; the file stays `.test.ts` so `app/vite.config.ts` (`src/**/*.test.ts`) continues to pick it up.
- **Checks run after rework round 2 (D-123):**
  - `cargo fmt --all` and `cargo check` only (`pnpm` not run here per D-123; verification task should re-run `pnpm --dir app typecheck` and `pnpm --dir app test`).

## Rework round 3

- **Finding 1 (`Path filtering drops real operands and fails open`) — fixed:** Replaced the spelling heuristic in `crates/atrium-core/src/policy/engine.rs` (`looks_like_path` / `is_command_path`) with role-based per-unit path operand extraction (`unit_path_words` + `looks_like_path_word`) in `all_paths` that retains bare filenames (`a.txt`, `b.txt`), quoted whitespace-containing paths, and `SessionGate` resolved symlink targets while skipping subcommands (`cargo test`, `git push`), short-flag cluster letters (`-rf`), and non-path flag values (`head -n 5`). Pinned by `round3_finding1_path_scope_retains_bare_and_whitespace_operands` in `crates/atrium-core/tests/M1-29-cr17-stores.rs`.
- **Finding 2 (`One command's derived unit satisfies another find command's guard`) — fixed:** Replaced the call-wide `has_derived` boolean in `crates/atrium-core/src/policy/engine.rs` with per-unit `find_exec_extracted_for_unit(a, unit_idx, argv)` in `rule_covers_argv`, requiring exact lowercase `find` and exact lowercase `-exec`/`-execdir`/`-ok`/`-okdir` clauses with matching directly owned derived `exec_roots.is_some()` units on that owning command (so `/usr/bin/FIND` and `-EXEC` fail closed). Pinned by `round3_finding2_per_command_find_derived_guard_and_folded_spelling` in `crates/atrium-core/tests/M1-29-cr17-stores.rs`.
- **Finding 3 (`A git prefix still covers command-line shell aliases`) — fixed:** Added `git_cli_exec_config` / `git_config_is_exec` in `crates/atrium-core/src/policy/rule_check.rs` so `command_prefix_blocked` and `command_allow_risk` refuse `git` allow rules / invocations with global `-c`, `--config-env`, `--exec-path`, `--ext-diff`, `--upload-pack`, `--receive-pack`, or `--exec=` (while preserving `git commit -c HEAD` and ordinary `git` prefix rules). Pinned by `round3_finding3_git_prefix_does_not_cover_cli_shell_aliases` in `crates/atrium-core/tests/M1-29-cr17-stores.rs` and `create_rule.rs`.
- **Finding 4 (`Interpreter option values are mistaken for script names`) — fixed in Rust (`rule_check.rs`); UI duplication escalated:** Updated `check_interpreter_risk` and `check_package_runner_risk` in `crates/atrium-core/src/policy/rule_check.rs` to consume known option values (`python3 -W ignore`, etc.), handle `-m <mod>`, and fail closed on unknown options before the script/package. Pinned by `round3_finding4_interpreter_option_values_not_mistaken_for_scripts` in `crates/atrium-core/tests/M1-29-cr17-stores.rs` and `create_rule.rs`. Note: `app/src/chat/rule-form.ts` is outside M1-29's owned paths, so the matching update to the editor's client-side `commandRisk` is escalated rather than edited here.
- **Finding 5 (`The daemon accepts Execute path rules that bypass command checks`) — fixed in Rust (`rule_check.rs`, `engine.rs`, `docs/contracts/m1.md`); editor wiring escalated:** Updated `validate_rule_draft` in `crates/atrium-core/src/policy/rule_check.rs` to require `ToolKind::Execute` for command matchers and `is_path_kind` (`Read`/`Edit`/`Delete`/`Move`/`Search`) for `PathGlob` and `path_scope`, updated `allow_covers` in `crates/atrium-core/src/policy/engine.rs` so `PathGlob` never covers `ToolSubject::Command` or non-file kinds, and updated `docs/contracts/m1.md` §9 wording to reflect that `CreateRule` is backed by the shared validator while the editor runs its own check (`app/src/chat/rule-form.ts` is outside owned paths). Pinned by `round3_finding5_execute_path_glob_cannot_allow_commands` in `crates/atrium-core/tests/M1-29-cr17-stores.rs` and `create_rule.rs`.
- **Finding 6 (`Malformed rules are stored instead of returning an error`) — fixed:** Updated `validate_rule_draft` in `crates/atrium-core/src/policy/rule_check.rs` with the editor's structural constraints (`validate_abs_glob` requiring leading `/`, length <= 1000, no `.`/`..` segments, no `\n`/`\r`/`\0`, valid `globset`; command length <= 4000, no control chars or `\n`/`\r`/`\u{2028}`/`\u{2029}`; `tool_name` length <= 100 with no whitespace/control chars; `Any` excluding `ToolKind::Think`). Pinned by `create_rule.rs` (including `ListRules` non-persistence checks).
- **Finding 7 (`Remembered rules cannot allow hard-linked CR-17 stores`) — blocked on out-of-scope file (`gate.rs`), escalated:** `crates/atrium-core/src/policy/gate.rs` (lines 362–366, `reaches_multiply_linked`) is outside M1-29's owned paths, and per the brief ("Stay inside your owned paths … If the fix needs a change elsewhere, stop that part and say so in the handoff: don't make it") `gate.rs` was not edited. Added a `DefaultPolicy::evaluate` test in `round3_finding8_and_cr17_test_gaps` (`crates/atrium-core/tests/M1-29-cr17-stores.rs`) proving the engine returns `Allow` once the hard-linked CR-17 alias is mapped onto `~/.terraform.d/credentials.tfrc.json`, and escalated the `gate.rs:362–366` change (allowing `Preset::Careful` rule-authorized reads through `reaches_multiply_linked` for positively identified ask-store aliases, matching lines 348–358).
- **Finding 8 (`The credential-store wording incorrectly promises approval for .git-credentials`) — fixed:** Removed `~/.git-credentials` from the CR-12/CR-17 ask-store enumeration in `docs/contracts/m1.md` §9 (line 153) while preserving its `HardDeny { id: "credential-read" }` verdict in `deny.rs`. Pinned by `round3_finding8_and_cr17_test_gaps` in `crates/atrium-core/tests/M1-29-cr17-stores.rs` (along with CR-17 `..`, glob, linked `Search`, and symlink remembered-rule coverage).
- **Checks run after rework round 3 (D-123):**
  - `cargo fmt --all` — passed (exit 0)
  - `cargo check -p atrium-core --tests` — passed (exit 0, 0 warnings)
  - `cargo check -p atrium-daemon --tests` — passed (exit 0, 0 warnings)

## Rework round 4

- **Clippy (`crates/atrium-core/tests/M1-29-cr17-stores.rs:1070`):** Replaced `let any_exec = vec![rule(...)];` with `let any_exec = [rule(...)];` in `round3_finding8_and_cr17_test_gaps` to eliminate `clippy::useless_vec`.
- **`Search ~ *.yml` (`crates/atrium-core/tests/M1-29-cr17-stores.rs:1116–1128`):**
  - **Hard-denied store matched:** **`~/.config/gh/hosts.yml`** (`CRED_FILES` at `crates/atrium-core/src/policy/deny.rs:405`).
  - **Depth-1 check:** No listed hard-denied store (`CRED_FILES`, `CRED_TREES`, `BROWSER_FILES`, `SSH_PROBES`) is a `*.yml` file directly in `~` itself; depth-1 entries in `CRED_FILES` are `.netrc`, `.git-credentials`, `.npmrc`, and `.claude.json`, while `.config/gh/hosts.yml` sits at depth 3.
  - **Why the test hit `HardDeny { id: "credential-read" }`:** `round3_finding8_and_cr17_test_gaps` passed `recursive: true` (`paths_check(ToolKind::Search, "Glob", &["~"], true, Some("*.yml"))`), which represents a recursive search under `~` matching `*.yml` filenames (shared with recursive `Grep` `--glob '*.yml'`). Under `recursive: true` (`deny.rs:1488`, `name_allowed = recursive || !rel.contains('/')`), `search_glob_hit` matches `hosts.yml` in `~/.config/gh/hosts.yml` and rightly hard-denies. (Flagged for the owner of `deny.rs`: even when `recursive: false`, `search_glob_hit` at `deny.rs:1461` compiles globs with `.literal_separator(false)` and evaluates `g.is_match(&probe) || (!rel.is_empty() && g.is_match(rel))`, so `*` crosses `/` and still matches `.config/gh/hosts.yml`; `deny.rs` below `ASK_TREES` is outside M1-29's owned paths and was left untouched.)
  - **Test fix:** Changed the CR-17 search case for `~/.yarnrc.yml` from `("~", "*.yml")` to `("~", ".yarnrc*")` (`Ask`), and added an explicit assertion pinning recursive `Search ~ *.yml` (`recursive: true`) -> `HardDeny { id: "credential-read" }`.
- **Checks run after rework round 4 (D-123):**
  - `cargo fmt --all` — passed (exit 0)
  - `cargo check -p atrium-core --tests` — passed (exit 0, 0 warnings)
  - `cargo check -p atrium-daemon --tests` — passed (exit 0, 0 warnings)

## Rework round 5

*(Per D-120 and `review-round2.md` findings 2, 4, 5, and 6—where round 2 rejected escalating `gate.rs` and `app/src/chat/rule-form.ts`—this round widened into `crates/atrium-core/src/policy/gate.rs` lines 272–390 and 1488–1575, non-overlapping with M1-32's 320–350, and `app/src/chat/rule-form.ts` / `rule-form.test.ts`.)*

- **Finding 1 (`Path filtering still drops paths and fails open`) — fixed:** Added `push_script_path_tokens` (extracting script path tokens from `sed`/`awk` positional scripts and `-e`/`--expression` values), fixed `diff -u` / `ls --color` / `wget -d`/`-m` / `git -f`/`-t` flag-arity bugs, and updated `unit_path_words` in `crates/atrium-core/src/policy/engine.rs` to treat `git` pathspecs and unknown `git` subcommand operands as paths (excluding only known non-path ref/remote subcommands like `push`/`fetch`/`branch`). Pinned by `round5_finding1_path_filtering_retains_script_and_git_pathspec_operands` in `crates/atrium-core/tests/M1-29-cr17-stores.rs`.
- **Finding 2 (`A PowerShell option value can masquerade as a pinned script`) — fixed:** Replaced the naive `.ps1` scan in `check_pwsh_risk` (`crates/atrium-core/src/policy/rule_check.rs`) and `checkPwshRisk` (`app/src/chat/rule-form.ts`) with full PowerShell option parsing (consuming `-WorkingDirectory`/`-wd`, `-ExecutionPolicy`/`-ep`, etc., handling `-File`, refusing missing/non-`.ps1`/stdin targets and unknown options, while keeping `/path/to/script.ps1` valid), so `["pwsh", "-WorkingDirectory", "work.ps1"]` is refused on creation and `command_prefix_blocked` falls through to block `-Command`. Pinned by `round5_finding2_pwsh_option_values_and_inline_code` in `M1-29-cr17-stores.rs`, `create_rule.rs`, and `rule-form.test.ts`.
- **Finding 3 (`Equivalent stdin paths bypass the interpreter guard`) — fixed:** Added lexical POSIX path normalization (`normalize_posix_path` / `is_stdin_script_path` in `crates/atrium-core/src/policy/rule_check.rs`, mirrored as `normalizePosixPath` / `isStdinPath` in `app/src/chat/rule-form.ts`) collapsing `.`, `..`, and redundant `/` before matching `-`, `/dev/stdin`, `/dev/fd/0`, `/proc/self/fd/0`, and `/proc/thread-self/fd/0` during both creation and evaluation. Pinned by `round5_finding3_stdin_alias_normalization` in `M1-29-cr17-stores.rs`, `create_rule.rs`, and `rule-form.test.ts`.
- **Finding 4 (`Remembered rules still cannot allow hard-linked CR-17 stores`) — fixed:** Updated `crates/atrium-core/src/policy/gate.rs` (`evaluate_sync`, `reaches_multiply_linked`, `tree_has_multiply_linked`) so when `Preset::Careful` confirms explicit rule authorization, `reaches_multiply_linked` exempts files whose `(dev, ino)` matches `deny::ask_roots(&c.home)` while retaining conservative `Ask` for all unknown multiply-linked files and unruled presets. Pinned by `round5_finding4_gate_hardlink_remembered_rule` (actual `SessionGate::check` test) in `crates/atrium-core/tests/M1-29-cr17-stores.rs`.
- **Finding 5 (`The editor still validates against a stale duplicate`) — fixed:** Synchronized `app/src/chat/rule-form.ts` `commandRisk` with `crates/atrium-core/src/policy/rule_check.rs` (`interpreterValueFlag`, `interpreterBoolOrAttached`, Python `-W`/`-X`/`-Q`/`-m`/bool flags, `check_package_runner_risk`, `check_pwsh_risk`, `isStdinPath`, and unknown-option fail-closed checks). Pinned in `app/src/chat/rule-form.test.ts` (`python3 -W ignore` exact/prefix, `node -r preload`, `npx -p prettier`).
- **Finding 6 (`§9 overstates refusal of exact Git configuration rules`) — fixed:** Updated `docs/contracts/m1.md` §9 (line 148) to state accurately that prefix or exact allow rules are refused for execution-bearing `git` CLI options while exact allow rules with safe `-c` options (e.g., `git -c color.ui=false status`) remain permitted and prefix `git -c` is refused; ported `gitConfigIsExec` / `gitCliExecConfig` into `app/src/chat/rule-form.ts`. Pinned by `round5_finding6_exact_git_safe_config_allow` in `M1-29-cr17-stores.rs`, `create_rule.rs`, and `rule-form.test.ts`. Note on updated test expectation (`app/src/chat/rule-form.test.ts:127`): changed `commandRisk(["git", "-c", "x=y", "status"])` (default `command_prefix`) from `.toBeNull()` to `{ refuse: ... }` (and added `.toBeNull()` for `command_exact`) per D-121 / contract §9 (`rev 3.2, D-121`) / `review-round2.md` finding 6.
- **Finding 7 (`Malformed thread scopes are accepted and stored`) — fixed:** Validated non-empty/non-control `RuleScope::Thread { thread_id }` in `validate_rule_draft` (`crates/atrium-core/src/policy/rule_check.rs`) and checked `inner.store.thread(thread_id)` in `create_rule` (`crates/atrium-daemon/src/commands.rs`), returning `ErrorCode::Invalid` without persisting when the thread is missing. Pinned in `crates/atrium-daemon/tests/create_rule.rs` (including `ListRules` non-persistence check and acceptance for an existing thread from `StartThread`).
- **Test gaps addressed:** Added `\;` spellings for `find -ok`/`-okdir`/`-exec` in `crates/atrium-core/tests/M1-29-cr17-stores.rs`, added `ListRules` non-persistence assertions in `crates/atrium-daemon/tests/create_rule.rs`, added `SessionGate::check` hard-link remembered-rule test in `M1-29-cr17-stores.rs`, and updated `app/src/ui/security.test.ts` to parse `ASK_FILES`, `ASK_TREES`, and `ASK_NAME_PREFIXES` directly from `crates/atrium-core/src/policy/deny.rs` to detect any core/UI drift.
- **Checks run after rework round 5 (D-123):**
  - `cargo fmt --all` — passed (exit 0)
  - `cargo check -p atrium-core --tests` — passed (exit 0, 0 warnings)
  - `cargo check -p atrium-daemon --tests` — passed (exit 0, 0 warnings)
  - Under D-123, `cargo test`, `cargo clippy`, and `pnpm` were not run here; the verification task should run the DoD commands listed above (including `pnpm --dir app typecheck` and `pnpm --dir app test`).

## Escalation (Opus, D-129)

Round-2 findings and the tests that pin them (reviewer's exact cases):
- F1 [major] path filtering (sed script output, bare git pathspec): `M1-29-cr17-stores.rs::round5_finding1_path_filtering_retains_script_and_git_pathspec_operands`.
- F2 [major] pwsh option value as a fake script: `round5_finding2_pwsh_option_values_and_inline_code`; daemon refusal in `crates/atrium-daemon/tests/create_rule.rs` (`pwsh -WorkingDirectory work.ps1`); editor parity in `rule-form.test.ts` "rework round 5 parity".
- F3 [major] `/dev/./stdin` and other stdin spellings: `round5_finding3_stdin_alias_normalization`; editor parity in `rule-form.test.ts`.
- F4 [minor] Read Allow rule on a hard-linked CR-17 store: `round5_finding4_gate_hardlink_remembered_rule` (SessionGate, `Read ["proj/tf_hard"]` -> Allow; unknown hard link -> Ask).
- F5 [minor] editor `python3 -W ignore`: `rule-form.test.ts` "rework round 5 parity" (refused, matching `create_rule.rs`).
- F6 [minor] §9 exact git `-c` wording: contract §9 now states the safe-`-c` exact exception; `round5_finding6_exact_git_safe_config_allow`, `create_rule.rs`, `rule-form.test.ts` finding 6.
- F7 [minor] malformed/unknown thread scope: `create_rule.rs` (empty, blank and unknown thread ids -> Invalid, rule count unchanged; an existing thread succeeds).

The two regressions (`adversarial_m1_18::cr_12_through_links_asks_in_home`, `k2_links_at_any_hop_unexpanded_globs_and_following_walkers`): rework 5's F4 fix in `gate.rs` exempted known ask-store hard links whenever a rule allowed the call, for commands and inside glob/tree walks too, so `cat asks2/hl` and `find -L clean -name '*.rs'` (glob over Home, which holds the hard-linked `.pgpass`) went to Allow under an Execute Any rule. Fixed (`74f3e94`): the exemption applies only to a file tool (`ToolSubject::Paths`) naming the alias literally (not a glob); shell commands and the glob/recursive walks keep `dev`'s ask. `round5_finding4` was adjusted accordingly: `cat proj/tf_hard` under an Execute Any rule now asserts Ask (it asserted Allow, contradicting dev's test). Note: `gate.rs` is outside the brief's owned paths (rework 5 touched it to close F4); the driver should record this.

Known failures not ours: `fix4_finding3_glob_escape_total_variant_cap_performance` fails alone here also with `dev`'s policy sources (155-326 ms vs 50 ms, debug build); `fix2` fails only under load.

Also fixed in the escalation: rework 5's editor parity change dropped dev's refusal of a stdin name after the script (`rule-form.test.ts` "fails closed (round 3, blocker 1)", `python3 x.py -`). The editor and `rule_check.rs` now both refuse a stdin name anywhere in an interpreter's arguments; pinned by `escalation_stdin_name_after_script_is_refused` and the existing editor test.

Other failures seen here and checked: `socket::replay_then_live_delivers_every_event_once_and_in_order` and `a_reader_that_stops_lags_alone` fail alone with `dev`'s policy and `commands.rs` too (not ours). `requests_notifications` failed only under the full parallel load and passes alone. `markdown.test.ts` "pathological emphasis and brackets parse quickly" (a timing test, ~6 s) fails alone too; this branch does not touch `markdown.ts` or its test.

## Escalation round 2 (review 3)

Each finding was written as a test first (`2f3bde6`, all four failing), then closed with a general rule rather than the reviewer's spelling:

- **F1 [major] quoted Git pathspec globs** (`engine.rs` `git_path_is_credential`, new `git_glob_reaches_credential`): a pathspec with `*`, `?` or `[` asks when the glob *can* match a credential name, suffix, key name or a file below a credential folder at any depth. It's a glob-against-glob intersection where `*` crosses `/` (Git's default) and classes match any character (fails closed). A pattern ending in a literal source-code extension (`*.rs`) skips key names and credential folders it doesn't name, matching `is_git_key_name`'s own exception. Pinned by `review3_finding1_git_pathspec_globs_reach_credentials` (the four CR-17 stores, `:(glob)`, `?`, `[...]`, `*`, `sub/*`; `'*.rs'`, `'src/*.rs'`, `'docs/*.md'` stay Allow). Scope note: `git_path_is_credential` sits just below the CR-12/Git credential lists the brief owns in `engine.rs`. It was already on `dev`, and the CR-17 `git show` checks go through it.
- **F2 [major] sed delimiters** (`engine.rs` `sed_script_files`, `all_paths`, `lexical_paths`, `path_scope_ok`): sed scripts are now parsed, not token-matched. `r`/`R`/`w`/`W` and the `s` command's `w` flag are recognised for any delimiter, after any address (`1`, `$`, `/re/`, `\%re%`), with or without a space before the file name. A script that can't be parsed yields an unresolved marker. `all_paths` now also resolves lexically every word the command reads as a path that the walk didn't resolve, using `~`, `$HOME`, `$PWD` and the cwd candidates. A word it can't resolve (a variable, a glob, an unparsed script) makes an allow scope not cover the line and a deny scope match it. That's the brief's every-path/any-path rule failing closed. Pinned by `review3_finding2_sed_delimiters_and_path_scopes` (both reviewer cases plus `|`, `#`, `,`, attached `w/…`, `1w`, `/x/w`, `\%x%w`, `$r`; deny cases Deny; in-scope lines stay Allow).
- **F3 [major] Git execution payloads** (`rule_check.rs` `git_subcommand_runs_code`, `git_config_writes_exec`, `git_long_abbrev`, `git_unpinned_external`, plus keys added to `git_config_is_exec`): these are refused for prefix and exact drafts and never covered at evaluation. They cover:
  - `rebase --exec`/`-x` (also clustered, attached or abbreviated);
  - `submodule foreach`, `bisect run`, `grep -O`/`--open-files-in-pager`, `clone`/`ls-remote -u`;
  - `--upload-pack`/`--receive-pack`/`--exec`/`--ext-diff`, abbreviations included;
  - `config` writes of a code-running key, or `config --edit`;
  - the subcommands that run tools, helpers or a shell (`filter-branch`, `difftool`, `mergetool`, `send-email`, `credential`, `hook`, `shell`, …);
  - the config keys `core.hooksPath`, `include.*`, `includeIf.*` and `hook.*`.

  A prefix rule on `git` alone also no longer covers a subcommand that isn't Git's own (an alias, which may be `!cmd`, or a `git-NAME` program). A rule that names it (`git lfs`) still does. Pinned by `review3_finding3_git_execution_payloads` (24 lines Ask, and `command_allow_risk` refuses each as prefix and exact; ordinary lines like `git cherry-pick -x`, `git commit -c HEAD`, `git fetch -u` and `git config --get core.editor` stay Allow), `review3_finding3_git_aliases_and_external_subcommands`, and `create_rule.rs` section 9 (Invalid for each, rule count unchanged, ordinary drafts accepted).
- **F4 [major] relative stdin paths** (`rule_check.rs` `is_stdin_script_path`, `is_proc_pid_fd0`): a draft has no cwd, so a relative script operand is refused when its folded path (`a/..` folded, leading `..` dropped) is a tail of `/dev/stdin`, `/dev/fd/0`, `/proc/self/fd/0`, `/proc/thread-self/fd/0` or `/proc/<pid>/fd/0`. Examples: `./stdin`, `stdin`, `fd/0`, `0`, `self/fd/0`, `../dev/stdin`. Absolute `/proc/<pid>/fd/0` is refused too. The engine uses the same check for prefix coverage, so `echo … | python3 ./stdin` from `/dev` asks. Relative spellings count only as the script itself: `python3 x.py 0` stays fine. The "anywhere in the arguments" refusal from the first escalation is narrowed to `-` and absolute paths. Pinned by `review3_finding4_relative_stdin_paths` and `create_rule.rs` section 9.
- **F5 [minor] editor ownership:** the editor changes are in two files. Brief item 5 makes the Rust check the source of truth for the lists the app's rule editor uses, and says the daemon must refuse "with the same folding and the same lists as the app's rule editor (`app/src/chat/rule-form.ts` `commandRisk`)". So keeping the editor in step is part of that item, but the owned-path list names only `Security.tsx` and its test, so **the driver needs to record the ownership** in `decisions.md`:
  - `app/src/chat/rule-form.ts`: `commandRisk`'s interpreter, PowerShell, stdin and Git checks (`checkPwshRisk`, `isStdinPath`/`isStdinName`, `gitCliExecConfig`, `gitSubcommandRunsCode`, `gitConfigWritesExec`, `gitLongAbbrev`, `GIT_BUILTINS`/`GIT_RUNS_CODE`), each mirroring the `rule_check.rs` function it names.
  - `app/src/chat/rule-form.test.ts`: the parity blocks "rework round 5 parity with rule_check.rs" and "review 3 parity with rule_check.rs (D-129)".

  The same applies to `gate.rs` (first escalation).
