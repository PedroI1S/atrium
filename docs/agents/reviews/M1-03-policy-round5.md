# Review: M1-03-policy (round 5)
Reviewer: Opus 5.5 (read-only; D-053) · 2026-11-09 · fixes at `7bdee82`

## Verdict
**rework (small)**: R1 and R2 are fixed, and the round-3 cases still ask. However, three here-doc shapes that were hard-denied at `6378100` and at `9521e56` now only ask (finding 1). They are a pipe into a `( … )` subshell, a process substitution, and a loop that owns the here-doc. Like R1, they ask rather than allow, and the fix is a few lines.

How I tested: every input was run through `DefaultPolicy::evaluate` (home `/h/u`) from a throwaway test, deleted afterwards. I built `6378100` and `9521e56` in a scratch copy, also deleted afterwards, and ran the finding-1 inputs there. The baseline `cargo test -p atrium-core --test ledger --test policy` gives 20 + 43 passed.

## Round-4 findings and the round-3 "must still ask" cases

| Item | Status | Re-test |
|---|---|---|
| R1 | **fixed** | All nine inputs are hard-denied: `bash <<EOF; true`, `&& true`, `\|\| true`, `bash <<EOF \| cat`, `cat <<EOF \| bash; true`, `bash <<A; cat <<B`, `{ bash; } <<EOF` and `cat <<< 'rm -rf ~' \| sh` give `HardDeny(recursive-delete-outside-cwd)`, and `bash <<EOF; true` with `sudo id` gives `HardDeny(privilege-escalation)`. |
| R2 | **fixed** | `awk -i inplace`, `gawk -i inplace`, `awk -iinplace` and `gawk --include=inplace` on the protected config give `HardDeny(atrium-self)`. |
| Round 3, must still ask | **hold** | These still ask: both `git commit -m` messages that mention Atrium's paths, the `cat > notes.md <<'EOF' … bash build.sh` line, the `git commit -F - <<'EOF' … sh -c make` line, `ssh -F ~/.ssh/config`, `open` and `codesign -dv` of the app, and `awk '{print}'` on the protected config. |

## Findings

1. **[minor, regression] A here-doc piped into a `( … )` subshell, fed through `>( … )`, or owned by a loop, only asks.**
   - Where: `shell.rs`, the body-ownership rule. Three things are missing:
     - the pipe check doesn't see a shell inside a `( … )` stage of the same pipeline;
     - a `>(bash)` process substitution isn't treated as a reader;
     - a `done`/`fi`/`esac` closer isn't treated like `}`.
   - Input and observed, all Ask at `7bdee82`:
     - `cat <<EOF | (bash)\nrm -rf ~\nEOF`
     - `cat <<EOF > >(bash)\nrm -rf ~\nEOF`
     - `while read l; do bash; done <<EOF\nrm -rf ~\nEOF`
   - Before: all three were `HardDeny(recursive-delete-outside-cwd)` at both `6378100` and `9521e56`.
   - Problem: the hard deny becomes something the user can approve. I checked with bash that both the `( … )` stage and a command inside the loop body read the here-doc. (A body passed through `bash < <(cat <<EOF …)` asked in every round, so it isn't a regression.)
   - Fix:
     - Treat a closer `done`, `fi` or `esac` the same as `}`: count the body when the line has any reader.
     - In the pipe check, count the units of a `( … )` stage (and any derived units) that belong to the same pipeline.
     - Count a body when its owner has a `>( … )`/`<( … )` substitution containing a reader.
     - Add the three inputs to the table.

## Checked and held
- **Here-doc ownership still hard-denies:**
  - `bash <<EOF`, `sh <<<`, `bash /dev/stdin`, `bash -o pipefail <<<`, `bash -s`, `<<-EOF`, `bash 2>&1 <<EOF`, `bash 0<<EOF`, `<<EOF bash`;
  - `eval <<<`, `source /dev/stdin`;
  - `env`, `exec`, `nice`, `time`, `command` and `x=1` prefixes;
  - `if … then bash <<EOF … fi`, `(bash) <<EOF`, `( bash ) <<EOF`, `(bash <<EOF … )`;
  - pipes: `cat <<EOF | bash`, `cat <<EOF | tee x | bash`, `cat <<EOF | { bash; }`, `{ cat; } <<EOF | bash`, `(cat) <<EOF | bash`, `cat <<< … | tee x | sh`, `true | bash <<< …`;
  - several here-docs on a line: `bash <<A <<B` with the payload in either body, `cat <<A; bash <<B`, `cat <<A | bash; cat <<B`, `cat <<A; cat <<B | bash`, `bash <<A && cat <<B && bash <<C`, and a second here-doc on a later line;
  - nesting: `sh -c 'bash <<EOF …'`;
  - bodies with `sudo`, `curl | sh` and `cat ~/.npmrc`.
- **awk:** credential reads (`awk 1 ~/.ssh/id_ed25519`) and `print >` to the protected config stay hard-denied. Plain `awk '{print $1}'` asks.
- **Performance:** a line of 130,000 `{` asks in 191 ms (debug build).

## Summary
Round 5 fixes R1 and R2. Every here-doc and here-string shape from rounds 1–4 is hard-denied again, including pipes, `{ … }` groups and several here-docs on one line. `awk -i inplace` is caught. The round-3 cases still ask. Three remaining shapes were hard-denied before this round and now only ask: a here-doc piped into a `( … )` subshell, one fed through `>(bash)`, and one owned by a `while … done` loop. They can't reach Allow, but they reopen the R1 class.

**Rework (small):** treat loop and `if` closers like `}`, look into `( … )` pipeline stages and process substitutions, and add the three inputs. Then approve on a re-run of this table.
