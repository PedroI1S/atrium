## Verdict

**rework**

Reviewed `dev...c12dbd4`. The existing evaluator remains unwired from rev 4; changes to existing implementation files are visibility-only. The frozen interface and annex still have findings. No files were modified.

## Findings

1. **major — `crates/atrium-core/src/policy/tools.rs:1848`: Unrecorded restrictions change ordinary verdicts.** `ls -la`, `head -n 5 README.md`, `wc -l README.md`, and `git status --short` are allowed in Home by dev but become Unread here. This affects empty and partial flag tables throughout the annex, plus omissions such as `test` under covering rules. **Fix:** audit every restriction, record the applicable §11 item—normally item 2—and obtain explicit contract approval for restrictions outside its approved scope. Merely mirroring Home’s forbidden-list metadata does not preserve its behavior.

2. **major — `crates/atrium-core/src/policy/tools.rs:567`: Followed-search relaxations lack the correct default-change record.** With an ordinary project containing no protected links, `rg -L x proj` and `grep -R x proj` ask in dev’s Home evaluator; the new table makes them eligible for Allow. The rows cite only item 9, which concerns operand extraction. **Fix:** explicitly reconcile these relaxations, including §10 row 15, with §11 and annotate the affected rows.

3. **major — `crates/atrium-core/src/policy/tools.rs:2330`: Three operand-extraction relaxations are unmarked.** `true`, `false`, and `id` change all operands from dev’s candidate paths to `Text` without `changes_default`. For example, an operand alias resolving to a protected file stops receiving the modeled path check. **Fix:** mark these rows with §11 item 9 and include them in the handoff’s change list.

4. **major — `crates/atrium-core/src/policy/tools.rs:3005`: The annex contradicts the unknown-tool fallback.** It says every absent tool makes its unit Unread. Contract §4 instead retains unknown tools for hard-deny/store checks, treating every word as a candidate path. Consequently, a simple custom command under an exact Allow rule would receive different coverage depending on which text P3 follows. **Fix:** distinguish unknown-tool fallback from explicitly excluded known forms in the schema documentation and generated annex.

5. **major — `crates/atrium-core/src/policy/tools.rs:2140`: Tar creation operands are incorrectly classified as archive members.** In `tar -ch proj/key`, `proj/key` is a host source path. If it aliases a protected file, this layout supplies no target to resolve. **Fix:** distinguish creation/update source paths from extraction/listing member selectors; do not apply the member-operand relaxation to creation sources.

6. **major — `crates/atrium-core/src/policy/tools.rs:1658`: Writer targets inherit read access.** `git clone` declares a destination root but inherits `AllRead`; signing `codesign` forms likewise inherit read access. `git checkout` classifies every operand as a ref, leaving explicit checkout destinations unrepresented. A destination alias into an Atrium protected configuration path can therefore miss the write-only protection. **Fix:** encode destination/write access for these forms and distinguish checkout refs from destination pathspecs.

7. **major — `crates/atrium-core/src/policy/tools.rs:1610`: Git branch creation is modeled as non-writing.** `git branch topic` inherits `WriteBehavior::Never`. Thus `git branch topic; cat README.md` does not acquire §5’s after-writer reason under covering rules. **Fix:** distinguish branch listing from branch creation and mark the latter as writing.

8. **major — `crates/atrium-core/src/policy/tools.rs:51`: Flag effects cannot express disabling or ordered overrides.** `ack --follow --nofollow x proj`, `find -L -P proj`, and `cp -LP src dst` require a later option to disable link following. The disabling flags have no effects, while enabling flags only set booleans. Recursion-disabling flags have the same problem. **Fix:** represent explicit enable/disable overrides and their ordering; otherwise conservative traversal can incorrectly become a hard-deny hit.

9. **major — `crates/atrium-core/src/policy/tools.rs:1349`: Every `sed -e` expression is marked as arbitrary code.** `sed -e p README.md` becomes plain Unread despite §4 restricting this treatment to scripts containing `e`, `w`, or `r`. Its positional layout also still expects a script operand after `-e`. **Fix:** encode expression-supplied script selection and inspect the specified script operations before assigning the code-runner outcome.

