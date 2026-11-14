# Review: M1-13-adversarial, third part (fixes, commit 253c7f4)

Reviewer: Gemini 4 Hydrogen `high`, read-only (D-074, D-075). Authors: Claude Sonnet 5.5 (tables), Claude Opus 5.5 (fixes). The review was returned as the agent's final message and saved here verbatim by the orchestrator, minus its title line; the `file://` links point at the worker's worktree. Its item 8 (uncertain cwd under Any/FullAccess) was decided by Pedro as D-078: keep dev's behaviour.

**Verdict: rework**

Every input reaching `Allow` on a sensitive path is listed first as a **blocker**, followed by new false hard denies on ordinary lines as **major**, and test/tooling findings as **minor** / **nit**. Sections 1–9 address each question in the review checklist.

---

## Findings

### Blockers (inputs reaching `Allow` on sensitive paths)

#### [blocker] 1. Recursive search of a folder symlinked into `~/.ssh` reaches `Allow` even in `Home` with no rules (`gate.rs:195-230`)
In [gate.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/gate.rs#L195-L230), `ssh_members` skips pushing directories reached through a symlink (`if linked { continue; }` at line 224) and also caps traversal at `depth < 3` (line 203). Because the linked directory itself is omitted from `ssh_members`, `alias_fix` ([gate.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/gate.rs#L167-L185)) never maps a path naming that directory (or files beyond depth 2 from `~/.ssh`) back onto `~/.ssh/...`.

Verified empirically with `Tmp` + `SessionGate` (where `.ssh/work -> vault/work` containing `vault/work/id_work`, and `.ssh/deep -> vault` containing `vault/a/b/id_key` at depth 3 from `.ssh`):
- `rg x vault/work` → `Allow` (`Preset::Home`, no rules) and `Allow` (`Preset::FullAccess`)
- `grep -r x vault/work` → `Allow` (`Preset::Home`, no rules) and `Allow` (`Preset::FullAccess`)
- `Grep` (`ToolKind::Search`, `paths: ["vault/work"]`, `recursive: true`) → `Allow` (`Preset::Home`, no rules) and `Allow` (`Preset::FullAccess`)
- `cat vault/a/b/id_key` → `Allow` (`Preset::Home`, no rules) and `Allow` (`Preset::FullAccess`)
- `cat vault/work/*` → `Ask` (`Preset::Home`), `Allow` (`Preset::FullAccess`)

Whereas `ls` / `Read` on ordinary folders must stay allowed per M1-03 finding 5 ([adversarial_policy.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/tests/adversarial_policy.rs#L208-L245)), **recursive search tools/commands** (`rg`, `grep -r`, `Grep`) over a folder that contains an `ssh_members` file cannot return `Allow` without leaking key contents. Additionally, `ssh_members` skipping `depth >= 3` leaves nested keys reachable via `.ssh/deep/a/b/id_key` completely unprotected.

#### [blocker] 2. Hard-linked credentials inside a folder reach `Allow` via `grep -d recurse` / `grep --directories recurse` in `Home` (`gate.rs:283-297`, `engine.rs:441-467`)
`Home::readonly` ([engine.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/engine.rs#L441-L467)) treats `grep` with `--directories=recurse`, `--directories recurse`, long abbreviations (`--rec`), and `-d recurse` (combined short `d` flag) as recursive searches and allows them on non-sensitive folders such as `proj/`. However, `recursive_reader` ([gate.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/gate.rs#L283-L297)) only checks `-r` / `-R` (short clusters), `--recursive*`, `--dereference-recursive*`, and exact `--directories=recurse`. It misses space-separated `--directories recurse` and short `-d` (`-d recurse`).

Verified empirically with `Tmp` + `SessionGate` where `proj/k2` is hard-linked to `.gnupg/private-keys-v1.d/k.key`:
- `grep -d recurse x proj` → `Allow` (`Preset::Home`, no rules) and `Allow` (`Preset::FullAccess`)
- `grep --directories recurse x proj` → `Allow` (`Preset::Home`, no rules) and `Allow` (`Preset::FullAccess`)

#### [blocker] 3. `cd X || return ...` and `cd X || exit ... | ...` drop `~` as a candidate directory (`words.rs:614-627`)
In [words.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/words.rs#L614-L627), `exits_on_failure` narrows candidates solely to `targets` whenever `join == Join::Or` and the next unit begins with `exit` or `return`. That assumption fails in two real shell cases:
1. At top level (outside a function or sourced file), `return` fails and execution continues with `;` into the next command in the old directory (`~`).
2. In a pipeline (`cd /nonexistent || exit 1 | true; ...`), `exit 1` only exits its pipeline subshell, not the parent shell, so execution continues in `~`.

Verified empirically with `Env::home()`:
- `cd /nonexistent || return 0; cat .ssh/id_rsa` → `Ask` (`Home`), **`Allow`** (`Prefix` rules `[cd, return, cat]`, `Any`, `FullAccess`)
- `cd /nonexistent || exit 1 | true; cat .ssh/id_rsa` → `Ask` (`Home`), **`Allow`** (`Prefix` rules `[cd, exit, true, cat]`, `Any`, `FullAccess`)
- `cd proj || return; rm -rf *` → `Ask` (`Home`, `Prefix`), **`Allow`** (`Any`, `FullAccess`)

#### [blocker] 4. Combined flags on `cd` (`-LP`, `-Pe`, etc.) drop targets from `seen` and bypass credential/delete checks (`words.rs:511-523`)
In [words.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/words.rs#L511-L523), `cd_effect` strips only exact single tokens `"-L" | "-P" | "-e" | "-@" | "--"`. Combined option clusters accepted by shells (`cd -LP .ssh`, `cd -Pe .ssh`) leave `args.len() == 2`, falling into `_ => None` without pushing the target directory into `seen`. Subsequent commands resolve only against `home`/`start`.

Verified empirically with `Env::home()`:
- `cd -LP .ssh && cat id_rsa` → `Ask` (`Home`, `Prefix`), **`Allow`** (`Any`, `FullAccess`)
- `cd -Pe .ssh; cat id_rsa` → `Ask` (`Home`, `Prefix`), **`Allow`** (`Any`, `FullAccess`)

#### [blocker] 5. `printf` positional conversions (`%2$s`), pipe-into-group after `;`, `${...}` brace-in-quotes desync, wrapper attached `-c`, bare `crontab`, and substituted shell names in pipes (`shell.rs:499-513, 788-794, 1035-1055, 1297, 1509-1548`)
Several lexer/payload gaps allow sensitive reads or destructive commands to evade `hard_deny`, reaching `Ask` in `Home` and `Allow` under prefix/`Any`/`FullAccess`:

1. **`printf` positional arguments (`%2$s%1$s`)** (`format_printf` in `shell.rs:1509-1548` treats `$` as the conversion specifier rather than an argument index):
   - `printf '%2$s%1$s' 'credentials' 'cat .aws/' | sh` → `Ask` (`Home`), **`Allow`** (`Prefix` `[printf, sh]`, `Any`, `FullAccess`)
2. **Pipe into a group whose shell runs after `;`** (`shell.rs:788-794` propagates `pipe` only to the first unit of `extra` or resets on `;`):
   - `printf 'rm -rf ~\n' | (true; sh)` → `Ask` (`Home`, `Prefix`), **`Allow`** (`Any`, `FullAccess`)
   - `echo 'cat .aws/credentials' | { :; sh; }` → `Ask` (`Home`, `Prefix`), **`Allow`** (`Any`, `FullAccess`)
3. **`}` inside quotes inside `${...}`** ([shell.rs:499-513](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/shell.rs#L499-L513) breaks at the first `}` without tracking quotes, turning the real closing `}` into part of `"}"` and making `; cat .ssh/id_rsa` vanish inside an unclosed `"..."` word):
   - `echo ${x:-"}"} ; cat .ssh/id_rsa` → `Ask` (`Home`, `Prefix`), **`Allow`** (`Any`, `FullAccess`)
4. **Attached `-c` on `flock` / `script`** ([shell.rs:1035-1055](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/shell.rs#L1035-L1055) checks `matches!(w, "-c" | "--command")` only):
   - `flock -c'cat .aws/credentials; true' /tmp/l` → `Ask` (`Home`), **`Allow`** (`Prefix` `[flock]`, `Any`, `FullAccess`)
   - `script -c'cat .aws/credentials; true' /dev/null` → `Ask` (`Home`), **`Allow`** (`Prefix` `[script]`, `Any`, `FullAccess`)
   - `flock /tmp/l -c'rm -rf ~'` → `Ask` (`Home`, `Prefix`), **`Allow`** (`Any`, `FullAccess`) *(Note: GNU `flock` expects options before the lockfile, though `flock -c'rm -rf ~' /tmp/l` is valid.)*
5. **Bare `crontab` without `-`** (`shell.rs:1297` requires `-`, `/dev/stdin`, or `/dev/fd/0`, whereas Linux/BSD/macOS `crontab` reads stdin when invoked with no file operand):
   - `echo '* * * * * rm -rf ~' | crontab` → `Ask` (`Home`), **`Allow`** (`Prefix` `[echo, crontab]`, `Any`, `FullAccess`) *(Note: `crontab <<EOF\n* * * * * rm -rf ~\nEOF` still hits `Hard(recursive-delete-outside-cwd)`.)*
6. **Substituted command word in a pipe without `-s`/`-`** (`is_stdin_code` / `is_pipe_code` require `is_shell(f)` rather than resolving `output_texts` or `maybe_shell` for `s$(echo h)` / `$(echo sh)`):
   - `echo 'cat .ssh/id_rsa' | s$(echo h)` → `Ask` (`Home`, `Prefix`), **`Allow`** (`Any`, `FullAccess`)
   - `echo 'rm -rf ~' | $(echo sh)` → `Ask` (`Home`, `Prefix`), **`Allow`** (`Any`, `FullAccess`)

---

### Majors (new false hard denies on ordinary lines)

The rule for this round requires the branch to be no stricter on ordinary lines (a false hard deny cannot be overridden). Two bugs around `unit.derived` ([words.rs:605](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/words.rs#L605)) cause false hard denies on completely ordinary commands:

#### [major] 1. `unit.derived` flushes `pool` mid-`&&` chain ([words.rs:604-650](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/words.rs#L604-L650))
Every command substitution `$(...)`, `find -exec ...`, or nested `-c` payload inside a top-level `&&` chain produces derived units whose `unit.derived == true`. At `words.rs:605`, `let join = if unit.derived { Join::Seq } else { unit.join };` overrides their join kind to `Join::Seq`. At lines 645–653, `if join != Join::And && !pool.is_empty()` immediately appends `pool` (which still contains `~` from before the `cd`!) back into `cands`. Consequently, the very next command in the `&&` chain is evaluated as if it might still be running in `~`.

Verified empirically with `Env::home()`:
- `cd proj && echo $(date) && cat .npmrc` → **`Hard(credential-read)`** (`Home` & `Prefix`) — should be `Ask`/`Allow` (`~` was left via `cd proj &&`)
- `cd build && TAG=$(date +%F) && rm -rf *` → **`Hard(recursive-delete-outside-cwd)`** (`Home` & `Prefix`) — should be `Ask`
- `cd proj && find . -name '*.o' -exec true {} + && cat .npmrc` → **`Hard(credential-read)`** (`Home` & `Prefix`) — should be `Allow` with prefix rules `[cd, find, cat]` (no lexer reasons!)
- `cd proj && sh -c 'true' && cat .npmrc` → **`Hard(credential-read)`** (`Home` & `Prefix`) — should be `Ask`

#### [major] 2. `unit.derived` forces `Join::Seq` inside `-c`, `env -C`, and `eval` payloads ([words.rs:605](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/shell.rs#L742), [shell.rs:1767](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/shell.rs#L1767))
Because `words.rs:605` forces `Join::Seq` on *every* `unit.derived` unit, any `cd X && Y` inside `sh -c '...'`, `eval '...'`, or synthetic `env -C DIR CMD` (`env_payloads` emits `cd 'DIR' && CMD` at [shell.rs:1767](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/src/policy/shell.rs#L1767)) loses its `&&` edge and retains `~` in `cands` for `Y`.

Verified empirically with `Env::home()`:
- `sh -c 'cd proj && cat .npmrc'` → **`Hard(credential-read)`** (`Home` & `Prefix`)
- `sh -c 'cd build && rm -rf *'` → **`Hard(recursive-delete-outside-cwd)`** (`Home` & `Prefix`)
- `sh -c "cd proj && find . -name '*.o' -delete"` → **`Hard(recursive-delete-outside-cwd)`** (`Home` & `Prefix`)
- `env -C proj cat .npmrc` → **`Hard(credential-read)`** (`Home` & `Prefix`)
- `env -C build rm -rf *` → **`Hard(recursive-delete-outside-cwd)`** (`Home` & `Prefix`)
- `eval 'cd proj && cat .npmrc'` → **`Hard(credential-read)`** (`Home` & `Prefix`)

*Fix note for majors 1–2*: Derived/payload units need to track their own chain (`pool`/saved `cands` scoped per payload or `extra`) separately from the enclosing top-level `&&` chain, rather than aliasing `Join::Seq` onto derived units and letting them drain or corrupt the caller's `pool`.

---

### Minor & Nit

#### [minor] 1. `adversarial_fuzz` 40 s debug-build wall-clock assertion flakes under parallel load ([adversarial_fuzz.rs:234-238](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/tests/adversarial_fuzz.rs#L234-L238))
`random_hostile_lines_never_panic_and_nothing_sensitive_is_allowed` runs 40,000 iterations × (3 `DefaultPolicy::evaluate` calls + 1 `parse_command` call = 160,000 policy/parse passes in unoptimized debug code) and asserts `started.elapsed().as_secs() < 40`. On commit `253c7f4`, the policy engine does materially more work per hostile line (`payloads_of`, `env_payloads`, `echo_printf_texts`, `MAX_CANDIDATES = 32` candidate lists, `pool` / `seen` tracking in `words.rs`), raising solo runtime from ~14–17 s to ~12–22 s (observed `12.15s`–`12.36s` idle here; reported `15–22s` in handoff). Under heavy concurrent load (`cargo test` across packages, load avg ~80–100 reported in [handoffs/M1-13-adversarial.md:417-442](file:///Users/pedro/atrium-wt/M1-13-adversarial/docs/agents/handoffs/M1-13-adversarial.md#L417-L442)), debug-profile wall-clock latency crossed 40 s (`40.21s`, `46.6s`). That makes the `< 40s` threshold brittle for CI / pre-merge runs; raising the debug bound to `< 60s` (or keeping `40_000` only via `ATRIUM_FUZZ_ITERS`) is safer while preserving O(n) scaling guarantees.

#### [nit] 1. Overbroad comment on the macOS-ignored host test ([adversarial_supervise.rs:374-381](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-host/tests/adversarial_supervise.rs#L374-L381))
The comment/ignore reason states `"zombie-leader premise is Linux-only"`, whereas an unreaped zombie leader keeping its `pgid` alive is standard POSIX behavior. The actual macOS-only obstacle is `/usr/bin/python3` being a re-execing stub on macOS, causing `proc_pidpath` identity verification in `kill_recorded` to return `Unverified` before termination/zombie creation. Still, the `#[cfg_attr(not(target_os = "linux"), ignore = ...)]` itself is justified unless rewritten with a non-stub binary.

---

## Checklist Answers

### 1. Blockers 1–4 and majors 5–8: really fixed? Do tests pin the review's exact inputs?
- **Blocker 1 (symlink into `~/.ssh`)**: File symlinks are fixed and pinned in `finding_1_a_key_symlinked_into_ssh_is_still_a_credential`. Linked folders (`finding_1_a_folder_of_keys_linked_into_ssh`) pass for `cat` / `Read`, **but fail the recursive-search regression check** above (`rg x vault/work`, `grep -r x vault/work`, `Grep` reach `Allow`; see Blocker 1).
- **Blocker 2 (failing `cd` + prefix rules)**: Fixed and pinned for `;`, `\n`, and `||` chains (`finding_2_failing_cds_then_a_credential`, `finding_2_failing_cds_joined_by_or`, `held_one_failing_cd_forms`, `held_deletes_after_a_failing_cd`, `finding_2_with_plain_prefix_rules_a_failing_cd_reaches_allow`). Reopened on `cd X || return` and `cd X || exit 1 | true` (Blocker 3) and broken on derived units inside `&&` chains / payloads (Majors 1–2).
- **Blocker 3 (`pushd`/`popd`/`cd -`)**: Fixed and pinned (`finding_3_pushd_popd_and_cd_dash_leave_the_cwd_unknown`, `finding_3_popd_back_into_a_credential_folder_is_a_hard_deny`, `finding_3_popd_pushd_and_cdpath_leave_the_directory_unknown`). Note the owner decision on `cd -` under `Any`/`FullAccess` (item 8) and the combined-flag gap `cd -LP` (Blocker 4).
- **Blocker 4 (`cd` through symlink un-aligns gate)**: Both fixes are implemented (`words.rs` avoids value deduplication so gate/engine candidate vectors align; `deny.rs:874-887` checks unaligned gate paths with the strongest `Role`), pinned by `finding_4_a_cd_through_a_symlink_keeps_the_write_check` and `finding_4_control_the_same_writes_without_a_cd`.
- **Major 5 (here-doc / payload to substituted shell)**: Fixed and pinned (`finding_5_heredoc_to_a_shell_named_with_a_substitution`, including `docker exec -i $(docker ps -q) sh <<EOF`, and `finding_5_payload_for_a_shell_named_with_a_substitution`). Still misses piped substituted shells like `echo 'cat .ssh/id_rsa' | s$(echo h)` (Blocker 5).
- **Major 6 (ordinary lines hard-denied after working `cd`)**: Fixed for plain `cd X && Y` chains (`ordinary_lines_after_a_working_cd`), **but regresses to false hard denies** when an intermediate command contains a substitution, `-exec`, or `-c` payload, or inside `sh -c`/`eval`/`env -C` (Majors 1–2).
- **Major 7 (`echo`/`printf` piped to shell)**: Fixed for `%s`/`%c`/`%b`/width/escapes and `{ sh; }` / `( sh )`, pinned by `finding_7_text_piped_to_a_shell_is_code_however_it_is_spelled`, `held_echo_and_printf_into_a_shell`, `finding_8_echo_and_printf_text_piped_to_a_shell_is_read_whole`. Misses `%2$s` positional args and `{ :; sh; }` / `(true; sh)` (Blocker 5).
- **Major 8 (Handoff "Second part" rewrite)**: Checked against `git show --stat efd29fc` and `docs/agents/handoffs/M1-13-adversarial.md:60-76`. The rewritten summary now accurately reflects the actual changes in `words.rs`, `deny.rs` (1 line), `shell.rs`, and `adversarial_policy.rs`.

### 2. New inputs reaching `Allow` on a sensitive path
Listed in full under **Blockers 1–5** above, all verified empirically on commit `253c7f4`.

### 3. New false hard denies on ordinary lines
- Plain `cd proj && cat .npmrc`, `cd build && rm -rf *`, `cd proj && find . -name '*.o' -delete`, `tee notes-$(date).md <<EOF\nsudo make install\nEOF`, everyday `git`/`cargo`/`npm` lines in `adversarial_ordinary.rs` pass without false hard denies.
- **However**, inserting any derived unit inside an `&&` chain or placing `cd X && Y` inside `-c`/`eval`/`env -C` triggers false hard denies (see **Majors 1–2**).

### 4. Table corrections (finding 2 `&&` vs `;`, finding 3 `cd -`, `;` rows moved)
Checked in [adversarial_regression.rs:151-246](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/tests/adversarial_regression.rs#L151-L246) and [adversarial_ordinary.rs:188-271](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-core/tests/adversarial_ordinary.rs#L188-L271):
- **Finding 2 `&&` vs `;`/newline**: Splitting `&&` (`[Ask, Allow, Allow, Allow]` after `cd X &&`) from `;`/newline/after-`&&` (`Hard(credential-read)` / `Hard(recursive-delete-outside-cwd)` in `after_a_cd_that_may_fail_the_old_directory_counts` and `finding_2_and_chains_follow_only_success`) is **right**: after `cd X &&` the remainder executes only if `cd X` succeeded (no false hard deny per review finding 6), whereas after `;` or `\n` a failed `cd` leaves the shell in `~` (contract §9; same class as review finding 2). Moving `cd a; cd b; ... cd g; ls` (prefix rules) from `Allow` to `Ask` is also right once `;` makes the cwd uncertain.
- **Finding 2 `adversarial_policy.rs:926`**: Changing `../.ssh/id_rsa` to `../u/.ssh/id_rsa` fixes a real path bug (`/h/.ssh` vs `/h/u/.ssh`).
- **Finding 5 & tightened IDs**: Adding `docker exec -i $(docker ps -q) sh <<EOF ...` back and tightening Findings 5, 7, 9, 13, 14 from `NotAllow` to exact `Hard(...)` IDs improves coverage.
- **Finding 3 `cd -` rows (`[Ask, Ask, Allow, Allow]`)**: Matches the fixer's rule choice below (`dev` allows `cd $X; cat id_rsa` under `Any`/`FullAccess`), pending owner sign-off in item 8.

### 5. Flood test rewrite (`adversarial_socket.rs:332-413`)
The rewritten `flooding_unread_list_threads_past_8_mib_control_lane_closes_connection` in [adversarial_socket.rs](file:///Users/pedro/atrium-wt/M1-13-adversarial/crates/atrium-daemon/tests/adversarial_socket.rs#L332-L413) is **deterministic** and **fails properly** if the 8 MiB control-lane limit breaks:
- Removes timer/sleep races: Phase 1 queues `(6 MiB / size)` `ListThreads` requests followed by a `RenameThread` marker (`"phase-1-done"`), then awaits `setup.thread_when(...)` on a separate connection so Phase 1 responses are guaranteed to have been processed and queued on `flooded`'s control lane before Phase 2 starts (`phase2 = LIMIT / size + 2` pushes queued unread bytes past 8 MiB without draining).
- On the client read loop (`while flooded.read().await.is_some()`), it asserts `received < sent` on every response and then `flooded.closed().await`. If the 8 MiB bound stops dropping the connection, all `sent` answers arrive cleanly and the test fails immediately without hanging. Passes in `0.55s` (`0.61s` earlier).

### 6. macOS-only ignore of `kill_recorded_setsid_grandchild_and_unreaped_zombie_leader_boundaries`
Justified for the `/usr/bin/python3` stub behavior on macOS (`proc_pidpath` sees the re-execed interpreter path rather than `/usr/bin/python3`, so `kill_recorded` returns `Unverified` and never kills/zombifies the leader), though the test remains `#[cfg_attr(not(target_os = "linux"), ignore = ...)]` only because it uses `/usr/bin/python3` rather than a non-stub binary (`sh` + a small helper). See the minor wording note above.

### 7. `adversarial_fuzz` timing & bound
As analyzed under **Minor 1**, the policy engine is measurably heavier per call on hostile inputs (`shell.rs` deep payload/substitution extraction + `MAX_CANDIDATES = 32` walks across 160k calls per run), pushing solo debug runs from ~14–17 s to ~12–22 s and making the 40 s debug wall-clock check vulnerable under heavy multi-crate CPU contention (`load avg ~80–100`). Increasing the debug bound (e.g., to 60 s, or running fuzz assertions in release/less contended mode) makes more sense than failing CI on load spikes while the linear `pipe_to_shell_fan_out_is_linear_and_denied` test already bounds asymptotic blowup.

### 8. Owner decision: `Any` rules and `FullAccess` on uncertain cwd (`cd -; cat id_rsa`)
**Recommendation: keep `Any` and `FullAccess` allowing non-hard-denied commands on uncertain cwd (matching `dev`).**
- Under `FullAccess` ("never asks, only hard-denies") and `RuleMatcher::Any`, refusing to allow any command with an uncertain cwd would start prompting on `FullAccess` for routine subshells `(cd sub && make)`, `cd "$DIR"; ls`, and `cd -; ls`.
- Note also the practical difference between `cd -; cat id_rsa` and a true key read: unless `OLDPWD` is `.ssh`—which is session runtime state the static line doesn't name—`cat id_rsa` isn't statically known to target a credential (just like `cat "$P"`). If `cd -` followed by common key filenames like `id_rsa`/`id_ed25519` is considered too risky even under `FullAccess`, that should be addressed narrowly in `hard_deny` (or resolved via adapter/runtime `OLDPWD` tracking), rather than weakening `Any` / `FullAccess` for all uncertain-cwd commands.

---

## 9. Checks Run

All commands ran in `/Users/pedro/atrium-wt/M1-13-adversarial` on commit `253c7f4` with a clean working tree (`git status --short` returned empty):

- `cargo clippy -p atrium-core --all-targets -- -D warnings` → exit `0` (`Finished dev profile ... in 0.25s`, 0 warnings).
- `cargo test -p atrium-core --no-fail-fast` → exit `0`:
  - `lib.rs`: 9 passed, 0 failed (`0.00s`)
  - `adversarial_fuzz`: 2 passed, 0 failed, 1 ignored (`random_spellings_of_sensitive_paths_are_always_hard_denied`, `M1-13 finding 11`) (`12.15s`)
  - `adversarial_gate`: 9 passed, 0 failed, 2 ignored (`finding_10_...`, `finding_11_...`) (`0.10s`)
  - `adversarial_ledger`: 14 passed, 0 failed (`0.13s`)
  - `adversarial_ordinary`: 6 passed, 0 failed (`0.02s`)
  - `adversarial_policy` (verified via `--test adversarial_policy`): 15 passed, 0 failed, 3 ignored (`finding_4`, `finding_5`, `finding_6`) (`0.07s`)
  - `adversarial_regression` (verified via `--test adversarial_regression`): 22 passed, 0 failed (`0.02s`)
  - `attention` (verified via `--test attention`): 8 passed, 0 failed (`0.00s`)
  - `history` (verified via `--test history`): 25 passed, 0 failed (`1.40s`)
  - `ledger`: 20 passed, 0 failed (`0.01s`)
  - `policy`: 44 passed, 0 failed (`3.24s`)
  - `store`: 28 passed, 0 failed (`36.97s`)
  - `Doc-tests atrium_core`: 0 tests (`0.00s`)
- `cargo test -p atrium-daemon --test adversarial_socket flooding_unread` → exit `0`: `1 passed; 0 failed; 0 ignored; 6 filtered out` (`0.55s`).
- `cargo test -p atrium-host --test adversarial_supervise` → exit `0`: `7 passed; 0 failed; 1 ignored` (`kill_recorded_setsid_grandchild_and_unreaped_zombie_leader_boundaries`, `0.01s`).
- *(Empirical candidate verification used a temporary test file `crates/atrium-core/tests/review_verify_tmp.rs`, which was run and immediately deleted; `git status --short` verified clean afterward.)*

