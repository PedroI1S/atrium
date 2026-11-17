# Review: M1-32-link-globs
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-17 · round 3

## Verdict

**rework**: the previous reproductions are addressed, but four credential-link escapes and excessive aggregate filesystem work remain.

Verdicts below are code-traced at HEAD `5bec74e`; no probes, tests, or files were created. Triples mean **Home / Home with an unscoped Any allow rule / FullAccess**. Fixtures use home and cwd `H`, single-linked regular files, and disjoint Atrium paths.

## Findings

1. [blocker] **Relocating a credential store’s parent still defeats glob protection**
   - Where: `crates/atrium-core/src/policy/gate.rs:1496`, `"resolve_fs(par, 0).join(name)"`; line 1539, `"s.hit(&alias, role).is_some()"`.
   - Case: existing `H/config_store/gcloud/token`; `H/.config -> config_store`; `H/proj/key -> ../config_store/gcloud/token`. Run `cat proj/key*`, or call `Glob({"path":"proj","pattern":"key*"})`.
   - Result: shell **Ask / Allow / Allow**; Glob **Allow / Allow / Allow**. Both should be **Deny / Deny / Deny** under S11 and contract §9’s credential identity protection. The `.config/gcloud` tree’s real and “logical” paths both become `H/config_store/gcloud`; neither spelling reaches the approved `.config/gcloud` classification (`deny.rs:410`). The round-2 fix preserves the root’s final name but loses protected parent names. Naming `proj/key` literally receives the identity alias correction and is denied.
   - Fix: retain the complete protected spelling relative to the resolved home, including `.config`, when classifying expanded targets.

2. [blocker] **Non-UTF-8 glob prefixes are still changed before traversal**
   - Where: `crates/atrium-core/src/policy/gate.rs:858`, `".map(|c| c.as_os_str().to_string_lossy().to_string())"`; line 870, `"let prefix: PathBuf = comps[..first].iter().collect();"`.
   - Case: existing `H/.ssh/id_rsa`; a directory whose name bytes are `store\xff`, containing `key -> ../.ssh/id_rsa`; `H/proj -> store\xff`. Run `cat proj/key*`, or call `Glob({"path":"proj","pattern":"key*"})`.
   - Result: shell **Ask / Allow / Allow**; Glob **Allow / Allow / Allow**. Both should be **Deny / Deny / Deny** under S11. Resolving `proj` introduces the real non-UTF-8 prefix, but expansion rebuilds it as `store�`. Reading that nonexistent directory produces an empty, complete expansion (`gate.rs:919`). Round-2 finding 2’s entry-name fix does not cover prefixes.
   - Fix: preserve literal components and the prefix as `OsString`/`PathBuf`; convert only pattern components for pattern compilation.

3. [blocker] **Expanded targets miss SSH files symlinked out of `.ssh`**
   - Where: `crates/atrium-core/src/policy/gate.rs:280`, `"ssh.members"` participates in the original operand identity map; line 349 supplies only `"&ssh.linked_dirs"` to the tree check; line 1805 checks expanded targets using `"in_trees(&real_path(f), ...)"`.
   - Case: existing `H/vault/key`; `H/.ssh/id_rsa -> ../vault/key`; `H/proj/key -> ../vault/key`. Run `cat proj/key*`, or call `Glob({"path":"proj","pattern":"key*"})`.
   - Result: shell **Ask / Allow / Allow**; Glob **Allow / Allow / Allow**. Both should be **Deny / Deny / Deny** under S11 and contract §9’s identity protection. `vault/key` is an identified SSH credential, but lies outside every checked tree. There is no linked SSH directory in this fixture. Literal `cat proj/key` receives the SSH-member identity alias and is denied; glob matches bypass that correction.
   - Fix: apply the existing credential identity alias check to expanded targets, including individually linked SSH members.

