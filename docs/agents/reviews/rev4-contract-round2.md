# Review: policy rev 4 contract draft 2
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-21 · round 2

Read-only review completed; no files changed. Unlisted round-1 findings are **closed**. Sections below use draft-2 numbering. Verdict vectors mean **Home / covering prefix or exact rule / unscoped Any Allow / FullAccess**, unless stated otherwise.

1. **[major] §4, §6, §14 — Round 1 #2 partly closed: the normative annex still leaves policy decisions to implementation.**  
   The assumptions close the unbounded runtime-effects claim, but the supported tool forms, operand roles and program-pinning rules are still unwritten. Q1 expressly leaves `ln`’s target classification undecided. Consequently, a reviewer cannot yet check the promised model completely.

   **Concrete case:** `ln -s ~/.ssh/id_rsa x` can be a model hit or merely name a text operand, depending on the future table.

   **Suggested wording:** “Before behavior work starts, approve the normative tool-form table, including operand roles, supported platforms, program-pinning conditions and link-target policy. Changes to that table change this contract.” Resolve Q1 there.

2. **[major] §6 — Round 1 #15 partly closed: ordinary Git wildcard pathspecs remain unspecified.**  
   Directory ancestors, magic and exclusions are now covered. Ordinary quoted wildcards are not expressly covered.

   **Concrete case:** HEAD contains `.gem/credentials`, absent from the working tree; execute `git show HEAD -- '.gem/*'`. The operand is neither an equal/ancestor literal nor magic/exclusion. Literal-only name checking admits **Allow / Allow / Allow / Allow**. Treating the selector as Unread gives **Ask / Ask / Allow / Allow**.

   **Suggested wording:** “Git pathspecs containing active wildcard syntax are Unread unless their selection semantics are explicitly modeled. Pattern operands are excluded from filesystem-path extraction, but remain subject to Git credential-selection checks.”

3. **[major] §1–§2, §7 — New: outcome combination is not a complete decision procedure.**  
   Precedence defines Too large, hard-deny hits and Deny rules, but does not define the join among Store hit, Clear, Unread, Unread(program) and Exhausted.

   **Concrete case:** a multi-root Search names an ask-store and exhausts its walk of another root. Store hit permits Any/FullAccess Allow; Exhausted requires **Ask everywhere**. Likewise, Unread(program) must not absorb an independent plain-Unread reason.

   **Suggested wording:** “Retain all independently established hits and uncertainty reasons. After the first three precedence steps, any Exhausted part forces Ask; otherwise every unit must satisfy its applicable authorization conditions. Program authorization discharges only that unit’s program-content uncertainty.”

4. **[blocker] §5, “Derived destinations” — Round 1 #8 partly closed: a disjoint destination root does not establish its descendants.**  
   Root containment handles the original `.config` case, but “Anything else is Clear” remains unsound.

   **Concrete case:** `src/plain` is ordinary, `proj/plain -> ~/.ssh/id_rsa`; run `cp -R src/. proj`. The explicit roots are clear, but the derived destination follows the existing file symlink and overwrites the key. Root-only checking admits **Ask / Allow / Allow / Allow**. Resolving the derived destination gives **HardDeny everywhere**; declining to derive it should give **Ask / Ask / Allow / Allow**.

   Archive absolute names, parent traversal and member-created links present the same boundary.

   **Suggested wording:** “A destination root is sufficient only when the modeled operation guarantees containment after destination-link resolution and intra-unit effects. Otherwise derive and resolve affected destinations within budget, or classify the writer itself Unread.”

5. **[major] §5, “Later units” — New: pipeline taint must travel in both directions.**  
   “At the same time” includes earlier pipeline stages, but the parenthetical “the rest of its pipeline” suggests forward-only taint.

   **Concrete case:** `cat proj/key | python mutate.py`, with rules covering both commands. `mutate.py` replaces initially ordinary `proj/key` with a credential alias before `cat` opens it. Forward-only handling permits **Ask / Allow / Allow / Allow**. The explicit reader should instead be Unread: **Ask / Ask / Allow / Allow**, absent an independently established hit.

   **Suggested wording:** “Every stage of a pipeline containing a possible writer has uncertain filesystem targets, including stages preceding that writer. Program runners with uninspected effects count as possible writers.”

