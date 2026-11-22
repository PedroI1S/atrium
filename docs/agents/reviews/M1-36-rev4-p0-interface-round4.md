## Verdict

**rework**

Reviewed `dev...1a09705`. The six round-3 cases are fixed. Existing evaluator edits remain visibility-only, and rev 4 remains unwired. No files were modified.

## Findings

1. **major — `crates/atrium-core/src/policy/tools.rs:2962`: Git’s filesystem comparisons lose their path targets.**  
   `git diff --no-index bridge/plain README.md`, with `bridge` a directory symlink into an Atrium data directory, reads protected filesystem content. The row accepts `--no-index` through `AnyExceptForbidden` but classifies both inputs as `Ref`, excluding them from canonical path checks. Item 9 does not authorize dropping actual filesystem inputs. **Fix:** preserve conservative path checks for this form or represent its operands as Read paths. [Git documentation](https://git-scm.com/docs/git-diff)

2. **major — `crates/atrium-core/src/policy/tools.rs:2454`: Sed’s `-i` arity is incorrect.**  
   On macOS, `sed -i .bak -e p README.md` consumes `.bak` as a backup extension. Here `-i` is Boolean, so `.bak` becomes a Write path. If `.bak` aliases an Atrium write-protected file, this manufactures a hard-deny hit. On GNU sed, `sed -ie p data` consumes `e` as the backup suffix, while the table interprets it as `-e p` and omits the backup output. **Fix:** represent platform-specific argument consumption and backup effects, or explicitly exclude unsupported forms. Correct the arity fixture at line 7551. [Apple sed manual](https://raw.githubusercontent.com/apple-oss-distributions/text_cmds/main/sed/sed.1), [GNU sed manual](https://www.gnu.org/software/sed/manual/sed.html)

3. **major — `crates/atrium-core/src/policy/tools.rs:3667`: Patch’s input patch file is marked Write.**  
   In `patch target.txt patch.diff`, the second operand is read. `AllWrite` instead produces an `atrium-self` hard deny when `patch.diff` is an Atrium write-protected file, even though the operation only reads it. **Fix:** distinguish the original-file operand’s write access from the patch-file operand’s read access, with a Rule G fixture. [GNU patch invocation](https://www.gnu.org/s/diffutils/manual/diffutils.pdf)

4. **major — `crates/atrium-core/src/policy/tools.rs:3669`: Patch’s content-derived destinations are declared explicit.**  
   Bare `patch` can obtain destination names from its stdin patch. The row inherits `DerivedDestinationsBehavior::Never`, supplies no targets, and can consequently be Clear under an exact Allow rule. Contract §5 requires the writer itself to be Unread when destinations depend on contents. **Fix:** mark this form as having derived destinations, cite item 10, and test a patch stream naming a destination absent from argv. [GNU patch invocation](https://www.gnu.org/s/diffutils/manual/diffutils.pdf)

5. **major — `crates/atrium-core/src/policy/tools.rs:2420`: Codesign’s force modifier independently enables writes.**  
   `codesign --verify --force app` verifies `app`; `--force` affects signing and is ignored for this operation. The table nevertheless assigns Write access, manufacturing a hard-deny hit when `app` is write-protected by Atrium. **Fix:** derive access from the selected operation; verification/display inputs remain Read, and force modifies signing only. [macOS codesign manual](https://man.freebsd.org/cgi/man.cgi?manpath=macOS+26.7&query=codesign&sektion=1)

6. **major — `crates/atrium-core/src/policy/tools.rs:3572`: Rsync is unconditionally classified as recursive.**  
   `rsync proj/ out/` does not recurse without a recursion-enabling option. With an Atrium data directory below `proj` outside home, the declared recursive read would incorrectly trigger S8’s ancestor hard deny. None of the currently listed rsync flags enables recursion. **Fix:** make recursion reflect the accepted form and add a fixture distinguishing directory skipping from recursive transfer. [Rsync manual](https://download.samba.org/pub/rsync/rsync.1)

## Test gaps

- Add documented-behavior fixtures for the cases above, including resulting access modes and modeled hits.
- The arity test currently enshrines sed’s incorrect zero-argument declaration.
- Verification is clean: fmt, Clippy, workspace tests (`rc=0`), and three core reruns passed. Compute-budget measurement remains P1’s obligation.