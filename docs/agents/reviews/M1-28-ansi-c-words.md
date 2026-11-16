# Review: M1-28-ansi-c-words
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-16

## Verdict
**rework**: protected operations still escape inspection through quoting and globbed directory changes, and several follow-ups remain incomplete.

## Findings

Policy results below are source traces with `cwd = home = /h/u` and no resolved paths. “Any” means an unscoped `Any → Allow` rule under Home. Cargo could not run because the read-only filesystem refused `.cargo-build-lock`.

1. [blocker] **`$"…"` retains a spurious dollar sign**
   - Where: `crates/atrium-core/src/policy/shell.rs:915`, `word.push('$');`; the `$` branch handles ANSI-C quotes but has no locale-quote branch.
   - Case: under the C locale, `$"sudo" id`, `cat $".aws/credentials"`, or `cat < $".aws/credentials"`.
   - Result: Home asks; Any and FullAccess allow. The parser sees `$sudo` or `$.aws/credentials`, although Bash produces `sudo` or `.aws/credentials`. All three require HardDeny under contract §9’s privilege/credential protections.
   - Fix: recognize `$"…"` quoting; preserve its literal spelling where translation is disabled and fail closed where translation cannot be determined.

2. [blocker] **Substitution scanners still terminate inside ANSI-C quotes**
   - Where: `crates/atrium-core/src/policy/shell.rs:212`, `while i < chars.len() && chars[i] != '\''`. This loop ignores escaped apostrophes.
   - Case: `echo $(printf $'\')'; $'\x73udo' id)`.
   - Result: Bash runs `sudo id` inside the substitution. The scanner treats the escaped apostrophe as the quote’s end and the following quoted `)` as the substitution’s end. The remaining quoting absorbs the command spelling: Home asks; Any and FullAccess allow. HardDeny is required by contract §9, including inside substitutions.
   - Fix: make the nested scanners understand ANSI-C quote boundaries using the same reader. The handoff identifies this restriction, but it remains an open S7 finding.

3. [major] **Decoder errors neither reliably fail closed nor preserve word identity**
   - Where: `crates/atrium-core/src/policy/words.rs:67`, `*bad_escape = true;`, followed by `pending.clear()` at line 71; `shell.rs:900` merely adds `"malformed escape"` to reasons.
   - Case: `cat $'\xff'`; separately, `cat $'\xff.aws/credentials'`.
   - Result: the first allows under Any and FullAccess—the new test explicitly expects that at `M1-28-ansi-c-words.rs:157`. The second drops the invalid byte and invents `.aws/credentials`, producing HardDeny even though Bash’s filename begins with byte `ff`. Both unreadable words should fail closed to Ask, per brief step 1; the latter also violates the prohibition on new false hard denies.
   - Fix: distinguish an unreadable word from an ordinary expansion, propagate that state through payloads, and prevent discarded bytes from creating a different executable/path spelling.

4. [major] **The duplicated `cd` directory list omits protected destinations**
   - Where: `crates/atrium-core/src/policy/words.rs:1124`, `for rel in SENSITIVE_CD_DIRS`; line 1131 checks only `SENSITIVE_CD_TREES`.
   - Case: `cd .config/google-chr?me/Profile*; cat 'Login Data'`. Also, with `atrium_data_paths = ["/h/u/atrium-state"]`: `cd atrium-st?te/dbs; cat atrium.db`.
   - Result: neither destination is represented in the fixed lists. Home asks; Any and FullAccess allow. The corresponding literal destinations require HardDeny: browser profile credential files and every file beneath supplied Atrium data paths are protected by contract §9.
   - Fix: derive candidates from the policy’s protected roots and actual supplied Atrium paths, including descendant directories. Avoid maintaining a separate partial copy of the protection catalogue.

5. [major] **Synthetic subtree candidates lose directory depth**
   - Where: `crates/atrium-core/src/policy/words.rs:1139`, `push_target(format!("{home_trimmed}/{tree}/x"));`.
   - Case: `cd .ss?/*/*; cat ../../id_rsa`, where the glob can match `.ssh/a/b`.
   - Result: the candidate becomes `.ssh/x`; resolving `../../id_rsa` from it produces `/h/u/id_rsa`, while Bash reads `/h/u/.ssh/id_rsa`. Home asks; Any and FullAccess allow. Added scope “finding 11” requires evaluation under every matching directory, yielding HardDeny here.
   - Fix: retain the matched suffix’s depth and relevant path structure instead of replacing every descendant with one component.

