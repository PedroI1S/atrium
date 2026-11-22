## Verdict

**Request changes.** Open findings prevent approval under D-120.

Reviewed `git diff a1f0174...HEAD`. Changes stay within P2’s owned paths, preserve P0’s signatures and types, and do not wire anything into `SessionGate`.

## Findings

Locations below are under `crates/atrium-core/src/policy/v4/paths/`, unless stated otherwise.

1. **High — Later exhaustion discards earlier hits.** `command.rs:737–750`  
   The collectors propagate later failures with `?`, losing their accumulated outcomes. For example, resolve `["~/.ssh/id_rsa", "ordinary"]` with enough compute to establish the first HardDenyHit but insufficient compute for the second path: the function returns only `Err(Exhausted)`. This violates §8’s requirement that an earlier hit stand. **Fix:** retain partial results and append an Exhausted outcome when subsequent work exhausts the budget; apply this throughout the command, subject and recursive collectors.

2. **High — The hard-link walk bypasses S8 exemptions.** `recursive.rs:1365–1380`  
   This explains the reported `section10_row_16_s8_aws_hard_deny_config_and_home_exempt` failure. Although S8 exempts `~/.config`, the later walk hard-denies its ordinary, singly linked `gh/hosts.yml` by identity. It likewise reintroduces hits under home and its ancestors. **The implementation is wrong; the test is correct** against §5, §10 row 16 and §11 item 12. **Fix:** preserve the exemptions throughout recursive evaluation; the hard-link pass must not turn ordinary protected descendants into hits that override those exemptions.

3. **High — Filters silently exclude non-UTF-8 names.** `recursive.rs:655–660`  
   Plant the fake symlink `b"/h/u/proj/key\xff" → b"/h/u/.ssh/id_rsa"` and call `Glob(paths=["proj"], glob="key*")`. `matches_entry` returns false before matching, so the selected alias is never resolved and the result is Clear. §5 and §10 row 23 require a hit. **Fix:** match raw filesystem paths using the matcher’s path interface; an encoding limitation must produce Unread rather than a false miss.

4. **High — S8 misses listed-fold credential spellings.** `recursive.rs:280–294`  
   On the case-sensitive fake, create only `/h/u/.aws/CREDENTIALS`. A recursive read rooted at `~/.aws`, including one filtered to `*.rs`, fails S8 because it probes only lowercase `credentials`. The builder also misses that spelling, so the hard-link pass does not recover the hit. §5’s listed folds and S8 require HardDeny. **Fix:** establish existence across protected listed-fold spellings within the filesystem budgets.

5. **High — Browser S8 traversal silently stops after three directory levels.** `recursive.rs:473–475`  
   With `/h/u/.config/google-chrome/a/b/c/Cookies` present, a recursive read of `~/.config/google-chrome` returns Clear: the browser scan never reaches `Cookies`. That filename is protected at that location, and the contract provides no special three-level exception. **Fix:** traverse within the frozen walk limits and emit Unread when a limit prevents completion.

6. **High — Dangling protected entries outside `.ssh` lose their own protection.** `sensitive.rs:992–1001`  
   Create `~/.aws/credentials → /tmp/missing`, with the destination absent. The builder’s `stat` drops the entry; literal resolution retains its symlink identity but returns `/tmp/missing` as the canonical name. Neither then matches the protected set, producing Clear. §5 requires dangling entries to count under their own names. **Fix:** retain the identities of dangling named protected entries with `lstat`, including stores and Atrium paths, rather than handling this only inside `.ssh`.

7. **High — Write-only protected identities are deliberately omitted.** `sensitive.rs:1006–1040`, `sensitive.rs:1070–1075`; handoff §4  
   Hard-link an ordinary alias to `~/.ssh/config` or an Atrium protected file, then write through that alias. P2 produces Clear plus the multiply-linked flag, yielding Ask instead of the required HardDeny. The handoff acknowledges this as “non-blocking,” but §5 and D-120 provide no such exception. **Fix:** implement access-aware identity protection in the resolver-backed flow. If the frozen interface prevents that, file the required concrete interface change request instead of deferring the defect to P5.

