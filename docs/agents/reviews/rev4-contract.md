# Review: policy rev 4 contract draft (docs/contracts/m1-policy-rev4.md)
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-21 · round 1

Read-only review completed; no files changed. Verdict vectors below mean **Home / matching Allow prefix rule / unscoped Any Allow / FullAccess**. For file tools, the second position means an applicable scoped rule. Where the draft gives conflicting answers, I identify both.

1. **[blocker] §1–§2, §12: “Exact hit” incorrectly includes ask-stores in hard-deny precedence.**  
   §1 defines an exact hit as including CR-12/CR-17 stores, then says any exact hit makes “the hard deny” win. §12 likewise requires every exact hit to be HardDeny. But §2 correctly distinguishes ask-stores. For `cat ~/.cargo/credentials.toml`, those provisions prescribe both **HardDeny everywhere** and **Ask / conditional Allow / Allow / Allow**.  
   **Suggested wording:** “Hits carry a disposition: `HardDenyHit` or `AskStoreHit`. Only `HardDenyHit` dominates all rules and settings. Uncertainty never cancels an independently established hit.” Restrict the property test accordingly.

2. **[major] §1, §3, §5: the remaining exactness promise is still open-ended.**  
   “Every command it runs and every path it touches is known” requires understanding executable identity, configuration, startup files and implicit operands. A readable `bash -ic 'cat README.md'` can execute startup code; a recognized `cat` basename can resolve through PATH to arbitrary code. The retained git-config limitation in `m1.md` §9 already contradicts that literal definition. Unknown tools ask under a prefix, but the contract never defines when a familiar basename establishes known semantics.  
   **Case:** with a project-supplied `cat` executable, `cat README.md` still gets the example’s **Allow / Allow / Allow / Allow**, despite unknown executed code.  
   **Suggested wording:** define exactness over a finite model of explicit operations, list assumptions about executable/configuration trust, and enumerate the supported tool forms and platforms in a normative annex. All unlisted forms are not established. Avoid claiming knowledge of all runtime effects.

3. **[major] §3–§4: quote and expansion provenance must survive parsing.**  
   Suppose ordinary `proj/key*` exists literally, while `proj/key1` links to an SSH key. `cat 'proj/key*'` must be **Allow everywhere**; `cat proj/key*` must be **HardDeny everywhere**. “Globs in a path” does not explicitly preserve that distinction. The same issue applies to escaped metacharacters, quoted `~`, concatenated word fragments, comments, and the boundary between `$HOME` and `$HOMEfoo`.  
   **Suggested wording:** “Words retain quote/escape provenance. Only syntactically active expansion characters expand; resolved or expansion-produced bytes are never reinterpreted except where the selected shell’s expansion order requires it.” Specify comments, brace expansion, parameter boundaries and invalid syntax explicitly.

4. **[major] §3: readable HOME/PWD expansions conflict with readable assignments.**  
   Consider `env HOME=/h/u/proj sh -c 'cat "$HOME/key"'`, where `proj/key` links to a credential and `/h/u/key` is ordinary. The gate’s supplied home is not the nested shell’s HOME. Bare assignment-only commands can also change later expansions. The assignment paragraph simultaneously permits readable values and says a value “that expands” makes the line not established, leaving `X=$HOME cat README.md` ambiguous.  
   **Literal implementation:** **Ask / Allow / Allow / Allow** after resolving the wrong HOME. **Safe fallback:** **Ask / Ask / Allow / Allow**, unless the changed environment is modeled and the credential hit established.  
   **Suggested wording:** “Assignments and wrappers that alter expansion, executable lookup, startup behavior or cwd make the affected unit and dependent units not established unless those effects are explicitly modeled.” Define assignment-only commands and HOME/PWD mutation.

5. **[major] §3, §6: readable redirects cannot safely reuse unchanged `CommandExact` storage.**  
   `cat README.md > out` and `cat README.md > elsewhere` have the same argv. Allowing readable redirects through exact rules while leaving `CommandExact` storage unchanged can make an “always…” approval authorize different writes. The current representation stores redirects separately from argv ([mod.rs:92](/home/rodrigo/atrium/crates/atrium-core/src/policy/mod.rs:92)); current rule coverage rejects ordinary redirects ([engine.rs:156](/home/rodrigo/atrium/crates/atrium-core/src/policy/engine.rs:156)), and remembered-rule creation does too ([gate.rs:1609](/home/rodrigo/atrium/crates/atrium-core/src/policy/gate.rs:1609)).  
   **Possible new verdict:** **Ask / Allow / Allow / Allow** for either write using the same argv rule.  
   **Suggested wording:** either retain the existing redirect/assignment exclusions for command rules, or make their identity part of a new matcher. Also define fd duplication/closure separately from path-bearing redirects, including inherited `/dev/fd/*` semantics.

