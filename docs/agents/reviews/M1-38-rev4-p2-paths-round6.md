## Verdict

**Request changes.**

Reviewed `git diff a5b9b3b...HEAD`. The six round-5 reproductions and both CR-P2-3 standalone cases are addressed. P2 preserves P0’s interface and remains unwired from `SessionGate`.

The pure `check_target` alias limit has no identified contract-defined failing flow; it is not a finding.

The findings below are source-traced. No files were modified or tests run. The verification log currently reports fmt/clippy passing and workspace `rc=0`; repeated core runs remain underway.

## Findings

1. **High — Later exhaustion discards an established written-name hit.**  
   **Location:** [sensitive.rs:1471](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:1471).  
   **Failing case:** use a protected set with home `/h/u`, empty identity/data lists and `FoldRules::REV4`. Resolve `~/.ssh/id_rsa` with ample budget and warm the sensitive cache. Call `check_recursive_root` with `ComputeBudget::new(53, 8_388_608)`. Written-name checking establishes HardDeny using exactly 53 compute units. The subsequent canonical check exhausts, and `?` discards the hit; the recursive entry point returns `Err(Exhausted)`. This violates §1 and §8.  
   **Fix:** retain established records before further fallible checks, returning the hit beside exhaustion in collectors.

2. **High — Alias discovery silently treats a filesystem cap as absence.**  
   **Location:** [sensitive.rs:1644](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:1644).  
   **Failing case:** create fake `~/.ssh → /tmp/keys`, with an empty target directory, and build the protected set using separate ample caps. Resolve the nonrecursive Read `/tmp/keys/id_rsa` with `FsCaps::new(1000, 1000, 1000, 1000, 1)`. Literal resolution consumes the canonicalization allowance. Every alias-root canonicalization then returns `CapReached`, which this branch ignores. The call returns Clear, losing protection that `dev`’s root alias supplies. The builder recorded no gap because its phase succeeded.  
   **Fix:** preserve classification-phase cap and I/O failures as Unread; do not cache failed alias discovery as a complete table.

3. **Medium — Alias spellings skip unlisted-equivalence detection.**  
   **Location:** [sensitive.rs:1733](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:1733).  
   **Failing case:** create `~/.config/google-chrome → /tmp/profile` and singly linked `/tmp/profile/log\u{0131}ns.json` (`ı`, dotless i). Reading that external path returns Clear. The alias rewrite produces the browser spelling, but checks only listed folds. Its filename matches protected `logins.json` under the implementation’s unlisted case fold, requiring Unread under §5 and §11 item 13.  
   **Fix:** apply unlisted-equivalence checking to rewritten alias spellings, preserving definitive hits.

4. **Medium — MSAL’s containing directory becomes a wholesale store root.**  
   **Location:** [sensitive.rs:1626](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:1626).  
   **Failing case:** create ordinary `~/.azure/README`, no token caches, and `proj/readme → ~/.azure/README`. An unfiltered `rg -L token proj` produces `StoreHit("azure-msal-cache")`: `RootAliases::reached` treats every descendant of `.azure` as a store. README matches neither a protected name nor identity nor probe. This contradicts §5 and §11 item 15’s Clear verdict for a followed search reaching no protected target.  
   **Fix:** distinguish the directory used for MSAL name rewriting from actual store roots used by followed-link reachability.

5. **Medium — Alias rewriting undercharges compute for copied bytes.**  
   **Location:** [sensitive.rs:1677](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:1677).  
   **Failing case:** cache a single browser-root alias from a spelling exceeding 1,000 bytes to `/p`. Rewriting `/p/Cookies` with 11 compute units and ample allocation budget succeeds while copying the entire long spelling. The charge depends only on the short canonical path, violating §8’s charge-before-copy requirement.  
   **Fix:** precharge the rewritten spelling’s copied bytes before constructing it.

## Test gaps

Add regressions for these five cases, especially exhaustion between written and canonical checks and cap failure during alias discovery.

The previous temporary-parent entry-count dependency is removed: the real-filesystem tests separate builder resources from assertion resources and continue using temporary fake homes.