# Review: M1-32-link-globs
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-17 · round 4

## Verdict
**rework**: earlier reproductions are addressed, but credential-link escapes, ordinary-call false denies and gaps in the aggregate work budget remain.

Code-traced at HEAD `ce047b9`; no tests or probes run, and no files written. Triples mean **Home / Home with an unscoped Any allow rule / FullAccess**. Fixtures use home and cwd `H`, with disjoint Atrium paths.

## Findings

1. [blocker] **Metacharacters introduced by a resolved link become glob syntax**
   - Where: `crates/atrium-core/src/policy/gate.rs:936`, `text(c).contains(['*', '?', '[', '{'])`; line 945, `comps.iter().position(&is_pat)?`.
   - Case: existing `H/.ssh/id_rsa`; directory `H/store[1]` containing `key -> ../.ssh/id_rsa`; `H/proj -> store[1]`; no `H/store1`. Run `cat proj/key*`, or submit Search/Glob with paths `["proj"]`, glob `"key*"`, recursive `true`.
   - Result: shell **Ask / Allow / Allow**; Glob **Allow / Allow / Allow**. Both should be **Deny / Deny / Deny** under S11 and contract §9. Resolution introduces the literal directory name `store[1]`, but expansion interprets `[1]` as a class. Nothing matches, and the empty expansion is considered complete. This is a sibling of round 3’s prefix corruption.
   - Fix: preserve the boundary between the resolved literal prefix and the remaining pattern; never reinterpret target-directory bytes as pattern syntax.

2. [major] **Dangling SSH members are omitted from the canonical credential set**
   - Where: `crates/atrium-core/src/policy/gate.rs:197`, `Err(_) => {}`; line 315 builds member trees from `&ssh.members`.
   - Case: existing directories `H/.ssh`, `H/vault`, `H/proj`; absent `H/vault/new_key`; `.ssh/id_rsa -> ../vault/new_key`; `proj/key -> ../.ssh/id_rsa`. Run `printf x > proj/key*`, or submit Search/Glob with paths `["proj"]`, glob `"key*"`, recursive `false`.
   - Result: shell **Ask / Allow / Allow**; Glob **Allow / Allow / Allow**. Both should be **Deny / Deny / Deny**. The project link reaches the protected SSH name, but resolution ends outside `.ssh`, and the dangling member was discarded. The redirect can create its target. S11 explicitly includes dangling links; contract §9 protects the credential spelling regardless of existence.
   - Fix: retain dangling SSH members in the canonical set and resolve their targets with the ordered resolver.

3. [major] **Literal Read paths are still expanded as globs**
   - Where: `crates/atrium-core/src/policy/gate.rs:2102`, `let globby = ps.contains(['*', '?', '['])`; line 2105, `expand_targets(p, include_dotfiles, &mut maybe)`.
   - Case: ordinary, single-linked `H/proj/file[1]`; existing `H/.ssh/id_rsa`; sibling `H/proj/file1 -> ../.ssh/id_rsa`. Call `Read({"file_path":"proj/file[1]"})`, normalized to Read, that path, `glob: None`, recursive `false`.
   - Result: **Deny / Deny / Deny**; should remain **Allow / Allow / Allow** under contract §9’s file-read rule and the brief’s ordinary-call requirement. The literal requested file is harmless. Expansion instead checks the sibling `file1`. Round 3’s dangling-bracket reproduction is fixed, but “literal means literal” is incomplete.
   - Fix: do not expand literal file-tool paths as patterns; handle explicit tool glob patterns separately.

4. [major] **Unreadable literal paths fail open**
   - Where: `crates/atrium-core/src/policy/gate.rs:1142`, `let Ok(md) = std::fs::symlink_metadata(&cur)`; line 1155, `if let Ok(canon) = std::fs::canonicalize(&cur)`; line 1161 returns `cur` without recording failure.
   - Case: existing `H/.ssh/id_rsa`; `H/proj/closed/key -> ../../.ssh/id_rsa`; `closed` has mode `000`, evaluated as its non-root owner. Call Read on `proj/closed/key`, with no glob and recursive `false`.
   - Result: **Allow / Allow / Allow**; should be **Ask / Ask / Ask** under the brief’s explicit “Fail closed where you can’t read something fully” rule. Resolution silently retains the unchecked project spelling. Neither the glob nor recursive-walk error handling runs.
   - Fix: propagate unresolved permission/I/O failures into the check’s uncertainty state, while retaining the intended missing-tail behavior.

