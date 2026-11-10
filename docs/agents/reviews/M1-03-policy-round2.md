# Review: M1-03-policy (round 2)
Reviewer: Opus 5.5 (read-only; D-053) · 2026-11-09 · fixes at `6378100`

## Verdict
**approve with fixes**: all twelve round-1 findings are fixed, and none of the round-1 attacks reaches `Allow` any more. The new code fails closed: every size cap gives `Deny`, never `Allow`, and a panic or join error in the gate gives `Deny`. One new major remains. An `echo … | sh` fan-out is re-lexed quadratically *before* the size caps run, so a 30 KB line takes 3.7 s and about 2 GB of memory, and a line near the 128 KiB cap would exhaust memory. It ends in `Deny`, so this is a denial of service, not a bypass, and the fix is local. Fix finding a, and findings c and d (an unoverridable false hard-deny, and protected-path writes that only ask), before the gate. The rest can go to a later round.

All inputs below were run through `DefaultPolicy::evaluate` (home `/h/u`, protected `/h/u/.config/atrium` and `/Applications/Atrium.app`, preset Home) or through a real `SessionGate` on a temp home (APFS, macOS), from a throwaway test (`tests/zz_attack2.rs`) that was deleted afterwards. Timings are from release builds. Baseline: `cargo test -p atrium-core --test ledger --test policy` gives 20 + 37 passed, and `cargo clippy --workspace --all-targets -- -D warnings` is clean.

## Round-1 findings

| # | Sev | Status | Re-test (all confirmed by running) |
|---|---|---|---|
| 1 | blocker | **fixed** | `ls \r#; rm -rf ~` → `HardDeny(recursive-delete-outside-cwd)`, from the engine and the gate. `ls \r# ; touch ~/.zshrc` → Ask. `ls \r#; sudo id` → `HardDeny(privilege-escalation)`. The `cat … \r#;curl … -o p; sh p` line → Ask. Other C0/C1 controls and U+2028/2029 give the reason "control character". |
| 2 | blocker | **fixed** | `cat /.vol/<dev>/<ino>` and a `Read` of `/.vol/…` → Deny ("paths under /.vol can't be checked"). These are also denied: `/System/Volumes/Data<H>/.npmrc`, the lowercase firmlink `/system/volumes/data<H>/…`, the `Documents/../.npmrc` form, a `Read` through the firmlink, a cwd given through the firmlink, and a write to a protected path through the firmlink. A hard link to `.npmrc`, and a hard link to `.ssh/id_ed25519` (in a small home), are denied by the `(st_dev, st_ino)` check. A gap in that check is finding e. |
| 3 | blocker | **fixed** | `date -uf.npmrc`, `date -uf.netrc` and `date -uf.kube/config` → `HardDeny(credential-read)`, from the engine and the gate. `date` now also forbids `-f`/`--file` (D-054). |
| 4 | major | **fixed** | `cat -é` and `ls -é` → Allow, with no panic. A 30,000-line Unicode fuzz run, with a bare `-` in its alphabet, gave 0 panics. `evaluate_sync` runs in `spawn_blocking` inside `catch_unwind`, and a panic or join error maps to Deny. |
| 5 | major | **fixed** (see a, b) | Brace bombs of 2,000 and 20,000 groups → Deny "too large" in 25 ms and 127 ms. A line with 3,000 `cd`s → Deny in ~1 ms. 20 distinct `cd`s → Deny. A 100 KB here-doc → Deny in 7 ms. Two other shapes are still slow (a, b). |
| 6 | major | **fixed** | Every core hard-deny miss from round 1 is now a `HardDeny`. |
| 7 | minor | **fixed** | Every further miss is now a `HardDeny`, including `tee /dev/disk1`, `newfs_*`, `diskutil apfs|cs`, `sudoedit`, the `security` prefixes and `git push` option prefixes. |
| 8 | minor | **fixed** | `grep -R …` and `grep --dereference-recursive …` → Ask. |
| 9 | minor | **fixed** | `tests/policy.rs` now has the inputs of findings 1–8 in its tables, including the `\r#` lines, `date -uf.npmrc`, and the `/.vol`, firmlink and hard-link gate test (`macos_path_aliases_and_hard_links_reach_nothing`). `tee /dev/disk1` is now expected as `Hard(DW)`, and the fuzz alphabet now has a bare `-`, `\r`, `\u{1}`, `\u{2028}` and `-é`. Findings a–f below are not in the tables yet. |
| 10 | nit | **fixed** | The `ſ` (long s) and `K` (Kelvin sign) spellings of credential paths and commands → `HardDeny`. `lower()` folds `ſ`, and the gate's identity check also covers APFS case folding. |
| 11 | nit | **fixed** | `resolve` on a question row (no binding) → `Err(Unknown)`. Answering it afterwards → `Ok`. |
| 12 | nit | **fixed** (CR-06) | An exact rule for `rm -rf build` doesn't cover `rm -rf build ~/Documents` (Ask) or `X=1 rm -rf build` (Ask). "Always…" on a plain command is still offered (`rememberable = true`) and stores `CommandExact`. |

