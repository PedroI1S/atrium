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
