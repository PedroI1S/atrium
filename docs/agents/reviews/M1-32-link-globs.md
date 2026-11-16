# Review: M1-32-link-globs
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-16

## Verdict
**rework**: credential-link cases still escape hard deny, and the new file-tool handling introduces an ordinary-read false deny.

Verdicts below are derived from HEAD (`229ffb8`), without creating probes or changing files. Triples mean **Home / Home with an unscoped Any allow rule / FullAccess**. All fixtures use a fake home `H`, cwd `H`, ordinary files with one hard link, and no unrelated links.

## Findings

1. [blocker] **File-tool globs inherit shell matching semantics**
   - Where: `crates/atrium-core/src/policy/gate.rs:1557`, `"expand_targets(&p, &mut maybe)"`; `gate.rs:862`, `"if name.starts_with('.') && !c.starts_with('.')"`; `gate.rs:1549`, `"raw_glob_pattern(&h, r, home)"`.
   - Case:
     - Existing `H/.ssh/id_rsa`; `H/proj/.key -> ../.ssh/id_rsa`. Call `Glob({"path":"proj","pattern":"*"})`, normalized to Search, paths `["proj"]`, glob `"*"`, recursive `true`.
     - Existing `H/.ssh/id_rsa`; `H/proj/sub/key -> ../../.ssh/id_rsa`. Call `Grep({"path":"proj","pattern":"secret","glob":"key*"})`, normalized to Search, paths `["proj"]`, glob `"key*"`, recursive `true`.
   - Result: **Allow / Allow / Allow**, rather than **Deny / Deny / Deny**. The first expansion skips the hidden link; the second checks only `proj/key*`, without applying the recursive basename filter below `proj`. `recursive` only changes `first_tree_hit`’s directory interpretation. File-tool matching deliberately differs from shell matching: `crates/atrium-agents/src/claude/tools.rs:444` says `"the tool's glob has neither rule"` concerning shell dot handling; `crates/atrium-core/src/policy/deny.rs:1483` uses `"recursive || !rel.contains('/')"`. S11 requires matching credential links in file-tool globs.
   - Fix: give file-tool expansion its own matching mode, honoring hidden names and recursive filters while retaining traversal caps.

2. [major] **Dangling links to absent credential files lose protection**
   - Where: `crates/atrium-core/src/policy/gate.rs:1338`, `".filter_map(|r| std::fs::canonicalize(r).ok())"`.
   - Case: `H/.ssh/id_rsa` exists, `H/.npmrc` does not, and `H/proj/dangling-npm -> ../.npmrc`. Input: `printf x > proj/dangling-*`.
   - Result: **Ask / Allow / Allow**, rather than **Deny / Deny / Deny**. Expansion finds the dangling link and `real_path` resolves it to `H/.npmrc`, but `trees_of` has discarded that protected file because it does not exist. The redirect can create `.npmrc`. Naming `H/.npmrc` directly is hard-denied regardless of existence: contract §9 applies the hard-deny list to named paths, and S11 explicitly includes dangling links.
   - Fix: retain protected roots using deepest-existing-ancestor resolution instead of filtering out canonicalization failures.

3. [major] **Dangling targets normalize `..` before resolving intermediate links**
   - Where: `crates/atrium-core/src/policy/gate.rs:928`, `"return resolve_fs(&normalize(&target), 1);"`. The resolver repeats this at `gate.rs:84`, `"break resolve_fs(&normalize(&target), hops + 1);"`.
   - Case: `H/.ssh/sub` exists; `H/.ssh/new_key` does not. Create `H/proj/dir -> ../.ssh/sub` and `H/proj/dangling-key -> dir/../new_key`. Input: `printf x > proj/dangling-*`.
   - Result: **Ask / Allow / Allow**, rather than **Deny / Deny / Deny**. Lexical normalization turns the target into `H/proj/new_key`. Filesystem resolution instead follows `dir` first, then `..`, reaching `H/.ssh/new_key`; the redirect creates that protected file. This violates S11’s relative/dangling-link requirement and contract §9’s credential-store write deny.
   - Fix: resolve target components in filesystem order, applying `..` after resolving preceding links. The corresponding resolver change outside the owned range needs escalation.

4. [blocker] **Following searches silently skip directories the command reads**
   - Where: `crates/atrium-core/src/policy/gate.rs:1382`, `"Some(\".git\" | \"node_modules\" | \"target\")"`, followed by `gate.rs:1385`, `"continue;"`.
   - Case: existing `H/.ssh/id_rsa`; `H/proj/node_modules/key -> ../../.ssh/id_rsa`. Input: `grep -R secret proj`. The same case works with the containing directory named `.git` or `target`.
   - Result: **Ask / Allow / Allow**, rather than **Deny / Deny / Deny**. `grep -R` follows the link and reads the key; `links_below` skips its containing directory without marking uncertainty. Neither the lexical paths nor the remaining checks name the credential. This gap also exists on `dev`, but is within the brief’s recursive-search link scope and D-120.
   - Fix: make pruning depend on what the specific invocation actually excludes. Do not prune these directories for `grep -R`.

