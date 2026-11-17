# Review: M1-29-cr17-stores
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-17 · round 2

## Verdict

**rework**: path-scope and interpreter-rule bypasses remain, and the round 1 hard-link finding is still unresolved.

Verdicts below are traced from HEAD; no files were written or tests rerun. Unless stated otherwise, cases use Home, `home = cwd = /h/u`, and global Execute rules without a tool-name restriction.

Round 1’s exact Rust cases 1–6 and 8 are addressed. Finding 7 remains open; editor parity from findings 4–5 remains incomplete.

## Findings

1. [major] **Path filtering still drops paths and fails open**
   - Where: `crates/atrium-core/src/policy/engine.rs:333`, `"if (skip_first_positional && !has_reference) || need_script"` skips script operands; line 553, `"if git_file_sub || looks_like_path_word(w)"` drops bare Git pathspecs. Line 640 filters resolutions using `"words.contains(&u.word)"`.
   - Case: Existing Allow prefix `["sed"]`, path scope `/h/u/work/**`; evaluate `sed 'w /h/u/out.txt' /h/u/work/input.txt`. Separately, with `cwd=/h/u/work`, existing Deny prefix `["git"]`, the same scope, evaluate `git status a.txt`.
   - Result: Both are **Allow**. The `sed` output path is discarded, although `words.rs:485` already extracts script tokens. It should **Ask** because it writes outside the approved scope. The Git pathspec is discarded, leaving no scoped paths; that call should **Deny**. Both violate brief item 3 and contract §9 (`docs/contracts/m1.md:147`).
   - Fix: Preserve paths embedded in supported scripts and recognize Git pathspec operands. Fail closed when operand roles cannot be established.

2. [major] **A PowerShell option value can masquerade as a pinned script**
   - Where: `crates/atrium-core/src/policy/rule_check.rs:192`, `".any(|w| !w.starts_with('-') && w.to_ascii_lowercase().ends_with(\".ps1\"))"`; line 932, `"if command_allow_risk(rule_prefix, true).is_ok()"` bypasses checking subsequent arguments.
   - Case: Create Allow prefix `["pwsh","-WorkingDirectory","work.ps1"]`, where `work.ps1` is a directory. Evaluate `pwsh -WorkingDirectory work.ps1 -Command 'Remove-Item victim'`.
   - Result: Creation succeeds and stores the rule; evaluation is **Allow**. The approved prefix names a directory, not a script, and appended inline code executes. Correct results are **Invalid** on creation and **Ask** on evaluation. Brief items 4–5 and contract §9 require rejecting interpreters without a script and preventing prefix rules from covering inline code.
   - Fix: Parse PowerShell option values and identify the actual script operand before treating a prefix as pinned. Apply the same correction to the editor’s duplicated check.

3. [major] **Equivalent stdin paths bypass the interpreter guard**
   - Where: `crates/atrium-core/src/policy/rule_check.rs:116`, `"const STDIN"` contains literal spellings; line 423 checks `"STDIN.contains(&lw.as_str())"`, then line 438 sets `"found_target = true"`.
   - Case: Create Allow prefix `["python3","/dev/./stdin"]`. With an additional Allow prefix `["echo"]`, evaluate `echo 'print(1)' | python3 /dev/./stdin`.
   - Result: Creation succeeds and stores the rule; the pipeline is **Allow**. `/dev/./stdin` resolves to `/dev/stdin`, so this authorizes arbitrary piped code without naming a script. Correct results are **Invalid** and **Ask**, under brief items 4–5 and contract §9.
   - Fix: Normalize script-path spellings before checking stdin aliases, including redundant separators and `.`/`..` components. Use the normalized check during both creation and evaluation.

