# Review: M1-03-policy (round 4)
Reviewer: Opus 5.5 (read-only; D-053) · 2026-11-09 · fixes at `9521e56`

## Verdict
**rework (small)**: round-3 findings 1–4 are all fixed. But the new "the body's owner must be a shell" rule for here-docs misplaces the owner. Some here-doc and here-string attacks that were hard-denied in rounds 1–2 now only ask (R1). In addition, `awk -i inplace` writes to Atrium's config with Ask where round 2 gave `atrium-self` (R2). Both regressions ask rather than allow, and both fixes are a few lines. After them, one confirmation run is enough: re-run the inputs below.

How I tested: every input was run through `DefaultPolicy::evaluate` (home `/h/u`) or through a real `SessionGate`, from a throwaway test that was deleted afterwards. The baseline `cargo test -p atrium-core --test ledger --test policy` gives 20 + 42 passed.

## Round-3 findings

| # | Status | Re-test |
|---|---|---|
| 1 | **fixed** | `git commit -m 'document /Applications/Atrium.app paths'` and `git commit -m 'stop writing ~/.config/atrium/config.toml'` → Ask. |
| 2 | **fixed** | `cat > notes.md <<'EOF'\nsudo make install\nEOF\nbash build.sh` and `git commit -F - <<'EOF'\nrm -rf ~ fix\nEOF\nsh -c make` → Ask. |
| 3 | **fixed** | All of these → `HardDeny(atrium-self)`: `sed '/./w …'`, `--expression='w …'`, `-e'w …'`, a 9-token script before `w`, `uniq -`, `xxd -r -`, `xxd` with two operands, `sort --out=` and `--o`, both `awk print >` forms, `codesign -v --force --sign -` and `codesign -dvf -s -`. |
| 4 | **fixed** | Hard links to `~/.ssh/work/id_work` and `~/.ssh/a/b/id_deep` → Deny in 0 ms. |

## Regressions

R1. **[minor] The owner of a here-doc body is the last command on the line when the body starts, not the command that has the `<<`. A here-string piped into a shell, or fed to a `{ …; }` group, has no shell owner.**
   - Where: `shell.rs`. Here-docs use `bodies.push((raw_units.len().saturating_sub(1), …))`, which reads the index at the newline. Here-strings use `raw_units.len()`.
   - Input, all Ask at `9521e56`:
     - `bash <<EOF; true\nrm -rf ~\nEOF`
     - `bash <<EOF && true …`
     - `bash <<EOF || true …`
     - `bash <<EOF | cat …`
     - `cat <<EOF | bash; true …`
     - `bash <<A; cat <<B\nrm -rf ~\nA\nls\nB`
     - `{ bash; } <<EOF …`
     - `cat <<< 'rm -rf ~' | sh`
     - also `bash <<EOF; true\nsudo id\nEOF` → Ask, where it should be `privilege-escalation`.
   - At `9aa0ea6` and at `6378100`, every one of these is a hard deny, because a shell with only flag arguments was present. `cat <<EOF | bash` and `bash <<A <<B` still hard-deny, but only by chance: the shell happens to be the last command on the line.
   - Problem: `rm -rf ~` and `sudo` fed to a shell can be approved, against D-050, again. The round-2 finding-f class has come back in a new shape. (`cat ~/.npmrc` in a body is still caught by the raw-text scan.)
   - Fix:
     - Record the owner when the `<<`/`<<<` operator is lexed, not at the newline. That is the index of the command that holds the redirect.
     - Also count a body when its owner is piped into a command that reads stdin as code (as is already done for `echo`/`printf`), or when its owner is a `{ … }` or `( … )` group that contains such a command.
     - Add the nine inputs above to the table.

R2. **[minor] `awk -i inplace` writes in place, but it now only asks.**
   - Where: `words.rs`, `is_writer`. `awk` is back in `READERS`, and its writer test only looks for `>`, `|` and `system(` in the script.
   - Input: `awk -i inplace '{print}' ~/.config/atrium/config.toml`. Also `gawk -i inplace …`, `awk -iinplace …` and `gawk --include=inplace …`.
   - Observed: Ask. At `9aa0ea6` (where `awk` was not a reader) this was `HardDeny(atrium-self)`. With gawk, which is the `awk` on many Linux systems, `-i inplace` rewrites the file.
   - Fix: count `awk`/`gawk` as a writer when an argument is `-i…` or `--include…` (any value), or when `inplace` appears.

## Checked and held
- **Here-docs and here-strings still hard-denied:**
  - `bash <<EOF`, `sh <<<`, `bash /dev/stdin <<…`, `bash -o pipefail <<<`, `bash --norc <<<`, `bash -s`, `<<-EOF`, `bash 2>&1 <<EOF`;
  - `eval <<<`, `source /dev/stdin`, `. /dev/stdin <<<`;
  - `env`/`exec`/`nice` wrappers, `if … then bash <<EOF … fi`, `(bash) <<EOF`, `(bash <<EOF … )`;
  - `cat <<A; bash <<B`, `bash <<< …; true`, a body followed by another command on the next line, and `echo 'rm -rf ~' | bash`;
  - bodies containing `sudo`, `cat ~/.npmrc` and `curl | sh`, when owned correctly.
- **Narrowed `sed`/`awk` splitting:**
  - credential reads are still hard-denied: `awk 1 ~/.ssh/id_ed25519`, `awk '{print}' ~/.aws/credentials`, `sed -n p ~/.npmrc`, `sed -n p .ssh/id_ed25519`;
  - Atrium's data stays hard-denied (`sed -n p …/atrium/db`);
  - these stay `atrium-self`: `sed -i`, `sed 's/…/w …'`, `sed -n 'w …'`, `awk -v f=<protected> '{print > f}'`;
  - every round-2 write form (`sort -o`, `uniq`, `find -delete`, `xxd -r`, `tree --output=`, `codesign --remove-signature`, `curl -o` into the app) stays a hard deny;
  - so do `date -uf.npmrc` and `ls \r#; rm -rf ~`;
  - `ssh -F ~/.ssh/config`, `open` and `codesign -dv` of the app still ask, and plain `sed -n p` and `awk '{print $1}'` on a notes file ask.
- **The `~/.ssh` walk:** depth 3, a cap of 256 entries, and symlinked folders aren't walked (`file_type` doesn't follow links). Each check takes 0 ms. The round-3 nit (a symlink in `~/.ssh` pointing at a folder hard-denies that folder) is unchanged.

## Summary
Round 4 closes all four round-3 findings: commit messages and notes no longer trigger false hard denies, every listed write form is hard-denied, and keys in `~/.ssh` subfolders are caught. Two regressions remain, and neither allows anything:
- Here-doc bodies are tied to whichever command is last on the line when the body starts, rather than to the command that has the `<<`. So `bash <<EOF; true` or `cat <<< 'rm -rf ~' | sh` only asks, where rounds 1–2 hard-denied it (R1).
- `awk -i inplace` on Atrium's config only asks (R2).

**Rework (small):** record the owner at the `<<` operator, count pipes and groups into a shell, and treat `awk -i` as a write. Then confirm with the inputs above.