8. **High — Canonicalization can erase a listed protected spelling.** `sensitive.rs:888–890`  
   On a case-sensitive fake, create `.SSH/ID_RSA` as a symlink to an ordinary file, with no lowercase `.ssh` tree. Resolution replaces the protected spelling with the ordinary canonical name, while the builder never records the uppercase member’s identity. Checking the target returns Clear. §5 explicitly protects listed-fold spellings in addition to identity. **Fix:** preserve/check the named protected spelling as well as the resolved target, or establish its identity before canonicalization erases it.

9. **Medium — Unicode normalization handling is incomplete.** `sensitive.rs:349–467`  
   Configure an Atrium data root containing `ǻ` (U+01FB), then check its NFD spelling `a\u{030A}\u{0301}` on the case-sensitive fake. U+01FB is absent from the handwritten decomposition table, so the result is Clear rather than Unread under §11 item 13. **Fix:** use complete canonical normalization and combining-class data; do not restrict the contract to the table’s selected characters.

10. **High — Global filesystem-cap failures become Clear in the hard-link walk.** `recursive.rs:1338`, `recursive.rs:1402`  
    Resolve an ordinary project root, then evaluate it with zero remaining directory opens, no filter and no followed-link walk. `walk_hard_links` swallows `CapReached`, and the recursive result becomes Clear. §8 exempts only the hard-link walk’s **local cutoff**; global `FsCaps` failures must be Unread. **Fix:** preserve the local-cutoff exception while recording `WalkCapReached` for resolver cap failures.

11. **High — Budget accounting omits work and charges allocations after they occur.** `sensitive.rs:888–934`; `resolver.rs:167–169`, `185–187`, `202–209`  
    A Clear `check_target` can succeed with zero allocation budget even though folding and cache preparation allocate. Separately, real `read_link` and `canonicalize` allocate their returned `PathBuf` before charging its size; `read_dir` advances the iterator and allocates the filename before the corresponding charges. These violate §8 and the frozen resolver contract. **Fix:** precharge all owned work and allocations, reserve filesystem output buffers before use, and charge entry reads before advancing the iterator.

12. **Medium — Unclosed brackets trigger repeated suffix scans.** `glob.rs:101–110`, `glob.rs:210–230`  
    An unquoted component containing 8,192 `[` bytes repeatedly scans the remaining suffix for a closing bracket. It performs quadratic parsing and exhausts the default compute budget despite being well below `MAX_BYTES`. §8 explicitly prohibits rescanning the remainder. **Fix:** determine bracket boundaries once, or consume an established unclosed remainder without restarting the scan at each byte.

13. **High — Cwd state ignores nested-shell scope.** `command.rs:100–105`, `command.rs:479–486`  
    The resolver uses one mutable cwd and never examines `parent_unit` or `depth`. Consequently, a modeled child’s `cd` can affect later outer units, or an appended child can inherit a cwd established after its parent invocation. For example, the child read in `cd sub; sh -c 'cat key'; cd other` must resolve at `sub`, regardless of where child units appear in the flattened vector. **Fix:** resolve the execution tree with each child’s entry cwd and isolate child directory changes from its parent.

14. **Medium — The carried `**/*.rs` test retains the wrong verdict.** `p2_tests.rs:1781–1796`; corresponding handoff disposition  
    This explains the reported `carried_m1_30_absent_targets_and_search_filter_probes` failure. Dev’s retained ASK_TREE directory-prefix check matches the directory prefix `**` against `.aws/sso/cache`, producing a StoreHit independently of the filename suffix. **The test is wrong; the implementation’s StoreHit is correct** under §5’s retained store-probe behavior and §11’s unchanged probe protection. **Fix:** assert StoreHit, retain the no-HardDeny assertion, and correct the handoff’s claimed Clear verdict. Do not restore M1-30’s dropped suffix-reasoning matcher.

## Test gaps

The listed §10 rows have named tests, but the boundary coverage misses the failing cases above. Add regressions for hit preservation after exhaustion, exempt-root identity walks, non-UTF-8 filter matches, folded S8 entries, deeper browser credentials, dangling protected entries outside `.ssh`, write-only aliases and nested cwd scopes.

Budget tests should exercise each global filesystem cap and small nonzero allocation budgets. Unicode tests need independent conformance cases; the normalization fake currently reuses the production folding function.

Real-filesystem tests use a temporary fake home. I modified no files and ran no tests; the two reported failures are supported by the supplied verification log, and the additional cases above are source-traced.