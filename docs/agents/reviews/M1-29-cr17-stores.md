# Review: M1-29-cr17-stores
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-16

## Verdict
**rework**: scoped rules can miss named paths, derived-command coverage has bypasses, and rule validation remains incomplete.

## Findings

Cases below are traced from HEAD; no probe files were written or new tests executed. Unless specified otherwise, rules are global, have no tool-name restriction, and use Home with `home = cwd = /h/u`.

1. [major] **Path filtering drops real operands and fails open**
   - Where: `crates/atrium-core/src/policy/engine.rs:35`, `"w.contains(char::is_whitespace)"`; line 63, `"looks_like_path(&u.word)"`; line 71, `"if is_command_path(a, u)"`.
   - Case: Allow prefix `["cp"]`, path scope `/h/u/work/**`; command `cp /h/u/work/a.txt b.txt`. Separately, with `cwd=/h/u/work`, Deny prefix `["rm"]`, the same scope, command `rm a.txt`.
   - Result: The copy is **Allow**, although its destination `/h/u/b.txt` is outside the approved scope; it should **Ask**. The delete is **Ask**, although `/h/u/work/a.txt` matches the deny scope; it should **Deny**. Bare filenames are discarded before matching. Quoted paths containing spaces are discarded too. This violates brief item 3 and contract §9’s every-path/any-path semantics.
   - Fix: Identify operands using command argument roles; retain bare and whitespace-containing path operands. Do not discard their resolved identities using a spelling heuristic.

2. [major] **One command’s derived unit satisfies another `find` command’s guard**
   - Where: `crates/atrium-core/src/policy/engine.rs:291`, `"let has_derived = !derived_ids.is_empty()"`; line 171, `"&& !has_derived"`. Extraction at `crates/atrium-core/src/policy/shell.rs:1264` requires `Some("find")`, without folding.
   - Case: Existing Allow prefixes `["find"]`, `["/usr/bin/FIND"]`, and `["echo"]`; command `find ./safe -exec echo {} + ; /usr/bin/FIND ./safe -exec rm {} +`. On case-insensitive macOS, `/usr/bin/FIND` names `find`.
   - Result: **Allow**. The first command supplies the covered `echo` derived unit, setting the call-wide boolean. The uppercase executable’s `rm` is never extracted, but its guard now passes. Correct verdict: **Ask**, because no rule covers `rm` (brief item 4). The new daemon guard prevents creating these prefixes, but the engine explicitly supports existing `find` prefix rules.
   - Fix: Track extraction/coverage per owning command, rather than using a call-wide boolean; fail closed for folded executable spellings whose payload was not extracted.

3. [major] **A `git` prefix still covers command-line shell aliases**
   - Where: `crates/atrium-core/src/policy/rule_check.rs:425`, the general fallback is `"false"`; `crates/atrium-core/src/policy/shell.rs:3167–3228` extracts shell/eval payloads, with no Git alias handling.
   - Case: Allow prefix `["git"]`; command `git -c alias.clean='!rm -f victim' clean`, in a repository under `/h/u`.
   - Result: **Allow**. The daemon accepts the prefix, and the engine covers the outer Git argv without checking the shell alias. Correct verdict: **Ask** under brief item 4’s requirement to cover every command the call runs. Contract §9’s documented Git-config limit concerns repository configuration; this payload is supplied directly in the call.
   - Fix: Refuse execution-bearing command-line Git configuration under broad prefix rules, or extract its payload and require separate coverage.

4. [major] **Interpreter option values are mistaken for script names**
   - Where: `crates/atrium-core/src/policy/rule_check.rs:219`, `"operands.push(lw)"`; line 237, `"if operands.len() <= verb"`.
   - Case: Create an Execute Allow `CommandExact` rule with argv `["python3", "-W", "ignore"]`. With that rule and an Allow prefix `["echo"]`, evaluate `echo 'print(1)' | python3 -W ignore` in Home.
   - Result: Creation succeeds and stores the rule; the pipeline is **Allow**. `ignore` is the value of `-W`, not a script; Python executes stdin. Correct results: creation **Invalid**, and the pipeline **Ask** without a valid covering rule. Brief item 5 explicitly refuses interpreters with no script.
   - Fix: Consume option values when identifying scripts, or conservatively refuse exact interpreter rules whose script cannot be established. Apply the correction to the editor’s duplicated check too.

