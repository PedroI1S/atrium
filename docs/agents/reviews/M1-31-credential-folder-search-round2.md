# Review: M1-31-credential-folder-search
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-17 · round 2

## Verdict
**rework**: source inspection shows missed credential-folder searches, new false hard denies, and substantial work outside the budgets.

Cases below are source-derived, with `home = cwd = /h/u`, empty `resolved_paths`, and no rules unless stated. “Any” means an unscoped Allow rule for the relevant tool kind. Source filenames refer to `crates/atrium-core/src/policy/`.

The original round 1 cases appear fixed; finding 3 is closed by D-133. The browser fixes leave the additional cases below. No stale-cache verdict was shown: `Sensitive` now compares its constructor inputs exactly, and lexer hits refresh identifiers. `cd_effect` is unchanged.

The reported over-budget Allow → Ask change is acceptable under the brief’s explicit K10 fail-closed allowance. N9 changes only the two ordinary commands and their empty-folder fixture; its other cases and assertions remain intact. No additional owned-path or duplication finding was shown.

## Findings

1. [major] **An option terminator before git’s pattern causes a false hard deny**
   - Where: `deny.rs:2302`, `"args.iter().position(|a| a == \"--\")"`; `deny.rs:2304`, `"for p in &args[dd_idx + 1..]"`.
   - Case: `git grep -- .aws proj`, with an ordinary tracked `proj` directory.
   - Result: `HardDeny(credential-read)` in Home, Any(Execute), and FullAccess. Here `.aws` is the pattern; `proj` is the pathspec. The new parser treats both as roots. `dev` gives Ask in Home and Allow under Any/FullAccess, which must be preserved under the brief’s prohibition on new ordinary-line hard denies. The equivalent Git syntax was checked locally: Git accepts `git grep -- .aws crates/atrium-core/Cargo.toml`.
   - Fix: Track whether `-e`/`-f` or a positional pattern has already supplied the pattern before interpreting arguments after `--` as pathspecs.

2. [major] **Globbed browser profiles remain outside the folder matcher**
   - Where: `deny.rs:1515`, `"add(&format!(\"{b}/default/cookies\"));"`; `deny.rs:1530`, `".filter(|c| g.is_match(c))"`.
   - Case: `rg x ~/.mozilla/firefox/abcd.defaul[t]-release`. Also Search `Paths { paths: ["~/.mozilla/firefox/abcd.defaul[t]-release"], recursive: true, glob: None }`.
   - Result: The command asks in Home and allows under Any(Execute)/FullAccess. The Paths call allows in Home, Any(Search), and FullAccess. The literal profile spelling is recognized, but this pattern misses both the literal profile-name heuristic and the fixed `default/cookies` candidates. A profile containing `logins.json` must produce `HardDeny(credential-read)` under S8 and contract §9.
   - Fix: Include the supported Firefox and Chromium profile-name families in glob-root containment matching instead of representing every browser with only `default/cookies`.

3. [major] **Explicit browser subdirectories containing credentials are allowed**
   - Where: `deny.rs:1566`, `"browser_profile_dir(b, r)"`; `deny.rs:4429`, `"second.is_none()"`.
   - Case: `rg x ~/.config/google-chrome/Default/Network/`, where that directory contains `Cookies`. The same case applies to recursive Search Paths rooted there.
   - Result: Allow in Home, Any, and FullAccess. `Sensitive::hit` hard-denies the contained `Cookies` file, but the new containment check rejects the two-component remainder `default/network`. S8 requires `HardDeny(credential-read)`. The handoff acknowledges this residual; D-133 authorizes only the `.config` exception.
   - Fix: Recognize credential-containing browser subdirectories such as profile `Network`, retaining the ordinary `Default/Preferences` behavior. Preserve explicit directory information where needed.

