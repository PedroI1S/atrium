# Review: M1-29-cr17-stores
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-17 · round 4

## Verdict
**rework**: the earlier behavioral reproducers are addressed, but credential-pathspec, path-scope and command-prefix bypasses remain.

Results below are traced from HEAD; no files were written or tests rerun. Unless specified, cases use Home, `home = cwd = /h/u`, and global Execute rules without tool-name restrictions.

## Findings

1. [major] **Literal Git directory pathspecs still expose CR-17 stores**
   - Where: `crates/atrium-core/src/policy/engine.rs:1709`, `"return args[i + 1..].iter().any(|a| git_names_credential(a));"`; line 1936 checks `"joined == *s || joined.ends_with(&format!("/{s}"))"`. At line 1771, exclusion magic is stripped with `"s.strip_prefix(['/', '!', '^', ':'])"`.
   - Case: No rules; cwd `/h/u/repo`; HEAD adds `.gem/credentials`, absent from the working tree. Evaluate `git show HEAD -- .gem`. Equivalent directory cases are `.terraform.d` and `.config/op`. Also evaluate `git show HEAD -- ':!safe.txt'`.
   - Result: **Allow**. The directory pathspec includes the committed credential; the exclusion-only pathspec includes everything except `safe.txt`. Neither passes the literal credential-name check. Both should **Ask**, under brief items 1–2 and contract §9’s CR-17 protection (`docs/contracts/m1.md:153`). Round 3’s wildcard fix handles neither selection.
   - Fix: Account for directory descendants and exclusion semantics when deciding whether Git pathspecs reach credentials.

2. [major] **Compact awk redirections escape path scopes**
   - Where: `crates/atrium-core/src/policy/engine.rs:304`, `"if after_cmd || is_explicit_rel_or_var_path(t) || quoted_path"`; line 566 calls `"push_script_path_tokens(cmd, w, out)"`, then skips the script operand.
   - Case: Existing Allow prefix `["awk"]`, scope `/h/u/work/**`; evaluate `awk 'BEGIN{print 1>"out.txt"}' /h/u/work/input.txt`. Conversely, use a Deny prefix with that scope, cwd `/h/u/work`, and input `/h/u/other/input.txt`.
   - Result: The first is **Allow**, although it writes `/h/u/out.txt` outside the scope; it should **Ask**. The second is **Ask**, although it writes `/h/u/work/out.txt` inside the deny scope; it should **Deny**. The token `1>"out.txt"` supplies no recognized path. This violates brief item 3 and contract §9’s every-path/any-path semantics (`docs/contracts/m1.md:147`).
   - Fix: Parse awk redirection operands, or mark scripts whose paths cannot be established as unresolved and apply the existing fail-closed scope handling.

3. [major] **Lexical fallback bypasses canonical path scopes**
   - Where: `crates/atrium-core/src/policy/engine.rs:912–913`, `"match lexical_paths(w, input.home, &cwds)"` and `"Some(ps) => v.extend(ps)"`. The gate’s script extraction instead uses whitespace tokens at `crates/atrium-core/src/policy/words.rs:484–487`.
   - Case: Existing Allow prefix `["sed"]`, scope `/h/u/work/**`; `/h/u/work/link` is a symlink to `/h/u/out.txt`. Through `SessionGate`, evaluate `sed 'w/h/u/work/link' /h/u/work/input.txt`.
   - Result: **Allow**. The new sed parser extracts `/h/u/work/link`, but the gate extracted `w/h/u/work/link`. The engine therefore falls back to the lexical in-scope spelling without obtaining the symlink’s out-of-scope target. Correct verdict: **Ask**, under brief item 3 and §9’s resolved-path handling (`docs/contracts/m1.md:144`, `147`).
   - Fix: Share script-path extraction with the gate so these operands receive canonical resolutions; otherwise refuse scoped authorization for the fallback.