6. **[major] §3–§5: unwrapping needs execution context and wrapper-owned effects.**  
   `env -C proj cat key` changes cwd; `time -o proj/key cat README.md` writes the wrapper’s own output file. Removing wrapper words while checking only the inner command loses those effects. The current code expressly models `env -C` through derived cwd-changing text ([shell.rs:3231](/home/rodrigo/atrium/crates/atrium-core/src/policy/shell.rs:3231)) and recognizes `time`’s output option ([shell.rs:1603](/home/rodrigo/atrium/crates/atrium-core/src/policy/shell.rs:1603)).  
   With `proj/key` a protected alias, a lossy unwrap yields **Ask / Allow / Allow / Allow**; a complete interpretation requires **HardDeny everywhere**.  
   **Suggested wording:** “Unwrapping preserves wrapper-owned reads/writes and transforms cwd, environment and descriptors. Unmodeled context changes are not established.” Pin the wrapper list, accepted forms and depth limit.

7. **[major] §3: `SHELLS` is not one readable language, and shell input sources are missing.**  
   The current list includes fish, csh, elvish and xonsh ([shell.rs:2589](/home/rodrigo/atrium/crates/atrium-core/src/policy/shell.rs:2589)). A POSIX parser does not establish their scripts. Even bash and zsh differ on unmatched globs. `sh script.sh`, bare `sh` consuming stdin, and startup-bearing shell options also need explicit treatment.  
   **Case:** `sh ordinary.sh`, whose contents are never inspected, must be **Ask / Ask / Allow / Allow**, even though its argv consists entirely of literal words.  
   **Suggested wording:** enumerate supported shell dialects and option combinations; initially support only a narrowly specified `-c` form. All script-file, stdin, startup-code and unsupported-dialect execution is not established.

8. **[blocker] §4: tainting later units does not cover effects inside the writing unit.**  
   Suppose `src/gh/hosts.yml` exists. `cp -R src/. ~/.config` writes `~/.config/gh/hosts.yml`, although neither explicit operand names that protected leaf. The unit “may write,” but there is no later unit to taint, and its own targets supposedly retain their outcome. Archive extraction presents the same problem through member names and links.  
   **Operand-only reading:** **Ask / Allow / Allow / Allow**. A complete destination derivation finds **HardDeny everywhere**; an unsupported derivation must instead produce **Ask / Ask / Allow / Allow**.  
   **Suggested wording:** “Exact classification of a unit requires its implicit recursive destinations and intra-unit filesystem effects to be modeled. Otherwise that unit itself is not established; tainting successors is additional.”

9. **[major] §1, §4: the time boundary implies a snapshot that the gate does not have.**  
   Another process can replace `proj/key` after its resolution but before the same check finishes. This is not literally “between the check and the run.” An automatically allowed call also has no redemption pass: `do_check` returns Allow directly ([gate.rs:1639](/home/rodrigo/atrium/crates/atrium-core/src/policy/gate.rs:1639)); redemption re-evaluation applies to approval tickets ([gate.rs:1705](/home/rodrigo/atrium/crates/atrium-core/src/policy/gate.rs:1705)).  
   **Case:** ordinary `cat proj/key` is **Allow everywhere**; replacing the link after its observation can change what execution reaches.  
   **Suggested wording:** “Classification uses non-atomic filesystem observations. External mutations after any relevant observation, including during evaluation, are outside the guarantee. Approved calls are re-evaluated at redemption; automatic Allows are not.”