10. **major — `crates/atrium-core/src/policy/tools.rs:2025`: Conditional code runners have inconsistent prefix validation.** `ag` and `ack` can execute the table’s `--pager`/`--output` code, but retain `code_runner_no_pin = false`. Likewise, `git diff` and `git push` expose program-running flags without marking prefixes ending there invalid. **Fix:** apply §7 consistently to these prefixes and record §11 item 4.

11. **major — `crates/atrium-core/src/policy/v4/paths.rs:19`: Resolver errors are indistinguishable from absence.** Both an inaccessible path and a nonexistent path return `Ok(None)`. P2 therefore cannot distinguish a legitimate nonexistent suffix from an unreadable directory requiring `UnresolvablePath`. **Fix:** provide separate absence and I/O-failure results across the resolver methods.

12. **major — `crates/atrium-core/src/policy/v4/paths.rs:29`: Resolver allocations cannot charge the shared budget.** `read_link`, `canonicalize`, and especially `read_dir` allocate returned buffers but receive only filesystem caps. With the allocation budget nearly exhausted, they cannot charge the actual allocation before performing it. **Fix:** pass the compute/allocation budget through the resolver interface, or provide a streaming/caller-buffer interface with explicit precharging and exhaustion reporting.

13. **major — `crates/atrium-core/src/policy/v4/paths.rs:98`: Recursive hard-link checks cannot return their required result.** `check_recursive_root` promises the bounded hard-link walk but returns only `Vec<PartResult>`. None of those outcomes expresses the multiply-linked-file condition. An otherwise ordinary multiply-linked file found under `rg x proj` must force Ask even under FullAccess. **Fix:** return a shared result containing both outcomes and the multiply-linked flag.

14. **major — `crates/atrium-core/src/policy/model.rs:760`: Filesystem-dependent derived destinations are lost between P3 and P2.** The table expresses `mv`’s `WhenSourceIsDir`, but `ToolEffects` reduces it to a boolean. `classify_unit` has no resolver, so it cannot determine that boolean for `mv src dst`. **Fix:** carry the conditional behavior into the shared effects record for P2 to resolve.

15. **major — `crates/atrium-core/src/policy/tools.rs:3315`: The code-runner integrity test accepts missing code locations.** `no_pin || has_code_operand(...)` lets every `command_runner_row` pass despite declaring all operands `Text` and providing no pinning condition. For example, `ssh host command` identifies no code operand. **Fix:** require an actual code location or program-pinning condition, and encode one for those rows; prefix invalidity is not a substitute.

16. **minor — `crates/atrium-core/src/policy/tools.rs:2998`: The generated copy omits review-relevant data.** It never renders operand access modes, and flag effects are rendered only for tools with more than twelve flags. Changing clone’s destination from read to write, or changing which `codesign` flag writes, can leave the Markdown unchanged. **Fix:** render access modes and effects for every flag, including subcommand flags; render find primaries with their actual single-dash spelling.

17. **minor — `crates/atrium-core/src/policy/model.rs:907`: The documented folds narrow the approved lowercase operation.** “ASCII lowercase” differs from dev’s Unicode `to_lowercase()`. For example, an Atrium data path containing `Café` also matches `CAFÉ` under dev’s listed folds. **Fix:** document the existing Unicode lowercase operation plus `ſ`→`s`, without normalization.

18. **major — `crates/atrium-core/src/policy/limits.rs:160`: Required Clippy verification fails.** The supplied verification log reports four `assertions_on_constants` errors at lines 160, 161, 162, and 164. **Fix:** move these assertions into const blocks or otherwise make them pass the required warnings-as-errors check.

## Test gaps

- Home parity tests compare metadata, not the effective accepted forms; they miss finding 1.
- Flag-parity tests check inclusion in one direction, allowing invented additional flags to escape detection.
- Add table assertions for operand access, tar operation modes, conditional writers, disabling flags, and conditional code-runner validation.
- Add resolver-interface fixtures distinguishing missing/inaccessible paths, exhausted allocation budgets, and recursive multiply-linked results.
- The budget rationale gives estimates but no measured charge totals for §10 or the longest adversarial inputs.
- The verification log records formatting and workspace tests passing (`workspace rc=0`), but Clippy failing. I did not run modifying verification commands.