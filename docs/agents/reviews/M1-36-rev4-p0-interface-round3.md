## Verdict

**rework**

Reviewed `dev...cde6cce`. The six round-2 targeted fixes are present, but the audit has new findings below. Existing evaluator edits remain visibility-only; rev 4 is unwired. No files were modified.

## Findings

1. **major — [policy/tools.rs:808](/home/rodrigo/atrium-wt/M1-36-rev4-p0-interface/crates/atrium-core/src/policy/tools.rs:808): Newly restricted rows lack default-change citations.**  
   The round-3 switch to `ListedOnly` makes `sort -r README.md` and `rm -f build.tmp` Unread under covering rules, where dev allows them. Both rows have `changes_default: None`; other affected rows cite unrelated items. **Fix:** audit all newly restricted forms and record the applicable approved §11 item, normally item 2, in each affected row and the handoff.

2. **major — [policy/tools.rs:4533](/home/rodrigo/atrium-wt/M1-36-rev4-p0-interface/crates/atrium-core/src/policy/tools.rs:4533): Home-forbidden flags are declared Unread under every setting.**  
   The normative annex says any forbidden flag on an `AnyExceptForbidden` row makes the unit Unread. Consequently, `tree -o out.txt` cannot receive coverage from an exact Allow rule, despite having explicitly modeled output semantics and being coverable on dev. The rule-acceptance test helper contradicts this annex text. **Fix:** distinguish Home eligibility restrictions from forms outside the model; update the annex and test both settings.

3. **major — [policy/tools.rs:2010](/home/rodrigo/atrium-wt/M1-36-rev4-p0-interface/crates/atrium-core/src/policy/tools.rs:2010): Tar extraction marks its input archive as written.**  
   `tar -xf archive.tar` enables `writes`, so `WriteWhenWriting` assigns Write access to `archive.tar`. Extraction reads that file. With the archive configured as an Atrium write-protected path, this manufactures a modeled `atrium-self` hit. **Fix:** make archive access depend on the operation: Read for extraction/listing, Write for creation/append/update. Audit `-C` access separately.

4. **major — [policy/tools.rs:2333](/home/rodrigo/atrium-wt/M1-36-rev4-p0-interface/crates/atrium-core/src/policy/tools.rs:2333): Sed program pinning introduces false input writes.**  
   With `print.sed` containing `p`, `sed -f print.sed protected` reads `protected`. Here `-f` enables writing, and `AllWriteWhenWriting` marks that input Write—the new fixture explicitly asserts this. If `protected` is an Atrium write-protected file, a covering exact rule receives HardDeny instead of authorizing the pinned program and ordinary read. **Fix:** separate possible-writer tracking for uninspected programs from input access; `-f` alone must not turn sed inputs into write targets.

5. **major — [policy/tools.rs:2197](/home/rodrigo/atrium-wt/M1-36-rev4-p0-interface/crates/atrium-core/src/policy/tools.rs:2197): Find’s `-fprintf` lacks its second argument role.**  
   `find proj -fprintf out.txt alias` consumes both FILE and FORMAT. The table consumes only `out.txt`; its all-Path layout then classifies the literal format `alias` as another filesystem path. A protected-path alias therefore becomes a false modeled hit. **Fix:** represent the two consumed arguments as Write Path and Text, or explicitly make this unsupported form Unread.

6. **minor — [policy/tools.rs:1369](/home/rodrigo/atrium-wt/M1-36-rev4-p0-interface/crates/atrium-core/src/policy/tools.rs:1369): The grep audit invents `--include-dir`.**  
   `grep --include-dir=sub x proj` receives a declared `Value(Pattern)` form absent from dev’s tables. The parity test masks this by adding `"include-dir"` to its expected set at line 5424. This violates the brief’s no-invented-flags requirement. **Fix:** remove the synthetic entry and expected-set exception; let the unknown-flag policy handle it.

## Test gaps

- Add fixtures for the cases above, especially operation-specific tar access, sed input reads, and multi-argument flags.
- Check default-change citations for newly rejected forms, alongside operand relaxations.
- Use independent expected semantics: the sed fixture currently enshrines finding 4.
- The completed verification reports fmt, Clippy, workspace tests, and all three core reruns passing. Compute-budget measurement remains P1’s obligation under the ruling.