10. **[major] §4: name identity both weakens dev behavior and leaves filesystem equivalence unspecified.**  
    On a case-sensitive filesystem, existing ordinary `.SSH/ID_RSA` and protected `.ssh/id_rsa` have different identities. The draft’s existing-path rule can therefore give `cat ~/.SSH/ID_RSA` **Allow everywhere**. Dev hard-denies the folded spelling regardless of identity ([deny.rs:378](/home/rodrigo/atrium/crates/atrium-core/src/policy/deny.rs:378), [deny.rs:505](/home/rodrigo/atrium/crates/atrium-core/src/policy/deny.rs:505)). That relaxation is absent from §10.  
    For nonexistent names, lowercase plus `ſ→s` does not specify all case/normalization equivalences of supported filesystems. An absent Atrium root spelled `Café` versus `Café` exposes the gap.  
    **Suggested wording:** retain folded-name policy protection **in addition to** identity checks, explicitly include existing ancestor identities, and specify how unsupported filesystem name equivalence becomes not established.

11. **[major] §4: search filters need per-tool semantics and an escaping-selector distinction.**  
    “A standard matcher such as globset” does not define basename versus relative-path matching, negation, ordered multiple filters, case-insensitive filters or directory pruning. More seriously, the existing `Glob` subject can select outside its roots.  
    **Case:** `Glob(paths=["~/proj"], glob="../.ssh/id_rsa")`. Testing only protected paths *under* `proj` misses it and permits **Allow everywhere**; existing semantics require **HardDeny everywhere**. Dev separately computes escaping candidates ([deny.rs:275](/home/rodrigo/atrium/crates/atrium-core/src/policy/deny.rs:275)); D-100’s tests preserve escaping-glob behavior ([adversarial_m1_18.rs:1320](/home/rodrigo/atrium/crates/atrium-core/tests/adversarial_m1_18.rs:1320)).  
    **Suggested wording:** distinguish path selectors from traversal filters. Specify each supported filter dialect; retain D-100 handling for selectors that escape roots. Unsupported forms are not established.

12. **[major] §4: recursive containment and link walks conflict with filters and “touches.”**  
    With `proj/keys -> ~/.ssh`, `rg -L -g '!keys/**' x proj` excludes that subtree. Nevertheless, the followed-link clause says any link reaching a protected path is an exact hit: **HardDeny everywhere**. Likewise, containment can hard-deny `rg -g '*.rs' x ~/.aws` even when the existing protected files are excluded.  
    **Suggested wording:** choose explicitly between a conservative *protected-root policy* and actual selected-path reachability. If conservative containment is intended, say it can block excluded contents and define its precedence over filters and the `.config` exception. Otherwise apply effective selection semantics to both containment and link walks.

13. **[major] §4, §7, Q2: recursive hard links remain incompatible with “exact clear.”**  
    Put a hard link to an SSH key at `proj/node_modules/leak`. `rg -uuu x proj` can read it. The retained dev walk skips `node_modules`/`.git`, limits depth, and returns clear after 4,096 entries ([gate.rs:1515](/home/rodrigo/atrium/crates/atrium-core/src/policy/gate.rs:1515)). This can leave **Allow everywhere** despite the protected identity. The exception is explicit, but §1’s unqualified exact-clear definition and §7’s universal exhaustion rule contradict it.  
    **Suggested wording:** decide Q2 before approval. Either exhaustion/pruning makes the relevant call not established, or narrow the outcome’s advertised guarantee and enumerate every retained exclusion—not just the entry cap.

14. **[major] §7: the budget is not yet a checkable resource contract.**  
    A constant-length `rg -L x proj` can require arbitrarily much filesystem work, so “linear in input length” cannot describe the whole check. Limits, units, memory accounting, directory-open costs, protected-set construction and the best-effort pass are unspecified.  
    **Cases:** 513 simple `cat README.md` segments exceed dev’s unit cap and currently reach `Deny` through overflow ([deny.rs:236](/home/rodrigo/atrium/crates/atrium-core/src/policy/deny.rs:236), [engine.rs:910](/home/rodrigo/atrium/crates/atrium-core/src/policy/engine.rs:910)). §7 instead implies **Ask / Ask / Allow / Allow**. A line over `MAX_BYTES` remains **Deny everywhere**. A discovered hard-deny hit before ordinary work exhaustion should remain **HardDeny everywhere**.  
    **Suggested wording:** specify numeric limits and a shared budget object; charge scans, allocations and filesystem operations before performing them. State complexity as input processing plus bounded charged work. Define size-limit, hit and exhaustion precedence, including the fallback pass.

