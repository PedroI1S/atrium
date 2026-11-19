# Review: M1-30-tree-wildcards
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-17 · round 3

## Verdict
**rework**: earlier runtime findings are fixed, but the replacement matcher introduces concrete verdict gaps, quadratic parsing, and false hard denies.

## Findings

Cases use home/cwd `/h/u`, empty `resolved_paths`, and the task’s virtual environment unless specified. “Any” means Home with a matching global `Any → Allow` rule. New cases below are code-traced, not executed.

1. [major] **Negated Unicode classes can bypass Atrium-folder protection**
   - Where: `crates/atrium-core/src/policy/deny.rs:974`, `"hi: !hi,"`; `:1058`, `"*hi"`.
   - Case: Add `/h/u/átrium` to `atrium_data_paths`. Call `Read { paths: ["/h/u/[!é]trium/secret"], recursive: false, glob: null }`.
   - Result: Allow in Home, Any and FullAccess. `[!é]` can match `á`, so the correct result is HardDeny (`atrium-self`) everywhere, like the plain path. `add_range` marks the excluded `é` as non-ASCII; complementing that approximation then excludes **every** non-ASCII character. This violates brief §1’s same-verdict rule and contract §9’s protection of supplied Atrium data paths.
   - Fix: Preserve a conservative superset when complementing Unicode classes; do not negate the single non-ASCII approximation bit.

2. [major] **The claimed `dev` floor loses embedded-`**` verdicts**
   - Where: `crates/atrium-core/src/policy/deny.rs:1289`, `"let globstar = comps.iter().any(|c| matches!(c, PComp::GlobStar));"`; `:1507`, `"let loose = p.globstar;"`.
   - Case: `cat ~/**ssh/id_rsa`.
   - Result: Ask in Home; Allow in Any and FullAccess. `dev` hard-denies: its `ShellGlob` selects whole-pattern globset matching whenever the spelling contains `"**"`, including inside a component, and matches the `.ssh/id_rsa` probe. HEAD enables that dot-matching behavior only for a component exactly equal to `**`. Expected HardDeny (`credential-read`) everywhere under the brief’s “no line more permissive than dev” rule.
   - Fix: Preserve the old matcher’s semantics for the floor, including embedded `**`; matching the same probe list with different semantics is insufficient.

3. [major] **Session-family shapes omit descendants of ask trees**
   - Where: `crates/atrium-core/src/policy/deny.rs:1432`, `"let mut v = lits(&abs(t));"`, immediately followed at `:1433` by `"v.push((SComp::Glob(format!(\"*{suf}\")), true));"`.
   - Case: `Search { paths: ["~"], recursive: false, glob: "**/foo/b.session" }`.
   - Result: Allow in Home, Any and FullAccess. Expected Ask in Home and Allow in Any/FullAccess: the filter can match `.zsh_sessions/foo/b.session`. `dev` asks. The direct-child suffix shape cannot represent that descendant, while the general tree shape rejects `**` crossing its key. K8 requires preserving Ask when later components can match a file in an ask tree; contract §9 protects the tree’s files.
   - Fix: Permit intervening components between an ask-tree root and its filename-family shape.

4. [major] **The four-literal-character heuristic misses Azure cache searches**
   - Where: `crates/atrium-core/src/policy/deny.rs:1481`, `"&& spells_part_of(atoms, pre)"`; `:2935`, `".windows(4)"`.
   - Case: `Search { paths: ["~"], recursive: false, glob: "**/ms?l_*.custom" }`.
   - Result: Allow in Home, Any and FullAccess. Expected Ask in Home, Allow in Any/FullAccess: it matches `.azure/msal_token_cache.custom`, protected by contract §9’s `msal_token_cache.*` rule. The pattern intersects that family but contains no qualifying four-character literal run. `dev` asks for this search.
   - Fix: Replace the literal-run threshold with matching that recognizes this cache-family intersection while retaining the explicitly required ordinary-glob exceptions.

5. [major] **Class parsing performs quadratic work before the budget**
   - Where: `crates/atrium-core/src/policy/deny.rs:1023`, `"'[' => match class_end(pat, i)"`; `:994`, `"while j < b.len()"`; `:1035`, `"None => out.push(Atom::One(CSet::Lit('['))),"`.
   - Case: The command `"cat ~/src/" + "[".repeat(32768) + ".rs"` in Home, Any or FullAccess.
   - Result: Every unmatched `[` rescans the remaining component. One parse performs over 536 million byte-loop iterations; `is_ssh_dir` and `glob_hit` each parse it. These operations spend no `Budget`. The eventual ordinary verdict is Ask / Allow / Allow, but the new work is Θ(n²), contrary to the bounded-matcher design and brief’s no-slowdown requirement. This is an operation-count proof, not a measured timing claim.
   - Fix: Find class boundaries in one pass, or budget parsing itself and stop rescanning suffixes after an unmatched opener.

