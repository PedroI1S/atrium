# Review: M1-30-tree-wildcards
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-16

## Verdict
**rework**: S5 and K8 still have concrete bypasses, and an explicitly protected ordinary-glob verdict regressed.

Cases below use the virtual home `/h/u`, cwd `/h/u`, and empty `resolved_paths`. “Any” means Home with a matching global `Any → Allow` rule. Unless identified as executed, verdicts follow from tracing HEAD’s code.

## Findings

1. [major] **Non-escaping search globs still miss arbitrary files inside sensitive trees**
   - Where: `crates/atrium-core/src/policy/deny.rs:575`, `"v.push(format!("{}/x", abs(t)));"`, and `:2092`, `"for probe in self.s.probes()"`.
   - Case: `Search { paths: ["~"], recursive: false, glob: ".g*/private-keys-v1.d/k.key" }`. Also `glob: ".local/share/atr*/missing.db"`, with Atrium data at `~/.local/share/atrium`.
   - Result: Allow in Home, Any and FullAccess. These contained filters reach only `search_glob_hit`’s finite probes; neither filename is represented. Expected HardDeny (`credential-read` and `atrium-self`, respectively): the brief’s “What to do” §1 includes globs, and contract §9 explicitly includes searches. This gap also exists on `dev`.
   - Fix: Check contained search filters symbolically against sensitive trees instead of relying on filenames sampled by `probes`. Escalate the ownership restriction if that requires changing `search_glob_hit`.

2. [major] **Wildcarded `cd` still loses the sensitive destination**
   - Where: `crates/atrium-core/src/policy/words.rs:628`, `"if has_unknown_var(&expanded) || expanded.contains(['*', '?', '['])"`, followed by `None`; `crates/atrium-core/tests/adversarial_fuzz.rs:422`, `#[ignore = "M1-13 finding 11"]`.
   - Case: `timeout 5 cd ~/.a?s; xxd credentials;`.
   - Result: The existing ignored fuzz binary fails at iteration 28 with Ask in Home. Code tracing gives Allow under Any and FullAccess. Expected HardDeny (`credential-read`), because `.a?s` can be `.aws` and the following read can name `.aws/credentials`. The plain-directory spelling is hard-denied. This is an existing `dev` gap, explicitly acknowledged in the handoff.
   - Fix: Preserve the wildcarded destination among possible cwd candidates for subsequent sensitive-path checks, then un-ignore the original test. The worker correctly escalated the ownership issue, but S5 remains incomplete under D-120.

3. [major] **POSIX named character classes bypass the new component matcher**
   - Where: `crates/atrium-core/src/policy/deny.rs:977`, `"while j < bytes.len() && bytes[j] != b']'"`; `:998`, `"fn class_matches_char(class: &str, ch: char) -> bool"`.
   - Case: `cat ~/.s[[:alpha:]]h/id_rsa`.
   - Result: Ask in Home; Allow under Any and FullAccess. `first_class_end` stops at the inner `]`, and the custom matcher does not recognize `[:alpha:]`. The shell pattern matches `.ssh/id_rsa`, so the correct result is HardDeny (`credential-read`) in every setting. The brief requires classes in tree names to count as the names they can match.
   - Fix: Recognize POSIX named classes, including their nested bracket terminator, or conservatively handle this unsupported syntax as potentially matching the sensitive component.

4. [major] **The `**` leaf exception misses Atrium’s WAL**
   - Where: `crates/atrium-core/src/policy/deny.rs:1298`, `"if self.parts[idx..] ... .all(|c| matches!(c, Comp::GlobStar))"`; `:620`, `"glob.matches_exact(&format!("{d}/atrium.db"))"`.
   - Case: `cat ~/.local/**/atrium.db-wal`, with data root `~/.local/share/atrium`.
   - Result: Ask in Home; Allow under Any and FullAccess. The tree matcher rejects a `GlobStar` consuming the sensitive leaf when a filename follows, and the fallback samples `atrium.db` but not its WAL. Expected HardDeny (`atrium-self`): contract §9 explicitly protects database WAL files. The equivalent plain path is hard-denied.
   - Fix: Extend the specific sensitive-file handling to the configured data artifacts, including WAL paths, while preserving the required ordinary `**/*.rs` verdicts.

5. [major] **K8’s fixed filename samples allow valid session-file searches**
   - Where: `crates/atrium-core/src/policy/deny.rs:916`, `"if files.iter().any(|f| rm.is_match(f))"`.
   - Case: `Search { paths: ["~"], recursive: false, glob: "**/b.session" }`; likewise `**/b.history`.
   - Result: Allow in Home. Neither filename matches the enumerated `0`, `a`, or `x` samples. Expected Ask: these filters can match `.zsh_sessions/b.session` or `.bash_sessions/b.history`. The dev brief’s K8 added scope requires preserving Ask when later components can match an ask-tree file. `dev` asks for these cases; Any and FullAccess should remain Allow.
   - Fix: Match the filename pattern against the relevant filename families, rather than a finite selection of stems.

