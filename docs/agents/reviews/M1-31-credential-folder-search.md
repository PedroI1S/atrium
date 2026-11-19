# Review: M1-31-credential-folder-search
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-17 · round 2

## Verdict
**rework**: source inspection shows credential-check bypasses, false hard denies, and work that escapes the new bounds.

Cases below are source-derived, using `home = cwd = /h/u`, empty `resolved_paths`, and no rules unless stated. `Any` means an unscoped Allow rule for the relevant tool kind. `deny.rs` means `crates/atrium-core/src/policy/deny.rs`.

The reported over-budget Allow → Ask change is acceptable under K10’s explicit fail-closed allowance. The brief also explicitly preserves searches rooted at `~` itself; I treated that as an exception despite the checklist’s conflicting wording.

## Findings
1. [major] **Equivalent cache keys can reuse different sensitive-path strings**
   - Where: `deny.rs:492`, `"h == input.home"`; `deny.rs:510`, `"let home = lower(input.home);"`. `Path` equality compares components, while `lower` preserves repeated separators.
   - Case: On one thread, evaluate `ToolSubject::None` with home `/h//u`, then evaluate `Paths { paths: ["/h/u/.aws/credentials"], glob: None, recursive: false }`, kind Read, with home `/h/u`, preset FullAccess. Keep cwd `/h/u` and both Atrium path lists empty throughout.
   - Result: The second call reuses the cached home string `/h//u`; its normalized credential path fails `under`, reaching Allow. A fresh evaluation with `/h/u` gives `HardDeny(credential-read)`, required by contract §9. This can cross sessions sharing a thread.
   - Fix: Normalize paths consistently before constructing both the cache key and `Sensitive`, or compare the exact path representations consumed by the constructor.

2. [major] **Globbed recursive roots miss credential-containing folders**
   - Where: `deny.rs:1529`, `"for p in self.target_paths(i, &er)"`; `deny.rs:1485`, `"CRED_FILES.iter().any(|f| under(f, rest).is_some())"`. Containment treats the root pattern literally.
   - Case: `rg x ~/.aw[s]` or `grep -r x ~/.ss[h]`.
   - Result: Home asks; Home + Any(Execute) and FullAccess allow. The existing glob probes do not represent the `.aws` or `.ssh` folder itself, and the new containment check does not interpret the glob. These roots match `.aws` and `.ssh`, respectively, so S8 requires `HardDeny(credential-read)`.
   - Fix: Match recursive root patterns against credential-containing folders and their relevant ancestors, retaining shell separator and leading-dot rules.

3. [major] **An unauthorized `.config` exception leaves S8 open**
   - Where: `deny.rs:1476`, `"if rest.is_empty() || rest == \".config\" {"`.
   - Case: `rg x ~/.config`; also Search `Paths { paths: ["~/.config"], glob: None, recursive: true }`.
   - Result: Home asks; Any and FullAccess allow. The folder contains hard-denied `.config/gcloud` and `.config/gh/hosts.yml`. The brief’s “What to do” §1 exempts only `~` itself, so the correct result is `HardDeny(credential-read)`.
   - Fix: Remove the `.config` exception and correct the expectations in `crates/atrium-core/tests/M1-31-credential-folder-search.rs:410` and `:438`.

4. [major] **Ordinary browser-profile files receive new hard denies**
   - Where: `deny.rs:1489`, `"under(b, rest).is_some() || under(rest, b).is_some()"`.
   - Case: `rg x ~/.config/google-chrome/Default/Preferences`, with `Preferences` an ordinary regular file.
   - Result: `HardDeny(credential-read)` in every setting. `dev` allows this plain file search in Home. The reverse containment test treats every browser-root descendant as a folder holding credentials, including ordinary files. This violates the brief’s prohibition on new hard denies for ordinary lines.
   - Fix: Apply descendant-folder containment only when the root is known to be a directory; retain the existing credential-file checks for regular files.

5. [major] **Disabling grep recursion does not clear recursive state**
   - Where: `deny.rs:1994`, the `directories` branch only executes `"recursive = true;"`; the short-option equivalent is at `deny.rs:2039`.
   - Case: `grep -r -d skip x ~/.aws`, or `grep -r --directories=skip x ~/.aws`.
   - Result: `HardDeny(credential-read)` everywhere, although the final directory action disables recursion. `dev` gives Ask in Home and Allow under Any/FullAccess. The brief requires non-recursive reads to preserve that verdict.
   - Fix: Update the directory action in argument order, clearing recursion for `skip` and `read`.

