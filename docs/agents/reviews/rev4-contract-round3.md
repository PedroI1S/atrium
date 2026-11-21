# Review: policy rev 4 contract draft 3
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-21 · round 3

Read-only; no files changed. **Round-2 #14 is partly closed; the other round-2 findings are closed.** Remaining and new findings follow. Verdict vectors are **Home / covering rule / unscoped Any / FullAccess**.

1. **[major] §5 filters, §7, §10 rows 12/24 — Probe matches still contradict rule coverage.**  
   **Round-2 #14: partly closed.** With no cache present, `Search(paths=["~/.azure"], glob="msal*")` is explicitly plain **Unread**. §§1–2 and §7 therefore require **Ask / Ask / Allow / Allow**, whereas row 24 permits a covering rule. A `PathGlob` cannot discharge that uncertainty either. Row 12 also needs clarification: an existing JSON cache does not establish another matching probe, such as the absent `.bin` name.

   This additionally changes dev’s rule-authorized behavior: covering rules allow before Home’s probe check ([engine.rs:939](/home/rodrigo/atrium/crates/atrium-core/src/policy/engine.rs:939)); probes include both suffixes ([deny.rs:647](/home/rodrigo/atrium/crates/atrium-core/src/policy/deny.rs:647)).

   **Suggested wording:** “A probe match creates an independent plain-Unread reason, including when another selected entry produces a Store hit.” Correct the examples and list the resulting loss of scoped-rule authorization in §11. Alternatively, explicitly define a coverable policy-store outcome for probes.

2. **[major] §3 redirects, §5 literal resolution — Process-dependent filesystem paths remain inside the exact model.**  
   **Concrete case:** the gate process’s cwd is `/g`, execution cwd is `/h/u`, `/g/key` is ordinary, and `/h/u/key → ~/.ssh/id_rsa`. Execute:
   ```sh
   cat /proc/self/cwd/key
   ```
   Canonicalization observes the **checker’s** process context: dev calls `std::fs::canonicalize` directly ([gate.rs:71](/home/rodrigo/atrium/crates/atrium-core/src/policy/gate.rs:71)). Under the draft’s resolver, the call appears Clear: **Allow everywhere**. Execution opens the credential instead. No filesystem mutation or excluded shell construct is involved. Excluding `/dev/fd/` only as redirect targets does not cover this case.

   **Suggested wording:** “Process-dependent paths and links, including `/proc/self`, `/proc/thread-self` and descriptor aliases, are Unread unless resolved in the execution process’s context. This applies to operands and redirect targets, including aliases through symlinks.” The unresolved case then gets **Ask / Ask / Allow / Allow**.

3. **[major] §5 hard links, §11 — Dev’s positive multiply-linked-file guard has no specified disposition.**  
   **Concrete case:** ordinary `proj/a` has another hard link at `proj/b`; neither identity is protected. Call `Read proj/a`.

   Dev returns **Ask everywhere**, including after a covering rule or FullAccess Allow: it detects `nlink > 1` ([gate.rs:144](/home/rodrigo/atrium/crates/atrium-core/src/policy/gate.rs:144)) and overrides Allow ([gate.rs:362](/home/rodrigo/atrium/crates/atrium-core/src/policy/gate.rs:362)). The draft’s literal-path rules instead establish no protected identity and therefore Clear: **Allow everywhere**. Its recursive-walk exception does not address this named-file case, and §11 does not disclose removing the guard.

   **Suggested wording:** “Retain dev’s unconditional Ask for a multiply-linked regular file not already classified as a protected hit, including positive results from the bounded recursive walk.” Specify its precedence explicitly; ordinary Unread would permit Any/FullAccess.

4. **[major] §5 Names, §11 — NFC adds an unlisted protected-spelling default.**  
   **Concrete case:** on a case-sensitive filesystem, distinct directories `/h/u/Café` and `/h/u/Café` exist—the latter uses `e` plus U+0301. Atrium’s data root is the former; call `Read /h/u/Café/secret`.

   Dev’s path fold performs lowercase and `ſ→s`, without NFC ([deny.rs:378](/home/rodrigo/atrium/crates/atrium-core/src/policy/deny.rs:378)); with distinct identities, this is **Allow everywhere**. Draft §5 normalizes both spellings and requires **HardDeny everywhere**. §11 nevertheless calls folded-name protection unchanged. CR-19 approved lowercase/`ſ` folding, not NFC ([change-requests.md:240](/home/rodrigo/atrium/docs/agents/change-requests.md:240)).

   **Suggested wording:** add to §11: “Protected-name comparison additionally applies Unicode NFC, including on filesystems that distinguish canonically equivalent spellings; this can change Allow to HardDeny.”

5. **[major] §§4/6, §11 — Folded tool lookup expands Home’s command allowance without listing it.**  
   **Concrete case:** `/bin/cat README.md`, with an ordinary file and executable. Dev’s Home list matches the original argv literally ([engine.rs:791](/home/rodrigo/atrium/crates/atrium-core/src/policy/engine.rs:791)); its entry is `cat` ([policy.rs:508](/home/rodrigo/atrium/crates/atrium-protocol/src/policy.rs:508)). Verdict: **Ask / Allow / Allow / Allow**.

   Under §4’s folded-basename lookup and §6’s Home eligibility property on that tool entry, it becomes **Allow everywhere**. CR-19’s shell/wrapper/privilege-name changes do not authorize this expansion of Home membership, and §11 omits it.

   **Suggested wording:** either preserve original-argv spelling requirements for Home eligibility, or list explicitly: “Home recognizes supported read-only tools by folded basename, including absolute executable paths,” with the before/after verdict.

**sound with the listed fixes — the remaining issues are bounded verdict contradictions and omitted default changes.**