15. **[major] §5–§6: implicit file contents and repository objects remain unbounded obligations.**  
    Marking an operand “ref” or “archive member” does not establish selected paths. `git show HEAD -- .gem` can expose committed CR-17 credentials absent from the working tree; exclusion-only pathspecs can select almost everything. “Git credential guards” does not specify which semantics survive. Similarly, a pinned `python safe.py` rule does not establish code whose file contents are never read.  
    **Case:** without an explicit git selection rule, `git show HEAD -- .gem` can remain **Allow everywhere** instead of asking in Home; the equivalent failure is documented in M1-29’s latest review. For uninspected `python safe.py`, the contract should require **Ask / Ask / Allow / Allow**.  
    **Suggested wording:** enumerate supported repository selectors and retained credential-name policy checks. Treat other selectors and external program contents as not established. State that pinning a program permits rule creation but does not establish its effects.

16. **[major] §2, §6: preserve Deny rules and define authorization of ask-stores.**  
    “Not established always asks” and “a rule never covers” can disable existing Deny prefixes. With a Deny `["cat"]`, `cat "$X"` currently reaches rule denial even though it is unreadable ([engine.rs:91](/home/rodrigo/atrium/crates/atrium-core/src/policy/engine.rs:91), [engine.rs:923](/home/rodrigo/atrium/crates/atrium-core/src/policy/engine.rs:923)). The draft can instead turn it into an approvable Ask.  
    Also, “a rule naming” an ask-store is not “as today”: dev applies ordinary covering rules before Home’s ask-store check ([engine.rs:939](/home/rodrigo/atrium/crates/atrium-core/src/policy/engine.rs:939)). Does `["cat"]` count, or must the credential path be pinned?  
    **Suggested wording:** apply established Deny-rule matches before the uncertainty allowance table; restrict that table to Allow rules. Define “naming the store,” scoped Any behavior, and treatment of already-stored rules that new validation would reject.

17. **[major] §9: several example verdicts do not follow as written.**  
    Three corrections are necessary:
    - `sh -c 'X="$1" sudo id' _ foo`: dev’s best-effort lexer extracts the inner script ([shell.rs:3161](/home/rodrigo/atrium/crates/atrium-core/src/policy/shell.rs:3161)) and checks derived units for `sudo` ([deny.rs:1245](/home/rodrigo/atrium/crates/atrium-core/src/policy/deny.rs:1245)). The result is **HardDeny everywhere**, not merely the footnote’s possible *raw-text* match.
    - `Read /h/u/[!é]trium/secret`: §4 calls Read paths literal. It cannot simultaneously require glob expansion. A literal Read gets the literal path’s outcome; an explicit glob selector can get **HardDeny everywhere** when it reaches Atrium data.
    - `awk …` with `["awk"]`: creation is Invalid; without that rule, execution is **Ask / no valid prefix / Allow / Allow**, subject to fallback hits—not unqualified “the call asks.”
    
    **Suggested wording:** make every example use the same four columns, explicit valid rules and concrete fixtures. Change the footnote to “unless the best-effort raw **or unit** check hits.” The remaining examples are plausible subject to the preceding definitions and budget assumptions.

18. **[major] §10: the security-default inventory is incomplete.**  
    Beyond the five listed items, the draft changes or leaves changes unresolved for:
    - `Unknown` under Any/FullAccess: dev always asks ([engine.rs:935](/home/rodrigo/atrium/crates/atrium-core/src/policy/engine.rs:935)).
    - Non-byte-limit exhaustion changing Deny/Ask to Any/FullAccess Allow.
    - `$HOME`/`$PWD`, assignments, globs and redirects becoming eligible for command-rule coverage—and possibly Home allowance. Dev expressly rejects these through its parser/guards ([mod.rs:135](/home/rodrigo/atrium/crates/atrium-core/src/policy/mod.rs:135), [engine.rs:782](/home/rodrigo/atrium/crates/atrium-core/src/policy/engine.rs:782)).
    - Missing protected matches ceasing to hard-deny because expansion/filter checks now require existing targets.
    - Known-tool text/pattern operands ceasing to count as candidate paths. For example, `grep .npmrc README.md` currently names `.npmrc` through dev’s all-arguments extraction ([words.rs:508](/home/rodrigo/atrium/crates/atrium-core/src/policy/words.rs:508)); a role-aware table can correctly stop blocking that pattern.
    - Existing-name folding, S8 containment, deny-rule precedence, ask-store authorization and the four CR-17 additions.
    
    **Suggested wording:** replace “Everything else…” with a complete before/after verdict inventory. Distinguish previously approved changes, such as CR-17/CR-19, from changes still needing Pedro’s confirmation. Explicitly retain Home’s existing guards unless changing them intentionally.