4. [major] **A Git prefix still covers an unchecked merge program**
   - Where: `crates/atrium-core/src/policy/rule_check.rs:1058`, `"merge-index"` is in `GIT_BUILTINS`; `git_subcommand_runs_code` falls through to `"_ => false"` at line 1170.
   - Case: Allow prefix `["git"]`; cwd `/h/u/repo`, with an unresolved index entry `a.txt` and executable `/h/u/run.sh`. Evaluate `git merge-index /h/u/run.sh a.txt`, with no rule covering that script.
   - Result: **Allow**. Git invokes the supplied merge program, but no derived unit is extracted and the Git guard treats the subcommand as safe. Correct verdict: **Ask**, under brief item 4 and contract §9’s requirement to cover every executed unit (`docs/contracts/m1.md:148`). This is another execution-bearing Git subcommand beyond round 3’s cases.
   - Fix: Include `merge-index` in the Git execution guard and synchronize the editor’s corresponding check.

5. [major] **`pipx run` is mistaken for a pinned package**
   - Where: `crates/atrium-core/src/policy/rule_check.rs:764–767`, `"if !is_option(w)"` immediately sets `"found_pkg = true"`. At line 1433, `"command_allow_risk(rule_prefix, true).is_ok()"` accepts that prefix during evaluation.
   - Case: Create an Allow `CommandPrefix(["pipx","run"])` rule, then evaluate `pipx run cowsay hello`.
   - Result: Creation succeeds and stores the rule; evaluation is **Allow**. `run` is pipx’s subcommand, leaving the package/executable unapproved. Correct results: **Invalid** without storage, and **Ask** without a valid covering rule, under brief items 4–5 and contract §9’s no-package refusal (`docs/contracts/m1.md:148`). The editor has the same parser error at `app/src/chat/rule-form.ts:675–678`.
   - Fix: Parse pipx’s subcommand before locating its package/executable operand, in both validation and runtime coverage.

6. [major] **Awk inline code bypasses the interpreter guard**
   - Where: `crates/atrium-core/src/policy/rule_check.rs:164–175`, `interpreter_spec` omits awk variants and returns `"_ => None"`; `command_prefix_blocked` falls through to `"false"` at line 1438. `shell.rs:3167–3227` extracts shell/eval payloads without awk payloads.
   - Case: Create Allow prefix `["awk"]`; evaluate `awk 'BEGIN{system("touch victim")}'`, with no rule covering `touch`.
   - Result: Creation succeeds and stores the rule; evaluation is **Allow**. The inline program executes an unchecked shell command. Correct results: **Invalid** for the unpinned interpreter prefix and **Ask** at evaluation, under brief items 4–5 and contract §9’s inline-code restrictions (`docs/contracts/m1.md:148`).
   - Fix: Add awk variants to the inline-code guard, with matching editor validation; do not let their prefixes cover unchecked programs.

7. [nit] **The ownership addendum names the wrong editor directory**
   - Where: The authoritative `dev` brief, `docs/agents/briefs/M1-29-cr17-stores.md:57`, names `"app/src/ui/rule-form.ts"` and its test. The changed editor is `app/src/chat/rule-form.ts`; for example, HEAD line 546 declares `"export function commandRisk"`.
   - Case: Round 3 finding 5 requested recorded ownership for `app/src/chat/rule-form.ts` and `app/src/chat/rule-form.test.ts`.
   - Result: The addendum records the intended editor work but names nonexistent `ui/` paths. Preset/verdict: not applicable.
   - Fix: Correct the addendum to name both actual `chat/` paths.

## Suspected, not shown

- `engine.rs:1844` recursively advances one pattern token per call in `globs_intersect`. A quoted Git pathspec containing tens of thousands of `?` characters may exhaust the stack while intersecting `*/credential-name`. This was not exercised.

## Test gaps

- Add the cases above, including daemon non-persistence assertions for rejected runner/interpreter drafts and an actual `SessionGate` symlink test for the sed fallback.
- `Security.tsx:56` (`"export const ASK_STORES"`) remains a duplicated runtime list. Its entries are consistent with core; `security.test.ts:52` now checks that parity against core declarations.
- Supplied verification results were accepted; the identified unrelated Linux/load failures were excluded.