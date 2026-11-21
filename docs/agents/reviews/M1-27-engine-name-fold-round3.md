# Review: M1-27-engine-name-fold
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-17 · round 3

## Verdict

**rework**: earlier findings are closed, but new K7 bypasses, wrapper-limit handling and a readable-`env` gap remain.

## Findings

Verdicts below use `home = cwd = /h/u`, ordered **Home / Home + Prefix / Home + Any / FullAccess**. Prefix matches the outer command’s first word. New cases are source-traced, not executed.

1. [blocker] **Resolving an expanding assignment hides the command after it**
   - Where: `crates/atrium-core/src/policy/shell.rs:2368`, `"if lit_only {"` gates assignment rejection at `:2377`, `"first && (text.contains('=') || text == \".\")"`. At `:2511`, `".map(|f| quote(f))"` quotes the reconstructed assignment.
   - Case: `sh -c 'X="$1" sudo id' _ foo`
   - Result: **Ask / Allow / Allow / Allow**. The resolver emits `'X=foo' 'sudo' 'id'`; quoting `X=foo` turns it into the inspected command name, hiding `sudo`. The real shell executes `sudo id` with an environment assignment. Required: **HardDeny `privilege-escalation` everywhere**, as on `dev`, under contract §9. This also violates the brief’s prohibition on becoming more permissive.
   - Fix: reject syntactic leading assignments even when their values contain positional expansions; retain the original script and unresolved marker.

2. [major] **Outer pathname expansion is treated as an exact positional value**
   - Where: `crates/atrium-core/src/policy/shell.rs:2408`, `"pos.iter().any(|v| v.contains(['$', '`']))"` rejects variable/substitution operands but accepts unresolved globs; `:2515`, `"PosResolve::Resolved(text)"`.
   - Case: `sh -c '"$1" id' _ /usr/bin/sud[o]`
   - Result: **Ask / Ask / Allow / Allow**. Inspection reconstructs `'/usr/bin/sud[o]' 'id'`. The outer shell expands the glob before starting `sh`; when it matches `/usr/bin/sudo`, the script executes `sudo id`. Required: **Ask everywhere** when that expansion cannot be established, per K7.
   - Fix: carry outer expansion provenance into resolution, or conservatively mark such operands unresolved.

3. [major] **Run-time replacements remain “exact” behind a wrapper**
   - Where: `crates/atrium-core/src/policy/shell.rs:2446`, `"if k == 0 && placeholder(v)"` checks only the first script word. At `:2423`, the `may_append` guard applies only when `"uses_all"` is true.
   - Cases:

     ```sh
     find /usr/bin -name sudo -exec sh -c 'exec "$1" id' _ {} \;
     xargs -I X sh -c '"$1" id' _ X
     ```

   - Result: both **Ask / Allow / Allow / Allow**. The first reconstructs `exec '{}' id`, although `find` replaces `{}` with the executable path. The second resolves `$1` to literal `X`, although `xargs -I X` replaces it from stdin. Either can execute `sudo id`. Required: **Ask everywhere** under K7’s unresolved fallback.
   - Fix: track replaced operands through wrapper command positions; do not restrict replacement handling to `$@`/`$*` or script word zero.

4. [blocker] **Quoted `getopts` evades positional-use detection**
   - Where: `crates/atrium-core/src/policy/shell.rs:2190`, `".split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')'))"`; `:2194`, `"matches!(*t, \"eval\" | \"source\" | \".\" | \"getopts\")"` compares the still-quoted text.
   - Case: `sh -c '"getopts" x: opt; "$OPTARG" id' _ -x sudo`
   - Result: **Ask / Allow / Allow / Allow**. `"getopts"` is missed, and `$OPTARG` is not recognized as positional use, so the script becomes `Plain`. Actual `getopts` sets `OPTARG=sudo`, then the shell executes `sudo id`. Required: **Ask everywhere** under the D-130 rule for unsupported positional-state use, or the resolved privilege hard deny under contract §9.
   - Fix: recognize quoted and escaped spellings of the state-reading builtins using shell token boundaries.

5. [blocker] **Wrapper exhaustion silently leaves a privilege command unchecked**
   - Where: `crates/atrium-core/src/policy/shell.rs:2801`, `"for _ in 0..12 {"`; `:3007`, `"(a, unknown, choices)"` returns without marking exhaustion unresolved.
   - Case: exactly thirteen `command` wrappers:

     ```sh
     command command command command command command command command command command command command command sudo id
     ```

   - Result: **Ask / Allow / Allow / Allow**. Twelve wrappers are removed; inspection stops at `command sudo id`. The real shell reaches `sudo id`. Required: **HardDeny `privilege-escalation` everywhere**, or at least **Ask everywhere** when inspection reaches its bound, under contract §9 and the brief’s fail-closed rule. This gap also exists on `dev`.
   - Fix: continue within a bounded scan, or propagate an unresolved marker when the wrapper limit is exhausted.

6. [minor] **A K7 unresolved marker can still be discarded**
   - Where: `crates/atrium-core/src/policy/shell.rs:2004`, `"let entry = UnboundedUnknown {"`; `crates/atrium-core/src/policy/deny.rs:232`, `"a.unbounded_unknown_dangerous = unknown_words_could_be_dangerous("` filters that marker.
   - Case: `sh -c 'getopts x: opt' _ -x echo`
   - Result: **Ask / Allow / Allow / Allow**. The resolver explicitly returns `Unresolved(None)` for `getopts` with operands, but none of the invocation’s words passes the dangerous-token filter. Required: **Ask everywhere** under the handoff’s D-130 rule for unresolved positional use. D-133 closes the unreadable-`env` case, not this K7 case.
   - Fix: propagate K7 uncertainty independently of the filtered `env` marker, preserving D-133’s behavior.

7. [minor] **Seventeen flat known `env -S` options still hit a needless bound**
   - Where: `crates/atrium-core/src/policy/shell.rs:3847`, `"if splices < 16 {"`; `:3857`, `"*unknown = true;"`, followed by `"unbounded = true;"`.
   - Case: the single line formed by `"env "` + **17 copies of `"-S '-i' "`** + `"/bin/echo hi"`. Compare with `env -i /bin/echo hi`.
   - Result: long form **Ask / Ask / Ask / Ask**; short form **Ask / Allow / Allow / Allow**. Every split string is the known flag `-i`; there is no nesting or unknown option. Required: short-form parity under K9’s requirement for long readable lines containing known options.
   - Fix: distinguish flat known-option processing from nested split expansion, retaining a total work/output budget.

## Suspected, not shown

None.

## Test gaps

- The branch’s prebuilt task-test binary passes **14/14**. No files were changed.
- Earlier round findings are closed: round 2’s 1–7 have passing regression coverage; finding 8 is closed by D-133; finding 9 is authorized by the widened `dev` brief.
- Add the cases above across all four settings.
- The diff stays within the widened owned ranges; the existing word reader is untouched. The separate positional parser and textual builtin detector still diverge from shell tokenization, as findings 1 and 4 demonstrate.