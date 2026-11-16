# Review: M1-18-policy-followup, fix round 5 (146bcaac, 222ba7f9), Claude Sonnet 5.5

> Reviewer: Claude Sonnet 5.5, read-only, cross-provider (D-117: Argon's quota ran out). Author: Gemini 4 Hydrogen `high`. Saved by the orchestrator from the reviewer's report. The orchestrator checked that the 4 new `fix5_*` tests fail on `a702441` and that all 37 `adversarial_m1_18` tests pass.


## Verdict: approve with changes (all the changes are known-limit wording; no code change needed)

I found no Allow on a sensitive path that is not already Allow on dev. Both of Argon's round-5 "same as dev" blockers are closed. There are no new false hard denies on ordinary lines. The remaining items are minor and are not regressions from round 5.

## Blockers
None. Every input I tried that reaches a sensitive path through `**`/`..` or `env -C`/`-S` gets a hard deny (credential-read, privilege-escalation or recursive-delete) in every setting. Each setting was checked (Search: Home, Any, FullAccess; commands: home, prefix, Any, FullAccess). The exceptions that stay at Ask/Allow are listed below as minor and are plain-shell limits that exist without `env`.

## 1. Argon's round-5 findings

**Finding 1 (`**` and `..` meeting only after normalization): closed.**

The move into `normalize_glob_variants` and `apply_dotdot` is sound. For a `..` that lands on `**`, it emits three outcomes. `**` matches 2 or more segments, so `**` stays. `**` matches 1 segment, so `**` is popped. `**` matches 0 segments, so the `..` is applied to what came before `**`, recursively. All three are supersets of the real glob semantics.

I ran about 130 Search globs against the sensitive paths. All hard-denied (the two Ask results marked below are traced in the next paragraph):
- Round-5 evidence spellings, from `~` and `~/proj`, recursive or not:
  - `**/./../.ssh/id_rsa`, `**//../.ssh/id_rsa`, `**/sub/../../.ssh/id_rsa`, `**/x*/../../.ssh/id_rsa`.
  - `.aws/credentials` and `.npmrc` targets with the same shapes.
- `**` at the start, at the end, and consecutive (`**/**/../.ssh/id_rsa`).
- `**` repeated with `..` interleaved (`**/../**/../.ssh/id_rsa`, `*/**/../../.ssh/id_rsa`, `x/y/**/../../../../.ssh/id_rsa`, `**/x/**/../../../.ssh/id_rsa`).
- `**` inside a segment (`a**b`, `.s**h/id_rsa`, `**id_rsa`, `.ssh**`). `**a/../../.ssh/id_rsa` and `a**/../../../.ssh/id_rsa` stay at Ask/Allow/Allow, correctly, because they resolve to `/h/.ssh`, which is outside home. They are not sensitive.
- Braces, classes and escapes around `**` or `..`:
  - `{**,x}`, `{**,**}`, `{**/..,x}/.ssh/id_rsa`, `**/{..,x}/`, `**/{.,..}/../`, `**/.{.,}/..`.
  - `**/[.]./..`, `**/.?/..`, `**/[.][.]/..`.
  - `\*\*/../.ssh/id_rsa` and `*\*/..`.
  - `[*][*]/..` is Allow, correctly: it is a literal directory named `**`.
- Absolute, `~` and `$HOME` anchored forms: `/**/../h/u/.ssh/id_rsa`, `/h/u/proj/**/../../.ssh/id_rsa`, `~/**/../u/.ssh/id_rsa`, `${HOME}/**/../.ssh/id_rsa`, `~/.ssh/**/..`.
- Wildcards after the `..` (`**/../.s*/id_rsa`, `**/../?ssh/id_rsa`, `**/../../.[s]sh/id_rsa`, `**/../../**/id_rsa`).
- Multi-root searches, and the plain forms (`**/.ssh/id_rsa`, `~/**/.ssh/id_rsa`, `**/.ssh/**`).

Nothing the removed `globstar_variants` and `drop_globstar` covered is now missed. The `**` matching 0 segments case is covered by the third branch of `apply_dotdot`. The same case without a `..` (`**/.ssh/id_rsa` from `~`) is still hard-denied, because the glob matcher itself treats `**/` as matching zero directories.

**Finding 2 (`env -C` with a separate `-S` argument): closed.**

I ran about 100 spellings, all CR in all four settings:
- `-C ..`, `--chdir ..`, `--chdir=..`, `-C..`, `--chd ..` and `-C ../..` in front of `-S`.
- Every form of `-S`: `-S x`, `-S'x'`, `-iS`, `-vS`, `--split-string x`, `--split-string='x'`, `--spl`.
- Quoting in the tail: single, double, `$(...)`, `\\`.
- 0 to 13 unknown options before `-C`, the 4-option block doubled, `-u A ... -u F`, a double `-C`, `nice`/`time`/`command`/`env -i` in front, and `;` or `|` after.
- `-S 'sh -c'` followed by `cat $1`, `cat "$@"`, `cat $0` (script argument words).
- `-S 'sh -c' 'cat $HOME/.ssh/id_rsa'` and `-S 'sh -c' 'cat .ssh/id_rsa; rm -rf ~'` (rm gives the recursive-delete hard deny).
- `sudo` gives the privilege-escalation hard deny.

Non-monotone case (needless-ask check): the 4-option block repeated 3 times (15 unknown options) with `-C ..` falls to Ask in all four settings, including Any and FullAccess. 1 and 2 repeats give CR, and 15 plain `-Z` options give CR. That is the unbounded-wrapper fail-closed path. It is stricter than dev (dev Allow in Any and FullAccess), so it is not a leak.

`-C`/`--chdir` placed after `-S` and its tail (for example `-S 'sh -c' -C .. 'cat ..'`) stays at Ask/Ask/Allow/Allow. That is correct: after `-S` the remaining words are arguments of the split command (`sh -c -C ..`), not options of `env`. Another correct no-leak case is `-S 'nice' 'cat .ssh/id_rsa'`, where the quoted tail becomes one word, a program name with a space in it.

**Finding 3 (variant budget): closed.** The split budgets still fail closed past the cap. `{{1..64}{a,b,c,d},extra}.rs` and its `/**/*.rs` form give Ask, Ask, Ask, and `{{1..64}{a,b},extra}/**/*.rs` gives Allow, Allow, Allow (59 ms). I did not re-prove the 256 boundary beyond the worker's tests.

## 2. Round-5 regressions

I found none in what I ran. Allow, Allow, Allow (same as dev):
- `**/*.rs`, `src/**/mod.rs`, `src/**/./mod.rs`, `src//**//mod.rs`, `./**/*.rs`, `**/**/*.rs`, `**/*/*.rs`, `a**b/*.rs`.
- `**/test*/**/*.rs`, `{src,tests}/**/*.{rs,toml}`, `**/.git/**`, `**/.*`, `**/*.toml`, `**/a/b/../c/*.rs`, `**/x*/../*.rs`.
- `**/../proj/*.rs`, `**/../proj/**/*.rs`, `src/**/../*.rs`, `src/*/../**/*.rs`.
- The same `**/*.rs` with two roots.

**Needless ask on `**/../sibling/*.rs`? No.** Ask in Home and Allow in Any and FullAccess is consistent with D-100. With `**` matching 0 segments the glob is literally `../sibling/*.rs`, which leaves the root, and the plain `../sibling/*.rs` and `../**/*.rs` give the same Ask. The same holds for `**/../*.rs`, `./**/../*.rs` and `src/**/../../sibling/*.rs`. It is a pathological spelling, not an ordinary search. The same as a702441. Dev was Allow, so this is the intended D-100 tightening and not a false hard deny.

Home-wide `Search ["~"] **/*.rs` (recursive or not) is Ask in Home, Allow in Any and FullAccess. `*.rs`, `src/**/*.rs` and `proj/**/*.rs` from `~` are Allow. Cause: `ask_search_glob_hit` (deny.rs, the ASK_TREES block around line 700) matches the `**` directory prefix against the `.zsh_sessions` tree. That code is not touched by round 5, so it is the same as a702441, and it is the CR-12 design. It is a needless ask only for a home-wide `**` search; I could not run dev to confirm dev's verdict (dev has no CR-12 tree check and the worker's table shows dev Allow for non-recursive home searches).