## Findings (new)

a. **[major] An `echo`/`printf` text piped into a shell is re-lexed for every (echo, shell) pair in the pipeline, before any size cap: quadratic in time and memory.**
   - Where: `crates/atrium-core/src/policy/shell.rs:690-711`. For each shell unit `j`, every earlier `echo`/`printf` in the same pipeline is lexed again, and the results pile up in `extra_units`. This happens inside `lex`, and `deny.rs:99`/`:169` (`MAX_UNITS` 512) only look at the result afterwards. The `MAX_BYTES` check at `deny.rs:92` passes, because the line is small.
   - Input: `"echo x|".repeat(n) + &"sh|".repeat(n) + "sh"`.
   - Observed:
     - Engine: n = 1,000 (10 KB) → Deny in 327 ms; n = 3,000 (30 KB) → Deny in 3.6 s.
     - `shell::parse_command` alone, measured with `/usr/bin/time -l`: n = 1,000 → 352 ms and 491 MB peak RSS; n = 3,000 → 3.7 s and 1.97 GB.
     - Gate: n = 1,000 → 666 ms (the gate lexes twice).
     - Memory grows with n². At the 128 KiB cap (n ≈ 13,000) the extrapolation is over 20 GB, so the process runs out of memory before it can return Deny.
   - Problem: one agent command can stall a blocking-pool thread for seconds, or kill the daemon. That is the round-1 finding-5 class again, in code that was added by the fix for finding 7. `catch_unwind` doesn't help with OOM.
   - Fix:
     - Check `MAX_BYTES` on the raw line before lexing anything (it is checked first today, but `lex` must not create more than the cap allows).
     - Collect the `echo`/`printf` texts of a pipeline once, and re-lex each text once per pipeline, not once per shell unit (any shell reading stdin is enough to mark the pipeline).
     - Stop and flag overflow as soon as `out.units.len() + extra_units.len()` exceeds `MAX_UNITS`, and turn that flag into the engine's "too large" Deny.
     - Add `echo x|…|sh|…|sh` with n = 3,000 to the performance test, with a time bound.

b. **[minor] A long run of `{` is quadratic in the lexer's brace look-ahead.**
   - Where: `shell.rs:502-520`. Each `{` scans forward to its matching `}` or to the end of the word, so a word of N `{` costs O(N²).
   - Input: `ls ` followed by 100,000 `{` (and by 130,000 `{`, which is under the 128 KiB cap).
   - Observed: Allow in 4.95 s at 100,000, and Allow in 8.3 s at 130,000, from the engine. The gate takes about twice as long.
   - Problem: the time, not the decision. Allow is the correct decision: bash and zsh don't brace-expand a group with no `,` or `..`, so this is one literal word that `ls` reads (see the worker's note below). A legitimate line can't trigger it, but an agent can stall a blocking-pool thread for about 15 s per call.
   - Fix: match braces in one pass. Pre-compute each `{`'s matching `}` and its "has `,`/`..` at depth 1" bit with a stack over the word, then look them up in the main loop. Or stop scanning after a fixed number of characters and treat the word as having brace expansion (over-resolving is safe).

