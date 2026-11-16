# Review: M1-18-policy-followup (commits 814b3c3, a20ac4c)

Reviewer: Gemini 4 Hydrogen `high`, read-only (D-099). Author: Claude Opus 5.5. Saved by the orchestrator from the run's final message.

## Verdict: `rework`

The branch closes the round-8 `Allow` gaps from **K1–K4** (`adversarial_m1_18` 13/13 and `adversarial_regression` 57/57 pass), closes the round-8 **Major 5** `rg -e -L` false hard-deny regression (`adversarial_ordinary` 16/16 passes), implements **CR-12** across command lines, file tools, symlinks, and `git show`/`log`/`diff`, and implements **P1** for `ToolSubject::Paths { glob }`. There are **no `Allow` regressions vs `dev`** and **no new false hard denies on ordinary lines vs `dev`** in what was inspected.

However, the branch cannot be approved as-is because:
1. **`cargo test -p atrium-core --no-fail-fast` fails** on `adversarial_fuzz::random_hostile_lines_never_panic_and_nothing_sensitive_is_allowed` (`193.49s` vs the `< 60s` bound; `58.04s` standalone vs `15.61s` on round 8), and `policy::engine_never_panics_on_garbage` slowed from `< 37s` (for the whole `policy` binary on round 8) to `129.47s` standalone (`291.77s` for `policy` in the suite run) due to a recursive `lex_depth` blowup in [shell.rs](crates/atrium-core/src/policy/shell.rs).
2. Four `Allow` edge-case gaps remain in **P1**, **K1**, and **CR-12** (all **`same as dev`**).

---

## Findings

> Sensitive-path / credential-store `Allow` findings are listed first, marked `same as dev` or `regression vs dev`.

### Blockers