## 3. Performance

Machine load was 6 during my runs, so the timings are noisy.
- `cargo test -p atrium-core --test adversarial_fuzz`: 15.7 s (2 passed, 1 ignored). Inside the full run 16.6 s. The worker's runs were 13.9, 14.0 and 19.8 s.
- Policy binary, 44 tests: 10.7 s and 5.8 s on two quiet-ish runs, 6.4 s in the full run, against 3.8 s the worker saw in 2 of 3 runs. The spread follows the machine load; I saw no change attributable to the code.
- `adversarial_m1_18`: 3.2 s.
- Per-evaluate time of worst-case globs from Argon's earlier reviews: `{{1..64}{a,b},extra}/**/*.rs` 59 ms for three evaluations. Brace and `**` combinations with `..`, 7 to 23 ms. `**/x*/` repeated 10 times plus 20 `..`: 161 ms. `**/` repeated 500 times plus 500 `..`: 297 ms. All of these still resolve correctly.

**Minor perf item (not a verdict leak, not caused by round 5):** long wildcard-then-`..` globs are slow. `*/` repeated n times plus `../` repeated n times: n=100 gives 0.4 s, n=200 gives 2.0 s, n=400 gives 2.4 s and falls closed to Ask (those figures are for three evaluations, so roughly a third each). The cause is the wildcard branching in `normalize_glob_variants` (round-3 code, unchanged), with cost roughly quadratic in glob length until the cap trips. A string of about 1 KB makes a single policy check take well under a second. I could not run a702441 or dev to compare, so I mark this "unchanged by round 5". One-line known-limit wording: "Search globs of 400 or more `*/` segments followed by matching `..` segments can take a few seconds to evaluate and fail closed to Ask."