6. **[major] §3–§5 — New: the declared shell intersection conflicts with accepted syntax, and command-word expansion lacks a defined second lookup.**  
   `&>` is not shared POSIX-sh/bash/zsh syntax. `cat README.md &> /dev/null` is admitted by the redirect list and harmless-redirect guards, allowing **Allow everywhere**; under the stated intersection it should be Unread: **Ask / Ask / Allow / Allow**.

   Separately, `/usr/bin/sud[o] id`, where that glob uniquely matches `/usr/bin/sudo`, must become **HardDeny everywhere**. §4 looks up the command before §5 expansion, but no rule requires lookup and wrapper recognition again afterward.

   **Suggested wording:** “Remove `&>` from the intersection grammar. Expand active command-word globs before tool classification, then fold and classify the resulting executable words. Unsupported or dialect-dependent expansion is Unread.” Explicitly define unmatched and malformed classes too.

7. **[major] §3, redirects; §2 assumptions — Round 1 #5 partly closed: inherited descriptors remain outside the exactness boundary.**  
   Keeping argv-only rules closes the redirect-identity problem. It does not establish descriptor targets.

   **Concrete case:** `cat <&3`, with inherited fd 3 referring to an SSH key, receives **Allow everywhere** under the permitted duplication grammar. The gate cannot establish that target from the command text. Dev likewise treats duplication as harmless: [mod.rs:126](/home/rodrigo/atrium/crates/atrium-core/src/policy/mod.rs:126).

   **Suggested wording:** either “Inherited descriptor contents and destinations are an explicit assumption outside the promise,” or restrict modeled duplication to descriptors whose provenance is known. Without that assumption, the case should be **Ask / Ask / Allow / Allow**.

8. **[major] §5, “Names” — Round 1 #10 partly closed: the two name rules contradict each other.**  
   Folded-name protection is expressly independent of filesystem identity, but the next bullet downgrades non-ASCII folded matches when the volume may distinguish them.

   **Concrete case:** on a case-sensitive volume, ordinary `~/.ſsh/id_rsa` is distinct from `~/.ssh/id_rsa`. The first bullet requires HardDeny; the second permits Unread. That means **HardDeny everywhere** versus **Ask / Ask / Allow / Allow**. Dev applies the `ſ→s` policy fold unconditionally: [deny.rs:378](/home/rodrigo/atrium/crates/atrium-core/src/policy/deny.rs:378).

   **Suggested wording:** “The listed normalization and folding rules define protected policy spellings irrespective of volume equivalence. The Unread rule applies only to additional, unsupported filesystem equivalences; it never downgrades a listed policy-name hit.”

9. **[major] §5, search filters and links — Round 1 #11 partly closed: checking protected names under a root misses selected aliases.**  
   Escaping selectors are now distinguished correctly. Ordinary nonescaping Glob filters still need selected-entry resolution.

   **Concrete case:** `proj/key -> ~/.ssh/id_rsa`; call `Glob(paths=["proj"], glob="key*")`. Matching only protected paths under `proj` finds none and admits **Allow everywhere**. Resolving the selected entry requires **HardDeny everywhere**. Dev’s glob/link path checks explicitly resolve expanded candidates: [gate.rs:1437](/home/rodrigo/atrium/crates/atrium-core/src/policy/gate.rs:1437).

   **Suggested wording:** “For Search/Glob selectors, enumerate selected entries within budget and apply literal-path resolution to each. Matching the protected catalogue alone cannot establish Clear because aliases may have unrelated names. State each file tool’s link behavior explicitly.”

10. **[major] §5, S8; §10 row 16; §11 — New: the Home-root exception has disappeared.**  
    S8 now hard-denies every recursive root containing an existing protected path, exempting only `.config`.

    **Concrete case:** with `~/.ssh/id_rsa` present, `rg token ~` becomes **HardDeny everywhere**. Dev instead gives **Ask / Allow / Allow / Allow** under the stated rules; Home rejects the home root through [engine.rs:755](/home/rodrigo/atrium/crates/atrium-core/src/policy/engine.rs:755). `m1.md` §9 expressly says a recursive search rooted at `~` asks.

    **Suggested wording:** “S8 excludes the home root and the D-133 `.config` exception; these retain their preset/rule behavior.” If removing the Home exception is intentional, list and approve that security-default change. Also give row 16 an existing-credential fixture.

11. **[major] §1, §2, §7 — Round 1 #16 partly closed: rule rank and scoped Any behavior remain ambiguous.**  
    “A user Deny rule” before the allowance table changes dev’s scope precedence.

    **Concrete case:** global Deny `["cat"]`, plus a thread Allow `["cat"]` in the prefix column or thread Any Allow in the Any column; execute `cat README.md`. Dev gives **Deny / Allow / Allow / Deny** because completed higher-priority coverage suppresses lower-rank denial: [engine.rs:200](/home/rodrigo/atrium/crates/atrium-core/src/policy/engine.rs:200). Draft precedence instead says Deny everywhere.

    Also, scoped Any plus `cat "$X"` encounters both §2’s Allow and §7’s unresolved-target exclusion.

    **Suggested wording:** “Preserve thread/Home/global rule precedence unless explicitly changed. The Unread allowance column means an **unscoped** Any Allow. Scoped Any and PathGlob rules require established canonical targets.”

