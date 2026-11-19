# Review: M1-29-cr17-stores
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-17 · round 3

## Verdict
**rework**: earlier concrete reproducers are addressed, but credential-glob, path-scope and command-rule bypasses remain.

Verdicts below are traced from HEAD. No files were written or tests rerun. Unless specified, cases use Home, `home = cwd = /h/u`, and global Execute rules.

## Findings

1. [major] **Quoted Git pathspec globs bypass CR-17 protection**
   - Where: `crates/atrium-core/src/policy/engine.rs:1490`, `"return args[i + 1..].iter().any(|a| git_names_credential(a));"`; line 1588, `"GIT_CRED_NAMES.contains(name)"`; line 1596 compares literal suffixes with `"joined.ends_with(&format!("/{s}"))"`.
   - Case: No rules; `cwd=/h/u/repo`, where HEAD adds a credential file. Evaluate `git show HEAD -- '.gem/cred*'`. Equivalent cases use `.terraform.d/credentials*`, `.config/op/*` or `.yarnrc.*`.
   - Result: **Allow**. Quoting avoids shell-glob reasons, while the credential check compares the Git pattern literally. Git can print the committed secrets. Correct verdict: **Ask**, under brief items 1–2 and contract §9 (`docs/contracts/m1.md:153`). The files need not exist in the working tree.
   - Fix: Check Git pathspec patterns against credential names, or ask when their reach cannot be established.

2. [major] **Valid sed delimiters still defeat path scopes**
   - Where: `crates/atrium-core/src/policy/engine.rs:133–139`, `"trimmed.ends_with(\"/w\")"`, `"trimmed.ends_with(\";w\")"` and `"trimmed.ends_with(\"|w\")"` recognize only selected delimiters; line 749 retains uses only when `"words.contains(&u.word)"`.
   - Case: Existing Allow prefix `["sed"]`, scope `/h/u/work/**`; evaluate `sed 's:a:b:w /h/u/out.txt' /h/u/work/input.txt`. Conversely, use a Deny prefix with that scope and evaluate `sed 's:a:b:w /h/u/work/out.txt' /h/u/other/input.txt`.
   - Result: The first is **Allow**, rather than **Ask**; the second is **Ask**, rather than **Deny**. Both output paths are extracted by `words.rs:483–487` but discarded by the new filter. Colon is a valid substitution delimiter. This violates brief item 3 and contract §9’s every-path/any-path semantics (`docs/contracts/m1.md:147`).
   - Fix: Recognize substitution delimiters generally, or fail closed when script path roles cannot be established.

3. [major] **Git execution payloads still pass prefix rules and draft validation**
   - Where: `crates/atrium-core/src/policy/rule_check.rs:880–885` checks selected options and `"w.starts_with(\"--exec=\")"`, but misses separate `--exec` and `-x`. `shell.rs:3167–3227` extracts shell/eval payloads without Git payload handling.
   - Case: Allow prefix `["git"]`; in a repository, evaluate `git rebase --exec 'touch victim' HEAD~1`, or its `-x 'touch victim'` spelling. An exact `CreateRule` draft containing either complete argv also passes validation and reaches storage at `crates/atrium-daemon/src/commands.rs:641`. `git submodule foreach 'touch victim'` likewise passes the broad prefix when submodules exist.
   - Result: **Allow**, without a rule covering `touch`. Correct runtime verdict: **Ask**, under brief item 4 and contract §9’s derived-command requirement (`docs/contracts/m1.md:148`). The execution-option drafts should return **Invalid**, without storage.
   - Fix: Handle both option spellings and code-taking Git subcommands; refuse unsafe drafts and prevent prefixes from covering unchecked payloads. Synchronize the editor’s duplicate at `app/src/chat/rule-form.ts:382–390`.

4. [major] **Relative stdin paths bypass the interpreter guard**
   - Where: `crates/atrium-core/src/policy/rule_check.rs:198–200`, `"if !raw.starts_with('/') { return false; }"`; line 1092 accepts a pinned prefix through `"command_allow_risk(rule_prefix, true).is_ok()"`.
   - Case: Home preset, command cwd `/dev`; Allow prefixes `["echo"]` and `["python3","./stdin"]`; evaluate `echo 'print(1)' | python3 ./stdin`.
   - Result: **Allow**. The operand resolves to `/dev/stdin`, but the guard checks its relative spelling and treats it as a script. Correct verdict: **Ask**, because the rule authorizes arbitrary piped code without a script, contrary to brief items 4–5 and contract §9 (`docs/contracts/m1.md:148`). Round 2’s absolute normalization cases are fixed; this equivalent relative case remains.
   - Fix: Check the script operand’s resolved paths during evaluation, including the command’s cwd, before accepting command-rule coverage.

5. [minor] **Rule-editor changes lack recorded ownership**
   - Where: `app/src/chat/rule-form.ts:181`, `"function checkPwshRisk"`; `app/src/chat/rule-form.test.ts:316`, `"rules editor: rework round 5 parity with rule_check.rs"`. The brief’s owned-path list names `Security.tsx` and its test (`docs/agents/briefs/M1-29-cr17-stores.md:25`), while line 38 requires `"Stay inside your owned paths"`.
   - Case: The diff changes both `app/src/chat/rule-form.ts` and its existing test. Neither appears in the authoritative `dev` brief’s ownership list; its added scope covers the `gate.rs` change.
   - Result: The changes exceed the recorded scope. Policy preset/verdict: not applicable.
   - Fix: Have the orchestrator explicitly record ownership for these two files.

## Suspected, not shown
None.

## Test gaps

- Add the exact cases above, including daemon refusal and non-persistence assertions for Git execution-option drafts.
- `Security.tsx:56` still duplicates the store list locally. Its entries currently match core; `security.test.ts:52–70` now checks them against core’s declarations. It does not obtain the list from core at runtime.