5. [major] **Public SSH files become hard-denied through file-tool globs**
   - Where: `crates/atrium-core/src/policy/gate.rs:1561`, `"first_tree_hit(&targets, trees, *recursive, write)"`; `gate.rs:1355`, `"real.starts_with(&t.path)"`.
   - Case: existing `H/.ssh/id_rsa.pub`; `H/proj/public-key -> ../.ssh/id_rsa.pub`. Call `Glob({"path":"proj","pattern":"public-*"})`.
   - Result: **Deny / Deny / Deny**, whereas `dev` returns **Allow / Allow / Allow** and those verdicts should remain. The new branch treats every descendant of the `.ssh` tree as a credential. The actual hard-deny matcher exempts public files at `crates/atrium-core/src/policy/deny.rs:527`, `"name.ends_with(\".pub\") || name.starts_with(\"known_hosts\")"`. Contract §9 preserves that approved list; the brief forbids new hard denies on ordinary calls.
   - Fix: apply the existing credential classification to expanded targets, including public-file and read-only SSH-config exceptions, rather than blanket tree membership.

6. [major] **Unreadable glob directories are treated as successfully checked**
   - Where: `crates/atrium-core/src/policy/gate.rs:853`, `"let Ok(rd) = std::fs::read_dir(&dir) else {"`, followed by `"continue;"`. `gate.rs:856` also uses `"rd.flatten()"`, discarding enumeration errors.
   - Case: existing `H/.ssh/id_rsa`; `H/proj/closed/key -> ../../.ssh/id_rsa`; `closed` has mode `000`, evaluated as its non-root owner. Search subject: paths `["proj"]`, glob `"closed/*"`, recursive `true`.
   - Result: **Allow / Allow / Allow**, rather than **Ask / Ask / Ask**. The failed directory read produces an empty expansion with `truncated == false`; no uncertainty reaches the caller. This violates the brief’s explicit instruction to fail closed when something cannot be read fully. It is a pre-existing helper gap now used by the added file-tool path.
   - Fix: propagate directory-read, entry and metadata errors as incomplete expansion; preserve any matches already found.

7. [minor] **The handoff still requests the already-decided K5 change**
   - Where: `docs/agents/handoffs/M1-32-link-globs.md:3`, `"K5 blocked (needs driver/orchestrator decision)"`; line 45, `"Decision needed:"`.
   - Case: cwd `H/proj`, containing a link into `H/.ssh`; input `rg -L pat`.
   - Result: **Deny / Deny / Deny** is preserved and correct under D-125. HEAD retains the explicit row at `crates/atrium-core/tests/adversarial_m1_18.rs:1030`, `"(\"cwd=proj|rg -L pat\", [\"Deny\", \"Deny\", \"Deny\"])"`. The handoff incorrectly describes this as unresolved work.
   - Fix: record K5 as closed under D-125 and remove the request to weaken its verdict.

## Suspected, not shown

- **Aggregate cost:** individual walks are bounded: `gate.rs:798` has `MAX_ENTRIES = 4096`, line 799 has `MAX_DEPTH = 12`; `links_below` uses 4096 entries and depth 16 at lines 1365–1366; resolution has `MAX_HOPS = 40` at line 52. Globstar’s visited set at line 838 prevents repeated canonical-directory traversal. No unbounded loop is shown. Budgets restart for each candidate, and the file-tool pass plus the later `a.uses` pass can repeat expansion; excessive aggregate latency has not been demonstrated.
- **Post-check race:** a swap before approval redemption is in scope and rechecked: `gate.rs:1880`, `"self.evaluate(check, &others, preset).await?"`. For `cat proj/safe*` in Home, swapping an ordinary matched link to an existing SSH key before redemption traces to Deny. Creation or swapping after the final evaluation and before execution remains a general TOCTOU limit; atomic execution is outside this brief’s stated resolver work.

## Test gaps

- Add regression cases for findings 1–6; current passing tests do not cover those shapes.
- Add explicit ordinary-folder rows for `ls proj_clean/to_other` and `cat proj_clean/to_other`: both trace to **Allow / Allow / Allow**. The existing ordinary test checks linked files and globbed file reads, but not these folder operands.
- No owned-path violation is shown: source edits are confined to `gate.rs` callers/helpers, with the permitted ignore removal and task-owned tests.
- I relied on the orchestrator’s supplied test results; no additional tests or probes were run.