6. [major] **Bash named character classes are silently treated as nonmatches**
   - Where: `crates/atrium-core/src/policy/words.rs:1053`, `GlobBuilder::new(pat)`, ending with `.is_ok_and(|g| g.compile_matcher().is_match(cand))`.
   - Case: `cd .s[[:lower:]]h; cat id_rsa`, which can enter `.ssh`.
   - Result: the pinned globset parser does not interpret Bash’s nested POSIX character-class notation. No `.ssh` candidate is retained; Home asks, and Any/FullAccess allow. Finding 11 requires HardDeny.
   - Fix: support shell bracket syntax or conservatively retain possible sensitive matches when that syntax cannot be interpreted exactly.

7. [minor] **Fish discovery corrupts the final PATH entry**
   - Where: `crates/atrium-daemon/src/lib.rs:112`, `string join : -- $PATH`; line 133 preserves the extracted bytes unchanged.
   - Case: fish’s PATH is `/usr/bin /fake/nvm/bin`, with the fake provider present only in the final directory; call `ProbeProviders { preset: Home }`.
   - Result: `string join` appends a newline. Discovery returns `/usr/bin:/fake/nvm/bin\n`; the existence filter rejects the final entry, so the CLI remains undiscovered. D-122 requires merging that directory.
   - Fix: emit the joined PATH without a newline. The fake fish at `crates/atrium-daemon/tests/M1-28-followups.rs:312` currently prints no newline and therefore misses the defect.

8. [minor] **`OrphanGuard` is armed after the cancellable operation**
   - Where: `crates/atrium-agents/src/claude/adapter.rs:1331`, `let d = gate.check(&for_gate).await;`, precedes `OrphanGuard::arm`. In `crates/atrium-core/src/store/mod.rs:111`, `otx.send(f(w))` executes the queued database job even if its receiver was dropped.
   - Case: a Claude `PreToolUse` for `Read {"file_path":"x"}` under Careful queues its approval insert; provider `result` invokes `clear_requests` while the check awaits the database response; the queued insert then commits.
   - Result: the check is aborted before returning its ticket, so no guard is armed and no withdrawal is queued. A pending row survives instead of being cancelled, leaving the D-122 leak unresolved.
   - Fix: make ticket creation cancellation-safe or retain a cleanup owner that collects and withdraws the result of every started check. Test cancellation while the insert is pending, before `check` returns.

9. [nit] **The two assigned M1-22 test nits remain**
   - Where: `crates/atrium-daemon/tests/binary_adapters.rs:61`, `if socket.exists()`; line 203, `message.contains("failed to start the provider")`.
   - Case: the socket pathname exists before `listen`, followed immediately by `Wire::dev`; separately, `StartThread` produces a permission-denied spawn error instead of the fixture’s expected missing-executable error.
   - Result: the first can attempt connection before readiness; the second accepts either spawn failure. Both are explicitly assigned by D-122 and remain unchanged.
   - Fix: wait for a successful connection and assert the expected missing-executable cause. Widen the original fixture’s owned scope as necessary.

## Suspected, not shown
No additional findings claimed.

## Test gaps
- Add the cases above, including redirections, a faithful fish newline, and a deterministic pending-insert cancellation barrier.
- The isolated `cd` tests do not cover browser profiles, custom Atrium roots, descendant depth, or named character classes. Keeping the combined fuzz test ignored for M1-30 does not cover these gaps.
- No additional defect found in N10: `conn.rs:516` selects on `conn.closed()`, while graceful `finish()` preserves control-lane draining. A never-closing daemon reaching the second marker fails the assertion at `adversarial_socket.rs:500`.
- Batch-error logging, damaged-plus-open export coverage, concurrent withdrawal deadlines, daemon stderr routing, and the M1-21 corrections are present. Changes stay within the widened owned paths.