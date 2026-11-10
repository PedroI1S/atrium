# Review: M1-03-policy (round 3)
Reviewer: Opus 5.5 (read-only; D-053) · 2026-11-09 · fixes at `9aa0ea6`

## Verdict
**approve with fixes**: findings a–g and the empty-argv part of h are fixed. None of the new code turns anything into Allow. The fan-out and brace costs are gone: every overflow shape I tried gives Deny within 1 ms.

The fix round adds two small regressions: false **hard denies**, which can't be overridden (findings 1 and 2 below). Fix both before the gate. They are a few lines each. Everything else below asks rather than allows, and can go to M1-13 or a later round.

How I tested: every input was run through `DefaultPolicy::evaluate` (home `/h/u`, protected `/h/u/.config/atrium` and `/Applications/Atrium.app`) or through a real `SessionGate` on a temp home. The runs used a throwaway test, deleted afterwards. To tell regressions apart, I built `6378100` in a scratch copy and ran the same lines. The baseline `cargo test -p atrium-core --test ledger --test policy` gives 20 + 40 passed.

## Round-2 findings

| # | Status | Re-test |
|---|---|---|
| a | **fixed** | `echo x|`×n + `sh|`×n + `sh`: n = 1,000, 3,000 and 13,000 (130 KB) → Deny "too large" in 0–1 ms (`parse_command` 0–1 ms). Other shapes also give Deny in 0–1 ms: 12,000 `echo x|sh;` pipelines, 12 nested `echo '…'|sh` levels, and 400 here-doc bodies on one `bash`. |
| b | **fixed** | `ls` + 100,000 / 130,000 `{` → Ask in 45 / 37 ms. It now asks, because a group longer than 256 characters counts as expanding, which is safe. `cat {` + 300 × `a` + `,.npmrc}` and the `~/{…,.ssh/id_ed25519}` form → `HardDeny(credential-read)`. |
| c | **fixed** | These now ask (they were hard denies in round 2): `ssh -F ~/.ssh/config host`, `ssh-keygen -lf ~/.ssh/authorized_keys`, `open`, `codesign -dv` and running `/Applications/Atrium.app/…/atrium --version`. `git diff --no-index ~/.ssh/config x` → Allow. Writes are still hard-denied: `cp k ~/.ssh/authorized_keys`, `echo k >> ~/.ssh/authorized_keys` and `curl -o` into Atrium.app. |
| d | **fixed** (gaps: finding 3) | All five round-2 inputs → `HardDeny(atrium-self)`, and so do `find -fprint0`, `tree --output=`, `sed 's/a/b/w …'` and `codesign --remove-signature`. |
| e | **fixed** (gap: finding 4) | A hard link to `~/.ssh/id_ed25519` next to a 2,100-file Chrome profile → Deny in 0 ms. |
| f | **fixed** | `bash /dev/stdin <<EOF`, `bash /dev/stdin <<<` and `bash -o pipefail <<<` (each with `rm -rf ~`) → `HardDeny(recursive-delete-outside-cwd)`. |
| g | **fixed** | `ssh -` + `v`×70 + `i.ssh/id_ed25519 host` → Ask, with the reason "flag cluster too long", so it can't be remembered. |
| h (empty argv) | **fixed** | With an allow rule `CommandExact { argv: [] }`, the line `>/dev/null` gives Ask, and so does the empty line. |

## Findings (new)

1. **[minor, regression] Quoted multi-word arguments are now split into path tokens, so a message that mentions a protected path is hard-denied.**
   - Where: `words.rs` `raw_words_of`, where any argument containing whitespace is split into 8 tokens for every command, not just `sed`.
   - Input: `git commit -m 'document /Applications/Atrium.app paths'` and `git commit -m 'stop writing ~/.config/atrium/config.toml'`.
   - Observed: `HardDeny(atrium-self)` for both. At `6378100` both gave Ask.
   - Problem: the hard deny can't be overridden. It will hit commits and PR bodies in Atrium's own repository. (Credential paths in message text were already hard-denied by the raw-text scan before this round, by design. That behaviour is unchanged.)
   - Fix: split only the script arguments of `sed` and `awk` (the commands whose scripts can name output files), not every argument.

2. **[minor, regression] Here-doc bodies are re-lexed whenever any shell appears anywhere in the line, so text written to a file is hard-denied as if it ran.**
   - Where: `shell.rs`, `is_stdin_code` / `texts.extend(bodies)`.
   - Input: `cat > notes.md <<'EOF'\nsudo make install\nEOF\nbash build.sh` and `git commit -F - <<'EOF'\nrm -rf ~ fix\nEOF\nsh -c make`.
   - Observed: `HardDeny(privilege-escalation)` and `HardDeny(recursive-delete-outside-cwd)`. At `6378100` both gave Ask.
   - Fix: re-lex a body only when the unit that owns the redirect is a shell, `eval` or `source`, whatever its arguments are. That keeps every finding-f case (`bash /dev/stdin <<…`, `bash -o pipefail <<<`).