6. [minor] **Budget exhaustion fabricates a hard deny on an unrelated glob**
   - Where: `crates/atrium-core/src/policy/deny.rs:1095`, `"let n = (la + 1) * w * 2;"`; `:1476`, `".unwrap_or(true)"`; `:1509`, `"if !budget.take(((n + 1) * w) as u64)"`, followed by `"return true;"`.
   - Case: The command `"cat ~/" + "a".repeat(50000) + "*/notes.txt"`, below the 128 KiB command limit.
   - Result: HardDeny (`credential-read`) in Home, Any and FullAccess. Repeated floor comparisons charge entire component-DP rectangles despite rejecting immediately on the initial `a` versus `.`, exhausting the shared budget. A later floor check then succeeds solely because the budget is empty. The ordinary baseline is Ask / Allow / Allow; D-131 also requires Ask when matching cannot be decided within budget, rather than an invented hard-deny match.
   - Fix: Propagate exhaustion separately from a positive match and return Ask; charge actual work or reject incompatible literal prefixes before charging full rectangles.

7. [minor] **The change crosses an explicitly excluded function boundary**
   - Where: `crates/atrium-core/src/policy/deny.rs:2233`, `".find_map(|v| self.s.glob_str_hit(v, Role::Read))"`, inside `Ctx::search_glob_hit`.
   - Case: `Search { paths: ["~"], recursive: false, glob: ".g*/private-keys-v1.d/k.key" }`, Home/Any/FullAccess.
   - Result: Correctly HardDeny (`credential-read`) now, as required by S5 and contract §9. However, the brief on `dev` explicitly says “Don't touch … `search_glob_hit`” and requires escalation before changing another task’s functions. D-131 records the model/design escalation but does not amend that ownership restriction.
   - Fix: Record the ownership widening and coordinate this caller change with M1-31 before merge. Keep the security fix.

8. [nit] **The handoff still contradicts the Azure tests**
   - Where: `docs/agents/handoffs/M1-30-tree-wildcards.md:32`, `".azure/*.rs"` and `"Search [\"~/.azure\"] *.rs"` are listed as returning `"Allow / Allow / Allow"`.
   - Case: Those two non-recursive Search calls in Home.
   - Result: Both correctly Ask, as asserted at `crates/atrium-core/tests/M1-30-tree-wildcards.rs:837–838`: they can match `msal_token_cache.rs` under contract §9. The inventory is outside the opening notice’s superseded matcher descriptions. Lines 35, 58 and 129 also still describe the now-unignored contained-search regression as ignored or deferred.
   - Fix: Update the current test inventory and verification instructions.

## Suspected, not shown

None.

## Test gaps

- Ran the existing branch binary’s 19 filesystem-free tests successfully, including the regressions for round 1 findings 1 and 3–8 and round 2 findings 1–7. The non-`cd` fuzz test also passed all 30,000 iterations in 26.70 seconds. Add coverage for findings 1–6 above.
- The deferred `cd` case remains unchanged: the existing ignored fuzz test fails at iteration 28 on `timeout 5 cd ~/.a?s; xxd credentials;` with Ask in Home. `words.rs:628`, `"expanded.contains(['*', '?', '['])"`, still loses the destination. It remains M1-28’s responsibility.
- The changed K8 expectations are correct: `**/*.toml` and `**/*.{rs,toml}` Ask because `deny.rs:446` lists `".cargo/credentials.toml"`; `**/*.json`, `**/*`, `**` and `**/` hard-deny because they reach credentials such as `deny.rs:403`, `".docker/config.json"`. The three explicit Azure `.rs` expectations correctly changed to Ask.
- The requested ordinary cases remain covered and pass: `M1-30-tree-wildcards.rs:548`, `"cat **/*.rs"`; `:550`, `"cat src/**/mod.rs"`; `:741`, `"cat ~/**/*.rs"`—Ask in Home and Allow in Any/FullAccess. `Read ~/**/*.rs` remains Allow.
- `finding_11` already passes on `dev` because its fixture creates the targets (`adversarial_gate.rs:24–26`) and filesystem reachability promotes the verdict at `gate.rs:327–330`. This task appropriately pins absent-on-disk behavior at `M1-30-tree-wildcards.rs:914`, `"cat ~/.local/share/atr*/missing.db"`—expected Deny through SessionGate in Home.
- The shared matcher removes substantial duplication; line count alone is not a finding. The explicit ownership crossing is finding 7. No files were changed.