6. [major] **Numeric brace estimation can panic**
   - Where: `deny.rs:3739`, `"x.abs_diff(y) / step + 1"`.
   - Case: Search `Paths { paths: ["~/proj"], recursive: true, glob: Some("{9223372036854775807..-9223372036854775808}") }`, under Home, Any(Search), or FullAccess.
   - Result: In checked builds, `abs_diff` is `u64::MAX`, and adding one panics before any verdict. This oversized expansion should fail closed to Ask under K10.
   - Fix: Use checked or saturating arithmetic, cap before addition, and propagate overflow as unbounded.

7. [major] **Nested braces perform quadratic work before the budgets stop them**
   - Where: `deny.rs:3704`, `"brace_sequence_len(&g[open + 1..i])"`; `crates/atrium-core/src/policy/words.rs:374`, `"cs[open + 1..i].iter().collect()"`. The latter’s budget at `words.rs:347` charges only the enclosing word length.
   - Case: Search `Paths` rooted at `~/proj`, recursive true, with glob `"{" .repeat(30_000) + "x" + &"}".repeat(30_000)`, in all three settings.
   - Result: It eventually asks, but first repeatedly scans and copies nested interiors totaling approximately 900 million characters. The 60,001-byte word passes the brace scan counter. Thus the new budget does not bound the actual work required by K10/N11. No elapsed-time claim was measured here.
   - Fix: Charge each interior scan/copy before performing it, and make the estimator avoid rescanning nested interiors.

8. [major] **Repeated git base directories multiply without an aggregate cap**
   - Where: `deny.rs:1547`, `"for prev in &bases"`; `deny.rs:1549`, `"next.push(lexical_resolve(eb, prev, self.home()));"`.
   - Case: `"git " + &"-C '{a,b}' ".repeat(32) + "grep pat"`, under Home, Any(Execute), or FullAccess.
   - Result: Each base argument expands to two candidates, and the new S8 check attempts at least \(2^{32}\) combinations before deciding. Duplicate paths increase the work further. Per-word brace limits do not bound this product.
   - Fix: Deduplicate and cap the aggregate base candidates before materializing each product; propagate exhaustion to Ask in every preset.

9. [major] **Command glob matchers bypass `GlobBudget` entirely**
   - Where: `deny.rs:1841`, `"self.s.glob_hit(p, role)"`; `deny.rs:819`, `"whole: compile(pattern)"`. The budget is created only through the `Paths` glob branch at `deny.rs:288`.
   - Case: Command line `"rg x ~/**/" + &"[a]".repeat(30_000)`.
   - Result: Home asks, while Any(Execute) and FullAccess allow after compiling the roughly 90 KB matcher. There is no budget charge or unbounded signal. Its single whole-pattern compile already exceeds `MAX_GLOB_WORK` under `compile_steps`’ own pricing. K10 requires bounded work and Ask when that budget cannot cover the check.
   - Fix: Budget command-path matcher construction too, and propagate exhaustion rather than treating it as no sensitive match.

10. [minor] **The brace “lower bound” can exceed the old expansion’s actual size**
    - Where: `crates/atrium-core/src/policy/words.rs:1089`, `"bound = bound.saturating_mul(commas + 1);"`.
    - Case: `cat '{,$}{,$}{,$}{,$}{,$}{,$}{,$}{,$}{,$}'`, preset FullAccess.
    - Result: The shortcut computes 512 and returns `None`, producing `Deny(too large to check)`. The old expander produces only 89 results and allows the call. Choosing `$` in one group suppresses expansion of the immediately following group, so the choices are not independent. This is a small input, unrelated to actual budget exhaustion.
    - Fix: Disable this shortcut for words containing `$`, or account conservatively for alternatives that can introduce a parameter-expansion prefix.

## Suspected, not shown
- No additional stale-entry problem shown for normalized cache inputs or the lexer memo. Lexer output depends on the line, and cache hits refresh its identifiers.
- Further matcher timing overruns remain unmeasured.

## Test gaps
- `.orchestration/review-input/review-round1.md` is absent; individual prior findings could not be verified.
- `cargo test -p atrium-core --test M1-31-credential-folder-search` could not start: opening `target/debug/.cargo-build-lock` failed with “Read-only file system.”
- The K10 assertions at `crates/atrium-core/tests/M1-31-credential-folder-search.rs:563` permit 750 ms across three evaluations, leaving the brief’s “well under 50 ms” requirement unverified.
- Add coverage for the cases above, especially cache reuse across equivalent path spellings and bounded hostile command inputs.
- N9’s diff changes only the two authorized ordinary commands and adds their empty-folder fixture; assertions remain unchanged. No owned-path violation was shown against the widened brief.