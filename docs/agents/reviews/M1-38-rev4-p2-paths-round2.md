## Verdict

**Request changes.** D-120’s requirement of no open findings is not met.

Reviewed `git diff a1f0174...HEAD`. P2 stays within its owned paths, preserves P0’s interface, and does not wire anything into `SessionGate`.

Round-1’s reported cases for findings **1–6, 8, 10, 13 and 14** are addressed. Findings **9, 11 and 12** remain incomplete for the additional cases below.

For **finding 7**, resolver-backed flows now catch the tested write-only aliases, but the collector’s silent cutoff leaves another failing case. The standalone gap described in **CR-P2-1 is real**: pure `check_target` cannot recover write-only identities from the frozen `ProtectedSet`. That inherited finding remains open pending the interface decision; it is not relisted below.

## Findings

Locations are under `crates/atrium-core/src/policy/v4/paths/`.

1. **Medium — The exhaustion regression measures incompatible cache states.**  
   **Location:** `p2_tests.rs:2749`, assertion at `2765`.  
   **The test is wrong.** The baseline recursive check initializes `SharedSensitive`; the second check reuses it and costs **47 fewer compute units**. Giving it `total_rec_cost - 25` therefore leaves **22 units**, so no Exhausted outcome is required. This failure does not demonstrate lost hits.  
   **Fix:** warm the cache before measuring the baseline, then ensure the reduced budget actually exhausts after S8 establishes its hit.

2. **Medium — The Unicode regression incorrectly requires Clear in its combined fixture.**  
   **Location:** `p2_tests.rs:3327`, assertion at `3339`.  
   **The test is wrong.** Its protected set includes Tibetan U+0F43, outside the exact normalization table. Comparing that component with `á` invokes the conservative fallback. **§11 item 13 permits this Unread result**: it creates no hit and downgrades no identity or listed-fold hit.  
   **Fix:** expect Unread for this combined fixture, or isolate the `à` versus `á` Clear assertion in a protected set containing only covered components.

3. **High — Write-only identity collection silently omits deeper protected members.**  
   **Location:** `sensitive.rs:1515`, `1523`, `1574`.  
   **Failing case:** create `~/.config/atrium/a/b/c/d/e/f/g/h/key` and hard-link it to `proj/alias`. Writing `proj/alias` returns Clear plus the multiply-linked flag instead of `HardDenyHit("atrium-self")`: collection stops before visiting `h/key`, without recording Unread. The 1,024-entry cutoff has the same problem. These are identity-collection limits, outside §5’s special Clear exception for the recursive hard-link walk.  
   **Fix:** collect within the contractual budgets and report Unread whenever collection stops incomplete.

4. **High — New resolver metadata calls bypass the stat cap.**  
   **Location:** `resolver.rs:193`, `237`, `242`, `299`.  
   **Failing case:** with zero remaining Stats but available LinkReads/Canonicalizations and compute, `read_link` and `canonicalize` still execute their internal metadata calls and can succeed. The `DT_UNKNOWN` directory-entry fallback similarly calls `statat` without charging Stats. This violates §8’s precharging requirement.  
   **Fix:** charge the stat cap and compute before each auxiliary metadata operation.

5. **High — Canonicalization still charges expanded output after allocation.**  
   **Location:** `resolver.rs:243–254`.  
   **Failing case:** in a temporary fake home, let an intermediate short directory symlink lead to a much longer directory path. Canonicalizing `short/file` reserves only the input length because the final component is not a symlink. `std::fs::canonicalize` allocates the longer output before the additional allocation charge fails. Round-1 finding 11 therefore remains incomplete.  
   **Fix:** reserve a valid output bound before canonicalization, or use a canonicalizer whose buffer growth can be charged before allocation.

6. **Medium — The “covered” Unicode range still contains omitted canonical decompositions.**  
   **Location:** `sensitive.rs:541–546`.  
   **Failing case:** configure Atrium data at `/h/u/e\u{0341}/atrium`, then check `/h/u/é/atrium/db` on the case-sensitive fake. These components are canonically equivalent, but U+0341 is passed through unchanged and marked covered. The fallback consequently returns false, producing Clear instead of Unread. U+0344 supplies another case.  
   **Fix:** supply the missing canonical decompositions, or mark unsupported scalars uncovered so the conservative fallback applies.

7. **Medium — Nested unclosed brackets still cause quadratic rescanning.**  
   **Location:** `glob.rs:249–251`, with reparsing at `107`.  
   **Failing case:** one component containing `"[[:digit:]"` repeated **2,048 times** is only 20,480 bytes. Each unmatched outer bracket scans the remaining suffix, then parsing resumes at its inner bracket. Closing-bracket scans alone require **16,799,744 steps**, exceeding the default compute budget. The repeated-`[` regression does not cover this form.  
   **Fix:** retain bracket-boundary information so later inner parses cannot restart scans of an already examined suffix.

8. **High — Per-character lowercase differs from the frozen listed fold.**  
   **Location:** `sensitive.rs:195`.  
   **Failing case:** configure Atrium data at `/h/u/ος/atrium` and check `/h/u/ΟΣ/atrium/db` on a case-sensitive fake with distinct entries. Normative `str::to_lowercase()` maps `ΟΣ` to `ος` using final sigma. P2’s per-character conversion produces `οσ`, so a required listed-fold HardDeny becomes Unread.  
   **Fix:** apply string lowercase with its contextual rules, then the explicit listed substitutions, while preserving invalid byte segments.

9. **High — An existing lowercase browser root hides protected folded siblings.**  
   **Location:** `recursive.rs:358`.  
   **Failing case:** create an empty `~/library/application support/google/chrome` and a sibling spelling `~/library/APPLICATION SUPPORT/google/chrome/Cookies`. A recursive search rooted at `~/library`, filtered to `*.rs`, returns Clear: the exact-path fast return prevents discovery of the other listed-fold spelling, and the hard-link pass ignores the singly linked Cookies file. S8 requires HardDeny regardless of the filter.  
   **Fix:** inspect every matching listed-fold browser root before concluding that the containing root has no existing credential.

## Test gaps

Add regressions for the new cases above, including collector depth/entry limits, auxiliary Stats charges, expanded canonicalization buffers, U+0341/U+0344, contextual lowercase, duplicate folded browser roots, and nested unmatched brackets. Correct both deterministically failing tests while retaining their intended assertions.

The requested §10 rows have named tests, and the real-filesystem tests use temporary fake homes. I modified no files and ran no repository tests; the two supplied failures are confirmed by the verification log, and the additional findings are source-traced.