5. [major] **The shared budget does not cover the complete link check**
   - Where: `crates/atrium-core/src/policy/gate.rs:264` calls `reaches_linked_ssh_dir(...)` before the budget starts at line 1941, `with_link_budget(|| {`. The earlier pass repeats expansion at lines 1245–1246, `for d in linked` / `glob_reaches(p, d, recursive)`. Separately, line 997 opens `std::fs::read_dir(&dir)` without charging the budget.
   - Case A: `H/.ssh` contains 1,024 links to distinct empty directories outside `proj`; `H/proj` contains exactly 4,096 ordinary files `ka0000`–`ka4095`. Submit the shell line `"cat" + " proj/k*".repeat(4095)`.
   - Result A: eventually **Ask / Ask / Ask**, correctly, but the earlier pass first performs **17,175,674,880 directory-entry visits**: 4,095 operands × 1,024 linked directories × 4,096 entries. These are code-derived counts, not measured runtime.
   - Case B: 4,096 distinct empty project directories; Search/Glob paths name each directory once, recursive `false`, glob is a comma-brace list of 256 distinct alternatives `x0`–`x255`, followed by `*`.
   - Result B: **Ask / Ask / Ask**, but the new pass still performs **1,048,576 empty-directory opens**, plus prefix resolutions. Empty enumerations spend nothing because charging occurs per successful entry.
   - Fix: share the budget across both protection passes, charge directory opens and resolutions, deduplicate the earlier pass, and stop filesystem work when exhausted. Round 3’s exact repeated-root reproduction is fixed; its aggregate-cost finding is only partially closed.

6. [major] **Archive listing scans unrelated project contents and hard-denies**
   - Where: `crates/atrium-core/src/policy/gate.rs:2084`, `recursive_reader(&x.eff) || tree_walker(&x.eff)` marks contents as read; line 2125 calls the non-following walk; lines 1832–1835 hard-deny credential hard links encountered there.
   - Case: existing `H/.ssh/id_rsa`; hard link `H/proj/h` to it; ordinary archive `H/plain.tar`. Run `tar -tf plain.tar proj`.
   - Result: **Deny / Deny / Deny**; should retain `dev`’s **Ask / Allow / Allow**. Tar lists matching members of `plain.tar`; it does not read the live `proj` directory. The added hard-link walk nevertheless scans that directory and finds `h`. This violates the brief’s prohibition on new hard denies for ordinary lines.
   - Fix: determine whether the archive operation reads filesystem operands before scanning their contents.

## Suspected, not shown

- No infinite link loop is demonstrated. Finding 5 concerns excessive finite work.
- Race scope remains as in earlier reviews: for Home’s initial Ask on `cat proj/safe*`, swapping a matched link to an SSH key before redemption is rechecked at `gate.rs:2446`, `self.evaluate(check, &others, preset).await?`, and traces to Deny. A swap after final evaluation and before execution remains outside this resolver task’s atomicity scope.

## Test gaps

- Add regressions for findings 1–6, including both budget cases.
- Earlier verdict reproductions and handoff corrections are addressed. The separate bulk cap remains at `gate.rs:1775`, `BULK_BUDGET: usize = 65536`; archive walkers no longer prune those directories.
- Ordinary linked-folder rows remain **Allow / Allow / Allow**: `M1-32-link-globs.rs:281–282`, `"ls proj_clean/to_other"` and `"cat proj_clean/to_other"`.
- K5 remains correctly **Deny / Deny / Deny** under D-125: `adversarial_m1_18.rs:1030`, `("cwd=proj|rg -L pat", ["Deny", "Deny", "Deny"])`.
- No owned-path violation shown. Resolver duplication is removed (`gate.rs:72`, `real_path(p)`); duplicated traversal remains in finding 5.