4. [major] **Literal file-tool paths containing metacharacters bypass the dangling-link correction**
   - Where: `crates/atrium-core/src/policy/gate.rs:1962`, `"!u.word.contains(['*', '?', '[', '{', '}', '$', '`'])"`; the older resolver still uses `"normalize(&target)"` at line 84.
   - Case: existing directory `H/.ssh/sub`, absent `H/.ssh/new_key`; `H/proj/dir -> ../.ssh/sub`; `H/proj/dangling-key[1] -> dir/../new_key`. Call `Read({"file_path":"proj/dangling-key[1]"})`, normalized to Read, that literal path, `glob: None`, `recursive: false`.
   - Result: **Allow / Allow / Allow**, rather than **Deny / Deny / Deny**. The old resolver produces `H/proj/new_key`; the new correction skips the original filename because it contains `[`. Filesystem-ordered resolution reaches `H/.ssh/new_key`. As in round-2 finding 5, S11 protects dangling credential targets regardless of existence. Brackets are literal filename characters for `Read`.
   - Fix: always re-resolve literal file-tool paths when `glob` is absent; retain the metacharacter guard for shell patterns.

5. [major] **Per-walk caps permit billions of filesystem operations in one check**
   - Where: `crates/atrium-core/src/policy/gate.rs:1864`, `"resolved.to_vec()"`; line 1869, `"for gv in exps"`; line 1890, `"for r in &roots"`; line 871 restarts `"let mut seen = 0usize"` for every expansion.
   - Case: `H/proj` contains exactly 4,096 ordinary files named `ka0000` through `ka4095`. Submit a normalized Search/Glob subject with `paths` equal to `"proj"` repeated 4,096 times, `recursive: false`, and glob `k{a,a,...,a}*` with exactly 256 `a` alternatives.
   - Result: eventually **Ask / Ask / Ask**, which is the correct verdict for exhausted aggregate checking capacity. However, the tree check first performs **1,048,576 identical expansions**, visiting **4,294,967,296 directory entries** and resolving their matches. Each expansion individually fits its cap. This is a code-derived operation count, not a measured runtime. The brief requires bounded link resolution; restarting the budget across this product defeats its practical purpose.
   - Fix: deduplicate roots and expanded patterns, and share a filesystem-work budget across the complete check, returning `Reach::Maybe` when exhausted.

6. [nit] **The handoff still claims archive walkers prune these directories**
   - Where: `docs/agents/handoffs/M1-32-link-globs.md:18`, `"walkers retain .git/node_modules/target pruning"`.
   - Case: existing `H/.ssh/id_rsa`; `H/proj/node_modules/key -> ../../.ssh/id_rsa`; `tar --dereference -cf out.tar proj`.
   - Result: **Deny / Deny / Deny**, correctly under S11 and contract §9. HEAD walks the directory: `gate.rs:1630`, `"_ => PruneDirs::None"`. The summary contradicts both the implementation and the handoff’s corrected explanation at line 57.
   - Fix: replace the stale pruning statement with the separate-budget behavior.

## Suspected, not shown

- No infinite link loop is shown. Individual glob walks retain the 4,096-entry/depth-12 limits; following walks retain depth 16 and separate 4,096/65,536-entry budgets (`gate.rs:854`, `855`, `1653`, `1654`, `1671`). Finding 5 concerns aggregate repetition.
- A link created or swapped before approval redemption is in scope: `gate.rs:2256`, `"self.evaluate(check, &others, preset).await?"`, rechecks it. For Home’s initial Ask on `cat proj/safe*`, replacing an ordinary match with an SSH-key link before redemption traces to Deny. A swap after final evaluation and before execution remains a general TOCTOU limit outside this brief’s resolver work.
- No owned-path violation is shown. Resolver duplication remains between `resolve_fs` and `real_path`; finding 4 demonstrates a remaining consequence.

## Test gaps

- Add regressions for findings 1–4 and an aggregate-budget check for finding 5.
- The original round-1 and round-2 reproductions are addressed. Round-2 finding 3’s larger bulk budget is implemented; archive walkers no longer prune those directories.
- Ordinary linked-folder rows remain **Allow / Allow / Allow**: `M1-32-link-globs.rs:281–282`, `"ls proj_clean/to_other"` and `"cat proj_clean/to_other"`.
- K5 remains correctly hard-denied under D-125: `adversarial_m1_18.rs:1030`, `("cwd=proj|rg -L pat", ["Deny", "Deny", "Deny"])`.
- Verification results are those supplied by the orchestrator; no additional tests were run.