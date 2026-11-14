# Review: M1-13-adversarial
Reviewer: Claude Fable 5.2 (read-only) · 2026-11-13

## Verdict
**rework**: part 1 made a private key that is symlinked into `~/.ssh` readable without asking, part 2's fix for finding 2 still reaches Allow on a credential with the same two prefix rules, and both parts turned hard denies into Allow or ordinary lines into hard denies.

How I tested. About 450 inputs in three scratch test files (`crates/atrium-core/tests/review_scratch_{a,b,c}.rs`, deleted), run at the branch tip `66ce368` and, for comparison, on an export of `dev` (`git archive dev`, built in a scratch folder, deleted). "Proved" below means a scratch test printed that verdict; "dev:" is the verdict the same test printed on `dev`.
- Engine inputs: `DefaultPolicy::evaluate`, home `/h/u`, cwd `/h/u`, nothing on disk (the branch's `Env::home()`). Four settings: **home** (preset Home, no rules), **prefix rules** (global `CommandPrefix` allow rules for `cd`, `cat`, `pushd`, `popd`, `ls`), **any** (one global `RuleMatcher::Any` allow rule for Execute), **full** (preset FullAccess).
- Gate inputs: `SessionGate` over the branch's `Tmp` (a temp folder with a fake HOME), the same settings without the prefix rules.
- The branch's own `adversarial_*`, `policy` and `ledger` tests of `atrium-core` pass here (13+5 ignored, 9+2, 2+1, 14, 44, 20).
- **Not reviewed:** `crates/atrium-daemon/tests/adversarial_recovery.rs`, `crates/atrium-host/tests/adversarial_supervise.rs`, and `adversarial_socket.rs` apart from the flood test. I ran no daemon or host test. See "Test gaps".

## Findings
1. [blocker] **Regression (part 1): a private key kept elsewhere and symlinked into `~/.ssh` is now read without asking**
   - Where: `crates/atrium-core/src/policy/gate.rs:257-261` (`evaluate_sync`: every `~/.ssh` member is identified with `identity_nofollow`, `gate.rs:144-149`). A member that is a symlink to a file gets the link's own inode, which no resolved path ever has, so the key's real path is no longer mapped onto `~/.ssh/<name>`.
   - Input: home with `keys/id_ed25519` (a file) and `.ssh/id_ed25519 -> ~/keys/id_ed25519`; also `.ssh/id_rsa -> ../dotfiles/ssh/id_rsa` and `.ssh/work -> ~/vault/work` (a folder holding `id_work`). Preset Home, no rules, cwd `~`. `cat keys/id_ed25519`, `head -c 64 keys/id_ed25519`, `cat dotfiles/ssh/id_rsa`, `cat vault/work/id_work`, and the `Read` tool on each.
   - Result: **Allow** (home, any, full). dev: Deny `credential-read` for every one. Required: hard deny (§9 hard-deny list, "every private key in ~/.ssh"; `redeem` and the identity map exist so that another name for a key is still the key). Proved at both commits. The branch's test asserts the weak behaviour: `adversarial_policy.rs:218,228` makes `.ssh/file-link -> proj/a.txt` and requires `cat proj/a.txt` to be allowed.
   - Fix: follow the link when its target is a regular file (`identity`), and skip only links whose target is a directory (M1-03 finding 5 was about a folder). For a linked folder of keys, decide separately; today `vault/work/id_work` is open either way only on this branch.

2. [blocker] **Finding 2 (S2) is not closed: a failing `cd` still reaches Allow on a credential with the `cd` and `cat` prefix rules**
   - Where: `crates/atrium-core/src/policy/words.rs:527-548` (`apply_cd`). A relative target is resolved against three directories only (`current`, which assumes every earlier `cd` worked, `start`, and `prev`); `cd -` (`words.rs:526`) uses the tracker's `prev`, which is wrong after a failed `cd`.
   - Input, rules `CommandPrefix[cd]` + `CommandPrefix[cat]`; `n1`, `n2`, `nope` don't exist:
     - `cd .config; cd n1; cd n2; cd gh; cat hosts.yml` (the shell is in `~/.config/gh`; the candidates are `~/.config/n1/n2/gh`, `~/gh`, `~/.config/n1/gh`)
     - `cd /h; cd n1; cd n2; cd u/.ssh; cat id_rsa`
     - `cd .config; cd nope; cd /tmp; cd -; cd gh; cat hosts.yml`
     - the same joined by `&&`/`||` or newlines; inside `( … )` it asks (any/full: Allow).
   - Result: **Allow** with the prefix rules, any and full; Ask in home. Required: HardDeny `credential-read` (§5 "hard deny wins", §9 "checked for hard deny against every cwd it could be"; must-fix S2). Proved. The single failing `cd` of the handoff is fixed (dev: Allow, now HardDeny).
   - Fix: (a) in `apply_cd`, resolve a relative target against every candidate of the current state, and make `cd -` add every directory seen; (b) make an uncertain cwd something rules can't cover: in `engine.rs:129` (`allow_covers`, `simple`) and `engine.rs:584` (`Home::evaluate`), a line with `a.cwd_unknown` or a unit with more than one cwd candidate asks. The handoff says (b) exists ("never auto-allow (`CwdUncertainty` / `Ask`)"); nothing in `engine.rs` reads `cwd_unknown` or the number of candidates.

3. [blocker] **Finding 3's inputs reach Allow with ordinary prefix rules (the handoff rates it Low, must-fix S6 minor)**
   - Where: `words.rs:509-514` (`popd`: state unknown, `current = None`) and `words.rs:534-545` (the next relative `cd` resolves against `start` and `prev` only); `engine.rs:129` ignores `Analysis::cwd_unknown`.
   - Input: `pushd .config; pushd /tmp; pushd /var; popd; popd; cd gh; cat hosts.yml`, rules `CommandPrefix` for `pushd`, `popd`, `cd`, `cat`. Also `cd -; cat id_rsa` with `cd` + `cat` (the shell goes to `$OLDPWD`, which the line doesn't name).
   - Result: **Allow** with the prefix rules, any, full; Ask in home. Required: §9 "any other directory change makes the cwd unknown, so the line asks". Proved; dev gives the same, so it is not a regression, but it is S2's class.
   - Fix: 2(b) closes the Allow. For the hard deny: after `popd`, a relative `cd` resolves against every directory seen.

4. [blocker] **Regression (part 2): a `cd` through a symlink un-aligns the gate's path list, and a write to Atrium's config or `~/.ssh/authorized_keys` through a link is no longer hard-denied**
   - Where: `words.rs:558-562` and `words.rs:598` dedupe candidates by resolved value, and `words.rs:572` now keeps every directory seen as a candidate. The gate's walk (filesystem) and the engine's walk (lexical) then have different numbers of paths, `deny.rs:194-203` leaves `aligned = None`, and the fallback `deny.rs:859-865` checks the gate's paths as `Role::Read` only, so write-only protections (`atrium_protected_paths`, `~/.ssh/config`, `authorized_keys`) are not seen.
   - Input: home with `lnk -> .config/atrium`, `s -> .ssh`, `self -> ~`, a file `evil`. `cd lnk/..; cp evil ~/lnk/config.toml`; `cd lnk/..; tee ~/lnk/config.toml`; `cd lnk/..; cp ~/evil ~/s/authorized_keys`; `cd lnk/..; tee ~/s/config`; `cd self; cp evil ~/s/authorized_keys`; `cd self && tee ~/lnk/config.toml`.
   - Result: Ask in home, **Allow** with any and full. dev: Deny (`atrium-self` / `credential-read`) in all three. Without the `cd` the branch also denies. Required: hard deny (§9 "Atrium's own paths … config, socket, service units and app, never written"; "writing ~/.ssh/config or authorized_keys"). Proved at both commits. Needs links that already exist; an `ln -s` asks.
   - Fix: when the lists don't line up, check every gate path as a write (`Role::Write`) in `deny.rs:861`, or stop deduping by value in `walk`/`apply_cd` so both walks always produce the same shape. The first is one line and also covers dev's `cd $X` variant.

5. [major] **Regression (part 1): a here-doc fed to a shell whose name is built with a substitution is no longer read**
   - Where: `crates/atrium-core/src/policy/shell.rs:741-751` (`clearly_text` now tests `!u.proc_sub` where dev tested `u.subs.is_empty()`), with `shell.rs:1042-1045` (`is_shell` only recognises a command word that *starts* with `$` or a backtick).
   - Input: `s$(echo h) <<EOF\nrm -rf ~\nEOF`, `ba$(printf sh) <<EOF\n…`, ``s`echo h` <<EOF\n…``, `ssh host $(echo bash) <<EOF\n…`, `nsenter -t 1 $(echo sh) <<EOF\n…`.
   - Result: Ask in home, **Allow** with any and full. dev: HardDeny `recursive-delete-outside-cwd`. Proved at both commits. The same hole for payloads: `s$(echo h) -c 'rm -rf ~'` and `s${E}h -c 'rm -rf ~'` (not read on dev either).
   - Fix: `is_shell`: a command word that *contains* `$` or a backtick is unknown, so assume a shell. The round 6 false hard denies (`cat > notes-$(date).md <<EOF`) stay fixed, since there the command word is `cat`.

6. [major] **Regression (part 2): after a `cd` that works, ordinary lines are hard-denied (can't be overridden)**
   - Where: `words.rs:568-573` (`CwdState::Known(seen.clone())`: every directory seen, the start included, stays a candidate after any literal `cd`) and `words.rs:590-603` (`walk`: a word starting with `..` is also resolved, with its `..` stripped, against `start` and `home`, a path no cwd could give).
   - Input → result (all four settings), dev in brackets:
     - `cd proj && cat .npmrc`, `cd ~/proj && cat .npmrc`, `cd /tmp && cat .npmrc`, `cd proj && cat .netrc`, `cd infra && cat .kube/config`, `cd app && cat .docker/config.json`, `cd proj && cat .claude.json`, `cd proj && grep registry .npmrc`, `cd proj && ls -la .gnupg` → HardDeny `credential-read` [Ask; Allow with `cd`+`cat` rules]
     - `cd packages/app && cat ../.npmrc`, `cd a/b/c && cat ../../.npmrc` → HardDeny `credential-read` [Ask]
     - `cd build && rm -rf *`, `cd dist && rm -rf ./*`, `cd proj && rm -rf ../proj-old`, `cd build && find . -delete`, `cd proj && find . -name '*.o' -delete` → HardDeny `recursive-delete-outside-cwd` [Ask]
   - Required: the brief, "nothing ordinary may become a hard deny". Proved at both commits. The branch's table has only `cd proj && cat README.md` and `cd proj && rm -rf build`.
   - Fix: record how a unit is joined to the next. After `cd X && …` the rest of the `&&` chain only runs if the `cd` worked, so the state there is the target alone, as on dev; the earlier directories come back as candidates after `;`, `||` or a newline (where 2(b) makes the line ask). Delete the `..` special case in `walk`; with 2(a) the real candidates cover it.

7. [major] **Finding 8's fix reads `echo`/`printf` text only when it has no escapes or conversions the decoder doesn't model**
   - Where: `shell.rs:1074-1101` (`unescape_printf_to`: only `\n \t \r \\`), `shell.rs:1187` (`echo` is decoded only if the text contains `\n`), `shell.rs:1103-1165` (`format_printf`: precision, `%c`, `*` and a conversion without an argument are not applied), `shell.rs:756-770` (only `echo`/`printf`, only when the shell unit itself is in the pipeline).
   - Input, each Ask in home and **Allow** with any and full (dev: the same; the fix didn't reach them): `echo -e 'rm -rf\t~' | sh`, `echo 'rm -rf\t~' | sh` (dash, zsh), `echo -e 'rm -rf \x7e' | sh`, `echo -e 'cat .aws/cr\145dentials' | sh`, `printf 'rm -rf \176' | sh`, `printf '%b' 'rm -rf \0176' | sh`, `printf 'cat .aws/credentials%s' | sh`, `printf 'cat .aws/credentials%.0s' x | sh`, `printf 'cat .aws/%.4sentials' credXXX | sh`, `printf '%*s%s' 0 'cat .aws/cred' entials | sh`, `printf 'rm -rf ~\n' | (sh)`, `yes 'rm -rf ~' | sh`, `sh -c "$(echo rm -rf ~)"`, `eval $(echo rm -rf ~)`, `bash -c "$(cat <<'EOF'\nrm -rf ~\nEOF\n)"`.
   - Required: must-fix S4 ("text piped into a shell must be read as code"). Proved. The inputs the handoff names are fixed (dev Ask, now HardDeny), and nothing got weaker.
   - Fix: decode `\xHH`, `\NNN`, `\0NNN`, `\e`, `\c` for `echo` (always, not only when `\n` is present) and `printf`; honour precision and `%c`, consume an argument for `*`, print nothing for a conversion with no argument; follow a pipeline into `( … )`/`{ …; }`; treat the text of `echo`/`printf`/`cat <<EOF` inside a `$(…)` that is the payload of `sh -c`/`eval` the same way. Text passed through `tr`, `sed` or `base64 -d` can't be read statically and should be stated as a limit.

8. [major] **The handoff's "Second part" describes code that is not on the branch**
   - Where: `docs/agents/handoffs/M1-13-adversarial.md:62-77`.
   - It names `cwd.dirs`, `cwd.seen`, `cwd.all()`, `cwd.only()`, `CwdUncertainty`, `in_subshell`, `deny::command_reason`, `words::is_cd`, `is_rm_Unit`, `writes_home_root`, `is_echo_flag`, `echo_text`, `check_inner`, `w.quoted`. None exists in `crates/atrium-core/src/policy/**`. Part 2's whole change to `deny.rs` is one line (`deny.rs:777`); there is no skip of `cd` units in any delete check, so "can anything hide behind that skip" has no object. "Never auto-allow on an uncertain working directory" is false (finding 2).
   - Fix: rewrite the section from `git diff 1989194..HEAD`.

9. [minor] **Part 1's wrapper fixes leave the long-option and "command's own `-c`" forms unread**
   - Where: `shell.rs:877-894` (`skip`: a long option's separate value is taken for the command), `shell.rs:914-944` (`flock`/`script`: the first `-c` anywhere in the line, the wrapped command's own included, replaces the whole command), `shell.rs:959-996` (`env`).
   - Input, each Ask in home and Allow with any and full (proved): `timeout --signal KILL 5 rm -rf ~`, `timeout --kill-after 3 5 rm -rf ~`, `nice --adjustment 5 rm -rf ~`, `ionice --class 3 rm -rf ~`, `stdbuf --output L rm -rf ~`, `arch -arch x86_64 rm -rf ~` (the documented macOS form), `flock /tmp/l sudo sh -c 'id'` and `script -q /dev/null sudo sh -c id` (the `sudo` disappears), `flock /tmp/l git -c x=y push -f origin main` (the force push disappears), `env - rm -rf ~`, `env -iS 'rm -rf ~'`, `env -a foo rm -rf ~`, `env -S 'cat .aws/credentials'` (the spliced words are checked as a command but not as paths), `env -C .aws cat credentials`, `xargs --max-args 1 rm -rf ~`, `time -f %e rm -rf ~`.
   - Fix: give `skip` each wrapper's value-taking long options; for `flock`/`script` look for `-c` only among the wrapper's own options (before the file operand); treat `env -S` text as a payload (lex it as its own unit) and `env -C DIR` as a `cd`.

10. [minor] **Part 1: a command word starting with `$` counts as a shell, which hard-denies ordinary pipes**
    - Where: `shell.rs:1042-1045` (`is_shell`), used by `deny.rs:523-582` and `shell.rs:717-722`.
    - Input → result: `curl -s https://example.com/notes.txt | $PAGER` → HardDeny `pipe-to-shell`; `echo "sudo apt install foo" | $CLIP` → HardDeny `privilege-escalation`. dev: Ask for both. Proved at both commits.
    - Fix: keep the assumption for here-docs and `-c` payloads, but for `pipe-to-shell` and piped `echo` text require a known shell name or an explicit `-s`/`-`; or accept the cost and add both lines to the table as wanted hard denies.

11. [minor] **Part 1's hard-link rule covers only a file that is named**
    - Where: `gate.rs:272-278` (checks `resolved` paths under Home that are files).
    - Input: `proj/k2` is a hard link to `~/.gnupg/private-keys-v1.d/k.key`. `rg x proj`, `grep -r x proj`, the Grep tool on `proj` → **Allow** in home (the search reads the key); `cat proj/*` → Allow with any/full. A hard link kept outside Home (`cat <tmp>/outside/innocent.txt`, also through a link `proj/out-link/…`) → Allow in home. Proved; dev allows all of these and `cat proj/k2` as well, so the direct case is an improvement.
    - Fix: for a recursive search root under Home, ask when the tree holds a multiply-linked file (bounded walk), or state the limit; drop the `starts_with(home)` test or state it.

12. [minor] **Regression (part 2): seven nested `cd`s are refused outright**
    - Where: `words.rs:534-545` (up to three targets per `cd`) with `words.rs:563` (`MAX_CANDIDATES` 16 → `overflow` → `engine.rs:605` `Deny`).
    - Input: `cd a && cd b && cd c && cd d && cd e && cd f && cd g && ls` → `Deny("too large to check")` in every setting; dev: Ask (Allow with rules). Six `cd`s still ask. Proved at both commits.
    - Fix: on too many candidates fall back to the unknown state and ask. Fix 2(a) grows the set faster, so do this with it.

13. [minor] **Finding 7's class: shell options that take a value still hide the payload**
    - Where: `shell.rs:1242-1250` (`payload_of`).
    - Input, Ask in home and Allow with any/full (proved; same on dev): `bash --rcfile /dev/null -c 'rm -rf ~'`, `bash --init-file /dev/null -c 'rm -rf ~'`, `zsh --emulate sh -c 'rm -rf ~'`, `fish -C true -c 'rm -rf ~'`, `fish --init-command='rm -rf ~'`.
    - Fix: skip the value of `--rcfile`, `--init-file`, `--emulate`, `-C`/`--init-command`; read `--init-command=` as a payload. The `--` fix itself is right.

14. [minor] **Part 1's stdin readers: `crontab -` is read as a shell script, which a crontab isn't**
    - Where: `shell.rs:1049-1071` (`reads_stdin_as_script`).
    - Input, Ask in home and Allow with any/full (proved): `echo '* * * * * rm -rf ~' | crontab -`, `echo '@reboot rm -rf ~' | crontab -`, `crontab - <<EOF\n* * * * * rm -rf ~\nEOF`; also `make -sf - <<EOF…`, `make -f /dev/stdin <<EOF…`, `chroot --userspec u:g /mnt <<EOF…`, `at now -f x <<EOF…`. The branch's test uses `echo 'rm -rf ~' | crontab -`, which no cron would run.
    - Fix: for `crontab`, drop the five schedule fields (or `@word`) of each line before lexing; accept `-f` inside a flag cluster and `/dev/stdin` for `make`.

## Suspected, not shown
- A `cd` in one tool call and a relative read in the next (`cd .ssh`, then `cat id_rsa`, both auto-allowed or rule-allowed) if the provider's shell keeps its cwd between calls and the adapter doesn't pass it in `ToolSubject::Command.cwd`. It depends on the adapters (M1-07/08), which I did not read.
- When the gate's and the engine's lists differ per word but have the same total, `deny.rs:196-202` would pair paths with the wrong words and roles. I found no input with equal totals.
- A home reached through a symlinked parent (`/home -> /var/home`): `cd ~` then makes the two walks differ as in finding 4. Not built.
- Finding 12's second half (`sessions.ended_ms` set by `Signal::Recovered`): I read `upkeep.rs`, not `store/writer.rs`.

## Tried and held
- `cd` forms, one failing `cd`: `builtin cd`, `command cd`, `cd -L/-P/--`, `cd a b`, bare `cd`, quoted and escaped targets, `~`, `$HOME`, `chdir`, `pushd DIR`, `if cd …; then`, `{ cd …; }`, a function that is called, `cd … &`, `cd` in a pipeline, `bash -c 'cd …'` followed by a read, `eval cd …` → HardDeny.
- Deletes after a `cd`: `cd nope; rm -rf .`, `cd proj; cd nope; rm -rf ..`, `cd a/b; cd n1; cd n2; rm -rf ../..`, `cd nope; find . -delete` → HardDeny.
- `sh -c`: `--` once, quoted `--`, `-ec`, `-c -e`, `-c -c`, `+o posix`, `-o pipefail`, `-O extglob`, `--norc`, `-lc`, `env sh -c`, `ksh -c '…' --`, `eval --` → HardDeny. `sh -c -- -- 'x'` and `sh -- -c 'x'` ask, correctly (the shell doesn't run `x`).
- `echo`/`printf` to a shell: `-n -e`, `-ne`, quoting of every word, `/bin/echo`, `command echo`, `env echo`, `|&`, a longer pipeline on either side, `tee | sh`, `cat | sh`, `xargs sh -c`, `sh -s`, `bash /dev/stdin`, `source /dev/stdin`, `<<<`, `%s` with several arguments, `%c`, `printf --`, `printf -v` → HardDeny.
- Ordinary lines not hard-denied: `cd build && make`, `cd proj && rm -rf node_modules`, `echo done | cat`, `printf '%s\n' x | sort`, `$CC -c foo.c -o foo.o`, `echo y | sh install.sh`, `git log | $PAGER`.
- No verdict went from Deny or Ask to something weaker in the roughly 200 engine inputs of the first scratch file compared on both commits, other than findings 1, 4 and 5 (found by reading the hunks, then proved).
- New shell names (`mksh`, `yash`, `rbash`, `posh`, `elvish`, `xonsh`): no ordinary tool I know shares one.
- `docker exec -i $(docker ps -q) sh <<EOF…`: HardDeny on dev, Ask now. That is round 6 finding 1 as asked.

## Test gaps
- **Mirrors the code:** `leftover_symlink_in_ssh_to_an_ordinary_folder` (`adversarial_policy.rs:211`) requires the regression of finding 1; `leftover_wrappers_shells_and_stdin_readers` uses a crontab body no cron accepts (finding 14) and only the short forms of each wrapper flag (finding 9); `finding_2_with_plain_prefix_rules_a_failing_cd_reaches_allow` has one failing `cd` (a second one survives, finding 2); `findings_1_and_2_table` and `ordinary_commands_are_not_hard_denied` have no `cd` followed by a dot-file, a glob delete or `..` (finding 6); `leftover_hard_linked_files_ask` names every file (finding 11).
- **The un-ignored tests are real:** on dev `cd nope; cat .ssh/id_rsa` is Allow with the prefix rules, `(cd .ssh && cat id_rsa)`, `sh -c -- 'rm -rf ~'` and `echo rm -rf ~ | sh` ask; all are HardDeny now.
- **No gate test with a `cd`** through a link, so finding 4 had nothing to fail.
- **`flooding_unread_list_threads_past_8_mib_control_lane_closes_connection`** (`adversarial_socket.rs:324`), my reading of the flake: a racy test, made unavoidable by finding 14. The test sends about 12 `ListThreads`, sleeps 200 ms, then reads until end of stream. If the daemon has not yet queued more than 8 MiB when reading starts (a debug build serialising a 1 MiB answer per command, other tests running), the client drains as fast as the daemon produces, the lane never passes the limit, all answers arrive, the connection stays open and the next `read()` waits out `soon` (20 s). That is the reported panic at `tests/common/mod.rs:42`, not the "must close before delivering" assertion. A test that doesn't read can't see the close sooner than `WRITE_STALL` (30 s, longer than `soon`): `Outbox::push_control` (`conn.rs:141-143`) sets `closed` without the wake-ups of `Conn::close` (`conn.rs:191-204`), and `write_loop` (`conn.rs:505`) waits on `writable()` alone. Check: on timeout, print `received_responses`; if it equals `needed`, it is the race. Not run.
- Fake HOME and temp folders: the three `atrium-core` adversarial files use `Env::home()` (nothing on disk) or `Tmp` under `std::env::temp_dir()`; none reads the real `~/.ssh`. Daemon and host files not checked for this.

## The open findings (3, 4, 5, 9, 10, 11, 12, 13, 14): real? severity? before the gate?
- **3** (`pushd`/`popd`/`CDPATH`): real, and worse than rated: Allow with ordinary prefix rules (finding 3 above). **Before the gate**, with S2.
- **4** (ANSI-C `$'…'`): real. Minor: asks in home, and the lexer flags the expansion so no prefix rule covers it. After the gate is acceptable; decoding `$'…'` is small.
- **5** (recursive search of a folder holding a credential): real, and wider than the test: `cp -r .aws /tmp/x`, `tar czf x.tgz .aws`, `find .aws -type f -exec cat {} +`, `zip -r x.zip .config/gh`, `rsync -a .kube host:` all ask in home and are Allow with any/full (proved). Minor as rated; fix with `Sensitive::is_or_contains` for any non-listing command. After the gate.
- **6** (CR-12): the owner's. Stores I know that the list in CR-12 doesn't name (from memory, not checked against each tool's current version): `~/.config/hub`, `~/.config/glab-cli/config.yml`, `~/.gitconfig` with `url.*.insteadOf` tokens, `~/.config/rclone/rclone.conf`, `~/.boto`, `~/.s3cfg`, `~/.config/doctl/config.yaml`, `~/.config/hcloud/cli.toml`, `~/.fly/config.yml`, `~/.config/containers/auth.json`, `~/.m2/settings.xml`, `~/.gradle/gradle.properties`, `~/.composer/auth.json`, `~/.config/pip/pip.conf`, `~/.netlify`/`~/.vercel` auth files, `~/.wrangler/config`, `~/.config/sops/age/keys.txt`, `~/.age`, `~/.password-store`, `~/.local/share/keyrings`, `~/.mysql_history`/`~/.psql_history`, `~/.my.cnf`, `~/.config/Code/User/globalStorage` tokens, `~/.config/1Password`, `~/.bitwarden*`.
- **9** (process substitution feeding a shell): real, minor, same class as finding 7 above. After the gate, as a stated limit, if S4 is fixed for the pipe and `$(…)` forms.
- **10** (a glob reaching a credential only through a link): real, minor. After the gate.
- **11** (a wildcard in the name of a credential tree or Atrium folder): real; must-fix S5 lists it as major and **before the gate**, and part 2's brief left it out. It is still open.
- **12** (recovery: `Recovered` stored before `close_incarnation`): real as far as I read (`upkeep.rs:60-70`: on the retry the thread is closed with `Verified` unless its state is `Starting`, so an earlier `Unverified` is lost). That contradicts §10 ("never shown as stopped"), so I rate it minor-to-major rather than Low. Small fix (keep `Interrupted { cleanup: Unverified }` in the match). **Before the gate.**
- **13** (identity file longer than 64 KiB accepted when its head is valid JSON): real (`server.rs:142`), no security effect (the file is the user's own, mode 0600). Low. After the gate; one line.
- **14** (`write_loop` doesn't watch `closed`): real, Low for security (a peer that stops reading holds a slot and up to 8 MiB for 30 s; only an App or Dev peer gets answers). But the flood test can't be made reliable without it, so fix it **with that test**, before the gate.