6. [major] **Azure prefix matching misses supported filenames with other extensions**
   - Where: `crates/atrium-core/src/policy/deny.rs:1212`, `"file_pat.starts_with(pre)"`; `:1217`, `"|| ASK_PREFIX_PROBES"`.
   - Case: `Read { paths: ["~/.a?ure/*token_cache.custom"], recursive: false, glob: null }`.
   - Result: Allow in Home. The pattern matches `.azure/msal_token_cache.custom`, but passes neither the textual-prefix shortcuts nor the five representative filenames. Expected Ask under contract §9’s `msal_token_cache.*` rule. Reading the plain path asks. Any and FullAccess should allow.
   - Fix: Determine whether the filename pattern intersects the protected prefix language, including arbitrary extensions, without requiring a representative filename.

7. [minor] **The ordinary-glob test codifies a forbidden new hard deny**
   - Where: `crates/atrium-core/tests/M1-30-tree-wildcards.rs:536`, `"cat ~/.config/*/settings.json"`, followed by four `"Hard(credential-read)"` expectations; responsible matching is `deny.rs:638`, `"t.matches_tree(tree)"`.
   - Case: `cat ~/.config/*/settings.json`.
   - Result: HEAD hard-denies in Home, prefix-rule Home, Any and FullAccess. `dev` gives Ask / Ask / Allow / Allow. The existing binaries reproduce this difference, and comparison of the source confirms it. The brief explicitly names this exact glob as retaining `dev`’s verdict; comparing it with the different `cat ~/.config/*/*` case does not justify changing the expectation.
   - Fix: Preserve the explicitly required ordinary-glob behavior and restore its baseline expectations.

8. [minor] **Singleton class spellings of readable SSH config become hard-denied**
   - Where: `crates/atrium-core/src/policy/deny.rs:1351`, `"sub[0].is_exact_literal("config")"`.
   - Case: `Read { paths: ["~/.s?h/confi[g]"], recursive: false, glob: null }`.
   - Result: HardDeny (`credential-read`) in every setting. The only matching name inside `.ssh` is `config`, whose read is exempt; the correct result is Allow in Home, Any and FullAccess. This is also a new hard deny compared with `dev`, contrary to the brief’s preservation rule.
   - Fix: Recognize singleton pattern spellings of exempt filenames. Do not exempt broader patterns that can also match secrets.

9. [nit] **The handoff still reports superseded verdicts**
   - Where: `docs/agents/handoffs/M1-30-tree-wildcards.md:34`, the K8 summary places `**/*.toml` in the Allow group and `**/*.json`, `**/*`, `**`, `**/` in the Ask group; `:50` says the ordinary-glob test should pass on `dev`.
   - Case: Non-recursive `Search ["~"]` with those filters, and finding 7’s command.
   - Result: HEAD correctly asks for the TOML filters and hard-denies the JSON/catch-all filters, contrary to that summary. The ordinary-glob baseline claim is also false.
   - Fix: Update the summary and verification instructions to match the final expectations and actual baseline results.

## Suspected, not shown

None.

## Test gaps

- The changed K8 expectations in `d87710d` are correct: `**/*.toml` and `**/*.{rs,toml}` must Ask in Home because `deny.rs:446` lists `".cargo/credentials.toml"`. `**/*.json`, `**/*`, `**`, and normalized `**/` must hard-deny in every setting because they match hard-denied credential probes such as `deny.rs:403`, `".docker/config.json"`. Those original expectations were wrong.
- Recursive `Search ["~"] **/*.rs` and `rg --glob '**/*.rs' x ~` correctly remain Ask under contract §9’s independent home-root recursion rule; `engine.rs:757` explicitly requires `"p != h"`.
- `finding_11`’s existing on-disk cases are already blocked on `dev`. Its fixture creates the targets at `adversarial_gate.rs:24–26`; filesystem expansion reaches them and `gate.rs:327–330` promotes the result to HardDeny. The new test at `M1-30-tree-wildcards.rs:895` appropriately pins absent-on-disk cases, including `"cat ~/.local/share/atr*/missing.db"` at `:904`.
- Add regressions for findings 1–8 and direct ordinary command cases `**/*.rs` and `src/**/mod.rs`. The covered `~/**/*.rs` case retains Ask / Ask / Allow / Allow.
- D-119 scope is respected: all `deny.rs` changes remain within the owned region. The duplicated representative-path mechanisms at `deny.rs:731`, `:827`, and `:1189` contribute to the demonstrated gaps; line count alone is not a separate finding.
- Cargo could not start because its build lock is on a read-only filesystem. The existing task binary passed all 10 tests that do not touch the filesystem; the existing ignored-fuzz binary reproduced finding 2. No fresh build or filesystem-creating gate tests were run, and no files were changed.