5. [major] **The daemon accepts Execute path rules that bypass command checks**
   - Where: `crates/atrium-core/src/policy/rule_check.rs:339` validates only glob syntax; line 358 permits `PathGlob` scopes independently of `is_path_kind`. `crates/atrium-core/src/policy/engine.rs:240–244` then covers commands using those paths.
   - Case: `CreateRule` with global Allow, `tool_kind=Execute`, matcher `PathGlob("/h/u/work/**")`, and `path_scope="/h/u/work/**"`. Then evaluate `python3 -c pass /h/u/work/input` in Home.
   - Result: Creation succeeds and stores the rule; the command is **Allow**, bypassing interpreter command-rule checks. Correct results: creation **Invalid** and command **Ask**. Contract §9 refuses scopes on non-file tool kinds. The editor cannot create this Execute path rule: `app/src/chat/rule-form.ts:473` coerces non-file kinds to `"edit"`.
   - Fix: Validate matcher/tool-kind combinations and require file kinds for path scopes; prevent invalid stored combinations from covering commands. The editor still calls its own `commandRisk` at `rule-form.ts:463`, so contract §9’s claim that it is “backed by the shared validator” is also inaccurate.

6. [minor] **Malformed rules are stored instead of returning an error**
   - Where: `crates/atrium-core/src/policy/rule_check.rs:340–344` checks only emptiness and `GlobBuilder`; `crates/atrium-daemon/src/commands.rs:628` stores every accepted draft.
   - Case: Create a global Edit Allow `PathGlob("relative/**")` rule. Another accepted example is Execute Allow `CommandExact(["cat", "a\nb"])`.
   - Result: Both return success and are stored. The editor refuses the relative glob at `app/src/chat/rule-form.ts:407` (“Start with / or ~/”) and the newline at line 266. Correct result: **Invalid**, with neither rule stored, as requested for malformed rules.
   - Fix: Add the editor’s structural constraints to the shared validator, including absolute normalized globs and command control-character checks.

7. [minor] **Remembered rules cannot allow hard-linked CR-17 stores**
   - Where: `crates/atrium-core/src/policy/gate.rs:362–366`, an Allow becomes Ask whenever `"reaches_multiply_linked(check, &analysis, &resolved)"`.
   - Case: Use the existing fixture’s hard link from `proj/tf_hard` to `.terraform.d/credentials.tfrc.json` (`crates/atrium-core/tests/M1-29-cr17-stores.rs:226–230`). In Home, add a global Read Allow `Any` rule and call `Read ["proj/tf_hard"]`.
   - Result: **Ask**, even after identity mapping and rule matching produce Allow. Correct result: **Allow** for the remembered rule, under D-082/contract §9’s permission to allow ask-store reads by rule, including their identity handling. This also affects the original store once it has multiple links.
   - Fix: Preserve explicit rule authorization for a positively identified ask-store alias while retaining conservative handling of unknown hard links. `gate.rs` is outside this brief’s owned paths, so this needs a scope escalation.

8. [minor] **The credential-store wording incorrectly promises approval for `.git-credentials`**
   - Where: `docs/contracts/m1.md:153` includes `"~/.git-credentials"` among stores that “are **not** hard-denied” and may be allowed by a remembered rule. `crates/atrium-core/src/policy/deny.rs:401` includes `".git-credentials"` in `CRED_FILES`.
   - Case: `cat ~/.git-credentials`, in Home, including with an Execute Allow `Any` rule.
   - Result: **HardDeny(credential-read)**. That is the correct code verdict under `HARD_DENY_DEFAULTS` (`crates/atrium-protocol/src/policy.rs:415`); the contract’s Ask/approvable promise is wrong. This inconsistency predates the task but remains in its edited §9 paragraph.
   - Fix: Remove `.git-credentials` from the ask-store enumeration and preserve its hard deny.

## Suspected, not shown
None listed.

## Test gaps

- Add the exact cases above, including mixed-case `find` with another derived command, option values without scripts, and invalid-rule persistence checks.
- CR-17 tests lack explicit `..` and glob cases, linked Search calls, and remembered-rule coverage through links.
- `Security.tsx:56` maintains a local `ASK_STORES` copy; its current entries match the core ask lists, including all four additions. Its test checks another hardcoded list, so it cannot detect future core/UI drift.
- No changed-path ownership violation found. Existing verification results supplied by the orchestrator were accepted; no tests were rerun during this read-only review.