## Remaining gaps (all same as dev, not Allow on a sensitive path unless noted)

1. [minor, same as dev] Plain `sh -c '$0' 'cat .ssh/id_rsa'` run from the home folder, and the same wrapped in `env -C .. -S 'sh -c $0' '...'` or `env -C .. sh -c '$0' '...'`, return Ask/Ask/Allow/Allow. The script is an expansion of `$0`, which the policy cannot resolve. The plain form and the `cd .. && sh -c '$0' ...` form give the same verdicts, so `env` adds nothing. This reaches Allow under Any and FullAccess on a sensitive path, but it is the same as dev, so it is outside this task's rule. Known-limit wording: "`sh -c` scripts that are only a positional-parameter expansion (`$0`, `$1`) are not resolved to the file they run."
2. [nit] Home-wide recursive `**/*.rs` asks in Home (CR-12 tree check, see section 2). Wording: "A `**` search rooted at the home folder asks in Home, because it could reach `.zsh_sessions`."
3. [nit] More than 15 unknown `env` options around `-C ..`/`-S` fall to Ask in every setting (see Finding 2). Wording: "`env` with more than about 24 words before the command fails closed to Ask, even in Any and FullAccess."

## 4. What I ran

- `cargo test -p atrium-core --no-fail-fast`: every test binary ok, nothing failed. `adversarial_m1_18` has 37 passed, `adversarial_fuzz` 2 passed and 1 ignored, `policy` 44, `adversarial_regression` 57, `store` 30. About 2 minutes in total.
- `cargo clippy -p atrium-core --all-targets -- -D warnings`: clean.
- `cargo test -p atrium-core --test adversarial_fuzz` (once) and `--test policy` (three times); timings in section 3.
- A throwaway probe test, `crates/atrium-core/tests/zz_probe.rs`, driven by input files in my scratchpad directory. It printed Home, Any and FullAccess (and the prefix rules setting for command lines) verdicts, about 330 inputs in 10 batches, plus timing. I deleted it. `git status` is clean, and I used the existing `target/` directory only.
- I did not run dev, so every dev verdict above comes from the worker's table and the round-5 review, not from a run of mine; where I say "same as dev" for a plain shell line, that is because the same line without `env` gives the same verdict on this branch.
- I did not read D-114, D-115 or D-117 (only in the main checkout).

Files: `crates/atrium-core/src/policy/deny.rs` (`apply_dotdot` and `normalize_glob_variants`, from about line 1575), `crates/atrium-core/src/policy/shell.rs` (`expand_unknown_env_cmd`, `with_tail`, about lines 3524 to 3680).