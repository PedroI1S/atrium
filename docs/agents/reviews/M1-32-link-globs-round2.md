# Review: M1-32-link-globs
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-17 · round 2

## Verdict
**rework**: the round-1 reproductions are addressed, but credential-link escapes and incomplete-walk fail-opens remain.

Verdicts below are derived from HEAD `0c80606`; no probes or files were created. Triples mean **Home / Home with an unscoped Any allow rule / FullAccess**. Fixtures use home and cwd `H`, ordinary files with one hard link, and no unrelated links.

## Findings

1. [blocker] **Relocated credential stores lose protection**
   - Where: `crates/atrium-core/src/policy/gate.rs:1489`, `"path: resolve_fs(r, 0)"`; `gate.rs:1520`, `"(names_dir && real.is_dir()) || s.hit(real, role).is_some()"`.
   - Case: existing `H/vault/id_rsa`; `H/.ssh -> vault`; `H/proj/key -> ../vault/id_rsa`. Run `cat proj/key*`, or call Search/Glob with paths `["proj"]`, glob `"key*"`, recursive `false`.
   - Result: shell **Ask / Allow / Allow**; Glob **Allow / Allow / Allow**. Both must be **Deny / Deny / Deny** under S11 and contract §9’s credential identity protection. The tree resolves to `H/vault`, but `Sensitive::hit` still classifies credentials by their logical home-relative names (`deny.rs:523`, `"under(rest, \".ssh\")"`). Expanded targets never receive the identity alias correction applied to original operands. This also weakens the previous blanket tree check.
   - Fix: retain each protected root’s logical spelling and classify expanded targets through that spelling, preserving the public-file exceptions.

2. [blocker] **Glob expansion changes non-UTF-8 filenames before resolving them**
   - Where: `crates/atrium-core/src/policy/gate.rs:938`, `"let name = e.file_name().to_string_lossy().to_string();"`; `gate.rs:942`, `"let path = dir.join(&name);"`.
   - Case: existing `H/.ssh/id_rsa`; a symlink in `H/proj` whose filename bytes are `[b'k', 0xff]`, targeting `../.ssh/id_rsa`. Run `cat proj/k*`, or call Search/Glob with paths `["proj"]`, glob `"k*"`, recursive `false`.
   - Result: shell **Ask / Allow / Allow**; Glob **Allow / Allow / Allow**. Both must be **Deny / Deny / Deny** under S11. Matching succeeds against the lossy name, but the resolver receives `H/proj/k�`, a different, nonexistent path. No uncertainty is recorded.
   - Fix: preserve the directory entry’s actual `PathBuf`—at minimum use `e.path()`—and match without replacing filename bytes.

3. [blocker] **Following archive walkers still skip directories they read**
   - Where: `crates/atrium-core/src/policy/gate.rs:1612`, `"_ => PruneDirs::VcsAndBuild"`; `gate.rs:1621`, `"(PruneDirs::VcsAndBuild, Some(\".git\" | \"node_modules\" | \"target\")) => true"`.
   - Case: existing `H/.ssh/id_rsa`; `H/proj/node_modules/key -> ../../.ssh/id_rsa`. Run `tar --dereference -cf out.tar proj`. The same case works with `.git` or `target` as the containing directory.
   - Result: **Ask / Allow / Allow**, rather than **Deny / Deny / Deny**. The gate recognizes tar as a walker and `--dereference` as following links, but discards the directory containing the key. Tar archives the credential. This remains within the brief’s following-link helpers and “all tools with follow flags” coverage, and violates contract §9. Round-1 finding 4 is fixed for its grep reproduction but retains this sibling gap.
   - Fix: prune only directories the invocation actually excludes; tar’s dereferencing walk must not use this default exclusion list.