4. [major] **Atrium data ancestors outside home escape S8**
   - Where: `deny.rs:1550`, `"let rest = under(&pl, &self.s.home)?;"`, before the data-ancestor loop; `deny.rs:1518`, `"if let Some(rel) = under(d, home)"`.
   - Case: Set `atrium_data_paths = ["/srv/atrium/db"]`; evaluate `rg x /srv/atrium` or recursive Search Paths rooted at `/srv/atrium`.
   - Result: Ask in Home; Allow under Any and FullAccess. Both literal and glob-root containment restrict data ancestors to home. The search reaches data that contract §9 says is never read; S8’s containing-folder rule requires `HardDeny(atrium-self)`. `/srv/atrium` is neither exempted root, `~` nor `~/.config`.
   - Fix: Check Atrium data ancestry before requiring home containment, and generate absolute data ancestors for glob roots.

5. [major] **Brace output allocation happens before either limit stops it**
   - Where: `words.rs:346`, `"if !w.contains('$')"` skips the shortcut; `words.rs:421`, `"work.push(format!(\"{pre}{alt}{post}\"));"`; the result-count check follows at `words.rs:425`.
   - Case: In any preset, command line constructed as `"cat '" + "a".repeat(60_000) + "{" + &",".repeat(60_000) + "$}'"`.
   - Result: This approximately 120 KB line passes the input limit. The first expansion scans only approximately 120 KB, then attempts to materialize 60,001 words with 60,000-byte prefixes: over 3.6 GB of string contents before checking `MAX_BRACE_RESULTS`. If allocation succeeds, it eventually returns `Deny(too large to check)`; allocation failure can prevent any verdict. N11 requires bounded brace work.
   - Fix: Check the projected work/result count before creating alternatives, and charge output bytes before copying prefixes and suffixes. Reject this case promptly with the existing size verdict.

6. [major] **The new short-option parser copies quadratic suffixes**
   - Where: `deny.rs:2125`, `"for (k, c) in cs.iter().enumerate()"`; `deny.rs:2126`, `"let attached: String = cs[k + 1..].iter().collect();"`.
   - Case: `"rg -" + "i".repeat(100_000) + " x ~/pro[j]"`, in Home, Any(Execute), or FullAccess.
   - Result: It eventually asks in Home and allows under Any/FullAccess, but first constructs suffixes totaling 4,999,950,000 characters. Every `i` is a boolean flag, so none needs an attached value. This work is uncharged and precedes the root matcher’s budget. No elapsed-time claim was measured; the quadratic work follows directly from the loop and conflicts with the task’s bounded-cost requirement.
   - Fix: Construct an attached value only inside branches that consume one, preferably as a borrowed slice.

7. [major] **Disabling ag recursion still creates a recursive hard deny**
   - Where: `deny.rs:2530`, `"\"norecurse\","`; `deny.rs:2534`, `"always_recursive: true"`; recursion updates in `parse_search_opts` are restricted to `check_grep_recursion`.
   - Case: `ag --norecurse x ~/Library`, with ordinary files directly in `Library` and protected material only in its child directories.
   - Result: `HardDeny(credential-read)` in Home, Any(Execute), and FullAccess. The [official ag manual](https://raw.githubusercontent.com/ggreer/the_silver_searcher/master/doc/ag.1.md) defines `--norecurse`/`-n` as disabling directory recursion. This call does not descend into `Keychains` or `Application Support`; `dev`’s Ask/Allow/Allow must remain under the brief’s non-recursive preservation rule.
   - Fix: Track ag’s `--norecurse`/`-n` and `--recurse`/`-r` in argument order, as the grep directory-action fix does.

## Suspected, not shown

- New matcher performance: `"rg x ~/" + "a/".repeat(4_000) + "[a]"` fits `shell_glob_steps`’ budget but makes `glob_root_hit` compile roughly 4,000 component matchers, including repeated literal components (`deny.rs:1527`, `ShellGlob::new` at `deny.rs:832`). Whether this exceeds 50 ms remains unmeasured.

## Test gaps

- `cargo test -p atrium-core --test M1-31-credential-folder-search review1_ -- --nocapture` could not start: opening `target/debug/.cargo-build-lock` failed with “Read-only file system.” No files were modified.
- Add regression cases for the findings above.
- The K10 assertion at `crates/atrium-core/tests/M1-31-credential-folder-search.rs:563` permits 750 ms across three evaluations; it does not establish the brief’s “well under 50 ms” bound.