12. **[major] §8, §13 — Round 1 #14 partly closed: budget accounting and its test oracle still disagree.**  
    The shared budget and exhaustion disposition are improvements. Numeric work/allocation limits remain deferred, and charges for metadata, canonicalization, link reads and directory opens are unspecified.

    **Concrete case:** `rg -L x huge`, exhausting the shared budget without a hit, must be **Ask everywhere**. §13 nevertheless permits exhaustion to Allow under Any/FullAccess. Its “never HardDeny” property also conflicts with preserving a hit found before exhaustion.

    **Suggested wording:** “P0 freezes numeric limits and charges for every filesystem operation, matcher step and allocation. Shared-budget exhaustion forces Ask in every setting unless a higher-precedence result already exists. Only the explicitly retained local hard-link-walk cutoff may return clear.” Make the properties assert exactly those rules.

13. **[major] §10 row 17 — New: the example’s verdict contradicts the retained best-effort pass.**  
    `ln -s ~/.ssh/id_rsa x; cat x` contains a literal home-anchored SSH target. Dev’s raw matcher recognizes it independently of operand roles: [deny.rs:733](/home/rodrigo/atrium/crates/atrium-core/src/policy/deny.rs:733), invoked at [deny.rs:1211](/home/rodrigo/atrium/crates/atrium-core/src/policy/deny.rs:1211).

    **Verdict:** **HardDeny everywhere**, not the row’s Ask/Ask/conditional-Allow result. Q1 does not decide this example while that fallback remains.

    **Suggested wording:** replace row 17’s verdict with HardDeny everywhere. More generally, clarify that §1’s “over-approximation never produces a hit” governs model classification; the retained raw-text fallback can conservatively block text that is not an accessed path.

14. **[major] §11 — Round 1 #18 partly closed: the security-default inventory is still incomplete.**  
    Besides the changes identified above, two concrete omissions remain:

    - With no cache files present, `Search(paths=["~/.azure"], glob="msal*", recursive=false)` changes from **Ask / Allow / Allow / Allow** to **Allow everywhere**. Dev matches synthetic ask-store probes without requiring existence: [deny.rs:639](/home/rodrigo/atrium/crates/atrium-core/src/policy/deny.rs:639), [deny.rs:683](/home/rodrigo/atrium/crates/atrium-core/src/policy/deny.rs:683).
    - `cat README.md >/dev/null; cat LICENSE` changes from **Allow everywhere** to **Ask / Ask / Allow / Allow** under §5’s writer taint. Dev permits the harmless redirect and both read-only units: [mod.rs:126](/home/rodrigo/atrium/crates/atrium-core/src/policy/mod.rs:126), [engine.rs:893](/home/rodrigo/atrium/crates/atrium-core/src/policy/engine.rs:893). Thus “In Home this changes nothing” is false.

    **Suggested wording:** add explicit before/after rows for existence-dependent protection, Home-root containment, rule-rank changes and harmless-output taint. Either exempt modeled `/dev/null` output from filesystem-mutation taint or disclose the Home change.

15. **[major] §12–§13 — Round 1 #21 partly closed: P0 helps, but ownership and activation still overlap.**  
    P1 owns the outcome table in `engine.rs`; P3 owns its rule behavior; §12 assigns P2 the shared `Sensitive` change in that same file. Today evaluation directly combines all three responsibilities: [engine.rs:907](/home/rodrigo/atrium/crates/atrium-core/src/policy/engine.rs:907). P1’s extraction also consumes P3’s operand roles, while P2 supplies the canonical targets used by P3.

    **Concrete integration case:** an incomplete form marked Unread can Allow under Any/FullAccess before its new path checker is integrated. “Leave incomplete forms Unread” therefore does not itself preserve protection.

    **Suggested wording:** “P0 freezes function-level ownership and executable interfaces, including aggregation and canonical-target consumption. Assign one owner to evaluator integration and the `Sensitive` wiring. Keep the dev evaluator active until the complete P1–P4 path passes integrated review.” Restrict fixture-hit properties to modeled targets; arbitrary Unread runtime effects cannot simultaneously have guaranteed HardDeny.

sound with the listed fixes — the bounded-model design remains viable, but destination containment, outcome/rule semantics and the remaining policy decisions need resolution before implementation approval.