# Review: policy rev 4 contract draft 4
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-21 · round 4

All five round-3 findings are **closed** by draft 4 (§§1, 3, 5–6 and 11). One additional omission meets the review threshold.

1. **[major] §11; §5 “Search tools” — Removing existence-independent hard-deny probes is an unlisted default change.**

   **Concrete case:** `~/.aws` exists and is empty. Call:
   ```text
   Search(paths=["~/.aws"], glob="credentia?s", recursive=false)
   ```

   **Dev:** HardDeny in every setting. Its protected probes include `.aws/credentials` regardless of existence ([deny.rs:566](/home/rodrigo/atrium/crates/atrium-core/src/policy/deny.rs:566)), and the search-filter check returns a hard-deny hit when the filter matches that probe ([deny.rs:1473](/home/rodrigo/atrium/crates/atrium-core/src/policy/deny.rs:1473)).

   **Draft 4:** §5 enumerates no selected entries. Its retained synthetic probes cover **stores that ask** (§1’s explicit exception), not this hard-denied credential. The filter is therefore Clear; S8 does not apply to this nonrecursive call. Verdict: **Allow in Home / Ask with only a command prefix or exact rule under Careful / Allow under unscoped Any / Allow under FullAccess**. A covering `PathGlob` can also allow it. The command-text best-effort pass does not restore the missing Search probe.

   §11 lists Unread allowances and variant-only matches, but this is a **HardDeny → Clear** change.

   **Suggested contract wording:** Add to §11: “Hard-deny glob and search-filter probes no longer block solely because a pattern could match an absent protected path. For example, nonrecursive Search of an empty `~/.aws` with filter `credentia?s` changes from HardDeny everywhere to Clear, governed by the preset and covering rules. Synthetic probes for stores that ask remain unchanged.”

sound with the listed fixes — §11 still omits one concrete security-default relaxation.