19. **[major] §11: the branch mapping is directionally right, but incomplete.**  
    I checked `git diff dev...HEAD --stat` in all six worktrees:

    | Branch | Files; additions/deletions | Mapping correction |
    |---|---:|---|
    | M1-27 | 4; +1,835/−223 | Keep folding; select bounded scans individually. “Where §3 still reads code” needs a finite list. |
    | M1-28 | 23; +4,814/−266 | Also account for client batch-error logging, general login-shell PATH discovery and the independent export regression test. |
    | M1-29 | 15; +6,232/−97 | Include `Security.tsx`/tests, daemon rule validation/tests, core validator wiring and canonical-scope semantics—not just stores and `rule-form.ts`. |
    | M1-30 | 4; +3,208/−83 | Replacement matcher can be dropped, but tests need individual verdict migration, especially absent-target cases. |
    | M1-31 | 8; +3,876/−205 | N11 is **not all in the removed glob normalizer**: it changes brace expansion in `words.rs`, lexer memoization in `shell.rs`, and Sensitive sharing in `engine.rs`. |
    | M1-32 | 4; +2,807/−212 | Replacement is reasonable, but preserve explicit obligations for ordered resolution, dangling targets, I/O uncertainty, identity and archive operand roles. |

    **Suggested wording:** attach a file/hunk disposition inventory and a test-case disposition list. Name the N11 work moving into P1/P2/P3 separately. “Tests kept” must not mean retaining expectations that the new contract intentionally changes.

20. **[minor] §11–§12: M1-33 and M1-34 can proceed independently of rev-4, with qualifications.**  
    M1-34’s `bce1a5a` changes only two ordinary search commands and their empty external-directory fixture; the assertions stay unchanged. That is genuinely separable from policy implementation.  
    M1-33’s production changes use existing gate interfaces and can be extracted onto dev without rev-4. They still affect approval lifecycle, and its adapter tests use a real `SessionGate` ([branch test:19](/home/rodrigo/atrium-wt/M1-28-ansi-c-words/crates/atrium-agents/tests/M1-28-followups.rs:19)); “non-policy” should mean independent of policy *implementation*, not unrelated to enforcement. The path whitelist also strands the export regression in core ([branch test:419](/home/rodrigo/atrium-wt/M1-28-ansi-c-words/crates/atrium-core/tests/M1-28-ansi-c-words.rs:419)).  
    **Suggested wording:** “Extract the listed non-policy hunks and their tests onto dev; verify against dev’s existing policy. Include the standalone export test separately. Do not cherry-pick mixed commits wholesale.”

21. **[major] §12: P1 first is sensible; P2/P3 are not independent merely because filenames differ.**  
    P2 needs P3’s operand roles, recursion, link-following and writer metadata. P3 needs P2’s canonical targets and per-unit uncertainty. Both depend on P1 retaining word provenance and execution structure. Today those responsibilities cross the proposed boundaries: extraction is in [words.rs:492](/home/rodrigo/atrium/crates/atrium-core/src/policy/words.rs:492), wrapper syntax in [shell.rs:1589](/home/rodrigo/atrium/crates/atrium-core/src/policy/shell.rs:1589), recursive-reader interpretation in [gate.rs:374](/home/rodrigo/atrium/crates/atrium-core/src/policy/gate.rs:374), and Home interpretation in [engine.rs:782](/home/rodrigo/atrium/crates/atrium-core/src/policy/engine.rs:782). P3 must modify or replace behavior in P1-owned files. §8’s reason/remembering changes also require ledger/UI plumbing absent from the task ownership list.  
    **Suggested wording:** land a shared interface first: provenance-bearing units, tool effects, canonical per-operand targets, separate hit dispositions, uncertainty reasons and one budget. Then permit parallel implementation behind that interface. Keep incomplete forms not established until integrated, and run a final integrated policy review. Property tests must check these semantic boundaries independently of the implementation’s own classifier.

sound with the listed fixes — the bounded-subset approach is viable, but the current exactness claims, verdict precedence and implementation boundaries are not yet precise enough to approve.