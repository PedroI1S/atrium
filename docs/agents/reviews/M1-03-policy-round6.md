# Review: M1-03-policy (round 6)
Reviewer: Opus 5.5 (read-only; D-053) · 2026-11-09 · fixes at `154d780`

## Verdict
**approve**: the inverted here-doc rule closes the round-5 finding, and every earlier here-doc and here-string hard deny holds. The round-3 cases still ask, and the diff causes no regression toward Allow. What remains is non-blocking:
- one new false hard deny (finding 1);
- some ways a body reaches a shell through a file, a fifo or a file descriptor, which ask (finding 2);
- some pre-existing gaps in the wrapper list (finding 3).

These can go to M1-13.

How I tested: 101 inputs through `DefaultPolicy::evaluate` (home `/h/u`), run at `154d780`, at `7bdee82` and at `6378100`, using scratch builds and a throwaway test, all deleted afterwards. The baseline `cargo test -p atrium-core --test ledger --test policy` gives 20 + 44 passed.

## Re-runs
- **Round-5 inputs, fixed:** these give `HardDeny(recursive-delete-outside-cwd)`: `cat <<EOF | (bash)`, `cat <<EOF > >(bash)` and `while read l; do bash; done <<EOF`. `bash < <(cat <<EOF …)` asks, as it did in every round.
- **Every earlier here-doc and here-string hard deny holds.** All are identical at `154d780` and `7bdee82`:
  - the R1 inputs;
  - `bash <<EOF`, `sh <<<`, `bash /dev/stdin`, `-o pipefail`, `--norc`, `-s`, `<<-`, `2>&1`, `0<<`, `<<EOF bash`;
  - `eval <<<`, `source` and `.` of `/dev/stdin`;
  - the `env`, `exec`, `nice`, `time`, `command`, `nohup`, `timeout`, `stdbuf`, `setsid`, `busybox` and `x=1` prefixes;
  - the `\bash`, `'bash'`, `b''ash` and `/bin/sh` spellings;
  - `if`, subshells and groups, and pipes on either side of a group;
  - several here-docs on a line in every order;
  - nested `sh -c`;
  - `xargs sh -c`;
  - bodies with `sudo`, `curl | sh` and `cat ~/.npmrc`.
  - New this round: `$(which bash) <<EOF` is now a hard deny.
- **Round-3 "must still ask" cases hold:** both `git commit -m` messages that mention Atrium's paths, the `cat > notes.md … bash build.sh` line, the `git commit -F - … sh -c make` line, `ssh -F ~/.ssh/config`, and `open`/`codesign -dv` of the app. The usual commit and PR form, `git commit -m "$(cat <<'EOF' … rm -rf ~ … EOF)"` (with `gh pr create --body` the same), also asks.
- **The accepted cost is confirmed:** `cat <<EOF | tee x` with `rm -rf ~` in the body → hard deny.

## Findings
1. **[minor] Any `$(…)` on the owner command counts as "not text", including a `$(…)` inside an unquoted body. Writing ordinary notes can therefore be hard-denied.**
   - Where: `shell.rs`, the `u.subs.is_empty()` test in `clearly_text`. Command substitutions from an unquoted body are attached to the last unit's `subs`.
   - Input and observed:
     - `cat > notes.md <<EOF\nBuilt $(date)\nsudo make install\nEOF` → `HardDeny(privilege-escalation)`.
     - `cat <<'EOF' > notes-$(date +%F).md …` → `HardDeny(privilege-escalation)`.
     - `git commit -F - <<EOF\nfix $(git rev-parse --short HEAD)\nrm -rf ~ docs\nEOF` → `HardDeny(recursive-delete-outside-cwd)`.
     - All three asked at `7bdee82`. `! cat <<EOF > n.md` is also hard-denied, because `!` is a reserved word.
   - Fix: count only process substitutions (`<(…)`/`>(…)`) here, not `$(…)`. The `$(…)` bodies are already analysed as substitutions in their own right.

2. **[minor, known limit] A body written to a file, a fifo or a file descriptor and run by a later command on the same line asks rather than hard-denies.**
   - Input:
     - `cat > x.sh <<EOF\nrm -rf ~\nEOF` followed by `bash < x.sh`, `. ./x.sh` or `source x.sh`;
     - `mkfifo p; bash < p & cat <<EOF > p …`;
     - `exec 3> >(bash); cat <<EOF >&3 …`.
   - Observed: Ask at `154d780` and at `7bdee82`, and also at `9521e56`, where the owner rule was introduced. All of them were hard-denied at `6378100`, but only by accident: a bare `bash`, `.` or `source` anywhere on the line was enough. I missed this in rounds 4 and 5.
   - Problem: it's the same thing as `cat > x.sh …; sh x.sh`, which asked in every round. Round 3 decided that text written to a file isn't analysed. Nothing reaches Allow.
   - Optional cheap fix: count a body when the owner's output target (a file, fifo or fd) is the `<` input, script argument or `source` argument of a shell unit on the same line. That doesn't bring back the round-3 `notes.md` / `bash build.sh` false positive.

3. **[nit, pre-existing] Wrappers and shells that aren't recognised.**
   - Input:
     - wrappers: `caffeinate -i bash`, `arch -arm64 bash`, `sandbox-exec -n … bash`, `flock /tmp/l bash`, `chroot / bash`;
     - `$SHELL` / `"$SHELL"` / `${SHELL}`;
     - shells: `mksh`, `yash`;
     - commands that run their stdin through a shell: `at now`, `batch`, `crontab -`, `make -f -`, `script -q /dev/null`.
   - Observed: each one with a `rm -rf ~` body → Ask, the same at all three commits.
   - Fix (M1-13): add those wrappers to `unwrap`, add `mksh` and `yash` to `SHELLS`, treat a command word that starts with `$` as reading code, and treat `at`, `batch`, `crontab` and `make -f -` as shell readers.

## Summary
The inverted rule works. Every here-doc and here-string attack from rounds 1–5 is hard-denied, including the three shapes from round 5. The round-3 false positives still ask, and so does the common `git commit -m "$(cat <<'EOF' … EOF)"` form. Nothing regressed toward Allow.

Three non-blocking items remain:
- `$(…)` in an unquoted body, or in the owner's arguments, now makes text count as shell input, which is a new false hard deny (1, a one-line fix);
- a body that reaches a shell through a file, a fifo or an fd asks, consistent with the round-3 decision (2);
- some wrappers and shells aren't recognised (3).

**Approve.** Fix 1 if convenient, and track 2 and 3 under M1-13.
