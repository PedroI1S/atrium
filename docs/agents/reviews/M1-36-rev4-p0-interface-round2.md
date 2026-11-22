## Verdict

**rework**

Reviewed `dev...f966066` and the 18 round-1 fixes. Those fixes are present, including both rulings. Existing evaluator edits are visibility-only; rev 4 remains unwired. Six new findings remain. No files were modified.

## Findings

1. **major — [policy/tools.rs:58](/home/rodrigo/atrium-wt/M1-36-rev4-p0-interface/crates/atrium-core/src/policy/tools.rs:58): Unlisted options retain acceptance but lose write semantics.**  
   `cp -t vault README.md`, where `vault` aliases an Atrium write-protected directory, treats `vault` as a read source and the final operand as the destination. Likewise, `sort --out=alias README.md` accepts the unlisted abbreviation without enabling writing, although dev recognizes `--out` as `--output`. These forms can lose dev’s `atrium-self` hit; §11 item 10 does not authorize that relaxation. **Fix:** preserve dev’s conservative write checks for unclassified path candidates, or correctly model these options and abbreviations.

2. **major — [policy/tools.rs:770](/home/rodrigo/atrium-wt/M1-36-rev4-p0-interface/crates/atrium-core/src/policy/tools.rs:770): Ripgrep’s global glob case setting has no effect.**  
   Both case-setting flags are effectless booleans, while `-g` always declares a case-sensitive filter. With `proj/key → ~/.ssh/id_rsa`, `rg -L --glob-case-insensitive -g KEY x proj` selects `key`, but the declared filter misses it. This can lose the selected-entry hard-deny hit. **Fix:** represent the global case toggle and apply its final setting to every `-g` filter, preserving `--iglob` semantics.

3. **major — [policy/tools.rs:943](/home/rodrigo/atrium-wt/M1-36-rev4-p0-interface/crates/atrium-core/src/policy/tools.rs:943): Unsupported search filters are declared without an Unread effect.**  
   `grep -R --exclude=key x proj`, with the protected alias above, has no declared filter effect. Consuming the table therefore permits an unfiltered followed-link check to produce HardDeny. Contract §5 instead makes unsupported/negated filters Unread; an over-approximation must not create a modeled hard-deny hit. Similar omissions exist for `ag`’s filtering options. **Fix:** explicitly identify unsupported filter forms in the table and propagate `UnsupportedFilterForm`.

4. **major — [policy/tools.rs:1391](/home/rodrigo/atrium-wt/M1-36-rev4-p0-interface/crates/atrium-core/src/policy/tools.rs:1391): Tar’s `-C` lacks operand-resolution semantics.**  
   With cwd `/h/u`, empty `sub`, and `/h/u/key` aliasing a protected file, `tar -chf out.tar -C sub ../key` reads `/h/u/key`. The table merely marks `sub` as `DestinationRoot`; it supplies no change of base for following source operands. Resolving `../key` against the original cwd misses the accessed alias. **Fix:** carry ordered, per-operand resolution bases from P3 to P2, or explicitly exclude this form with the required approved citation.

5. **major — [policy/tools.rs:1627](/home/rodrigo/atrium-wt/M1-36-rev4-p0-interface/crates/atrium-core/src/policy/tools.rs:1627): A named sed program cannot receive program coverage.**  
   `sed -f edit.sed README.md` declares a `ProgramFile`, but `ScriptCommands` treats `-f` as an inline-code trigger and supplies no program-pinning condition. An exact rule naming this program consequently cannot authorize its `Unread (program)` reason as §§4 and 7 require. **Fix:** represent flag-held program files and their pinning condition, distinguish them from inline expressions, and cite §11 item 8.

6. **major — [policy/tools.rs:2785](/home/rodrigo/atrium-wt/M1-36-rev4-p0-interface/crates/atrium-core/src/policy/tools.rs:2785): Flag-value operand relaxations remain unmarked.**  
   `codesign -s identity app`, where `identity` aliases a protected file, receives dev’s candidate-path check. Here `identity` becomes `Text`, but `codesign.changes_default` is `None`. This is a §11 item 9 relaxation missing from the approval summary. The citation test checks positional layouts only, so it misses this case. **Fix:** annotate the row and audit value-taking flags, including Git’s text-valued global options and find’s text-valued options; extend the citation test accordingly.

## Test gaps

- Acceptance helpers check flag acceptance without verifying resulting operand access, resolution bases, filter effects, or program coverage. Add fixtures for the cases above.
- The completed verification reports `FMT_OK` and `CLIPPY_OK`, but workspace tests exited **101**: `questions_use_the_gates_question_ticket` failed at `crates/atrium-agents/tests/claude_adapter.rs:1088`. The three core reruns passed; a clean workspace result or baseline diagnosis is still missing.
- The compute-budget rationale remains estimated; the verification contains no measured charge totals.