4. [minor] **Remembered rules still cannot allow hard-linked CR-17 stores**
   - Where: `crates/atrium-core/src/policy/gate.rs:364`, `"reaches_multiply_linked(check, &analysis, &resolved)"`; line 366 unconditionally assigns `"verdict = PolicyVerdict::Ask"`.
   - Case: Use the hard link from `proj/tf_hard` to `.terraform.d/credentials.tfrc.json` created at `crates/atrium-core/tests/M1-29-cr17-stores.rs:226`. In Home, add a Read Allow `Any` rule and call `Read ["proj/tf_hard"]`.
   - Result: **Ask**, despite the rule-authorized engine result being Allow. Correct verdict: **Allow**, under D-082 and contract §9’s remembered-rule and identity-handling promise (`docs/contracts/m1.md:153`). This is round 1 finding 7.
   - Fix: Preserve explicit rule authorization for positively identified ask-store aliases while retaining conservative treatment of unknown hard links. The handoff correctly escalates the required `gate.rs` scope change; escalation does not close the finding.

5. [minor] **The editor still validates against a stale duplicate**
   - Where: `app/src/chat/rule-form.ts:186`, `"else operands.push(lw)"`; line 463 invokes its local `"commandRisk(split.argv, form.matcher)"`. Rust now consumes Python option values at `crates/atrium-core/src/policy/rule_check.rs:459`.
   - Case: In the editor, submit an Allow exact rule for `python3 -W ignore`.
   - Result: The editor accepts it with a warning because it treats `ignore` as a script; `CreateRule` returns **Invalid** because there is no script. Correct behavior is a consistent editor refusal before submission, as required by brief item 5’s matching validation/source-of-truth requirement.
   - Fix: Connect editor validation to the core’s authoritative checks, or synchronize this parser with parity tests pending that wiring. This remains the unresolved UI portion of round 1 finding 4.

6. [minor] **§9 overstates refusal of exact Git configuration rules**
   - Where: `docs/contracts/m1.md:148` says both components refuse allow rules `"(prefix or exact)"` on `"git with command-line config/exec options (-c, --config-env, --exec-path)"`. In `crates/atrium-core/src/policy/rule_check.rs:772`, refusal is only `"(prefix && has_any) || has_dangerous"`.
   - Case: Create an Allow exact rule for `["git","-c","color.ui=false","status"]`, then evaluate `git -c color.ui=false status` in Home.
   - Result: Creation succeeds; evaluation is **Allow**. The edited contract promises **Invalid**. The editor also lacks the new Git-specific refusal and accepts dangerous Git configuration drafts that the daemon rejects.
   - Fix: State the exact-rule safe-configuration exception accurately in §9, and align the editor with the core’s Git refusal logic.

7. [minor] **Malformed thread scopes are accepted and stored**
   - Where: `crates/atrium-core/src/policy/rule_check.rs:814`, `"validate_rule_draft"` never validates `draft.scope`; `crates/atrium-daemon/src/commands.rs:619` copies `"scope: draft.scope"` and line 628 stores it. The editor refuses missing chats at `app/src/chat/rule-form.ts:501`, `"!form.threadId || !ctx.threadExists(form.threadId)"`.
   - Case: Send `CreateRule` with `scope=Thread { thread_id: "" }`, Execute Allow, matcher `CommandPrefix(["cargo","test"])`, and no optional restrictions.
   - Result: The daemon returns success and stores the rule. Correct result: **Invalid**, with no stored rule, matching the editor and the requested malformed-rule guard.
   - Fix: Check that a thread-scoped rule names an existing thread before storing it.

## Suspected, not shown

None listed.

## Test gaps

- Add the cases above, including daemon non-persistence assertions and an actual `SessionGate` remembered-rule test for the hard link. The assertion at `M1-29-cr17-stores.rs:1257` tests only `DefaultPolicy`.
- The `find -ok`/`-okdir` cases at `M1-29-cr17-stores.rs:499` use an unescaped shell `;`. Add valid `\;` spellings so these tests exercise executable find clauses.
- `Security.tsx:56` duplicates the list in local `ASK_STORES`; it does not read it from core. Its current entries match the core ask lists, including all four additions. `security.test.ts:27` checks another hardcoded list, so it cannot detect core/UI drift.
- No changed-path ownership violation found. Supplied verification results were accepted; N9–N11 were excluded.