3. **[minor] The write forms of the reader commands still have gaps. They ask, not hard-deny.**
   - These each ask instead of giving `HardDeny(atrium-self)`:
     - `sed -n '/./w ~/.config/atrium/config.toml' x` (a script starting with `/`)
     - `sed --expression='w …' x`
     - `sed -e'w …' x`
     - a `sed` script with more than 8 tokens before `w`
     - `uniq - <protected>`
     - `xxd -r - <protected>`
     - `xxd /etc/hosts <protected>` (`xxd` writes to a second operand even without `-r`)
     - `sort --out=` / `--o` (GNU long-option prefixes)
     - `awk '{print > "<protected>"}'`
     - `codesign -v --force --sign - /Applications/Atrium.app` and `codesign -dvf -s - …`. I confirmed on this Mac that both re-sign the file (exit 0; `Signature=adhoc` afterwards).
   - None of these is a bypass: none of these commands is on Home's read-only list, so each one asks.
   - Fix (M1-13 or later):
     - `sed`: treat any non-flag script, and any `-e…`/`--expression=…` value, that contains `w`/`W` as a write.
     - Count `-` as an operand for `uniq` and `xxd`, and make `xxd` with two operands a write.
     - Match `sort` long options by prefix.
     - Make `codesign` a write whenever `-s`/`--sign`/`-f` is present.
     - Treat `awk` like `sed`.

4. **[minor, regression] The identity map now covers only one level of `~/.ssh`.**
   - Input: a hard link `proj/kw` → `~/.ssh/work/id_work`, then `cat proj/kw`.
   - Observed: Allow. At `6378100` the depth-3 walk caught it, at least in a small home.
   - Fix: walk `~/.ssh` (only that root, a small tree) to depth 3 with a small cap. Same precondition as finding 5.

5. **[nit] A symlink in `~/.ssh` that points at an ordinary folder hard-denies that folder.**
   - Input: `~/.ssh/proj-link` → `~/proj`.
   - Observed: `cat proj/readme.txt` and `ls proj` → Deny (credential-read). `metadata` follows the link, so `~/proj` gets the identity of a `.ssh` member.
   - Status: this was already true in round 2, and it's a rare setup.
   - Fix: use `symlink_metadata` for the `.ssh` members that are themselves symlinks to directories, or skip them.

## Hard links inside the other credential trees (the worker's note)

I confirmed the limit with these inputs:
- A hard link to `~/.gnupg/private-keys-v1.d/A.key` → `cat` is allowed.
- A hard link to Chrome `Default/Login Data` → `cat` is allowed.
- The same holds for Atrium's own data: a hard link to `~/.local/share/atrium/db` → `cat` is allowed.
- `cp x <hard link to ~/.config/atrium/config.toml>` → Ask, not a hard deny.

**This is acceptable for M1, with M1-13 as the follow-up.**
- The agent can't create such a link without a prompt. `ln` or `cp -l` naming the target is hard-denied, and a glob or `find` that reaches it either asks or names the tree root, which is denied.
- So the link has to exist already, for example from a restore or a sync tool. Hard links to these files are rare.
- The same precondition already applied to every round-2 alias check.

M1-13 should cover all four kinds: the credential trees, browser files, Atrium data and protected paths. A cheap first step: for any file under Home with `st_nlink > 1`, outside the sensitive roots, never auto-allow (ask). That closes the class without walking any tree. Finding 4 should be fixed now: it is a regression, and the fix is one line.

## Summary
Round 3 fixes everything it set out to fix:
- the fan-out and brace costs are gone (Deny or Ask in about 1 ms, even at the 128 KiB cap);
- reads of `~/.ssh/config` and launching Atrium ask again instead of being hard-denied;
- the five write forms from round 2, and here-docs fed to `bash /dev/stdin`, are hard-denied;
- long flag clusters and empty exact rules are handled.

I found no new path to Allow. Two changes overreach, though: splitting every quoted argument into path tokens (1), and re-lexing here-doc bodies whenever any shell is present (2). Both create false hard denies that can't be overridden, on ordinary commit messages and on notes written to files. Narrow them before the gate.

The remaining items ask rather than allow, so they can follow:
- more `sed`/`uniq`/`xxd`/`sort`/`awk`/`codesign` write forms (3);
- `~/.ssh` subfolders in the identity map (4, a one-line fix worth doing now);
- the hard-link limit for the other credential trees, which is acceptable for M1 if it is tracked under M1-13.