c. **[minor] "Every non-reader is a writer" applies to every path, not just Atrium's protected paths, and it hard-denies ordinary reads that D-050/D-051 allow.**
   - Where: `crates/atrium-core/src/policy/words.rs:158-170` (`is_writer` = not in `READERS`). It is used for credential paths and atrium-self alike. Its own doc comment says "for Atrium's protected paths".
   - Input and observed, all a `HardDeny` from the engine:
     - `ssh -F ~/.ssh/config host`, `ssh-keygen -lf ~/.ssh/authorized_keys` and `git diff --no-index ~/.ssh/config x` → `credential-read`. D-050/D-051 allow reading `~/.ssh/config`, `known_hosts` and `authorized_keys`, and round 1 gave Ask or Allow for these.
     - `open /Applications/Atrium.app`, `codesign -dv /Applications/Atrium.app` and `/Applications/Atrium.app/Contents/MacOS/atrium --version` → `atrium-self`.
   - Problem: hard denies can't be overridden by a rule or an answer (D-050), so a user can't `ssh -F` their own config or launch or inspect Atrium from an agent at all. That is a usability regression on a common command, and it teaches users to work around the policy.
   - Fix:
     - Use the reader inversion (`!READERS.contains`) only for `atrium_protected_paths` and Atrium's data root. Keep `is_known_writer` (the old explicit list) for the readable members of `~/.ssh`.
     - Don't count `argv[0]` (running a binary) or `open`/`codesign -d`/`spctl`/`mdls` as a write.
     - Add `ssh -F ~/.ssh/config host` (expected Ask) and `open /Applications/Atrium.app` (expected Ask) to the table.

