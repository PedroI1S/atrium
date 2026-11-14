# Review: M1-13-adversarial, fourth round (commit 26262aa)

Reviewer: Gemini 4 Hydrogen `high`, read-only (D-079). Author: Claude Opus 5.5. Saved verbatim by the orchestrator from the agent's final message, minus its progress line and title; the `file://` links point at the worker's worktree. The reviewer could not read the previous review (the worker policy refuses absolute paths outside the worktree) and worked from the round_4 tests and the handoff instead.


**Verdict: rework**

Every `round_4_*` test on the branch passes, `clippy` is clean, and `adversarial_fuzz` finishes in ~12.2 s (well within the 60 s bound). However, targeted testing against the new fourth-round code turned up **7 security blockers** (inputs touching sensitive paths or bypassing hard denies that reach `Allow`, including one that allows in `Home` with no rules) and **1 major** family of false hard denies on ordinary `cd X && ...` chains ending in `sh -c`, `eval`, `$(...)`, or `find -exec`.

*(Scope note: reading `/Users/pedro/Harness/docs/agents/reviews/M1-13-adversarial-third-part.md` directly was blocked by the workspace pre-tool hook, so the previous review's inputs were verified via the `round_4_*` tests in [adversarial_regression.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/tests/adversarial_regression.rs#L662-L842), [adversarial_ordinary.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/tests/adversarial_ordinary.rs#L407-L447), and [M1-13-adversarial.md](file:///Users/pedro/atrium-wt/M1-13-adversarial/docs/agents/handoffs/M1-13-adversarial.md#L447-L558), plus a temporary test binary that was deleted afterward.)*

---

## Findings

### 1. [blocker] `Grep` tool with empty `paths` in a linked `~/.ssh` folder allows even in `Home`; `rgrep` allows under `Any`/`FullAccess`
- **Location:** [gate.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/gate.rs#L316-L393), [engine.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/engine.rs#L566-L580), [deny.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/deny.rs#L131-L142)
- **Cause:**
  1. When `ToolSubject::Paths { paths: [], recursive: true, .. }` (`Grep`) is checked with `cwd` inside a folder symlinked into `~/.ssh` (`~/.ssh/work -> ~/vault/work`), `a.uses` is empty ([deny.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/deny.rs#L139)). `reaches_linked_ssh_dir` only iterates `for u in &a.uses`, so it returns `false`. `Home::evaluate` ([engine.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/engine.rs#L568-L579)) falls back to `vec![input.cwd.to_path_buf()]` and checks `self.root_ok`, which uses `Sensitive::is_or_contains` (unaware of `linked_dirs`).
  2. `recursive_reader` ([gate.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/gate.rs#L322)) lists `grep`, `egrep`, `fgrep`, `zgrep`, `ggrep`, missing `rgrep` (GNU recursive grep). Also note non-`recursive_reader` directory walkers/archivers (`cp -r vault/work /tmp/exfil`, `find vault/work -exec cat {} +`, `rm -rf vault/work`) and multi-level globs (`cat vault/*/id_work`) are not caught by `reaches_linked_ssh_dir`, though `dev` didn't block those on linked folders either.
- **Evidence (SessionGate with `~/.ssh/work -> ~/vault/work`, settings `[home, any, full]`):**
  - `Grep { paths: [], recursive: true }` from `cwd = ~/vault/work` -> **`[Allow, Allow, Allow]`** (allows reading all keys in `~/.ssh/work` even in `Home` with no rules).
  - `rgrep x vault/work` -> `[Ask, Allow, Allow]`.
  - (`cp -r vault/work /tmp/exfil`, `find vault/work -exec cat {} +`, `rm -rf vault/work`, `cat vault/*/id_work` -> `[Ask, Allow, Allow]`.)

### 2. [blocker] Double-quoted `"${...}"` is not handled by the quote/brace-aware `${...}` scanner
- **Location:** [shell.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/shell.rs#L489-L493) (inside the `'"'` loop) vs [shell.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/shell.rs#L526-L578) (unquoted `'$'`)
- **Cause:** The quote/escape/nesting-aware `${...}` scanner was added only to the unquoted `'$'` branch. Inside `"..."`, `'$'` still pushes `'$'` and advances by 1. In POSIX/bash/zsh, `"${x:-"'"}" ; rm -rf ~` and `"${x:-"a'b"}" ; rm -rf ~` evaluate the inner `"..."` inside `${...}` and then run the command after `;`. In `shell.rs`, the inner `"` closes the outer `"`, and the `'` opens an unterminated single quote that swallows the rest of the line (`"unterminated quote"`).
- **Evidence (`[home, prefix, any, full]`):**
  - `echo "${x:-"'"}" ; rm -rf ~` -> `["Ask", "Ask", "Allow", "Allow"]` (expected `Hard(recursive-delete-outside-cwd)`).
  - `echo "${x:-"'"}" ; cat .ssh/id_rsa` -> `["Ask", "Ask", "Allow", "Allow"]` (expected `Hard(credential-read)`).
  - `echo "${x:-"a'b"}" ; rm -rf ~` -> `["Ask", "Ask", "Allow", "Allow"]` (expected `Hard(recursive-delete-outside-cwd)`).

### 3. [blocker] Nested compound commands inside a piped compound command prematurely pop the outer pipe group
- **Location:** [shell.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/shell.rs#L718-L729)
- **Cause:** `groups.push(...)` only triggers when `cur.argv.is_empty() && cur.assignments.is_empty() && quoted_prefix_empty`. Inside `if true; then { true; }; sh; fi` or `while read l; do if true; then true; fi; sh; done`, `then` / `do` / `else` is already in `cur.argv[0]`, so the inner `{` or `if` never pushes onto `groups`. Its closing `}` or `fi` appears with `cur.argv.is_empty()` (after `;`), so it pops the **outer** piped group instead, leaving `sh` with `piped = false, pipe = 0`.
- **Evidence (`[home, prefix, any, full]`):**
  - `echo 'rm -rf ~' | if true; then { true; }; sh; fi` -> `["Ask", "Ask", "Allow", "Allow"]` (expected `Hard(recursive-delete-outside-cwd)`).
  - `echo 'rm -rf ~' | while read l; do if true; then true; fi; sh; done` -> `["Ask", "Ask", "Allow", "Allow"]` (expected `Hard(recursive-delete-outside-cwd)`).
  - `echo 'cat .aws/credentials' | if true; then if true; then true; fi; sh; fi` -> `["Ask", "Ask", "Allow", "Allow"]` (expected `Hard(credential-read)`).

### 4. [blocker] Clustered value-taking options on `flock`/`script` (and `env`) before `-c` / command lose payload parsing
- **Location:** [shell.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/shell.rs#L1184-L1208) (`flock`/`script`), [shell.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/shell.rs#L1277-L1314) (`env`)
- **Cause:** In `flock`/`script`, the cluster branch only handles `cs[k] == 'c'`. When another value-taking short option (`w`, `E`, `t`, `F`, etc.) ends a cluster (e.g. `flock -nw 10 /tmp/l -c 'rm -rf ~'`), `with_value.contains(&"-nw")` is false, so `j` advances by 1 instead of 2; `10` sets `file_seen = true`, `/tmp/l` hits `break`, and `-c 'rm -rf ~'` is never unwrapped or lexed. Similarly, `env` only checks exact `"-u" | "-C" | "-P" | "-a"` and treats clustered `-iC proj` / `-iu PATH` as a 1-word flag, making `proj`/`PATH` the command word instead of `sh`.
- **Evidence (`[home, prefix(incl. flock, env), any, full]`):**
  - `flock -nw 10 /tmp/l -c 'rm -rf ~'` -> `["Ask", "Allow", "Allow", "Allow"]`
  - `flock -nE 2 /tmp/l -c 'cat .aws/credentials'` -> `["Ask", "Allow", "Allow", "Allow"]`
  - `script -qt 1 /dev/null -c 'rm -rf ~'` -> `["Ask", "Allow", "Allow", "Allow"]`
  - `env -iC proj sh -c 'rm -rf ~'` -> `["Ask", "Allow", "Allow", "Allow"]`
  - `env -iu PATH sh -c 'rm -rf ~'` -> `["Ask", "Allow", "Allow", "Allow"]`

### 5. [blocker] Attached `crontab -uUSER` returns `false` whenever `USER` contains `l`, `r`, or `e`
- **Location:** [shell.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/shell.rs#L1454-L1476)
- **Cause:** `reads_stdin_as_script` skips `k += 2` only for exact `a == "-u"`. An attached `-uroot` or `-upedro` falls through to `a.strip_prefix('-')` -> `flags.contains(['l', 'r', 'e'])`, which matches `r`/`e`/`l` in the username and returns `false`.
- **Evidence (`[home, prefix(incl. crontab), any, full]`):**
  - `echo '* * * * * rm -rf ~' | crontab -uroot` -> `["Ask", "Allow", "Allow", "Allow"]` (expected `Hard(recursive-delete-outside-cwd)`).
  - `echo '@reboot cat .aws/credentials' | crontab -upedro` -> `["Ask", "Allow", "Allow", "Allow"]` (expected `Hard(credential-read)`).

### 6. [blocker] `cd ~-` / `cd ~+1` / `cd -1` no longer mark cwd unknown, and `|| exit 1 &` falsely narrows `cd` candidates
- **Location:** [words.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/words.rs#L274-L276), [words.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/words.rs#L521-L545), [words.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/words.rs#L589-L601)
- **Cause:**
  1. The `253c7f4..26262aa` refactor dropped `|| a.starts_with("~+") || a.starts_with("~-")` from `cd_effect`. `expand_home("~-", home, c)` maps `~-` ($OLDPWD) to `c` (current candidate), and `~+1` / `-1` resolve as relative paths `~/~+1` / `~/-1`, all returning `Cd::Moved` instead of `Cd::Lost`. In particular, `cd .ssh && cd /tmp && cd ~- && cat id_rsa` moves `cands` to `[/tmp]` instead of `Lost` (dropping `~/.ssh`!).
  2. `exits_after` checks `n.pipe == 0 && !n.grouped && !n.seg.piped`, which is true for a backgrounded `exit 1 &`. In `cd /nonexistent || exit 1 & cat .ssh/id_rsa`, `exit 1 &` exits only a background subshell, yet `exits_after` drops `~` from `cands`.
- **Evidence (`[home, prefix, any, full]`):**
  - `cd /nonexistent || exit 1 & cat .ssh/id_rsa` -> `["Ask", "Ask", "Allow", "Allow"]` (expected `Hard(credential-read)`).
  - `cd proj || exit 1 & rm -rf *` -> `["Ask", "Ask", "Allow", "Allow"]` (expected `Hard(recursive-delete-outside-cwd)`).
  - `cd ~-; cat id_rsa`, `cd ~+1; cat id_rsa`, `cd -1; cat id_rsa` -> `["Ask", "Ask", "Allow", "Allow"]` (missing `Lost` / `cwd_uncertain`). (By inspection of `words.rs:274,544,712`: `cd .ssh && cd /tmp && cd ~- && cat id_rsa` drops `~/.ssh` when `cd /tmp &&` is followed by `cd ~-`.)

### 7. [blocker] Wrappers before a substituted command word (`env $(echo 'rm -rf ~')`) skip `output_texts`
- **Location:** [shell.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/shell.rs#L798-L802)
- **Cause:** Line 798 checks `seg.argv.first()` instead of `eff.first()`, so when a transparent wrapper (`env`, `nohup`, `command`, `time`) precedes `$(echo 'rm -rf ~')`, `has_command_substitution` is checked on `"env"` instead of `"$(echo 'rm -rf ~')"`.
- **Evidence (`[home, prefix, any, full]`):**
  - `env $(echo 'rm -rf ~')` -> `["Ask", "Ask", "Allow", "Allow"]` (expected `Hard(recursive-delete-outside-cwd)`).
  - `nohup $(echo 'rm -rf ~')` -> `["Ask", "Ask", "Allow", "Allow"]` (expected `Hard(recursive-delete-outside-cwd)`).

### 8. [major] False hard denies on `cd X && ...` chains ending in `sh -c`, `eval`, `$(...)`, or `find -exec`
- **Location:** [words.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/words.rs#L734-L742)
- **Cause:** In `words::walk`, when a top-level unit `u` is the last command in an `&&` chain (`u.join == Join::Seq`), lines 734–742 flush `lv.pool` (`[~]`) back into `lv.cands` **before** the loop moves on to `u`'s derived child units (`sh -c` payload, `eval` payload, `$(...)` substitution, `find -exec` command). Those child units only run if `cd X` succeeded, yet they enter `Level` with `top.cands = [~/proj, ~]`. (`eval 'cd proj && echo hi' && cat .npmrc` hits the same issue when `L1` ends with `Join::Seq` and flushes `L1.pool` before handing `L1.cands` back to `L0`.) On `dev`, all of these are `["Ask", "Ask", "Allow", "Allow"]`.
- **Evidence (`[home, prefix, any, full]`):**
  - `cd proj && sh -c 'cat .npmrc'` -> `Hard(credential-read)` in all 4 settings
  - `cd build && sh -c 'rm -rf *'` -> `Hard(recursive-delete-outside-cwd)` in all 4 settings
  - `cd proj && eval 'cat .npmrc'` -> `Hard(credential-read)` in all 4 settings
  - `cd build && eval 'rm -rf *'` -> `Hard(recursive-delete-outside-cwd)` in all 4 settings
  - `cd proj && echo $(cat .npmrc)` -> `Hard(credential-read)` in all 4 settings
  - `cd proj && find . -name '*.o' -exec cat .npmrc \;` -> `Hard(credential-read)` in all 4 settings
  - `cd build && find . -name '*.o' -exec rm -rf * \;` -> `Hard(recursive-delete-outside-cwd)` in all 4 settings
  - `eval 'cd proj && echo hi' && cat .npmrc` -> `Hard(credential-read)` in all 4 settings

### 9. [minor] Answers to prompt checks on `ls vault/*` and ordinary lines
- **`ls vault/*` vs `ls vault/work/*`:** Note a wording error in [M1-13-adversarial.md](file:///Users/pedro/atrium-wt/M1-13-adversarial/docs/agents/handoffs/M1-13-adversarial.md#L531): when `~/.ssh/work -> ~/vault/work`, **`ls vault/*` is NOT hard-denied** (`Ask` in `Home`, `Allow` under `Any`/`FullAccess`). It is **`ls vault/work/*`** (a glob *inside* the linked folder itself) that is `Hard(credential-read)`, matching `ls ~/.ssh/*` on `dev`, while `ls vault/work` stays `Allow`. That behavior is acceptable.
- **Substituted pipe consumer trade-off:** `curl -s https://example.com/data.json | $(brew --prefix)/bin/jq .` is now `Hard(pipe-to-shell)` ([deny.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/deny.rs#L558-L560)), whereas `dev` returned `Ask`/`Allow`. This is a direct consequence of treating any pipe into `$(...)` as potential shell code.
- **Other ordinary lines checked (all matched expected non-hard-deny verdicts):** `git log | while read l; do echo $l; done` (`[Ask, Ask, Allow, Allow]`), `ls | for f in a b; do echo $f; done` (`[Ask, Ask, Allow, Allow]`), `grep -d skip x proj` (`Allow`), `grep -d skip x .` (`[Ask, Ask, Allow, Allow]`), `cd -P build && make` (`[Ask, Ask, Allow, Allow]`), `cd -L .. && ls` (`[Ask, Allow, Allow, Allow]`), `cd proj || return; make` (`[Ask, Ask, Allow, Allow]`), `cd proj || exit 1; make` (`[Ask, Ask, Allow, Allow]`), `crontab -l` (`[Ask, Allow, Allow, Allow]`), `flock /tmp/l make` (`[Ask, Allow, Allow, Allow]`), `echo ${HOME:-"/tmp"}` (`[Ask, Ask, Allow, Allow]`), `rg TODO src` (`Allow`), `grep -rn TODO src` (`Allow`).

---

## Checks Run

- `cargo clippy -p atrium-core --all-targets -- -D warnings`: exit 0, 0 warnings (`1.08s`).
- `cargo test -p atrium-core --no-fail-fast -q`: exit 0 (`209 passed; 0 failed; 6 ignored` across `atrium-core` lib 9, `adversarial_fuzz` 2 + 1 ignored [`12.21s`], `adversarial_gate` 9 + 2 ignored, `adversarial_ledger` 14, `adversarial_ordinary` 7, `adversarial_policy` 15 + 3 ignored, `adversarial_regression` 28, `attention` 8, `history` 25, `ledger` 20, `policy` 44, `store` 28).
- `cargo test -p atrium-core --test adversarial_fuzz` (standalone timing): exit 0 (`2 passed; 0 failed; 1 ignored; finished in 12.24s`).
- `git status`: clean (`nothing to commit, working tree clean`).