#### 1. [blocker] P1: `**` followed by `..` in `glob_escape` is popped by `normalize`, allowing escaping globs (`**/../.ssh/id_rsa`) — **`same as dev`**
- **Locations:** [deny.rs:1324-1342](crates/atrium-core/src/policy/deny.rs#L1324-L1342) (`glob_escape`), [words.rs:296-313](crates/atrium-core/src/policy/words.rs#L296-L313) (`normalize`)
- **Mechanism:** `glob_escape` computes `normalize(&r.join(&e))` for relative globs, and `normalize` pops the preceding component whenever it sees `..`. When `e` has `..` after `**` (e.g., `ToolSubject::Paths { paths: ["~/proj"], glob: Some("**/../.ssh/id_rsa"), recursive: true }` or `glob: Some("src/**/../../.ssh/id_rsa")`), `normalize` pops `**` for `..`, yielding `/h/u/proj/.ssh/id_rsa`. Because `start` is `/h/u/proj/.ssh/id_rsa`, `roots.iter().any(|r| start.starts_with(r))` returns `true` (`leaves() == false`), even though `**` matches 0 directories (`proj/../.ssh/id_rsa` = `/h/u/.ssh/id_rsa`). `search_glob_hit` likewise misses it because the pattern contains `**/..`. Note that [claude/tools.rs:403-406](crates/atrium-agents/src/claude/tools.rs#L403-L406) already turns `..` after a wildcard into `Unknown`, so this affects raw `ToolSubject::Paths` or other adapters that rely on the engine per P1.
- **Comparison with `dev`:** `dev` had no `glob_escape` check, so these inputs also reached `Allow` on `dev` across Home, `Any`, and `FullAccess` (**`same as dev`**).

#### 2. [blocker] K1: `env` with unknown options misses `-C` payloads after an unknown long option (`env --zzz X -C ..`) and skips 2-word branching on unknown short flags (`env -Z X`) — **`same as dev`**
- **Locations:** [shell.rs:3075-3142](crates/atrium-core/src/policy/shell.rs#L3075-L3142) (`env_payloads`), [shell.rs:2235-2328](crates/atrium-core/src/policy/shell.rs#L2235-L2328) (`unwrap_reading_steps` `"env"` arm)
- **Mechanism:**
  1. `unwrap_reading_steps` branches 1 or 2 words for unknown `--` long flags on `env` (so `cwd=proj|env --zzz X -C .. cat .ssh/id_rsa` unwraps to `["cat", ".ssh/id_rsa"]` in mask 1), and `unwrap_steps` records the argv at the start of the `"env"` iteration (`["env", "--zzz", "X", "-C", "..", ...]`) rather than after skipping `--zzz X`. `env_payloads` scans `argv` in one pass and breaks at `"X"` (`line 3132`), so `-C ..` is never extracted into `cd '..' && ...` and `cat .ssh/id_rsa` resolves against `proj` (`Allow` under `Any` and `FullAccess`).
  2. Unlike wrappers handled by `table()` (`command`, `timeout`, `nice`, etc.), `env`'s custom arm only branches 2 words on `--` long flags (`lines 2288-2315`). An unknown short option (`is_option(w)` at `lines 2316-2320`) always advances `j += 1`. Thus `env -Z X sudo id`, `env -Z X eval 'cd ~/.ssh && true' && cat id_rsa`, `env -Z X bash <(echo 'rm -rf ~/')`, and `cwd=proj|env -Z X -C .. cat .ssh/id_rsa` leave `eff = ["X", ...]` in every reading and reach `Allow` under `Any` and `FullAccess`.
- **Comparison with `dev`:** `dev` (`git show dev:crates/atrium-core/src/policy/shell.rs:2124-2167`) has the same single-word short-option handling in `env` and has no `env_payloads`, so all of these reach `Allow` under `Any` and `FullAccess` on `dev` (**`same as dev`**).

#### 3. [blocker] CR-12: Multi-component relative globs with `recursive: false` in `ask_search_glob_hit` and git pathspec magic in `git_names_credential` reach `Allow` in Home — **`same as dev`**
- **Locations:** [deny.rs:509-522](crates/atrium-core/src/policy/deny.rs#L509-L522) (`ask_search_glob_hit`), [engine.rs:557-610](crates/atrium-core/src/policy/engine.rs#L557-L610) (`git_names_credential`)
- **Mechanism:**
  1. `ask_search_glob_hit` matches the glob only against `name` (the basename) or `probe` (the full `/h/u/...` path), never against `probe` relative to a root in `roots`. For `ToolSubject::Paths { paths: ["~"], glob: Some(".cargo/credentials.toml"), recursive: false }` (or `glob: Some(".aws/sso/cache/*")`, `glob: Some(".local/share/fish/fish_history")`, `glob: Some("~/.cargo/credentials.toml")`), `glob_escape` sees the path stays under `~`, `ask_search_glob_hit` returns `false`, and `recursive: false` skips `root_ok("~")` at [engine.rs:732](crates/atrium-core/src/policy/engine.rs#L732), returning `Allow` in Home. (Claude's `Glob`/`Grep` tools set `recursive: true`, which hits `root_ok("~") == false`, so this affects `recursive: false` search subjects.)
  2. `git_names_credential` splits at the first `:` (`split_once(':')` at `line 561`), turning git pathspec magic such as `:(top).npmrc`, `:(icase).env`, `:(literal).pypirc`, or `:(top).aws/credentials` into `"(top).npmrc"` / `"(icase).env"`, which misses `GIT_CRED_NAMES`, `GIT_CRED_SUFFIXES`, and `GIT_CRED_DIRS`, so `git log -p -- ':(top).npmrc'` and `git diff HEAD~1 -- ':(icase).env'` return `Allow` in Home.
- **Comparison with `dev`:** Neither check existed on `dev`, which returned `Allow` in Home for all of these (**`same as dev`**).

#### 4. [blocker] Performance / DoD failure: `lex_depth` multiplies recursive `$(...)` lexes (`5^MAX_DEPTH`), failing `adversarial_fuzz` (`193.49s` > `60s`) and slowing `policy::engine_never_panics_on_garbage` to `129.47s` — **`regression vs dev` (latency / test failure)**
- **Locations:** [shell.rs:1084-1126](crates/atrium-core/src/policy/shell.rs#L1084-L1126) (`lex_depth`), [shell.rs:1810-1818](crates/atrium-core/src/policy/shell.rs#L1810-L1818) (`runs_here`), [shell.rs:1869-1925](crates/atrium-core/src/policy/shell.rs#L1869-L1925) (`code_texts`), [shell.rs:2072-2079](crates/atrium-core/src/policy/shell.rs#L2072-L2079) (`unwrap_steps`), [adversarial_fuzz.rs:237-241](crates/atrium-core/tests/adversarial_fuzz.rs#L237-L241)
- **Mechanism:** On each `$(...)` command word, `lex_depth` now triggers up to 5 recursive `lex_depth(..., depth + 1)` walks up to `MAX_DEPTH = 6`:
  1. `substituted_program_at(f, depth)` (`line 1086`) -> `output_texts` -> `lex_depth`;
  2. `for body in &subs` (`line 1102`) -> `lex_depth`;
  3. `code_texts` -> unconditional `unwrap_steps(argv, depth)` (`line 1882`) -> `substituted_program_at` -> `lex_depth` (even when `argv` contains no `env` or wrappers);
  4. `code_texts` -> `for s in &code_subs` -> `output_texts(s, depth)` (`line 1922`) -> `lex_depth`;
  5. `runs_here(e, depth)` (`line 1816`) -> `unwrap_readings(eff, depth)` -> `substituted_program_at` -> `lex_depth` (plus `pipe_code_eff` at `line 1856` calling `unwrap_readings(e, depth)` unconditionally on every piped command reading).
  On `engine_never_panics_on_garbage`'s `nested = "$(".repeat(5_000) + &")".repeat(5_000)` and `adversarial_fuzz`'s random lines with `$(` and wrappers, this creates an order-of-magnitude slowdown:
- **Evidence:**
  - `cargo test -p atrium-core --no-fail-fast`: `adversarial_fuzz::random_hostile_lines_never_panic_and_nothing_sensitive_is_allowed` **FAILED** at [adversarial_fuzz.rs:237:5](crates/atrium-core/tests/adversarial_fuzz.rs#L237) (`too slow: 193.489416416s`, limit `< 60s`), and `tests/policy.rs` took `291.77s`.
  - Standalone `cargo test -p atrium-core --test adversarial_fuzz`: `58.04s` (vs `15.61s` on round 8 and `12.76s` on round 7).
  - Standalone `cargo test -p atrium-core --test policy engine_never_panics_on_garbage`: `129.47s` (vs `36.89s` for all 44 `policy` tests on round 8).

---

### Minor / Nit

#### 5. [minor] `follows_links` `short('L')` / `short('R')` does not pair flag values (`rg -e -L`, `grep -r -e -R` with no root in a folder holding a symlink into `.ssh`) — **`same as dev`**
- **Location:** [gate.rs:1042-1058](crates/atrium-core/src/policy/gate.rs#L1042-L1058) (`follows_links`)
- **Detail:** `searches_cwd_with` (`lines 699-723`) fixes round 8's Major 5 (`rg -e -L safe_sub` has `searches_cwd_with == Some(false)` and does not walk `cwd`). However, when **no** search root is given (`rg -e -L` searching literal `-L` in `cwd`, while `cwd` contains a symlink into `~/.ssh`), `follows_links` inspects `args` without pairing `-e`/`-g`/`-f` values, so `-L` is treated as `--follow` and triggers `HardDeny("credential-read")`. `dev`'s `follows_links` (`git show dev:crates/atrium-core/src/policy/gate.rs:940-960`) used the exact same `short('L')` / `short('R')` check (**`same as dev`** when `~/.ssh` has a linked folder).

#### 6. [minor] `links_below` `BUDGET = 4096` without skipping `.git` / `node_modules` and `expand_glob == None` with `lister == true` cause needless `Ask` under `Any` / `FullAccess` on large ordinary repos — **`stricter than dev` (Ask, not HardDeny)**
- **Locations:** [gate.rs:1199-1216](crates/atrium-core/src/policy/gate.rs#L1199-L1216) (`links_below`), [gate.rs:1291-1308](crates/atrium-core/src/policy/gate.rs#L1291-L1308) (`reaches_tree_through_links`)
- **Detail:** Unlike `reaches_multiply_linked`, `links_below` does not skip `.git` or `node_modules`. Any ordinary repository with >4096 entries causes `cp -r proj /tmp/x`, `zip -r out.zip proj`, `rg -L pat proj`, and `tar -chzf out.tgz proj` to hit `BUDGET = 4096` and return `Reach::Maybe` (`Ask` even under `Any` and `FullAccess`, though never a false hard deny). Similarly, at `line 1303-1308`, `expand_glob(p) => None` sets `maybe = true` even when `lister && !(recursive && follow != Follow::No)` (so `ls big_dir/*` asks under `Any`/`FullAccess`).

#### 7. [nit] Minor `git_names_credential` needless asks and `ASK_TREES` / `GIT_CRED_DIRS` asymmetry — **`stricter than dev` (Ask, not HardDeny)**
- **Location:** [engine.rs:545-610](crates/atrium-core/src/policy/engine.rs#L545-L610)
- **Detail:** `git_names_credential` checks `arg.starts_with('-') && !arg.contains(':')`, so flags containing `:` whose suffix happens to match (e.g., `git log --grep=foo:.env` or `git show HEAD:config/id_rsa_helper.rs`) ask in Home (`Ask`, never hard-denied). Conversely, `.zsh_sessions` and `.bash_sessions` are in `ASK_TREES` but not in `GIT_CRED_DIRS`.

---

## Check-by-Check Assessment

1. **CR-12:**
   - All stores listed in [docs/contracts/m1.md](docs/contracts/m1.md#L150) §9 (`~/.cargo/credentials.toml`, `~/.cargo/credentials`, `~/.pypirc`, `~/.pgpass`, `~/.vault-token`, `~/.azure/accessTokens.json`, `~/.azure/msal_token_cache.*`, `~/.aws/sso/cache/`, `~/.config/git/credentials`, `~/.git-credentials` [stays hard-denied as on `dev`], shell history files, `~/.env`) are covered in `ASK_FILES`, `ASK_TREES`, `ASK_NAME_PREFIXES`, `ask_roots`, `is_or_contains`, `names_ask_store`, and `gate.rs:342-359`.
   - `adversarial_m1_18::cr_12_*` and `adversarial_policy::finding_6_other_credential_stores_ask_in_home` verify Home asks, one-time approvals, remembered rules (`CommandExact` and `PathGlob`), `Any`, `FullAccess`, symlinks, hard links (which ask across all presets via existing `reaches_multiply_linked`), `rg -L`, and `git show`/`git cat-file`/`git log`/`git diff` (wrappers `env`, `sh -c`, `eval`, `xargs`, `find -exec` are not on `HOME_READONLY_COMMANDS`, so they also ask in Home with no rules and are allowed by rules/`Any`/`FullAccess`). Remaining gaps: Finding 3 (`recursive: false` relative multi-component search globs and git `:(top)`/` :(icase)` pathspec magic).

2. **P1 (and the `FullAccess` / `Any` question):**
   - `glob_escape` catches absolute globs, `~`, `~user`, `..`, brace expansions (`brace_expand`, and `None` -> `unknown`), character classes (`may_be_dot_component`), and backslashes/NUL, hard-denying only when an expanded path hits `Sensitive::hit` / `glob_hit` / `is_ssh_dir`. Remaining gap: Finding 1 (`**` before `..`).
   - **Choice to ask under `FullAccess` and `Any` rules:** Matches the brief ("The engine must refuse or ask for a `ToolSubject::Paths { paths, glob }` whose `glob` can name files outside every root", with no preset carve-out) and matches contract §9's rule for `ToolSubject::Unknown` ("it can only be hard-denied (on the raw input) or asked", [engine.rs:794-809](crates/atrium-core/src/policy/engine.rs#L794-L809)), as well as [claude/tools.rs:193-195](crates/atrium-agents/src/claude/tools.rs#L193-L195) returning `Unknown` for unparseable globs. Note as a trade-off that a `PathGlob` rule or `FullAccess` cannot override an escaping glob unless the adapter puts its targets into `paths` (as Claude's adapter does for `~/notes/*.md`).

3. **K1–K4 vs round-8 blockers 1–4:**
   - **K1:** Closed for `command -Z X eval`, `env --unknown-val X eval`, `$(which env) --chd/-C ..`, `env -i $(which env) -C ..`, `$(which timeout) 5 $(which env) -C ..`, `env --zzz X bash <(...)`, and `xargs -l 5 bash <(...)`. Incomplete on `env --zzz X -C ..` and `env -Z X ...` (Finding 2).
   - **K2:** Closed for 2-hop and 3-hop symlinks, `expand_glob` returning `None` when `~/.ssh` has no outbound link, `atrium_data_paths` / `atrium_protected_paths`, GNU abbreviated prefixes (`grep --deref`, `ag --fol`, `ack --fol`), and tree walkers (`cp -RL`, `cp --dereference -r`, `tar -chzf`, `rsync -aL`, `zip -r`, `scp -r`, plus `Follow::Maybe` on `cp -r` / `7z`).
   - **K3:** Closed for `-o`/`+o`/`-O`/`+O`/`-eo`/`--rcfile`/`--init-file`/`--emulate` before `-c` in `shell_operand_starts`/`shell_c_codes`, and 1-letter `--c` / `--c=` on `script` and `flock` in `take_opt`.
   - **K4:** Closed for `find -exec`/`-execdir`/`-ok`/`-okdir` extracting `payloads_of`, `env_payloads`, `code_subs`, and nested `find`s in `find_exec_units`.

4. **Ordinary lines & code quality:**
   - `adversarial_ordinary` (16/16) and the `*_ordinary_*` tests in `adversarial_m1_18` pass with no false hard denies.
   - Walk limits (`links_below` `BUDGET = 4096` / `MAX_DEPTH = 16` + `visited` set, `MAX_DEPTH = 6` and `MAX_UNITS` in `shell.rs`) fail closed (`Ask` / `overflow`) and do not panic, **except** the `lex_depth` fan-out regression in Finding 4.

---

## Commands Run

- `git status --short` -> empty (clean worktree).
- `cargo clippy -p atrium-core --all-targets -- -D warnings` -> exit `0` (`0` warnings, `14.74s`).
- `cargo test -p atrium-core --no-fail-fast` -> exit `101` (`adversarial_fuzz::random_hostile_lines_never_panic_and_nothing_sensitive_is_allowed` **FAILED**: `too slow: 193.489416416s`; `tests/policy.rs`: 44 passed in `291.77s`; `tests/store.rs`: 30 passed in `141.66s`; `tests/adversarial_gate.rs`: 9 passed, 2 ignored; `tests/adversarial_ledger.rs`: 14 passed; `tests/ledger.rs`: 20 passed; `src/lib.rs`: 9 passed; doc-tests: 0).
- `cargo test -p atrium-core --test adversarial_fuzz` (standalone) -> exit `0`: `2 passed; 0 failed; 1 ignored; finished in 58.04s`.
- `cargo test -p atrium-core --test policy engine_never_panics_on_garbage` (standalone) -> exit `0`: `1 passed; 0 failed; 43 filtered out; finished in 129.47s`.
- `cargo test -p atrium-core --test adversarial_m1_18` -> exit `0`: `13 passed; 0 failed; 0 ignored; finished in 4.88s`.
- `cargo test -p atrium-core --test adversarial_regression --test adversarial_ordinary --test adversarial_policy --test adversarial_gate --test adversarial_ledger` -> exit `0`:
  - `adversarial_gate`: `9 passed; 0 failed; 2 ignored` (`0.12s`)
  - `adversarial_ledger`: `14 passed; 0 failed; 0 ignored` (`0.17s`)
  - `adversarial_ordinary`: `16 passed; 0 failed; 0 ignored` (`0.11s`)
  - `adversarial_policy`: `16 passed; 0 failed; 3 ignored` (`0.08s`)
  - `adversarial_regression`: `57 passed; 0 failed; 0 ignored` (`1.67s`)