d. **[minor] `READERS` lists commands that can write, so writes to protected paths through them come back Ask instead of `HardDeny(atrium-self)`.**
   - Where: `words.rs:89-…` (`READERS`).
   - Input and observed, all Ask (not `HardDeny`):
     - `sort -o ~/.config/atrium/config.toml x`
     - `uniq x ~/.config/atrium/config.toml` (the second operand is the output file)
     - `find ~/.config/atrium -delete`
     - `sed -n 'w /h/u/.config/atrium/config.toml' x` (the `w` command inside the script)
     - `xxd -r x ~/.config/atrium/config.toml`
   - Problem: the contract says writes to Atrium's own config and app are always hard-denied. These ask, and a tired user who approves one has rewritten Atrium's config. Ask is a fallback, not a bypass, so this is minor.
   - Fix: drop `sort`, `uniq`, `xxd`, `find`, `awk` and `tree` (which writes with `-o`) from `READERS`, so they fall under "writer". Or keep them and treat their write forms as writes: `sort -o`/`--output`, a second operand for `uniq`, `find -delete`/`-exec`/`-fprint*`, `xxd -r` with two operands, `tree -o`. Do the same for `sed` scripts containing `w`/`W` (or any `sed -e`/script that isn't a plain print), as is already done for `-i`.

e. **[minor] The hard-link check misses `~/.ssh` members when another sensitive root holds 2,000 entries or more.**
   - Where: `crates/atrium-core/src/policy/gate.rs:177-197` (`expand_roots`). It walks depth-first, popping the *last* root first, and stops adding entries after a single global cap (`MAX_ENTRIES` 2000). `.ssh` is the first root, so it is enumerated last.
   - Input: a gate home with 2,100 files under a Chrome profile (a credential tree) and a pre-existing hard link `Documents/k1` → `.ssh/id_ed25519`. The check is `cat Documents/k1`.
   - Observed: `GateDecision::Allow` in 9 ms. The same link in a small home → Deny. Real browser profiles easily hold more than 2,000 entries.
   - Problem: an existing hard link to a private key is read without asking. The agent can't make that link itself (`ln` naming a credential is hard-denied), so it needs a link already on disk (put there by an earlier tool, a restore or a sync), which is why this is minor. The walk also costs about 8 ms of `readdir` on every check.
   - Fix:
     - Enumerate `~/.ssh` and the single credential files first, and give each root its own cap, so one big tree can't starve the others.
     - Better: don't enumerate. For each resolved path whose `st_nlink > 1`, compare its identity against the credential files and against the members of `~/.ssh` only (a small directory). For the large trees, compare the identity of the path's ancestors against the tree roots, which `alias_fix` already does.
     - Cache the walk per check, not per path.

f. **[minor] A here-doc or here-string fed to a shell that has a positional argument or a non-flag option is not re-lexed.**
   - Where: `shell.rs:672-679` (`takes_stdin_code`: a shell counts only if every argument starts with `-`).
   - Input: `bash /dev/stdin <<EOF\nrm -rf ~\nEOF`, `bash /dev/stdin <<< 'rm -rf ~'` and `bash -o pipefail <<< 'rm -rf ~'`.
   - Observed: Ask for all three. `bash --norc <<< 'rm -rf ~'` → `HardDeny(recursive-delete-outside-cwd)`.
   - Problem: the hard-deny list doesn't see the body, so a user who answers the prompt runs `rm -rf ~`, which should be unoverridable. It asks, so it isn't a silent bypass.
   - Fix:
     - Treat `/dev/stdin`, `/dev/fd/0` and `-` as "reads stdin" (as `reads_stdin_as_code` in `deny.rs:527` already does for `source`).
     - Skip option values (`-o X`, `-O X`, `+o X`) before deciding a shell has a script argument.
     - Simplest: re-lex the bodies whenever *any* shell or interpreter unit is present. Over-analysis is safe.

g. **[nit] Short-flag clusters push suffixes for the first 64 letters only.**
   - Where: `words.rs:337` (`for k in 1..=cs.len().min(64)`).
   - Input: `ssh -` + `v`×70 + `i.ssh/id_ed25519 host`.
   - Observed: Ask (not `HardDeny(credential-read)`). `ssh -vvvi.ssh/id_ed25519 host` → `HardDeny`.
   - Problem: it asks, so this isn't a bypass, but the unoverridable credential-read becomes overridable.
   - Fix: when a cluster is longer than 64 letters, add a reason ("flag cluster too long"), so the line asks with a reason and can't be remembered. Or push only the suffixes after the letters that take a value (`O(n)` and enough).

h. **[nit] CommandExact edge cases.**
   - A remembered exact rule has no path scope, so it isn't tied to a working directory. An "Always" given for `rm -rf build` in the project also allows `rm -rf build` with `cwd: Some("Documents")`, which deletes a different directory. The global and Home scopes make this wider. Consider setting `path_scope` to the cwd when remembering a command with relative-path operands, or say in the approval UI that the rule applies in any folder.
   - An editor-made `CommandExact { argv: [] }` allows lines that are only redirects (`>/dev/null`). Harmless, but the rules editor should reject an empty argv.
   - A deny rule `CommandExact` doesn't match the same command with extra arguments (`rm -rf build x` → Ask). That is the defined behaviour of an exact rule. The rules editor should point to `CommandPrefix` for deny rules.

Out of scope, for M1-09: a bidi override (`ls\u{202e}…`) gives Ask with the reason "control character", which is correct here. The approval UI must show such characters visibly, or the user approves a line that reads differently from what runs.

## The worker's notes

- **"A 100,000-`{` line is allowed (one word)."** The *decision* is correct: neither bash nor zsh brace-expands `{{{…` with no `,` or `..`, so it is one literal word, and `ls` only reads it. The problem is the cost: 5 s at 100,000, and about 8 s under the 128 KiB cap (×2 in the gate). See finding b. It's minor, because only an agent that is already misbehaving would send it, but the fix is small.
- **"`ssh -F ~/.ssh/config` is now blocked."** It's a problem, and wider than the note says. It is an unoverridable `HardDeny(credential-read)` for a read that D-050/D-051 explicitly allow. The same cause hard-denies `ssh-keygen -lf ~/.ssh/authorized_keys`, `git diff --no-index ~/.ssh/config`, and launching or inspecting `/Applications/Atrium.app`. See finding c. It should be fixed in this task, not accepted.

## CR-06 (D-054)
CR-06 is sound, and it closes finding 12. "Always…" now stores `CommandExact` of the whole argv. It's offered only for one simple command with no assignments, harmless redirects only, and no `$`, globs or braces. An allow rule matches only a simple line whose `seg.argv` is equal, with no assignments and harmless redirects. Extra arguments, an added `X=1`, a second segment, and a wrapper (`env rm …`, `sh -c 'rm …'`) all ask again. A deny exact rule does match through wrappers (`eff` or `seg.argv`). `CommandPrefix` is now editor-only. `date -f`/`--file` is forbidden, and `date -f .npmrc` is a credential-read hard deny anyway. Remaining edges are in finding h (no cwd binding, an empty argv).

## Checked and held
- **Panics:** `catch_unwind` plus `spawn_blocking` around `evaluate_sync` (in `check` and `redeem`). There were no panics in 30,000 fuzz lines. An error maps to Deny, never to Allow.
- **Caps:** every overflow path I hit ends in Deny ("too large to check"): `MAX_BYTES`, `MAX_UNITS`, `MAX_USES`, `MAX_PATHS`, `MAX_CANDIDATES`, `MAX_BRACE_RESULTS`. That holds for the `Paths`, `Mcp` and `Unknown` subjects too. No cap turned into Allow.
- **Large legitimate lines:** a 94 KB line of 4,000 deep words gave Allow through the gate in 327 ms, which is acceptable.
- **The identity check:**
  - Hard links to credential files that are roots themselves are caught.
  - Every `/.vol` form tried is denied: `/.vol`, `/.vol/`, `/.vol/<dev>/<ino>`, and `/.vol` reached through `..`. `/.VOL` isn't readable on this Mac.
  - Lowercase and mixed-case firmlinks are caught.
  - A firmlink in `cwd` is caught.
  - A protected-path write through the firmlink is caught.
  - Symlinks are followed by `metadata`, so a symlink to a credential is caught too.
- **Short-flag clusters:** `-uf.npmrc`, `-vvvi.ssh/id_ed25519` and `-Ff~/.netrc` are resolved for every suffix up to 64 letters (see g).
- **Here-docs and here-strings:** bodies fed to `bash`/`sh`/`zsh` with flag-only arguments, and to `eval`/`source`/`.`, are re-lexed and hard-denied (see f for the gaps).
- **`grep` and `rg` search roots, the `date -f` guard, and `~user`/`~+`/`~-` expansion** are correct.
- **Ledger:** single-use redeem holds. `resolve` on a row with no binding → `Unknown`.

## Summary
The round-2 fixes are thorough. The three round-1 bypasses are closed:
- `\r` is now a word character;
- path aliases are compared by `(st_dev, st_ino)`, with `/.vol` refused and firmlinks stripped;
- every cluster suffix is resolved.

The gate can no longer panic, and every size cap fails closed. The remaining issues are mostly cost and precision, not bypasses.

What must change before the gate:
- The `echo … | sh` fan-out is re-lexed quadratically before the caps run, and can take seconds and gigabytes from a 30 KB line (a, major).
- Treating every non-reader as a writer now hard-denies ordinary, allowed reads of `~/.ssh/config` and launches of Atrium (c).
- A handful of "readers" can write to Atrium's protected config, which then only asks (d).

What can follow later:
- the quadratic `{` look-ahead (b);
- a hard-link gap when a big browser profile starves the identity walk (e);
- here-docs fed to `bash /dev/stdin` (f);
- two nits (g, h).

**Approve with fixes**: fix a, c and d, add their inputs to the tests, and this is ready for the gate.