4. [major] **Following searches treat unreadable directories as fully checked**
   - Where: `crates/atrium-core/src/policy/gate.rs:1645`, `"let Ok(rd) = std::fs::read_dir(&dir) else {"`, followed at line 1646 by `"continue;"`. Line 1648 also uses `"rd.flatten()"`.
   - Case: existing `H/.ssh/id_rsa`; `H/proj/closed/key -> ../../.ssh/id_rsa`; `closed` has mode `000`, evaluated as its non-root owner. Run `grep -R secret proj`.
   - Result: **Ask / Allow / Allow**, rather than **Ask / Ask / Ask**. The failed enumeration disappears, and the walker returns `Reach::No`. This violates the brief’s explicit requirement to fail closed when something cannot be read fully. Round-1 finding 6’s glob-expansion fix does not cover this walker.
   - Fix: propagate directory-read, entry and file-type failures as `Reach::Maybe`, while retaining any definite credential hit.

5. [major] **The dangling-link resolver gap remains reachable through file tools**
   - Where: `crates/atrium-core/src/policy/gate.rs:1700`, `"return resolve_fs(&abs, 0);"`; `gate.rs:84`, `"break resolve_fs(&normalize(&target), hops + 1);"`.
   - Case: existing directories `H/.ssh/sub` and `H/proj/sub`; absent `H/.ssh/new_key`. Links:
     - `H/proj/sub/dir -> ../../.ssh/sub`
     - `H/proj/sub/dangling-key -> dir/../new_key`

     Call Read on `proj/sub/dangling-key`, or Search/Glob with paths `["proj"]`, glob `"sub/dangling-key"`, recursive `true`.
   - Result: **Allow / Allow / Allow**, rather than **Deny / Deny / Deny**. Filesystem resolution reaches `H/.ssh/new_key`; lexical normalization instead produces `H/proj/sub/new_key`. The new component-ordered `real_path` receives the already-wrong path. S11 includes dangling links, and contract §9 protects named credential paths regardless of existence.
   - Fix: apply component-ordered resolution before discarding the original literal target. The unchanged resolver is outside the owned range and is correctly escalated in the handoff, but round-1 finding 3 is only partially resolved. Consolidating the two resolvers requires ownership coordination.

6. [nit] **The handoff summary misstates entry-cap behavior**
   - Where: `docs/agents/handoffs/M1-32-link-globs.md:14`, `"sets truncated = true on MAX_DEPTH/MAX_ENTRIES while preserving already-found matches"`.
   - Case: the existing over-budget fixture and `cat big_dir/*/id_rsa`.
   - Result: **Ask / Ask / Ask**, correctly. However, `gate.rs:936`, `"return None;"`, discards matches on entry-budget exhaustion, as the handoff’s later rework section explains.
   - Fix: update the summary to distinguish depth/I/O truncation from entry-budget exhaustion.

## Suspected, not shown

- **Aggregate cost:** individual glob walks have 4096-entry and depth-12 limits (`gate.rs:854–855`); following walks have 4096-entry and depth-16 limits (`gate.rs:1636–1637`); resolution has 40 hops (`gate.rs:52`). Repeated roots, brace alternatives and protection passes restart budgets. No excessive aggregate latency or unbounded loop was demonstrated.
- **Race scope:** for an initial Home Ask on `cat proj/safe*`, swapping a matched link to an existing SSH key before redemption is in scope and rechecked at `gate.rs:2170`, `"let e = self.evaluate(check, &others, preset).await?;"`. A swap after the final evaluation and before execution remains a general TOCTOU limit outside this brief’s resolver work.

## Test gaps

- Add regressions for findings 1–5, including a relocated `.ssh`, a non-UTF-8 link name, dereferencing tar under the pruned directories, an unreadable following-search subtree, and literal file-tool dangling targets.
- The seven original round-1 reproductions are addressed. Ordinary `ls proj_clean/to_other` and `cat proj_clean/to_other` retain **Allow / Allow / Allow** in `M1-32-link-globs.rs:281–282`.
- K5 remains **Deny / Deny / Deny**, correctly under D-125: `adversarial_m1_18.rs:1030`, `"(\"cwd=proj|rg -L pat\", [\"Deny\", \"Deny\", \"Deny\"])"`.
- No owned-path violation is shown. No additional tests were run; verification results are those supplied by the orchestrator.