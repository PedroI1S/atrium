## Verdict

**Request changes.** New defects remain, so D-120’s approval gate is not met.

Reviewed `git diff b50f726...HEAD`. P2 preserves P0’s amended interface, stays within its owned paths, and does not wire anything into `SessionGate`. The previously reported reproductions are addressed, subject to the accepted CR-P2-2 work. Its two acknowledged failing cases are excluded below.

## Findings

All locations are under `crates/atrium-core/src/policy/v4/paths/`. Cases below are source-traced; no repository tests were run.

1. **High — Browser profile symlinks silently lose protection.**  
   **Location:** [sensitive.rs:1895](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:1895).  
   **Failing case:** create `~/.config/google-chrome/Default → /tmp/profile`, with `/tmp/profile/Cookies` present. The collector skips `Default` because it is a symlink whose basename is not a browser credential filename. Reading `~/.config/google-chrome/Default/Cookies` canonicalizes outside the browser root and returns Clear, with no collection gap. This loses §5’s protected spelling and identity check.  
   **Fix:** inspect symlinked browser directories and collect their credential members within the contractual budgets, recording Unread when traversal cannot finish.

2. **High — `Through` gaps miss singly linked targets of undiscovered symlinks.**  
   **Locations:** [sensitive.rs:1856](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:1856), [sensitive.rs:1483](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:1483).  
   **Failing case:** put `~/.gnupg/key → /tmp/token`, then make `.gnupg` unlistable but searchable. Collection emits `Through(gnupg_identity)`. A direct read of `/tmp/token` is Clear even after `apply_identity_gaps`: its regular file has one hard link and carries no `.gnupg` ancestor identity. Successful collection would have protected that target identity. Incomplete collection must therefore produce Unread.  
   **Fix:** scope gaps to include external targets of potentially undiscovered symlinks. Moving the existing scope unchanged into `ProtectedSet` will not fix this case.

3. **High — Cached collection failures retain the first access scope.**  
   **Location:** [sensitive.rs:1697](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:1697).  
   **Failing case:** configure `/h/u/.azure/atrium` as write-protected; make `.azure` unlistable but searchable; place `msal_token_cache.json → /tmp/token` there, with `accesstokens.json` absent. Protected-path discovery first caches the listing failure with `write_only=true`. Later store discovery reuses cached `None` without recording a read-applicable gap. Reading `/tmp/token` remains Clear after gap application.  
   **Fix:** cache the failure reason and register it for each consuming access scope, or widen the cached gap when a read-sensitive collection reuses it.

4. **High — Folded discovery stops at the exact home spelling.**  
   **Location:** [sensitive.rs:2176](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:2176).  
   **Failing case:** on the case-sensitive fake, use home `/h/u`, create a distinct `/H/U/.aws/credentials → /tmp/token`, and read `/H/U/.aws/credentials`. Its written name is protected by the listed lowercase fold. Discovery searches only beneath `/h/u`, however, and resolution erases the protected spelling; the result is Clear. Atrium paths below home have the same anchoring problem.  
   **Fix:** cover listed-fold variants of the home components too, or preserve the protected written spelling through canonicalization. Incomplete discovery must remain Unread.

5. **High — Linux canonicalization bypasses the link-read cap.**  
   **Location:** [resolver.rs:241](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/resolver.rs:241).  
   **Failing case:** call `FsResolver::canonicalize` on an existing temporary file with zero LinkReads and an available Canonicalization. It executes `readlinkat_raw("/proc/self/fd/N", …)` and succeeds without charging LinkReads. This violates §8’s requirement to charge each filesystem operation before execution.  
   **Fix:** pass `FsCaps` into this helper and charge the auxiliary readlink before calling it. Update the regression that currently requires only the canonicalization counter to change.

6. **High — Allocation accounting still permits uncharged allocations.**  
   **Locations:** [resolve.rs:187](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/resolve.rs:187), [sensitive.rs:1715](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:1715).  
   **Failing case:** `resolve_literal(b"x\0", …)` with sufficient compute and zero allocation budget returns an allocated `Unread(UnresolvablePath)` instead of allocation Exhausted. Separately, the collector’s first directory-cache insertion deep-clones every entry and name while charging only the directory key and `Option<Vec<DirEntry>>` header.  
   **Fix:** precharge error-path copies and the complete cache clone, including vector storage and names. Audit the remaining structural buffers against §8; charging outcome-list growth alone does not cover them.

7. **High — Allocation exhaustion discards an established `TooLarge` result.**  
   **Locations:** [command.rs:1221](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/command.rs:1221), [sensitive.rs:738](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:738).  
   **Failing case:** call `resolve_subject_paths` with `MAX_USES + 1` paths, sufficient compute and zero allocation budget. P2 detects `TooLarge(Uses)`, but `push_charged` drops it when its slot cannot be allocated. The function returns allocation Exhausted, changing the required Deny into Ask.  
   **Fix:** preserve structural-limit decisions across budget failures, including every `TooLarge` insertion. §1 places TooLarge above Exhausted.

8. **Medium — `mv` treats a directory destination as a directory source.**  
   **Location:** [command.rs:525](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/command.rs:525).  
   **Failing case:** resolve `mv ordinary_file ordinary_directory` with the annex-prescribed all-`Path`, ReadWrite effects for derived-destination writers. P2 tests both operands as sources, so the destination directory produces `Unread(DerivedDestinations)`. The table’s trigger is a directory **source**. The existing test hides this mismatch by assigning `DestinationRoot` to the destination.  
   **Fix:** distinguish source positions consistently with the frozen table, and test using its actual operand roles.

## Test gaps

Add regressions for these eight cases, especially external symlink targets after incomplete collection, cached failures reused across access modes, and TooLarge combined with allocation exhaustion.

The requested §10 rows and carried-work dispositions are present. Real-filesystem fixtures use temporary fake homes. The supplied verification at `5429523` is clean; these additional cases are uncovered. No files were modified.